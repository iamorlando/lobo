//! Credential-free Kraken Spot WebSocket v2 L2 feed.
//! Rules: https://docs.kraken.com/exchange/guides/websockets/book-checksum-v2
pub mod messages;
#[cfg(feature = "python")]
pub mod python;
use crate::adapter::decimal::Decimal;
use crate::adapter::market::AdaptToFeed;
use crate::adapter::{AdapterInfo, FeedMode, FeedState, Instrument, MarketDataAdapter};
use lobo_primitives::Price64;
use lobo_replay::custom::operations::FormattedLevelUpdate;
use lobo_storage::policies::checksum::{DecimalWidths, Specification};
use messages::KrakenTrade;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
pub const INFO: AdapterInfo = AdapterInfo {
    id: "kraken",
    name: "Kraken Spot",
    mode: FeedMode::Live,
    endpoint: Some("wss://ws.kraken.com/v2"),
    default_symbol: "BTC/USD",
    timezone: "UTC",
    supports_trades: true,
    level: crate::adapter::market::BookLevel::L2,
};
pub struct Kraken {
    state: FeedState,
    depth: usize,
    wanted: BTreeSet<String>,
    sent: BTreeSet<String>,
    directory_ready: bool,
    last_trade: BTreeMap<String, u64>,
}
impl Kraken {
    pub fn new(symbol: &str, depth: usize) -> Result<Self, String> {
        if ![10, 25, 100, 500, 1000].contains(&depth) {
            return Err("Kraken depth must be 10, 25, 100, 500, or 1000".into());
        }
        let mut state = FeedState::new(symbol)?;
        state.context.set_checksum(Specification::kraken().prepare().map_err(str::to_owned)?)?;
        let wanted = BTreeSet::from([state.selected.clone()]);
        Ok(Self {
            state,
            depth,
            wanted,
            sent: BTreeSet::new(),
            directory_ready: false,
            last_trade: BTreeMap::new(),
        })
    }
    fn request(&mut self, method: &str, symbol: &str) {
        let mut params = json!({"channel":"book","symbol":[symbol],"depth":self.depth});
        if method == "subscribe" {
            params["snapshot"] = true.into();
        }
        self.state
            .outgoing
            .push(json!({"method":method,"params":params}).to_string());
    }
    fn refresh_readiness(&mut self) {
        self.state.warming = !self.state.synchronized(&self.state.selected);
    }
    fn subscribe_pair(&mut self, symbol: &str) {
        self.request("subscribe", symbol);
        self.state.outgoing.push(
            json!({"method":"subscribe","params":{
                "channel":"trade","symbol":[symbol],"snapshot":false
            }})
            .to_string(),
        );
    }
    fn timestamp(value: &Value) -> Result<u64, String> {
        chrono::DateTime::parse_from_rfc3339(value.as_str().ok_or("Missing Kraken timestamp")?)
            .map_err(|_| "Invalid Kraken timestamp")?
            .timestamp_nanos_opt()
            .and_then(|n| u64::try_from(n).ok())
            .ok_or_else(|| "Kraken timestamp out of range".into())
    }
    fn trades(&mut self, message: &Value) -> Result<(), String> {
        // Never replay historical trade snapshots into a live session.
        if message["type"] != "update" {
            return Ok(());
        }
        let data = message["data"]
            .as_array()
            .ok_or("Missing Kraken trade data")?;
        let mut decoded = Vec::with_capacity(data.len());
        for trade in data {
            let symbol = trade["symbol"].as_str().ok_or("Missing trade symbol")?;
            if !self.sent.contains(symbol) {
                continue;
            }
            let instrument = self
                .state
                .instruments
                .get(symbol)
                .ok_or("Trade before directory")?;
            let price = Decimal::from_json(&trade["price"])?.atoms(instrument.price_decimals)?;
            let quantity =
                Decimal::from_json(&trade["qty"])?.atoms(instrument.quantity_decimals)?;
            if price == 0 || quantity == 0 {
                return Err("Zero trade price or quantity".into());
            }
            let maker_side = match trade["side"].as_str() {
                Some("buy") => lobo_models::Side::Sell,
                Some("sell") => lobo_models::Side::Buy,
                _ => return Err("Invalid Kraken taker side".into()),
            };
            decoded.push((
                symbol,
                trade["trade_id"].as_u64().ok_or("Missing trade ID")?,
                KrakenTrade {
                    timestamp: Self::timestamp(&trade["timestamp"])?,
                    price: Price64::from(price),
                    quantity,
                    maker_side,
                },
            ));
        }
        for (symbol, id, trade) in decoded {
            let last = self.last_trade.entry(symbol.to_owned()).or_default();
            if id <= *last {
                continue;
            }
            *last = id;
            self.state.start_ns.get_or_insert(trade.timestamp);
            self.state.clock_ns = self.state.clock_ns.max(trade.timestamp);
            trade
                .process_feed(self.state.book_mut(symbol))
                .map_err(|error| format!("Kraken trade: {error:?}"))?;
        }
        Ok(())
    }
    fn resnapshot(&mut self, symbol: &str) {
        self.state.invalidate_aggregate(symbol);
        self.request("unsubscribe", symbol);
        self.request("subscribe", symbol);
        self.refresh_readiness();
    }
    fn instruments(&mut self, message: &Value) -> Result<(), String> {
        let pairs = message["data"]["pairs"]
            .as_array()
            .ok_or("Missing Kraken instrument pairs")?;
        for pair in pairs {
            if pair["status"] == "delisted" || pair["status"] == "work_in_progress" {
                continue;
            }
            let symbol = pair["symbol"].as_str().ok_or("Missing instrument symbol")?;
            let precision = |field: &str| -> Result<u8, String> {
                let value = pair[field]
                    .as_u64()
                    .ok_or_else(|| format!("Missing {field}"))?;
                if value > 18 {
                    return Err("Instrument precision exceeds supported 18 decimals".into());
                }
                Ok(value as u8)
            };
            self.state.register(Instrument {
                symbol: symbol.to_owned(),
                price_decimals: precision("price_precision")?,
                quantity_decimals: precision("qty_precision")?,
            })?;
        }
        self.directory_ready = true;
        self.subscribe_scope()?;
        for symbol in self.wanted.clone() {
            if !self.state.instruments.contains_key(&symbol) {
                return Err(format!("{symbol} is unavailable on Kraken"));
            }
            if self.sent.insert(symbol.clone()) {
                self.subscribe_pair(&symbol);
            }
        }
        Ok(())
    }
    fn levels(updates: &Value, instrument: &Instrument) -> Result<Vec<(Price64, u64, DecimalWidths)>, String> {
        let mut result = Vec::new();
        if updates.is_null() {
            return Ok(result);
        }
        for update in updates.as_array().ok_or("Invalid Kraken price levels")? {
            let price = Decimal::from_json(&update["price"])?;
            let quantity = Decimal::from_json(&update["qty"])?;
            let raw = price.atoms(instrument.price_decimals)?;
            if raw == 0 {
                return Err("Zero book price".into());
            }
            let key = Price64::from(raw);
            result.push((key, quantity.atoms(instrument.quantity_decimals)?, DecimalWidths {
                price: price.scale(), quantity: quantity.scale(),
            }));
        }
        Ok(result)
    }
    fn book(&mut self, message: &Value) -> Result<(), String> {
        let snapshot = match message["type"].as_str() {
            Some("snapshot") => true,
            Some("update") => false,
            _ => return Err("Unknown Kraken book message type".into()),
        };
        let data = message["data"]
            .as_array()
            .ok_or("Missing Kraken book data")?;
        if data.len() != 1 {
            return Err("Expected one Kraken book per message".into());
        }
        let data = &data[0];
        let symbol = data["symbol"]
            .as_str()
            .ok_or("Missing Kraken book symbol")?;
        if !self.sent.contains(symbol) {
            return Ok(());
        }
        // Discard deltas until a validated snapshot has established this connection.
        if !snapshot && !self.state.synchronized(symbol) {
            return Ok(());
        }
        let instrument = self
            .state
            .instruments
            .get(symbol)
            .ok_or("Book arrived before instrument directory")?
            .clone();
        // Decode the complete message before touching native state.
        let asks = Self::levels(&data["asks"], &instrument)?;
        let bids = Self::levels(&data["bids"], &instrument)?;
        let checksum = data["checksum"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("Missing/invalid Kraken checksum")?;
        let timestamp = Self::timestamp(&data["timestamp"])?;
        if snapshot {
            self.state.invalidate_aggregate(symbol);
            }
        let book = self.state.book_mut(symbol);
        FormattedLevelUpdate::<Price64> {
            timestamp,
            asks,
            bids,
            depth: self.depth,
        }
        .process_feed(book)
        .map_err(|error| format!("Kraken native mutation: {error:?}"))?;
        let actual = book.checksum().map_err(str::to_owned)?;
        self.state.checksum_checks += 1;
        if actual != checksum {
            self.state.checksum_failures += 1;
            self.resnapshot(symbol);
            return Ok(());
        }
        self.state.commit();
        self.state.outputs[symbol].borrow_mut().synchronized = true;
        self.state.start_ns.get_or_insert(timestamp);
        self.state.clock_ns = self.state.clock_ns.max(timestamp);
        self.refresh_readiness();
        Ok(())
    }
}
impl MarketDataAdapter for Kraken {
    fn info(&self) -> AdapterInfo<'_> {
        INFO
    }
    fn state(&self) -> &FeedState {
        &self.state
    }
    fn state_mut(&mut self) -> &mut FeedState {
        &mut self.state
    }
    fn receive(&mut self, bytes: &[u8], eof: bool) -> Result<(), String> {
        if eof {
            self.disconnected();
            return Ok(());
        }
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("WebSocket message exceeds 8 MiB".into());
        }
        // arbitrary_precision retains each JSON number's original decimal spelling.
        let message: Value =
            serde_json::from_slice(bytes).map_err(|e| format!("Invalid Kraken JSON: {e}"))?;
        self.state.messages += 1;
        self.state.consumed += bytes.len() as u64;
        if message["success"] == false {
            return Err(format!(
                "Kraken rejected a request: {}",
                message["error"].as_str().unwrap_or("unknown error")
            ));
        }
        match message["channel"].as_str() {
            Some("instrument") => self.instruments(&message),
            Some("book") => self.book(&message),
            Some("trade") => self.trades(&message),
            _ => Ok(()), // status, heartbeat and request acknowledgements carry no levels.
        }
    }
    fn connected(&mut self) -> Result<(), String> {
        self.sent.clear();
        self.directory_ready = false;
        self.state.outgoing.clear();
        for symbol in self.wanted.clone() {
            self.state.invalidate_aggregate(&symbol);
        }
        self.state.warming = true;
        self.state.needs_input = false;
        self.state.outgoing.push(
            json!({"method":"subscribe","params":{"channel":"instrument","snapshot":true}})
                .to_string(),
        );
        Ok(())
    }
    fn keepalive(&mut self) {
        self.state
            .outgoing
            .push(json!({"method":"ping"}).to_string());
    }
    fn disconnected(&mut self) {
        self.sent.clear();
        self.directory_ready = false;
        self.state.outgoing.clear();
        for symbol in self.wanted.clone() {
            self.state.invalidate_aggregate(&symbol);
        }
        self.state.warming = true;
    }
    fn subscribe(&mut self, symbol: &str) -> Result<(), String> {
        if !self.state.scope.contains(symbol) {
            return Err(format!("{symbol} is outside the current book scope"));
        }
        if self.directory_ready && !self.state.instruments.contains_key(symbol) {
            return Err(format!("{symbol} is unavailable on Kraken"));
        }
        self.wanted.insert(symbol.to_owned());
        if self.directory_ready && self.sent.insert(symbol.to_owned()) {
            self.subscribe_pair(symbol);
        }
        Ok(())
    }
}
