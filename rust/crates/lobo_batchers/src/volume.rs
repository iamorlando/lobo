//! OHLC messages accumulated independently for each book. Aggregation dispatch
//! belongs to this consumer; native book publication remains statically typed.
use crate::traits::{MessageSink, Sink};
use lobo_events::{BookEvent, OhlcBar, PriceChangeEvent, TradedVolumeEvent};
use lobo_primitives::PriceType;
use std::{
    collections::HashMap,
    num::{NonZeroU64, NonZeroU128},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aggregation {
    Volume(NonZeroU64),
    /// Number of execution messages, independent of their quantities.
    Ticks(NonZeroU64),
    /// Source-clock nanoseconds; buckets are aligned to clock zero.
    Time(NonZeroU64),
    /// Quote currency units, multiplied by each book's configured fixed-point scale.
    /// The whole execution crossing the threshold belongs to the closing bar.
    Notional(NonZeroU64),
}
impl Aggregation {
    pub fn size(self) -> u64 {
        match self {
            Self::Volume(n) | Self::Ticks(n) | Self::Time(n) | Self::Notional(n) => n.get(),
        }
    }
}
impl From<NonZeroU64> for Aggregation {
    fn from(value: NonZeroU64) -> Self {
        Self::Volume(value)
    }
}
struct Progress<P> {
    forming: Option<OhlcBar<P>>,
    completed: u64,
    revision: u64,
    quotes: [Option<PriceChangeEvent<P>>; 2],
    notional: u128,
    scale: NonZeroU128,
    quantity_scale: NonZeroU64,
    source: BookEvent<()>,
}
impl<P> Progress<P> {
    fn new(symbol: &str) -> Self {
        Self {
            forming: None,
            completed: 0,
            revision: 0,
            quotes: [None, None],
            notional: 0,
            scale: NonZeroU128::MIN,
            quantity_scale: NonZeroU64::MIN,
            source: BookEvent::from_event((), 0, Arc::from(symbol)),
        }
    }
    fn reset(&mut self) {
        self.forming = None;
        self.completed = 0;
        self.revision = 0;
        self.notional = 0;
    }
}
pub struct OhlcBars<P, D> {
    target: Aggregation,
    books: HashMap<Arc<str>, Progress<P>>,
    pub destination: D,
}
/// Backwards-compatible volume constructor: `VolumeBars::new(size, destination)`.
pub type VolumeBars<P, D> = OhlcBars<P, D>;
impl<P: PriceType, D: Sink<BookEvent<OhlcBar<P>>>> OhlcBars<P, D> {
    pub fn new(target: impl Into<Aggregation>, destination: D) -> Self {
        Self {
            target: target.into(),
            books: HashMap::new(),
            destination,
        }
    }
    pub fn aggregation(&self) -> Aggregation {
        self.target
    }
    pub fn bar_size(&self) -> u64 {
        self.target.size()
    }
    pub fn forming(&self, book: &str) -> Option<OhlcBar<P>> {
        self.books.get(book)?.forming
    }
    pub fn completed(&self, book: &str) -> u64 {
        self.books.get(book).map_or(0, |b| b.completed)
    }
    pub fn quotes(&self, book: &str) -> [Option<PriceChangeEvent<P>>; 2] {
        self.books.get(book).map_or([None, None], |b| b.quotes)
    }
    pub fn states(&self) -> impl Iterator<Item = (&str, Option<OhlcBar<P>>, u64)> {
        self.books
            .iter()
            .map(|(symbol, p)| (symbol.as_ref(), p.forming, p.revision))
    }
    /// Configure once from instrument metadata. One quote-currency unit equals
    /// this many raw price × raw quantity units. Defaults to 1 for integer feeds.
    pub fn set_notional_scale(&mut self, book: &str, scale: NonZeroU128) {
        self.books
            .entry(Arc::from(book))
            .or_insert_with(|| Progress::new(book))
            .scale = scale;
    }
    /// Number of raw quantity atoms in one displayed unit (default: 1).
    pub fn set_quantity_scale(&mut self, book: &str, scale: NonZeroU64) {
        self.books
            .entry(Arc::from(book))
            .or_insert_with(|| Progress::new(book))
            .quantity_scale = scale;
    }
    /// Progress in the aggregation's units, for display only; arithmetic uses integers.
    pub fn progress(&self, book: &str, clock_ns: u64) -> f64 {
        let Some(p) = self.books.get(book) else {
            return 0.0;
        };
        let Some(bar) = p.forming else { return 0.0 };
        match self.target {
            Aggregation::Volume(_) => bar.volume as f64 / p.quantity_scale.get() as f64,
            Aggregation::Ticks(_) => bar.ticks as f64,
            Aggregation::Time(n) => clock_ns.saturating_sub(bar.start_ns).min(n.get()) as f64,
            Aggregation::Notional(_) => p.notional as f64 / p.scale.get() as f64,
        }
    }
    pub fn reset(&mut self, target: impl Into<Aggregation>) {
        self.target = target.into();
        for p in self.books.values_mut() {
            p.reset();
        }
    }
    fn complete(p: &mut Progress<P>, destination: &mut D) -> Result<(), D::SinkError> {
        if let Some(bar) = p.forming {
            destination.write(&p.source.with_event(bar))?;
            p.completed += 1;
            p.forming = None;
            p.notional = 0;
            p.revision += 1;
        }
        Ok(())
    }
    /// Advance an ordered source watermark to close time buckets even when an
    /// individual book has no new trades. Empty buckets never invent candles.
    pub fn advance_time(&mut self, timestamp_ns: u64) -> Result<(), D::SinkError> {
        if let Aggregation::Time(_) = self.target {
            for p in self.books.values_mut() {
                if p.forming.is_some_and(|bar| timestamp_ns >= bar.end_ns) {
                    Self::complete(p, &mut self.destination)?;
                }
            }
        }
        Ok(())
    }
    fn trade(&mut self, event: &BookEvent<TradedVolumeEvent<P>>) -> Result<(), D::SinkError> {
        let trade = event.event();
        if trade.quantity == 0 {
            return Ok(());
        }
        let p = self
            .books
            .entry(event.shared_book_id())
            .or_insert_with(|| Progress::new(event.book_id()));
        let timestamp = event.timestamp_ns();
        if let Aggregation::Time(_) = self.target {
            if p.forming.is_some_and(|bar| timestamp >= bar.end_ns) {
                Self::complete(p, &mut self.destination)?;
            }
        }
        p.source = event.with_event(());
        p.revision += 1;
        let mut remaining = trade.quantity;
        while remaining != 0 {
            let (start_ns, end_ns) = match self.target {
                Aggregation::Time(n) => {
                    let start = timestamp / n.get() * n.get();
                    (start, start.saturating_add(n.get()))
                }
                _ => (timestamp, timestamp),
            };
            let bar = p.forming.get_or_insert(OhlcBar {
                open: trade.price,
                high: trade.price,
                low: trade.price,
                close: trade.price,
                volume: 0,
                index: p.completed,
                ticks: 0,
                start_ns,
                end_ns,
            });
            let quantity = match self.target {
                Aggregation::Volume(n) => {
                    remaining.min(n.get().saturating_mul(p.quantity_scale.get()) - bar.volume)
                }
                _ => remaining,
            };
            bar.high = bar.high.max(trade.price);
            bar.low = bar.low.min(trade.price);
            bar.close = trade.price;
            bar.volume += quantity;
            bar.ticks += 1;
            bar.end_ns = end_ns;
            remaining -= quantity;
            let complete = match self.target {
                Aggregation::Volume(n) => {
                    bar.volume == n.get().saturating_mul(p.quantity_scale.get())
                }
                Aggregation::Ticks(n) => bar.ticks == n.get(),
                Aggregation::Time(_) => false,
                Aggregation::Notional(n) => {
                    // Saturation only occurs above any representable target;
                    // it therefore preserves the threshold comparison exactly.
                    p.notional = p
                        .notional
                        .saturating_add(trade.price.into_u128().saturating_mul(quantity as u128));
                    p.notional >= (n.get() as u128).saturating_mul(p.scale.get())
                }
            };
            if complete {
                Self::complete(p, &mut self.destination)?;
            }
        }
        Ok(())
    }
}
impl<P: PriceType, D: Sink<BookEvent<OhlcBar<P>>>> Sink<BookEvent<PriceChangeEvent<P>>>
    for OhlcBars<P, D>
{
    type SinkResult = ();
    type SinkError = D::SinkError;
    fn write(&mut self, event: &BookEvent<PriceChangeEvent<P>>) -> Result<(), D::SinkError> {
        self.books
            .entry(event.shared_book_id())
            .or_insert_with(|| Progress::new(event.book_id()))
            .quotes[event.event().side as usize] = Some(*event.event());
        Ok(())
    }
    fn flush(&mut self) -> Result<(), D::SinkError> {
        self.destination.flush()
    }
    fn finish(&mut self) -> Result<(), D::SinkError> {
        self.destination.finish()
    }
}
impl<P: PriceType, D: Sink<BookEvent<OhlcBar<P>>>> Sink<BookEvent<TradedVolumeEvent<P>>>
    for OhlcBars<P, D>
{
    type SinkResult = ();
    type SinkError = D::SinkError;
    fn write(&mut self, event: &BookEvent<TradedVolumeEvent<P>>) -> Result<(), D::SinkError> {
        self.trade(event)
    }
    fn flush(&mut self) -> Result<(), D::SinkError> {
        self.destination.flush()
    }
    /// Incomplete bars remain available as previews, never labelled complete.
    fn finish(&mut self) -> Result<(), D::SinkError> {
        self.destination.finish()
    }
}
impl<P: PriceType, D: Sink<BookEvent<OhlcBar<P>>>> MessageSink<TradedVolumeEvent<P>>
    for OhlcBars<P, D>
{
    type Message = OhlcBar<P>;
    type RequiredMessages = (PriceChangeEvent<P>, TradedVolumeEvent<P>);
}
impl<P: PriceType, D: Sink<BookEvent<OhlcBar<P>>>> MessageSink<PriceChangeEvent<P>>
    for OhlcBars<P, D>
{
    type Message = OhlcBar<P>;
    type RequiredMessages = (PriceChangeEvent<P>, TradedVolumeEvent<P>);
}
