//! The default order-message interface used when no exchange adapter is supplied.
//!
//! HTTP commands deserialize directly into [`Command`] and apply through
//! [`ApplyOrderCommand`]. The browser consumes the same server's snapshots and
//! updates with `OrderMessages` when the `json` feature is enabled. There is
//! no custom declaration to construct or compile for this path.
//!
//! Exchange adapters keep their own protocols and call the book API directly.

pub mod commands;
pub use commands::{ApplyFeedCommand, ApplyOrderCommand, CommandError};
pub use lobo_models::server::Command;
#[cfg(feature = "json")]
mod feed;
#[cfg(feature = "json")]
pub use feed::{INFO, OrderMessages};
