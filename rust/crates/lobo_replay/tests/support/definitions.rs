#![allow(dead_code)]
//! Test constructors read declarations exported from the Python examples.
use lobo_replay::custom::{
    AdapterDescriptor, BookLevel, CustomAdapter, FeedMode,
    definition::{DefinedProtocol, schema::Definition},
};
use std::sync::{Arc, OnceLock};
pub fn descriptor(name: &str, symbol: &str) -> AdapterDescriptor {
    AdapterDescriptor {
        id: name.into(),
        name: name.into(),
        mode: if name == "itch" {
            FeedMode::Replay
        } else {
            FeedMode::Live
        },
        level: if name == "kraken" {
            BookLevel::L2
        } else {
            BookLevel::L3
        },
        default_symbol: symbol.into(),
        endpoint: None,
        timezone: if name == "itch" {
            "America/New_York"
        } else {
            "UTC"
        }
        .into(),
        supports_trades: true,
    }
}
pub fn definition(name: &str) -> Arc<Definition> {
    static DEFINITIONS: OnceLock<std::collections::BTreeMap<&'static str, Arc<Definition>>> =
        OnceLock::new();
    let definitions = DEFINITIONS.get_or_init(|| {
        ["itch", "kraken", "bitfinex", "server"]
            .into_iter()
            .map(|name| {
                let text = match name {
                    "itch" => include_str!("../definitions/itch.json"),
                    "kraken" => include_str!("../definitions/kraken.json"),
                    "bitfinex" => include_str!("../definitions/bitfinex.json"),
                    "server" => include_str!("../definitions/server.json"),
                    _ => panic!("Unknown test fixture"),
                };
                (name, Arc::new(Definition::parse(text).unwrap()))
            })
            .collect()
    });
    definitions[name].clone()
}
fn adapter(name: &str, symbol: &str) -> Result<CustomAdapter<DefinedProtocol>, String> {
    let info = descriptor(name, symbol);
    let protocol = DefinedProtocol::new(definition(name), info.clone(), symbol)?;
    CustomAdapter::new(info, protocol, symbol)
}
pub mod itch {
    pub fn new(
        symbol: &str,
        _start: u64,
    ) -> Result<super::CustomAdapter<super::DefinedProtocol>, String> {
        super::adapter("itch", symbol)
    }
}
pub mod kraken {
    pub fn new(
        symbol: &str,
        depth: usize,
    ) -> Result<super::CustomAdapter<super::DefinedProtocol>, String> {
        assert_eq!(depth, 10);
        super::adapter("kraken", symbol)
    }
}
pub mod bitfinex {
    pub fn new(symbol: &str) -> Result<super::CustomAdapter<super::DefinedProtocol>, String> {
        super::adapter("bitfinex", symbol)
    }
}
pub mod server {
    pub fn new(symbol: &str) -> Result<super::CustomAdapter<super::DefinedProtocol>, String> {
        super::adapter("server", symbol)
    }
}
