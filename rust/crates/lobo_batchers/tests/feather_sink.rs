#![cfg(feature = "feather")]

mod common;

use std::{
    cell::Cell,
    io::{self, BufWriter, Cursor, Write},
};

use arrow_array::{FixedSizeBinaryArray, UInt64Array};
use arrow_ipc::reader::FileReader;
use arrow_schema::ArrowError;
use lobo_batchers::{
    arrow::{batch::PriceLevelArrowBatcher, sink::FeatherSink},
    traits::{Batch, Sink},
};
use lobo_events::PriceLevelChangeEvent;
use lobo_primitives::Price128;

#[derive(Debug, Default)]
struct ObservedWriter {
    bytes: Vec<u8>,
    flushes: usize,
    fail_write: Cell<bool>,
    fail_flush: Cell<bool>,
}

impl Write for ObservedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail_write.get() {
            return Err(io::Error::other("write failed"));
        }
        self.bytes.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.fail_flush.get() {
            return Err(io::Error::other("flush failed"));
        }
        self.flushes += 1;
        Ok(())
    }
}

#[test]
fn writes_full_batches_before_finish_and_round_trips_the_tail() {
    let mut batcher = PriceLevelArrowBatcher::new(2);
    let writer = BufWriter::with_capacity(64 * 1024, ObservedWriter::default());
    let mut sink = FeatherSink::try_new(writer, &batcher.schema()).unwrap();
    assert!(batcher.push(&common::event(1)).unwrap().is_none());
    let first = batcher.push(&common::event(2)).unwrap().unwrap();
    sink.write(&first).unwrap();

    // Even though the caller supplied a large buffer and the producer is still
    // running, the completed batch has already reached the actual writer.
    let bytes_after_first = sink.get_ref().get_ref().bytes.len();
    assert!(bytes_after_first > 0);
    assert_eq!(sink.get_ref().get_ref().flushes, 1);
    assert!(sink.get_ref().buffer().is_empty());
    assert!(batcher.push(&common::event(3)).unwrap().is_none());
    let tail =
        <PriceLevelArrowBatcher as Batch<PriceLevelChangeEvent<Price128>>>::flush(&mut batcher)
            .unwrap()
            .unwrap();
    sink.write(&tail).unwrap();
    assert!(sink.get_ref().get_ref().bytes.len() > bytes_after_first);
    assert_eq!(sink.get_ref().get_ref().flushes, 2);
    sink.finish().unwrap();

    let writer = sink.into_inner().unwrap().into_inner().unwrap();
    let batches = FileReader::try_new(Cursor::new(writer.bytes), None)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(batches, vec![first, tail]);
    let sequences = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .unwrap();
    assert_eq!(sequences.values().as_ref(), &[1, 2]);
    let prices = batches[0]
        .column(2)
        .as_any()
        .downcast_ref::<FixedSizeBinaryArray>()
        .unwrap();
    assert_eq!(prices.value(0), (u128::MAX - 1).to_be_bytes());
}

#[test]
fn rejects_schema_mismatch_without_writing_and_remains_usable() {
    let batch = common::batch();
    let mut sink = FeatherSink::try_new(Vec::new(), &batch.schema()).unwrap();
    let initial_len = sink.get_ref().len();
    let wrong_schema = batch.project(&[0]).unwrap();
    assert!(matches!(
        sink.write(&wrong_schema),
        Err(ArrowError::SchemaError(_))
    ));
    assert_eq!(sink.get_ref().len(), initial_len);
    sink.write(&batch).unwrap();
    let bytes = sink.into_inner().unwrap();
    let mut reader = FileReader::try_new(Cursor::new(bytes), None).unwrap();
    assert_eq!(reader.next().unwrap().unwrap(), batch);
    assert!(reader.next().is_none());
}

#[test]
fn finalizes_an_empty_file_and_rejects_writes_after_finish() {
    let batch = common::batch();
    let mut sink = FeatherSink::try_new(Vec::new(), &batch.schema()).unwrap();
    sink.finish().unwrap();
    assert!(sink.write(&batch).is_err());
    let reader = FileReader::try_new(Cursor::new(sink.into_inner().unwrap()), None).unwrap();
    assert_eq!(reader.schema(), batch.schema());
    assert_eq!(reader.count(), 0);
}

#[test]
fn propagates_destination_write_and_flush_errors() {
    let batch = common::batch();
    let writer = ObservedWriter::default();
    writer.fail_write.set(true);
    assert!(FeatherSink::try_new(writer, &batch.schema()).is_err());

    let mut sink = FeatherSink::try_new(ObservedWriter::default(), &batch.schema()).unwrap();
    sink.get_ref().fail_write.set(true);
    assert!(sink.write(&batch).is_err());

    let mut sink = FeatherSink::try_new(ObservedWriter::default(), &batch.schema()).unwrap();
    sink.get_ref().fail_flush.set(true);
    assert!(sink.write(&batch).is_err());
    assert!(sink.flush().is_err());
    assert!(sink.finish().is_err());
}
