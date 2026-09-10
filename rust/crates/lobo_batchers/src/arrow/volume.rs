use crate::traits::Batch;
use arrow_array::{
    RecordBatch,
    builder::{FixedSizeBinaryBuilder, StringBuilder, UInt64Builder},
};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use lobo_events::{BookEvent, VolumeBar};
use lobo_primitives::PriceType;
use std::sync::Arc;
pub struct VolumeBarArrowBatcher {
    capacity: usize,
    rows: usize,
    schema: SchemaRef,
    sequence: UInt64Builder,
    book: StringBuilder,
    index: UInt64Builder,
    prices: [FixedSizeBinaryBuilder; 4],
    volume: UInt64Builder,
    ticks: UInt64Builder,
    start: UInt64Builder,
    end: UInt64Builder,
}
impl VolumeBarArrowBatcher {
    pub fn new(capacity: usize) -> Self {
        let schema = Arc::new(Schema::new(vec![
            Field::new("sequence_number", DataType::UInt64, false),
            Field::new("book_id", DataType::Utf8, false),
            Field::new("bar_index", DataType::UInt64, false),
            Field::new("open", DataType::FixedSizeBinary(16), false),
            Field::new("high", DataType::FixedSizeBinary(16), false),
            Field::new("low", DataType::FixedSizeBinary(16), false),
            Field::new("close", DataType::FixedSizeBinary(16), false),
            Field::new("volume", DataType::UInt64, false),
            Field::new("ticks", DataType::UInt64, false),
            Field::new("start_ns", DataType::UInt64, false),
            Field::new("end_ns", DataType::UInt64, false),
        ]));
        Self {
            capacity: capacity.max(1),
            rows: 0,
            schema,
            sequence: UInt64Builder::new(),
            book: StringBuilder::new(),
            index: UInt64Builder::new(),
            prices: std::array::from_fn(|_| FixedSizeBinaryBuilder::new(16)),
            volume: UInt64Builder::new(),
            ticks: UInt64Builder::new(),
            start: UInt64Builder::new(),
            end: UInt64Builder::new(),
        }
    }
    pub fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }
}
impl<P: PriceType> Batch<VolumeBar<P>> for VolumeBarArrowBatcher {
    type BatchResult = RecordBatch;
    type BatchError = ArrowError;
    fn increment_message_count(&mut self) {
        self.rows += 1;
    }
    fn message_count(&self) -> usize {
        self.rows
    }
    fn capacity(&self) -> usize {
        self.capacity
    }
    fn append_msg_book_data(&mut self, event: &BookEvent<VolumeBar<P>>) {
        self.sequence.append_value(event.sequence_number());
        self.book.append_value(event.book_id());
    }
    fn push_inner_message(&mut self, bar: &VolumeBar<P>) -> Result<Option<()>, ArrowError> {
        self.index.append_value(bar.index);
        for (builder, value) in self
            .prices
            .iter_mut()
            .zip([bar.open, bar.high, bar.low, bar.close])
        {
            builder.append_value(value.into_u128().to_be_bytes())?;
        }
        self.volume.append_value(bar.volume);
        self.ticks.append_value(bar.ticks);
        self.start.append_value(bar.start_ns);
        self.end.append_value(bar.end_ns);
        Ok(Some(()))
    }
    fn finish_batch(&mut self) -> Result<RecordBatch, ArrowError> {
        let mut columns: Vec<arrow_array::ArrayRef> = vec![
            Arc::new(self.sequence.finish()),
            Arc::new(self.book.finish()),
            Arc::new(self.index.finish()),
        ];
        columns.extend(
            self.prices
                .iter_mut()
                .map(|b| Arc::new(b.finish()) as arrow_array::ArrayRef),
        );
        columns.push(Arc::new(self.volume.finish()));
        columns.push(Arc::new(self.ticks.finish()));
        columns.push(Arc::new(self.start.finish()));
        columns.push(Arc::new(self.end.finish()));
        self.rows = 0;
        RecordBatch::try_new(self.schema.clone(), columns)
    }
}

/// Shared Arrow schema for all OHLC aggregation targets.
pub type OhlcArrowBatcher = VolumeBarArrowBatcher;
