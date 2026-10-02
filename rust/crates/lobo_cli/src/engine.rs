use crate::{
    args::{Args, OrderKind, OrderSide, SourceKind},
    demo::{Demo, DemoControl, DemoDriver},
};
use anyhow::{Result, anyhow, bail};
use lobo_adapters::adapter::market::{
    AdapterInfo, BootstrapRequest, FeedConnection, FeedState, MarketDataAdapter, create_adapter,
};
use lobo_context::{BookLevel, BookScope};
use lobo_models::{
    Side,
    orders::order_types::{LimitOrder, MarketOrder},
    server::Command,
};
use lobo_primitives::{Price64, uuid::Uuid};
use lobo_replay::{
    custom::runtime::{Session, Source},
    feed::Instrument,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Level {
    pub price: f64,
    pub quantity: f64,
    pub side: u8,
    pub orders: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Candle {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub ticks: u64,
    pub start_ns: u64,
    pub end_ns: u64,
    pub forming: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Flow {
    pub clock_ns: u64,
    pub incoming: f64,
    pub outgoing: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DepthFrame {
    pub clock_ns: u64,
    pub levels: Vec<Level>,
}
#[derive(Clone, Default)]
pub struct History {
    pub levels: BTreeMap<(u64, u8), Level>,
    pub candles: VecDeque<Candle>,
    pub depth: VecDeque<DepthFrame>,
    pub flow: VecDeque<Flow>,
    pub generation: u64,
    pending_flow: Flow,
}
pub type Store = Arc<Mutex<BTreeMap<String, History>>>;
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QueueEntry {
    pub id: String,
    pub price: f64,
    pub quantity: f64,
    pub mine: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Fill {
    pub clock_ns: u64,
    pub price: f64,
    pub quantity: f64,
    pub maker: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Simulation {
    pub requested: f64,
    pub filled: f64,
    pub remaining: f64,
    pub average_price: Option<f64>,
    pub alternate: bool,
    pub stopped: Option<String>,
    pub fills: Vec<Fill>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Snapshot {
    pub session_sequence: Option<u64>,
    pub session_revision: Option<u64>,
    pub feed_error: Option<String>,
    pub symbol: String,
    pub source: String,
    pub level: String,
    pub clock_ns: u64,
    pub source_clock_ns: u64,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub warming: bool,
    pub complete: bool,
    pub messages: u64,
    pub updates: u64,
    pub consumed: u64,
    pub checksum_checks: u64,
    pub checksum_failures: u64,
    pub price_decimals: u8,
    pub quantity_decimals: u8,
    pub levels: Vec<Level>,
    pub candles: Vec<Candle>,
    pub depth: Vec<DepthFrame>,
    pub flow: Vec<Flow>,
    pub queue: Vec<QueueEntry>,
    pub queue_price: Option<f64>,
    pub orders_ahead: Option<usize>,
    pub quantity_ahead: Option<f64>,
    pub simulation: Option<Simulation>,
    pub tickers: Vec<String>,
}

/// Consume staging messages on the feed worker, so high-speed replay stays bounded
/// even when the terminal draws slowly. All calculations use native adapter output.
struct Collector {
    inner: Box<dyn MarketDataAdapter>,
    store: Store,
    capacity: usize,
    interval_ns: Arc<AtomicU64>,
}
impl Collector {
    fn collect(&self) {
        let mut store = self.store.lock().unwrap();
        self.collect_state(self.inner.state(), &mut store);
        if let Some(branch) = &self.inner.state().simulation {
            self.collect_state(&branch.feed, &mut store);
        }
    }
    fn collect_state(&self, state: &FeedState, store: &mut BTreeMap<String, History>) {
        for key in state.context.books.keys() {
            let symbol = &key.to_ascii_uppercase();
            let Some(output) = state.outputs.get(symbol) else {
                continue;
            };
            let Some(instrument) = state.instruments.get(symbol) else {
                continue;
            };
            let mut output = output.borrow_mut();
            if !output.synchronized && output.pending.is_empty() {
                continue;
            }
            let history = store.entry(state.chart_key(symbol)).or_default();
            if history.generation != output.generation {
                *history = History {
                    generation: output.generation,
                    ..Default::default()
                };
            }
            for (key, event) in std::mem::take(&mut output.pending) {
                let event = event.event();
                let quantity = instrument.quantity(event.visible_quantity());
                let previous = history.levels.get(&key).map_or(0.0, |v| v.quantity);
                let delta = quantity - previous;
                if delta > 0.0 {
                    history.pending_flow.incoming += delta;
                } else {
                    history.pending_flow.outgoing -= delta;
                }
                if quantity == 0.0 {
                    history.levels.remove(&key);
                } else {
                    history.levels.insert(
                        key,
                        Level {
                            price: instrument.price(event.price().into()),
                            quantity,
                            side: u8::from(event.side() == Side::Sell),
                            orders: event.number_of_orders(),
                        },
                    );
                }
            }
            if !state.warming
                && history.depth.back().is_none_or(|f| {
                    state.clock_ns.saturating_sub(f.clock_ns)
                        >= self.interval_ns.load(Ordering::Relaxed)
                })
            {
                push(
                    &mut history.depth,
                    DepthFrame {
                        clock_ns: state.clock_ns,
                        levels: history.levels.values().cloned().collect(),
                    },
                    self.capacity,
                );
                if history.flow.is_empty() {
                    history.pending_flow = Flow::default();
                }
                history.pending_flow.clock_ns = state.clock_ns;
                push(
                    &mut history.flow,
                    std::mem::take(&mut history.pending_flow),
                    self.capacity,
                );
            }
        }
        let bars = state.volume_bars().borrow();
        for event in bars.destination.0.borrow_mut().drain(..) {
            if let Some(instrument) = state.instruments.get(event.book_id()) {
                let history = store.entry(state.chart_key(event.book_id())).or_default();
                push(
                    &mut history.candles,
                    candle(event.event(), instrument, false),
                    self.capacity,
                );
            }
        }
    }
}
fn push<T>(queue: &mut VecDeque<T>, item: T, capacity: usize) {
    if queue.len() >= capacity {
        queue.pop_front();
    }
    queue.push_back(item);
}
fn candle(bar: &lobo_events_bar::Bar, instrument: &Instrument, forming: bool) -> Candle {
    Candle {
        open: instrument.price(bar.open.into()),
        high: instrument.price(bar.high.into()),
        low: instrument.price(bar.low.into()),
        close: instrument.price(bar.close.into()),
        volume: instrument.quantity(bar.volume),
        ticks: bar.ticks,
        start_ns: bar.start_ns,
        end_ns: bar.end_ns,
        forming,
    }
}
// Keep the native bar type explicit at the presentation boundary.
mod lobo_events_bar {
    pub type Bar = lobo_events::OhlcBar<lobo_primitives::Price64>;
}
impl MarketDataAdapter for Collector {
    fn simulate(&mut self, order: LimitOrder<Price64>) -> Result<(), String> {
        self.inner.simulate(order)?;
        self.collect();
        Ok(())
    }
    fn simulate_market(&mut self, order: MarketOrder<Price64>) -> Result<(), String> {
        self.inner.simulate_market(order)?;
        self.collect();
        Ok(())
    }
    fn return_to_main(&mut self) {
        let prefix = self
            .inner
            .state()
            .simulation
            .as_ref()
            .map(|b| format!("{}:", b.feed.namespace));
        self.inner.return_to_main();
        if let Some(prefix) = prefix {
            self.store
                .lock()
                .unwrap()
                .retain(|key, _| !key.starts_with(&prefix));
        }
    }
    fn submit_command(
        &mut self,
        symbol: &str,
        command: Command,
    ) -> Result<lobo_books::price_time_priority::CommandResult<Price64>, String> {
        let result = self.inner.submit_command(symbol, command)?;
        self.inner.state_mut().commit();
        self.collect();
        Ok(result)
    }
    fn info(&self) -> AdapterInfo<'_> {
        self.inner.info()
    }
    fn state(&self) -> &FeedState {
        self.inner.state()
    }
    fn state_mut(&mut self) -> &mut FeedState {
        self.inner.state_mut()
    }
    fn receive(&mut self, bytes: &[u8], eof: bool) -> Result<(), String> {
        self.inner.receive(bytes, eof)?;
        self.collect();
        Ok(())
    }
    fn advance(&mut self, elapsed: u64, budget: usize) -> Result<(), String> {
        let budget = if self.inner.info().id == "itch" && self.inner.state().start_ns.is_none() {
            budget.max(4096)
        } else {
            budget
        };
        self.inner.advance(elapsed, budget)?;
        // Replay clocks close time bars even when a later source record is not
        // an execution. This is the same watermark used by the browser session.
        if self.inner.info().mode == lobo_context::FeedMode::Replay {
            let state = self.inner.state();
            state
                .volume_bars()
                .borrow_mut()
                .advance_time(state.clock_ns)
                .map_err(|e| e.to_string())?;
            if let Some(branch) = &state.simulation
                && !branch.stopped()
            {
                branch
                    .feed
                    .volume_bars()
                    .borrow_mut()
                    .advance_time(branch.feed.clock_ns)
                    .map_err(|e| e.to_string())?;
            }
        }
        self.collect();
        Ok(())
    }
    fn bootstrap_requests(&self) -> Vec<BootstrapRequest<'_>> {
        self.inner.bootstrap_requests()
    }
    fn bootstrap(&mut self, id: &str, bytes: &[u8]) -> Result<(), String> {
        self.inner.bootstrap(id, bytes)
    }
    fn connections(&self) -> Vec<FeedConnection<'_>> {
        self.inner.connections()
    }
    fn receive_on(&mut self, id: u32, bytes: &[u8]) -> Result<(), String> {
        self.inner.receive_on(id, bytes)?;
        self.collect();
        Ok(())
    }
    fn connected_on(&mut self, id: u32) -> Result<(), String> {
        self.inner.connected_on(id)
    }
    fn disconnected_on(&mut self, id: u32) {
        self.inner.disconnected_on(id);
        self.collect();
    }
    fn keepalive_on(&mut self, id: u32) {
        self.inner.keepalive_on(id);
    }
    fn commands_on(&mut self, id: u32) -> Vec<String> {
        self.inner.commands_on(id)
    }
    fn buffered_bytes(&self) -> usize {
        self.inner.buffered_bytes()
    }
    fn subscribe(&mut self, symbol: &str) -> Result<(), String> {
        self.inner.subscribe(symbol)
    }
    fn connected(&mut self) -> Result<(), String> {
        self.inner.connected()
    }
    fn disconnected(&mut self) {
        self.inner.disconnected();
        self.collect();
    }
    fn keepalive(&mut self) {
        self.inner.keepalive();
    }
    fn commands(&mut self) -> Vec<String> {
        self.inner.commands()
    }
    fn simulation_note(&self) -> Option<&str> {
        self.inner.simulation_note()
    }
}

pub struct Engine {
    pub session: Session,
    pub store: Store,
    pub source: SourceKind,
    pub demo_control: Arc<Mutex<DemoControl>>,
    interval_ns: Arc<AtomicU64>,
    capacity: u32,
}
impl Engine {
    pub fn start(args: &Args) -> Result<Self> {
        let source = args.source_kind();
        let store = Store::default();
        let interval_ns = Arc::new(AtomicU64::new(
            (args.history_seconds * 1e9 / f64::from(args.capacity)).max(1.0) as u64,
        ));
        let factory_interval = interval_ns.clone();
        let demo_control = Arc::new(Mutex::new(DemoControl {
            speed: args.speed,
            paused: args.paused,
            elapsed_ns: 0,
        }));
        let args = args.clone();
        let scope = scope(&args)?;
        let factory_store = store.clone();
        let factory_args = args.clone();
        let factory = move |collect: bool| -> Result<Box<dyn MarketDataAdapter>, String> {
            let args = &factory_args;
            let mut inner: Box<dyn MarketDataAdapter> = if source == SourceKind::Demo {
                let symbols = match &scope {
                    BookScope::Selected(s) => s.iter().cloned().collect(),
                    _ => vec![args.ticker()],
                };
                Box::new(Demo::new(args, symbols)?)
            } else if let Some(path) = &args.descriptor {
                let value =
                    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                let descriptor = lobo_replay::custom::observer::parse_descriptor(value)?;
                let protocol = lobo_replay::custom::observer::ObservedProtocol::new(
                    descriptor.clone(),
                    &args.ticker(),
                )?;
                Box::new(lobo_replay::custom::CustomAdapter::new(
                    descriptor,
                    protocol,
                    &args.ticker(),
                )?)
            } else {
                create_adapter(
                    match source {
                        SourceKind::Itch | SourceKind::Nasdaq => "itch",
                        SourceKind::Kraken => "kraken",
                        SourceKind::Bitfinex => "bitfinex",
                        SourceKind::Server => "server",
                        _ => unreachable!(),
                    },
                    &args.ticker(),
                    args.start_at,
                )?
            };
            if source != SourceKind::Demo {
                inner.set_book_scope(scope.clone())?;
            }
            inner
                .state_mut()
                .set_bar_aggregation(args.bars().map_err(|e| e.to_string())?);
            if collect {
                factory_store.lock().unwrap().clear();
                let collector = Collector {
                    inner,
                    store: factory_store.clone(),
                    capacity: args.capacity as usize,
                    interval_ns: factory_interval.clone(),
                };
                collector.collect();
                Ok(Box::new(collector))
            } else {
                Ok(inner)
            }
        };
        let session = if source == SourceKind::Demo {
            Session::spawn_boxed(
                move || factory(true),
                Box::new(DemoDriver {
                    control: demo_control.clone(),
                }),
            )
        } else if matches!(source, SourceKind::Itch | SourceKind::Nasdaq) {
            let input = if let Some(path) = args.file.clone() {
                Source::File {
                    path,
                    chunk_size: 65536,
                }
            } else {
                let url = if source == SourceKind::Nasdaq {
                    let name = args
                        .session
                        .clone()
                        .ok_or_else(|| anyhow!("choose --session from `lobo sessions`"))?;
                    if name.contains('/') || name.contains('\\') || !name.ends_with(".gz") {
                        bail!("invalid Nasdaq session filename");
                    }
                    format!("https://emi.nasdaq.com/ITCH/Nasdaq%20ITCH/{name}")
                } else {
                    args.url
                        .clone()
                        .ok_or_else(|| anyhow!("ITCH requires --file or --url"))?
                };
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    bail!("replay URL must be HTTP(S)");
                }
                Source::Http {
                    url,
                    chunk_size: 65536,
                }
            };
            Session::spawn_playback(factory, input, args.paused, args.speed, false, None)
        } else {
            let endpoint = args.url.clone();
            if source == SourceKind::Server && endpoint.is_none() {
                bail!("server requires --url ws://HOST/api/feed (or an observed adapter endpoint)");
            }
            if endpoint
                .as_ref()
                .is_some_and(|s| !s.starts_with("ws://") && !s.starts_with("wss://"))
            {
                bail!("live URL must be ws:// or wss://");
            }
            Session::spawn_boxed(
                move || factory(true),
                Box::new(Source::WebSocket {
                    endpoint,
                    heartbeat: None,
                }),
            )
        }
        .map_err(|e| anyhow!(e))?;
        Ok(Self {
            session,
            store,
            source,
            demo_control,
            interval_ns,
            capacity: args.capacity,
        })
    }
    pub fn snapshot(&self, side: OrderSide, queue_price: Option<f64>) -> Result<Snapshot> {
        let store = self.store.clone();
        self.session
            .with(move |adapter| {
                let root = adapter.state();
                let state = root.view();
                let mut snapshot = Snapshot {
                    symbol: state.selected.clone(),
                    source: adapter.info().name.into(),
                    level: adapter.info().level.as_str().into(),
                    clock_ns: state.clock_ns,
                    source_clock_ns: root.clock_ns,
                    warming: root.warming,
                    complete: root.complete,
                    messages: root.messages,
                    consumed: root.consumed,
                    checksum_checks: root.checksum_checks,
                    checksum_failures: root.checksum_failures,
                    tickers: adapter.scoped_tickers(),
                    ..Default::default()
                };
                let Some(instrument) = state.instruments.get(&state.selected) else {
                    return Ok(snapshot);
                };
                snapshot.price_decimals = instrument.price_decimals;
                snapshot.quantity_decimals = instrument.quantity_decimals;
                snapshot.updates = root
                    .outputs
                    .get(&root.selected)
                    .map_or(0, |o| o.borrow().count);
                if let Some(history) = store.lock().unwrap().get(&state.chart_key(&state.selected))
                {
                    snapshot.levels = history.levels.values().cloned().collect();
                    snapshot.candles = history.candles.iter().cloned().collect();
                    snapshot.depth = history.depth.iter().cloned().collect();
                    snapshot.flow = history.flow.iter().cloned().collect();
                }
                if let Some(bar) = state.volume_bars().borrow().forming(&state.selected) {
                    snapshot.candles.push(candle(&bar, instrument, true));
                }
                if let Some(book) = state.selected_book() {
                    snapshot.bid = book
                        .best_price(Side::Buy)
                        .map(|p| instrument.price(p.into()));
                    snapshot.ask = book
                        .best_price(Side::Sell)
                        .map(|p| instrument.price(p.into()));
                    if adapter.info().level == BookLevel::L3 {
                        let (selected_side, selected_price) = root
                            .simulation
                            .as_ref()
                            .and_then(|s| {
                                s.report
                                    .price
                                    .map(|p| (s.report.side, instrument.price(p.into())))
                            })
                            .unwrap_or_else(|| {
                                (
                                    side.native(),
                                    queue_price.unwrap_or(if side == OrderSide::Buy {
                                        snapshot.bid.unwrap_or(0.0)
                                    } else {
                                        snapshot.ask.unwrap_or(0.0)
                                    }),
                                )
                            });
                        if selected_price > 0.0 {
                            let raw = atoms(selected_price, instrument.price_decimals)?;
                            let orders = book
                                .queue_view(selected_side, Price64::from(raw)..=Price64::from(raw));
                            let mine = root.simulation.as_ref().map(|s| s.report.order_id);
                            let position = orders.iter().position(|o| Some(o.id) == mine);
                            snapshot.queue_price = Some(selected_price);
                            snapshot.orders_ahead = position;
                            snapshot.quantity_ahead = position.map(|i| {
                                orders[..i]
                                    .iter()
                                    .map(|o| instrument.quantity(o.quantity))
                                    .sum()
                            });
                            snapshot.queue = orders
                                .iter()
                                .take(512)
                                .map(|o| QueueEntry {
                                    id: o.id.to_string(),
                                    price: instrument.price(o.price.into()),
                                    quantity: instrument.quantity(o.quantity),
                                    mine: Some(o.id) == mine,
                                })
                                .collect();
                        }
                    }
                }
                if let Some(report) = root
                    .simulation
                    .as_ref()
                    .map(|s| &s.report)
                    .or(root.market_preview.as_ref())
                {
                    snapshot.simulation = Some(Simulation {
                        requested: instrument.quantity(report.requested),
                        filled: instrument.quantity(report.filled),
                        remaining: instrument.quantity(report.remaining()),
                        average_price: report
                            .average_price()
                            .map(|p| p / 10f64.powi(instrument.price_decimals as i32)),
                        alternate: root.simulation.is_some(),
                        stopped: root
                            .simulation
                            .as_ref()
                            .and_then(|s| s.stopped_reason.clone()),
                        fills: report
                            .executions
                            .iter()
                            .rev()
                            .take(200)
                            .map(|e| Fill {
                                clock_ns: e.timestamp_ns(),
                                price: instrument.price(e.event().execution.price.into()),
                                quantity: instrument.quantity(e.event().execution.quantity),
                                maker: e.event().maker_order_id == report.order_id,
                            })
                            .collect(),
                    });
                }
                Ok(snapshot)
            })
            .map_err(|e| anyhow!(e))
    }
    pub fn simulate(
        &self,
        side: OrderSide,
        kind: OrderKind,
        quantity: f64,
        price: Option<f64>,
    ) -> Result<()> {
        self.session
            .with(move |adapter| {
                let i = adapter
                    .state()
                    .instruments
                    .get(&adapter.state().selected)
                    .ok_or("Waiting for instrument")?;
                let quantity = atoms(quantity, i.quantity_decimals)?;
                match kind {
                    OrderKind::Market => adapter.simulate_market(MarketOrder::new(
                        quantity,
                        Uuid::new_v4(),
                        side.native(),
                    )),
                    OrderKind::Limit => {
                        let price = atoms(price.ok_or("limit requires price")?, i.price_decimals)?;
                        adapter.simulate(LimitOrder::new(
                            Some(Price64::from(price)),
                            quantity,
                            Uuid::new_v4(),
                            side.native(),
                        ))
                    }
                }
            })
            .map_err(|e| anyhow!(e))
    }
    pub fn submit(&self, command: Command) -> Result<String> {
        if self.source != SourceKind::Demo {
            bail!(
                "native order entry is available in demo; configure --order-endpoint for a hosted book"
            );
        }
        self.session
            .with(move |adapter| {
                let symbol = adapter.state().selected.clone();
                let id = command.order_id();
                adapter.submit_command(&symbol, command)?;
                adapter.state_mut().commit();
                Ok(format!("accepted {id}"))
            })
            .map_err(|e| anyhow!(e))
    }
    pub fn transport(&self, paused: bool, speed: f64) -> Result<()> {
        if self.source == SourceKind::Demo {
            let mut control = self.demo_control.lock().unwrap();
            control.paused = paused;
            control.speed = speed;
            return Ok(());
        }
        if !matches!(self.source, SourceKind::Itch | SourceKind::Nasdaq) {
            bail!("live feeds keep receiving; replay transport controls are unavailable");
        }
        use lobo_replay::custom::runtime::PlaybackCommand;
        self.session
            .handle()
            .playback(PlaybackCommand::Speed { speed })
            .map_err(|e| anyhow!(e))?;
        self.session
            .handle()
            .playback(if paused {
                PlaybackCommand::Pause
            } else {
                PlaybackCommand::Play
            })
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }
    pub fn set_bars(&self, kind: crate::args::BarKind, size: u64) -> Result<()> {
        let bars = crate::args::aggregation(kind, size)?;
        let store = self.store.clone();
        self.session
            .with(move |a| {
                for history in store.lock().unwrap().values_mut() {
                    history.candles.clear();
                }
                a.state_mut().set_bar_aggregation(bars);
                if let Some(branch) = &mut a.state_mut().simulation {
                    branch.feed.set_bar_aggregation(bars);
                }
                Ok(())
            })
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }
    pub fn set_history(&self, seconds: f64) -> Result<()> {
        let interval = self.interval_ns.clone();
        let capacity = self.capacity;
        let store = self.store.clone();
        self.session
            .with(move |_| {
                interval.store(
                    (seconds * 1e9 / f64::from(capacity)).max(1.0) as u64,
                    Ordering::Relaxed,
                );
                for h in store.lock().unwrap().values_mut() {
                    h.depth.clear();
                    h.flow.clear();
                    h.pending_flow = Flow::default();
                }
                Ok(())
            })
            .map_err(|e| anyhow!(e))
    }
}
pub fn atoms(value: f64, decimals: u8) -> Result<u64, String> {
    let raw = value * 10f64.powi(decimals as i32);
    if !raw.is_finite() || raw < 1.0 || raw >= u64::MAX as f64 {
        return Err("value is outside instrument precision/range".into());
    }
    if (raw - raw.round()).abs() > (raw.abs() * f64::EPSILON * 4.0).max(1e-7) {
        return Err(format!("value has more than {decimals} decimal places"));
    }
    Ok(raw.round() as u64)
}
fn scope(args: &Args) -> Result<BookScope> {
    if args.scope == "all" {
        return Ok(BookScope::All);
    }
    let symbols: BTreeSet<String> = match args.scope.as_str() {
        "selected" => BTreeSet::from([args.ticker()]),
        "top-tech" => [
            "AAPL", "MSFT", "AMZN", "GOOGL", "GOOG", "META", "NVDA", "TSLA", "NFLX",
        ]
        .into_iter()
        .map(str::to_string)
        .chain([args.ticker()])
        .collect(),
        "sp500" => {
            let catalog: serde_json::Value =
                serde_json::from_str(include_str!("../../../../web/lib/ticker-categories.json"))?;
            let mut symbols: BTreeSet<String> = catalog["holdings"]
                .as_object()
                .ok_or_else(|| anyhow!("invalid bundled SPY holdings"))?
                .keys()
                .cloned()
                .collect();
            symbols.insert(args.ticker());
            symbols
        }
        text => text
            .split(',')
            .map(|s| lobo_replay::custom::normalize_symbol(s).map_err(|e| anyhow!(e)))
            .collect::<Result<_>>()?,
    };
    if !symbols.contains(&args.ticker()) {
        bail!("--symbol must belong to --scope");
    }
    Ok(BookScope::Selected(symbols))
}
