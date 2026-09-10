use lobo_models::Side;
use lobo_primitives::PriceType;
use std::sync::Arc;

mod publisher;
pub use publisher::{MpscPublisher, NullPublisher, PublisherFactory, Receiver};

#[derive(Clone, Debug)]
pub struct BookEvent<T> {
    event: T,
    sequence_number: u64,
    book_id: Arc<str>,
    timestamp_ns: u64,
}
impl<T> BookEvent<T> {
    pub fn event(&self) -> &T {
        &self.event
    }
    pub fn sequence_number(&self) -> u64 {
        self.sequence_number
    }
    pub fn book_id(&self) -> &str {
        &self.book_id
    }
    pub fn shared_book_id(&self) -> Arc<str> {
        Arc::clone(&self.book_id)
    }
    /// Source clock in nanoseconds (ITCH: exchange time since midnight).
    pub fn timestamp_ns(&self) -> u64 {
        self.timestamp_ns
    }
    pub fn at_timestamp(mut self, timestamp_ns: u64) -> Self {
        self.timestamp_ns = timestamp_ns;
        self
    }
    /// Transform the payload while retaining source identity and timing.
    pub fn with_event<U>(&self, event: U) -> BookEvent<U> {
        BookEvent {
            event,
            sequence_number: self.sequence_number,
            book_id: self.book_id.clone(),
            timestamp_ns: self.timestamp_ns,
        }
    }
    pub fn from_event(event: T, sequence_number: u64, book_id: Arc<str>) -> Self {
        Self {
            event,
            sequence_number,
            book_id,
            timestamp_ns: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PriceLevelChangeEvent<T> {
    visible_quantity: u64,
    hidden_quantity: u64,
    price: T,
    number_of_orders: usize,
    side: Side,
}

impl<T> PriceLevelChangeEvent<T>
where
    T: PriceType,
{
    pub fn new(
        visible_quantity: u64,
        hidden_quantity: u64,
        price: T,
        number_of_orders: usize,
        side: Side,
    ) -> Self {
        PriceLevelChangeEvent {
            visible_quantity,
            hidden_quantity,
            price,
            number_of_orders,
            side,
        }
    }
    pub fn visible_quantity(&self) -> u64 {
        self.visible_quantity
    }
    pub fn hidden_quantity(&self) -> u64 {
        self.hidden_quantity
    }
    pub fn price(&self) -> T {
        self.price
    }
    pub fn number_of_orders(&self) -> usize {
        self.number_of_orders
    }
    pub fn side(&self) -> Side {
        self.side
    }
}

mod mutation;
pub use mutation::{EventPublisher, MutationPublisher};
pub use publisher::{BookPublisherFactory, Publishers};

/// One side's new best price. None denotes an empty side; quantity/count are
/// sampled after the mutation. Quantity-only changes at the same price do not emit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PriceChangeEvent<P> {
    pub price: Option<P>,
    pub quantity: u64,
    pub number_of_orders: usize,
    pub side: Side,
}
pub type BestPriceMessageEvent<P> = PriceChangeEvent<P>;

/// Executed liquidity at one price, independent of optional fill reports.
/// `side` is the resting/maker side. Cancellations and simulations do not emit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TradedVolumeEvent<P> {
    pub price: P,
    pub quantity: u64,
    pub side: Side,
}

/// Counterfactual fills use a distinct payload type, never a market trade route.
#[derive(Clone, Debug)]
pub struct SimulatedExecutionEvent<P> {
    pub execution: TradedVolumeEvent<P>,
    pub maker_order_id: lobo_primitives::uuid::Uuid,
    pub taker_order_id: lobo_primitives::uuid::Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OhlcBar<P> {
    pub open: P,
    pub high: P,
    pub low: P,
    pub close: P,
    pub volume: u64,
    pub index: u64,
    /// Executions contributing to this bar; a split execution counts in each part.
    pub ticks: u64,
    pub start_ns: u64,
    pub end_ns: u64,
}
pub type VolumeBar<P> = OhlcBar<P>;
