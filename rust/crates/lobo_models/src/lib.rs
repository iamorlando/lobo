mod book_policy;
pub mod events;
pub mod orders;
#[cfg(feature = "server-api")]
pub mod server;
pub use book_policy::{BookPolicy, CheckSum};

#[cfg(feature = "python")]
use pyo3::prelude::*;

#[cfg_attr(
    feature = "python",
    pyclass(module = "lobo.orders", eq, eq_int, from_py_object)
)]
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
#[cfg_attr(
    feature = "server-api",
    derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)
)]
#[cfg_attr(feature = "server-api", serde(rename_all = "snake_case"))]
pub enum Side {
    Buy,
    Sell,
}

#[cfg(test)]
mod commands;

#[cfg(test)]
mod tests {
    use super::Side;

    #[test]
    fn sides_are_distinct_copyable_values() {
        let side = Side::Buy;
        assert_eq!(side, Side::Buy);
        assert_ne!(side, Side::Sell);
        assert_eq!(format!("{side:?}"), "Buy");
    }
}
