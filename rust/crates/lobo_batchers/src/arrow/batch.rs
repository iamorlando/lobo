use super::super::traits::Batch;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, RecordBatch,
    builder::{FixedSizeBinaryBuilder, StringBuilder, UInt8Builder, UInt64Builder},
};

use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use lobo_events::{BookEvent, PriceLevelChangeEvent};
use lobo_primitives::PriceType;

pub struct PriceLevelArrowBatcher {
    metrics: crate::PriceLevelMetrics,
    schema: SchemaRef,
    capacity: usize,
    rows: usize,

    sequence_number: UInt64Builder,
    book_id: StringBuilder,
    price: FixedSizeBinaryBuilder,
    visible_quantity: UInt64Builder,
    hidden_quantity: UInt64Builder,
    number_of_orders: UInt64Builder,
    side: UInt8Builder,
}

impl PriceLevelArrowBatcher {
    pub fn new(capacity: usize) -> Self {
        Self::with_metrics(capacity, crate::PriceLevelMetrics::all())
    }

    pub fn with_metrics(capacity: usize, metrics: crate::PriceLevelMetrics) -> Self {
        let schema = Arc::new(Schema::new(vec![
            Field::new("sequence_number", DataType::UInt64, false),
            Field::new("book_id", DataType::Utf8, false),
            Field::new("price", DataType::FixedSizeBinary(16), false),
            Field::new("visible_quantity", DataType::UInt64, false),
            Field::new("hidden_quantity", DataType::UInt64, false),
            Field::new("number_of_orders", DataType::UInt64, false),
            Field::new("side", DataType::UInt8, false),
        ]));

        Self {
            metrics,
            schema,
            capacity,
            rows: 0,

            sequence_number: UInt64Builder::with_capacity(capacity),
            book_id: StringBuilder::new(),
            price: FixedSizeBinaryBuilder::with_capacity(capacity, 16),
            visible_quantity: UInt64Builder::with_capacity(capacity),
            hidden_quantity: UInt64Builder::with_capacity(capacity),
            number_of_orders: UInt64Builder::with_capacity(capacity),
            side: UInt8Builder::with_capacity(capacity),
        }
    }

    pub fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }
}
impl<P: PriceType> Batch<PriceLevelChangeEvent<P>> for PriceLevelArrowBatcher {
    type BatchError = ArrowError;
    type BatchResult = RecordBatch;
    fn append_msg_book_data(&mut self, event: &BookEvent<PriceLevelChangeEvent<P>>) {
        self.sequence_number.append_value(event.sequence_number());

        self.book_id.append_value(event.book_id());
    }
    fn capacity(&self) -> usize {
        self.capacity
    }
    fn increment_message_count(&mut self) {
        self.rows += 1
    }
    fn message_count(&self) -> usize {
        self.rows
    }
    fn push_inner_message(
        &mut self,
        event: &PriceLevelChangeEvent<P>,
    ) -> Result<Option<()>, Self::BatchError> {
        self.price
            .append_value(event.price().into_u128().to_be_bytes())?;

        self.visible_quantity.append_value(event.visible_quantity());

        self.hidden_quantity
            .append_value(self.metrics.hidden_quantity(event.hidden_quantity()));

        self.number_of_orders
            .append_value(event.number_of_orders() as u64);
        self.side.append_value(event.side() as u8);
        Ok(Some(()))
    }

    fn finish_batch(&mut self) -> Result<RecordBatch, ArrowError> {
        let columns: Vec<ArrayRef> = vec![
            Arc::new(self.sequence_number.finish()) as ArrayRef,
            Arc::new(self.book_id.finish()) as ArrayRef,
            Arc::new(self.price.finish()) as ArrayRef,
            Arc::new(self.visible_quantity.finish()) as ArrayRef,
            Arc::new(self.hidden_quantity.finish()) as ArrayRef,
            Arc::new(self.number_of_orders.finish()) as ArrayRef,
            Arc::new(self.side.finish()) as ArrayRef,
        ];

        self.rows = 0;

        RecordBatch::try_new(Arc::clone(&self.schema), columns)
    }
}
