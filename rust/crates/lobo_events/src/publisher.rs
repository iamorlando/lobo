use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::BookEvent;

/// Each subscriber owns its receiver; subscribers share immutable event data.
pub type Receiver<T> = UnboundedReceiver<Arc<BookEvent<T>>>;

/// Select a book's publishing callback once through static dispatch.
///
/// The book supplies the callback that stamps its ID and sequence number. The
/// null factory discards that callback, so even stamping disappears from the
/// optimized mutation path. No optional callback or function pointer is used.
pub trait PublisherFactory<T>: Clone + Default {
    /// Lazy observation, statically erased by unsupported message publishers.
    #[inline(always)]
    fn observe<R: Default>(&self, build: impl FnOnce() -> R) -> R {
        build()
    }

    fn publisher<F: FnMut(T)>(&self, publish: F) -> impl FnMut(T) {
        publish
    }

    fn send(&self, event: BookEvent<T>);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NullPublisher;

impl<T> PublisherFactory<T> for NullPublisher {
    #[inline(always)]
    fn observe<R: Default>(&self, _: impl FnOnce() -> R) -> R {
        R::default()
    }

    #[inline(always)]
    fn publisher<F: FnMut(T)>(&self, _publish: F) -> impl FnMut(T) {
        |_| {}
    }

    #[inline(always)]
    fn send(&self, _event: BookEvent<T>) {}
}

/// One unbounded Tokio MPSC channel per sink, established before replay.
///
/// A slow receiver retains its queued events instead of losing price-level
/// changes to broadcast lag. Each book preserves sequence order; different
/// books may interleave differently across receivers during parallel replay.
/// Applications must provision consumers for their
/// input rate. Closed receivers are skipped; consumer errors are reported by
/// the context's sink tasks when those tasks are joined.
pub struct MpscPublisher<T> {
    senders: Arc<[UnboundedSender<Arc<BookEvent<T>>>]>,
}

impl<T> MpscPublisher<T> {
    pub fn new(senders: Vec<UnboundedSender<Arc<BookEvent<T>>>>) -> Self {
        Self {
            senders: senders.into(),
        }
    }
}

impl<T> Clone for MpscPublisher<T> {
    fn clone(&self) -> Self {
        Self {
            senders: Arc::clone(&self.senders),
        }
    }
}

impl<T> Default for MpscPublisher<T> {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl<T> PublisherFactory<T> for MpscPublisher<T> {
    #[inline(always)]
    fn send(&self, event: BookEvent<T>) {
        let event = Arc::new(event);
        for sender in self.senders.iter() {
            let _ = sender.send(Arc::clone(&event));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_factory_discards_the_entire_book_callback() {
        let mut publish =
            NullPublisher.publisher(|_: u64| panic!("null publisher stamped an event"));
        publish(1);
        publish(2);
    }
}

use crate::{PriceChangeEvent, PriceLevelChangeEvent, TradedVolumeEvent};
use lobo_primitives::PriceType;

pub trait BookPublisherFactory<P: PriceType>:
    PublisherFactory<PriceLevelChangeEvent<P>>
    + PublisherFactory<PriceChangeEvent<P>>
    + PublisherFactory<TradedVolumeEvent<P>>
{
}
impl<P: PriceType, T> BookPublisherFactory<P> for T where
    T: PublisherFactory<PriceLevelChangeEvent<P>>
        + PublisherFactory<PriceChangeEvent<P>>
        + PublisherFactory<TradedVolumeEvent<P>>
{
}

/// Each typed route is chosen at context construction. Unconfigured routes are
/// NullPublisher; there is no type ID, enum dispatch or subscription test on send.
#[derive(Clone, Default)]
pub struct Publishers<L = NullPublisher, B = NullPublisher, T = NullPublisher> {
    pub levels: L,
    pub prices: B,
    pub trades: T,
}
macro_rules! route {
    ($event:ident, $field:ident, $route:ident) => {
        impl<P: PriceType, L: Clone + Default, B: Clone + Default, T: Clone + Default>
            PublisherFactory<$event<P>> for Publishers<L, B, T>
        where
            $route: PublisherFactory<$event<P>>,
        {
            #[inline(always)]
            fn observe<R: Default>(&self, build: impl FnOnce() -> R) -> R {
                self.$field.observe(build)
            }
            #[inline(always)]
            fn publisher<F: FnMut($event<P>)>(&self, publish: F) -> impl FnMut($event<P>) {
                self.$field.publisher(publish)
            }
            #[inline(always)]
            fn send(&self, event: BookEvent<$event<P>>) {
                self.$field.send(event);
            }
        }
    };
}
route!(PriceLevelChangeEvent, levels, L);
route!(PriceChangeEvent, prices, B);
route!(TradedVolumeEvent, trades, T);

// Existing single-message MPSC configurations stay single-message configurations.
macro_rules! ignore_mpsc {
    ($source:ident, $ignored:ident) => {
        impl<P: PriceType> PublisherFactory<$ignored<P>> for MpscPublisher<$source<P>> {
            #[inline(always)]
            fn observe<R: Default>(&self, _: impl FnOnce() -> R) -> R {
                R::default()
            }
            #[inline(always)]
            fn publisher<F: FnMut($ignored<P>)>(&self, _: F) -> impl FnMut($ignored<P>) {
                |_| {}
            }
            #[inline(always)]
            fn send(&self, _: BookEvent<$ignored<P>>) {}
        }
    };
}
ignore_mpsc!(PriceLevelChangeEvent, PriceChangeEvent);
ignore_mpsc!(PriceLevelChangeEvent, TradedVolumeEvent);
