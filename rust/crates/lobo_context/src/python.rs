//! Python destination configuration and the shared native sink runtime.
use crate::{BatchSink, MpscSinks, Sink, SinkError, SinkTasks};
use arrow_array::RecordBatch;
use arrow_schema::ArrowError;
use lobo_batchers::{
    PriceLevelMetrics,
    arrow::{batch::PriceLevelArrowBatcher, sink::FeatherSink},
    traits::Sink as Destination,
};
use lobo_events::{MpscPublisher, PriceLevelChangeEvent, PublisherFactory};
use lobo_primitives::CompressedPrice;
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::PyBytes,
};
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{self, BufWriter, Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};
use tokio::runtime::Runtime;

pub type Message = PriceLevelChangeEvent<CompressedPrice>;
pub type Publisher = MpscPublisher<Message>;

fn parse_metrics(bits: u32) -> PyResult<PriceLevelMetrics> {
    PriceLevelMetrics::from_bits(bits)
        .ok_or_else(|| PyValueError::new_err("unknown price-level metric bits"))
}

/// Configure the GPU observer hosted by server_context. The browser owns the
/// canvas/device; this sink selects metrics before that observer connects.
#[pyclass(name = "GpuSink", module = "lobo.sinks", frozen)]
pub struct PyGpuSink {
    pub metrics: PriceLevelMetrics,
}
#[pymethods]
impl PyGpuSink {
    #[new]
    #[pyo3(signature = (*, metrics=0))]
    fn new(metrics: u32) -> PyResult<Self> {
        Ok(Self {
            metrics: parse_metrics(metrics)?,
        })
    }
    #[getter]
    fn metrics(&self) -> u32 {
        self.metrics.bits()
    }
}

fn runtime() -> PyResult<&'static Runtime> {
    static RUNTIME: OnceLock<io::Result<Runtime>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("lobo-sinks")
                .build()
        })
        .as_ref()
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))
}

#[derive(Default)]
struct Progress {
    started: bool,
    finished: bool,
    rows: u64,
    batches: u64,
    bytes: u64,
    stream_bytes: u64,
    error: Option<String>,
}

/// Write price-level updates to Feather V2 on a native background worker.
/// Create the sink first, then pass it in Context(sinks=[sink]) or
/// ReplayContext(..., sinks=[sink]). Context.close()/exit drains and finalizes it.
/// batch_size controls rows per write; the final partial batch is written on close.
/// buffer_size controls the IO buffer in bytes. Existing files require overwrite=True.
#[pyclass(name = "FeatherSink", module = "lobo.sinks", frozen)]
pub struct PyFeatherSink {
    metrics: PriceLevelMetrics,
    path: PathBuf,
    batch_size: usize,
    buffer_size: usize,
    overwrite: bool,
    progress: Arc<Mutex<Progress>>,
}

impl PyFeatherSink {
    fn progress(&self) -> MutexGuard<'_, Progress> {
        self.progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn connect(&self) -> PyResult<BatchSink<PriceLevelArrowBatcher, ObservedFeather>> {
        let mut progress = self.progress();
        if progress.started {
            return Err(PyValueError::new_err(
                "a FeatherSink can only be connected to one context",
            ));
        }
        let file = OpenOptions::new()
            .write(true)
            .create(self.overwrite)
            .truncate(self.overwrite)
            .create_new(!self.overwrite)
            .open(&self.path)?;
        let writer = CountingWriter {
            writer: BufWriter::with_capacity(self.buffer_size, file),
            bytes: 0,
        };
        let batcher = PriceLevelArrowBatcher::with_metrics(self.batch_size, self.metrics);
        let mut inner = FeatherSink::try_new(writer, &batcher.schema()).map_err(sink_error)?;
        inner.flush().map_err(sink_error)?;
        progress.started = true;
        progress.bytes = inner.get_ref().bytes;
        progress.stream_bytes = progress.bytes;
        drop(progress);
        Ok(BatchSink::new(
            batcher,
            ObservedFeather {
                inner,
                progress: Arc::clone(&self.progress),
                finalized: false,
            },
        ))
    }
}

#[pymethods]
impl PyFeatherSink {
    #[new]
    #[pyo3(signature = (path, *, batch_size=8192, buffer_size=262144, overwrite=false, metrics=PriceLevelMetrics::all().bits()))]
    fn new(
        path: PathBuf,
        batch_size: usize,
        buffer_size: usize,
        overwrite: bool,
        metrics: u32,
    ) -> PyResult<Self> {
        if batch_size == 0 || buffer_size == 0 {
            return Err(PyValueError::new_err(
                "batch_size and buffer_size must be greater than zero",
            ));
        }
        // Resolve relative paths now so later working-directory changes are harmless.
        let path = std::path::absolute(path)?;
        Ok(Self {
            metrics: parse_metrics(metrics)?,
            path,
            batch_size,
            buffer_size,
            overwrite,
            progress: Arc::default(),
        })
    }
    #[getter]
    fn metrics(&self) -> u32 {
        self.metrics.bits()
    }
    #[getter]
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
    #[getter]
    fn batch_size(&self) -> usize {
        self.batch_size
    }
    #[getter]
    fn buffer_size(&self) -> usize {
        self.buffer_size
    }
    #[getter]
    fn rows_written(&self) -> u64 {
        self.progress().rows
    }
    #[getter]
    fn batches_written(&self) -> u64 {
        self.progress().batches
    }
    #[getter]
    fn bytes_written(&self) -> u64 {
        self.progress().bytes
    }
    #[getter]
    fn started(&self) -> bool {
        self.progress().started
    }
    #[getter]
    fn finished(&self) -> bool {
        self.progress().finished
    }
    #[getter]
    fn error(&self) -> Option<String> {
        self.progress().error.clone()
    }
    fn __fspath__(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
    fn __repr__(&self) -> String {
        format!(
            "FeatherSink(path={:?}, batch_size={}, buffer_size={}, rows_written={}, finished={})",
            self.path,
            self.batch_size,
            self.buffer_size,
            self.rows_written(),
            self.finished()
        )
    }

    /// Read only committed batches from disk, encoded as a standalone IPC stream.
    /// Used by lobo.sinks.read_feather; no events are retained in a Python queue.
    fn _snapshot_ipc(&self, py: Python<'_>) -> PyResult<Py<PyBytes>> {
        let committed = {
            let progress = self.progress();
            if !progress.started {
                return Err(PyRuntimeError::new_err(
                    "sink has not been connected to a context",
                ));
            }
            if let Some(error) = &progress.error {
                return Err(PyRuntimeError::new_err(error.clone()));
            }
            progress.stream_bytes
        };
        let path = &self.path;
        let bytes = py.detach(|| -> io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            File::open(path)?.take(committed).read_to_end(&mut bytes)?;
            if bytes.len() as u64 != committed || !bytes.starts_with(b"ARROW1") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Feather file changed outside its sink",
                ));
            }
            // Arrow pads its file magic to the configured alignment. The schema
            // begins with the first nonzero byte (the IPC continuation marker).
            let start = bytes[6..]
                .iter()
                .position(|&byte| byte != 0)
                .map(|offset| offset + 6)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "missing Feather schema")
                })?;
            bytes.drain(..start);
            bytes.extend_from_slice(&[255, 255, 255, 255, 0, 0, 0, 0]);
            Ok(bytes)
        })?;
        Ok(PyBytes::new(py, &bytes).unbind())
    }
}

struct CountingWriter {
    writer: BufWriter<File>,
    bytes: u64,
}
impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.writer.write(bytes)?;
        self.bytes += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

struct ObservedFeather {
    inner: FeatherSink<CountingWriter>,
    progress: Arc<Mutex<Progress>>,
    finalized: bool,
}
impl Destination<RecordBatch> for ObservedFeather {
    type SinkResult = ();
    type SinkError = ArrowError;
    fn write(&mut self, batch: &RecordBatch) -> Result<(), ArrowError> {
        let result = self.inner.write(batch);
        let mut progress = self
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &result {
            Ok(()) => {
                progress.rows += batch.num_rows() as u64;
                progress.batches += 1;
                progress.bytes = self.inner.get_ref().bytes;
                progress.stream_bytes = progress.bytes;
            }
            Err(error) => progress.error = Some(error.to_string()),
        }
        result
    }
    fn flush(&mut self) -> Result<(), ArrowError> {
        self.inner.flush()
    }
    fn finish(&mut self) -> Result<(), ArrowError> {
        if self.finalized {
            return Ok(());
        }
        self.finalized = true;
        let result = self.inner.finish();
        let mut progress = self
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        progress.finished = true;
        progress.bytes = self.inner.get_ref().bytes;
        if let Err(error) = &result {
            progress.error = Some(error.to_string());
        }
        result
    }
}
impl Drop for ObservedFeather {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn sink_error(error: impl std::fmt::Display) -> PyErr {
    PyRuntimeError::new_err(error.to_string())
}

/// Own the worker tasks independently from books handed out to Python.
pub struct SinkSession {
    publisher: Option<Publisher>,
    tasks: SinkTasks,
}
impl SinkSession {
    pub fn connect(py: Python<'_>, sinks: Vec<Py<PyFeatherSink>>) -> PyResult<Self> {
        if sinks.is_empty() {
            return Ok(Self {
                publisher: None,
                tasks: SinkTasks::default(),
            });
        }
        let runtime = runtime()?;
        let mut paths = HashSet::new();
        for sink in &sinks {
            let sink = sink.borrow(py);
            if sink.started() || !paths.insert(sink.path.clone()) {
                return Err(PyValueError::new_err(
                    "sinks must be unused and have distinct output paths",
                ));
            }
        }
        let mut destinations = MpscSinks::<Message>::new();
        for sink in sinks {
            destinations = destinations.with(sink.borrow(py).connect()?);
        }
        let _guard = runtime.enter();
        let (publisher, tasks) = destinations.connect().map_err(sink_error)?;
        Ok(Self {
            publisher: Some(publisher),
            tasks,
        })
    }
    pub fn publisher(&self) -> Option<Publisher> {
        self.publisher.clone()
    }
    /// Disconnect every Python book before closing its session.
    pub fn close(self, py: Python<'_>) -> PyResult<()> {
        let Self { publisher, tasks } = self;
        if publisher.is_none() {
            return Ok(());
        }
        drop(publisher);
        let runtime = runtime()?;
        py.detach(|| runtime.block_on(tasks.finish()))
            .map_err(sink_error)
    }
}

/// Attach an already-connected publisher to a native replay context. The Python
/// session owns task finalization, including subsequent interactive book fills.
pub struct ConnectedPublisher<P>(pub P);
impl<T, P: PublisherFactory<T>> Sink<T> for ConnectedPublisher<P> {
    type Publisher = P;
    fn connect(self) -> Result<(P, SinkTasks), SinkError> {
        Ok((self.0, SinkTasks::default()))
    }
}
