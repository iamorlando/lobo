#[cfg(test)]
use lobo_events::PriceLevelChangeEvent;
mod simulation;
pub mod stream;

pub mod messages;
#[cfg(feature = "python")]
pub mod python;

use std::{
    collections::HashMap,
    error::Error,
    fmt,
    fs::File,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Timelike, Utc};
use chrono_tz::America::New_York;
use itchy::{Body, EventCode, Message, MessageStream};
use lobo_books::price_time_priority::Book;
#[cfg(feature = "polars")]
use lobo_replay::ReplayRows;
use lobo_replay::{ReplaySource, ReplayStats};
use lobo_storage::{
    DoNotUpdateUserMap, policies::DoNotUpdateHiddenQuantity, price_level::PriceLevelContract,
    price_sorting::PriceSortingPolicy,
};
#[cfg(feature = "polars")]
use polars::{
    frame::row::Row,
    prelude::{AnyValue, DataType, Field, PolarsError, Schema},
};

use crate::itch::messages::{ItchBook, adapt_message as adapt_itch_message};

/// A reusable ITCH source with adapter-owned cutoff comparison state.
///
/// ```no_run
/// use chrono::{DateTime, Utc};
/// use lobo_adapters::itch::ItchReplaySource;
/// use lobo_primitives::CompressedPrice;
/// use lobo_replay::ReplayContext;
/// use lobo_storage::{
///     DoNotUpdateUserMap, policies::DoNotUpdateHiddenQuantity,
///     price_level::DeepPriceLevel, price_sorting::BTreeMapPriceSorting,
/// };
///
/// type Level = DeepPriceLevel<CompressedPrice, DoNotUpdateHiddenQuantity>;
/// type Context = ReplayContext<
///     Level, BTreeMapPriceSorting, DoNotUpdateUserMap, DoNotUpdateHiddenQuantity,
/// >;
/// let source = ItchReplaySource::from_file("session.itch", "AAPL")?;
/// let cutoff = DateTime::parse_from_rfc3339("2026-07-15T09:30:00-04:00")?
///     .with_timezone(&Utc);
/// let mut context = Context::new(Some(cutoff));
/// context.replay_from_sources(&[&source])?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
pub struct ItchReplaySource<S = PathBuf> {
    source_location: S,
    ticker: String,
    stock_locate: u16,
    // None is prepared as u64::MAX so the hot path is always one comparison.
    cutoff_nanoseconds: u64,
}

impl ItchReplaySource<PathBuf> {
    /// Open an ITCH file and resolve the requested ticker's stock-locate code.
    pub fn from_file(
        source_location: impl Into<PathBuf>,
        ticker: impl Into<String>,
    ) -> Result<Self, ItchReplayError> {
        let source_location = source_location.into();
        let ticker = ticker.into().trim().to_owned();
        let stream = MessageStream::from_file(&source_location)?;

        for message in stream {
            let message = message?;
            if let Body::StockDirectory(directory) = &message.body
                && directory
                    .stock
                    .as_str()
                    .trim()
                    .eq_ignore_ascii_case(&ticker)
            {
                return Ok(Self {
                    source_location,
                    ticker,
                    stock_locate: message.stock_locate,
                    cutoff_nanoseconds: u64::MAX,
                });
            }
        }

        Err(ItchReplayError::TickerNotFound(ticker))
    }

    /// Read the ticker directory entries declared before system hours begin.
    pub fn from_file_tickers(
        source_location: impl Into<PathBuf>,
    ) -> Result<HashMap<String, u16>, ItchReplayError> {
        let source_location = source_location.into();
        let stream = MessageStream::from_file(&source_location)?;
        let mut resolved = vec![false; usize::from(u16::MAX) + 1];
        let mut stocks: HashMap<String, u16> = HashMap::new();

        for message in stream {
            let message = message?;
            if matches!(
                &message.body,
                Body::SystemEvent {
                    event: EventCode::StartOfSystemHours
                }
            ) {
                break;
            }
            let Body::StockDirectory(directory) = &message.body else {
                continue;
            };
            let stock_locate = message.stock_locate;
            if resolved[usize::from(stock_locate)] {
                continue;
            }
            resolved[usize::from(stock_locate)] = true;
            stocks.insert(directory.stock.as_str().trim().to_owned(), stock_locate);
        }

        Ok(stocks)
    }

    /// Resolve every listed ticker to a dedicated, single-book source.
    pub fn from_file_all(
        source_location: impl Into<PathBuf>,
    ) -> Result<Vec<Self>, ItchReplayError> {
        let source_location = source_location.into();
        let stream = MessageStream::from_file(&source_location)?;
        let mut sources = Vec::new();
        let mut resolved = vec![false; usize::from(u16::MAX) + 1];

        for message in stream {
            let message = message?;
            if matches!(
                &message.body,
                Body::SystemEvent {
                    event: EventCode::StartOfSystemHours
                }
            ) && !sources.is_empty()
            {
                break;
            }
            let Body::StockDirectory(directory) = &message.body else {
                continue;
            };
            let stock_locate = usize::from(message.stock_locate);
            if resolved[stock_locate] {
                continue;
            }
            resolved[stock_locate] = true;
            sources.push(Self {
                source_location: source_location.clone(),
                ticker: directory.stock.as_str().trim().to_owned(),
                stock_locate: message.stock_locate,
                cutoff_nanoseconds: u64::MAX,
            });
        }

        Ok(sources)
    }
}

impl<S> ItchReplaySource<S> {
    /// Prepare an inclusive cutoff before replay, or clear it with `None`.
    ///
    /// The caller supplies a timezone-aware instant normalized to UTC. Its
    /// date is assumed to be the session date: convert to America/New_York
    /// using that date's EST/EDT offset, then discard the date. ITCH compares
    /// local wall-clock nanoseconds from midnight, not elapsed time across a
    /// daylight-saving transition. No date is inferred from the file name.
    pub fn with_cutoff(mut self, cutoff_time: Option<DateTime<Utc>>) -> Self {
        self.cutoff_nanoseconds = cutoff_time.map_or(u64::MAX, |cutoff| {
            let time = cutoff.with_timezone(&New_York).time();
            u64::from(time.num_seconds_from_midnight()) * 1_000_000_000
                + u64::from(time.nanosecond())
        });
        self
    }
}

impl<S> ItchReplaySource<S>
where
    S: AsRef<Path>,
{
    pub fn path(&self) -> &Path {
        self.source_location.as_ref()
    }

    pub fn ticker(&self) -> &str {
        &self.ticker
    }

    pub fn stock_locate(&self) -> u16 {
        self.stock_locate
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ItchReplayStats {
    pub source_messages: u64,
    pub security_messages: u64,
    pub replay_messages: u64,
    pub replay_events: u64,

    pub adds: u64,
    pub executes: u64,
    pub cancels: u64,
    pub deletes: u64,
    pub replaces: u64,
}

impl ReplayStats<Message> for ItchReplayStats {
    fn merge(&mut self, other: Self) {
        self.source_messages += other.source_messages;
        self.security_messages += other.security_messages;
        self.replay_messages += other.replay_messages;
        self.replay_events += other.replay_events;
        self.adds += other.adds;
        self.executes += other.executes;
        self.cancels += other.cancels;
        self.deletes += other.deletes;
        self.replaces += other.replaces;
    }
    #[inline(always)]
    fn record_source_message(&mut self, _message: &Message) {
        self.source_messages += 1;
    }

    #[inline(always)]
    fn record_security_message(&mut self, _message: &Message) {
        self.security_messages += 1;
    }

    #[inline(always)]
    fn record_replay_message(&mut self, message: &Message) {
        self.replay_messages += 1;
        match &message.body {
            Body::AddOrder(_) => self.adds += 1,
            Body::OrderExecuted { .. } | Body::OrderExecutedWithPrice { .. } => {
                self.executes += 1;
            }
            Body::OrderCancelled { .. } => self.cancels += 1,
            Body::DeleteOrder { .. } => self.deletes += 1,
            Body::ReplaceOrder(_) => self.replaces += 1,
            _ => {}
        }
    }

    #[inline(always)]
    fn record_replay_event(&mut self) {
        self.replay_events += 1;
    }
}

#[derive(Debug)]
pub enum ItchReplayError {
    Itchy(itchy::Error),
    #[cfg(feature = "polars")]
    Polars(PolarsError),
    TickerNotFound(String),
}

impl fmt::Display for ItchReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Itchy(error) => write!(f, "{error}"),
            #[cfg(feature = "polars")]
            Self::Polars(error) => write!(f, "{error}"),
            Self::TickerNotFound(ticker) => {
                write!(f, "ticker {ticker} not found in ITCH stock directory")
            }
        }
    }
}

impl Error for ItchReplayError {}

impl From<itchy::Error> for ItchReplayError {
    fn from(error: itchy::Error) -> Self {
        Self::Itchy(error)
    }
}

#[cfg(feature = "polars")]
impl From<PolarsError> for ItchReplayError {
    fn from(error: PolarsError) -> Self {
        Self::Polars(error)
    }
}

impl<L, Sort, Src, Pub> ReplaySource<L, Sort, DoNotUpdateUserMap, DoNotUpdateHiddenQuantity, Pub>
    for ItchReplaySource<Src>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    Src: AsRef<Path>,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    type Error = ItchReplayError;
    type Message = Message;
    type StreamError = itchy::Error;
    type Stream = MessageStream<File>;
    type Stats = ItchReplayStats;
    type DefaultBookConcreteType =
        Book<L, Sort, DoNotUpdateUserMap, DoNotUpdateHiddenQuantity, Pub>;

    fn with_cutoff(self, cutoff_time: Option<DateTime<Utc>>) -> Self {
        Self::with_cutoff(self, cutoff_time)
    }

    #[inline(always)]
    fn is_at_or_before_cutoff(&self, message: &Message) -> bool {
        message.timestamp <= self.cutoff_nanoseconds
    }

    fn ticker(&self) -> &str {
        &self.ticker
    }

    fn new_book(&self) -> Self::DefaultBookConcreteType {
        Book::new()
    }

    fn get_stream(&self) -> Result<Self::Stream, Self::Error> {
        Ok(MessageStream::from_file(self.path())?)
    }

    #[inline(always)]
    fn is_relevant(&self, message: &Message) -> bool {
        message.stock_locate == self.stock_locate
    }

    #[inline(always)]
    fn include_message(&self, message: &Message) -> bool {
        matches!(
            &message.body,
            Body::AddOrder(_)
                | Body::OrderExecuted { .. }
                | Body::OrderExecutedWithPrice { .. }
                | Body::OrderCancelled { .. }
                | Body::DeleteOrder { .. }
                | Body::ReplaceOrder(_)
        )
    }

    #[inline(always)]
    fn adapt_message(&self, message: Message, book: &mut ItchBook<L, Sort, Pub>) -> bool {
        adapt_itch_message(message, book).is_some()
    }

    fn for_each_source(
        sources: &[&Self],
        context_books: &mut HashMap<String, Self::DefaultBookConcreteType>,
    ) -> Result<(), Self::Error> {
        struct SourceGroup<'a, T> {
            path: PathBuf,
            sources: Vec<&'a T>,
        }

        // Preserve source order across files while coalescing sources which
        // share one physical ITCH stream.
        let mut path_groups = HashMap::<PathBuf, usize>::new();
        let mut groups = Vec::<SourceGroup<'_, Self>>::new();
        for source in sources.iter().copied() {
            let path = source.path().to_path_buf();
            let group_index = *path_groups.entry(path.clone()).or_insert_with(|| {
                groups.push(SourceGroup {
                    path,
                    sources: Vec::new(),
                });
                groups.len() - 1
            });
            groups[group_index].sources.push(source);
        }

        for group in groups {
            let mut dispatch = vec![None; usize::from(u16::MAX) + 1];
            let mut routed_sources = Vec::with_capacity(group.sources.len());

            for source in group.sources {
                let stock_locate = usize::from(source.stock_locate);
                if dispatch[stock_locate].is_some() {
                    continue;
                }
                dispatch[stock_locate] = Some(routed_sources.len());

                let ticker = source.ticker.trim().to_ascii_lowercase();
                let book = context_books
                    .remove(&ticker)
                    .unwrap_or_else(|| source.new_book());
                routed_sources.push((source, ticker, book));
            }

            let replay_result: Result<(), ItchReplayError> = (|| {
                for message in MessageStream::from_file(&group.path)? {
                    let message = message?;
                    let stock_locate = usize::from(message.stock_locate);
                    let Some(source_index) = dispatch[stock_locate] else {
                        continue;
                    };
                    let source = routed_sources[source_index].0;
                    if !<Self as ReplaySource<
                        L,
                        Sort,
                        DoNotUpdateUserMap,
                        DoNotUpdateHiddenQuantity,
                        Pub,
                    >>::include_message(source, &message)
                        || !<Self as ReplaySource<
                            L,
                            Sort,
                            DoNotUpdateUserMap,
                            DoNotUpdateHiddenQuantity,
                            Pub,
                        >>::is_at_or_before_cutoff(source, &message)
                    {
                        continue;
                    }

                    let book = &mut routed_sources[source_index].2;
                    let _ = <Self as ReplaySource<
                        L,
                        Sort,
                        DoNotUpdateUserMap,
                        DoNotUpdateHiddenQuantity,
                        Pub,
                    >>::adapt_message(source, message, book);
                }

                Ok(())
            })();

            context_books.extend(
                routed_sources
                    .into_iter()
                    .map(|(_, ticker, book)| (ticker, book)),
            );
            replay_result?;
        }

        Ok(())
    }

    fn for_each_source_with_stats(
        sources: &[&Self],
        context_books: &mut HashMap<String, Self::DefaultBookConcreteType>,
    ) -> Result<Self::Stats, Self::Error> {
        struct SourceGroup<'a, T> {
            path: PathBuf,
            sources: Vec<&'a T>,
        }

        // Preserve source order across files while coalescing sources which
        // share one physical ITCH stream.
        let mut path_groups = HashMap::<PathBuf, usize>::new();
        let mut groups = Vec::<SourceGroup<'_, Self>>::new();
        for source in sources.iter().copied() {
            let path = source.path().to_path_buf();
            let group_index = *path_groups.entry(path.clone()).or_insert_with(|| {
                groups.push(SourceGroup {
                    path,
                    sources: Vec::new(),
                });
                groups.len() - 1
            });
            groups[group_index].sources.push(source);
        }

        let mut stats = Self::Stats::default();
        for group in groups {
            let mut dispatch = vec![None; usize::from(u16::MAX) + 1];
            let mut routed_sources = Vec::with_capacity(group.sources.len());

            for source in group.sources {
                let stock_locate = usize::from(source.stock_locate);
                if dispatch[stock_locate].is_some() {
                    continue;
                }
                dispatch[stock_locate] = Some(routed_sources.len());

                let ticker = source.ticker.trim().to_ascii_lowercase();
                let book = context_books
                    .remove(&ticker)
                    .unwrap_or_else(|| source.new_book());
                routed_sources.push((source, ticker, book));
            }

            let replay_result: Result<(), ItchReplayError> = (|| {
                for message in MessageStream::from_file(&group.path)? {
                    let message = message?;
                    stats.record_source_message(&message);

                    let stock_locate = usize::from(message.stock_locate);
                    let Some(source_index) = dispatch[stock_locate] else {
                        continue;
                    };
                    let source = routed_sources[source_index].0;
                    if !<Self as ReplaySource<
                        L,
                        Sort,
                        DoNotUpdateUserMap,
                        DoNotUpdateHiddenQuantity,
                        Pub,
                    >>::is_relevant(source, &message)
                    {
                        continue;
                    }
                    stats.record_security_message(&message);

                    if !<Self as ReplaySource<
                        L,
                        Sort,
                        DoNotUpdateUserMap,
                        DoNotUpdateHiddenQuantity,
                        Pub,
                    >>::include_message(source, &message)
                        || !<Self as ReplaySource<
                            L,
                            Sort,
                            DoNotUpdateUserMap,
                            DoNotUpdateHiddenQuantity,
                            Pub,
                        >>::is_at_or_before_cutoff(source, &message)
                    {
                        continue;
                    }
                    stats.record_replay_message(&message);

                    let book = &mut routed_sources[source_index].2;
                    if <Self as ReplaySource<
                        L,
                        Sort,
                        DoNotUpdateUserMap,
                        DoNotUpdateHiddenQuantity,
                        Pub,
                    >>::adapt_message(source, message, book)
                    {
                        stats.record_replay_event();
                    }
                }

                Ok(())
            })();

            context_books.extend(
                routed_sources
                    .into_iter()
                    .map(|(_, ticker, book)| (ticker, book)),
            );
            replay_result?;
        }

        Ok(stats)
    }
}

#[cfg(feature = "polars")]
impl<S> ReplayRows<Message> for ItchReplaySource<S> {
    type RowState = HashMap<u16, String>;

    fn row_schema(&self) -> Schema {
        Schema::from_iter([
            Field::new("message_type".into(), DataType::String),
            Field::new("stock_locate".into(), DataType::UInt32),
            Field::new("tracking_number".into(), DataType::UInt32),
            Field::new("timestamp".into(), DataType::Time),
            Field::new("stock".into(), DataType::String),
            Field::new("operation".into(), DataType::String),
            Field::new("reference".into(), DataType::UInt64),
            Field::new("new_reference".into(), DataType::UInt64),
            Field::new("side".into(), DataType::String),
            Field::new("shares".into(), DataType::UInt32),
            Field::new("price".into(), DataType::UInt32),
            Field::new("execution_price".into(), DataType::UInt32),
            Field::new("body".into(), DataType::String),
        ])
    }

    fn row_data(&self, message: &Message, tickers: &mut Self::RowState) -> Row<'static> {
        let announced_ticker = match &message.body {
            Body::StockDirectory(directory) => Some(directory.stock.as_str()),
            Body::AddOrder(add) => Some(add.stock.as_str()),
            _ => None,
        };
        if let Some(ticker) = announced_ticker {
            tickers.insert(message.stock_locate, ticker.trim().to_owned());
        }

        let (operation, reference, new_reference, side, shares, price, execution_price) =
            match &message.body {
                Body::AddOrder(add) => (
                    Some("add"),
                    Some(add.reference),
                    None,
                    Some(match add.side {
                        itchy::Side::Buy => "buy",
                        itchy::Side::Sell => "sell",
                    }),
                    Some(add.shares),
                    Some(add.price.raw()),
                    None,
                ),
                Body::OrderExecuted {
                    reference,
                    executed,
                    ..
                } => (
                    Some("execute"),
                    Some(*reference),
                    None,
                    None,
                    Some(*executed),
                    None,
                    None,
                ),
                Body::OrderExecutedWithPrice {
                    reference,
                    executed,
                    price,
                    ..
                } => (
                    Some("execute"),
                    Some(*reference),
                    None,
                    None,
                    Some(*executed),
                    None,
                    Some(price.raw()),
                ),
                Body::OrderCancelled {
                    reference,
                    cancelled,
                } => (
                    Some("cancel"),
                    Some(*reference),
                    None,
                    None,
                    Some(*cancelled),
                    None,
                    None,
                ),
                Body::DeleteOrder { reference } => (
                    Some("delete"),
                    Some(*reference),
                    None,
                    None,
                    None,
                    None,
                    None,
                ),
                Body::ReplaceOrder(replace) => (
                    Some("replace"),
                    Some(replace.old_reference),
                    Some(replace.new_reference),
                    None,
                    Some(replace.shares),
                    Some(replace.price.raw()),
                    None,
                ),
                _ => (None, None, None, None, None, None, None),
            };

        Row::new(vec![
            AnyValue::StringOwned(char::from(message.tag).to_string().into()),
            AnyValue::UInt32(u32::from(message.stock_locate)),
            AnyValue::UInt32(u32::from(message.tracking_number)),
            AnyValue::Time(message.timestamp as i64),
            optional_string(tickers.get(&message.stock_locate).map(String::as_str)),
            optional_string(operation),
            optional_u64(reference),
            optional_u64(new_reference),
            optional_string(side),
            optional_u32(shares),
            optional_u32(price),
            optional_u32(execution_price),
            AnyValue::StringOwned(format!("{:?}", message.body).into()),
        ])
    }
}

#[cfg(feature = "polars")]
fn optional_string(value: Option<&str>) -> AnyValue<'static> {
    value.map_or(AnyValue::Null, |value| AnyValue::StringOwned(value.into()))
}

#[cfg(feature = "polars")]
fn optional_u32(value: Option<u32>) -> AnyValue<'static> {
    value.map_or(AnyValue::Null, AnyValue::UInt32)
}

#[cfg(feature = "polars")]
fn optional_u64(value: Option<u64>) -> AnyValue<'static> {
    value.map_or(AnyValue::Null, AnyValue::UInt64)
}

#[cfg(test)]
mod tests {
    use std::{fs, process};

    use lobo_primitives::{CompressedPrice, PriceType};
    use lobo_replay::ReplayContext;
    use lobo_storage::{
        DoNotUpdateUserMap, policies::DoNotUpdateHiddenQuantity, price_level::DeepPriceLevel,
        price_sorting::BTreeMapPriceSorting,
    };

    use super::*;

    type TestLevel = DeepPriceLevel<CompressedPrice, DoNotUpdateHiddenQuantity>;
    type TestContext = ReplayContext<
        TestLevel,
        BTreeMapPriceSorting,
        DoNotUpdateUserMap,
        DoNotUpdateHiddenQuantity,
    >;

    struct TestFile(PathBuf);

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn append_add_message(
        output: &mut Vec<u8>,
        stock_locate: u16,
        tracking_number: u16,
        timestamp: u64,
        reference: u64,
        side: u8,
        shares: u32,
        stock: &[u8],
        price: u32,
    ) {
        let mut payload = Vec::with_capacity(36);
        payload.push(b'A');
        payload.extend_from_slice(&stock_locate.to_be_bytes());
        payload.extend_from_slice(&tracking_number.to_be_bytes());
        payload.extend_from_slice(&timestamp.to_be_bytes()[2..]);
        payload.extend_from_slice(&reference.to_be_bytes());
        payload.push(side);
        payload.extend_from_slice(&shares.to_be_bytes());

        let mut padded_stock = [b' '; 8];
        padded_stock[..stock.len()].copy_from_slice(stock);
        payload.extend_from_slice(&padded_stock);
        payload.extend_from_slice(&price.to_be_bytes());

        output.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        output.extend_from_slice(&payload);
    }

    fn two_ticker_file() -> TestFile {
        let mut bytes = Vec::new();
        append_add_message(&mut bytes, 1, 1, 1, 11, b'B', 100, b"AAPL", 1_010_000);
        append_add_message(&mut bytes, 2, 2, 2, 22, b'S', 75, b"MSFT", 2_020_000);
        write_test_file(bytes)
    }

    fn write_test_file(bytes: Vec<u8>) -> TestFile {
        let unique = lobo_primitives::uuid::Uuid::new_v4();
        let path = std::env::temp_dir().join(format!(
            "lobo-itch-no-stats-{}-{unique}.bin",
            process::id()
        ));
        fs::write(&path, bytes).expect("test ITCH file should be writable");
        TestFile(path)
    }

    fn sources(path: &Path) -> (ItchReplaySource<PathBuf>, ItchReplaySource<PathBuf>) {
        (
            ItchReplaySource {
                source_location: path.to_path_buf(),
                ticker: "AAPL".to_owned(),
                stock_locate: 1,
                cutoff_nanoseconds: u64::MAX,
            },
            ItchReplaySource {
                source_location: path.to_path_buf(),
                ticker: "MSFT".to_owned(),
                stock_locate: 2,
                cutoff_nanoseconds: u64::MAX,
            },
        )
    }

    fn assert_replayed_books(context: &TestContext) {
        let aapl = context.get("AAPL").expect("AAPL book should exist");
        assert_eq!(
            aapl.order_storage
                .bids
                .best()
                .map(|(price, _)| price.into_u128()),
            Some(1_010_000)
        );
        assert_eq!(aapl.order_storage.bids.visible_quantity, 100);

        let msft = context.get("MSFT").expect("MSFT book should exist");
        assert_eq!(
            msft.order_storage
                .asks
                .best()
                .map(|(price, _)| price.into_u128()),
            Some(2_020_000)
        );
        assert_eq!(msft.order_storage.asks.visible_quantity, 75);
    }

    #[test]
    fn no_stats_multi_source_replay_matches_the_stats_path() {
        let file = two_ticker_file();
        let (aapl, msft) = sources(&file.0);

        let mut no_stats = TestContext::new(None);
        no_stats
            .replay_from_sources(&[&aapl, &msft])
            .expect("no-stat replay should succeed");
        assert_replayed_books(&no_stats);

        let mut with_stats = TestContext::new(None);
        let stats = with_stats
            .replay_from_sources_with_stats(&[&aapl, &msft])
            .expect("stats replay should succeed");
        assert_replayed_books(&with_stats);
        assert_eq!(stats.source_messages, 2);
        assert_eq!(stats.security_messages, 2);
        assert_eq!(stats.replay_messages, 2);
        assert_eq!(stats.replay_events, 2);
        assert_eq!(stats.adds, 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_replay_route_connects_all_sinks_and_stamps_each_book() {
        use lobo_context::{AsyncSink, MpscSinks};
        use lobo_events::Receiver;
        use std::{sync::Arc, time::Duration};
        use tokio::{sync::mpsc, time::timeout};

        let mut bytes = Vec::new();
        append_add_message(&mut bytes, 1, 1, 1, 11, b'B', 100, b"AAPL", 1_010_000);
        append_add_message(&mut bytes, 2, 2, 2, 22, b'S', 75, b"MSFT", 2_020_000);
        append_add_message(&mut bytes, 1, 3, 3, 33, b'B', 50, b"AAPL", 1_010_000);
        append_add_message(&mut bytes, 2, 4, 4, 44, b'S', 25, b"MSFT", 2_020_000);
        let file = write_test_file(bytes);
        let (aapl, msft) = sources(&file.0);
        for mode in 0..4 {
            let (first_tx, mut first_rx) = mpsc::unbounded_channel();
            let (second_tx, mut second_rx) = mpsc::unbounded_channel();
            let sinks = MpscSinks::new()
                .with(AsyncSink::new(
                    move |mut rx: Receiver<PriceLevelChangeEvent<CompressedPrice>>| async move {
                        while let Some(event) = rx.recv().await {
                            first_tx.send(event)?;
                        }
                        Ok(())
                    },
                ))
                .with(AsyncSink::new(
                    move |mut rx: Receiver<PriceLevelChangeEvent<CompressedPrice>>| async move {
                        while let Some(event) = rx.recv().await {
                            second_tx.send(event)?;
                        }
                        Ok(())
                    },
                ));
            let mut context = TestContext::with_sinks(None, sinks).unwrap();
            match mode {
                0 => {
                    context.replay_from(&aapl).unwrap();
                    context.replay_from(&msft).unwrap();
                }
                1 => context.replay_from_sources(&[&aapl, &msft]).unwrap(),
                2 => {
                    context
                        .replay_from_sources_with_stats(&[&aapl, &msft])
                        .unwrap();
                }
                _ => context
                    .parallel_replay_from_sources(&[&aapl, &msft], 2)
                    .unwrap(),
            }
            let mut first_events = HashMap::new();
            let mut second_events = HashMap::new();
            for (receiver, events) in [
                (&mut first_rx, &mut first_events),
                (&mut second_rx, &mut second_events),
            ] {
                let mut sequences = HashMap::new();
                for _ in 0..4 {
                    // Each book is ordered; independent parallel books can interleave.
                    // Receive before finish: consumers are already running.
                    let event = timeout(Duration::from_secs(5), receiver.recv())
                        .await
                        .unwrap()
                        .unwrap();
                    assert!(
                        context
                            .books
                            .values()
                            .any(|book| book.book_id() == event.book_id())
                    );
                    let sequence = sequences.entry(event.book_id().to_owned()).or_insert(0);
                    *sequence += 1;
                    assert_eq!(event.sequence_number(), *sequence);
                    assert_eq!(event.event().number_of_orders() as u64, *sequence);
                    events.insert((event.book_id().to_owned(), *sequence), event);
                }
                assert_eq!(sequences.len(), 2);
            }
            for (key, first) in first_events {
                assert!(Arc::ptr_eq(&first, &second_events[&key]));
            }
            let books = timeout(Duration::from_secs(5), context.finish())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(books.len(), 2);
            assert!(first_rx.recv().await.is_none());
            assert!(second_rx.recv().await.is_none());
        }
    }

    fn cutoff(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn cutoff_conversion_uses_session_date_timezone_and_nanosecond_precision() {
        const HOUR: u64 = 3_600_000_000_000;
        const SECOND: u64 = 1_000_000_000;
        let (source, _) = sources(Path::new("unused"));
        for (value, expected) in [
            ("2026-01-15T14:30:00Z", 9 * HOUR + 1_800 * SECOND),
            ("2026-07-15T13:30:00Z", 9 * HOUR + 1_800 * SECOND),
            ("2026-07-15T09:30:00-04:00", 9 * HOUR + 1_800 * SECOND),
            ("2026-07-15T19:00:00+05:30", 9 * HOUR + 1_800 * SECOND),
            ("2026-01-15T00:00:00-05:00", 0),
            ("2026-07-15T23:59:59.999999999-04:00", 86_400 * SECOND - 1),
            ("2026-07-16T00:00:00Z", 20 * HOUR),
            ("2026-01-16T00:00:00Z", 19 * HOUR),
            (
                "2026-07-15T13:30:00.123456789Z",
                9 * HOUR + 1_800 * SECOND + 123_456_789,
            ),
            ("2026-03-08T06:59:59.999999999Z", 2 * HOUR - 1),
            ("2026-03-08T07:00:00Z", 3 * HOUR),
            ("2026-11-01T05:30:00Z", HOUR + 1_800 * SECOND),
            ("2026-11-01T06:30:00Z", HOUR + 1_800 * SECOND),
            ("2026-11-01T07:00:00Z", 2 * HOUR),
        ] {
            let prepared = source.clone().with_cutoff(Some(cutoff(value)));
            assert_eq!(prepared.cutoff_nanoseconds, expected, "{value}");
            assert!(prepared.cutoff_nanoseconds < 86_400 * SECOND);
        }
    }

    fn passes_cutoff(source: &ItchReplaySource, timestamp: u64) -> bool {
        let message = Message {
            tag: b'S',
            stock_locate: 0,
            tracking_number: 0,
            timestamp,
            body: Body::SystemEvent {
                event: EventCode::StartOfSystemHours,
            },
        };
        <ItchReplaySource as ReplaySource<
            TestLevel,
            BTreeMapPriceSorting,
            DoNotUpdateUserMap,
            DoNotUpdateHiddenQuantity,
        >>::is_at_or_before_cutoff(source, &message)
    }

    #[test]
    fn cutoff_is_inclusive_and_none_clears_the_comparison() {
        let (source, _) = sources(Path::new("unused"));
        assert!(passes_cutoff(&source, u64::MAX));
        let prepared = source.with_cutoff(Some(cutoff("2026-07-15T00:00:00.000000001-04:00")));
        assert!(passes_cutoff(&prepared, 0));
        assert!(passes_cutoff(&prepared, 1));
        assert!(!passes_cutoff(&prepared, 2));
        assert!(passes_cutoff(&prepared.with_cutoff(None), u64::MAX));
    }

    #[test]
    fn every_replay_path_builds_the_same_inclusive_cutoff_state() {
        let cutoff_time = cutoff("2026-07-15T13:30:00.123456789Z");
        let timestamp = 34_200_123_456_789;
        let mut bytes = Vec::new();
        append_add_message(
            &mut bytes,
            1,
            1,
            timestamp - 1,
            11,
            b'B',
            100,
            b"AAPL",
            1_010_000,
        );
        append_add_message(
            &mut bytes, 2, 2, timestamp, 22, b'S', 75, b"MSFT", 2_020_000,
        );
        append_add_message(
            &mut bytes,
            1,
            3,
            timestamp + 1,
            33,
            b'B',
            25,
            b"AAPL",
            1_010_000,
        );
        // Continue scanning even if one message is past the cutoff.
        append_add_message(
            &mut bytes, 1, 4, timestamp, 44, b'B', 20, b"AAPL", 1_010_000,
        );
        let file = write_test_file(bytes);
        let (aapl, msft) = sources(&file.0);

        for path in 0..5 {
            let mut context = TestContext::with_capacity_and_cutoff(2, Some(cutoff_time));
            let stats = match path {
                0 => {
                    let mut stats = context.replay_from(&aapl).unwrap();
                    stats.merge(context.replay_from(&msft).unwrap());
                    Some(stats)
                }
                1 => {
                    context.replay_from_sources(&[&aapl, &msft]).unwrap();
                    None
                }
                2 => Some(
                    context
                        .replay_from_sources_with_stats(&[&aapl, &msft])
                        .unwrap(),
                ),
                3 => {
                    context
                        .parallel_replay_from_sources(&[&aapl, &msft], 2)
                        .unwrap();
                    None
                }
                _ => {
                    for source in [&aapl, &msft] {
                        let prepared = source.clone().with_cutoff(Some(cutoff_time));
                        let mut book = Book::<
                            TestLevel,
                            BTreeMapPriceSorting,
                            DoNotUpdateUserMap,
                            DoNotUpdateHiddenQuantity,
                        >::new();
                        prepared.for_each_event(&mut book).unwrap();
                        context
                            .books
                            .insert(source.ticker().to_ascii_lowercase(), book);
                    }
                    None
                }
            };
            assert_eq!(
                context
                    .get("AAPL")
                    .unwrap()
                    .order_storage
                    .bids
                    .visible_quantity,
                120
            );
            assert_eq!(
                context
                    .get("MSFT")
                    .unwrap()
                    .order_storage
                    .asks
                    .visible_quantity,
                75
            );
            if let Some(stats) = stats {
                assert_eq!(stats.source_messages, if path == 0 { 8 } else { 4 });
                assert_eq!(stats.security_messages, 4);
                assert_eq!(stats.replay_messages, 3);
                assert_eq!(stats.replay_events, 3);
                assert_eq!(stats.adds, 3);
            }
        }

        // Reuse a source prepared with another cutoff; the context's None wins.
        let aapl = aapl.with_cutoff(Some(cutoff_time));
        let mut full = TestContext::default();
        full.replay_from_sources(&[&aapl, &msft]).unwrap();
        assert_eq!(
            full.get("AAPL")
                .unwrap()
                .order_storage
                .bids
                .visible_quantity,
            145
        );

        let mut midnight = TestContext::new(Some(cutoff("2026-07-15T00:00:00-04:00")));
        midnight.replay_from_sources(&[&aapl, &msft]).unwrap();
        assert_eq!(
            midnight
                .get("AAPL")
                .unwrap()
                .order_storage
                .bids
                .visible_quantity,
            0
        );
        assert_eq!(
            midnight
                .get("MSFT")
                .unwrap()
                .order_storage
                .asks
                .visible_quantity,
            0
        );
    }
}
