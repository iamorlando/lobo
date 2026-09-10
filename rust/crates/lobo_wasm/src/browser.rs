use crate::{
    deferred::{RecentBars, compact_levels},
    presentation,
    renderer::Renderer,
};
use arrow_array::{RecordBatch, builder::FixedSizeBinaryBuilder};
use arrow_schema::{DataType, Field, Schema};
use lobo_adapters::adapter::market::{adapters, create_adapter};
use lobo_batchers::{
    Aggregation, PriceLevelMetrics,
    arrow::{batch::PriceLevelArrowBatcher, sink::GpuCanvasSink},
    traits::{Batch, Sink},
};
use lobo_replay::custom::{BookLevel, FeedMode, MarketDataAdapter};
use std::{collections::HashMap, sync::Arc};
use wasm_bindgen::prelude::*;
type BarEvent = lobo_events::BookEvent<lobo_events::VolumeBar<lobo_primitives::Price64>>;
struct ChartBook {
    index: u32,
    generation: u64,
    prices: HashMap<u64, u32>,
    bar_revision: u64,
}
struct QueueSelection {
    side: lobo_models::Side,
    lower: lobo_primitives::Price64,
    upper: lobo_primitives::Price64,
}
#[wasm_bindgen]
pub struct FeedSession {
    metrics: PriceLevelMetrics,
    adapter: Box<dyn MarketDataAdapter>,
    canvas: web_sys::HtmlCanvasElement,
    sink: GpuCanvasSink<Renderer>,
    charts: HashMap<String, ChartBook>,
    price_slots: u32,
    next_book: u32,
    free_books: Vec<u32>,
    free_slots: Vec<u32>,
    queue_selection: Option<QueueSelection>,
    deferred_bars: HashMap<String, RecentBars<BarEvent>>,
    deferred_record_budget: u32,
}
impl FeedSession {
    /// Construct the existing GPU viewer from any custom adapter implementation.
    pub async fn with_adapter(
        canvas: web_sys::HtmlCanvasElement,
        adapter: Box<dyn MarketDataAdapter>,
    ) -> Result<Self, JsValue> {
        let sink = GpuCanvasSink::new(canvas.clone(), Renderer::new)
            .await
            .map_err(error)?;
        Ok(Self {
            metrics: PriceLevelMetrics::empty(),
            adapter,
            canvas,
            sink,
            charts: HashMap::new(),
            price_slots: 0,
            next_book: 0,
            free_books: Vec::new(),
            free_slots: Vec::new(),
            queue_selection: None,
            deferred_bars: HashMap::new(),
            deferred_record_budget: 0,
        })
    }

    fn price_selection(&mut self, x: f32, y: f32) -> Option<(u32, QueueSelection)> {
        let bin = presentation::bin_at(x, y)?;
        let midpoint = presentation::midpoint(self.bid(), self.ask());
        let renderer = self.sink.renderer_mut();
        if renderer.bars.enabled {
            return None;
        }
        let center = renderer.center();
        let span = renderer.price_span();
        let minimum = center - span / 2.0;
        let low = minimum + span * bin as f32 / presentation::BINS as f32;
        let high = minimum + span * (bin + 1) as f32 / presentation::BINS as f32;
        let state = self.adapter.state().view();
        let instrument = state.instruments.get(&state.selected)?;
        let scale = 10f64.powi(instrument.price_decimals as i32);
        let lower = (f64::from(low) * scale).ceil().max(0.0) as u64;
        let upper = ((f64::from(high) * scale).ceil() - 1.0).max(0.0) as u64;
        if lower > upper {
            return None;
        }
        // Split a bin containing both quotes at the actual spread midpoint so
        // either side remains selectable at coarse display aggregation.
        let price = f64::from(center)
            + f64::from(span) * (f64::from(presentation::price_fraction(y)) - 0.5);
        Some((
            bin,
            QueueSelection {
                side: if price < midpoint {
                    lobo_models::Side::Buy
                } else {
                    lobo_models::Side::Sell
                },
                lower: lower.into(),
                upper: upper.into(),
            },
        ))
    }
}
#[wasm_bindgen]
pub fn chart_layout() -> String {
    presentation::layout_json()
}

fn error(message: impl std::fmt::Display) -> JsValue {
    js_sys::Error::new(&message.to_string()).into()
}
#[wasm_bindgen]
pub fn available_adapters() -> String {
    serde_json::to_string(&adapters().iter().map(|info|serde_json::json!({
        "id":info.id,"name":info.name,"mode":info.mode.as_str(),
        "endpoint":info.endpoint,"defaultSymbol":info.default_symbol,"timezone":info.timezone,"supportsTrades":info.supports_trades,"level":info.level.as_str(),
    })).collect::<Vec<_>>()).unwrap()
}
#[wasm_bindgen]
impl FeedSession {
    #[wasm_bindgen(getter)]
    pub fn gpu_backend(&self) -> String {
        format!("{:?}", self.sink.backend())
    }

    pub async fn create(
        canvas: web_sys::HtmlCanvasElement,
        adapter_id: String,
        ticker: String,
        start_ms: f64,
    ) -> Result<FeedSession, JsValue> {
        Self::create_scoped(canvas, adapter_id, ticker, start_ms, "null".into()).await
    }
    pub async fn create_observed(
        canvas: web_sys::HtmlCanvasElement,
        descriptor_json: String,
        ticker: String,
    ) -> Result<FeedSession, JsValue> {
        Self::create_observed_scoped(canvas, descriptor_json, ticker, "null".into()).await
    }
    /// Limit this browser's books while the hosted feed retains shared subscriptions.
    pub async fn create_observed_scoped(
        canvas: web_sys::HtmlCanvasElement,
        descriptor_json: String,
        ticker: String,
        scope_json: String,
    ) -> Result<FeedSession, JsValue> {
        let descriptor = lobo_replay::custom::observer::parse_descriptor(
            serde_json::from_str(&descriptor_json).map_err(error)?,
        )
        .map_err(error)?;
        let protocol =
            lobo_replay::custom::observer::ObservedProtocol::new(descriptor.clone(), &ticker)
                .map_err(error)?;
        let mut adapter = lobo_replay::custom::CustomAdapter::new(descriptor, protocol, &ticker)
            .map_err(error)?;
        let symbols: Option<Vec<String>> = serde_json::from_str(&scope_json).map_err(error)?;
        if let Some(symbols) = symbols {
            adapter
                .set_book_scope(lobo_context::BookScope::Selected(
                    symbols
                        .iter()
                        .map(|s| lobo_replay::custom::normalize_symbol(s))
                        .collect::<Result<_, _>>()
                        .map_err(error)?,
                ))
                .map_err(error)?;
        }
        Self::with_adapter(canvas, Box::new(adapter)).await
    }
    /// null means All; a JSON array is a fixed scope for this session's lifetime.
    pub async fn create_scoped(
        canvas: web_sys::HtmlCanvasElement,
        adapter_id: String,
        ticker: String,
        start_ms: f64,
        scope_json: String,
    ) -> Result<FeedSession, JsValue> {
        if !start_ms.is_finite() || !(0.0..86_400_000.0).contains(&start_ms) {
            return Err(error("Invalid session start time"));
        }
        let mut adapter =
            create_adapter(&adapter_id, &ticker, (start_ms * 1e6) as u64).map_err(error)?;
        let symbols: Option<Vec<String>> = serde_json::from_str(&scope_json).map_err(error)?;
        let scope = match symbols {
            None => lobo_context::BookScope::All,
            Some(symbols) => lobo_context::BookScope::Selected(
                symbols
                    .into_iter()
                    .map(|s| lobo_adapters::adapter::market::normalize_symbol(&s))
                    .collect::<Result<_, _>>()
                    .map_err(error)?,
            ),
        };
        adapter.set_book_scope(scope).map_err(error)?;
        Self::with_adapter(canvas, adapter).await
    }
    pub fn append(&mut self, bytes: &[u8], eof: bool) -> Result<(), JsValue> {
        self.adapter.receive(bytes, eof).map_err(error)
    }
    /// Configure the observer once before input; native publication is unchanged.
    pub fn set_observer_metrics(&mut self, bits: u32) -> Result<(), JsValue> {
        if self.adapter.state().messages != 0 {
            return Err(error("Set observer metrics before the feed starts"));
        }
        self.metrics = PriceLevelMetrics::from_bits(bits)
            .ok_or_else(|| error("unknown price-level metrics"))?;
        Ok(())
    }
    pub fn set_loading_tickers(&mut self, tickers: Vec<String>) {
        self.sink.renderer_mut().loading.set_tickers(tickers);
    }
    /// An empty sink submission presents only the animated glyph pipeline.
    /// It neither consumes pending events nor allocates chart pages/history.
    pub fn render_loading(&mut self, seconds: f32, pixel_ratio: f32) -> Result<(), JsValue> {
        if !seconds.is_finite() || !pixel_ratio.is_finite() || pixel_ratio <= 0.0 {
            return Err(error("Invalid loading viewport"));
        }
        let renderer = self.sink.renderer_mut();
        renderer.loading.active = true;
        renderer.loading.time = seconds;
        renderer.loading.scale = pixel_ratio;
        renderer.width = self.canvas.width();
        renderer.height = self.canvas.height();
        self.sink
            .write(&RecordBatch::new_empty(Arc::new(Schema::empty())))
            .map_err(error)?;
        Ok(())
    }
    /// Inputs use displayed instrument units; conversion happens once at the UI boundary.
    pub fn simulate(&mut self, side: &str, price: f64, quantity: f64) -> Result<(), JsValue> {
        use lobo_models::orders::order_types::LimitOrder;
        use lobo_primitives::{Price64, uuid::Uuid};
        let state = self.adapter.state();
        let instrument = state
            .instruments
            .get(&state.selected)
            .ok_or_else(|| error("No instrument selected"))?;
        let side = parse_side(side)?;
        let order = LimitOrder::new(
            Some(Price64::from(atoms(price, instrument.price_decimals)?)),
            atoms(quantity, instrument.quantity_decimals)?,
            Uuid::new_v4(),
            side,
        );
        self.adapter.simulate(order).map_err(error)?;
        self.follow_simulated_order();
        Ok(())
    }
    pub fn simulate_market(&mut self, side: &str, quantity: f64) -> Result<(), JsValue> {
        use lobo_models::orders::order_types::MarketOrder;
        let state = self.adapter.state();
        let instrument = state
            .instruments
            .get(&state.selected)
            .ok_or_else(|| error("No instrument selected"))?;
        self.adapter
            .simulate_market(MarketOrder::new(
                atoms(quantity, instrument.quantity_decimals)?,
                lobo_primitives::uuid::Uuid::new_v4(),
                parse_side(side)?,
            ))
            .map_err(error)
    }
    pub fn follow_simulated_order(&mut self) {
        if let Some(simulation) = &self.adapter.state().simulation {
            if let Some(price) = simulation.report.price {
                self.queue_selection = Some(QueueSelection {
                    side: simulation.report.side,
                    lower: price,
                    upper: price,
                });
            }
        }
    }
    pub fn clear_queue(&mut self) {
        self.queue_selection = None;
    }
    /// Select a displayed aggregation bin using chart coordinates, never pixels
    /// read back from the GPU. Query the current native book at this price range.
    pub fn select_queue_at(&mut self, x: f32, y: f32) -> bool {
        if self.adapter.info().level != BookLevel::L3 {
            return false;
        }
        let Some((_, selection)) = self.price_selection(x, y) else {
            return false;
        };
        self.queue_selection = Some(selection);
        true
    }
    /// Hover uses exactly the same bin/price/side calculation as queue selection.
    /// This returns native camera metadata, never GPU pixels or buffer contents.
    pub fn price_band_at(&mut self, x: f32, y: f32) -> String {
        let Some((bin, selection)) = self.price_selection(x, y) else {
            return "null".into();
        };
        let state = self.adapter.state().view();
        let instrument = &state.instruments[&state.selected];
        serde_json::json!({
            "bin": bin,
            "side": if selection.side == lobo_models::Side::Buy { "buy" } else { "sell" },
            "lower": instrument.price(selection.lower.into()),
            "upper": instrument.price(selection.upper.into()),
        })
        .to_string()
    }
    /// Interleaved bid/ask quantity per display bucket. Read native levels once;
    /// preserve fractional instruments and never scan orders or read GPU memory.
    pub fn depth_quantities(&mut self) -> Vec<f64> {
        use lobo_storage::price_level::{HasHiddenQuantity, PriceLevelContract};
        let renderer = self.sink.renderer_mut();
        let minimum = renderer.center() - renderer.price_span() / 2.0;
        let span = renderer.price_span();
        let mut quantities = [0u128; presentation::BINS as usize * 2];
        let state = self.adapter.state().view();
        let Some(book) = state.selected_book() else {
            return vec![0.0; quantities.len()];
        };
        let instrument = &state.instruments[&state.selected];
        lobo_adapters::adapter::market::dispatch_feed_book!(
            book,
            native,
            for (price, level) in native
                .order_storage
                .bids
                .visible_price_levels()
                .chain(native.order_storage.asks.visible_price_levels())
            {
                let fraction = (instrument.price((*price).into()) as f32 - minimum) / span;
                if (0.0..1.0).contains(&fraction) {
                    let bin = (fraction * presentation::BINS as f32) as usize;
                    let side = usize::from(level.side() == lobo_models::Side::Sell);
                    quantities[bin * 2 + side] += u128::from(level.visible_quantity())
                        + u128::from(self.metrics.hidden_quantity(level.hidden_quantity()));
                }
            }
        );
        let scale = 10f64.powi(instrument.quantity_decimals as i32);
        quantities.into_iter().map(|q| q as f64 / scale).collect()
    }
    pub fn queue_status(&self) -> String {
        let Some(selection) = &self.queue_selection else {
            return "null".into();
        };
        let root = self.adapter.state();
        let state = root.view();
        let Some(book) = state.selected_book() else {
            return "null".into();
        };
        let instrument = &state.instruments[&state.selected];
        let own = root.simulation.as_ref().map(|s| s.report.order_id);
        let orders = book.queue_view(selection.side, selection.lower..=selection.upper);
        let position = orders.iter().position(|o| Some(o.id) == own);
        let ahead = position.map(|i| orders[..i].iter().map(|o| o.quantity).sum::<u64>());
        serde_json::json!({
            "symbol":state.selected,"side":if selection.side==lobo_models::Side::Buy {"buy"} else {"sell"},
            "lower":instrument.price(selection.lower.into()),"upper":instrument.price(selection.upper.into()),
            "timestampMs":state.clock_ns as f64/1e6,"ordersAhead":position,
            "quantityAhead":ahead.map(|q|instrument.quantity(q)),
            "totalQuantity":instrument.quantity(orders.iter().map(|o|o.quantity).sum()),
            "orders":orders.iter().map(|o|serde_json::json!({
                "id":o.id.to_string(),"price":instrument.price(o.price.into()),"quantity":instrument.quantity(o.quantity),
                "timestampNs":o.created_at.timestamp_nanos_opt().map(|t|t.to_string()),
                "mine":Some(o.id)==own,
            })).collect::<Vec<_>>()
        }).to_string()
    }
    pub fn return_to_main(&mut self) {
        if let Some(simulation) = &self.adapter.state().simulation {
            self.deferred_bars.remove(&simulation.feed.namespace);
            let key = simulation.feed.chart_key(&simulation.feed.selected);
            if let Some(chart) = self.charts.remove(&key) {
                let renderer = self.sink.renderer_mut();
                renderer.release_slots(chart.prices.values().copied());
                renderer.invalidate(chart.index);
                renderer.bars.reset_book(chart.index);
                self.free_slots.extend(chart.prices.into_values());
                self.free_books.push(chart.index);
            }
        }
        self.adapter.return_to_main();
        self.clear_queue();
    }
    /// Explicit simulated execution messages; no GPU readback and no market-trade mixing.
    pub fn simulation_status(&self) -> String {
        let state = self.adapter.state();
        let branch = state.simulation.as_ref();
        let Some(simulation) = branch.map(|s| &s.report).or(state.market_preview.as_ref()) else {
            return "null".into();
        };
        let instrument = &state.instruments[simulation.symbol.as_ref()];
        serde_json::json!({
            "simulated":true, "orderId":simulation.order_id.to_string(),
            "symbol":simulation.symbol.as_ref(), "side":if simulation.side == lobo_models::Side::Buy {"buy"} else {"sell"},
            "price":simulation.price.map(|p|instrument.price(p.into())),
            "kind":if simulation.price.is_some() {"limit"} else {"market"},
            "alternateTimeline":branch.is_some(),
            "averagePrice":simulation.average_price().map(|p|p/10f64.powi(instrument.price_decimals as i32)),
            "startedMs":simulation.started_ns as f64 / 1e6,
            "requested":instrument.quantity(simulation.requested),
            "filled":instrument.quantity(simulation.filled), "remaining":instrument.quantity(simulation.remaining()),
            "complete":simulation.complete(), "ignored":branch.map_or(0, |s|s.ignored),
            "stoppedReason":branch.and_then(|s|s.stopped_reason.as_deref()),
            "executionCount":simulation.executions.len(),
            "executions":simulation.executions.iter().rev().take(50).map(|message| {
                let event = message.event();
                serde_json::json!({ "type":"simulated_execution", "simulated":true,
                    "timeMs":message.timestamp_ns() as f64 / 1e6,
                    "price":instrument.price(event.execution.price.into()),
                    "quantity":instrument.quantity(event.execution.quantity),
                    "maker":event.maker_order_id == simulation.order_id,
                    "sequence":message.sequence_number(),
                })
            }).collect::<Vec<_>>()
        }).to_string()
    }
    pub fn advance(&mut self, elapsed_ms: f64, budget: u32) -> Result<(), JsValue> {
        if !elapsed_ms.is_finite() || elapsed_ms < 0.0 {
            return Err(error("Invalid playback time"));
        }
        self.adapter
            .advance((elapsed_ms * 1e6) as u64, budget.clamp(1, 100000) as usize)
            .map_err(error)?;
        if self.adapter.info().mode == FeedMode::Replay {
            let state = self.adapter.state();
            let Ok(()) = state
                .volume_bars()
                .borrow_mut()
                .advance_time(state.clock_ns);
            if let Some(simulation) = &state.simulation {
                let Ok(()) = simulation
                    .feed
                    .volume_bars()
                    .borrow_mut()
                    .advance_time(simulation.feed.clock_ns);
            }
        }
        Ok(())
    }
    /// Advance the same native books and aggregators without submitting a frame.
    /// Bound presentation messages outside storage mutations.
    pub fn advance_without_render(&mut self, elapsed_ms: f64, budget: u32) -> Result<(), JsValue> {
        self.advance(elapsed_ms, budget)?;
        self.sink.renderer_mut().history_gap = true;
        // Amortize level-map scans over a bounded number of records. Completed
        // candles are drained every batch; a single execution can produce many.
        self.deferred_record_budget += budget.clamp(1, 100000);
        let compact = self.deferred_record_budget >= 100000;
        if compact {
            self.deferred_record_budget = 0;
        }
        let root = self.adapter.state();
        for feed in std::iter::once(root).chain(root.simulation.iter().map(|s| &s.feed)) {
            if compact {
                for (symbol, output) in &feed.outputs {
                    let chart = self.charts.get(&feed.chart_key(symbol));
                    compact_levels(&mut output.borrow_mut().pending, chart.map(|c| &c.prices));
                }
            }
            let recent = self
                .deferred_bars
                .entry(feed.namespace.clone())
                .or_default();
            let bars = feed.volume_bars().borrow();
            for event in bars.destination.0.borrow_mut().drain(..) {
                // Clone just the event's shared book id before moving the message.
                recent.push(&event.shared_book_id(), event);
            }
        }
        Ok(())
    }
    /// The minimum chart range is a fraction of each book's centered price;
    /// cameras widen independently to keep both best quotes visible. Each
    /// instrument's raw price units are converted only at the GPU upload boundary.
    pub fn render(&mut self, window_seconds: f32, range_fraction: f32) -> Result<(), JsValue> {
        if !window_seconds.is_finite()
            || !range_fraction.is_finite()
            || window_seconds < 10.0
            || range_fraction <= 0.0
        {
            return Err(error("Invalid chart range"));
        }
        let replay = self.adapter.info().mode == FeedMode::Replay;
        self.deferred_record_budget = 0;
        let root = self.adapter.state();
        let feed = root;
        let renderer = self.sink.renderer_mut();
        renderer.configure(window_seconds, range_fraction);
        renderer.loading.active = false;
        renderer.width = self.canvas.width();
        renderer.height = self.canvas.height();
        let warming = replay && feed.warming;
        if renderer.warming && !warming {
            for view in &mut renderer.views {
                view.centered = false;
            }
        }
        renderer.warming = warming;
        renderer.elapsed = feed.clock_ns.saturating_sub(feed.start_ns.unwrap_or(0)) as f64 / 1e9;
        let feeds = || std::iter::once(root).chain(root.simulation.iter().map(|s| &s.feed));
        let rows = feeds()
            .flat_map(|feed| feed.outputs.values())
            .map(|output| output.borrow().pending.len())
            .sum::<usize>();
        let mut batcher = PriceLevelArrowBatcher::with_metrics(rows + 1, self.metrics);
        let mut routes = FixedSizeBinaryBuilder::with_capacity(rows, 16);
        for feed in feeds() {
            let deferred = renderer.history_gap;
            if let Some(mut recent) = self.deferred_bars.remove(&feed.namespace) {
                let bars = feed.volume_bars().borrow();
                let mut pending = bars.destination.0.borrow_mut();
                // Normal advances may have followed the last undrawn advance.
                let newer = std::mem::take(&mut *pending);
                pending.extend(recent.drain());
                pending.extend(newer);
            }
            for (symbol, output) in &feed.outputs {
                let mut state = output.borrow_mut();
                if deferred {
                    let chart = self.charts.get(&feed.chart_key(symbol));
                    compact_levels(&mut state.pending, chart.map(|c| &c.prices));
                }
                let instrument = &feed.instruments[symbol];
                if !replay && !state.synchronized {
                    if let Some(chart) = self.charts.get(&feed.chart_key(symbol)) {
                        renderer.invalidate(chart.index);
                    }
                    continue;
                }
                if state.count == 0 {
                    continue;
                }
                let chart = self
                    .charts
                    .entry(feed.chart_key(symbol))
                    .or_insert_with(|| ChartBook {
                        index: self.free_books.pop().unwrap_or_else(|| {
                            let index = self.next_book;
                            self.next_book += 1;
                            index
                        }),
                        generation: state.generation,
                        prices: HashMap::new(),
                        bar_revision: 0,
                    });
                if chart.generation != state.generation {
                    renderer.invalidate(chart.index);
                    chart.generation = state.generation;
                }
                let bid = feed
                    .book(symbol)
                    .and_then(|book| book.best_price(lobo_models::Side::Buy))
                    .map(|p| instrument.price(p.into()));
                let ask = feed
                    .book(symbol)
                    .and_then(|book| book.best_price(lobo_models::Side::Sell))
                    .map(|p| instrument.price(p.into()));
                renderer.camera(chart.index, bid, ask).map_err(error)?;
                let frozen = root
                    .simulation
                    .as_ref()
                    .filter(|s| s.stopped() && feed.namespace == s.feed.namespace)
                    .map(|s| {
                        s.feed.clock_ns.saturating_sub(root.start_ns.unwrap_or(0)) as f64 / 1e9
                    });
                renderer.freeze_at(chart.index, frozen);
                for ((price, _), event) in std::mem::take(&mut state.pending) {
                    let slot = *chart.prices.entry(price).or_insert_with(|| {
                        if let Some(slot) = self.free_slots.pop() {
                            return slot;
                        }
                        let next = self.price_slots;
                        self.price_slots += 1;
                        next
                    });
                    if self.price_slots > 8_388_608 {
                        return Err(error("Shared GPU price capacity exceeded"));
                    }
                    batcher.push(&event).map_err(error)?;
                    let values = [
                        slot,
                        chart.index,
                        (instrument.price(price) as f32).to_bits(),
                        (self
                            .metrics
                            .hidden_quantity(event.event().hidden_quantity())
                            as f32)
                            .to_bits(),
                    ];
                    let bytes = values
                        .into_iter()
                        .flat_map(u32::to_le_bytes)
                        .collect::<Vec<_>>();
                    routes.append_value(bytes).map_err(error)?;
                }
            }
            {
                let bars = feed.volume_bars().borrow();
                bars.destination.0.borrow_mut().retain(|event| {
                    if let Some(chart) = self.charts.get(&feed.chart_key(event.book_id())) {
                        let instrument = &feed.instruments[event.book_id()];
                        let bar = event.event();
                        renderer.bars.push(
                            chart.index,
                            bar.index,
                            [
                                instrument.price(bar.open.into()) as f32,
                                instrument.price(bar.high.into()) as f32,
                                instrument.price(bar.low.into()) as f32,
                                instrument.price(bar.close.into()) as f32,
                                instrument.quantity(bar.volume) as f32,
                            ],
                            bar.start_ns,
                        );
                        false
                    } else {
                        true
                    }
                });
                for (symbol, forming, revision) in bars.states() {
                    if let Some(chart) = self.charts.get_mut(&feed.chart_key(symbol)) {
                        if chart.bar_revision == revision {
                            continue;
                        }
                        chart.bar_revision = revision;
                        if let Some(bar) = forming {
                            let instrument = &feed.instruments[symbol];
                            renderer.bars.push(
                                chart.index,
                                bar.index,
                                [
                                    instrument.price(bar.open.into()) as f32,
                                    instrument.price(bar.high.into()) as f32,
                                    instrument.price(bar.low.into()) as f32,
                                    instrument.price(bar.close.into()) as f32,
                                    instrument.quantity(bar.volume) as f32,
                                ],
                                bar.start_ns,
                            );
                        }
                    }
                }
                if feed.namespace == root.view().namespace {
                    renderer.bars.completed = bars.completed(&feed.selected);
                    renderer.bars.forming = bars.forming(&feed.selected).is_some();

                    let quotes = bars.quotes(&feed.selected);
                    if let Some(instrument) = feed.instruments.get(&feed.selected) {
                        renderer.bars.bid = quotes[0]
                            .and_then(|quote| quote.price)
                            .map_or(0.0, |price| instrument.price(price.into()) as f32);
                        renderer.bars.ask = quotes[1]
                            .and_then(|quote| quote.price)
                            .map_or(0.0, |price| instrument.price(price.into()) as f32);
                    }
                }
            }
        }
        let active = root.view();
        renderer.selected = self
            .charts
            .get(&active.chart_key(&active.selected))
            .map(|chart| chart.index);
        renderer.price_slots = self.price_slots;
        let batch = <PriceLevelArrowBatcher as Batch<
            lobo_events::PriceLevelChangeEvent<lobo_primitives::Price64>,
        >>::flush(&mut batcher)
        .map_err(error)?
        .unwrap_or_else(|| RecordBatch::new_empty(batcher.schema()));
        let mut fields = batch.schema().fields().to_vec();
        fields.push(Arc::new(Field::new(
            "gpu_route",
            DataType::FixedSizeBinary(16),
            false,
        )));
        let mut columns = batch.columns().to_vec();
        columns.push(Arc::new(routes.finish()));
        self.sink
            .write(&RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).map_err(error)?)
            .map_err(error)?;
        Ok(())
    }
    pub fn bootstrap_requests(&self) -> String {
        serde_json::json!(
            self.adapter
                .bootstrap_requests()
                .iter()
                .map(|r| serde_json::json!({"id":r.id,"url":r.url}))
                .collect::<Vec<_>>()
        )
        .to_string()
    }
    pub fn bootstrap(&mut self, id: &str, bytes: &[u8]) -> Result<(), JsValue> {
        self.adapter.bootstrap(id, bytes).map_err(error)
    }
    pub fn connections(&self) -> String {
        serde_json::json!(
            self.adapter
                .connections()
                .iter()
                .map(|c| serde_json::json!({"id":c.id,"endpoint":c.endpoint,"selected":c.selected}))
                .collect::<Vec<_>>()
        )
        .to_string()
    }
    pub fn socket_receive(&mut self, id: u32, bytes: &[u8]) -> Result<(), JsValue> {
        self.adapter.receive_on(id, bytes).map_err(error)
    }
    pub fn socket_connected(&mut self, id: u32) -> Result<(), JsValue> {
        self.adapter.connected_on(id).map_err(error)
    }
    pub fn socket_disconnected(&mut self, id: u32) {
        self.adapter.disconnected_on(id);
    }
    pub fn socket_keepalive(&mut self, id: u32) {
        self.adapter.keepalive_on(id);
    }
    pub fn socket_commands(&mut self, id: u32) -> Vec<String> {
        self.adapter.commands_on(id)
    }
    pub fn simulation_note(&self) -> Option<String> {
        self.adapter.simulation_note().map(str::to_owned)
    }
    pub fn connected(&mut self) -> Result<(), JsValue> {
        self.adapter.connected().map_err(error)
    }
    pub fn disconnected(&mut self) {
        self.adapter.disconnected();
    }
    pub fn keepalive(&mut self) {
        self.adapter.keepalive();
    }
    pub fn commands(&mut self) -> Vec<String> {
        self.adapter.commands()
    }
    pub fn tickers(&self) -> Vec<String> {
        self.adapter.tickers()
    }
    pub fn scoped_tickers(&self) -> Vec<String> {
        self.adapter.scoped_tickers()
    }
    pub fn select_ticker(&mut self, ticker: &str) -> Result<(), JsValue> {
        self.adapter.select_ticker(ticker).map_err(error)?;
        self.clear_queue();
        self.sink.renderer_mut().selected = self
            .charts
            .get(&self.adapter.state().selected)
            .map(|c| c.index);
        Ok(())
    }
    pub fn show_volume_bars(&mut self, enabled: bool) {
        self.sink.renderer_mut().bars.enabled = enabled;
    }
    /// Recolor cached GPU history without changing the session or its cameras.
    pub fn set_chart_theme(&mut self, colors: &[f32]) -> Result<(), JsValue> {
        self.sink.renderer_mut().palette.set(colors).map_err(error)
    }
    pub fn set_volume_bar_size(&mut self, size: u32) -> Result<(), JsValue> {
        self.set_bar_aggregation("volume", size)
    }
    /// Configuration is selected outside replay; the typed publisher routes stay unchanged.
    pub fn set_bar_aggregation(&mut self, kind: &str, size: u32) -> Result<(), JsValue> {
        let size = std::num::NonZeroU64::new(size as u64)
            .ok_or_else(|| error("Bar size must be positive"))?;
        let aggregation = match kind {
            "volume" => Aggregation::Volume(size),
            "ticks" => Aggregation::Ticks(size),
            "time" => Aggregation::Time(
                size.checked_mul(
                    std::num::NonZeroU64::new(1_000_000_000).expect("positive constant"),
                )
                .ok_or_else(|| error("Time interval too large"))?,
            ),
            "notional" => Aggregation::Notional(size),
            _ => return Err(error("Unknown bar aggregation")),
        };
        let state = self.adapter.state_mut();
        state.set_bar_aggregation(aggregation);
        if let Some(simulation) = &mut state.simulation {
            simulation.feed.set_bar_aggregation(aggregation);
        }
        self.sink.renderer_mut().bars.reset();
        self.deferred_bars.clear();
        for chart in self.charts.values_mut() {
            chart.bar_revision = 0;
        }
        Ok(())
    }
    #[wasm_bindgen(getter)]
    pub fn forming_progress(&self) -> f64 {
        let state = self.adapter.state().view();
        let bars = state.volume_bars().borrow();
        let progress = bars.progress(&state.selected, state.clock_ns);
        if matches!(bars.aggregation(), Aggregation::Time(_)) {
            progress / 1e9
        } else {
            progress
        }
    }
    /// Axis metadata already available from messages, never read from the GPU.
    pub fn bar_timestamps(&mut self) -> Vec<f64> {
        let renderer = self.sink.renderer_mut();
        renderer.bars.timestamps(renderer.selected)
    }
    pub fn pan_price(&mut self, fraction: f32) {
        if fraction.is_finite() {
            self.sink
                .renderer_mut()
                .pan_price(fraction.clamp(-1.0, 1.0));
        }
    }
    #[wasm_bindgen(getter)]
    pub fn volume_bar_count(&self) -> f64 {
        let state = self.adapter.state().view();
        state.volume_bars().borrow().completed(&state.selected) as f64
    }
    #[wasm_bindgen(getter)]
    pub fn forming_volume(&self) -> f64 {
        let state = self.adapter.state().view();
        state
            .volume_bars()
            .borrow()
            .forming(&state.selected)
            .map_or(0.0, |b| b.volume as f64)
    }
    pub fn recenter(&mut self) {
        self.sink.renderer_mut().recenter();
    }
    #[wasm_bindgen(getter)]
    pub fn ticker_count(&self) -> usize {
        self.adapter.state().instruments.len()
    }
    #[wasm_bindgen(getter)]
    pub fn active_books(&self) -> usize {
        self.adapter
            .state()
            .outputs
            .values()
            .filter(|output| output.borrow().synchronized)
            .count()
    }
    #[wasm_bindgen(getter)]
    pub fn center(&mut self) -> f32 {
        self.sink.renderer_mut().center()
    }
    #[wasm_bindgen(getter)]
    pub fn span(&mut self) -> f32 {
        self.sink.renderer_mut().price_span()
    }
    #[wasm_bindgen(getter)]
    pub fn buffered_bytes(&self) -> usize {
        self.adapter.buffered_bytes()
    }
    #[wasm_bindgen(getter)]
    pub fn needs_input(&self) -> bool {
        self.adapter.state().needs_input
    }
    #[wasm_bindgen(getter)]
    pub fn complete(&self) -> bool {
        self.adapter.state().complete
    }
    #[wasm_bindgen(getter)]
    pub fn warming(&self) -> bool {
        self.adapter.state().warming
    }
    #[wasm_bindgen(getter)]
    pub fn clock_ms(&self) -> f64 {
        self.adapter.state().view().clock_ns as f64 / 1e6
    }
    /// Playback pacing follows the source even when a stopped simulation keeps
    /// the displayed clock frozen at its final execution.
    #[wasm_bindgen(getter)]
    pub fn source_clock_ms(&self) -> f64 {
        self.adapter.state().clock_ns as f64 / 1e6
    }
    #[wasm_bindgen(getter)]
    pub fn start_ms(&self) -> f64 {
        self.adapter.state().start_ns.unwrap_or(0) as f64 / 1e6
    }
    #[wasm_bindgen(getter)]
    pub fn bytes_consumed(&self) -> f64 {
        self.adapter.state().consumed as f64
    }
    #[wasm_bindgen(getter)]
    pub fn messages(&self) -> f64 {
        self.adapter.state().messages as f64
    }
    #[wasm_bindgen(getter)]
    pub fn checksum_checks(&self) -> f64 {
        self.adapter.state().checksum_checks as f64
    }
    #[wasm_bindgen(getter)]
    pub fn checksum_failures(&self) -> f64 {
        self.adapter.state().checksum_failures as f64
    }
    #[wasm_bindgen(getter)]
    pub fn quantity_decimals(&self) -> u8 {
        let state = self.adapter.state();
        state
            .instruments
            .get(&state.selected)
            .map_or(0, |i| i.quantity_decimals)
    }
    #[wasm_bindgen(getter)]
    pub fn price_decimals(&self) -> u8 {
        let state = self.adapter.state();
        state
            .instruments
            .get(&state.selected)
            .map_or(2, |i| i.price_decimals)
    }
    #[wasm_bindgen(getter)]
    pub fn updates(&self) -> f64 {
        let feed = self.adapter.state();
        feed.outputs
            .get(&feed.selected)
            .map_or(0.0, |o| o.borrow().count as f64)
    }
    #[wasm_bindgen(getter)]
    pub fn bid(&self) -> f64 {
        let state = self.adapter.state().view();
        state
            .selected_book()
            .and_then(|book| book.best_price(lobo_models::Side::Buy))
            .map_or(0.0, |price| {
                state.instruments[&state.selected].price(price.into())
            })
    }
    #[wasm_bindgen(getter)]
    pub fn ask(&self) -> f64 {
        let state = self.adapter.state().view();
        state
            .selected_book()
            .and_then(|book| book.best_price(lobo_models::Side::Sell))
            .map_or(0.0, |price| {
                state.instruments[&state.selected].price(price.into())
            })
    }
}

fn parse_side(side: &str) -> Result<lobo_models::Side, JsValue> {
    match side {
        "buy" => Ok(lobo_models::Side::Buy),
        "sell" => Ok(lobo_models::Side::Sell),
        _ => Err(error("Invalid side")),
    }
}
fn atoms(value: f64, decimals: u8) -> Result<u64, JsValue> {
    let raw = value * 10f64.powi(decimals as i32);
    if !raw.is_finite() || raw < 1.0 || raw >= u64::MAX as f64 {
        return Err(error(
            "Enter a positive price and quantity within the instrument's precision",
        ));
    }
    Ok(raw.round() as u64)
}
