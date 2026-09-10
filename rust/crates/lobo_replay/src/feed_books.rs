//! Policy selection at feed boundaries; book mutation loops stay generic.
use crate::feed::FeedPublishers;
use lobo_books::price_time_priority::Book;
use lobo_models::{BookPolicy, CheckSum, Side, orders::core::RestingOrder};
use lobo_primitives::{Price64, uuid::Uuid};
use lobo_storage::{
    policies::checksum::{self, ChecksumPolicy, Precision, Prepared},
    price_level::IntrusivePriceLevel,
    price_sorting::BTreeMapPriceSorting,
};
use std::{collections::HashMap, sync::Arc};

pub type FeedBookWith<U, H, C> =
    Book<IntrusivePriceLevel<Price64, H, C>, BTreeMapPriceSorting, U, H, FeedPublishers>;

macro_rules! feed_books {
    (() [$(($u:ident, $ub:literal, $ut:ty, $h:ident, $hb:literal, $ht:ty, $c:ident, $ct:ty),)*]) => {
        lobo_storage::__paste! {
            pub enum FeedBook { $([<$u $h $c>](FeedBookWith<$ut, $ht, $ct>),)* }
            impl FeedBook {
                fn configured(policy: BookPolicy, spec: CheckSum, symbol: &str,
                    publisher: FeedPublishers, prepared: Option<&Arc<Prepared>>, precision: Precision) -> Self {
                    match (policy.update_user_map(), policy.update_hidden(), spec) {
                        $(($ub, $hb, CheckSum::$c) => {
                            let mut book = Book::<IntrusivePriceLevel<Price64, $ht, $ct>, _, $ut, $ht>::new()
                                .with_id(symbol.to_owned()).with_publisher(publisher);
                            if let Some(prepared) = prepared {
                                book.order_storage.configure_checksum(prepared.clone(), precision)
                                    .expect("checksum settings validated before constructing books");
                            }
                            Self::[<$u $h $c>](book)
                        },)*
                    }
                }
                pub fn fork_with_publisher(&self, publisher: FeedPublishers) -> Self {
                    match self { $(Self::[<$u $h $c>](book) => Self::[<$u $h $c>](book.fork_with_publisher(publisher)),)* }
                }
                pub fn policy(&self) -> BookPolicy {
                    match self { $(Self::[<$u $h $c>](_) => BookPolicy::from_flags($ub, $hb),)* }
                }
                pub fn checksum_spec(&self) -> CheckSum {
                    match self { $(Self::[<$u $h $c>](_) => <$ct as ChecksumPolicy>::SPEC,)* }
                }
            }
        }
    };
}
lobo_storage::book_policy_matrix!(feed_books);

#[doc(hidden)]
#[macro_export]
macro_rules! __dispatch_feed_book {
    (($value:expr, $book:ident, $call:expr)
        [$(($u:ident, $ub:literal, $ut:ty, $h:ident, $hb:literal, $ht:ty, $c:ident, $ct:ty),)*]) => {
        $crate::__storage::__paste! {
            match $value { $($crate::feed::FeedBook::[<$u $h $c>]($book) => $call,)* }
        }
    };
}
#[macro_export]
macro_rules! dispatch_feed_book {
    ($value:expr, $book:ident, $call:expr) => {{
        #[allow(unused_imports)]
        use $crate::__dispatch_feed_book;
        $crate::__storage::book_policy_matrix!(__dispatch_feed_book, $value, $book, $call)
    }};
}
impl FeedBook {
    pub fn new(policy: BookPolicy, symbol: &str, publisher: FeedPublishers) -> Self {
        Self::configured(
            policy,
            CheckSum::Null,
            symbol,
            publisher,
            None,
            Precision::default(),
        )
    }
    /// Finish the checksum after the entire message has changed the book.
    pub fn checksum(&mut self) -> Result<u32, checksum::Error> {
        crate::dispatch_feed_book!(self, book, book.order_storage.checksum())
    }
    pub fn checksum_with(&mut self, prepared: &Arc<Prepared>) -> Result<u32, checksum::Error> {
        crate::dispatch_feed_book!(self, book, book.order_storage.checksum_with(prepared))
    }

    pub fn sequence(&self) -> u64 {
        crate::dispatch_feed_book!(self, book, book.sequence())
    }
    pub fn order(&self, id: Uuid) -> Option<&RestingOrder<Price64>> {
        crate::dispatch_feed_book!(self, book, book.order_storage.order(id))
    }
    pub fn queue_view(
        &self,
        side: Side,
        prices: std::ops::RangeInclusive<Price64>,
    ) -> Vec<lobo_storage::bidask::QueueOrder<Price64>> {
        crate::dispatch_feed_book!(self, book, book.order_storage.queue_view(side, prices))
    }
    pub fn best_price(&self, side: Side) -> Option<Price64> {
        crate::dispatch_feed_book!(
            self,
            book,
            match side {
                Side::Buy => book
                    .order_storage
                    .bids
                    .visible_price_levels()
                    .next()
                    .map(|(price, _)| *price),
                Side::Sell => book
                    .order_storage
                    .asks
                    .visible_price_levels()
                    .next()
                    .map(|(price, _)| *price),
            }
        )
    }
    pub fn visible_quantity(&self, side: Side) -> u64 {
        crate::dispatch_feed_book!(
            self,
            book,
            match side {
                Side::Buy => book.order_storage.bids.visible_quantity,
                Side::Sell => book.order_storage.asks.visible_quantity,
            }
        )
    }
    pub fn order_count(&self) -> usize {
        crate::dispatch_feed_book!(self, book, book.order_storage.order_to_arena_map.len())
    }
}

/// Books and their construction settings belong to the context.
pub struct FeedBooks {
    pub books: HashMap<String, FeedBook>,
    policies: HashMap<String, BookPolicy>,
    publisher: FeedPublishers,
    checksum: Option<Arc<Prepared>>,
    precision: HashMap<String, Precision>,
}
impl FeedBooks {
    pub fn with_publisher(publisher: FeedPublishers) -> Self {
        Self {
            books: HashMap::new(),
            policies: HashMap::new(),
            checksum: None,
            precision: HashMap::new(),
            publisher,
        }
    }
    /// Configure the feed before any books are constructed.
    pub fn set_checksum(&mut self, prepared: Arc<Prepared>) -> Result<(), String> {
        if !self.books.is_empty() {
            return Err("Select checksum policy before constructing books".into());
        }
        self.checksum = Some(prepared);
        Ok(())
    }
    pub fn set_precision(&mut self, symbol: &str, precision: Precision) -> Result<(), String> {
        if precision.price > 38 || precision.quantity > 38 {
            return Err("Unsupported checksum precision".into());
        }
        self.precision
            .insert(symbol.to_ascii_lowercase(), precision);
        Ok(())
    }
    fn make(&self, key: &str, symbol: &str, policy: BookPolicy) -> FeedBook {
        FeedBook::configured(
            policy,
            self.checksum
                .as_ref()
                .map_or(CheckSum::Null, |p| p.specification.policy()),
            symbol,
            self.publisher.clone(),
            self.checksum.as_ref(),
            self.precision.get(key).copied().unwrap_or_default(),
        )
    }
    pub fn get(&self, symbol: &str) -> Option<&FeedBook> {
        self.books.get(&symbol.to_ascii_lowercase())
    }
    pub fn get_mut(&mut self, symbol: &str) -> Option<&mut FeedBook> {
        self.books.get_mut(&symbol.to_ascii_lowercase())
    }
    /// Construction policy is available before an instrument has a book.
    pub fn policy(&self, symbol: &str) -> BookPolicy {
        self.policies
            .get(&symbol.to_ascii_lowercase())
            .copied()
            .unwrap_or(BookPolicy::NoUpdates)
    }
    pub fn set_policy(&mut self, symbol: &str, policy: BookPolicy) -> Result<(), String> {
        if self.get(symbol).is_some_and(|book| book.policy() != policy) {
            if self.get(symbol).is_some_and(|book| book.order_count() != 0) {
                return Err("Book policy changed; reconnect for a new snapshot".into());
            }
            // Disconnect invalidates the old native book before the new
            // directory arrives. Recreate that empty book with its new policy.
            self.books.insert(
                symbol.to_ascii_lowercase(),
                self.make(&symbol.to_ascii_lowercase(), symbol, policy),
            );
        }
        self.policies.insert(symbol.to_ascii_lowercase(), policy);
        Ok(())
    }
    pub fn book_mut(&mut self, symbol: &str) -> &mut FeedBook {
        let key = symbol.to_ascii_lowercase();
        self.books.entry(key.clone()).or_insert_with(|| {
            FeedBook::configured(
                self.policies
                    .get(&key)
                    .copied()
                    .unwrap_or(BookPolicy::NoUpdates),
                self.checksum
                    .as_ref()
                    .map_or(CheckSum::Null, |p| p.specification.policy()),
                symbol,
                self.publisher.clone(),
                self.checksum.as_ref(),
                self.precision.get(&key).copied().unwrap_or_default(),
            )
        })
    }
}
impl<T> lobo_context::Listens<std::sync::Arc<lobo_events::BookEvent<T>>> for FeedBooks {}
impl lobo_context::Context<lobo_events::PriceLevelChangeEvent<Price64>> for FeedBooks {
    type Publisher = FeedPublishers;
    fn publisher_factory(&self) -> &FeedPublishers {
        &self.publisher
    }
}
