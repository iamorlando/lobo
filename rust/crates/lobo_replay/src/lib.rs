//! Replay books with statically selected message publishers.
//!
//! `ReplayContext::<L>::new(None)` selects `NullSink` behavior: no channels,
//! tasks, sequence stamping, or sending in the optimized mutation loop.
//! Connect destinations before replay using `ReplayContext::with_sinks`.
//! `MpscSinks::new().with(sink_a).with(sink_b)` gives each destination its own
//! unbounded Tokio receiver. `AsyncSink` consumes messages asynchronously;
//! `BatchSink` drives an Arrow batcher and Feather/GPU destination on a blocking
//! worker, writing each completed batch immediately.
//!
//! Run synchronous replay on a blocking worker when using a Tokio runtime with
//! one worker thread, so async consumers can run while messages are produced.
//! After replay, `context.finish().await` closes the book publishers, drains
//! consumers, flushes partial batches, and returns the books with null publishers.

pub mod custom;
#[cfg(feature = "order-api")]
pub mod order_messages;
pub mod feed;
mod feed_books;
pub mod simulation;

#[doc(hidden)]
pub use lobo_storage as __storage;

use lobo_books::price_time_priority::Book;
use lobo_context::{Context, Listens, Sink, SinkError, SinkTasks};
use lobo_events::{BookEvent, NullPublisher, PriceLevelChangeEvent};
use lobo_primitives::time::{DateTime, Utc};
use lobo_storage::{
    UpdateUserMap, UserMapUpdatePolicy,
    policies::{HiddenQuantityPolicy, UpdateHiddenQuantity},
    price_level::PriceLevelContract,
    price_sorting::{BTreeMapPriceSorting, PriceSortingPolicy},
};
#[cfg(feature = "polars")]
use polars::{
    frame::row::Row,
    prelude::{DataFrame, IntoLazy, LazyFrame, PolarsError, Schema},
};
#[cfg(feature = "concurrent")]
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

impl<L, Sort, U, H, Pub> ReplayContext<L, Sort, U, H, Pub>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    /// Create an empty replay context with an inclusive, timezone-aware cutoff.
    ///
    /// Normalize other timezones to UTC before passing the cutoff. Sources own
    /// its interpretation and prepare their comparison state before streaming.
    pub fn new(cutoff_time: Option<DateTime<Utc>>) -> Self {
        Self {
            books: HashMap::new(),
            dummy_time: DateTime::<Utc>::MIN_UTC,
            cutoff_time,
            publisher: Pub::default(),
            sink_tasks: SinkTasks::default(),
        }
    }

    pub fn with_publisher(publisher: Pub) -> Self {
        Self {
            publisher,
            ..Self::new(None)
        }
    }

    #[cfg(feature = "concurrent")]
    pub fn parallel_replay_from_sources<S>(
        &mut self,
        sources: &[&S],
        scanner_count: usize,
    ) -> Result<(), S::Error>
    where
        S: ReplaySource<L, Sort, U, H, Pub, DefaultBookConcreteType = Book<L, Sort, U, H, Pub>>
            + Clone
            + Sync,
        S::DefaultBookConcreteType: Send,
        S::Error: Send,
    {
        let prepared = self.prepare_sources(sources);
        let sources = prepared.iter().collect::<Vec<_>>();
        S::parallel_for_each_source(&sources, &mut self.books, scanner_count)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_capacity_and_cutoff(capacity, None)
    }

    pub fn with_capacity_and_cutoff(capacity: usize, cutoff_time: Option<DateTime<Utc>>) -> Self {
        Self {
            books: HashMap::with_capacity(capacity),
            dummy_time: DateTime::<Utc>::MIN_UTC,
            cutoff_time,
            publisher: Pub::default(),
            sink_tasks: SinkTasks::default(),
        }
    }

    pub fn replay_from<S>(&mut self, source: &S) -> Result<S::Stats, S::Error>
    where
        S: ReplaySource<L, Sort, U, H, Pub, DefaultBookConcreteType = Book<L, Sort, U, H, Pub>>
            + Clone,
    {
        let source = source.clone().with_cutoff(self.cutoff_time);
        let ticker = source.ticker().trim().to_ascii_lowercase();
        let book = self
            .books
            .entry(ticker)
            .or_insert_with(|| source.new_book());
        book.set_publisher(self.publisher.clone());
        source.for_each_event_with_stats(book)
    }

    pub fn replay_from_sources<S>(&mut self, sources: &[&S]) -> Result<(), S::Error>
    where
        S: ReplaySource<L, Sort, U, H, Pub, DefaultBookConcreteType = Book<L, Sort, U, H, Pub>>
            + Clone,
    {
        let prepared = self.prepare_sources(sources);
        let sources = prepared.iter().collect::<Vec<_>>();
        S::for_each_source(&sources, &mut self.books)
    }

    pub fn replay_from_sources_with_stats<S>(
        &mut self,
        sources: &[&S],
    ) -> Result<S::Stats, S::Error>
    where
        S: ReplaySource<L, Sort, U, H, Pub, DefaultBookConcreteType = Book<L, Sort, U, H, Pub>>
            + Clone,
    {
        let prepared = self.prepare_sources(sources);
        let sources = prepared.iter().collect::<Vec<_>>();
        S::for_each_source_with_stats(&sources, &mut self.books)
    }

    fn prepare_sources<S>(&mut self, sources: &[&S]) -> Vec<S>
    where
        S: ReplaySource<L, Sort, U, H, Pub, DefaultBookConcreteType = Book<L, Sort, U, H, Pub>>
            + Clone,
    {
        for source in sources {
            self.books
                .entry(source.ticker().trim().to_ascii_lowercase())
                .or_insert_with(|| source.new_book())
                .set_publisher(self.publisher.clone());
        }
        sources
            .iter()
            .map(|source| (*source).clone().with_cutoff(self.cutoff_time))
            .collect()
    }

    pub fn get(&self, ticker: &str) -> Option<&Book<L, Sort, U, H, Pub>> {
        self.books.get(&ticker.trim().to_ascii_lowercase())
    }

    pub fn get_mut(&mut self, ticker: &str) -> Option<&mut Book<L, Sort, U, H, Pub>> {
        self.books.get_mut(&ticker.trim().to_ascii_lowercase())
    }
}

impl<L, Sort, U, H, Pub> Default for ReplayContext<L, Sort, U, H, Pub>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    fn default() -> Self {
        Self::new(None)
    }
}

/// Replay state whose price representation is selected by `L::Price`.
pub struct ReplayContext<
    L: PriceLevelContract,
    Sort: PriceSortingPolicy = BTreeMapPriceSorting,
    U: UserMapUpdatePolicy = UpdateUserMap,
    H: HiddenQuantityPolicy = UpdateHiddenQuantity,
    Pub = NullPublisher,
> {
    pub books: HashMap<String, Book<L, Sort, U, H, Pub>>,
    pub dummy_time: DateTime<Utc>,
    /// Applied when preparing sources for replay. Use a fresh context when
    /// rebuilding state at an earlier cutoff; existing books are not rewound.
    pub cutoff_time: Option<DateTime<Utc>>,
    publisher: Pub,
    sink_tasks: SinkTasks,
}

impl<L, Sort, U, H> ReplayContext<L, Sort, U, H>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
{
    /// Connect sink receivers before replay starts. A Tokio runtime must be
    /// active when connecting asynchronous or blocking batch consumers.
    pub fn with_sinks<D: Sink<PriceLevelChangeEvent<L::Price>>>(
        cutoff_time: Option<DateTime<Utc>>,
        sinks: D,
    ) -> Result<ReplayContext<L, Sort, U, H, D::Publisher>, SinkError>
    where
        D::Publisher: lobo_events::BookPublisherFactory<L::Price>,
    {
        let (publisher, sink_tasks) = sinks.connect()?;
        Ok(ReplayContext {
            books: HashMap::new(),
            dummy_time: DateTime::<Utc>::MIN_UTC,
            cutoff_time,
            publisher,
            sink_tasks,
        })
    }
}

impl<L, Sort, U, H, Pub> Listens<Arc<BookEvent<PriceLevelChangeEvent<L::Price>>>>
    for ReplayContext<L, Sort, U, H, Pub>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
{
}

impl<L, Sort, U, H, Pub> Context<PriceLevelChangeEvent<L::Price>>
    for ReplayContext<L, Sort, U, H, Pub>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    type Publisher = Pub;
    fn publisher_factory(&self) -> &Pub {
        &self.publisher
    }
}

impl<L, Sort, U, H, Pub> ReplayContext<L, Sort, U, H, Pub>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    /// Disconnect every book, drain all receivers, and finalize destinations.
    /// Returns the replayed books with null publishers, retaining their state.
    /// External publisher clones must be dropped before awaiting this method.
    pub async fn finish(self) -> Result<HashMap<String, Book<L, Sort, U, H>>, SinkError> {
        let books = self
            .books
            .into_iter()
            .map(|(ticker, book)| (ticker, book.with_publisher(NullPublisher)))
            .collect();
        drop(self.publisher);
        self.sink_tasks.finish().await?;
        Ok(books)
    }
}

/// Accumulates source-specific counters while the generic replay loop runs.
///
/// Every callback is optional so sources only need to account for stages that
/// are meaningful to them.
pub trait ReplayStats<M>: Default {
    fn record_source_message(&mut self, _message: &M) {}
    fn record_security_message(&mut self, _message: &M) {}
    fn record_replay_message(&mut self, _message: &M) {}
    fn record_replay_event(&mut self) {}
    fn merge(&mut self, other: Self);
}

/// Describes how source messages become typed Polars rows.
///
/// Row state is local to one conversion. It can retain source metadata (for
/// example, an ITCH stock-locate-to-ticker map) without mutating the source.
#[cfg(feature = "polars")]
pub trait ReplayRows<M> {
    type RowState: Default;

    fn row_schema(&self) -> Schema;

    fn row_data(&self, message: &M, state: &mut Self::RowState) -> Row<'static>;
}

pub trait ReplaySource<L, S, U, H, Pub = NullPublisher>
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
{
    type Error;
    type Message;
    type StreamError: Into<Self::Error>;
    type Stream: Iterator<Item = Result<Self::Message, Self::StreamError>>;
    type Stats: ReplayStats<Self::Message>;
    type DefaultBookConcreteType;

    #[cfg(feature = "concurrent")]
    fn parallel_for_each_source(
        sources: &[&Self],
        books: &mut HashMap<String, Self::DefaultBookConcreteType>,
        scanner_count: usize,
    ) -> Result<(), Self::Error>
    where
        Self: Sized + Sync,
        Self::DefaultBookConcreteType: Send,
        Self::Error: Send,
    {
        if sources.is_empty() {
            return Ok(());
        }

        let scanner_count = scanner_count.clamp(1, sources.len());

        // Each job owns its sources and books.
        let mut jobs = (0..scanner_count)
            .map(|_| {
                (
                    Vec::<&Self>::new(),
                    HashMap::<String, Self::DefaultBookConcreteType>::new(),
                )
            })
            .collect::<Vec<_>>();

        // Sources for the same ticker must always go to the same scanner.
        let mut assignments = HashMap::<String, usize>::new();
        let mut next_scanner = 0;

        for source in sources.iter().copied() {
            let ticker = source.ticker().trim().to_ascii_lowercase();

            let scanner = *assignments.entry(ticker.clone()).or_insert_with(|| {
                let scanner = next_scanner % scanner_count;
                next_scanner += 1;
                scanner
            });

            jobs[scanner].0.push(source);

            // Move an existing context book into its exclusive worker.
            if let Some(book) = books.remove(&ticker) {
                jobs[scanner].1.insert(ticker, book);
            }
        }

        let completed = jobs
            .into_par_iter()
            .map(|(sources, mut local_books)| {
                // For ITCH, this opens the common file once for this partition
                // and runs the existing locator-dispatch loop.
                let result = Self::for_each_source(&sources, &mut local_books);
                (local_books, result)
            })
            .collect::<Vec<_>>();

        // Always restore books, including partial state from a failed scanner.
        let mut first_error = None;

        for (local_books, result) in completed {
            books.extend(local_books);

            if first_error.is_none()
                && let Err(error) = result
            {
                first_error = Some(error);
            };
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    /// Prepare adapter-owned cutoff state before opening the replay stream.
    ///
    /// The context calls this on a source clone once per replay invocation,
    /// before any scanning or parallel dispatch. Convert the timezone, validate
    /// assumptions, and precompute the representation needed by
    /// [`Self::is_at_or_before_cutoff`] here. `None` must clear a previous cutoff.
    /// Sources without timestamps can keep both default implementations.
    #[inline]
    fn with_cutoff(self, _cutoff_time: Option<DateTime<Utc>>) -> Self
    where
        Self: Sized,
    {
        self
    }

    /// Inclusive cutoff predicate on an already-prepared source.
    ///
    /// Keep this to a cheap comparison. Message timestamp types and comparison
    /// state belong to the adapter; replay imposes no timestamp requirement.
    #[inline(always)]
    fn is_at_or_before_cutoff(&self, _message: &Self::Message) -> bool {
        true
    }

    fn ticker(&self) -> &str;

    fn new_book(&self) -> Self::DefaultBookConcreteType;

    fn get_stream(&self) -> Result<Self::Stream, Self::Error>;

    /// Whether a source message can produce a replay event.
    fn include_message(&self, _message: &Self::Message) -> bool {
        true
    }

    /// Whether a source message belongs to this source's pre-resolved security.
    fn is_relevant(&self, message: &Self::Message) -> bool;

    /// Adapt and apply one already-filtered source message.
    ///
    /// The return value indicates whether the message produced a replay event.
    fn adapt_message(
        &self,
        message: Self::Message,
        book: &mut Self::DefaultBookConcreteType,
    ) -> bool;

    /// Read every source message into a typed Polars lazy frame.
    ///
    /// This deliberately uses [`ReplayRows::row_data`] rather than replay
    /// adaptation: tabular conversion and order-book mutation are independent
    /// operations.
    #[cfg(feature = "polars")]
    fn to_lazy_frame(&self) -> Result<LazyFrame, Self::Error>
    where
        Self: ReplayRows<Self::Message>,
        Self::Error: From<PolarsError>,
    {
        let mut state = <Self as ReplayRows<Self::Message>>::RowState::default();
        let mut rows = Vec::new();

        for message in self.get_stream()? {
            let message = message.map_err(Into::into)?;
            rows.push(self.row_data(&message, &mut state));
        }

        let frame = DataFrame::from_rows_and_schema(&rows, &self.row_schema())?;
        Ok(frame.lazy())
    }

    /// Replay matching messages without collecting statistics.
    fn for_each_event(&self, book: &mut Self::DefaultBookConcreteType) -> Result<(), Self::Error> {
        for message in self.get_stream()? {
            let message = message.map_err(Into::into)?;
            if self.is_relevant(&message)
                && self.include_message(&message)
                && self.is_at_or_before_cutoff(&message)
            {
                let _ = self.adapt_message(message, book);
            }
        }
        Ok(())
    }

    fn for_each_event_with_stats(
        &self,
        book: &mut Self::DefaultBookConcreteType,
    ) -> Result<Self::Stats, Self::Error> {
        let mut stats = Self::Stats::default();

        for message in self.get_stream()? {
            let message = message.map_err(Into::into)?;
            stats.record_source_message(&message);

            if !self.is_relevant(&message) {
                continue;
            }
            stats.record_security_message(&message);

            if !self.include_message(&message) || !self.is_at_or_before_cutoff(&message) {
                continue;
            }
            stats.record_replay_message(&message);

            if self.adapt_message(message, book) {
                stats.record_replay_event();
            }
        }

        Ok(stats)
    }

    /// Replay multiple sources without collecting statistics, allowing adapters
    /// to coalesce shared streams.
    fn for_each_source(
        sources: &[&Self],
        books: &mut HashMap<String, Self::DefaultBookConcreteType>,
    ) -> Result<(), Self::Error>
    where
        Self: Sized,
    {
        for source in sources {
            let ticker = source.ticker().trim().to_ascii_lowercase();
            let book = books.entry(ticker).or_insert_with(|| source.new_book());
            source.for_each_event(book)?;
        }
        Ok(())
    }

    /// Replay multiple sources while collecting statistics, allowing adapters
    /// to coalesce shared streams.
    fn for_each_source_with_stats(
        sources: &[&Self],
        books: &mut HashMap<String, Self::DefaultBookConcreteType>,
    ) -> Result<Self::Stats, Self::Error>
    where
        Self: Sized,
    {
        let mut combined = Self::Stats::default();
        for source in sources {
            let ticker = source.ticker().trim().to_ascii_lowercase();
            let book = books.entry(ticker).or_insert_with(|| source.new_book());
            combined.merge(source.for_each_event_with_stats(book)?);
        }
        Ok(combined)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    use lobo_primitives::CompressedPrice;
    use lobo_storage::{
        policies::UpdateHiddenQuantity, price_level::DeepPriceLevel,
        price_sorting::BTreeMapPriceSorting,
    };

    use super::*;

    type TestLevel = DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>;
    type TestBook = Book<TestLevel, BTreeMapPriceSorting>;

    #[derive(Clone)]
    struct TestSource {
        constructed_book: Rc<Cell<bool>>,
        replayed: Rc<Cell<bool>>,
    }

    #[derive(Debug, Default, PartialEq, Eq)]
    struct TestStats {
        source_messages: u64,
        replay_events: u64,
    }

    impl<M> ReplayStats<M> for TestStats {
        fn record_source_message(&mut self, _message: &M) {
            self.source_messages += 1;
        }

        fn record_replay_event(&mut self) {
            self.replay_events += 1;
        }

        fn merge(&mut self, other: Self) {
            self.source_messages += other.source_messages;
            self.replay_events += other.replay_events;
        }
    }

    impl ReplaySource<TestLevel, BTreeMapPriceSorting, UpdateUserMap, UpdateHiddenQuantity>
        for TestSource
    {
        type Error = ();
        type Message = ();
        type StreamError = ();
        type Stream = std::array::IntoIter<Result<(), ()>, 1>;
        type Stats = TestStats;
        type DefaultBookConcreteType = TestBook;

        fn ticker(&self) -> &str {
            "test"
        }

        fn new_book(&self) -> Self::DefaultBookConcreteType {
            self.constructed_book.set(true);
            TestBook::new()
        }

        fn get_stream(&self) -> Result<Self::Stream, Self::Error> {
            Ok([Ok(())].into_iter())
        }

        fn is_relevant(&self, _message: &()) -> bool {
            true
        }

        fn adapt_message(&self, _message: (), _book: &mut TestBook) -> bool {
            self.replayed.set(true);
            true
        }
    }

    #[test]
    fn context_construction_and_replay_delegate_to_the_source() {
        let source = TestSource {
            constructed_book: Rc::default(),
            replayed: Rc::default(),
        };

        let mut context = ReplayContext::default();
        assert!(!source.constructed_book.get());
        assert_eq!(
            context.replay_from(&source),
            Ok(TestStats {
                source_messages: 1,
                replay_events: 1,
            })
        );
        assert!(source.constructed_book.get());
        assert!(source.replayed.get());
        assert!(context.get("TEST").is_some());
    }

    #[test]
    fn multi_source_replay_without_stats_delegates_to_the_no_stats_path() {
        let source = TestSource {
            constructed_book: Rc::default(),
            replayed: Rc::default(),
        };

        let mut context = ReplayContext::default();
        assert_eq!(context.replay_from_sources(&[&source]), Ok(()));
        assert!(source.constructed_book.get());
        assert!(source.replayed.get());
        assert!(context.get("TEST").is_some());
    }

    #[test]
    fn multi_source_replay_with_stats_preserves_the_stats_path() {
        let source = TestSource {
            constructed_book: Rc::default(),
            replayed: Rc::default(),
        };

        let mut context = ReplayContext::default();
        assert_eq!(
            context.replay_from_sources_with_stats(&[&source]),
            Ok(TestStats {
                source_messages: 1,
                replay_events: 1,
            })
        );
        assert!(source.constructed_book.get());
        assert!(source.replayed.get());
        assert!(context.get("TEST").is_some());
    }

    #[test]
    fn sources_without_timestamps_ignore_context_cutoffs() {
        let source = TestSource {
            constructed_book: Rc::default(),
            replayed: Rc::default(),
        };
        let mut context = ReplayContext::new(Some(DateTime::<Utc>::MIN_UTC));
        assert_eq!(context.replay_from(&source).unwrap().replay_events, 1);
        assert!(source.replayed.get());
    }

    #[derive(Clone, Default)]
    struct DatetimeSource {
        cutoff: Option<DateTime<Utc>>,
        preparations: Rc<Cell<usize>>,
        applied: Rc<RefCell<Vec<DateTime<Utc>>>>,
        stream_error: bool,
    }

    fn instant(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    impl ReplaySource<TestLevel, BTreeMapPriceSorting, UpdateUserMap, UpdateHiddenQuantity>
        for DatetimeSource
    {
        type Error = ();
        type Message = DateTime<Utc>;
        type StreamError = ();
        type Stream = std::vec::IntoIter<Result<Self::Message, ()>>;
        type Stats = TestStats;
        type DefaultBookConcreteType = TestBook;

        fn with_cutoff(mut self, cutoff: Option<DateTime<Utc>>) -> Self {
            self.preparations.set(self.preparations.get() + 1);
            self.cutoff = cutoff;
            self
        }

        fn is_at_or_before_cutoff(&self, message: &Self::Message) -> bool {
            self.cutoff.is_none_or(|cutoff| *message <= cutoff)
        }

        fn ticker(&self) -> &str {
            "datetime"
        }

        fn new_book(&self) -> TestBook {
            TestBook::new()
        }

        fn get_stream(&self) -> Result<Self::Stream, ()> {
            // A later message can follow one past the cutoff; filtering must
            // not assume every adapter's stream is timestamp-sorted.
            let mut messages = vec![Ok(instant(1)), Ok(instant(3)), Ok(instant(2))];
            if self.stream_error {
                messages.push(Err(()));
            }
            Ok(messages.into_iter())
        }

        fn is_relevant(&self, _message: &Self::Message) -> bool {
            true
        }

        fn adapt_message(&self, message: Self::Message, _book: &mut TestBook) -> bool {
            self.applied.borrow_mut().push(message);
            true
        }
    }

    #[test]
    fn context_prepares_custom_datetime_cutoffs_once_on_every_serial_path() {
        for path in 0..3 {
            let source = DatetimeSource::default();
            let mut context = ReplayContext::new(Some(instant(2)));
            let stats = match path {
                0 => Some(context.replay_from(&source).unwrap()),
                1 => {
                    context.replay_from_sources(&[&source]).unwrap();
                    None
                }
                _ => Some(context.replay_from_sources_with_stats(&[&source]).unwrap()),
            };
            assert_eq!(source.preparations.get(), 1);
            assert_eq!(*source.applied.borrow(), [instant(1), instant(2)]);
            assert_eq!(source.cutoff, None, "the caller's source remains reusable");
            if let Some(stats) = stats {
                assert_eq!(stats.source_messages, 3);
                assert_eq!(stats.replay_events, 2);
            }
        }
    }

    #[test]
    fn direct_replay_honors_cutoffs_and_propagates_stream_errors() {
        for collect_stats in [false, true] {
            let source = DatetimeSource {
                stream_error: true,
                ..Default::default()
            }
            .with_cutoff(Some(instant(2)));
            let mut book = TestBook::new();
            let result = if collect_stats {
                source.for_each_event_with_stats(&mut book).map(|_| ())
            } else {
                source.for_each_event(&mut book)
            };
            assert_eq!(result, Err(()));
            assert_eq!(*source.applied.borrow(), [instant(1), instant(2)]);
        }
    }
}
