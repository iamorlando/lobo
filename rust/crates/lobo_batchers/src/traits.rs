use lobo_events::BookEvent;

pub trait Batch<T> {
    type BatchResult;
    type BatchError;
    fn increment_message_count(&mut self);
    fn message_count(&self) -> usize;
    fn capacity(&self) -> usize;

    fn append_msg_book_data(&mut self, event: &BookEvent<T>);
    fn push_inner_message(&mut self, event: &T) -> Result<Option<()>, Self::BatchError>;

    fn push(
        &mut self,
        event: &BookEvent<T>,
    ) -> Result<Option<Self::BatchResult>, Self::BatchError> {
        self.append_msg_book_data(event);

        self.push_inner_message(event.event())?;

        self.increment_message_count();

        if self.message_count() >= self.capacity() {
            Ok(Some(self.finish_batch()?))
        } else {
            Ok(None)
        }
    }

    fn flush(&mut self) -> Result<Option<Self::BatchResult>, Self::BatchError> {
        if self.message_count() == 0 {
            return Ok(None);
        }

        Ok(Some(self.finish_batch()?))
    }

    fn finish_batch(&mut self) -> Result<Self::BatchResult, Self::BatchError>;
}

/// A destination for one completed batch at a time.
///
/// Call `write` whenever [`Batch::push`] returns a batch, and again for any
/// partial batch returned by [`Batch::flush`], before calling `finish`.
/// Implementations write or submit each batch immediately instead of retaining
/// a run's worth of batches. They may block on IO; the caller chooses where to
/// run the consumer. A successful write does not imply durable storage or GPU
/// completion.
pub trait Sink<T> {
    type SinkResult;
    type SinkError;

    /// Write or submit this batch, returning any destination-owned resource.
    fn write(&mut self, batch: &T) -> Result<Self::SinkResult, Self::SinkError>;

    /// Flush destination buffers without ending the sink.
    fn flush(&mut self) -> Result<(), Self::SinkError>;

    /// Complete destination bookkeeping, such as a file footer.
    /// Callers must not write more batches after finishing.
    fn finish(&mut self) -> Result<(), Self::SinkError>;
}

/// A sink whose batching result is another typed message. RequiredMessages
/// declares the inputs its publisher must enable when wiring the context.
pub trait MessageSink<T>: Sink<BookEvent<T>> {
    type Message;
    type RequiredMessages;
}

/// Forward message batches to an IO/GPU destination without retaining a run.
pub struct BatchedDestination<B, D> {
    pub batcher: B,
    pub destination: D,
}
impl<T, B: Batch<T>, D: Sink<B::BatchResult, SinkError = B::BatchError>> Sink<BookEvent<T>>
    for BatchedDestination<B, D>
{
    type SinkResult = ();
    type SinkError = B::BatchError;
    fn write(&mut self, event: &BookEvent<T>) -> Result<(), Self::SinkError> {
        if let Some(batch) = self.batcher.push(event)? {
            self.destination.write(&batch)?;
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<(), Self::SinkError> {
        if let Some(batch) = self.batcher.flush()? {
            self.destination.write(&batch)?;
        }
        self.destination.flush()
    }
    fn finish(&mut self) -> Result<(), Self::SinkError> {
        self.flush()?;
        self.destination.finish()
    }
}
