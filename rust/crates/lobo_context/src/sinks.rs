use std::{error::Error, future::Future};

use lobo_batchers::traits::{Batch, Sink as BatchDestination};
use lobo_events::{MpscPublisher, NullPublisher, PublisherFactory, Receiver};
use tokio::{runtime::Handle, sync::mpsc, task::JoinHandle};

pub type SinkError = Box<dyn Error + Send + Sync>;
pub type SinkTask = JoinHandle<Result<(), SinkError>>;

/// Connect destinations when the context is instantiated, before replay starts.
pub trait Sink<T> {
    type Publisher: PublisherFactory<T>;

    fn connect(self) -> Result<(Self::Publisher, SinkTasks), SinkError>;
}

#[derive(Default)]
pub struct SinkTasks(Vec<SinkTask>);

impl SinkTasks {
    pub fn new(tasks: impl IntoIterator<Item = SinkTask>) -> Self {
        Self(tasks.into_iter().collect())
    }

    /// Join every consumer, including when an earlier consumer fails. All
    /// publishers must be dropped first so receivers can drain and finalize.
    pub async fn finish(self) -> Result<(), SinkError> {
        let mut first_error = None;
        for task in self.0 {
            let result = match task.await {
                Ok(result) => result,
                Err(error) => Err(Box::new(error) as SinkError),
            };
            if first_error.is_none() {
                first_error = result.err();
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// The default destination: no channel, consumer task, stamping, or send call.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullSink;

impl<T> Sink<T> for NullSink {
    type Publisher = NullPublisher;

    fn connect(self) -> Result<(NullPublisher, SinkTasks), SinkError> {
        Ok((NullPublisher, SinkTasks::default()))
    }
}

/// A destination owns the receiver created for it at context construction.
pub trait ReceiverSink<T>: Send + 'static {
    fn start(self: Box<Self>, receiver: Receiver<T>, runtime: &Handle) -> SinkTask;
}

/// Heterogeneous sinks, with an independent Tokio MPSC receiver for each one.
pub struct MpscSinks<T> {
    sinks: Vec<Box<dyn ReceiverSink<T>>>,
}

impl<T> Default for MpscSinks<T> {
    fn default() -> Self {
        Self { sinks: Vec::new() }
    }
}

impl<T> MpscSinks<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, sink: impl ReceiverSink<T>) -> Self {
        self.sinks.push(Box::new(sink));
        self
    }
}

impl<T: 'static> Sink<T> for MpscSinks<T> {
    type Publisher = MpscPublisher<T>;

    fn connect(self) -> Result<(Self::Publisher, SinkTasks), SinkError> {
        let runtime = Handle::try_current()?;
        let mut senders = Vec::with_capacity(self.sinks.len());
        let mut tasks = Vec::with_capacity(self.sinks.len());
        for sink in self.sinks {
            let (sender, receiver) = mpsc::unbounded_channel();
            senders.push(sender);
            tasks.push(sink.start(receiver, &runtime));
        }
        Ok((MpscPublisher::new(senders), SinkTasks(tasks)))
    }
}

/// Adapt an async receiver consumer to a context sink.
pub struct AsyncSink<F>(F);

impl<F> AsyncSink<F> {
    pub fn new(consume: F) -> Self {
        Self(consume)
    }
}

impl<T, F, Fut> ReceiverSink<T> for AsyncSink<F>
where
    F: FnOnce(Receiver<T>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), SinkError>> + Send + 'static,
{
    fn start(self: Box<Self>, receiver: Receiver<T>, runtime: &Handle) -> SinkTask {
        runtime.spawn((self.0)(receiver))
    }
}

/// Run a batcher and synchronous destination on a Tokio blocking worker while
/// replay produces messages. Each completed batch is written immediately; the
/// final partial batch and destination footer are flushed when senders close.
pub struct BatchSink<B, S, F = fn(())> {
    batcher: B,
    sink: S,
    output: F,
}

impl<B, S> BatchSink<B, S> {
    /// For IO destinations whose write result is `()`.
    pub fn new(batcher: B, sink: S) -> Self {
        Self {
            batcher,
            sink,
            output: |()| {},
        }
    }
}

impl<B, S, F> BatchSink<B, S, F> {
    /// Handle destination-owned output, such as an uploaded GPU batch.
    pub fn with_output(batcher: B, sink: S, output: F) -> Self {
        Self {
            batcher,
            sink,
            output,
        }
    }
}

impl<T, B, S, F> ReceiverSink<T> for BatchSink<B, S, F>
where
    T: Send + Sync + 'static,
    B: Batch<T> + Send + 'static,
    B::BatchError: Error + Send + Sync + 'static,
    S: BatchDestination<B::BatchResult> + Send + 'static,
    S::SinkError: Error + Send + Sync + 'static,
    F: FnMut(S::SinkResult) + Send + 'static,
{
    fn start(self: Box<Self>, mut receiver: Receiver<T>, runtime: &Handle) -> SinkTask {
        runtime.spawn_blocking(move || {
            let Self {
                mut batcher,
                mut sink,
                mut output,
            } = *self;
            while let Some(event) = receiver.blocking_recv() {
                if let Some(batch) = batcher.push(event.as_ref())? {
                    output(sink.write(&batch)?);
                }
            }
            if let Some(batch) = batcher.flush()? {
                output(sink.write(&batch)?);
            }
            sink.finish()?;
            Ok(())
        })
    }
}
