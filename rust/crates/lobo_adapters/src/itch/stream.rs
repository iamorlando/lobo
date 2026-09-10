//! Incremental ITCH adapter, usable by native and browser consumers.
use crate::adapter::market::FeedMessage;
use crate::{
    adapter::{AdapterInfo, FeedMode, FeedState, Instrument, MarketDataAdapter},
    itch::messages::{ItchAdd, ItchCancel, ItchDelete, ItchExecute, ItchReplace},
};
use std::collections::HashMap;
pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
pub const INFO: AdapterInfo = AdapterInfo {
    id: "itch",
    name: "NASDAQ ITCH 5.0",
    mode: FeedMode::Replay,
    endpoint: None,
    default_symbol: "AAPL",
    timezone: "ET",
    supports_trades: true,
    level: crate::adapter::market::BookLevel::L3,
};
/// Streaming browser context. Its ticker directory is populated from the same
/// ITCH StockDirectory entries used by ItchReplaySource::from_file_tickers.
pub struct ItchStream {
    live_orders: HashMap<u16, HashMap<u64, u32>>,
    symbols: HashMap<u16, String>,
    routes: HashMap<u16, String>,
    state: FeedState,
    input: Vec<u8>,
    cursor: usize,
    eof: bool,
    requested_start: u64,
}

impl ItchStream {
    pub fn new(ticker: &str, start_ns: u64) -> Result<Self, String> {
        Ok(Self {
            live_orders: HashMap::new(),
            symbols: HashMap::new(),
            routes: HashMap::new(),
            state: FeedState::new(ticker)?,
            input: Vec::new(),
            cursor: 0,
            eof: false,
            requested_start: start_ns,
        })
    }
    fn register(&mut self, locate: u16, bytes: &[u8]) -> Result<(), String> {
        let ticker = std::str::from_utf8(bytes)
            .map_err(|_| "Invalid ITCH ticker encoding")?
            .trim()
            .to_ascii_uppercase();
        if ticker.is_empty() || ticker.len() > 8 || !ticker.bytes().all(|c| c.is_ascii_graphic()) {
            return Err("Invalid ITCH ticker".into());
        }
        if let Some(existing) = self.symbols.get(&locate) {
            if existing != &ticker {
                return Err("Stock locate changed ticker within the session".into());
            }
            return Ok(());
        }
        if self.state.instruments.contains_key(&ticker) {
            return Err("Ticker has conflicting stock locates".into());
        }
        self.state.register(Instrument {
            symbol: ticker.clone(),
            price_decimals: 4,
            quantity_decimals: 0,
        })?;
        if self.state.scope.contains(&ticker) {
            self.routes.insert(locate, ticker.clone());
        }
        self.symbols.insert(locate, ticker);
        Ok(())
    }
    pub fn buffered(&self) -> usize {
        self.input.len() - self.cursor
    }
    pub fn append(&mut self, bytes: &[u8], eof: bool) -> Result<(), String> {
        if self.eof {
            return Err("Input is already complete".into());
        }
        if self.buffered() + bytes.len() > MAX_INPUT_BYTES {
            return Err("Input queue exceeds 8 MiB; consume before appending".into());
        }
        self.input.drain(..self.cursor);
        self.cursor = 0;
        self.input.extend_from_slice(bytes);
        self.eof = eof;
        self.state.needs_input = false;
        Ok(())
    }
    /// Consume at most `budget` records, stopping before the first future record.
    /// Nanoseconds are exact within an ITCH trading day (also within JS's 53 bits).
    pub fn advance(&mut self, elapsed_ns: u64, budget: usize) -> Result<(), String> {
        self.state.needs_input = false;
        for _ in 0..budget {
            let remaining = self.buffered();
            if remaining < 2 {
                if self.eof && remaining != 0 {
                    return Err("Truncated ITCH record length".into());
                }
                self.state.complete = self.eof;
                self.state.needs_input = !self.eof;
                if self.state.complete && self.state.start_ns.is_none() {
                    return Err("No orders found in the file".into());
                }
                break;
            }
            let len =
                u16::from_be_bytes(self.input[self.cursor..self.cursor + 2].try_into().unwrap())
                    as usize;
            if !(11..=512).contains(&len) {
                return Err(format!(
                    "Invalid ITCH record length {len} at byte {} (use uncompressed ITCH 5.0)",
                    self.state.consumed
                ));
            }
            if remaining < len + 2 {
                if self.eof {
                    return Err("Truncated ITCH record body".into());
                }
                self.state.needs_input = true;
                break;
            }
            let begin = self.cursor + 2;
            let record = &self.input[begin..begin + len];
            let timestamp = record[5..11].iter().fold(0u64, |n, b| (n << 8) | *b as u64);
            if let Some(start) = self.state.start_ns {
                let target = start.saturating_add(elapsed_ns);
                if timestamp > target {
                    self.state.clock_ns = target;
                    self.state.warming = false;
                    break;
                }
            }
            // Copy only one <=512-byte record to permit mutating the book/input.
            let mut bytes = [0u8; 512];
            bytes[..len].copy_from_slice(record);
            self.process(&bytes[..len], timestamp)?;
            self.cursor += len + 2;
            self.state.consumed += (len + 2) as u64;
            self.state.messages += 1;
            if self.state.start_ns.is_some() {
                self.state.clock_ns = timestamp;
            }
        }
        if self.state.complete {
            self.state.warming = false;
        }
        self.state.commit();
        Ok(())
    }
    fn process(&mut self, r: &[u8], timestamp: u64) -> Result<(), String> {
        let locate = u16::from_be_bytes([r[1], r[2]]);
        if r[0] == b'R' {
            if r.len() < 39 {
                return Err("Truncated stock directory".into());
            }
            self.register(locate, &r[11..19])?;
            return Ok(());
        }
        if matches!(r[0], b'A' | b'F') {
            let required = if r[0] == b'A' { 36 } else { 40 };
            if r.len() != required {
                return Err("Invalid add-order record".into());
            }
            self.register(locate, &r[24..32])?;
        }
        let expected = match r[0] {
            b'A' => 36,
            b'F' => 40,
            b'E' => 31,
            b'C' => 36,
            b'X' => 23,
            b'D' => 19,
            b'U' => 35,
            _ => return Ok(()),
        };
        if r.len() != expected {
            return Err(format!("Invalid {} record length", r[0] as char));
        }
        if self.state.start_ns.is_none() {
            self.state.start_ns = Some(timestamp.max(self.requested_start));
        }
        // Scope is resolved once per stock locate. Excluded records advance the
        // source clock without allocating order maps, books or publishing events.
        let Some(symbol) = self.routes.get(&locate) else {
            if !self.symbols.contains_key(&locate) {
                return Err(format!(
                    "Unknown stock locate {locate}; start replay before its directory/adds"
                ));
            }
            return Ok(());
        };
        let live_orders = self.live_orders.entry(locate).or_default();
        self.state.outputs[symbol].borrow_mut().synchronized = true;
        let reference = u64::from_be_bytes(r[11..19].try_into().unwrap());
        let read32 = |offset| u32::from_be_bytes(r[offset..offset + 4].try_into().unwrap());
        // Validate external input before invoking the native hot-path adapters.
        // Those adapters intentionally trust a well-formed feed.
        let old_quantity = live_orders.get(&reference).copied();
        match r[0] {
            b'A' | b'F' => {
                if old_quantity.is_some() || read32(32) == u32::MAX {
                    return Err("Duplicate order reference or unsupported maximum price".into());
                }
            }
            b'D' | b'U' | b'X' | b'E' | b'C' => {
                let quantity=old_quantity.ok_or_else(||format!("Unknown order reference {reference}; replay requires a file starting before its adds"))?;
                if matches!(r[0], b'X' | b'E' | b'C') && read32(19) > quantity {
                    return Err("Share reduction exceeds the resting order quantity".into());
                }
                if r[0] == b'U' {
                    let next = u64::from_be_bytes(r[19..27].try_into().unwrap());
                    if live_orders.contains_key(&next) || read32(31) == u32::MAX {
                        return Err(
                            "Duplicate replacement reference or unsupported maximum price".into(),
                        );
                    }
                }
            }
            _ => {}
        }
        let result = match r[0] {
            b'A' | b'F' => {
                let side = match r[19] {
                    b'B' => itchy::Side::Buy,
                    b'S' => itchy::Side::Sell,
                    _ => return Err("Invalid order side".into()),
                };
                let shares = read32(20);
                if shares == 0 {
                    return Err("Zero-quantity add order".into());
                }
                ItchAdd {
                    timestamp,
                    reference,
                    side,
                    shares,
                    price: itchy::Price4::from(read32(32)),
                }
                .route(&mut self.state, symbol)
            }
            b'E' | b'C' => ItchExecute {
                timestamp,
                reference,
                shares: read32(19),
                execution_price: (r[0] == b'C').then(|| read32(32)),
            }
            .route(&mut self.state, symbol),
            b'X' => ItchCancel {
                timestamp,
                reference,
                shares: read32(19),
            }
            .route(&mut self.state, symbol),
            b'D' => ItchDelete {
                timestamp,
                reference,
            }
            .route(&mut self.state, symbol),
            b'U' => {
                let shares = read32(27);
                if shares == 0 {
                    return Err("Zero-quantity replacement".into());
                }
                ItchReplace {
                    timestamp,
                    old_reference: reference,
                    new_reference: u64::from_be_bytes(r[19..27].try_into().unwrap()),
                    shares,
                    price: read32(31),
                }
                .route(&mut self.state, symbol)
            }
            _ => unreachable!(),
        };
        result.map_err(|error| {
            format!(
                "ITCH {} at byte {}: {error:?}",
                r[0] as char, self.state.consumed
            )
        })?;
        match r[0] {
            b'A' | b'F' => {
                live_orders.insert(reference, read32(20));
            }
            b'D' => {
                live_orders.remove(&reference);
            }
            b'U' => {
                live_orders.remove(&reference);
                live_orders.insert(
                    u64::from_be_bytes(r[19..27].try_into().unwrap()),
                    read32(27),
                );
            }
            _ => {
                let quantity = old_quantity.unwrap() - read32(19);
                if quantity == 0 {
                    live_orders.remove(&reference);
                } else {
                    live_orders.insert(reference, quantity);
                }
            }
        }
        Ok(())
    }
}

impl MarketDataAdapter for ItchStream {
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
        self.append(bytes, eof)
    }
    fn buffered_bytes(&self) -> usize {
        self.buffered()
    }
    fn advance(&mut self, elapsed_ns: u64, budget: usize) -> Result<(), String> {
        ItchStream::advance(self, elapsed_ns, budget)
    }
}
