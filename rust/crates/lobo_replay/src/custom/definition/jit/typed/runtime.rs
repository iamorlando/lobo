//! Statically linked primitives for generated programs. Field, path and
//! variable access is emitted as loads, so there are no such primitives here.
use super::{decode::Buffers, layout::Layout, storage::*};
use crate::{custom::definition::projection::OrderRecord, feed::FeedState};
use lobo_models::Side;
use lobo_primitives::{time::DateTime, uuid::Uuid};
use std::{
    collections::{BTreeMap, HashMap},
    ptr::NonNull,
    sync::Arc,
};

/// Owns the execution allocation without lending it through the protocol.
/// Generated callbacks borrow protocol metadata and execution slots separately;
/// a reference to the protocol must not also borrow the slot allocation.
pub(crate) struct ExecutionState(NonNull<State>);

impl ExecutionState {
    pub(super) fn new(state: State) -> Self {
        Self(NonNull::from(Box::leak(Box::new(state))))
    }
    pub(super) fn as_ptr(&self) -> *mut State {
        self.0.as_ptr()
    }
}
impl std::ops::Deref for ExecutionState {
    type Target = State;
    fn deref(&self) -> &State {
        // The owner keeps this allocation alive and has no cloning operation.
        unsafe { self.0.as_ref() }
    }
}
impl std::ops::DerefMut for ExecutionState {
    fn deref_mut(&mut self) -> &mut State {
        // Callers end this borrow before entering generated code.
        unsafe { self.0.as_mut() }
    }
}
impl Drop for ExecutionState {
    fn drop(&mut self) {
        unsafe { drop(Box::from_raw(self.0.as_ptr())) }
    }
}
// The protocol exclusively owns State, which is Send. Execution requires
// &mut DefinedProtocol; callbacks never outlive that synchronous invocation.
unsafe impl Send for ExecutionState {}

#[derive(Default)]
pub(super) struct Saved {
    pub record: Record,
    pub arena: Arena,
}
pub(crate) struct State {
    pub(super) variables: Vec<Record>,
    pub(super) owners: Vec<Arena>,
    pub(super) connections: BTreeMap<u32, Box<Vec<HashMap<Vec<u8>, Saved>>>>,
    pub(super) pool: Vec<Arena>,
    pub(super) buffers: Vec<Buffers>,
    pub(super) symbols: HashMap<Vec<u8>, Arc<str>>,
    pub(super) depth: usize,
    pub(super) frames: Vec<Box<Frame>>,
    pub(super) retired: Vec<Arena>,
}
impl State {
    pub(crate) fn disconnect(&mut self, id: u32) {
        self.connections.remove(&id);
    }
    pub(super) fn new(layout: &Layout, missing: Record) -> Self {
        Self {
            variables: vec![missing; layout.variables.len()],
            owners: (0..layout.variables.len())
                .map(|_| Arena::default())
                .collect(),
            connections: BTreeMap::new(),
            pool: Vec::new(),
            buffers: Vec::new(),
            symbols: HashMap::new(),
            depth: 0,
            frames: Vec::new(),
            retired: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct Slots {
    pub connection: usize,
    pub clock: usize,
    pub symbol: usize,
    pub price: usize,
    pub quantity: usize,
    pub key: usize,
}
impl Slots {
    pub(super) fn new(layout: &Layout) -> Self {
        let position = |name: &str| {
            layout
                .variables
                .keys()
                .position(|key| key == name)
                .expect("builtin layout")
        };
        Self {
            connection: position("connection"),
            clock: position("clock"),
            symbol: position("symbol"),
            price: position("price_decimals"),
            quantity: position("quantity_decimals"),
            key: position("key"),
        }
    }
}

pub(super) struct BookScope {
    pub symbol: Arc<str>,
    pub timestamp: u64,
}

#[repr(C)]
#[derive(Default)]
pub(super) struct Frame {
    pub failed: u64,
    pub item: usize,
    pub root: usize,
    pub variables: usize,
    pub output: usize,
    pub scale: u64,
    pub record: OrderRecord,
    pub owned_value: Record,
    pub own: Arena,
    pub temporary: Arena,
    pub retired: Vec<Arena>,
    pub buffers: Buffers,
    pub protocol: usize,
    pub state: usize,
    pub environment: usize,
    pub tables: usize,
    pub missing: usize,
    pub connection: u32,
    pub slots: Slots,
    pub symbol: Arc<str>,
    pub books: Vec<BookScope>,
    pub levels: Vec<(Side, u64, u64, u32, u32)>,
    pub error: Option<String>,
    pub concatenation: String,
    pub bytes: usize,
    pub length: usize,
    pub header: Header,
    pub directories: Vec<DirectoryScope>,
    pub sorting: Vec<SortRows>,
    pub message: String,
}
impl Frame {
    pub fn fail(&mut self, error: impl ToString) -> u64 {
        self.failed = 1;
        self.error = Some(error.to_string());
        0
    }
    pub fn missing(&self) -> Record {
        unsafe { *(self.missing as *const Record) }
    }
    pub fn save(&mut self, value: Record) -> u64 {
        self.temporary.put(value) as u64
    }
    pub fn uint_record(&self, value: u64) -> Record {
        Record {
            kind: NUMBER,
            present: 1,
            valid: VALID_NUMBER | VALID_UNSIGNED | VALID_ID,
            unsigned: value,
            identifier: Uuid::from_u128(u128::from(value)),
            number: Number {
                low: value,
                ..Default::default()
            },
            ..self.missing()
        }
    }
    pub fn set_uint(&mut self, index: usize, value: u64) {
        let value = self.uint_record(value);
        unsafe {
            (self.variables as *mut Record).add(index).write(value);
        }
    }
    pub fn set_text(&mut self, index: usize, value: &str) {
        let value = Record {
            kind: TEXT,
            text: Span {
                address: value.as_ptr() as usize,
                length: value.len(),
            },
            ..self.missing()
        };
        unsafe {
            (self.variables as *mut Record).add(index).write(value);
        }
    }
    pub fn timestamp(&mut self, state: &mut FeedState, value: u64) {
        self.set_uint(self.slots.clock, value);
        state.start_ns.get_or_insert(value);
        state.clock_ns = state.clock_ns.max(value);
    }
}

pub(super) fn number(value: &Record) -> Result<Number, &'static str> {
    if value.valid & VALID_NUMBER == 0 {
        return Err("Expected a number within exact storage");
    }
    Ok(value.number)
}
pub(super) fn integer(value: &Record) -> Result<u64, &'static str> {
    if value.valid & VALID_UNSIGNED == 0 {
        return Err("Expected an unsigned integer");
    }
    Ok(value.unsigned)
}
pub(super) fn text(value: &Record) -> Result<&str, &'static str> {
    if value.kind != TEXT {
        return Err("Expected text");
    }
    Ok(unsafe { value.text.text() })
}
pub(super) extern "C" fn unsigned(frame: &mut Frame, value: &Record) -> u64 {
    match integer(value) {
        Ok(value) => value,
        Err(error) => frame.fail(error),
    }
}
pub(super) extern "C" fn decimal(frame: &mut Frame, value: &Record, places: u64) -> u64 {
    match number(value).and_then(|n| {
        if n.negative != 0 {
            return Err("Expected a non-negative decimal");
        }
        frame.scale = n.scale.max(0) as u64;
        n.atoms(u8::try_from(places).map_err(|_| "Invalid decimal precision")?)
    }) {
        Ok(value) => value,
        Err(error) => frame.fail(error),
    }
}
pub(super) extern "C" fn absolute_decimal(frame: &mut Frame, value: &Record, places: u64) -> u64 {
    match number(value).and_then(|n| {
        frame.scale = n.scale.max(0) as u64;
        n.atoms(u8::try_from(places).map_err(|_| "Invalid decimal precision")?)
    }) {
        Ok(value) => value,
        Err(error) => frame.fail(error),
    }
}
pub(super) extern "C" fn make_number(frame: &mut Frame, value: u64) -> u64 {
    frame.save(frame.uint_record(value))
}
pub(super) extern "C" fn make_boolean(frame: &mut Frame, value: u64) -> u64 {
    frame.save(Record {
        kind: BOOL,
        present: 1,
        number: Number {
            low: value,
            ..Default::default()
        },
        ..frame.missing()
    })
}
pub(super) extern "C" fn absolute(frame: &mut Frame, value: &Record) -> u64 {
    if value.kind == TEXT {
        return frame.transformed_text(unsafe { value.text.text() }.trim_start_matches('-'));
    }
    match number(value) {
        Ok(mut n) => {
            n.negative = 0;
            let mut record = Record {
                kind: NUMBER,
                present: 1,
                ..frame.missing()
            };
            record.number(n);
            frame.save(record)
        }
        Err(error) => frame.fail(error),
    }
}
pub(super) extern "C" fn timestamp(frame: &mut Frame, value: &Record) -> u64 {
    match text(value)
        .map_err(str::to_owned)
        .and_then(|value| DateTime::parse_from_rfc3339(value).map_err(|e| e.to_string()))
        .and_then(|value| {
            value
                .timestamp_nanos_opt()
                .and_then(|n| u64::try_from(n).ok())
                .ok_or_else(|| "Timestamp out of range".into())
        }) {
        Ok(value) => value,
        Err(error) => frame.fail(error),
    }
}
pub(super) extern "C" fn identifier_word(value: u64, destination: &mut Uuid) -> u64 {
    *destination = Uuid::from_u128(u128::from(value));
    0
}
fn compare_numbers(a: Number, b: Number) -> Result<std::cmp::Ordering, &'static str> {
    let av = a.coefficient();
    let bv = b.coefficient();
    let an = a.negative != 0 && av != 0;
    let bn = b.negative != 0 && bv != 0;
    if an != bn {
        return Ok(bn.cmp(&an));
    }
    let order = if av == 0 || bv == 0 {
        av.cmp(&bv)
    } else if a.scale == b.scale {
        av.cmp(&bv)
    } else {
        let ad = av.ilog10() + 1;
        let bd = bv.ilog10() + 1;
        let magnitude = (i64::from(ad) - a.scale).cmp(&(i64::from(bd) - b.scale));
        if !magnitude.is_eq() {
            magnitude
        } else if ad < bd {
            let factor = 10u128.pow(bd - ad);
            av.cmp(&(bv / factor))
                .then_with(|| 0u128.cmp(&(bv % factor)))
        } else {
            let factor = 10u128.pow(ad - bd);
            (av / factor).cmp(&bv).then_with(|| (av % factor).cmp(&0))
        }
    };
    Ok(if an { order.reverse() } else { order })
}
pub(super) extern "C" fn equal(frame: &mut Frame, a: &Record, b: &Record) -> u64 {
    if a.kind != b.kind {
        return 0;
    }
    match a.kind {
        NULL => 1,
        BOOL => u64::from(a.number.low == b.number.low),
        TEXT => u64::from(unsafe { a.text.bytes() == b.text.bytes() }),
        NUMBER => match number(a).and_then(|a| number(b).and_then(|b| compare_numbers(a, b))) {
            Ok(order) => u64::from(order.is_eq()),
            Err(error) => frame.fail(error),
        },
        _ => u64::from(std::ptr::eq(a, b)),
    }
}
pub(super) extern "C" fn greater(frame: &mut Frame, a: &Record, b: &Record) -> u64 {
    if a.kind != NUMBER || b.kind != NUMBER {
        return frame.fail("Comparison requires numbers");
    }
    match number(a).and_then(|a| number(b).and_then(|b| compare_numbers(a, b))) {
        Ok(order) => u64::from(order.is_gt()),
        Err(error) => frame.fail(error),
    }
}
pub(super) extern "C" fn trim(frame: &mut Frame, value: &Record) -> u64 {
    match text(value) {
        Ok(text) => {
            let text = text.trim();
            frame.transformed_text(text)
        }
        Err(error) => frame.fail(error),
    }
}
pub(super) extern "C" fn strip(frame: &mut Frame, value: &Record, prefix: &Record) -> u64 {
    match text(value)
        .and_then(|value| text(prefix).map(|prefix| value.strip_prefix(prefix).unwrap_or(value)))
    {
        Ok(text) => frame.transformed_text(text),
        Err(error) => frame.fail(error),
    }
}
pub(super) extern "C" fn allocate(frame: &mut Frame, bytes: usize) -> u64 {
    frame.temporary.allocate::<u8>(bytes) as u64
}
pub(super) extern "C" fn own_begin(frame: &mut Frame) -> u64 {
    frame.owned_value = frame.missing();
    frame.own.reset();
    0
}
pub(super) extern "C" fn bind(frame: &mut Frame, index: usize) -> u64 {
    let state = unsafe { &mut *(frame.environment as *mut State) };
    state.variables[index] = frame.owned_value;
    let old = std::mem::replace(&mut state.owners[index], std::mem::take(&mut frame.own));
    frame.retired.push(old);
    frame.own = state.pool.pop().unwrap_or_default();
    0
}
pub(super) fn key<'a>(value: &'a Record) -> std::borrow::Cow<'a, [u8]> {
    if value.text.length != 0 {
        return std::borrow::Cow::Borrowed(unsafe { value.text.bytes() });
    }
    match value.kind {
        TEXT => std::borrow::Cow::Borrowed(b""),
        NULL => std::borrow::Cow::Borrowed(b"null"),
        BOOL if value.number.low != 0 => std::borrow::Cow::Borrowed(b"true"),
        BOOL => std::borrow::Cow::Borrowed(b"false"),
        _ => {
            let n = value.number;
            let sign = if n.negative == 0 { "" } else { "-" };
            std::borrow::Cow::Owned(if n.scale == 0 {
                format!("{sign}{}", n.coefficient()).into_bytes()
            } else {
                format!("{sign}{}e{}", n.coefficient(), -n.scale).into_bytes()
            })
        }
    }
}
pub(super) extern "C" fn lookup(frame: &mut Frame, table: usize, value: &Record) -> u64 {
    let tables = unsafe { &*(frame.tables as *const Vec<HashMap<Vec<u8>, Saved>>) };
    tables[table]
        .get(key(value).as_ref())
        .map_or(frame.missing as u64, |saved| {
            &saved.record as *const Record as u64
        })
}
pub(super) extern "C" fn remember(frame: &mut Frame, table: usize, value: &Record) -> u64 {
    let tables = unsafe { &mut *(frame.tables as *mut Vec<HashMap<Vec<u8>, Saved>>) };
    let saved = Saved {
        record: frame.owned_value,
        arena: std::mem::take(&mut frame.own),
    };
    let old = tables[table].insert(key(value).into_owned(), saved);
    if let Some(old) = old {
        frame.retired.push(old.arena);
    }
    let state = unsafe { &mut *(frame.environment as *mut State) };
    frame.own = state.pool.pop().unwrap_or_default();
    0
}
pub(super) extern "C" fn key_equal(value: &Record, expected: &String) -> u64 {
    u64::from(key(value).as_ref() == expected.as_bytes())
}
pub(super) extern "C" fn concat_start(frame: &mut Frame) -> u64 {
    frame.concatenation.clear();
    0
}
pub(super) extern "C" fn concat_append(frame: &mut Frame, value: &Record) -> u64 {
    let value = key(value);
    frame
        .concatenation
        .push_str(unsafe { std::str::from_utf8_unchecked(value.as_ref()) });
    0
}
pub(super) extern "C" fn concat_finish(frame: &mut Frame) -> u64 {
    let text = frame.temporary.bytes(frame.concatenation.as_bytes());
    let mut value = Record {
        kind: TEXT,
        present: 1,
        text,
        ..frame.missing()
    };
    value.text_projection::<true, true, true>();
    frame.save(value)
}
pub(super) extern "C" fn fail(frame: &mut Frame, message: &String) -> u64 {
    frame.fail(message)
}
pub(super) fn symbols() -> Vec<(&'static str, *const u8, usize)> {
    let mut symbols = Vec::new();
    macro_rules! primitive {
        ($name:ident, $arity:expr) => {
            symbols.push((
                concat!("typed_", stringify!($name)),
                $name as *const u8,
                $arity,
            ));
        };
    }
    primitive!(exists, 1);
    primitive!(unsigned, 2);
    primitive!(decimal, 3);
    primitive!(absolute_decimal, 3);
    primitive!(make_number, 2);
    primitive!(make_boolean, 2);
    primitive!(absolute, 2);
    primitive!(timestamp, 2);
    primitive!(identifier_word, 2);
    primitive!(equal, 3);
    primitive!(greater, 3);
    primitive!(trim, 2);
    primitive!(strip, 3);
    primitive!(allocate, 2);
    primitive!(own_begin, 1);
    primitive!(bind, 2);
    primitive!(lookup, 3);
    primitive!(remember, 3);
    primitive!(key_equal, 2);
    primitive!(concat_start, 1);
    primitive!(concat_append, 2);
    primitive!(concat_finish, 1);
    primitive!(fail, 2);
    symbols
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Header {
    pub timestamp: u64,
    pub key: u64,
    pub tag: u64,
}
#[derive(Default)]
pub(super) struct DirectoryScope {
    pub symbols: std::collections::BTreeSet<String>,
    pub removed: Vec<String>,
}
#[derive(Default)]
pub(super) struct SortRows {
    pub rows: Vec<(usize, Option<u64>, Vec<u8>)>,
    pub unique: std::collections::HashSet<Vec<u8>>,
}
impl Frame {
    pub fn symbol(&mut self, symbol: &str, state: &FeedState) {
        let environment = unsafe { &mut *(self.environment as *mut State) };
        self.symbol = if let Some(symbol) = environment.symbols.get(symbol.as_bytes()) {
            symbol.clone()
        } else {
            let value: Arc<str> = Arc::from(symbol);
            environment
                .symbols
                .insert(symbol.as_bytes().to_vec(), value.clone());
            value
        };
        let symbol = self.symbol.clone();
        self.set_text(self.slots.symbol, &symbol);
        if let Some(instrument) = state.instruments.get(symbol.as_ref()) {
            self.set_uint(self.slots.price, u64::from(instrument.price_decimals));
            self.set_uint(self.slots.quantity, u64::from(instrument.quantity_decimals));
        }
    }
}
pub(super) extern "C" fn exists(value: &Record) -> u64 {
    u64::from(value.kind != NULL)
}

impl Frame {
    fn transformed_text(&mut self, text: &str) -> u64 {
        let mut value = Record {
            kind: TEXT,
            present: 1,
            text: Span {
                address: text.as_ptr() as usize,
                length: text.len(),
            },
            ..self.missing()
        };
        value.text_projection::<true, true, true>();
        self.save(value)
    }
}
