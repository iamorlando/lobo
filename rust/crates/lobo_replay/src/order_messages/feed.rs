//! Live order-message snapshots and updates over the existing book API.
use super::commands::{ApplyFeedCommand, simulate_command};
use crate::custom::{AdapterInfo, BookLevel, FeedMode, MarketDataAdapter};
use crate::feed::{FeedState, Instrument};
use lobo_models::{
    events::Reports,
    server::{Command, FeedMessage},
};
use std::collections::HashMap;

/// Capabilities of the default server order-message feed.
pub const INFO: AdapterInfo = AdapterInfo {
    id: "server",
    name: "Python books",
    mode: FeedMode::Live,
    endpoint: Some("/api/feed"),
    default_symbol: "BOOK",
    timezone: "UTC",
    supports_trades: true,
    level: BookLevel::L3,
};
/// Apply snapshots and typed order commands from a server without a custom protocol.
pub struct OrderMessages {
    state: FeedState,
    sequences: HashMap<String, u64>,
}
impl OrderMessages {
    /// Create a live L3 view, initially selecting the supplied book symbol.
    pub fn new(symbol: &str) -> Result<Self, String> {
        Ok(Self {
            state: FeedState::new(symbol)?,
            sequences: HashMap::new(),
        })
    }
    fn register(&mut self, book: lobo_models::server::BookInfo) -> Result<(), String> {
        self.state.context.set_policy(&book.symbol, book.policy)?;
        self.state.register(Instrument {
            symbol: book.symbol,
            price_decimals: book.price_decimals,
            quantity_decimals: book.quantity_decimals,
        })
    }
    fn select_available(&mut self) {
        if !self.state.instruments.contains_key(&self.state.selected) {
            if let Some(first) = self.state.instruments.keys().next() {
                self.state.selected = first.clone();
            }
        }
    }
}
impl MarketDataAdapter for OrderMessages {
    fn info(&self) -> AdapterInfo<'_> {
        INFO
    }
    fn state(&self) -> &FeedState {
        &self.state
    }
    fn state_mut(&mut self) -> &mut FeedState {
        &mut self.state
    }
    fn receive(&mut self, bytes: &[u8], _eof: bool) -> Result<(), String> {
        let message: FeedMessage = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        match message {
            FeedMessage::Directory { books } => {
                let removed = self
                    .state
                    .instruments
                    .keys()
                    .filter(|symbol| !books.iter().any(|book| &book.symbol == *symbol))
                    .cloned()
                    .collect::<Vec<_>>();
                for symbol in removed {
                    self.state.invalidate_orders(&symbol);
                    self.state.instruments.remove(&symbol);
                    self.state
                        .context
                        .books
                        .remove(&symbol.to_ascii_lowercase());
                    self.state.outputs.remove(&symbol);
                    self.sequences.remove(&symbol);
                }
                for book in books {
                    self.register(book)?;
                }
                self.select_available();
            }
            FeedMessage::Snapshot {
                book,
                sequence,
                timestamp_ns,
                orders,
            } => {
                let symbol = book.symbol.clone();
                // A book registered during connection may also have been
                // included in the initial snapshot set. Never rewind it.
                if self
                    .sequences
                    .get(&symbol)
                    .is_some_and(|previous| *previous >= sequence)
                {
                    return Ok(());
                }
                self.register(book)?;
                self.select_available();
                self.state.invalidate_orders(&symbol);
                let native = self.state.book_mut(&symbol);
                for snapshot in orders {
                    Command::Add {
                        order: snapshot.order,
                    }
                    .apply_feed(native, snapshot.timestamp_ns, Reports::default())
                    .map_err(|e| e.to_string())?;
                }
                self.sequences.insert(symbol.clone(), sequence);
                self.state.outputs[&symbol].borrow_mut().synchronized = true;
                self.state.clock_ns = self.state.clock_ns.max(timestamp_ns);
                self.state.start_ns.get_or_insert(timestamp_ns);
                self.state.warming = !self.state.synchronized(&self.state.selected);
            }
            FeedMessage::Update {
                book,
                sequence,
                timestamp_ns,
                command,
            } => {
                let previous = self
                    .sequences
                    .get(&book)
                    .ok_or("update arrived before book snapshot")?;
                if sequence <= *previous {
                    return Ok(());
                }
                if sequence != previous + 1 {
                    return Err("server feed sequence gap; reconnect for a snapshot".into());
                }
                if command.simulated() {
                    return Err("simulated commands must not enter the main feed".into());
                }
                if let Some(branch) = &mut self.state.simulation {
                    if branch.feed.selected == book {
                        if let Some(source) = self.state.context.get(&book) {
                            if let Err(error) =
                                simulate_command(&command, timestamp_ns, branch, source)
                            {
                                branch.interrupt(&error);
                            }
                        }
                    }
                }
                command
                    .apply_feed(self.state.book_mut(&book), timestamp_ns, Reports::default())
                    .map_err(|e| e.to_string())?;
                self.sequences.insert(book, sequence);
                self.state.messages += 1;
                self.state.clock_ns = self.state.clock_ns.max(timestamp_ns);
            }
            FeedMessage::Heartbeat => {}
        }
        self.state.consumed += bytes.len() as u64;
        self.state.commit();
        Ok(())
    }
    fn disconnected(&mut self) {
        for symbol in self.state.instruments.keys().cloned().collect::<Vec<_>>() {
            self.state.invalidate_orders(&symbol);
        }
        self.sequences.clear();
        self.state.warming = true;
    }
    fn keepalive(&mut self) {
        self.state.outgoing.push("{\"op\":\"ping\"}".into());
    }
}
