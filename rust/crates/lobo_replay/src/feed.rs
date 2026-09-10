//! Incremental feed routing and frame batching over the native replay context.
pub use crate::feed_books::FeedBook;
use crate::feed_books::FeedBooks as Books;
use lobo_batchers::{Aggregation, VolumeBars, traits::Sink};
use lobo_context::{
    Context, InlineMessageSink, InstrumentDirectory, volume::InlineVolumePublisher,
};
use lobo_events::{BookEvent, PriceLevelChangeEvent, PublisherFactory};
use lobo_events::{PriceChangeEvent, Publishers, TradedVolumeEvent, VolumeBar};
use lobo_primitives::{Price64, PriceType};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use std::{
    convert::Infallible,
    num::{NonZeroU64, NonZeroU128},
};

pub type LevelEvent = BookEvent<PriceLevelChangeEvent<Price64>>;
/// A synchronous staging sink. Publication always appends; the context releases
/// a validated message's events as a unit. This stores events, not book state.
#[derive(Clone, Default)]
pub struct FramePublisher(Rc<RefCell<Vec<LevelEvent>>>);
impl PublisherFactory<PriceLevelChangeEvent<Price64>> for FramePublisher {
    #[inline(always)]
    fn send(&self, event: LevelEvent) {
        self.0.borrow_mut().push(event);
    }
}
/// Completed output messages waiting for GPU upload, with no raw input retention.
#[derive(Clone, Default)]
pub struct VolumeFrameSink(pub Rc<RefCell<Vec<BookEvent<VolumeBar<Price64>>>>>);
impl Sink<BookEvent<VolumeBar<Price64>>> for VolumeFrameSink {
    type SinkResult = ();
    type SinkError = Infallible;
    fn write(&mut self, event: &BookEvent<VolumeBar<Price64>>) -> Result<(), Infallible> {
        self.0.borrow_mut().push(event.clone());
        Ok(())
    }
    fn flush(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
    fn finish(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
}
pub type FeedPublishers = Publishers<
    FramePublisher,
    InlineVolumePublisher<Price64, VolumeFrameSink, PriceChangeEvent<Price64>>,
    InlineVolumePublisher<Price64, VolumeFrameSink, TradedVolumeEvent<Price64>>,
>;

#[derive(Clone, Debug)]
pub struct Instrument {
    pub symbol: String,
    pub price_decimals: u8,
    pub quantity_decimals: u8,
}
impl Instrument {
    pub fn price(&self, raw: u64) -> f64 {
        raw as f64 / 10f64.powi(self.price_decimals as i32)
    }
    pub fn quantity(&self, raw: u64) -> f64 {
        raw as f64 / 10f64.powi(self.quantity_decimals as i32)
    }
}
/// Coalesced events waiting for a frame; never used for book queries or checksum.
#[derive(Default)]
pub struct FrameUpdates {
    pub pending: BTreeMap<(u64, u8), LevelEvent>,
    pub count: u64,
    pub generation: u64,
    pub synchronized: bool,
}
pub struct FeedState {
    pub scope: lobo_context::BookScope,
    pub simulation: Option<Box<crate::simulation::Simulation>>,
    pub market_preview: Option<crate::simulation::SimulationReport>,
    /// Distinguishes GPU state belonging to an isolated scenario.
    pub namespace: String,
    pub context: Books,
    pub instruments: BTreeMap<String, Instrument>,
    pub outputs: BTreeMap<String, RefCell<FrameUpdates>>,
    pub selected: String,
    pub clock_ns: u64,
    pub start_ns: Option<u64>,
    pub messages: u64,
    pub consumed: u64,
    pub warming: bool,
    pub complete: bool,
    pub needs_input: bool,
    pub outgoing: Vec<String>,
    pub checksum_checks: u64,
    pub checksum_failures: u64,
}
impl FeedState {
    pub fn new(symbol: &str) -> Result<Self, String> {
        let routes = VolumeBars::new(
            NonZeroU64::new(5000).expect("positive constant"),
            VolumeFrameSink::default(),
        )
        .connect_inline();
        Ok(Self {
            scope: Default::default(),
            simulation: None,
            market_preview: None,
            namespace: String::new(),
            context: Books::with_publisher(Publishers {
                levels: FramePublisher::default(),
                prices: routes.prices,
                trades: routes.trades,
            }),
            instruments: BTreeMap::new(),
            outputs: BTreeMap::new(),
            selected: normalize_symbol(symbol)?,
            clock_ns: 0,
            start_ns: None,
            messages: 0,
            consumed: 0,
            warming: true,
            complete: false,
            needs_input: true,
            outgoing: Vec::new(),
            checksum_checks: 0,
            checksum_failures: 0,
        })
    }
    pub fn register(&mut self, instrument: Instrument) -> Result<(), String> {
        if let Some(existing) = self.instruments.get(&instrument.symbol) {
            if existing.price_decimals != instrument.price_decimals
                || existing.quantity_decimals != instrument.quantity_decimals
            {
                return Err(format!(
                    "Precision changed for {}; reconnect to obtain a fresh directory",
                    instrument.symbol
                ));
            }
        } else {
            self.context.set_precision(&instrument.symbol, lobo_storage::policies::checksum::Precision {
                price: instrument.price_decimals, quantity: instrument.quantity_decimals,
            })?;
            let scale = 10u128
                .checked_pow(
                    u32::from(instrument.price_decimals) + u32::from(instrument.quantity_decimals),
                )
                .and_then(NonZeroU128::new)
                .ok_or("Instrument notional scale exceeds 128 bits")?;
            self.volume_bars()
                .borrow_mut()
                .set_notional_scale(&instrument.symbol, scale);
            self.volume_bars().borrow_mut().set_quantity_scale(
                &instrument.symbol,
                10u64
                    .checked_pow(u32::from(instrument.quantity_decimals))
                    .and_then(NonZeroU64::new)
                    .ok_or("Invalid quantity precision")?,
            );
            self.outputs
                .insert(instrument.symbol.clone(), RefCell::default());
            self.instruments
                .insert(instrument.symbol.clone(), instrument);
        }
        Ok(())
    }
    pub fn volume_bars(&self) -> &RefCell<VolumeBars<Price64, VolumeFrameSink>> {
        self.context.publisher_factory().trades.sink()
    }
    /// Configure before input, or start fresh bars from the current replay point.
    pub fn set_volume_bar_size(&mut self, size: NonZeroU64) {
        self.set_bar_aggregation(Aggregation::Volume(size));
    }
    pub fn set_bar_aggregation(&mut self, aggregation: Aggregation) {
        let mut bars = self.volume_bars().borrow_mut();
        bars.reset(aggregation);
        bars.destination.0.borrow_mut().clear();
    }
    pub fn book(&self, symbol: &str) -> Option<&FeedBook> {
        self.context.get(symbol)
    }
    pub fn selected_book(&self) -> Option<&FeedBook> {
        self.book(&self.selected)
    }
    pub fn view(&self) -> &Self {
        self.simulation
            .as_ref()
            .map_or(self, |simulation| &simulation.feed)
    }
    pub fn chart_key(&self, symbol: &str) -> String {
        if self.namespace.is_empty() {
            symbol.to_owned()
        } else {
            format!("{}:{symbol}", self.namespace)
        }
    }
    /// Seed a new view from native levels without replaying historical messages.
    pub fn seed_frame(&self) {
        use lobo_storage::price_level::{HasHiddenQuantity, PriceLevelContract};
        let symbol = &self.selected;
        if let Some(book) = self.selected_book() {
            let mut output = self.outputs[symbol].borrow_mut();
            crate::dispatch_feed_book!(
                book,
                native,
                for (price, level) in native
                    .order_storage
                    .bids
                    .visible_price_levels()
                    .chain(native.order_storage.asks.visible_price_levels())
                {
                    let event = PriceLevelChangeEvent::new(
                        level.visible_quantity(),
                        level.hidden_quantity(),
                        *price,
                        level.len(),
                        level.side(),
                    );
                    output.pending.insert(
                        (price.into_u128() as u64, level.side() as u8),
                        BookEvent::from_event(
                            event,
                            book.sequence(),
                            std::sync::Arc::from(symbol.as_str()),
                        )
                        .at_timestamp(self.clock_ns),
                    );
                    output.count += 1;
                }
            );
            output.synchronized = true;
        }
    }
    /// Allocate native order storage lazily; directory entries alone are cheap.
    pub fn book_mut(&mut self, symbol: &str) -> &mut FeedBook {
        self.context.book_mut(symbol)
    }
    pub fn synchronized(&self, symbol: &str) -> bool {
        self.outputs
            .get(symbol)
            .is_some_and(|output| output.borrow().synchronized)
    }
    /// Call only at a complete, validated message boundary. Last event wins for
    /// each price/side, while its identity and sequence remain the native book's.
    pub fn commit(&mut self) {
        for event in self
            .context
            .publisher_factory()
            .levels
            .0
            .borrow_mut()
            .drain(..)
        {
            let mut output = self
                .outputs
                .entry(event.book_id().to_owned())
                .or_default()
                .borrow_mut();
            output.count += 1;
            let change = event.event();
            output.pending.insert(
                (change.price().into_u128() as u64, change.side() as u8),
                event,
            );
        }
        if let Some(simulation) = &mut self.simulation {
            if !simulation.stopped() {
                simulation.feed.clock_ns = self.clock_ns;
            }
            simulation.feed.commit();
        }
    }
    /// Remove L3 liquidity through native mutations, retaining directory/routes.
    pub fn invalidate_orders(&mut self, symbol: &str) {
        if let Some(book) = self.context.get_mut(symbol) {
            crate::dispatch_feed_book!(book, book, {
                let ids: Vec<_> = book
                    .order_storage
                    .order_to_arena_map
                    .keys()
                    .copied()
                    .collect();
                let (storage, mut publish) = book.storage_and_publisher();
                for id in ids {
                    // IDs were collected from this native book's live index.
                    let _ = storage.remove_unchecked(id, &mut publish);
                }
            });
        }
        if let Some(output) = self.outputs.get(symbol) {
            let mut output = output.borrow_mut();
            output.generation += 1;
            output.synchronized = false;
        }
        if let Some(branch) = &mut self.simulation {
            if branch.feed.selected == symbol {
                branch.interrupt("Live feed lost synchronization; return to the main timeline");
            }
        }
        self.commit();
    }
    /// Invalidate aggregate-feed liquidity using the native mutation API. Any
    /// staged invalid values are superseded by zero events before commit.
    pub fn invalidate_aggregate(&mut self, symbol: &str) {
        if let Some(book) = self.context.get_mut(symbol) {
            crate::dispatch_feed_book!(book, book, {
                let (storage, mut publish) = book.storage_and_publisher();
                storage.bids.retain_level_quantities(0, &mut publish);
                storage.asks.retain_level_quantities(0, &mut publish);
            });
        }
        if let Some(output) = self.outputs.get(symbol) {
            let mut output = output.borrow_mut();
            output.generation += 1;
            output.synchronized = false;
        }
        self.commit();
    }
}
impl InstrumentDirectory for FeedState {
    fn tickers(&self) -> Vec<String> {
        self.instruments.keys().cloned().collect()
    }
}
impl lobo_context::ScopedInstruments for FeedState {
    fn book_scope(&self) -> &lobo_context::BookScope {
        &self.scope
    }
}
pub fn normalize_symbol(symbol: &str) -> Result<String, String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if symbol.is_empty() || symbol.len() > 64 || !symbol.bytes().all(|c| c.is_ascii_graphic()) {
        return Err("Enter a symbol of 1–64 printable ASCII characters".into());
    }
    Ok(symbol)
}
