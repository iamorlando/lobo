//! Statically selected feed operations. Generated code supplies typed operands.
use super::{
    runtime::{self, BookScope, DirectoryScope, Frame, SortRows, State, integer, key, text},
    storage::*,
};
use crate::{
    custom::{
        AdaptToFeed, FeedMessage, Protocol,
        definition::{DefinedProtocol, projection::OrderRecord, schema::Checksum},
        observer::EventSink,
        operations::{LevelUpdate, PublicTrade},
        orders::*,
        upsert::OrderUpdate,
    },
    feed::{FeedState, Instrument, normalize_symbol},
};
use lobo_models::{BookPolicy, Side, orders::traits::Trades};
use lobo_primitives::Price64;
use std::sync::Arc;

pub(super) unsafe fn host<O: EventSink>(
    run: *mut Frame,
    f: impl FnOnce(&mut DefinedProtocol<O>, &mut FeedState, &mut Frame) -> Result<u64, String>,
) -> u64 {
    let r = unsafe { &mut *run };
    let p = unsafe { &mut *(r.protocol as *mut DefinedProtocol<O>) };
    let s = unsafe { &mut *(r.state as *mut FeedState) };
    match f(p, s, r) {
        Ok(value) => value,
        Err(error) => r.fail(error),
    }
}
pub(super) unsafe extern "C" fn directory_complete<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, _, _| {
            p.directory_complete = true;
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn register<O: EventSink>(
    run: *mut Frame,
    symbol: &Record,
    price: u64,
    quantity: u64,
    route: &Record,
    policy: &Record,
) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let symbol = normalize_symbol(text(symbol)?)?;
            let price_decimals = u8::try_from(price).map_err(|_| "Invalid price precision")?;
            let quantity_decimals =
                u8::try_from(quantity).map_err(|_| "Invalid quantity precision")?;
            let policy = match text(policy).ok() {
                Some("full") => BookPolicy::Full,
                Some("no_user_map") => BookPolicy::NoUserMap,
                Some("no_hidden_quantity") => BookPolicy::NoHiddenQuantity,
                Some("no_updates") => BookPolicy::NoUpdates,
                _ => return Err("Invalid book policy".into()),
            };
            let is_new = !s.instruments.contains_key(&symbol);
            s.register(Instrument {
                symbol: symbol.clone(),
                price_decimals,
                quantity_decimals,
            })?;
            if is_new {
                s.context.set_policy(&symbol, policy)?;
            }
            p.policies.entry(symbol.clone()).or_insert(policy);
            if route.kind != NULL {
                let route = integer(route)?;
                if p.routes.get(&route).is_some_and(|old| old != &symbol) {
                    return Err("Instrument routing key changed symbol".into());
                }
                if p.route_keys.get(&symbol).is_some_and(|old| *old != route) {
                    return Err("Instrument has conflicting routing keys".into());
                }
                p.routes.insert(route, symbol.clone());
                p.route_keys.insert(symbol.clone(), route);
                r.symbol(&symbol, s);
            }
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn directory_remove_begin<O: EventSink>(
    run: *mut Frame,
    index: u64,
) -> u64 {
    unsafe {
        host::<O>(run, |_p, s, r| {
            let old = r
                .directories
                .last()
                .unwrap_unchecked()
                .removed
                .get_unchecked(index as usize);
            let old = old.clone();
            r.symbol(&old, s);
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn directory_remove_end<O: EventSink>(
    run: *mut Frame,
    index: u64,
) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let old = r
                .directories
                .last()
                .unwrap_unchecked()
                .removed
                .get_unchecked(index as usize);
            s.invalidate_orders(old);
            s.instruments.remove(old);
            s.context.books.remove(&old.to_ascii_lowercase());
            s.outputs.remove(old);
            p.policies.remove(old);
            if let Some(route) = p.route_keys.remove(old) {
                p.routes.remove(&route);
            }
            p.snapshots.remove(old);
            p.last_trade.remove(old);
            p.sequences.retain(|(_, symbol), _| symbol != old);
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn directory_end<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let scope = r.directories.pop().unwrap_unchecked();
            if !scope.removed.is_empty() {
                p.observer.reject();
            }
            if !s.instruments.contains_key(&s.selected) {
                if let Some(symbol) = s
                    .instruments
                    .keys()
                    .find(|symbol| s.scope.contains(symbol))
                    .cloned()
                {
                    s.selected = symbol;
                }
            }
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn book_begin<O: EventSink>(
    run: *mut Frame,
    symbol: &Record,
    snapshot: u64,
    timestamp: u64,
) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let symbol = normalize_symbol(text(symbol)?)?;
            if !s.scope.contains(&symbol) {
                return Ok(0);
            }
            let instrument = s
                .instruments
                .get(&symbol)
                .ok_or("Book arrived before its instrument definition")?;
            if s.book(&symbol).is_none() {
                let policy = p.policies.get(&symbol).copied().unwrap_or_default();
                p.observer.record(|| {
                    Ok(crate::custom::observer::register(
                        &symbol,
                        instrument.price_decimals,
                        instrument.quantity_decimals,
                        policy,
                    ))
                });
            }
            if snapshot == 0 && !p.snapshots.contains(&symbol) {
                return Ok(0);
            }
            if snapshot != 0 {
                p.invalidate(s, &symbol);
                p.snapshots.insert(symbol.clone());
                s.book_mut(&symbol);
            }
            r.symbol(&symbol, s);
            r.timestamp(s, timestamp);
            r.books.push(BookScope {
                symbol: r.symbol.clone(),
                timestamp,
            });
            Ok(1)
        })
    }
}
pub(super) unsafe extern "C" fn book_depth<O: EventSink>(run: *mut Frame, depth: u64) -> u64 {
    unsafe {
        host::<O>(run, |_p, s, r| {
            let scope = r.books.last().unwrap_unchecked();
            LevelUpdate::<Price64> {
                timestamp: scope.timestamp,
                bids: vec![],
                asks: vec![],
                depth: depth as usize,
            }
            .process_feed(s.book_mut(scope.symbol.as_ref()))
            .map_err(crate::custom::definition::error)?;
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn book_finish<O: EventSink, const READY: bool>(
    run: *mut Frame,
    depth: u64,
) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let scope = r.books.pop().unwrap_unchecked();
            if READY {
                if let Some(output) = s.outputs.get(scope.symbol.as_ref()) {
                    output.borrow_mut().synchronized = true;
                }
            }
            let synchronized = s.synchronized(scope.symbol.as_ref());
            p.observer.record(|| {
                Ok(crate::custom::observer::book(
                    scope.symbol.as_ref(),
                    vec![],
                    false,
                    scope.timestamp,
                    synchronized,
                    depth as usize,
                ))
            });
            if synchronized {
                s.commit();
            }
            s.warming = !s.synchronized(&s.selected);
            Ok(0)
        })
    }
}
pub(super) extern "C" fn book_skip(run: &mut Frame) -> u64 {
    run.books.pop();
    0
}

pub(super) unsafe extern "C" fn order_begin<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let symbol = &*(r.variables as *const Record).add(r.slots.symbol);
            r.symbol(text(symbol)?, s);
            if !s.scope.contains(r.symbol.as_ref()) {
                return Ok(0);
            }
            p.prepare_order::<false>(s, r.symbol.as_ref())?;
            Ok(1)
        })
    }
}
pub(super) unsafe extern "C" fn apply_add<O: EventSink, const REPLAY: bool, const BIND: bool>(
    run: *mut Frame,
) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let d = r.record;
            let m = AddOrder {
                timestamp: d.timestamp,
                id: d.id,
                side: d.side,
                price: Price64::from(d.price),
                quantity: d.quantity,
            };
            if BIND {
                r.timestamp(s, d.timestamp);
            } else {
                s.start_ns.get_or_insert(d.timestamp);
                s.clock_ns = s.clock_ns.max(d.timestamp);
            }
            m.route(s, r.symbol.as_ref())
                .map_err(crate::custom::definition::error)?;
            if REPLAY {
                p.snapshots.insert(r.symbol.to_string());
                s.outputs[r.symbol.as_ref()].borrow_mut().synchronized = true;
                s.warming = false;
            }
            p.observer.record(|| {
                Ok(crate::custom::observer::book(
                    r.symbol.as_ref(),
                    vec![crate::custom::definition::mutation::Mutation::Add(m).event()],
                    false,
                    s.clock_ns,
                    s.synchronized(r.symbol.as_ref()),
                    0,
                ))
            });
            Ok(0)
        })
    }
}
macro_rules! mutation {
    ($name:ident,$variant:ident,$message:ident,$($field:ident : $value:expr),* $(,)?)=>{
        pub(super) unsafe extern "C" fn $name<O:EventSink>(run:*mut Frame)->u64 {
            unsafe {host::<O>(run,|p,s,r|{
                let m=$message{$($field:($value)(&r.record)),*};
                m.route(s,r.symbol.as_ref()).map_err(crate::custom::definition::error)?;
                p.observer.record(||Ok(crate::custom::observer::book(r.symbol.as_ref(),vec![crate::custom::definition::mutation::Mutation::$variant(m).event()],false,s.clock_ns,s.synchronized(r.symbol.as_ref()),0)));
                Ok(0)
            })}
        }
    }
}
mutation!(apply_execute,Execute,ExecuteOrder,timestamp:|d:&OrderRecord|d.timestamp,id:|d:&OrderRecord|d.id,quantity:|d:&OrderRecord|d.quantity,price:|d:&OrderRecord|d.price());
mutation!(apply_cancel,Cancel,CancelOrder,timestamp:|d:&OrderRecord|d.timestamp,id:|d:&OrderRecord|d.id,quantity:|d:&OrderRecord|d.quantity);
mutation!(apply_remove,Remove,RemoveOrder,timestamp:|d:&OrderRecord|d.timestamp,id:|d:&OrderRecord|d.id);
mutation!(apply_replace,Replace,ReplaceOrder,timestamp:|d:&OrderRecord|d.timestamp,id:|d:&OrderRecord|d.id,new_id:|d:&OrderRecord|d.new_id,quantity:|d:&OrderRecord|d.quantity,price:|d:&OrderRecord|Price64::from(d.price));

pub(super) unsafe extern "C" fn apply_update<O: EventSink, const MODIFY: bool>(
    run: *mut Frame,
) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let d = r.record;
            let old = s.book(r.symbol.as_ref()).and_then(|b| b.order(d.id));
            let previous = old.map(Trades::quantity);
            let (price, side) = if MODIFY {
                let old = old.ok_or("Modify refers to missing order")?;
                (old.price(), old.side())
            } else {
                (d.price(), d.side)
            };
            let update = OrderUpdate {
                timestamp: d.timestamp,
                id: d.id,
                price,
                side,
                quantity: d.quantity,
            };
            if let Some(branch) = s.simulation.as_mut() {
                if branch.feed.selected == r.symbol.as_ref() {
                    p.reconciler.update(branch, update, previous)?;
                }
            }
            update
                .process_feed(s.book_mut(r.symbol.as_ref()))
                .map_err(crate::custom::definition::error)?;
            p.observer.record(||{
            use crate::custom::observer::literal as v;
            let action=serde_json::json!({"action":"upsert","id":v(d.id),"side":v(side),"price":v(price.map(u64::from)),"quantity":v(d.quantity),"timestamp":v(d.timestamp)});
            Ok(crate::custom::observer::book(r.symbol.as_ref(),vec![action],false,s.clock_ns,s.synchronized(r.symbol.as_ref()),0))
        });
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn trade_ready<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |_, s, r| {
            Ok(u64::from(s.synchronized(r.symbol.as_ref())))
        })
    }
}
pub(super) unsafe extern "C" fn apply_history<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let previous = p.last_trade.entry(r.symbol.to_string()).or_default();
            *previous = (*previous).max(r.record.trade_id);
            p.observer.record(||Ok(crate::custom::observer::book(r.symbol.as_ref(),vec![serde_json::json!({"action":"trade_history","id":crate::custom::observer::literal(r.record.trade_id)})],false,s.clock_ns,s.synchronized(r.symbol.as_ref()),0)));
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn apply_trade<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let d = r.record;
            if d.has_trade_id != 0 {
                let last = p.last_trade.entry(r.symbol.to_string()).or_default();
                if d.trade_id <= *last {
                    return Ok(0);
                }
                *last = d.trade_id;
            }
            let trade = PublicTrade {
                timestamp: d.timestamp,
                price: Price64::from(d.price),
                quantity: d.quantity,
                maker_side: d.side,
            };
            if let Some(branch) = s.simulation.as_mut() {
                if branch.feed.selected == r.symbol.as_ref() {
                    p.reconciler.trade(branch, trade)?;
                }
            }
            trade
                .process_feed(s.book_mut(r.symbol.as_ref()))
                .map_err(crate::custom::definition::error)?;
            p.observer.record(||{
            use crate::custom::observer::literal as v;
            let action=serde_json::json!({"action":"trade","id":v((d.has_trade_id!=0).then_some(d.trade_id)),"side":v(d.side),"price":v(d.price),"quantity":v(d.quantity),"timestamp":v(d.timestamp)});
            Ok(crate::custom::observer::book(r.symbol.as_ref(),vec![action],false,s.clock_ns,s.synchronized(r.symbol.as_ref()),0))
        });
            Ok(0)
        })
    }
}
pub(super) extern "C" fn level_collect(run: &mut Frame) -> u64 {
    let d = run.record;
    run.levels.push((
        d.side,
        d.price,
        d.quantity,
        d.price_width as u32,
        d.quantity_width as u32,
    ));
    0
}
pub(super) unsafe extern "C" fn levels_flush<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            if r.levels.is_empty() {
                return Ok(0);
            }
            let mut bids = Vec::new();
            let mut asks = Vec::new();
            for &(side, price, quantity, pw, qw) in &r.levels {
                (if side == Side::Buy {
                    &mut bids
                } else {
                    &mut asks
                })
                .push((
                    Price64::from(price),
                    quantity,
                    lobo_storage::policies::checksum::DecimalWidths {
                        price: pw,
                        quantity: qw,
                    },
                ));
            }
            let timestamp = (*(r.variables as *const Record).add(r.slots.clock))
                .number
                .low;
            crate::custom::operations::FormattedLevelUpdate::<Price64> {
                timestamp,
                bids,
                asks,
                depth: usize::MAX,
            }
            .process_feed(s.book_mut(r.symbol.as_ref()))
            .map_err(crate::custom::definition::error)?;
            p.observer.record(||{
            use crate::custom::observer::literal as v;
            let actions=r.levels.iter().map(|&(side,price,quantity,_,_)|serde_json::json!({"action":"level","side":v(side),"price":v(price),"quantity":v(quantity)})).collect();
            Ok(crate::custom::observer::book(r.symbol.as_ref(),actions,false,timestamp,s.synchronized(r.symbol.as_ref()),0))
        });
            r.levels.clear();
            Ok(0)
        })
    }
}

pub(super) unsafe extern "C" fn packet_start<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |_p, s, r| {
            r.set_uint(r.slots.connection, u64::from(r.connection));
            r.set_uint(r.slots.clock, crate::custom::definition::now());
            r.symbol(&s.selected, s);
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn packet_finish<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            s.messages += 1;
            s.consumed += r.length as u64;
            p.observer.commit(s)?;
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn send<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, _, r| {
            p.commands
                .entry(r.connection)
                .or_default()
                .push(std::mem::take(&mut r.message));
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn subscribe<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            p.bind("connection", r.connection.into());
            p.subscribe_scope(s)?;
            p.bind("connection", r.connection.into());
            p.subscription_ready.insert(r.connection);
            p.subscribe_ready(s)?;
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn sequence<O: EventSink, const RESET: bool>(
    run: *mut Frame,
    next: u64,
    value: &Record,
) -> u64 {
    unsafe {
        host::<O>(run, |p, _, r| {
            let key = String::from_utf8(key(value).into_owned()).map_err(|e| e.to_string())?;
            let last = p
                .sequences
                .entry((r.connection, key))
                .or_insert(next.saturating_sub(1));
            if !RESET && last.checked_add(1) != Some(next) {
                return Err("Sequence gap; a fresh snapshot is required".into());
            }
            *last = next;
            Ok(0)
        })
    }
}
pub(super) unsafe extern "C" fn checksum<O: EventSink, const SIGNED: bool>(
    run: *mut Frame,
    config: &Checksum,
    expected: &Record,
) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let symbol = r.symbol.clone();
            let checksum = s
                .context
                .get_mut(&symbol)
                .ok_or("Checksum arrived before snapshot")?
                .checksum_with(&config.prepared)
                .map_err(str::to_owned)?;
            let n = runtime::number(expected).map_err(str::to_owned)?;
            let actual = if SIGNED {
                (checksum as i32 as i64).unsigned_abs()
            } else {
                u64::from(checksum)
            };
            let negative = SIGNED && (checksum as i32) < 0;
            let valid = n.atoms(0).ok() == Some(actual) && (n.negative != 0) == negative;
            s.checksum_checks += 1;
            if !valid {
                s.checksum_failures += 1;
                p.invalidate(s, &symbol);
                s.warming = !s.synchronized(&s.selected);
            }
            Ok(u64::from(valid))
        })
    }
}
pub(super) extern "C" fn directory_start(r: &mut Frame) -> u64 {
    r.directories.push(DirectoryScope::default());
    0
}
pub(super) extern "C" fn directory_symbol(r: &mut Frame, value: &Record) -> u64 {
    match text(value)
        .map_err(str::to_owned)
        .and_then(normalize_symbol)
    {
        Ok(symbol) => {
            unsafe { r.directories.last_mut().unwrap_unchecked() }
                .symbols
                .insert(symbol);
            0
        }
        Err(error) => r.fail(error),
    }
}
pub(super) unsafe extern "C" fn directory_begin<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |_, s, r| {
            let scope = r.directories.last_mut().unwrap_unchecked();
            scope.removed = s
                .instruments
                .keys()
                .filter(|s| !scope.symbols.contains(*s))
                .cloned()
                .collect();
            Ok(scope.removed.len() as u64)
        })
    }
}
pub(super) extern "C" fn sort_start(r: &mut Frame) -> u64 {
    r.sorting.push(SortRows::default());
    0
}
pub(super) extern "C" fn sort_push(
    r: &mut Frame,
    row: usize,
    order: &Record,
    unique: &Record,
) -> u64 {
    let rows = unsafe { r.sorting.last_mut().unwrap_unchecked() };
    if unique as *const Record as usize != r.missing
        && !rows.unique.insert(key(unique).into_owned())
    {
        return r.fail("Duplicate key in snapshot");
    }
    rows.rows
        .push((row, integer(order).ok(), key(order).into_owned()));
    0
}
pub(super) extern "C" fn sort_finish<const SORT: bool>(r: &mut Frame) -> u64 {
    let rows = unsafe { r.sorting.last_mut().unwrap_unchecked() };
    if SORT {
        rows.rows.sort_by(|a, b| match (a.1, b.1) {
            (Some(a), Some(b)) => a.cmp(&b),
            _ => a.2.cmp(&b.2),
        });
    }
    rows.rows.len() as u64
}
pub(super) extern "C" fn sort_row(r: &mut Frame, index: usize) -> u64 {
    unsafe {
        r.sorting
            .last()
            .unwrap_unchecked()
            .rows
            .get_unchecked(index)
            .0 as u64
    }
}
pub(super) extern "C" fn sort_end(r: &mut Frame) -> u64 {
    r.sorting.pop();
    0
}
pub(super) extern "C" fn message_start(r: &mut Frame) -> u64 {
    r.message.clear();
    0
}
pub(super) extern "C" fn message_text(r: &mut Frame, text: &String) -> u64 {
    r.message.push_str(text);
    0
}
pub(super) extern "C" fn message_scalar(r: &mut Frame, value: &Record) -> u64 {
    match value.kind {
        NULL => r.message.push_str("null"),
        BOOL => r.message.push_str(if value.number.low == 0 {
            "false"
        } else {
            "true"
        }),
        TEXT => match serde_json::to_string(unsafe { value.text.text() }) {
            Ok(text) => r.message.push_str(&text),
            Err(e) => return r.fail(e),
        },
        NUMBER => {
            if value.text.length != 0 {
                r.message.push_str(unsafe { value.text.text() });
            } else {
                use std::fmt::Write;
                let n = value.number;
                if n.negative != 0 {
                    r.message.push('-');
                }
                if n.scale == 0 {
                    let _ = write!(r.message, "{}", n.coefficient());
                } else {
                    let _ = write!(r.message, "{}e{}", n.coefficient(), -n.scale);
                }
            }
        }
        _ => return r.fail("Expected scalar message value"),
    }
    0
}
pub(super) unsafe extern "C" fn binary_start<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            r.set_uint(r.slots.key, r.header.key);
            r.set_uint(r.slots.clock, r.header.timestamp);
            if let Some(symbol) = p.routes.get(&r.header.key) {
                r.symbol(symbol, s);
            }
            Ok(0)
        })
    }
}
pub(super) extern "C" fn binary_size(r: &mut Frame, size: usize) -> u64 {
    if r.length != size {
        return r.fail("Binary record has an unexpected length");
    }
    0
}
pub(super) extern "C" fn binary_text(r: &mut Frame, offset: usize, length: usize) -> u64 {
    let bytes = unsafe { std::slice::from_raw_parts((r.bytes as *const u8).add(offset), length) };
    match std::str::from_utf8(bytes) {
        Ok(text) => {
            let text = text.trim();
            r.save(Record {
                kind: TEXT,
                text: Span {
                    address: text.as_ptr() as usize,
                    length: text.len(),
                },
                ..r.missing()
            })
        }
        Err(error) => r.fail(error),
    }
}

pub(super) unsafe extern "C" fn binary_fast_start<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, _, r| {
            let Some(symbol) = p.routes.get(&r.header.key) else {
                return Ok(0);
            };
            let environment = &mut *(r.environment as *mut State);
            r.symbol = if let Some(symbol) = environment.symbols.get(symbol.as_bytes()) {
                symbol.clone()
            } else {
                let value: Arc<str> = Arc::from(symbol.as_str());
                environment
                    .symbols
                    .insert(symbol.as_bytes().to_vec(), value.clone());
                value
            };
            Ok(1)
        })
    }
}
pub(super) unsafe extern "C" fn binary_order<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            if r.symbol.is_empty() {
                return Err("Order before instrument directory".into());
            }
            if !s.scope.contains(&r.symbol) {
                return Ok(0);
            }
            p.prepare_order::<true>(s, &r.symbol)?;
            Ok(1)
        })
    }
}

pub(super) extern "C" fn message_raw(r: &mut Frame, value: &Record) -> u64 {
    match std::str::from_utf8(unsafe { value.raw.bytes() }) {
        Ok(text) => {
            r.message.push_str(text);
            0
        }
        Err(error) => r.fail(error),
    }
}
