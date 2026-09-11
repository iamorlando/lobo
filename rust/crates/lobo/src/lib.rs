#![doc = include_str!("../README.md")]

/// Price-time-priority books and typed commands.
pub use lobo_books as books;
/// Book events and publisher interfaces.
pub use lobo_events as events;
/// Orders, sides, execution reports, and book policies.
pub use lobo_models as models;
/// Price representations, quantities, timestamps, and identifiers.
pub use lobo_primitives as primitives;
/// Configurable order storage, price levels, sorting, and execution policies.
pub use lobo_storage as storage;

/// Built-in market-data adapters (`adapters` feature).
#[cfg(feature = "adapters")]
pub use lobo_adapters as adapters;
/// Event aggregation and optional Arrow, Feather, and GPU destinations (`batchers` feature).
#[cfg(feature = "batchers")]
pub use lobo_batchers as batchers;
/// Feed context and event sinks (`context` feature).
#[cfg(feature = "context")]
pub use lobo_context as context;
/// Replay, custom adapters, and simulation (`replay` feature).
#[cfg(feature = "replay")]
pub use lobo_replay as replay;
/// Native REST order API and terminal hosting (`server` feature).
#[cfg(feature = "server")]
pub use lobo_server as server;

/// General-purpose book with FIFO queues, B-tree price sorting, and user/hidden-quantity tracking.
///
/// Prices default to [`primitives::Price64`], expressed in integer price units
/// chosen by the caller. Use `OrderBook<primitives::Price128>` for wider prices,
/// or [`books::price_time_priority::Book`] to choose every storage policy.
pub type OrderBook<P = primitives::Price64> = books::price_time_priority::Book<
    storage::price_level::DeepPriceLevel<P, storage::policies::UpdateHiddenQuantity>,
>;

/// Book for replaying visible orders, with intrusive FIFO levels and sorted-vector prices.
///
/// This preset skips the per-user index and hidden-quantity tracking. Choose
/// [`OrderBook`] when either is needed. Both presets use a null event publisher
/// until a publisher is attached with `with_publisher`.
pub type ReplayBook<P = primitives::Price64> = books::price_time_priority::Book<
    storage::price_level::IntrusivePriceLevel<P, storage::policies::DoNotUpdateHiddenQuantity>,
    storage::price_sorting::SortedVectorPriceSorting,
    storage::DoNotUpdateUserMap,
    storage::policies::DoNotUpdateHiddenQuantity,
>;

/// Common types for creating books and submitting orders.
///
/// Advanced configuration is available through [`storage`], [`models`], and
/// [`books`]; optional components have their own named modules.
pub mod prelude {
    pub use crate::books::price_time_priority::{Book, Command, CommandError, CommandResult};
    pub use crate::models::{
        Side,
        events::Reports,
        orders::order_types::{
            IcebergOrder, IcebergOrderData, LimitOrder, LimitOrderData, MarketOrder,
            MarketOrderData,
        },
    };
    pub use crate::primitives::{CompressedPrice, Price64, Price128, PriceType, uuid::Uuid};
    pub use crate::storage::{MutatingFills, SimulatedFills};
    pub use crate::{OrderBook, ReplayBook};
}
