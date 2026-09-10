//! Transport-independent adapter contract; all book state belongs to the context.
pub use crate::dispatch_feed_book;
pub use crate::feed::normalize_symbol;
pub use crate::feed::{FeedBook, FeedState, Instrument};
pub use lobo_context::{BookLevel, FeedMode};
use lobo_context::{BookScope, InstrumentDirectory, ScopedInstruments};
#[derive(Clone, Copy, Debug)]
pub struct AdapterInfo<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub mode: FeedMode,
    pub endpoint: Option<&'a str>,
    pub default_symbol: &'a str,
    pub timezone: &'a str,
    pub supports_trades: bool,
    pub level: BookLevel,
}
/// HTTP bytes are decoded by the adapter, just like socket bytes.
#[derive(Clone, Debug)]
pub struct BootstrapRequest<'a> {
    pub id: &'a str,
    pub url: &'a str,
}
#[derive(Clone, Debug)]
pub struct FeedConnection<'a> {
    pub id: u32,
    pub endpoint: &'a str,
    pub selected: bool,
}
/// Consumers drive this contract regardless of venue or transport. Wire parsing,
/// protocol commands and validation belong to implementations; directory lookup,
/// selection, book invalidation and command draining have shared defaults.
pub trait MarketDataAdapter {
    fn info(&self) -> AdapterInfo<'_>;
    fn state(&self) -> &FeedState;
    fn state_mut(&mut self) -> &mut FeedState;
    fn register_instrument(
        &mut self,
        instrument: Instrument,
        policy: lobo_models::BookPolicy,
    ) -> Result<(), String> {
        let symbol = instrument.symbol.clone();
        self.state_mut().register(instrument)?;
        self.state_mut().context.set_policy(&symbol, policy)
    }
    #[cfg(feature = "order-api")]
    fn submit_command(
        &mut self,
        symbol: &str,
        command: lobo_models::server::Command,
    ) -> Result<lobo_books::price_time_priority::CommandResult<lobo_primitives::Price64>, String>
    {
        use crate::order_messages::ApplyFeedCommand;
        if self.state().book(symbol).is_none() {
            return Err("Book does not exist".into());
        }
        let timestamp = self.state().clock_ns;
        command
            .route_feed(
                self.state_mut(),
                symbol,
                timestamp,
                lobo_models::events::Reports {
                    include_fills: true,
                    include_summary: false,
                    include_market_impact: false,
                },
            )
            .map_err(|e| e.to_string())
    }
    /// Bytes are file chunks for replay, or one complete raw UTF-8 socket message.
    fn receive(&mut self, bytes: &[u8], eof: bool) -> Result<(), String>;
    fn bootstrap_requests(&self) -> Vec<BootstrapRequest<'_>> {
        Vec::new()
    }
    fn bootstrap(&mut self, _id: &str, _bytes: &[u8]) -> Result<(), String> {
        Err("Unexpected adapter bootstrap response".into())
    }
    /// Default single-socket routing; adapters can shard subscriptions without
    /// exposing venue protocol or channel limits to their consumers.
    fn connections(&self) -> Vec<FeedConnection<'_>> {
        self.info()
            .endpoint
            .into_iter()
            .map(|endpoint| FeedConnection {
                id: 0,
                endpoint,
                selected: true,
            })
            .collect()
    }
    fn receive_on(&mut self, _connection: u32, bytes: &[u8]) -> Result<(), String> {
        self.receive(bytes, false)
    }
    fn connected_on(&mut self, _connection: u32) -> Result<(), String> {
        self.connected()
    }
    fn disconnected_on(&mut self, _connection: u32) {
        self.disconnected();
    }
    fn keepalive_on(&mut self, _connection: u32) {
        self.keepalive();
    }
    fn commands_on(&mut self, _connection: u32) -> Vec<String> {
        self.commands()
    }
    fn simulation_note(&self) -> Option<&str> {
        None
    }
    fn buffered_bytes(&self) -> usize {
        0
    }
    fn advance(&mut self, elapsed_ns: u64, _budget: usize) -> Result<(), String> {
        let state = self.state_mut();
        if let Some(start) = state.start_ns {
            state.clock_ns = state.clock_ns.max(start.saturating_add(elapsed_ns));
        }
        Ok(())
    }
    fn selected_book(&self) -> Option<&FeedBook> {
        self.state().view().selected_book()
    }
    fn tickers(&self) -> Vec<String> {
        self.state().tickers()
    }
    fn scoped_tickers(&self) -> Vec<String> {
        self.state().scoped_tickers()
    }
    /// Configure once, before directory registration or any book allocation.
    fn set_book_scope(&mut self, scope: BookScope) -> Result<(), String> {
        if !self.state().instruments.is_empty() || self.state().messages != 0 {
            return Err("Book scope must be set before the feed starts".into());
        }
        if !scope.contains(&self.state().selected) {
            return Err("The selected ticker must belong to the book scope".into());
        }
        self.state_mut().scope = scope;
        Ok(())
    }
    /// All permits discovery and subscriptions as visited; an explicit scope
    /// subscribes its selected instruments once the live directory is available.
    fn subscribe_scope(&mut self) -> Result<(), String> {
        let symbols = match &self.state().scope {
            BookScope::All => vec![self.state().selected.clone()],
            BookScope::Selected(symbols) => symbols.iter().cloned().collect(),
        };
        for symbol in symbols {
            self.subscribe(&symbol)?;
        }
        Ok(())
    }
    fn validate_ticker(&self, symbol: &str) -> Result<String, String> {
        if self.state().simulation.is_some() {
            return Err("Return to the main timeline before changing ticker".into());
        }
        let symbol = normalize_symbol(symbol)?;
        if !self.state().scope.contains(&symbol) {
            return Err(format!("{symbol} is outside the current book scope"));
        }
        if !self.state().instruments.contains_key(&symbol) {
            return Err(format!("{symbol} is not in this feed's directory"));
        }
        Ok(symbol)
    }
    fn select_ticker(&mut self, symbol: &str) -> Result<(), String> {
        let symbol = self.validate_ticker(symbol)?;
        self.subscribe(&symbol)?;
        self.state_mut().selected = symbol.clone();
        self.state_mut().market_preview = None;
        if self.info().mode == FeedMode::Live {
            self.state_mut().warming = !self.state().synchronized(&symbol);
        }
        Ok(())
    }
    /// Implementations can enqueue subscribe requests. Replay already routes all books.
    fn subscribe(&mut self, _symbol: &str) -> Result<(), String> {
        Ok(())
    }
    fn connected(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn disconnected(&mut self) {}
    fn keepalive(&mut self) {}
    fn commands(&mut self) -> Vec<String> {
        std::mem::take(&mut self.state_mut().outgoing)
    }
    fn simulate(
        &mut self,
        order: lobo_models::orders::order_types::LimitOrder<lobo_primitives::Price64>,
    ) -> Result<(), String> {
        if self.info().level != BookLevel::L3 {
            return Err("L2 supports market previews only".into());
        }
        let mode = self.info().mode;
        let state = self.state_mut();
        if state.warming || state.simulation.is_some() {
            return Err("Wait for the book or return to the main timeline first".into());
        }
        state.simulation = Some(Box::new(crate::simulation::Simulation::start(
            state, order, mode,
        )?));
        state.market_preview = None;
        Ok(())
    }
    fn simulate_market(
        &mut self,
        order: lobo_models::orders::order_types::MarketOrder<lobo_primitives::Price64>,
    ) -> Result<(), String> {
        let level = self.info().level;
        let state = self.state_mut();
        if state.warming || state.simulation.is_some() {
            return Err("Wait for the book or return to the main timeline first".into());
        }
        state.market_preview = Some(crate::simulation::market_preview(state, order, level)?);
        Ok(())
    }
    fn return_to_main(&mut self) {
        self.state_mut().simulation = None;
        self.state_mut().market_preview = None;
    }
}

/// Decoded messages share routing; venue implementations only adapt each
/// operation to the branch API. No simulation checks enter storage loops.
pub trait AdaptToFeed {
    fn process_feed(self, book: &mut FeedBook) -> Result<(), lobo_storage::OrderStateError>;
}
macro_rules! feed_message_policies {
    (() [$(($u:ident, $ub:literal, $ut:ty, $h:ident, $hb:literal, $ht:ty, $c:ident, $ct:ty),)*]) => {
        impl<T> AdaptToFeed for T
        where $(T: super::AdaptForReplay<
            lobo_storage::price_level::IntrusivePriceLevel<lobo_primitives::Price64, $ht, $ct>,
            lobo_storage::price_sorting::BTreeMapPriceSorting, $ut, $ht, crate::feed::FeedPublishers>,)*
        {
            fn process_feed(self, book: &mut FeedBook) -> Result<(), lobo_storage::OrderStateError> {
                crate::dispatch_feed_book!(book, book, self.process(book))
            }
        }
    };
}
lobo_storage::book_policy_matrix!(feed_message_policies);
pub trait FeedMessage: AdaptToFeed + Sized {
    fn simulate(
        &self,
        branch: &mut crate::simulation::Simulation,
        source: &FeedBook,
    ) -> Result<(), lobo_storage::OrderStateError>;
    fn route(
        self,
        state: &mut FeedState,
        symbol: &str,
    ) -> Result<(), lobo_storage::OrderStateError> {
        if let Some(branch) = &mut state.simulation {
            if branch.feed.selected == symbol && !branch.stopped() {
                if let Some(source) = state.context.get(symbol) {
                    self.simulate(branch, source)?;
                }
            }
        }
        self.process_feed(state.book_mut(symbol))
    }
}
