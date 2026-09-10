//! Composable market-data adapters over books.
//!
//! A protocol owns wire state; `CustomAdapter` owns the shared feed context.
//! Transport drivers and the web app share the same lifecycle contract.
mod contract;
mod message;
mod protocol;
pub use contract::*;
pub use message::*;
pub use protocol::*;

#[cfg(feature = "json")]
pub mod decimal;
#[cfg(feature = "native")]
mod http;
pub mod operations;
pub mod orders;
#[cfg(feature = "python")]
pub mod python;
pub mod reconcile;
#[cfg(feature = "native")]
pub mod runtime;
pub mod source;
#[cfg(feature = "native")]
mod transport;
#[cfg(feature = "native")]
pub use transport::TextHeartbeat;
pub mod upsert;

#[cfg(feature = "json")]
pub mod definition;

#[cfg(feature = "json")]
pub mod observer;
