use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use arrow_array::RecordBatch;
use arrow_ipc::writer::FileWriter;
use arrow_schema::{ArrowError, Schema};

use crate::traits::Sink;

/// An incremental Feather V2 (Arrow IPC file) destination.
///
/// Each `write` serializes a batch and flushes the underlying IO, including a
/// caller-supplied `BufWriter`. Arrow maintains the file index and dictionary
/// state between writes. Call [`Sink::finish`] or [`Self::into_inner`] to write the footer;
/// ordinary Feather readers need that footer before opening the file.
/// Dropping the sink does not finalize it. IO errors should be treated as fatal
/// for this file, since a failed write may have emitted part of a batch.
pub struct FeatherSink<W: Write> {
    writer: FileWriter<W>,
}

impl<W: Write> FeatherSink<W> {
    /// Write uncompressed Feather V2 to any synchronous IO destination.
    pub fn try_new(writer: W, schema: &Schema) -> Result<Self, ArrowError> {
        Ok(Self {
            writer: FileWriter::try_new(writer, schema)?,
        })
    }

    pub fn get_ref(&self) -> &W {
        self.writer.get_ref()
    }

    /// Finalize the file and return its IO destination.
    pub fn into_inner(self) -> Result<W, ArrowError> {
        self.writer.into_inner()
    }
}

impl FeatherSink<BufWriter<File>> {
    /// Create (or truncate) a file, buffering the writes within each batch.
    pub fn create(path: impl AsRef<Path>, schema: &Schema) -> Result<Self, ArrowError> {
        Self::try_new(BufWriter::new(File::create(path)?), schema)
    }
}

impl<W: Write> Sink<RecordBatch> for FeatherSink<W> {
    type SinkResult = ();
    type SinkError = ArrowError;

    fn write(&mut self, batch: &RecordBatch) -> Result<(), ArrowError> {
        if batch.schema().as_ref() != self.writer.schema().as_ref() {
            return Err(ArrowError::SchemaError(
                "batch schema does not match the Feather sink schema".into(),
            ));
        }
        self.writer.write(batch)?;
        self.writer.flush()
    }

    fn flush(&mut self) -> Result<(), ArrowError> {
        self.writer.flush()
    }

    fn finish(&mut self) -> Result<(), ArrowError> {
        self.writer.finish()
    }
}
