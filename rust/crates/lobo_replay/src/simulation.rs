//! Nonmutating market previews and isolated native limit-order branches.
//! Replay can redirect worse-price execution flow; live branches retain FIFO.
//! External order references remain validated against the main timeline.
use crate::feed::{FeedBook, FeedState};
use lobo_context::{BookLevel, Context, FeedMode};
use lobo_events::{BookEvent, SimulatedExecutionEvent, TradedVolumeEvent};
use lobo_models::{
    Side,
    events::{Fill, OrderDetails, Reports},
    orders::{
        core::RestingOrder,
        order_types::{LimitOrder, MarketOrder},
        traits::Trades,
    },
};
use lobo_primitives::{Price64, time::DateTime, uuid::Uuid};
use lobo_storage::{MutatingFills, SimulatedAggregateFills, SimulatedFills, WithoutRest};
use std::sync::Arc;

const REPORTS: Reports = Reports {
    include_fills: true,
    include_summary: false,
    include_market_impact: false,
};

pub struct SimulationReport {
    pub symbol: Arc<str>,
    pub order_id: Uuid,
    pub started_ns: u64,
    pub requested: u64,
    pub filled: u64,
    pub side: Side,
    pub price: Option<Price64>,
    notional: u128,
    pub executions: Vec<BookEvent<SimulatedExecutionEvent<Price64>>>,
}

pub struct Simulation {
    pub feed: FeedState,
    pub report: SimulationReport,
    pub ignored: u64,
    pub stopped_reason: Option<String>,
    limit: Price64,
    execute_flow: fn(&mut Self, u64, Side, Price64, u64),
}

/// Preview native liquidity without copying it or publishing real executions.
pub fn market_preview(
    main: &mut FeedState,
    order: MarketOrder<Price64>,
    level: BookLevel,
) -> Result<SimulationReport, String> {
    let mut report = SimulationReport::new(&main.selected, main.clock_ns, &order);
    if report.requested == 0 {
        return Err("Simulation quantity must be positive".into());
    }
    let symbol = main.selected.clone();
    if main.selected_book().is_none() {
        return Err("No book to simulate".into());
    }
    let result = crate::dispatch_feed_book!(main.book_mut(&symbol), native, {
        let storage = &mut native.order_storage;
        let mut publish = |_: lobo_events::PriceLevelChangeEvent<Price64>| {};
        match level {
            BookLevel::L1 | BookLevel::L2 => storage.submit_order_with_reports(
                order,
                SimulatedAggregateFills,
                REPORTS,
                &mut publish,
            ),
            BookLevel::L3 => {
                storage.submit_order_with_reports(order, SimulatedFills, REPORTS, &mut publish)
            }
        }
    });
    report.record(
        main.clock_ns,
        result.report.and_then(|r| r.fills).unwrap_or_default(),
    );
    Ok(report)
}

fn transition(order: &mut RestingOrder<Price64>, details: OrderDetails<Price64>) {
    order.update_in_place(details);
}

impl Simulation {
    pub fn start(
        main: &FeedState,
        mut order: LimitOrder<Price64>,
        mode: FeedMode,
    ) -> Result<Self, String> {
        let symbol = &main.selected;
        let source = main.selected_book().ok_or("No book to simulate")?;
        let limit = order.price().ok_or("Simulation requires a limit price")?;
        if order.quantity() == 0 {
            return Err("Simulation quantity must be positive".into());
        }
        let timestamp = main.clock_ns;
        order.common_data.creation_time = DateTime::from_timestamp_nanos(timestamp as i64);
        let mut feed = FeedState::new(symbol)?;
        feed.register(
            main.instruments
                .get(symbol)
                .ok_or("Missing instrument")?
                .clone(),
        )?;
        feed.set_bar_aggregation(main.volume_bars().borrow().aggregation());
        let book = source.fork_with_publisher(feed.context.publisher_factory().clone());
        feed.context.books.insert(symbol.to_ascii_lowercase(), book);
        feed.start_ns = main.start_ns;
        feed.clock_ns = timestamp;
        feed.warming = false;
        feed.namespace = order.uuid().to_string();
        let mut simulation = Self {
            feed,
            report: SimulationReport::new(symbol, timestamp, &order),
            ignored: 0,
            stopped_reason: None,
            limit,
            execute_flow: match mode {
                FeedMode::Replay => Self::execute_replay,
                FeedMode::Live => Self::execute_fifo,
            },
        };
        let fills = crate::dispatch_feed_book!(simulation.book_mut(), native, {
            let (storage, mut publish) = native.storage_and_publisher_at(timestamp);
            // Preview without changing the source. Commit only the filled quantity
            // through native matching in the fork, including iceberg replenishment;
            // the previewed remainder is inserted separately without matching again.
            let mut execution = order.clone();
            let result =
                storage.submit_order_with_reports(order, SimulatedFills, REPORTS, &mut publish);
            let fills = result
                .report
                .and_then(|report| report.fills)
                .unwrap_or_default();
            execution.common_data.quantity = fills.iter().map(|fill| fill.fill_quantity).sum();
            if execution.quantity() != 0 {
                storage.submit_order_with_reports(
                    execution,
                    WithoutRest(MutatingFills),
                    Reports::default(),
                    &mut publish,
                );
            }
            if let Some(remainder) = result.remaining_order {
                storage
                    .add_order(remainder, &mut publish)
                    .map_err(|e| format!("Simulation remainder: {e:?}"))?;
            }
            fills
        });
        simulation.report.record(timestamp, fills);
        simulation.feed.seed_frame();
        simulation.feed.commit();
        Ok(simulation)
    }
    pub fn complete(&self) -> bool {
        self.report.complete()
    }
    pub fn stopped(&self) -> bool {
        self.complete() || self.stopped_reason.is_some()
    }
    pub fn interrupt(&mut self, reason: &str) {
        if !self.stopped() {
            self.stopped_reason = Some(reason.to_owned());
        }
    }
    pub fn remaining(&self) -> u64 {
        self.report.remaining()
    }
    pub fn book_mut(&mut self) -> &mut FeedBook {
        let symbol = self.feed.selected.clone();
        self.feed.book_mut(&symbol)
    }
    /// Replay executions describe incoming taker flow. Re-match that flow in the
    /// fork through the native fill API, allowing the simulated order to be a maker.
    pub fn execute(&mut self, timestamp: u64, maker_side: Side, price: Price64, quantity: u64) {
        if !self.stopped() {
            (self.execute_flow)(self, timestamp, maker_side, price, quantity);
        }
    }
    /// A newly observed resting order can cross the counterfactual limit while
    /// remaining passive in the real book. Match and rest its remainder using
    /// the same native API; only fills involving the user's order enter reports.
    pub fn submit(&mut self, timestamp: u64, order: LimitOrder<Price64>) {
        if self.stopped() {
            return;
        }
        let result = crate::dispatch_feed_book!(self.book_mut(), native, {
            let (storage, mut publish) = native.storage_and_publisher_at(timestamp);
            let result =
                storage.submit_order_with_reports(order, MutatingFills, REPORTS, &mut publish);
            result
        });
        self.report.record(
            timestamp,
            result.report.and_then(|r| r.fills).unwrap_or_default(),
        );
        self.feed.clock_ns = self.feed.clock_ns.max(timestamp);
    }
    fn execute_replay(&mut self, timestamp: u64, maker_side: Side, price: Price64, quantity: u64) {
        let limit = self.limit;
        let worse = match self.report.side {
            Side::Buy => price < limit,
            Side::Sell => price > limit,
        };
        if maker_side != self.report.side || !worse {
            self.execute_fifo(timestamp, maker_side, price, quantity);
            return;
        }
        let id = self.report.order_id;
        let quantity = quantity.min(self.remaining());
        let remaining = self.remaining() - quantity;
        crate::dispatch_feed_book!(self.book_mut(), native, {
            let (storage, mut publish) = native.storage_and_publisher_at(timestamp);
            storage
                .modify_with_fill_event(
                    id,
                    OrderDetails {
                        quantity: Some(remaining),
                        uuid: None,
                        price: None,
                        creation_time: None,
                        trader: None,
                        side: None,
                    },
                    transition,
                    |_| limit,
                    &mut publish,
                )
                .expect("active simulated resting order");
        });
        self.report.record(
            timestamp,
            vec![Fill {
                maker_order_id: id,
                taker_order_id: Uuid::nil(),
                fill_quantity: quantity,
                maker_depleted: remaining == 0,
                maker_trader_uid: Uuid::nil(),
                price: limit,
                fill_time: DateTime::from_timestamp_nanos(timestamp as i64),
            }],
        );
        self.feed.clock_ns = timestamp;
    }
    fn execute_fifo(&mut self, timestamp: u64, maker_side: Side, price: Price64, quantity: u64) {
        let side = match maker_side {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        };
        let order = LimitOrder::new(Some(price), quantity, Uuid::nil(), side)
            .with_creation_time(DateTime::from_timestamp_nanos(timestamp as i64));
        let result = crate::dispatch_feed_book!(self.book_mut(), native, {
            let (storage, mut publish) = native.storage_and_publisher_at(timestamp);
            let result = storage.submit_order_with_reports(
                order,
                WithoutRest(MutatingFills),
                REPORTS,
                &mut publish,
            );
            result
        });
        self.report.record(
            timestamp,
            result
                .report
                .and_then(|report| report.fills)
                .unwrap_or_default(),
        );
        self.feed.clock_ns = timestamp;
    }
}

impl SimulationReport {
    fn new(symbol: &str, timestamp: u64, order: &impl Trades<Price64>) -> Self {
        Self {
            symbol: Arc::from(symbol),
            order_id: order.uuid(),
            started_ns: timestamp,
            requested: order.quantity(),
            filled: 0,
            side: order.side(),
            price: order.price(),
            notional: 0,
            executions: Vec::new(),
        }
    }
    pub fn complete(&self) -> bool {
        self.filled == self.requested
    }
    pub fn remaining(&self) -> u64 {
        self.requested - self.filled
    }
    pub fn average_price(&self) -> Option<f64> {
        (self.filled != 0).then(|| self.notional as f64 / self.filled as f64)
    }
    /// Record native fills involving the simulated order, keeping real feed
    /// executions out of the counterfactual report.
    pub fn record(&mut self, timestamp: u64, fills: Vec<Fill<Price64>>) {
        for fill in fills {
            if fill.maker_order_id != self.order_id && fill.taker_order_id != self.order_id {
                continue;
            }
            self.filled += fill.fill_quantity;
            self.notional += u128::from(u64::from(fill.price)) * u128::from(fill.fill_quantity);
            let side = if fill.maker_order_id == self.order_id {
                self.side
            } else {
                match self.side {
                    Side::Buy => Side::Sell,
                    Side::Sell => Side::Buy,
                }
            };
            self.executions.push(
                BookEvent::from_event(
                    SimulatedExecutionEvent {
                        execution: TradedVolumeEvent {
                            price: fill.price,
                            quantity: fill.fill_quantity,
                            side,
                        },
                        maker_order_id: fill.maker_order_id,
                        taker_order_id: fill.taker_order_id,
                    },
                    self.executions.len() as u64 + 1,
                    self.symbol.clone(),
                )
                .at_timestamp(timestamp),
            );
        }
    }
}
