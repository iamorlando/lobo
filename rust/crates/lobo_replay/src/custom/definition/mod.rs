//! Message definitions, routing, and feed lifecycle shared by custom adapters.
#[cfg(feature = "native")]
pub mod binary;
mod binary_layout;
mod checksum;
mod compiled;
pub mod expression;
#[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
pub mod jit;
mod mutation;
#[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
mod projection;
#[cfg(feature = "python")]
pub mod python;
pub mod schema;
#[cfg(all(feature = "native", feature = "polars"))]
mod table;
use super::{
    AdaptToFeed, AdapterDescriptor, BootstrapRequest, FeedConnection, Protocol,
    operations::{LevelUpdate, PublicTrade},
    orders::*,
    reconcile::OrderReconciler,
    upsert::OrderUpdate,
};
use crate::feed::{FeedState, Instrument, normalize_symbol};
use expression::{Expr, Input, Tables, Variables, integer, key};
use lobo_context::{BookLevel, FeedMode};
use lobo_models::{BookPolicy, Side, orders::traits::Trades};
use lobo_primitives::{Price64, uuid::Uuid};
use schema::{Action, Definition, Format, Operation};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub struct DefinedProtocol<O = super::observer::Discard> {
    pub observer: O,
    pub definition: Arc<Definition>,
    #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
    program: Arc<jit::Program>,
    #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
    execution: jit::ExecutionState,
    descriptor: AdapterDescriptor,
    #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
    binary_plan: Option<Arc<compiled::Plan>>,
    vars: Variables,
    tables: Tables,
    pub(super) routes: BTreeMap<u64, String>,
    route_keys: BTreeMap<String, u64>,
    pub(super) policies: BTreeMap<String, BookPolicy>,
    pub(super) directory_complete: bool,
    wanted: BTreeSet<String>,
    assignments: BTreeMap<String, u32>,
    subscription_ready: BTreeSet<u32>,
    sent: BTreeMap<u32, BTreeSet<String>>,
    sequences: BTreeMap<(u32, String), u64>,
    commands: BTreeMap<u32, Vec<String>>,
    reconciler: OrderReconciler,
    last_trade: BTreeMap<String, u64>,
    checksum: Option<Arc<lobo_storage::policies::checksum::Prepared>>,
    snapshots: BTreeSet<String>,
    input: Vec<u8>,
    cursor: usize,
    eof: bool,
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as u64
}
fn side(v: Value) -> Result<Side, String> {
    match v.as_str() {
        Some("buy") => Ok(Side::Buy),
        Some("sell") => Ok(Side::Sell),
        _ => Err("side must map to buy or sell".into()),
    }
}
fn id(v: Value) -> Result<Uuid, String> {
    id_ref(&v)
}
fn id_ref(v: &Value) -> Result<Uuid, String> {
    if let Some(s) = v.as_str() {
        if let Ok(id) = Uuid::parse_str(s) {
            return Ok(id);
        }
        return s
            .parse::<u128>()
            .map(Uuid::from_u128)
            .map_err(|_| "Invalid order ID".into());
    }
    Ok(Uuid::from_u128(u128::from(integer(&v)?)))
}
fn error(e: lobo_storage::OrderStateError) -> String {
    format!("Order operation failed: {e:?}")
}
impl DefinedProtocol {
    pub fn new(
        definition: Arc<Definition>,
        descriptor: AdapterDescriptor,
        symbol: &str,
    ) -> Result<Self, String> {
        Self::with_observer(definition, descriptor, symbol, super::observer::Discard)
    }
}
impl<O: super::observer::EventSink> DefinedProtocol<O> {
    pub fn with_observer(
        definition: Arc<Definition>,
        descriptor: AdapterDescriptor,
        symbol: &str,
        observer: O,
    ) -> Result<Self, String> {
        #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
        let (definition, checksum) = {
            let mut definition = definition;
            let checksum = checksum::prepare(Arc::make_mut(&mut definition))?;
            (definition, checksum)
        };
        let capacity = std::num::NonZeroUsize::new(definition.reconcile_capacity)
            .ok_or("reconcile_capacity must be positive")?;
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        let program = jit::prepare_program::<O>(definition.clone(), descriptor.mode)?;
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        let (definition, checksum) = (program.definition.clone(), program.checksum.clone());
        #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
        let binary_plan = match &definition.format {
            Format::Binary(binary) => compiled::Plan::new(Arc::new(binary.clone()))
                .ok()
                .map(Arc::new),
            _ => None,
        };
        Ok(Self {
            #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
            execution: program.state(),
            #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
            program,
            observer,
            #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
            binary_plan,
            reconciler: OrderReconciler::new(definition.reconcile_window_ns, capacity),
            definition,
            descriptor,
            vars: BTreeMap::new(),
            tables: BTreeMap::new(),
            routes: BTreeMap::new(),
            route_keys: BTreeMap::new(),
            policies: BTreeMap::new(),
            directory_complete: false,
            wanted: BTreeSet::from([symbol.into()]),
            assignments: BTreeMap::from([(symbol.into(), 0)]),
            subscription_ready: BTreeSet::new(),
            sent: BTreeMap::new(),
            sequences: BTreeMap::new(),
            commands: BTreeMap::new(),
            last_trade: BTreeMap::new(),
            checksum,
            snapshots: BTreeSet::new(),
            input: Vec::new(),
            cursor: 0,
            eof: false,
        })
    }
    pub(crate) fn apply_observed(
        &mut self,
        actions: &[Action],
        state: &mut FeedState,
    ) -> Result<(), String> {
        for action in actions {
            if let Operation::Book { symbol, .. } = &action.operation {
                if let Some(symbol) = symbol.value.as_str() {
                    self.snapshots.insert(symbol.into());
                }
            }
        }
        self.actions(actions, state, &Value::Null, &Value::Null)
    }
    fn evaluate(&self, expr: &Expr, item: &Value, root: &Value) -> Result<Value, String> {
        expr.eval(&Input {
            item,
            root,
            vars: &self.vars,
            tables: &self.tables,
        })
    }
    fn scalar(&self, expr: &Expr, item: &Value, root: &Value) -> Result<u64, String> {
        expr.unsigned(&Input {
            item,
            root,
            vars: &self.vars,
            tables: &self.tables,
        })
    }
    fn current_symbol(&self) -> Result<String, String> {
        self.vars
            .get("symbol")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or("An order mapping requires a book symbol".into())
    }
    fn bind(&mut self, name: &str, value: Value) {
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        self.program.bind(&mut self.execution, name, &value);
        match self.vars.get_mut(name) {
            Some(previous) => *previous = value,
            None => {
                self.vars.insert(name.into(), value);
            }
        }
    }
    fn set_symbol(&mut self, state: &FeedState, symbol: &str) {
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        {
            // Lifecycle selection must refresh compiled slots even if the last
            // lifecycle symbol matches: intervening packets can bind another.
            self.bind("symbol", symbol.into());
            if let Some(instrument) = state.instruments.get(symbol) {
                self.bind("price_decimals", instrument.price_decimals.into());
                self.bind("quantity_decimals", instrument.quantity_decimals.into());
            }
        }
        #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
        {
            if self.vars.get("symbol").and_then(Value::as_str) != Some(symbol) {
                self.bind("symbol", symbol.into());
            }
            if let Some(instrument) = state.instruments.get(symbol) {
                for (name, value) in [
                    ("price_decimals", instrument.price_decimals),
                    ("quantity_decimals", instrument.quantity_decimals),
                ] {
                    if self.vars.get(name).and_then(Value::as_u64) != Some(u64::from(value)) {
                        self.bind(name, value.into());
                    }
                }
            }
        }
    }
    fn connection(&self) -> u32 {
        self.vars
            .get("connection")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32
    }
    fn group(&self, symbol: &str) -> u32 {
        self.assignments.get(symbol).copied().unwrap_or(0)
    }
    fn subscribe_ready(&mut self, state: &mut FeedState) -> Result<(), String> {
        let connection = self.connection();
        let symbols = self
            .wanted
            .iter()
            .filter(|s| {
                self.group(s) == connection
                    && state.instruments.contains_key(*s)
                    && !self
                        .sent
                        .get(&connection)
                        .is_some_and(|sent| sent.contains(*s))
            })
            .cloned()
            .collect::<Vec<_>>();
        let definition = self.definition.clone();
        for symbol in symbols {
            self.set_symbol(state, &symbol);
            self.actions(&definition.subscriptions, state, &Value::Null, &Value::Null)?;
            self.sent.entry(connection).or_default().insert(symbol);
        }
        Ok(())
    }
    fn invalidate(&mut self, state: &mut FeedState, symbol: &str) {
        self.observer.record(|| {
            Ok(super::observer::book(
                symbol,
                vec![],
                true,
                state.clock_ns,
                false,
                0,
            ))
        });
        if self.descriptor.level != BookLevel::L3 {
            state.invalidate_aggregate(symbol);
        } else {
            state.invalidate_orders(symbol);
        }
        self.snapshots.remove(symbol);
    }
    fn timestamp(&mut self, state: &mut FeedState, value: u64) {
        self.bind("clock", value.into());
        state.start_ns.get_or_insert(value);
        state.clock_ns = state.clock_ns.max(value);
    }
    pub(crate) fn actions(
        &mut self,
        actions: &[Action],
        state: &mut FeedState,
        item: &Value,
        root: &Value,
    ) -> Result<(), String> {
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        if let Some(result) = self
            .program
            .clone()
            .lifecycle(self, state, actions, item, root)
        {
            return result;
        }
        use Operation::*;
        for action in actions {
            match &action.operation {
                DirectoryComplete => self.directory_complete = true,
                Directory {
                    items,
                    symbol,
                    actions,
                    on_remove,
                } => {
                    let values = self.evaluate(items, item, root)?;
                    let rows = values.as_array().ok_or("Directory requires an array")?;
                    let symbols = rows
                        .iter()
                        .map(|row| {
                            normalize_symbol(
                                self.evaluate(symbol, row, root)?
                                    .as_str()
                                    .ok_or("Directory symbol must be text")?,
                            )
                        })
                        .collect::<Result<BTreeSet<_>, String>>()?;
                    let removed = state
                        .instruments
                        .keys()
                        .filter(|s| !symbols.contains(*s))
                        .cloned()
                        .collect::<Vec<_>>();
                    for old in &removed {
                        self.set_symbol(state, old);
                        self.actions(on_remove, state, item, root)?;
                        state.invalidate_orders(old);
                        state.instruments.remove(old);
                        state.context.books.remove(&old.to_ascii_lowercase());
                        state.outputs.remove(old);
                        self.policies.remove(old);
                        if let Some(route) = self.route_keys.remove(old) {
                            self.routes.remove(&route);
                        }
                        self.snapshots.remove(old);
                        self.last_trade.remove(old);
                        self.sequences.retain(|(_, symbol), _| symbol != old);
                    }
                    for row in rows {
                        self.actions(actions, state, row, root)?;
                    }
                    if !removed.is_empty() {
                        self.observer.reject();
                    }
                    if !state.instruments.contains_key(&state.selected) {
                        if let Some(symbol) = state
                            .instruments
                            .keys()
                            .find(|s| state.scope.contains(s))
                            .cloned()
                        {
                            state.selected = symbol;
                        }
                    }
                }
                Register {
                    symbol,
                    price_decimals,
                    quantity_decimals,
                    key: route,
                    policy,
                } => {
                    let symbol = normalize_symbol(
                        self.evaluate(symbol, item, root)?
                            .as_str()
                            .ok_or("Instrument symbol must be text")?,
                    )?;
                    let price_decimals = u8::try_from(self.scalar(price_decimals, item, root)?)
                        .map_err(|_| "Invalid price precision")?;
                    let quantity_decimals =
                        u8::try_from(self.scalar(quantity_decimals, item, root)?)
                            .map_err(|_| "Invalid quantity precision")?;
                    let policy_value = self.evaluate(policy, item, root)?;
                    let policy = match policy_value.as_str().ok_or("Book policy must be text")? {
                        "full" => BookPolicy::Full,
                        "no_user_map" => BookPolicy::NoUserMap,
                        "no_hidden_quantity" => BookPolicy::NoHiddenQuantity,
                        "no_updates" => BookPolicy::NoUpdates,
                        _ => return Err("Invalid book policy".into()),
                    };
                    let is_new = !state.instruments.contains_key(&symbol);
                    state.register(Instrument {
                        symbol: symbol.clone(),
                        price_decimals,
                        quantity_decimals,
                    })?;
                    if is_new {
                        state.context.set_policy(&symbol, policy)?;
                    }
                    self.policies.entry(symbol.clone()).or_insert(policy);

                    let value = self.evaluate(route, item, root)?;
                    if !value.is_null() {
                        let route = integer(&value)?;
                        if self.routes.get(&route).is_some_and(|old| old != &symbol) {
                            return Err("Instrument routing key changed symbol".into());
                        }
                        if self
                            .route_keys
                            .get(&symbol)
                            .is_some_and(|previous| *previous != route)
                        {
                            return Err("Instrument has conflicting routing keys".into());
                        }
                        self.route_keys.insert(symbol.clone(), route);
                        self.routes.insert(route, symbol.clone());
                        self.set_symbol(state, &symbol);
                    }
                }
                ForEach {
                    items,
                    actions,
                    order_by,
                    unique_by,
                } => {
                    let owned;
                    let values = if let Some(value) = items.source(item, root) {
                        value
                    } else {
                        owned = self.evaluate(items, item, root)?;
                        &owned
                    };
                    // Missing sides are valid in incremental L2 updates.
                    if values.is_null() {
                        continue;
                    }
                    let mut rows = values
                        .as_array()
                        .ok_or("ForEach requires an array")?
                        .iter()
                        .collect::<Vec<_>>();
                    if let Some(by) = order_by {
                        let mut keyed = rows
                            .into_iter()
                            .map(|row| Ok((self.evaluate(by, row, root)?, row)))
                            .collect::<Result<Vec<_>, String>>()?;
                        keyed.sort_by(|(a, _), (b, _)| match (a.as_u64(), b.as_u64()) {
                            (Some(a), Some(b)) => a.cmp(&b),
                            _ => key(a).cmp(&key(b)),
                        });
                        rows = keyed.into_iter().map(|(_, row)| row).collect();
                    }
                    if let Some(by) = unique_by {
                        let mut seen = BTreeSet::new();
                        for row in &rows {
                            if !seen.insert(key(&self.evaluate(by, row, root)?)) {
                                return Err("Duplicate key in snapshot".into());
                            }
                        }
                    }
                    if let [
                        level @ Action {
                            operation: Operation::Level { .. },
                            ..
                        },
                    ] = actions.as_slice()
                    {
                        self.levels(state, &rows, root, level)?;
                    } else {
                        for row in rows {
                            self.actions(actions, state, row, root)?;
                        }
                    }
                }
                When {
                    condition,
                    actions,
                    otherwise,
                } => {
                    let yes = self.evaluate(condition, item, root)?.as_bool() == Some(true);
                    self.actions(if yes { actions } else { otherwise }, state, item, root)?;
                }
                Let { name, value } => {
                    let value = self.evaluate(value, item, root)?;
                    self.vars.insert(name.clone(), value);
                }
                Remember {
                    table,
                    key: expr,
                    value,
                } => {
                    let key = self.evaluate(expr, item, root)?;
                    let value = self.evaluate(value, item, root)?;
                    let entries = self
                        .tables
                        .entry(self.connection())
                        .or_default()
                        .entry(table.clone())
                        .or_default();
                    let key = expression::key_ref(&key);
                    if let Some(previous) = entries.get_mut(key.as_ref()) {
                        *previous = value;
                    } else {
                        entries.insert(key.into_owned(), value);
                    }
                }
                Send { message } => {
                    let value = self.evaluate(message, item, root)?;
                    let text = serde_json::to_string(&value).map_err(|e| e.to_string())?;
                    self.commands
                        .entry(self.connection())
                        .or_default()
                        .push(text);
                }
                Subscribe => {
                    let connection = self.connection();
                    self.subscribe_scope(state)?;
                    self.bind("connection", connection.into());
                    self.subscription_ready.insert(connection);
                    self.subscribe_ready(state)?;
                }
                Sequence {
                    value,
                    key: sequence_key,
                    reset,
                } => {
                    let next = self.scalar(value, item, root)?;
                    let sequence_key = key(&self.evaluate(sequence_key, item, root)?);
                    let last = self
                        .sequences
                        .entry((self.connection(), sequence_key))
                        .or_insert(next.saturating_sub(1));
                    if !reset && last.checked_add(1) != Some(next) {
                        return Err("Sequence gap; a fresh snapshot is required".into());
                    }
                    *last = next;
                }
                Fail { message } => return Err(message.clone()),
                Book {
                    symbol,
                    actions,
                    snapshot,
                    timestamp,
                    depth,
                    checksum,
                    ready,
                } => {
                    let symbol = normalize_symbol(
                        self.evaluate(symbol, item, root)?
                            .as_str()
                            .ok_or("Book symbol must be text")?,
                    )?;
                    if !state.scope.contains(&symbol) {
                        continue;
                    }
                    if !state.instruments.contains_key(&symbol) {
                        return Err("Book arrived before its instrument definition".into());
                    }
                    if state.book(&symbol).is_none() {
                        let instrument = &state.instruments[&symbol];
                        let policy = self.policies.get(&symbol).copied().unwrap_or_default();
                        self.observer.record(|| {
                            Ok(super::observer::register(
                                &symbol,
                                instrument.price_decimals,
                                instrument.quantity_decimals,
                                policy,
                            ))
                        });
                    }
                    let snapshot = self.evaluate(snapshot, item, root)?.as_bool() == Some(true);
                    if !snapshot && !self.snapshots.contains(&symbol) {
                        continue;
                    }
                    let timestamp = self.scalar(timestamp, item, root)?;
                    if snapshot {
                        self.invalidate(state, &symbol);
                        self.snapshots.insert(symbol.clone());
                        state.book_mut(&symbol);
                    }
                    self.set_symbol(state, &symbol);
                    self.timestamp(state, timestamp);
                    if let Err(e) = self.actions(actions, state, item, root) {
                        self.invalidate(state, &symbol);
                        return Err(e);
                    }
                    if *depth > 0 {
                        LevelUpdate::<Price64> {
                            timestamp,
                            bids: vec![],
                            asks: vec![],
                            depth: *depth,
                        }
                        .process_feed(state.book_mut(&symbol))
                        .map_err(error)?;
                    }
                    if let Some(checksum) = checksum {
                        if !self.checksum(checksum, state, item, root, &symbol)? {
                            continue;
                        }
                    }
                    if *ready {
                        if let Some(output) = state.outputs.get(&symbol) {
                            output.borrow_mut().synchronized = true;
                        }
                    }
                    let synchronized = state.synchronized(&symbol);
                    self.observer.record(|| {
                        Ok(super::observer::book(
                            &symbol,
                            vec![],
                            false,
                            timestamp,
                            synchronized,
                            *depth,
                        ))
                    });
                    if synchronized {
                        state.commit();
                    }
                    state.warming = !state.synchronized(&state.selected);
                }
                _ => self.order_action(action, state, item, root)?,
            }
        }
        Ok(())
    }
    fn prepare_order<const ROUTED: bool>(
        &mut self,
        state: &FeedState,
        symbol: &str,
    ) -> Result<(), String> {
        // A compiled binary route exists only after Register installed its
        // instrument. Other entry points must still reject unknown symbols.
        if !ROUTED && state.book(symbol).is_none() && !state.instruments.contains_key(symbol) {
            return Err("Order before instrument".into());
        }
        self.observer.record_many(|| {
            let events = if state.book(symbol).is_none() {
                let instrument = state
                    .instruments
                    .get(symbol)
                    .ok_or("Order before instrument")?;
                let policy = self.policies.get(symbol).copied().unwrap_or_default();
                Some([
                    super::observer::register(
                        symbol,
                        instrument.price_decimals,
                        instrument.quantity_decimals,
                        policy,
                    ),
                    super::observer::book(symbol, vec![], true, state.clock_ns, false, 0),
                ])
            } else {
                None
            };
            Ok(events.into_iter().flatten())
        })
    }
    fn levels(
        &mut self,
        state: &mut FeedState,
        rows: &[&Value],
        root: &Value,
        action: &Action,
    ) -> Result<(), String> {
        let Operation::Level {
            side: side_expr,
            price,
            quantity,
        } = &action.operation
        else {
            return Err("Expected a level projection".into());
        };
        if rows.is_empty() {
            return Ok(());
        }
        let symbol = self.current_symbol()?;
        if !state.scope.contains(&symbol) {
            return Ok(());
        }
        self.prepare_order::<false>(state, &symbol)?;
        let instrument = state
            .instruments
            .get(&symbol)
            .ok_or("Level before instrument")?;
        let mut decoded = Vec::with_capacity(rows.len());
        let mut bids = Vec::new();
        let mut asks = Vec::new();
        for row in rows {
            let input = Input {
                item: row,
                root,
                vars: &self.vars,
                tables: &self.tables,
            };
            let which = side(side_expr.eval(&input)?)?;
            let (p, pw) = price.scaled(&input, instrument.price_decimals)?;
            let (q, qw) = quantity.scaled(&input, instrument.quantity_decimals)?;
            if p == 0 {
                return Err("Level price must be positive".into());
            }
            (if which == Side::Buy {
                &mut bids
            } else {
                &mut asks
            })
            .push((
                Price64::from(p),
                q,
                lobo_storage::policies::checksum::DecimalWidths {
                    price: pw,
                    quantity: qw,
                },
            ));
            decoded.push((which, p, q, pw, qw));
        }
        let timestamp = self
            .vars
            .get("clock")
            .map(integer)
            .transpose()?
            .unwrap_or(state.clock_ns);
        super::operations::FormattedLevelUpdate::<Price64> {
            timestamp,
            bids,
            asks,
            depth: usize::MAX,
        }
        .process_feed(state.book_mut(&symbol))
        .map_err(error)?;
        self.observer.record(|| {
            use super::observer::literal as v;
            let actions = decoded.iter().map(|&(side,price,quantity,_,_)| serde_json::json!({"action":"level","side":v(side),"price":v(price),"quantity":v(quantity)})).collect();
            Ok(super::observer::book(&symbol,actions,false,timestamp,state.synchronized(&symbol),0))
        });
        Ok(())
    }
    fn order_action(
        &mut self,
        action: &Action,
        state: &mut FeedState,
        item: &Value,
        root: &Value,
    ) -> Result<(), String> {
        use Operation::*;
        let symbol = self.current_symbol()?;
        if !state.scope.contains(&symbol) {
            return Ok(());
        }
        self.prepare_order::<false>(state, &symbol)?;
        let eval = |expr: &Expr| self.evaluate(expr, item, root);
        let number = |expr: &Expr| self.scalar(expr, item, root);
        match &action.operation {
            OrderCommand { value, timestamp } => {
                use crate::order_messages::ApplyFeedCommand;
                let input = Input {
                    item,
                    root,
                    vars: &self.vars,
                    tables: &self.tables,
                };
                let value = value.eval_ref(&input)?;
                let command = <lobo_models::server::Command as serde::Deserialize>::deserialize(
                    value.as_ref(),
                )
                .map_err(|e| e.to_string())?;
                let timestamp = number(timestamp)?;
                command
                    .route_feed(
                        state,
                        &symbol,
                        timestamp,
                        lobo_models::events::Reports::default(),
                    )
                    .map_err(|e| e.to_string())?;
                self.timestamp(state, timestamp);
            }
            RestoreOrder { value } => {
                let value = eval(value)?;
                let order: lobo_models::server::RestingOrderState =
                    serde_json::from_value(value).map_err(|e| e.to_string())?;
                let timestamp = order.created_at_ns;
                let order = order.into_order::<Price64>()?;
                crate::dispatch_feed_book!(state.book_mut(&symbol), book, {
                    let (storage, mut publish) = book.storage_and_publisher_at(timestamp);
                    storage.add_order(order, &mut publish).map_err(error)?;
                });
            }
            Add {
                id: oid,
                side: which,
                price,
                quantity,
                timestamp,
            } => {
                let message = AddOrder {
                    timestamp: number(timestamp)?,
                    id: id(eval(oid)?)?,
                    side: side(eval(which)?)?,
                    price: Price64::from(number(price)?),
                    quantity: number(quantity)?,
                };
                self.timestamp(state, message.timestamp);
                mutation::Mutation::Add(message).apply(state, &symbol)?;
                if self.descriptor.mode == FeedMode::Replay {
                    self.snapshots.insert(symbol.clone());
                    state.outputs[&symbol].borrow_mut().synchronized = true;
                    state.warming = false;
                }
            }
            Execute {
                id: oid,
                price,
                quantity,
                timestamp,
            } => {
                let message = {
                    let id = id(eval(oid)?)?;
                    let quantity = number(quantity)?;
                    let p = eval(price)?;
                    let timestamp = number(timestamp)?;
                    ExecuteOrder {
                        timestamp,
                        id,
                        quantity,
                        price: if p.is_null() {
                            None
                        } else {
                            Some(Price64::from(integer(&p)?))
                        },
                    }
                };
                mutation::Mutation::Execute(message).apply(state, &symbol)?;
            }
            Cancel {
                id: oid,
                quantity,
                timestamp,
            } => {
                let message = CancelOrder {
                    id: id(eval(oid)?)?,
                    quantity: number(quantity)?,
                    timestamp: number(timestamp)?,
                };
                mutation::Mutation::Cancel(message).apply(state, &symbol)?;
            }
            Remove { id: oid, timestamp } => {
                let message = RemoveOrder {
                    id: id(eval(oid)?)?,
                    timestamp: number(timestamp)?,
                };
                mutation::Mutation::Remove(message).apply(state, &symbol)?;
            }
            Replace {
                id: oid,
                new_id,
                price,
                quantity,
                timestamp,
            } => {
                let message = {
                    let id = id(eval(oid)?)?;
                    let new_id = id_fn(eval(new_id)?)?;
                    let timestamp = number(timestamp)?;
                    let quantity = number(quantity)?;
                    let price = number(price)?;
                    ReplaceOrder {
                        timestamp,
                        id,
                        new_id,
                        price: Price64::from(price),
                        quantity,
                    }
                };
                mutation::Mutation::Replace(message).apply(state, &symbol)?;
            }
            Modify {
                id: oid,
                quantity,
                timestamp,
            } => {
                let (id, quantity, timestamp) =
                    (id(eval(oid)?)?, number(quantity)?, number(timestamp)?);
                let order = state
                    .book(&symbol)
                    .and_then(|b| b.order(id))
                    .ok_or("Modify refers to missing order")?;
                let update = OrderUpdate {
                    timestamp,
                    id,
                    price: order.price(),
                    side: order.side(),
                    quantity,
                };
                let previous = Some(order.quantity());
                if let Some(branch) = state.simulation.as_mut() {
                    if branch.feed.selected == symbol {
                        self.reconciler.update(branch, update, previous)?;
                    }
                }
                update
                    .process_feed(state.book_mut(&symbol))
                    .map_err(error)?;
            }
            Upsert {
                id: oid,
                side: which,
                price,
                quantity,
                timestamp,
            } => {
                let update = {
                    let p = eval(price)?;
                    OrderUpdate {
                        timestamp: number(timestamp)?,
                        id: id(eval(oid)?)?,
                        side: side(eval(which)?)?,
                        price: if p.is_null() {
                            None
                        } else {
                            Some(Price64::from(integer(&p)?))
                        },
                        quantity: number(quantity)?,
                    }
                };
                let previous = state
                    .book(&symbol)
                    .and_then(|b| b.order(update.id))
                    .map(Trades::quantity);
                if let Some(branch) = state.simulation.as_mut() {
                    if branch.feed.selected == symbol {
                        self.reconciler.update(branch, update, previous)?;
                    }
                }
                update
                    .process_feed(state.book_mut(&symbol))
                    .map_err(error)?;
            }
            Level { .. } => return self.levels(state, &[item], root, action),
            TradeHistory { id: tid } => {
                let tid = number(tid)?;
                let previous = self.last_trade.entry(symbol.clone()).or_default();
                *previous = (*previous).max(tid);
            }
            Trade {
                id: tid,
                side: which,
                price,
                quantity,
                timestamp,
            } => {
                if !state.synchronized(&symbol) {
                    return Ok(());
                }
                let trade = PublicTrade {
                    timestamp: number(timestamp)?,
                    price: Price64::from(number(price)?),
                    quantity: number(quantity)?,
                    maker_side: side(eval(which)?)?,
                };
                if trade.quantity == 0 || u64::from(trade.price) == 0 {
                    return Err("Trade requires positive quantity and price".into());
                }
                let tid = {
                    let tid = eval(tid)?;
                    if tid.is_null() {
                        None
                    } else {
                        Some(integer(&tid)?)
                    }
                };
                if let Some(tid) = tid {
                    let previous = self.last_trade.entry(symbol.clone()).or_default();
                    if tid <= *previous {
                        return Ok(());
                    }
                    *previous = tid;
                }
                if let Some(branch) = state.simulation.as_mut() {
                    if branch.feed.selected == symbol {
                        self.reconciler.trade(branch, trade)?;
                    }
                }
                trade.process_feed(state.book_mut(&symbol)).map_err(error)?;
            }
            _ => return Err("Expected a book operation".into()),
        }
        let input = Input {
            item,
            root,
            vars: &self.vars,
            tables: &self.tables,
        };
        self.observer.record(|| {
            let action = super::observer::resolve(action, &input)?;
            Ok(super::observer::book(
                &symbol,
                vec![action],
                false,
                state.clock_ns,
                state.synchronized(&symbol),
                0,
            ))
        });
        Ok(())
    }
    fn packet(
        &mut self,
        state: &mut FeedState,
        connection: u32,
        bytes: &[u8],
    ) -> Result<(), String> {
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Message exceeds 8 MiB".into());
        }
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        {
            return self.program.clone().receive(self, state, bytes, connection);
        }
        #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
        {
            let value: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            let definition = self.definition.clone();
            self.bind("connection", connection.into());
            self.bind("clock", now().into());
            self.set_symbol(state, &state.selected.clone());
            let Format::Json { messages } = &definition.format else {
                return Err("JSON packet supplied to binary format".into());
            };
            for rule in messages {
                if self.evaluate(&rule.condition, &value, &value)?.as_bool() == Some(true) {
                    self.actions(&rule.actions, state, &value, &value)?;
                }
            }
            state.messages += 1;
            state.consumed += bytes.len() as u64;
            self.observer.commit(state)?;
            Ok(())
        }
    }
}
fn id_fn(value: Value) -> Result<Uuid, String> {
    id(value)
}
impl<O: super::observer::EventSink> Protocol for DefinedProtocol<O> {
    fn configure(&self, state: &mut FeedState) -> Result<(), String> {
        if let Some(prepared) = &self.checksum {
            state.context.set_checksum(prepared.clone())?;
        }
        Ok(())
    }

    fn submit_command(
        &mut self,
        state: &mut FeedState,
        symbol: &str,
        command: lobo_models::server::Command,
    ) -> Result<lobo_books::price_time_priority::CommandResult<Price64>, String> {
        use crate::order_messages::ApplyFeedCommand;
        if state.book(symbol).is_none() {
            return Err("Book does not exist".into());
        }
        let result = command
            .route_feed(
                state,
                symbol,
                state.clock_ns,
                lobo_models::events::Reports {
                    include_fills: true,
                    include_summary: false,
                    include_market_impact: false,
                },
            )
            .map_err(|e| e.to_string())?;
        self.observer.record(||Ok(super::observer::book(symbol,vec![serde_json::json!({"action":"order_command","value":super::observer::literal(command),"timestamp":super::observer::literal(state.clock_ns)})],false,state.clock_ns,state.synchronized(symbol),0)));
        state.messages += 1;
        state.commit();
        self.observer.commit(state)?;
        Ok(result)
    }
    fn register_instrument(
        &mut self,
        state: &mut FeedState,
        instrument: Instrument,
        policy: BookPolicy,
    ) -> Result<(), String> {
        let symbol = instrument.symbol.clone();
        state.register(instrument)?;
        state.context.set_policy(&symbol, policy)?;
        self.policies.insert(symbol, policy);
        Ok(())
    }
    fn connected(&mut self, state: &mut FeedState) -> Result<(), String> {
        self.connected_on(state, 0)
    }
    fn disconnected(&mut self, state: &mut FeedState) {
        self.disconnected_on(state, 0)
    }
    fn commands(&mut self, state: &mut FeedState) -> Vec<String> {
        self.commands_on(state, 0)
    }
    fn keepalive(&mut self, state: &mut FeedState) {
        self.keepalive_on(state, 0)
    }

    fn receive(&mut self, state: &mut FeedState, bytes: &[u8], eof: bool) -> Result<(), String> {
        if matches!(self.definition.format, Format::Json { .. }) {
            if !bytes.is_empty() {
                if let Err(e) = self.packet(state, 0, bytes) {
                    self.disconnected_on(state, 0);
                    self.observer.reject();
                    return Err(e);
                }
            }
            state.complete = eof;
            return Ok(());
        }
        if self.eof {
            return Err("Input is already complete".into());
        }
        self.input.drain(..self.cursor);
        self.cursor = 0;
        if self.input.len() + bytes.len() > 8 * 1024 * 1024 {
            return Err("Input buffer exceeds 8 MiB".into());
        }
        self.input.extend_from_slice(bytes);
        self.eof = eof;
        state.needs_input = false;
        Ok(())
    }
    fn advance(
        &mut self,
        state: &mut FeedState,
        elapsed_ns: u64,
        budget: usize,
    ) -> Result<(), String> {
        if matches!(self.definition.format, Format::Json { .. }) {
            if let Some(start) = state.start_ns {
                state.clock_ns = state.clock_ns.max(start.saturating_add(elapsed_ns));
            }
        }
        if let Some(branch) = state.simulation.as_mut() {
            self.reconciler.flush(branch, state.clock_ns)?;
        }
        let definition = self.definition.clone();
        #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
        let binary_plan = self.binary_plan.clone();
        let Format::Binary(binary) = &definition.format else {
            state.commit();
            return Ok(());
        };
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        let program = self.program.clone();
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        let framing = program
            .framing
            .as_ref()
            .expect("binary program has compiled framing");
        state.needs_input = false;
        for _ in 0..budget {
            let remaining = &self.input[self.cursor..];
            let prefix = binary.length.size;
            if remaining.len() < prefix {
                state.complete = self.eof && remaining.is_empty();
                if self.eof && !remaining.is_empty() {
                    return Err("Truncated record prefix".into());
                }
                state.needs_input = !self.eof;
                break;
            }
            #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
            let size = framing.length(remaining)?;
            #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
            let size = binary.payload_length(binary.length.number(remaining)?)?;
            if remaining.len() < prefix + size {
                if self.eof {
                    return Err("Truncated record body".into());
                }
                state.needs_input = true;
                break;
            }
            let bytes = &remaining[prefix..prefix + size];
            #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
            let header = framing.header(bytes)?;
            #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
            let timestamp = header.timestamp;
            #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
            let timestamp = binary.timestamp.number(bytes)?;
            if state
                .start_ns
                .is_some_and(|start| timestamp > start.saturating_add(elapsed_ns))
            {
                state.clock_ns = state.start_ns.unwrap_or(0).saturating_add(elapsed_ns);
                break;
            }
            #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
            let (tag, route) = (header.tag, header.key);
            #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
            let tag = binary.tag.number(bytes)?;
            #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
            let route = binary.key.number(bytes)?;
            #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
            {
                let pointer = bytes.as_ptr();
                let length = bytes.len();
                // The packet program mutates books and routing state; it never
                // mutates the transport input buffer during this borrowed call.
                let bytes = unsafe { std::slice::from_raw_parts(pointer, length) };
                program.record(self, state, bytes, timestamp, route, tag)?;
            }
            #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
            {
                if let Some((message, registration)) = binary_plan
                    .as_ref()
                    .map(|plan| plan.streaming(tag, bytes))
                    .transpose()?
                    .flatten()
                {
                    let new_registration = if let Some(registration) = registration {
                        if let Some(symbol) = self.routes.get(&route) {
                            if !symbol.eq_ignore_ascii_case(registration.symbol) {
                                return Err("Instrument routing key changed symbol".into());
                            }
                            None
                        } else {
                            Some((
                                Instrument {
                                    symbol: normalize_symbol(registration.symbol)?,
                                    price_decimals: registration.price_decimals,
                                    quantity_decimals: registration.quantity_decimals,
                                },
                                registration.policy,
                            ))
                        }
                    } else {
                        None
                    };
                    if let Some((instrument, policy)) = new_registration {
                        let symbol = instrument.symbol.clone();
                        if self
                            .route_keys
                            .get(&symbol)
                            .is_some_and(|key| *key != route)
                        {
                            return Err("Instrument has conflicting routing keys".into());
                        }
                        if !state.instruments.contains_key(&symbol) {
                            state.register(instrument)?;
                            state.context.set_policy(&symbol, policy)?;
                        }
                        self.policies.entry(symbol.clone()).or_insert(policy);
                        self.route_keys.insert(symbol.clone(), route);
                        self.routes.insert(route, symbol);
                    }
                    let symbol = self
                        .routes
                        .get(&route)
                        .ok_or("Order before instrument directory")?
                        .clone();
                    if state.scope.contains(&symbol) {
                        self.prepare_order::<true>(state, &symbol)?;
                        let mutation = message.mutation()?;
                        mutation.apply(state, &symbol)?;
                        state.start_ns.get_or_insert(timestamp);
                        self.snapshots.insert(symbol.clone());
                        state.outputs[&symbol].borrow_mut().synchronized = true;
                        state.warming = false;
                        self.observer.record(|| {
                            Ok(super::observer::book(
                                &symbol,
                                vec![mutation.event()],
                                false,
                                timestamp,
                                true,
                                0,
                            ))
                        });
                    }
                    self.cursor += prefix + size;
                    state.messages += 1;
                    state.consumed += (prefix + size) as u64;
                    if state.start_ns.is_some() {
                        state.clock_ns = timestamp;
                    }
                    continue;
                }
                self.vars.insert("key".into(), route.into());
                self.vars.insert("clock".into(), timestamp.into());
                if let Some(record) = binary.records.get(&tag) {
                    let object = record.decode(bytes, binary.minimum_header())?;
                    if let Some(symbol) = self.routes.get(&route).cloned() {
                        self.set_symbol(state, &symbol);
                    }
                    self.actions(&record.actions, state, &object, &object)?;
                }
            }
            self.cursor += prefix + size;
            state.messages += 1;
            state.consumed += (prefix + size) as u64;
            if state.start_ns.is_some() {
                state.clock_ns = timestamp;
            }
        }
        state.commit();
        if state.complete {
            state.warming = false;
        }
        self.observer.commit(state)?;
        Ok(())
    }
    fn buffered_bytes(&self) -> usize {
        self.input.len() - self.cursor
    }
    fn bootstrap_requests(&self) -> Vec<BootstrapRequest<'_>> {
        self.definition
            .bootstrap
            .iter()
            .map(|b| BootstrapRequest {
                id: &b.name,
                url: &b.url,
            })
            .collect()
    }
    fn bootstrap(&mut self, state: &mut FeedState, id: &str, bytes: &[u8]) -> Result<(), String> {
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        {
            return self.program.clone().bootstrap(self, state, id, bytes);
        }
        #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
        {
            let definition = self.definition.clone();
            let b = definition
                .bootstrap
                .iter()
                .find(|b| b.name == id)
                .ok_or("Unknown bootstrap")?;
            let value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            self.vars.insert("connection".into(), 0.into());
            self.actions(&b.actions, state, &value, &value)
        }
    }
    fn connections(&self, state: &FeedState) -> Option<Vec<FeedConnection<'_>>> {
        self.descriptor.endpoint.as_deref().map(|endpoint| {
            let ids = self
                .wanted
                .iter()
                .map(|symbol| self.group(symbol))
                .collect::<BTreeSet<_>>();
            ids.into_iter()
                .map(|id| FeedConnection {
                    id,
                    endpoint,
                    selected: id == self.group(&state.selected),
                })
                .collect()
        })
    }
    fn connected_on(&mut self, state: &mut FeedState, id: u32) -> Result<(), String> {
        self.bind("connection", id.into());
        self.sequences
            .retain(|(connection, _), _| *connection != id);
        self.sent.remove(&id);
        self.subscription_ready.remove(&id);
        self.commands.remove(&id);
        self.tables.remove(&id);
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        self.execution.disconnect(id);
        let definition = self.definition.clone();
        self.actions(&definition.connect, state, &Value::Null, &Value::Null)
    }
    fn receive_on(&mut self, state: &mut FeedState, id: u32, bytes: &[u8]) -> Result<(), String> {
        let result = self.packet(state, id, bytes);
        if result.is_err() {
            self.disconnected_on(state, id);
            self.observer.reject();
        }
        result
    }
    fn commands_on(&mut self, _: &mut FeedState, id: u32) -> Vec<String> {
        self.commands.remove(&id).unwrap_or_default()
    }
    fn subscribe(&mut self, state: &mut FeedState, symbol: &str) -> Result<(), String> {
        if self.wanted.insert(symbol.into()) {
            let capacity = self.definition.symbols_per_connection;
            let group = if capacity == 0 {
                0
            } else {
                self.assignments.len() / capacity
            };
            self.assignments.insert(symbol.into(), group as u32);
        }
        let connection = self.group(symbol);
        self.bind("connection", connection.into());
        if self.subscription_ready.contains(&connection) {
            self.subscribe_ready(state)?;
        }
        Ok(())
    }
    fn keepalive_on(&mut self, state: &mut FeedState, id: u32) {
        let definition = self.definition.clone();
        self.bind("connection", id.into());
        let _ = self.actions(&definition.keepalive, state, &Value::Null, &Value::Null);
    }
    fn simulation_note(&self) -> Option<&str> {
        self.definition.simulation_note.as_deref()
    }
    fn disconnected_on(&mut self, state: &mut FeedState, id: u32) {
        self.sent.remove(&id);
        let symbols = state
            .instruments
            .keys()
            .filter(|symbol| self.group(symbol) == id)
            .cloned()
            .collect::<Vec<_>>();
        for symbol in symbols {
            self.invalidate(state, &symbol);
        }
        self.sequences
            .retain(|(connection, _), _| *connection != id);
        state.warming = true;
    }
}
