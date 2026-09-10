//! Statically selected mutation callbacks. Disabled observers never evaluate
//! their closures, so unused top-price scans and trade accounting disappear.
use crate::{PriceChangeEvent, PriceLevelChangeEvent, TradedVolumeEvent};
use lobo_primitives::PriceType;

pub trait EventPublisher<T> {
    fn observe<R: Default>(&self, build: impl FnOnce() -> R) -> R;
    fn emit_with(&mut self, build: impl FnOnce() -> Option<T>);
}
pub trait MutationPublisher<P: PriceType>:
    EventPublisher<PriceLevelChangeEvent<P>>
    + EventPublisher<PriceChangeEvent<P>>
    + EventPublisher<TradedVolumeEvent<P>>
{
    #[inline(always)]
    fn level(&mut self, event: PriceLevelChangeEvent<P>) {
        <Self as EventPublisher<PriceLevelChangeEvent<P>>>::emit_with(self, || Some(event));
    }
    #[inline(always)]
    fn observe_price(&self, build: impl FnOnce() -> Option<P>) -> Option<P> {
        <Self as EventPublisher<PriceChangeEvent<P>>>::observe(self, build)
    }
    #[inline(always)]
    fn price_change(&mut self, build: impl FnOnce() -> Option<PriceChangeEvent<P>>) {
        <Self as EventPublisher<PriceChangeEvent<P>>>::emit_with(self, build);
    }
    #[inline(always)]
    fn observe_trade(&self, build: impl FnOnce() -> u64) -> u64 {
        <Self as EventPublisher<TradedVolumeEvent<P>>>::observe(self, build)
    }
    #[inline(always)]
    fn traded_volume(&mut self, build: impl FnOnce() -> Option<TradedVolumeEvent<P>>) {
        <Self as EventPublisher<TradedVolumeEvent<P>>>::emit_with(self, build);
    }
}
impl<P: PriceType, T> MutationPublisher<P> for T where
    T: EventPublisher<PriceLevelChangeEvent<P>>
        + EventPublisher<PriceChangeEvent<P>>
        + EventPublisher<TradedVolumeEvent<P>>
{
}

// Preserve the existing direct-storage callback API: a level-only callback
// has no observers for the new message types.
impl<P: PriceType, F: FnMut(PriceLevelChangeEvent<P>)> EventPublisher<PriceLevelChangeEvent<P>>
    for F
{
    #[inline(always)]
    fn observe<R: Default>(&self, build: impl FnOnce() -> R) -> R {
        build()
    }
    #[inline(always)]
    fn emit_with(&mut self, build: impl FnOnce() -> Option<PriceLevelChangeEvent<P>>) {
        if let Some(event) = build() {
            self(event);
        }
    }
}
macro_rules! ignore_callback {
    ($event:ident) => {
        impl<P: PriceType, F: FnMut(PriceLevelChangeEvent<P>)> EventPublisher<$event<P>> for F {
            #[inline(always)]
            fn observe<R: Default>(&self, _: impl FnOnce() -> R) -> R {
                R::default()
            }
            #[inline(always)]
            fn emit_with(&mut self, _: impl FnOnce() -> Option<$event<P>>) {}
        }
    };
}
ignore_callback!(PriceChangeEvent);
ignore_callback!(TradedVolumeEvent);
