use std::sync::Arc;

use lobo_events::{BookEvent, PublisherFactory};

/// Describe whether a source advances recorded time or receives ongoing updates.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import models as lm
///     mode = lm.FeedMode.Live
///     ```
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(eq, frozen, from_py_object, module = "lobo.replay.adapters.models")
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedMode {
    /// Recorded messages whose source clock can advance independently of wall time.
    Replay,
    /// Ongoing updates received as the source publishes them.
    Live,
}
impl FeedMode {
    /// Return the lowercase representation used in serialized feed metadata.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Replay => "replay",
            Self::Live => "live",
        }
    }
}
/// Describe the order-book detail provided by a source, independently of timing.
///
/// L1 provides the best bid and ask, L2 aggregates quantities at each price, and
/// L3 identifies individual resting orders. Only L3 supports viewing order queues
/// and simulating a resting limit order on an alternate timeline.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import models as lm
///     depth = lm.BookLevel.L2
///     ```
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(eq, frozen, from_py_object, module = "lobo.replay.adapters.models")
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookLevel {
    /// The best quoted price and aggregate quantity on each side.
    L1,
    /// Aggregate quantities at the published price levels.
    L2,
    /// Individual orders and their queue priority at each price.
    L3,
}
impl BookLevel {
    /// Return the lowercase representation used in serialized feed metadata.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::L1 => "l1",
            Self::L2 => "l2",
            Self::L3 => "l3",
        }
    }
}

mod sinks;
pub use sinks::{
    AsyncSink, BatchSink, MpscSinks, NullSink, ReceiverSink, Sink, SinkError, SinkTask, SinkTasks,
};

/// A typed message context that supplies publishers to its routed books.
pub trait Context<T>: Listens<Arc<BookEvent<T>>> {
    type Publisher: PublisherFactory<T>;

    fn publisher_factory(&self) -> &Self::Publisher;
}

pub trait Listens<T> {
    fn create_channel() -> (
        tokio::sync::mpsc::UnboundedSender<T>,
        tokio::sync::mpsc::UnboundedReceiver<T>,
    ) {
        tokio::sync::mpsc::unbounded_channel()
    }
    fn create_broadcast(
        capacity: usize,
    ) -> (
        tokio::sync::broadcast::Sender<T>,
        tokio::sync::broadcast::Receiver<T>,
    )
    where
        T: Clone,
    {
        tokio::sync::broadcast::channel(capacity)
    }

    fn subscribe(sender: &tokio::sync::broadcast::Sender<T>) -> tokio::sync::broadcast::Receiver<T>
    where
        T: Clone,
    {
        sender.subscribe()
    }
}

#[cfg(feature = "python")]
pub mod python;

/// Instrument discovery shared by file and live contexts.
pub trait InstrumentDirectory {
    fn tickers(&self) -> Vec<String>;
}

/// Immutable routing scope, chosen before a feed starts. An empty selection is
/// deliberately distinct from All; adapters resolve this at registration time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BookScope {
    #[default]
    All,
    Selected(std::collections::BTreeSet<String>),
}
impl BookScope {
    pub fn contains(&self, symbol: &str) -> bool {
        match self {
            Self::All => true,
            Self::Selected(symbols) => symbols.contains(symbol),
        }
    }
}

pub trait ScopedInstruments: InstrumentDirectory {
    fn book_scope(&self) -> &BookScope;
    fn scoped_tickers(&self) -> Vec<String> {
        self.tickers()
            .into_iter()
            .filter(|s| self.book_scope().contains(s))
            .collect()
    }
}

pub mod volume;
pub use volume::InlineMessageSink;
