//! Resolved operations for serving an adapter's books. The same declaration
//! engine applies these operations in observers, including simulation routing.
use super::{
    AdapterDescriptor, Protocol,
    definition::{
        DefinedProtocol,
        expression::{Expr, Input},
        schema::{Action, Definition},
    },
};
use crate::feed::FeedState;
use lobo_models::{BookPolicy, Side};
use lobo_primitives::{Price64, PriceType};
use serde_json::{Value, json};
use std::sync::Arc;

pub trait EventSink {
    fn record(&mut self, event: impl FnOnce() -> Result<Value, String>);
    /// Build a group only when the sink consumes events. Discarding sinks can
    /// erase preparation such as book lookup and snapshot construction as well.
    fn record_many<I: IntoIterator<Item = Value>>(
        &mut self,
        events: impl FnOnce() -> Result<I, String>,
    ) -> Result<(), String> {
        for event in events()? {
            self.record(|| Ok(event));
        }
        Ok(())
    }
    fn commit(&mut self, state: &FeedState) -> Result<(), String>;
    fn reject(&mut self);
}
#[derive(Default)]
pub struct Discard;
impl EventSink for Discard {
    #[inline(always)]
    fn record(&mut self, _: impl FnOnce() -> Result<Value, String>) {}
    #[inline(always)]
    fn record_many<I: IntoIterator<Item = Value>>(
        &mut self,
        _: impl FnOnce() -> Result<I, String>,
    ) -> Result<(), String> {
        Ok(())
    }
    #[inline(always)]
    fn commit(&mut self, _: &FeedState) -> Result<(), String> {
        Ok(())
    }
    #[inline(always)]
    fn reject(&mut self) {}
}
pub fn literal(value: impl serde::Serialize) -> Value {
    json!({"op":"literal","value":value})
}
pub fn book(
    symbol: &str,
    actions: Vec<Value>,
    snapshot: bool,
    timestamp: u64,
    ready: bool,
    depth: usize,
) -> Value {
    json!({"action":"book","symbol":literal(symbol),"actions":actions,"snapshot":literal(snapshot),"timestamp":literal(timestamp),"ready":ready,"depth":depth,"checksum":null})
}
pub fn register(
    symbol: &str,
    price_decimals: u8,
    quantity_decimals: u8,
    policy: BookPolicy,
) -> Value {
    json!({"action":"register","symbol":literal(symbol),"price_decimals":literal(price_decimals),"quantity_decimals":literal(quantity_decimals),"key":literal(Value::Null),"policy":literal(policy)})
}
pub fn resolve(action: &Action, input: &Input<'_>) -> Result<Value, String> {
    fn walk(value: Value, input: &Input<'_>) -> Result<Value, String> {
        Ok(match value {
            Value::Object(ref map) if map.contains_key("op") => literal(
                serde_json::from_value::<Expr>(value)
                    .map_err(|e| e.to_string())?
                    .eval(input)?,
            ),
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(key, value)| Ok((key, walk(value, input)?)))
                    .collect::<Result<_, String>>()?,
            ),
            Value::Array(values) => Value::Array(
                values
                    .into_iter()
                    .map(|v| walk(v, input))
                    .collect::<Result<_, _>>()?,
            ),
            value => value,
        })
    }
    walk(
        serde_json::to_value(action).map_err(|e| e.to_string())?,
        input,
    )
}
pub fn descriptor(info: super::AdapterInfo<'_>) -> Value {
    json!({"id":info.id,"name":info.name,"mode":info.mode.as_str(),"level":info.level.as_str(),"defaultSymbol":info.default_symbol,"timezone":info.timezone,"supportsTrades":info.supports_trades,"endpoint":info.endpoint})
}
pub fn parse_descriptor(value: Value) -> Result<AdapterDescriptor, String> {
    let text = |name: &str| {
        value[name]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("Missing {name}"))
    };
    Ok(AdapterDescriptor {
        id: text("id")?,
        name: text("name")?,
        mode: match text("mode")?.as_str() {
            "live" => super::FeedMode::Live,
            "replay" => super::FeedMode::Replay,
            _ => return Err("Invalid feed mode".into()),
        },
        level: match text("level")?.as_str() {
            "l1" => super::BookLevel::L1,
            "l2" => super::BookLevel::L2,
            "l3" => super::BookLevel::L3,
            _ => return Err("Invalid book level".into()),
        },
        default_symbol: text("defaultSymbol")?,
        timezone: text("timezone")?,
        supports_trades: value["supportsTrades"]
            .as_bool()
            .ok_or("Missing supportsTrades")?,
        endpoint: value["endpoint"].as_str().map(str::to_owned),
    })
}
#[derive(serde::Serialize, serde::Deserialize)]
struct DirectoryInstrument {
    symbol: String,
    price_decimals: u8,
    quantity_decimals: u8,
    policy: BookPolicy,
}
fn instruments(adapter: &dyn super::MarketDataAdapter) -> Vec<DirectoryInstrument> {
    let state = adapter.state();
    state
        .instruments
        .values()
        .filter(|i| state.scope.contains(&i.symbol))
        .map(|i| DirectoryInstrument {
            symbol: i.symbol.clone(),
            price_decimals: i.price_decimals,
            quantity_decimals: i.quantity_decimals,
            policy: state.context.policy(&i.symbol),
        })
        .collect()
}
pub fn snapshot(adapter: &dyn super::MarketDataAdapter, sequence: u64) -> Result<Value, String> {
    use lobo_storage::price_level::PriceLevelContract;
    let state = adapter.state();
    let mut actions = Vec::new();
    for instrument in state
        .instruments
        .values()
        .filter(|i| state.scope.contains(&i.symbol))
    {
        let Some(current) = state.book(&instrument.symbol) else {
            continue;
        };
        actions.push(register(
            &instrument.symbol,
            instrument.price_decimals,
            instrument.quantity_decimals,
            current.policy(),
        ));
        let mut contents = Vec::new();
        if adapter.info().level != super::BookLevel::L3 {
            crate::dispatch_feed_book!(current, book, {
                for (side, levels) in [
                    (
                        Side::Buy,
                        book.order_storage
                            .bids
                            .visible_price_levels()
                            .collect::<Vec<_>>(),
                    ),
                    (
                        Side::Sell,
                        book.order_storage
                            .asks
                            .visible_price_levels()
                            .collect::<Vec<_>>(),
                    ),
                ] {
                    for (price, level) in levels {
                        contents.push(json!({"action":"level","side":literal(side),"price":literal(price.into_u128() as u64),"quantity":literal(level.visible_quantity())}));
                    }
                }
            });
        } else {
            for side in [Side::Buy, Side::Sell] {
                for entry in current.queue_view(side, Price64::from(0u64)..=Price64::from(u64::MAX))
                {
                    let order = current
                        .order(entry.id)
                        .ok_or("Queue references missing order")?;
                    let order = lobo_models::server::RestingOrderState::from_order(order)?;
                    contents.push(json!({"action":"restore_order","value":literal(order)}));
                }
            }
        }
        actions.push(book(
            &instrument.symbol,
            contents,
            true,
            state.clock_ns,
            state.synchronized(&instrument.symbol),
            0,
        ));
    }
    Ok(
        json!({"type":"snapshot","sequence":sequence,"instruments":instruments(adapter),"actions":actions,"clock_ns":state.clock_ns,"start_ns":state.start_ns,"messages":state.messages,"consumed":state.consumed,"checksum_checks":state.checksum_checks,"checksum_failures":state.checksum_failures,"complete":state.complete}),
    )
}

/// Consume an ordered observer stream while retaining the ordinary book and
/// simulation APIs. Source decoding stays with the adapter serving the feed.
pub struct ObservedProtocol {
    inner: DefinedProtocol,
    sequence: Option<u64>,
    descriptor: AdapterDescriptor,
}
impl ObservedProtocol {
    pub fn new(descriptor: AdapterDescriptor, symbol: &str) -> Result<Self, String> {
        let definition = Definition::parse(
            r#"{"format":{"format":"json","messages":[]},"connect":[],"subscriptions":[],"bootstrap":[],"reconcile_window_ns":250000000,"reconcile_capacity":4096,"symbols_per_connection":0}"#,
        )?;
        Ok(Self {
            inner: DefinedProtocol::new(Arc::new(definition), descriptor.clone(), symbol)?,
            descriptor,
            sequence: None,
        })
    }
}
impl Protocol for ObservedProtocol {
    fn simulation_note(&self) -> Option<&str> {
        self.inner.simulation_note()
    }
    fn receive(&mut self, state: &mut FeedState, bytes: &[u8], _: bool) -> Result<(), String> {
        let value: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if value["type"] == "heartbeat" {
            return Ok(());
        }
        if value["type"] == "reset" {
            return Err("Source changed; reconnect for a snapshot".into());
        }
        let sequence = value["sequence"].as_u64().ok_or("Missing feed sequence")?;
        let is_snapshot = value["type"] == "snapshot";
        if !is_snapshot {
            let previous = self.sequence.ok_or("Update before snapshot")?;
            if sequence <= previous {
                return Ok(());
            }
            if previous.checked_add(1) != Some(sequence) {
                return Err("Observer sequence gap; reconnect for a snapshot".into());
            }
        } else {
            let definition = match value.get("protocol") {
                Some(protocol) => Arc::new(Definition::parse(&protocol.to_string())?),
                None => self.inner.definition.clone(),
            };
            self.inner =
                DefinedProtocol::new(definition, self.descriptor.clone(), &state.selected)?;
            if self.descriptor.mode == super::FeedMode::Replay {
                let aggregation = state.volume_bars().borrow().aggregation();
                state.set_bar_aggregation(aggregation);
            }
            state.context.books.clear();
            state.simulation = None;
            state.market_preview = None;
            if let Some(directory) = value.get("instruments") {
                let directory: Vec<DirectoryInstrument> =
                    serde_json::from_value(directory.clone()).map_err(|e| e.to_string())?;
                let symbols = directory
                    .iter()
                    .map(|i| i.symbol.as_str())
                    .collect::<std::collections::BTreeSet<_>>();
                state
                    .instruments
                    .retain(|symbol, _| symbols.contains(symbol.as_str()));
                // Preserve frame generations across reconnects so a snapshot
                // cannot reuse an old renderer generation for different depth.
                state
                    .outputs
                    .retain(|symbol, _| symbols.contains(symbol.as_str()));
                for instrument in directory {
                    self.inner.register_instrument(
                        state,
                        crate::feed::Instrument {
                            symbol: super::normalize_symbol(&instrument.symbol)?,
                            price_decimals: instrument.price_decimals,
                            quantity_decimals: instrument.quantity_decimals,
                        },
                        instrument.policy,
                    )?;
                }
            }
        }
        let actions: Vec<Action> =
            serde_json::from_value(value["actions"].clone()).map_err(|e| e.to_string())?;
        self.inner.apply_observed(&actions, state)?;
        self.sequence = Some(sequence);
        state.clock_ns = value["clock_ns"].as_u64().ok_or("Missing feed clock")?;
        state.start_ns = value["start_ns"].as_u64();
        state.messages = value["messages"].as_u64().ok_or("Missing message count")?;
        state.consumed = value["consumed"].as_u64().ok_or("Missing byte count")?;
        state.checksum_checks = value["checksum_checks"].as_u64().unwrap_or(0);
        state.checksum_failures = value["checksum_failures"].as_u64().unwrap_or(0);
        // A partial recorded book is still displayable; synchronization remains
        // an independent property and must never be invented by playback.
        state.warming =
            self.descriptor.mode == super::FeedMode::Live && !state.synchronized(&state.selected);
        state.complete = value["complete"].as_bool().unwrap_or(false);
        state.commit();
        Ok(())
    }
    fn advance(
        &mut self,
        state: &mut FeedState,
        elapsed: u64,
        budget: usize,
    ) -> Result<(), String> {
        if self.descriptor.mode == super::FeedMode::Replay {
            // The host owns historical time, including pauses and EOF.
            let elapsed = state
                .clock_ns
                .saturating_sub(state.start_ns.unwrap_or(state.clock_ns));
            self.inner.advance(state, elapsed, budget)
        } else {
            self.inner.advance(state, elapsed, budget)
        }
    }
    fn disconnected(&mut self, state: &mut FeedState) {
        self.sequence = None;
        state.warming = true;
    }
}

#[cfg(feature = "native")]
pub mod channel {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use tokio::sync::broadcast;
    #[derive(Clone)]
    pub struct Publisher {
        pub events: broadcast::Sender<Arc<Value>>,
        sequence: Arc<AtomicU64>,
        suspended: Arc<AtomicBool>,
    }
    impl Publisher {
        pub fn new(capacity: usize) -> Self {
            Self {
                events: broadcast::channel(capacity).0,
                sequence: Arc::new(AtomicU64::new(0)),
                suspended: Arc::new(AtomicBool::new(false)),
            }
        }
        pub fn sequence(&self) -> u64 {
            self.sequence.load(Ordering::Relaxed)
        }
        pub fn suspend(&self) {
            self.suspended.store(true, Ordering::Relaxed);
        }
        pub fn resume_snapshot(
            &self,
            adapter: &dyn super::super::MarketDataAdapter,
        ) -> Result<(), String> {
            let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
            let value = snapshot(adapter, sequence)?;
            self.suspended.store(false, Ordering::Relaxed);
            let _ = self.events.send(Arc::new(value));
            Ok(())
        }
        pub fn publish_clock(&self, state: &FeedState) {
            if self.suspended.load(Ordering::Relaxed) {
                return;
            }
            let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = self.events.send(Arc::new(json!({"type":"update","sequence":sequence,"actions":[],"clock_ns":state.clock_ns,"start_ns":state.start_ns,"messages":state.messages,"consumed":state.consumed,"checksum_checks":state.checksum_checks,"checksum_failures":state.checksum_failures,"complete":state.complete})));
        }
        pub fn sink(&self) -> Channel {
            Channel {
                publisher: self.clone(),
                pending: Vec::new(),
                error: None,
            }
        }
    }
    pub struct Channel {
        publisher: Publisher,
        pending: Vec<Value>,
        error: Option<String>,
    }
    impl EventSink for Channel {
        fn record(&mut self, event: impl FnOnce() -> Result<Value, String>) {
            match event() {
                Ok(value) => self.pending.push(value),
                Err(e) => self.error = Some(e),
            }
        }
        fn commit(&mut self, state: &FeedState) -> Result<(), String> {
            if let Some(e) = self.error.take() {
                self.reject();
                return Err(e);
            }
            if self.publisher.suspended.load(Ordering::Relaxed) {
                self.pending.clear();
                return Ok(());
            }
            let sequence = self.publisher.sequence.fetch_add(1, Ordering::Relaxed) + 1;
            let event = json!({"type":"update","sequence":sequence,"actions":std::mem::take(&mut self.pending),"clock_ns":state.clock_ns,"start_ns":state.start_ns,"messages":state.messages,"consumed":state.consumed,"checksum_checks":state.checksum_checks,"checksum_failures":state.checksum_failures,"complete":state.complete});
            let _ = self.publisher.events.send(Arc::new(event));
            Ok(())
        }
        fn reject(&mut self) {
            self.pending.clear();
            self.error = None;
            let _ = self
                .publisher
                .events
                .send(Arc::new(json!({"type":"reset"})));
        }
    }
}

#[cfg(feature = "native")]
#[derive(Clone)]
pub struct HostedAdapter {
    pub session: super::runtime::SessionHandle,
    pub publisher: channel::Publisher,
    pub descriptor: AdapterDescriptor,
    pub observer_definition: Value,
}
#[cfg(feature = "native")]
impl HostedAdapter {
    /// Subscribe through the adapter contract, retaining subscriptions shared
    /// by other observers. The scope configured by Python remains authoritative.
    pub fn subscribe(&self, symbols: Vec<String>, selected: String) -> Result<(), String> {
        self.session.with(move |adapter| {
            let selected = adapter.validate_ticker(&selected)?;
            let symbols = symbols
                .iter()
                .map(|s| adapter.validate_ticker(s))
                .collect::<Result<Vec<_>, _>>()?;
            for symbol in symbols {
                adapter.subscribe(&symbol)?;
            }
            adapter.select_ticker(&selected)
        })
    }
    pub fn submit(
        &self,
        symbol: String,
        command: lobo_models::server::Command,
    ) -> Result<lobo_models::server::CommandResponse, String> {
        let publisher = self.publisher.clone();
        self.session.with(move |adapter| {
            use lobo_models::server::{Command, CommandResponse, Execution};
            let book = adapter.state().book(&symbol).ok_or("Book does not exist")?;
            let hidden = matches!(book.policy(), BookPolicy::Full | BookPolicy::NoUserMap);
            let requested = match &command {
                Command::Add { order } | Command::Fill { order } | Command::Simulate { order } => {
                    order
                        .requested_quantity_with(|q| if hidden { q } else { 0 })
                        .ok_or("Quantity overflow")?
                }
                Command::Execute { quantity, .. }
                | Command::Cancel { quantity, .. }
                | Command::Modify { quantity, .. } => *quantity,
                Command::Remove { .. } => 0,
            };
            let order_id = command.order_id();
            let simulated = command.simulated();
            let result = adapter.submit_command(&symbol, command)?;
            let (resting_order_id, executions) = match result {
                lobo_books::price_time_priority::CommandResult::Filled {
                    remaining_order_id,
                    report,
                } => (
                    remaining_order_id,
                    report
                        .and_then(|r| r.fills)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|fill| Execution {
                            maker_id: fill.maker_order_id,
                            price: fill.price.into_u128() as u64,
                            quantity: fill.fill_quantity,
                            simulated,
                        })
                        .collect::<Vec<_>>(),
                ),
                lobo_books::price_time_priority::CommandResult::Added { order_id } => {
                    (Some(order_id), vec![])
                }
                _ => (None, vec![]),
            };
            let filled = executions.iter().map(|e| e.quantity).sum::<u64>();
            let notional = executions
                .iter()
                .map(|e| u128::from(e.quantity) * u128::from(e.price))
                .sum::<u128>();
            Ok(CommandResponse {
                book: symbol,
                sequence: publisher.sequence(),
                order_id,
                simulated,
                filled,
                remaining: requested.saturating_sub(filled),
                resting_order_id: if simulated { None } else { resting_order_id },
                average_price: (filled > 0).then(|| notional as f64 / filled as f64),
                executions,
            })
        })
    }
    pub fn snapshot(&self) -> Result<Value, String> {
        let publisher = self.publisher.clone();
        let protocol = self.observer_definition.clone();
        self.session.with(move |adapter| {
            let mut value = snapshot(adapter, publisher.sequence())?;
            value["protocol"] = protocol;
            Ok(value)
        })
    }
    pub fn books(&self) -> Result<Vec<Value>, String> {
        self.session.with(|adapter|Ok(adapter.state().instruments.values().filter(|i|adapter.state().scope.contains(&i.symbol) && adapter.state().book(&i.symbol).is_some()).map(|i|json!({"symbol":i.symbol,"price_decimals":i.price_decimals,"quantity_decimals":i.quantity_decimals})).collect()))
    }
}
