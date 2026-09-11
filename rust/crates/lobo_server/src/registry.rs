use lobo_replay::{
    feed::normalize_symbol,
    order_messages::{ApplyOrderCommand, CommandError},
};
use lobo_books::price_time_priority::{Book, CommandResult};
use lobo_models::{
    Side, CheckSum,
    events::Reports,
    server::{
        BookInfo, BookPolicy, Command, CommandResponse, Execution, FeedMessage,
        RestingOrderSnapshot,
    },
};
use lobo_primitives::{CompressedPrice, PriceType};
use lobo_storage::{
    UpdateUserMap, UserMapUpdatePolicy,
    policies::{HiddenQuantityPolicy, UpdateHiddenQuantity},
    price_level::DeepPriceLevel,
    price_sorting::SortedVectorPriceSorting,
};
use parking_lot::{Mutex, RwLock};
use lobo_storage::policies::checksum::{ChecksumPolicy, NoChecksum, Precision, Specification};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{broadcast, watch};

pub type NativeBook<U = UpdateUserMap, H = UpdateHiddenQuantity, C = NoChecksum> =
    Book<DeepPriceLevel<CompressedPrice, H, C>, SortedVectorPriceSorting, U, H>;
fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as u64
}
pub struct RegisteredBook<
    U: UserMapUpdatePolicy = UpdateUserMap,
    H: HiddenQuantityPolicy = UpdateHiddenQuantity,
    C: ChecksumPolicy = NoChecksum,
> {
    pub info: BookInfo,
    pub native: Arc<Mutex<NativeBook<U, H, C>>>,
    sequence: AtomicU64,
    events: broadcast::Sender<Arc<FeedMessage>>,
}
impl<U: UserMapUpdatePolicy, H: HiddenQuantityPolicy, C: ChecksumPolicy> RegisteredBook<U, H, C> {
    pub fn snapshot(&self) -> FeedMessage {
        let native = self.native.lock();
        let mut orders = Vec::with_capacity(native.order_storage.order_to_arena_map.len());
        let prices = CompressedPrice::from(0u32)..=CompressedPrice::from(u32::MAX);
        for side in [Side::Buy, Side::Sell] {
            for entry in native.order_storage.queue_view(side, prices.clone()) {
                // The existing queue view preserves native price/FIFO priority.
                let order = native
                    .order_storage
                    .order(entry.id)
                    .expect("queued native order");
                orders.push(RestingOrderSnapshot::from_native(order).expect("native server order"));
            }
        }
        FeedMessage::Snapshot {
            book: self.info.clone(),
            sequence: self.sequence.load(Ordering::Relaxed),
            timestamp_ns: timestamp(),
            orders,
        }
    }
    /// One lock covers native mutation, sequence assignment and publication, so
    /// Python and HTTP cannot reorder commands or race a snapshot boundary.
    pub fn apply(
        &self,
        command: Command,
        reports: Reports,
    ) -> Result<(CommandResult<CompressedPrice>, u64), CommandError> {
        let mut native = self.native.lock();
        let timestamp_ns = timestamp();
        let result = command.apply(&mut native, timestamp_ns, reports)?;
        native.order_storage.checksum().map_err(CommandError::Invalid)?;
        let sequence = if command.simulated() {
            self.sequence.load(Ordering::Relaxed)
        } else {
            let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = self.events.send(Arc::new(FeedMessage::Update {
                book: self.info.symbol.clone(),
                sequence,
                timestamp_ns,
                command,
            }));
            sequence
        };
        Ok((result, sequence))
    }
    pub fn submit(&self, command: Command) -> Result<CommandResponse, CommandError> {
        let id = command.order_id();
        let simulated = command.simulated();
        let requested = match &command {
            Command::Add { order } | Command::Fill { order } | Command::Simulate { order } => order
                .requested_quantity_with(H::included_quantity)
                .ok_or(CommandError::Invalid("quantity overflow"))?,
            Command::Execute { quantity, .. }
            | Command::Cancel { quantity, .. }
            | Command::Modify { quantity, .. } => *quantity,
            Command::Remove { .. } => 0,
        };
        let (result, sequence) = self.apply(
            command,
            Reports {
                include_fills: true,
                include_summary: false,
                include_market_impact: false,
            },
        )?;
        let (resting_order_id, executions) = match result {
            CommandResult::Filled {
                remaining_order_id,
                report,
            } => (
                remaining_order_id,
                report
                    .and_then(|r| r.fills)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|fill| Execution {
                        maker_id: fill.maker_order_id,
                        price: fill.price.into_u128() as u64,
                        quantity: fill.fill_quantity,
                        simulated,
                    })
                    .collect::<Vec<_>>(),
            ),
            CommandResult::Added { order_id } => (Some(order_id), vec![]),
            _ => (None, vec![]),
        };
        let filled = executions.iter().map(|e| e.quantity).sum::<u64>();
        let notional = executions
            .iter()
            .map(|e| u128::from(e.quantity) * u128::from(e.price))
            .sum::<u128>();
        Ok(CommandResponse {
            book: self.info.symbol.clone(),
            sequence,
            order_id: id,
            simulated,
            filled,
            remaining: requested.saturating_sub(filled),
            resting_order_id: if simulated { None } else { resting_order_id },
            average_price: (filled > 0).then(|| notional as f64 / filled as f64),
            executions,
        })
    }
}

macro_rules! registered_books {
    (() [$(($u:ident, $ub:literal, $ut:ty, $h:ident, $hb:literal, $ht:ty, $c:ident, $ct:ty),)*]) => {
        lobo_storage::__paste! {
            #[derive(Clone)]
            pub enum RegisteredBooks { $([<$u $h $c>](Arc<RegisteredBook<$ut,$ht,$ct>>),)* }
            impl Registry {
                fn construct(&self, info: BookInfo, checksum: CheckSum) -> RegisteredBooks {
                    match (info.policy.update_user_map(),info.policy.update_hidden(),checksum) {
                        $(($ub,$hb,CheckSum::$c) => RegisteredBooks::[<$u $h $c>](self.make_book(info)),)*
                    }
                }
            }
        }
    };
}
lobo_storage::book_policy_matrix!(registered_books);
macro_rules! dispatch_registered_matrix {
    (($value:expr, $book:ident, $call:expr) [$(($u:ident, $ub:literal, $ut:ty, $h:ident, $hb:literal, $ht:ty, $c:ident, $ct:ty),)*]) => {
        lobo_storage::__paste! { match $value { $(RegisteredBooks::[<$u $h $c>]($book) => $call,)* } }
    };
}
pub(crate) use dispatch_registered_matrix;
macro_rules! dispatch_registered {
    ($value:expr, $book:ident, $call:expr) => {{
        #[allow(unused_imports)] use crate::registry::{RegisteredBooks, dispatch_registered_matrix};
        lobo_storage::book_policy_matrix!(dispatch_registered_matrix, $value, $book, $call)
    }};
}
#[cfg(feature = "python")]
pub(crate) use dispatch_registered;
impl RegisteredBooks {
    pub fn info(&self) -> &BookInfo {
        dispatch_registered!(self, book, &book.info)
    }
    pub fn snapshot(&self) -> FeedMessage {
        dispatch_registered!(self, book, book.snapshot())
    }
    pub fn submit(&self, command: Command) -> Result<CommandResponse, CommandError> {
        dispatch_registered!(self, book, book.submit(command))
    }
}
pub struct Registry {
    pub adapters: RwLock<Vec<lobo_replay::custom::observer::HostedAdapter>>,
    pub metrics: lobo_batchers::PriceLevelMetrics,
    books: RwLock<BTreeMap<String, RegisteredBooks>>,
    pub events: broadcast::Sender<Arc<FeedMessage>>,
    pub shutdown: watch::Receiver<bool>,
}
impl Registry {
    pub fn new(queue_capacity: usize, shutdown: watch::Receiver<bool>) -> Self {
        Self::with_metrics(queue_capacity, shutdown, Default::default())
    }
    pub fn with_metrics(
        queue_capacity: usize,
        shutdown: watch::Receiver<bool>,
        metrics: lobo_batchers::PriceLevelMetrics,
    ) -> Self {
        let (events, _) = broadcast::channel(queue_capacity);
        Self {
            metrics,
            adapters: RwLock::default(),
            books: RwLock::default(),
            events,
            shutdown,
        }
    }
    pub fn register(
        &self,
        name: Option<String>,
        price_decimals: u8,
        quantity_decimals: u8,
    ) -> Result<Arc<RegisteredBook>, String> {
        match self.register_policy(name, price_decimals, quantity_decimals, BookPolicy::Full)? {
            RegisteredBooks::UsersHiddenNull(book) => Ok(book),
            _ => unreachable!("requested full policy"),
        }
    }
    pub fn register_policy(
        &self,
        name: Option<String>,
        price_decimals: u8,
        quantity_decimals: u8,
        policy: BookPolicy,
    ) -> Result<RegisteredBooks, String> {
        self.register_options(name, price_decimals, quantity_decimals, policy, CheckSum::Null)
    }
    pub fn register_options(&self, name: Option<String>, price_decimals: u8, quantity_decimals: u8,
        policy: BookPolicy, checksum: CheckSum) -> Result<RegisteredBooks, String> {
        if price_decimals > 9 || quantity_decimals > 18 {
            return Err("price_decimals must be <= 9; quantity_decimals must be <= 18".into());
        }
        let mut books = self.books.write();
        let symbol = normalize_symbol(&name.unwrap_or_else(|| {
            let mut index = books.len() + 1;
            while books.contains_key(&format!("BOOK{index}")) {
                index += 1;
            }
            format!("BOOK{index}")
        }))?;
        if symbol.contains(['/', '\\', '?', '#', '%']) {
            return Err("book names cannot contain URL delimiters".into());
        }
        if books.contains_key(&symbol) {
            return Err(format!("book {symbol} already exists"));
        }
        let info = BookInfo {
            symbol: symbol.clone(),
            price_decimals,
            quantity_decimals,
            policy,
        };
        let book = self.construct(info, checksum);
        books.insert(symbol, book.clone());
        let _ = self.events.send(Arc::new(book.snapshot()));
        Ok(book)
    }
    fn make_book<U: UserMapUpdatePolicy, H: HiddenQuantityPolicy, C: ChecksumPolicy>(
        &self,
        info: BookInfo,
    ) -> Arc<RegisteredBook<U, H, C>> {
        let mut book = NativeBook::<U,H,C>::new().with_id(info.symbol.clone());
        let spec = match C::SPEC { CheckSum::Null => None, CheckSum::Kraken => Some(Specification::kraken()), CheckSum::BitFinex => Some(Specification::bitfinex()) };
        if let Some(spec) = spec {
            book.order_storage.configure_checksum(spec.prepare().expect("constant specification"),
                Precision { price: info.price_decimals, quantity: info.quantity_decimals })
                .expect("validated book precision");
        }
        Arc::new(RegisteredBook {
            native: Arc::new(Mutex::new(book)),
            info,
            sequence: AtomicU64::new(0),
            events: self.events.clone(),
        })
    }
    pub fn get(&self, symbol: &str) -> Option<RegisteredBooks> {
        self.books
            .read()
            .get(&symbol.trim().to_ascii_uppercase())
            .cloned()
    }
    pub fn books(&self) -> Vec<RegisteredBooks> {
        self.books.read().values().cloned().collect()
    }
    pub fn directory(&self) -> Vec<BookInfo> {
        self.books
            .read()
            .values()
            .map(|b| b.info().clone())
            .collect()
    }
}
