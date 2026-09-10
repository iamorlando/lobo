//! Credential-free Bitfinex v2 R0 trading books and public executions.
//! Wire protocol only: all orders, levels and matching belong to native lobo.
//! https://docs.bitfinex.com/reference/ws-public-raw-books
pub mod messages;
#[cfg(feature = "python")]
pub mod python;
mod simulation;
use crate::adapter::market::AdaptToFeed;
use crate::adapter::{
    AdapterInfo, BookLevel, FeedMode, FeedState, Instrument, MarketDataAdapter,
    decimal::Decimal,
    market::{BootstrapRequest, FeedConnection},
    messages::PublicTrade,
};
use lobo_models::{Side, orders::traits::Trades};
use lobo_primitives::{Price64, uuid::Uuid};
use messages::RawOrder;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const ENDPOINT: &str = "wss://api-pub.bitfinex.com/ws/2";
pub const DIRECTORY: &str = "https://api-pub.bitfinex.com/v2/conf/pub:list:pair:exchange";
pub const INFO: AdapterInfo = AdapterInfo {
    id: "bitfinex",
    name: "Bitfinex R0",
    mode: FeedMode::Live,
    endpoint: Some(ENDPOINT),
    default_symbol: "BTCUSD",
    timezone: "UTC",
    supports_trades: true,
    level: BookLevel::L3,
};
const FLAGS: u64 = 32768 | 65536 | 131072; // TIMESTAMP | SEQ_ALL | OB_CHECKSUM
const PAIRS_PER_CONNECTION: usize = 15; // Two channels per pair, public limit 30.
#[derive(Clone)]
struct Channel {
    symbol: String,
    book: bool,
    snapshot: bool,
}
#[derive(Default)]
struct Connection {
    symbols: BTreeSet<String>,
    channels: BTreeMap<u64, Channel>,
    outgoing: Vec<String>,
    sequence: Option<u64>,
    configured: bool,
}
impl Connection {
    fn subscribe(&mut self, symbol: &str) {
        self.outgoing.push(json!({"event":"subscribe","channel":"book","symbol":format!("t{symbol}"),"prec":"R0","freq":"F0","len":"250"}).to_string());
        self.outgoing.push(
            json!({"event":"subscribe","channel":"trades","symbol":format!("t{symbol}")})
                .to_string(),
        );
    }
}
pub struct Bitfinex {
    state: FeedState,
    connections: Vec<Connection>,
    assignments: BTreeMap<String, u32>,
    last_trade: BTreeMap<String, u64>,
    simulation: simulation::LiveSimulation,
}
impl Bitfinex {
    pub fn new(symbol: &str) -> Result<Self, String> {
        let mut state = FeedState::new(symbol)?;
        state.context.set_checksum(lobo_storage::policies::checksum::Specification::bitfinex()
            .prepare().map_err(str::to_owned)?)?;
        Ok(Self {
            state,
            connections: Vec::new(),
            assignments: BTreeMap::new(),
            last_trade: BTreeMap::new(),
            simulation: Default::default(),
        })
    }
    fn readiness(&mut self) {
        self.state.warming = !self.state.synchronized(&self.state.selected);
    }
    fn connection(&mut self, id: u32) -> Result<&mut Connection, String> {
        self.connections
            .get_mut(id as usize)
            .ok_or_else(|| "Unknown Bitfinex connection".into())
    }
    fn control(&mut self, id: u32, message: &Value) -> Result<(), String> {
        match message["event"].as_str() {
            Some("conf") => {
                if message["status"] != "OK" || message["flags"].as_u64() != Some(FLAGS) {
                    return Err(
                        "Bitfinex did not enable timestamps, sequencing and checksums".into(),
                    );
                }
                let connection = self.connection(id)?;
                if !connection.configured {
                    connection.configured = true;
                    for symbol in connection.symbols.clone() {
                        connection.subscribe(&symbol);
                    }
                }
            }
            Some("subscribed") => {
                let wire = message["symbol"]
                    .as_str()
                    .ok_or("Missing Bitfinex symbol")?;
                let symbol = wire
                    .strip_prefix('t')
                    .ok_or("Expected Bitfinex trading symbol")?;
                let chan_id = message["chanId"].as_u64().ok_or("Missing channel ID")?;
                let book = match message["channel"].as_str() {
                    Some("book") if message["prec"] == "R0" => true,
                    Some("trades") => false,
                    _ => return Err("Unexpected Bitfinex subscription".into()),
                };
                let connection = self.connection(id)?;
                if !connection.configured || !connection.symbols.contains(symbol) {
                    return Err("Unrequested Bitfinex subscription".into());
                }
                connection.channels.insert(
                    chan_id,
                    Channel {
                        symbol: symbol.into(),
                        book,
                        snapshot: false,
                    },
                );
            }
            Some("error") => {
                return Err(format!(
                    "Bitfinex request {}: {}",
                    message["code"], message["msg"]
                ));
            }
            Some("info") => {
                if message["version"].as_u64().is_some_and(|v| v != 2) {
                    return Err("Unsupported Bitfinex protocol version".into());
                }
                if message["platform"]["status"] == 0
                    || matches!(message["code"].as_u64(), Some(20051 | 20060 | 20061))
                {
                    return Err("Bitfinex maintenance/restart; a fresh snapshot is required".into());
                }
            }
            Some("unsubscribed") => {
                return Err("Bitfinex subscription ended; reconnect required".into());
            }
            _ => {}
        }
        Ok(())
    }
    fn amount(value: &Value) -> Result<(Side, u64), String> {
        let text = value
            .as_number()
            .ok_or("Expected numeric Bitfinex amount")?
            .to_string();
        let (side, unsigned) = match text.strip_prefix('-') {
            Some(s) => (Side::Sell, s),
            None => (Side::Buy, text.as_str()),
        };
        let quantity = Decimal::parse(unsigned)?.atoms(8)?;
        if quantity == 0 {
            return Err("Zero Bitfinex amount".into());
        }
        Ok((side, quantity))
    }
    fn order(value: &Value, timestamp: u64) -> Result<RawOrder, String> {
        let row = value.as_array().ok_or("Invalid Bitfinex order")?;
        if row.len() < 3 {
            return Err("Incomplete Bitfinex order".into());
        }
        let reference = row[0]
            .as_u64()
            .filter(|n| *n != 0)
            .ok_or("Invalid Bitfinex order ID")?;
        let price = Decimal::from_json(&row[1])?.atoms(8)?;
        let (side, quantity) = Self::amount(&row[2])?;
        Ok(RawOrder {
            timestamp,
            reference,
            price,
            quantity,
            side,
        })
    }
    fn book(
        &mut self,
        id: u32,
        chan_id: u64,
        channel: &Channel,
        data: &Value,
        timestamp: u64,
    ) -> Result<(), String> {
        let rows = data.as_array().ok_or("Invalid R0 book payload")?;
        if !channel.snapshot {
            // R0 snapshots contain orders without creation times. Seed each price
            // in ID order; subsequent arrivals use their observed server timestamp.
            let mut orders = rows
                .iter()
                .map(|r| Self::order(r, timestamp))
                .collect::<Result<Vec<_>, _>>()?;
            if orders.iter().any(|o| o.price == 0) {
                return Err("Deletion in R0 snapshot".into());
            }
            orders.sort_unstable_by_key(|o| o.reference);
            if orders.windows(2).any(|w| w[0].reference == w[1].reference) {
                return Err("Duplicate R0 snapshot order".into());
            }
            self.state.invalidate_orders(&channel.symbol);
            for order in orders {
                order
                    .process_feed(self.state.book_mut(&channel.symbol))
                    .map_err(|e| format!("R0 snapshot: {e:?}"))?;
            }
            if let Some(channel) = self.connection(id)?.channels.get_mut(&chan_id) {
                channel.snapshot = true;
            }
        } else {
            // BULK_UPDATES is not enabled; an array of rows here is not a delta.
            let update = Self::order(data, timestamp)?;
            if let Some(branch) = &mut self.state.simulation {
                if branch.feed.selected == channel.symbol && !branch.stopped() {
                    let previous = self
                        .state
                        .context
                        .get(&channel.symbol)
                        .and_then(|book| book.order(Uuid::from_u128(update.reference.into())))
                        .map(Trades::quantity);
                    self.simulation.update(branch, update, previous)?;
                }
            }
            update
                .process_feed(self.state.book_mut(&channel.symbol))
                .map_err(|e| format!("R0 update: {e:?}"))?;
            // Start with a checked snapshot, then stream mutations immediately.
            // Subsequent periodic CRC failures invalidate all affected liquidity.
            if self.state.synchronized(&channel.symbol) {
                self.state.commit();
            }
        }
        Ok(())
    }
    fn trade(&mut self, channel: &Channel, row: &Value) -> Result<(), String> {
        if !self.state.synchronized(&channel.symbol) {
            return Ok(());
        }
        let values = row.as_array().ok_or("Invalid Bitfinex trade")?;
        if values.len() < 4 {
            return Err("Incomplete Bitfinex trade".into());
        }
        let id = values[0].as_u64().ok_or("Invalid trade ID")?;
        let timestamp = millis(&values[1])?;
        let (taker, quantity) = Self::amount(&values[2])?;
        let price = Decimal::from_json(&values[3])?.atoms(8)?;
        if price == 0 {
            return Err("Zero trade price".into());
        }
        let last = self.last_trade.entry(channel.symbol.clone()).or_default();
        if id <= *last {
            return Ok(());
        }
        *last = id;
        let trade = PublicTrade {
            timestamp,
            price: Price64::from(price),
            quantity,
            maker_side: if taker == Side::Buy {
                Side::Sell
            } else {
                Side::Buy
            },
        };
        if let Some(branch) = &mut self.state.simulation {
            if branch.feed.selected == channel.symbol {
                self.simulation.trade(branch, trade)?;
            }
        }
        trade
            .process_feed(self.state.book_mut(&channel.symbol))
            .map_err(|e| format!("Bitfinex execution: {e:?}"))?;
        self.state.commit();
        Ok(())
    }
    fn message(&mut self, id: u32, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Bitfinex message exceeds 8 MiB".into());
        }
        let message: Value =
            serde_json::from_slice(bytes).map_err(|e| format!("Invalid Bitfinex JSON: {e}"))?;
        self.state.messages += 1;
        self.state.consumed += bytes.len() as u64;
        if message.is_object() {
            return self.control(id, &message);
        }
        let frame = message.as_array().ok_or("Expected Bitfinex frame")?;
        let channel_id = frame
            .first()
            .and_then(Value::as_u64)
            .ok_or("Invalid channel ID")?;
        let payload = frame.get(1).ok_or("Missing Bitfinex payload")?;
        let tagged = payload.as_str().is_some_and(|s| s != "hb");
        let metadata = if tagged { 3 } else { 2 };
        let sequence = frame
            .get(metadata)
            .and_then(Value::as_u64)
            .ok_or("Missing Bitfinex sequence")?;
        let timestamp = millis(
            frame
                .get(metadata + 1)
                .ok_or("Missing Bitfinex timestamp")?,
        )?;
        let connection = self.connection(id)?;
        if connection
            .sequence
            .is_some_and(|last| last.checked_add(1) != Some(sequence))
        {
            return Err("Bitfinex sequence gap; reconnecting for a fresh snapshot".into());
        }
        connection.sequence = Some(sequence);
        let channel = connection
            .channels
            .get(&channel_id)
            .ok_or("Unknown Bitfinex channel")?
            .clone();
        self.state.start_ns.get_or_insert(timestamp);
        self.state.clock_ns = self.state.clock_ns.max(timestamp);
        match payload.as_str() {
            Some("hb") => {}
            Some("cs") if channel.book && channel.snapshot => {
                let expected = frame
                    .get(2)
                    .and_then(Value::as_i64)
                    .and_then(|v| i32::try_from(v).ok())
                    .ok_or("Invalid signed Bitfinex checksum")?;
                let book = self
                    .state
                    .context.get_mut(&channel.symbol)
                    .ok_or("Checksum before book")?;
                let actual = book.checksum().map_err(str::to_owned)? as i32;
                self.state.checksum_checks += 1;
                if actual != expected {
                    self.state.checksum_failures += 1;
                    return Err(format!(
                        "Bitfinex {} checksum mismatch; fresh snapshot required",
                        channel.symbol
                    ));
                }
                self.state.outputs[&channel.symbol]
                    .borrow_mut()
                    .synchronized = true;
                self.state.commit();
                self.readiness();
            }
            Some("te" | "tu") if !channel.book => self.trade(&channel, &frame[2])?,
            Some(_) => return Err("Unexpected Bitfinex channel message".into()),
            None if channel.book => self.book(id, channel_id, &channel, payload, timestamp)?,
            None => {
                // Historical trade snapshots only establish a deduplication floor.
                let trades = payload.as_array().ok_or("Invalid trade snapshot")?;
                for row in trades {
                    let trade_id = row
                        .get(0)
                        .and_then(Value::as_u64)
                        .ok_or("Invalid snapshot trade ID")?;
                    let last = self.last_trade.entry(channel.symbol.clone()).or_default();
                    *last = (*last).max(trade_id);
                }
            }
        }
        Ok(())
    }
}
fn millis(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .and_then(|v| v.checked_mul(1_000_000))
        .filter(|v| *v <= i64::MAX as u64)
        .ok_or_else(|| "Invalid Bitfinex millisecond timestamp".into())
}
impl MarketDataAdapter for Bitfinex {
    fn info(&self) -> AdapterInfo<'_> {
        INFO
    }
    fn state(&self) -> &FeedState {
        &self.state
    }
    fn state_mut(&mut self) -> &mut FeedState {
        &mut self.state
    }
    fn bootstrap_requests(&self) -> Vec<BootstrapRequest<'_>> {
        vec![BootstrapRequest {
            id: "instruments",
            url: DIRECTORY,
        }]
    }
    fn bootstrap(&mut self, id: &str, bytes: &[u8]) -> Result<(), String> {
        if id != "instruments" || bytes.len() > 1024 * 1024 {
            return Err("Invalid Bitfinex directory response".into());
        }
        let data: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let pairs = data
            .get(0)
            .and_then(Value::as_array)
            .ok_or("Missing Bitfinex instrument directory")?;
        for pair in pairs {
            let symbol = pair.as_str().ok_or("Invalid Bitfinex pair")?;
            if symbol.is_empty()
                || !symbol
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b':')
            {
                return Err("Invalid Bitfinex pair name".into());
            }
            self.state.register(Instrument {
                symbol: symbol.into(),
                price_decimals: 8,
                quantity_decimals: 8,
            })?;
        }
        self.subscribe_scope()
    }
    fn subscribe(&mut self, symbol: &str) -> Result<(), String> {
        if !self.state.scope.contains(symbol) {
            return Err(format!("{symbol} is outside the current book scope"));
        }
        if !self.state.instruments.contains_key(symbol) {
            return Err(format!("{symbol} is unavailable on Bitfinex"));
        }
        if self.assignments.contains_key(symbol) {
            return Ok(());
        }
        let index = self
            .connections
            .iter()
            .position(|c| c.symbols.len() < PAIRS_PER_CONNECTION)
            .unwrap_or(self.connections.len());
        if index == self.connections.len() {
            self.connections.push(Connection::default());
        }
        let connection = &mut self.connections[index];
        connection.symbols.insert(symbol.into());
        self.assignments.insert(symbol.into(), index as u32);
        if connection.configured {
            connection.subscribe(symbol);
        }
        Ok(())
    }
    fn connections(&self) -> Vec<FeedConnection<'_>> {
        self.connections
            .iter()
            .enumerate()
            .map(|(i, c)| FeedConnection {
                id: i as u32,
                endpoint: ENDPOINT,
                selected: c.symbols.contains(&self.state.selected),
            })
            .collect()
    }
    fn receive(&mut self, bytes: &[u8], eof: bool) -> Result<(), String> {
        if eof {
            self.disconnected();
            Ok(())
        } else {
            self.receive_on(0, bytes)
        }
    }
    fn receive_on(&mut self, id: u32, bytes: &[u8]) -> Result<(), String> {
        let result = self.message(id, bytes);
        if result.is_err() {
            self.disconnected_on(id);
        }
        result
    }
    fn connected(&mut self) -> Result<(), String> {
        self.connected_on(0)
    }
    fn connected_on(&mut self, id: u32) -> Result<(), String> {
        self.disconnected_on(id);
        self.connection(id)?
            .outgoing
            .push(json!({"event":"conf","flags":FLAGS}).to_string());
        self.state.needs_input = false;
        Ok(())
    }
    fn disconnected(&mut self) {
        for id in 0..self.connections.len() as u32 {
            self.disconnected_on(id);
        }
    }
    fn disconnected_on(&mut self, id: u32) {
        if let Some(connection) = self.connections.get_mut(id as usize) {
            connection.configured = false;
            connection.sequence = None;
            connection.channels.clear();
            connection.outgoing.clear();
            for symbol in connection.symbols.clone() {
                self.state.invalidate_orders(&symbol);
            }
        }
        self.readiness();
    }
    fn commands(&mut self) -> Vec<String> {
        self.commands_on(0)
    }
    fn commands_on(&mut self, id: u32) -> Vec<String> {
        self.connections
            .get_mut(id as usize)
            .map_or_else(Vec::new, |c| std::mem::take(&mut c.outgoing))
    }
    fn keepalive(&mut self) {
        self.keepalive_on(0);
    }
    fn keepalive_on(&mut self, id: u32) {
        if let Some(c) = self.connections.get_mut(id as usize) {
            c.outgoing.push(json!({"event":"ping"}).to_string());
        }
    }
    fn advance(&mut self, elapsed_ns: u64, _budget: usize) -> Result<(), String> {
        if let Some(start) = self.state.start_ns {
            self.state.clock_ns = self.state.clock_ns.max(start.saturating_add(elapsed_ns));
        }
        if let Some(branch) = &mut self.state.simulation {
            self.simulation.flush(branch, self.state.clock_ns)?;
        }
        self.state.commit();
        Ok(())
    }
    fn simulation_note(&self) -> Option<&'static str> {
        Some(
            "Estimated FIFO from the visible 250 orders per side and public trades. Snapshot arrivals are observation times; maker IDs are absent from trades. Book corrections reconcile for 250 ms; cancellations never count as executions.",
        )
    }
}
