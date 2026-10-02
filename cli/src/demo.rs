//! Offline feed uses native L3 mutations and native execution publication.
use crate::args::Args;
use lobo_adapters::adapter::market::{
    AdapterInfo, BookLevel, FeedMode, FeedState, Instrument, MarketDataAdapter,
};
use lobo_models::{
    BookPolicy, Side,
    server::{Command, LimitOrder, Order, OrderFields},
};
use lobo_primitives::uuid::Uuid;
use lobo_replay::custom::runtime::{ControlReceiver, Driver};
use std::time::{Duration, Instant};

pub struct Demo {
    state: FeedState,
    random: u64,
    next_ns: u64,
    symbols: Vec<String>,
}
impl Demo {
    pub fn new(args: &Args, symbols: Vec<String>) -> Result<Self, String> {
        let mut state = FeedState::new(&args.ticker())?;
        state.set_bar_aggregation(args.bars().map_err(|e| e.to_string())?);
        state.start_ns = Some(args.start_at);
        state.clock_ns = args.start_at;
        state.warming = false;
        let mut demo = Self {
            state,
            random: args.seed.max(1),
            next_ns: args.start_at,
            symbols,
        };
        for symbol in demo.symbols.clone() {
            demo.register_instrument(
                Instrument {
                    symbol: symbol.clone(),
                    price_decimals: 2,
                    quantity_decimals: 0,
                },
                BookPolicy::default(),
            )?;
            demo.state.book_mut(&symbol);
            for i in 1..=40 {
                demo.add(&symbol, Side::Buy, 10000 - i, u64::from(40 - i) * 70 + 50)?;
                demo.add(&symbol, Side::Sell, 10000 + i, u64::from(40 - i) * 60 + 50)?;
            }
            demo.state.outputs[&symbol].borrow_mut().synchronized = true;
        }
        demo.state.commit();
        Ok(demo)
    }
    fn random(&mut self) -> u64 {
        self.random = self
            .random
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.random >> 32
    }
    fn add(&mut self, symbol: &str, side: Side, price: u32, quantity: u64) -> Result<(), String> {
        let id = Uuid::from_u128(u128::from(self.random()) << 64 | u128::from(self.random()));
        self.submit_command(
            symbol,
            Command::Add {
                order: Order::Limit(LimitOrder {
                    fields: OrderFields {
                        id,
                        trader: Uuid::nil(),
                        side,
                        quantity,
                    },
                    price,
                }),
            },
        )?;
        Ok(())
    }
    fn step(&mut self) -> Result<(), String> {
        self.state.clock_ns = self.next_ns;
        for symbol in self.symbols.clone() {
            let side = if self.random().is_multiple_of(2) {
                Side::Buy
            } else {
                Side::Sell
            };
            let quote = self
                .state
                .book(&symbol)
                .and_then(|b| b.best_price(side))
                .map(u64::from)
                .unwrap_or(10000);
            let distance = self.random() % 15;
            let price = if side == Side::Buy {
                quote.saturating_sub(distance)
            } else {
                quote + distance
            } as u32;
            let quantity = 20 + self.random() % 200;
            self.add(&symbol, side, price, quantity)?;
            let queue = self
                .state
                .book(&symbol)
                .unwrap()
                .queue_view(side, quote.into()..=quote.into());
            if let Some(order) = queue.first() {
                let quantity = order.quantity.min(30 + self.random() % 160);
                let command = if self.random().is_multiple_of(6) {
                    Command::Cancel {
                        id: order.id,
                        quantity,
                    }
                } else {
                    Command::Execute {
                        id: order.id,
                        quantity,
                        price: None,
                    }
                };
                self.submit_command(&symbol, command)?;
            }
            // Replenish exhausted quotes and bound the demo's distant levels.
            let book = self.state.book(&symbol).unwrap();
            let bid = book.best_price(Side::Buy).map(u64::from).unwrap_or(9999);
            let ask = book.best_price(Side::Sell).map(u64::from).unwrap_or(10001);
            if ask.saturating_sub(bid) > 6 {
                let mid = (bid + ask) / 2;
                self.add(&symbol, Side::Buy, (mid - 1) as u32, 400)?;
                self.add(&symbol, Side::Sell, (mid + 1) as u32, 400)?;
            }
            let book = self.state.book(&symbol).unwrap();
            if book.order_count() > 1600 {
                let orders = book.queue_view(Side::Buy, 1u64.into()..=u64::MAX.into());
                for order in orders.into_iter().take(500) {
                    self.submit_command(&symbol, Command::Remove { id: order.id })?;
                }
                let orders = self
                    .state
                    .book(&symbol)
                    .unwrap()
                    .queue_view(Side::Sell, 1u64.into()..=u64::MAX.into());
                for order in orders.into_iter().rev().take(500) {
                    self.submit_command(&symbol, Command::Remove { id: order.id })?;
                }
            }
        }
        self.state.commit();
        Ok(())
    }
}
impl MarketDataAdapter for Demo {
    fn info(&self) -> AdapterInfo<'_> {
        AdapterInfo {
            id: "demo",
            name: "Offline native L3 demo",
            mode: FeedMode::Replay,
            endpoint: None,
            default_symbol: "AAPL",
            timezone: "ET",
            supports_trades: true,
            level: BookLevel::L3,
        }
    }
    fn state(&self) -> &FeedState {
        &self.state
    }
    fn state_mut(&mut self) -> &mut FeedState {
        &mut self.state
    }
    fn receive(&mut self, _: &[u8], _: bool) -> Result<(), String> {
        Ok(())
    }
    fn advance(&mut self, elapsed_ns: u64, budget: usize) -> Result<(), String> {
        let target = self.state.start_ns.unwrap().saturating_add(elapsed_ns);
        for _ in 0..budget.min(200) {
            if self.next_ns > target {
                break;
            }
            self.step()?;
            self.state.messages += 1;
            self.next_ns = self.next_ns.saturating_add(25_000_000);
        }
        let state = &self.state;
        state
            .volume_bars()
            .borrow_mut()
            .advance_time(state.clock_ns)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
#[derive(Default)]
pub struct DemoControl {
    pub speed: f64,
    pub paused: bool,
    pub elapsed_ns: u64,
}
pub struct DemoDriver {
    pub control: std::sync::Arc<std::sync::Mutex<DemoControl>>,
}
impl Driver for DemoDriver {
    fn run(
        self: Box<Self>,
        adapter: &mut dyn MarketDataAdapter,
        controls: &ControlReceiver,
    ) -> Result<(), String> {
        let mut anchor = Instant::now();
        while controls.poll(adapter) {
            let mut control = self.control.lock().unwrap();
            let now = Instant::now();
            if !control.paused {
                control.elapsed_ns = control.elapsed_ns.saturating_add(
                    (now.duration_since(anchor).as_nanos() as f64 * control.speed) as u64,
                );
            }
            anchor = now;
            let elapsed = control.elapsed_ns;
            let paused = control.paused;
            drop(control);
            if !paused {
                adapter.advance(elapsed, 200)?;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}
