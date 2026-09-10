//! Built-in adapter registry. Contracts belong to the replay API.
pub use lobo_replay::custom::*;

/// Central adapter registry: consumers select capabilities without venue switches.
pub fn adapters() -> Vec<AdapterInfo<'static>> {
    vec![
        #[cfg(feature = "itchy")]
        crate::itch::stream::INFO,
        #[cfg(feature = "kraken")]
        crate::kraken::INFO,
        #[cfg(feature = "bitfinex")]
        crate::bitfinex::INFO,
        #[cfg(feature = "server")]
        lobo_replay::order_messages::INFO,
    ]
}
pub fn create_adapter(
    id: &str,
    symbol: &str,
    start_ns: u64,
) -> Result<Box<dyn MarketDataAdapter>, String> {
    let _ = (symbol, start_ns);
    match id {
        #[cfg(feature = "server")]
        "server" => Ok(Box::new(lobo_replay::order_messages::OrderMessages::new(symbol)?)),
        #[cfg(feature = "itchy")]
        "itch" => Ok(Box::new(crate::itch::stream::ItchStream::new(
            symbol, start_ns,
        )?)),
        #[cfg(feature = "kraken")]
        "kraken" => Ok(Box::new(crate::kraken::Kraken::new(symbol, 100)?)),
        #[cfg(feature = "bitfinex")]
        "bitfinex" => Ok(Box::new(crate::bitfinex::Bitfinex::new(symbol)?)),
        _ => Err(format!("Unknown or disabled adapter: {id}")),
    }
}
