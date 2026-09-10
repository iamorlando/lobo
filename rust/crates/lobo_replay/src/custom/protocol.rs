use super::{AdapterInfo, BootstrapRequest, FeedConnection, MarketDataAdapter};
use crate::feed::{FeedState, Instrument, normalize_symbol};
use lobo_context::{BookLevel, BookScope, FeedMode};

/// Owned, runtime metadata. Borrowed views avoid allocating in UI polling.
#[derive(Clone, Debug)]
pub struct AdapterDescriptor {
    pub id: String,
    pub name: String,
    pub mode: FeedMode,
    pub endpoint: Option<String>,
    pub default_symbol: String,
    pub timezone: String,
    pub supports_trades: bool,
    pub level: BookLevel,
}
impl From<AdapterInfo<'_>> for AdapterDescriptor {
    fn from(info: AdapterInfo<'_>) -> Self {
        Self {
            id: info.id.into(),
            name: info.name.into(),
            mode: info.mode,
            endpoint: info.endpoint.map(Into::into),
            default_symbol: info.default_symbol.into(),
            timezone: info.timezone.into(),
            supports_trades: info.supports_trades,
            level: info.level,
        }
    }
}
impl AdapterDescriptor {
    pub fn info(&self) -> AdapterInfo<'_> {
        AdapterInfo {
            id: &self.id,
            name: &self.name,
            mode: self.mode,
            endpoint: self.endpoint.as_deref(),
            default_symbol: &self.default_symbol,
            timezone: &self.timezone,
            supports_trades: self.supports_trades,
            level: self.level,
        }
    }
}

/// Wire decoding and connection-local state, independent of any transport.
///
/// Implementations receive the context explicitly. They may apply typed
/// messages directly, stage a snapshot, validate state, and commit at a
/// validated packet boundary. No intermediate universal-message allocation is
/// required. HTTP, WebSocket and file drivers all call this same contract.
pub trait Protocol {
    fn configure(&self, _state: &mut FeedState) -> Result<(), String> { Ok(()) }
    #[cfg(feature = "order-api")]
    fn submit_command(
        &mut self,
        state: &mut FeedState,
        symbol: &str,
        command: lobo_models::server::Command,
    ) -> Result<lobo_books::price_time_priority::CommandResult<lobo_primitives::Price64>, String>
    {
        use crate::order_messages::ApplyFeedCommand;
        if state.book(symbol).is_none() {
            return Err("Book does not exist".into());
        }
        command
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
            .map_err(|e| e.to_string())
    }

    fn register_instrument(
        &mut self,
        state: &mut FeedState,
        instrument: Instrument,
        policy: lobo_models::BookPolicy,
    ) -> Result<(), String> {
        let symbol = instrument.symbol.clone();
        state.register(instrument)?;
        state.context.set_policy(&symbol, policy)
    }
    fn receive(&mut self, state: &mut FeedState, bytes: &[u8], eof: bool) -> Result<(), String>;
    fn bootstrap_requests(&self) -> Vec<BootstrapRequest<'_>> {
        Vec::new()
    }
    fn bootstrap(
        &mut self,
        _state: &mut FeedState,
        _id: &str,
        _bytes: &[u8],
    ) -> Result<(), String> {
        Err("Unexpected adapter bootstrap response".into())
    }
    /// None selects the descriptor's single endpoint. Some supports arbitrary
    /// connection sharding, including an initially empty connection set.
    fn connections(&self, _state: &FeedState) -> Option<Vec<FeedConnection<'_>>> {
        None
    }
    fn receive_on(
        &mut self,
        state: &mut FeedState,
        _connection: u32,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.receive(state, bytes, false)
    }
    fn connected(&mut self, _state: &mut FeedState) -> Result<(), String> {
        Ok(())
    }
    fn connected_on(&mut self, state: &mut FeedState, _connection: u32) -> Result<(), String> {
        self.connected(state)
    }
    fn disconnected(&mut self, _state: &mut FeedState) {}
    fn disconnected_on(&mut self, state: &mut FeedState, _connection: u32) {
        self.disconnected(state);
    }
    fn keepalive(&mut self, _state: &mut FeedState) {}
    fn keepalive_on(&mut self, state: &mut FeedState, _connection: u32) {
        self.keepalive(state);
    }
    fn commands(&mut self, state: &mut FeedState) -> Vec<String> {
        std::mem::take(&mut state.outgoing)
    }
    fn commands_on(&mut self, state: &mut FeedState, _connection: u32) -> Vec<String> {
        self.commands(state)
    }
    fn subscribe(&mut self, _state: &mut FeedState, _symbol: &str) -> Result<(), String> {
        Ok(())
    }
    fn subscribe_scope(&mut self, state: &mut FeedState) -> Result<(), String> {
        let symbols = match &state.scope {
            BookScope::All => vec![state.selected.clone()],
            BookScope::Selected(symbols) => symbols.iter().cloned().collect(),
        };
        for symbol in symbols {
            self.subscribe(state, &symbol)?;
        }
        Ok(())
    }
    fn buffered_bytes(&self) -> usize {
        0
    }
    fn advance(
        &mut self,
        state: &mut FeedState,
        elapsed_ns: u64,
        _budget: usize,
    ) -> Result<(), String> {
        if let Some(start) = state.start_ns {
            state.clock_ns = state.clock_ns.max(start.saturating_add(elapsed_ns));
        }
        Ok(())
    }
    fn simulation_note(&self) -> Option<&str> {
        None
    }
}

/// A custom adapter is a protocol plus the library's existing feed state.
pub struct CustomAdapter<P> {
    pub protocol: P,
    pub state: FeedState,
    descriptor: AdapterDescriptor,
}
impl<P: Protocol> CustomAdapter<P> {
    pub fn new(
        descriptor: impl Into<AdapterDescriptor>,
        protocol: P,
        symbol: &str,
    ) -> Result<Self, String> {
        let mut state = FeedState::new(symbol)?;
        protocol.configure(&mut state)?;
        Ok(Self {
            protocol,
            state,
            descriptor: descriptor.into(),
        })
    }
    pub fn with_scope(mut self, scope: BookScope) -> Result<Self, String> {
        self.set_book_scope(scope)?;
        Ok(self)
    }
    /// Accepts lazy directory generators. Registration does not allocate books.
    /// Protocols can also register instruments discovered after connecting.
    pub fn register_instruments(
        &mut self,
        instruments: impl IntoIterator<Item = Instrument>,
    ) -> Result<(), String> {
        for mut instrument in instruments {
            instrument.symbol = normalize_symbol(&instrument.symbol)?;
            self.state.register(instrument)?;
        }
        Ok(())
    }
    pub fn into_parts(self) -> (AdapterDescriptor, P, FeedState) {
        (self.descriptor, self.protocol, self.state)
    }
}
impl<P: Protocol> MarketDataAdapter for CustomAdapter<P> {
    #[cfg(feature = "order-api")]
    fn submit_command(
        &mut self,
        symbol: &str,
        command: lobo_models::server::Command,
    ) -> Result<lobo_books::price_time_priority::CommandResult<lobo_primitives::Price64>, String>
    {
        self.protocol
            .submit_command(&mut self.state, symbol, command)
    }
    fn info(&self) -> AdapterInfo<'_> {
        self.descriptor.info()
    }
    fn register_instrument(
        &mut self,
        instrument: Instrument,
        policy: lobo_models::BookPolicy,
    ) -> Result<(), String> {
        self.protocol
            .register_instrument(&mut self.state, instrument, policy)
    }
    fn state(&self) -> &FeedState {
        &self.state
    }
    fn state_mut(&mut self) -> &mut FeedState {
        &mut self.state
    }
    #[inline]
    fn receive(&mut self, bytes: &[u8], eof: bool) -> Result<(), String> {
        self.protocol.receive(&mut self.state, bytes, eof)
    }
    fn bootstrap_requests(&self) -> Vec<BootstrapRequest<'_>> {
        self.protocol.bootstrap_requests()
    }
    fn bootstrap(&mut self, id: &str, bytes: &[u8]) -> Result<(), String> {
        self.protocol.bootstrap(&mut self.state, id, bytes)
    }
    fn connections(&self) -> Vec<FeedConnection<'_>> {
        self.protocol.connections(&self.state).unwrap_or_else(|| {
            self.descriptor
                .endpoint
                .iter()
                .map(|endpoint| FeedConnection {
                    id: 0,
                    endpoint,
                    selected: true,
                })
                .collect()
        })
    }
    #[inline]
    fn receive_on(&mut self, connection: u32, bytes: &[u8]) -> Result<(), String> {
        self.protocol.receive_on(&mut self.state, connection, bytes)
    }
    fn connected(&mut self) -> Result<(), String> {
        self.protocol.connected(&mut self.state)
    }
    fn connected_on(&mut self, connection: u32) -> Result<(), String> {
        self.protocol.connected_on(&mut self.state, connection)
    }
    fn disconnected(&mut self) {
        self.protocol.disconnected(&mut self.state);
    }
    fn disconnected_on(&mut self, connection: u32) {
        self.protocol.disconnected_on(&mut self.state, connection);
    }
    fn keepalive(&mut self) {
        self.protocol.keepalive(&mut self.state);
    }
    fn keepalive_on(&mut self, connection: u32) {
        self.protocol.keepalive_on(&mut self.state, connection);
    }
    fn commands(&mut self) -> Vec<String> {
        self.protocol.commands(&mut self.state)
    }
    fn commands_on(&mut self, connection: u32) -> Vec<String> {
        self.protocol.commands_on(&mut self.state, connection)
    }
    fn subscribe(&mut self, symbol: &str) -> Result<(), String> {
        self.protocol.subscribe(&mut self.state, symbol)
    }
    fn subscribe_scope(&mut self) -> Result<(), String> {
        self.protocol.subscribe_scope(&mut self.state)
    }
    fn buffered_bytes(&self) -> usize {
        self.protocol.buffered_bytes()
    }
    #[inline]
    fn advance(&mut self, elapsed_ns: u64, budget: usize) -> Result<(), String> {
        self.protocol.advance(&mut self.state, elapsed_ns, budget)
    }
    fn simulation_note(&self) -> Option<&str> {
        self.protocol.simulation_note()
    }
}
