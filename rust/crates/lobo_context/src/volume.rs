//! VolumeBars declares its inputs and connects exactly those typed publishers.
use crate::{Sink, SinkError, SinkTasks};
use lobo_batchers::{VolumeBars, traits::Sink as Destination};
use lobo_events::{
    BookEvent, NullPublisher, PriceChangeEvent, PriceLevelChangeEvent, PublisherFactory,
    Publishers, TradedVolumeEvent, VolumeBar,
};
use lobo_primitives::PriceType;
use std::{cell::RefCell, convert::Infallible, marker::PhantomData, num::NonZeroU64, rc::Rc};
use tokio::sync::mpsc;

/// One ordered queue for a transform's heterogeneous inputs. Dispatch occurs
/// in the consumer; publishing chooses the variant statically from its type.
pub enum VolumeInput<P> {
    Price(BookEvent<PriceChangeEvent<P>>),
    Trade(BookEvent<TradedVolumeEvent<P>>),
}
pub trait VolumeInputMessage<P>: Sized {
    fn input(event: BookEvent<Self>) -> VolumeInput<P>;
}
impl<P> VolumeInputMessage<P> for PriceChangeEvent<P> {
    fn input(event: BookEvent<Self>) -> VolumeInput<P> {
        VolumeInput::Price(event)
    }
}
impl<P> VolumeInputMessage<P> for TradedVolumeEvent<P> {
    fn input(event: BookEvent<Self>) -> VolumeInput<P> {
        VolumeInput::Trade(event)
    }
}
pub struct VolumePublisher<P, T> {
    sender: mpsc::UnboundedSender<VolumeInput<P>>,
    marker: PhantomData<fn(T)>,
}
impl<P, T> Clone for VolumePublisher<P, T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            marker: PhantomData,
        }
    }
}
impl<P, T> Default for VolumePublisher<P, T> {
    fn default() -> Self {
        let (sender, _) = mpsc::unbounded_channel();
        Self {
            sender,
            marker: PhantomData,
        }
    }
}
impl<P, T: VolumeInputMessage<P>> PublisherFactory<T> for VolumePublisher<P, T> {
    #[inline(always)]
    fn send(&self, event: BookEvent<T>) {
        let _ = self.sender.send(T::input(event));
    }
}
impl<P, D> Sink<PriceLevelChangeEvent<P>> for VolumeBars<P, D>
where
    P: PriceType + Send + Sync + 'static,
    D: Destination<BookEvent<VolumeBar<P>>> + Send + 'static,
    D::SinkError: std::error::Error + Send + Sync + 'static,
{
    type Publisher = Publishers<
        NullPublisher,
        VolumePublisher<P, PriceChangeEvent<P>>,
        VolumePublisher<P, TradedVolumeEvent<P>>,
    >;
    fn connect(mut self) -> Result<(Self::Publisher, SinkTasks), SinkError> {
        let runtime = tokio::runtime::Handle::try_current()?;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let publisher = Publishers {
            levels: NullPublisher,
            prices: VolumePublisher {
                sender: sender.clone(),
                marker: PhantomData,
            },
            trades: VolumePublisher {
                sender,
                marker: PhantomData,
            },
        };
        let task = runtime.spawn_blocking(move || {
            while let Some(input) = receiver.blocking_recv() {
                match input {
                    VolumeInput::Price(event) => self.write(&event)?,
                    VolumeInput::Trade(event) => self.write(&event)?,
                }
            }
            <Self as Destination<BookEvent<TradedVolumeEvent<P>>>>::finish(&mut self)?;
            Ok(())
        });
        Ok((publisher, SinkTasks::new([task])))
    }
}

/// Browser transforms run on the caller's thread and feed an infallible staging
/// destination. IO transforms above use an ordered Tokio consumer instead.
pub trait InlineMessageSink<P: PriceType> {
    type Destination: Destination<BookEvent<VolumeBar<P>>, SinkError = Infallible>;
    fn connect_inline(
        self,
    ) -> Publishers<
        NullPublisher,
        InlineVolumePublisher<P, Self::Destination, PriceChangeEvent<P>>,
        InlineVolumePublisher<P, Self::Destination, TradedVolumeEvent<P>>,
    >;
}
pub struct InlineVolumePublisher<P, D, T> {
    sink: Rc<RefCell<VolumeBars<P, D>>>,
    marker: PhantomData<fn(T)>,
}
impl<P, D, T> Clone for InlineVolumePublisher<P, D, T> {
    fn clone(&self) -> Self {
        Self {
            sink: self.sink.clone(),
            marker: PhantomData,
        }
    }
}
impl<P: PriceType, D: Default + Destination<BookEvent<VolumeBar<P>>, SinkError = Infallible>, T>
    Default for InlineVolumePublisher<P, D, T>
{
    fn default() -> Self {
        Self {
            sink: Rc::new(RefCell::new(VolumeBars::new(
                NonZeroU64::new(5000).expect("positive constant"),
                D::default(),
            ))),
            marker: PhantomData,
        }
    }
}
impl<P, D, T> InlineVolumePublisher<P, D, T> {
    pub fn sink(&self) -> &RefCell<VolumeBars<P, D>> {
        &self.sink
    }
}
impl<P: PriceType, D, T> PublisherFactory<T> for InlineVolumePublisher<P, D, T>
where
    D: Default + Destination<BookEvent<VolumeBar<P>>, SinkError = Infallible>,
    VolumeBars<P, D>: Destination<BookEvent<T>, SinkResult = (), SinkError = Infallible>,
{
    #[inline(always)]
    fn send(&self, event: BookEvent<T>) {
        let Ok(()) = self.sink.borrow_mut().write(&event);
    }
}
impl<P: PriceType, D: Destination<BookEvent<VolumeBar<P>>, SinkError = Infallible>>
    InlineMessageSink<P> for VolumeBars<P, D>
{
    type Destination = D;
    fn connect_inline(
        self,
    ) -> Publishers<
        NullPublisher,
        InlineVolumePublisher<P, D, PriceChangeEvent<P>>,
        InlineVolumePublisher<P, D, TradedVolumeEvent<P>>,
    > {
        let sink = Rc::new(RefCell::new(self));
        Publishers {
            levels: NullPublisher,
            prices: InlineVolumePublisher {
                sink: sink.clone(),
                marker: PhantomData,
            },
            trades: InlineVolumePublisher {
                sink,
                marker: PhantomData,
            },
        }
    }
}
