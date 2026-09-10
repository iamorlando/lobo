//! Aggregate wire updates use the shared level mutation API.
pub use lobo_replay::custom::operations::LevelUpdate as KrakenBookUpdate;
pub use crate::adapter::messages::PublicTrade as KrakenTrade;
