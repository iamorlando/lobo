use lobo_adapters::itch::{
    ItchReplayStats,
    python::{PyBook, PyItchSource, ReplayLevel},
};
use lobo_books::price_time_priority::Book;
use lobo_context::python::{ConnectedPublisher, PyFeatherSink, SinkSession};
use lobo_events::{BookPublisherFactory, NullPublisher};
use lobo_primitives::time::{DateTime, Utc};
use lobo_replay::ReplayContext;
use lobo_storage::{
    DoNotUpdateUserMap, policies::DoNotUpdateHiddenQuantity,
    price_sorting::SortedVectorPriceSorting,
};
use pyo3::{
    exceptions::{PyKeyError, PyRuntimeError, PyTypeError, PyValueError},
    prelude::*,
    types::{PyDateTime, PyTzInfo},
};
use std::collections::HashMap;

type NativeReplayContext = ReplayContext<
    ReplayLevel,
    SortedVectorPriceSorting,
    DoNotUpdateUserMap,
    DoNotUpdateHiddenQuantity,
>;
type NativeBook<P> =
    Book<ReplayLevel, SortedVectorPriceSorting, DoNotUpdateUserMap, DoNotUpdateHiddenQuantity, P>;

/// Reconstruct ITCH books through an optional inclusive cutoff. Sources can be
/// reused between contexts. Sink receivers are connected during construction;
/// context exit drains replay updates and any subsequent fills, then finalizes IO.
#[pyclass(name = "ReplayContext", module = "lobo.replay")]
pub struct PyReplayContext {
    sources: Vec<Py<PyItchSource>>,
    collect_stats: bool,
    cutoff_time: Option<DateTime<Utc>>,
    books: Option<HashMap<String, Py<PyBook>>>,
    stats: Option<ItchReplayStats>,
    session: Option<SinkSession>,
    #[cfg(feature = "concurrent")]
    concurrent: bool,
}

#[cfg(feature = "concurrent")]
#[pymethods]
impl PyReplayContext {
    #[new]
    #[pyo3(signature = (source, *, collect_stats=false, concurrent=false, cutoff_time=None, sinks=None))]
    fn new(
        source: &Bound<'_, PyAny>,
        collect_stats: bool,
        concurrent: bool,
        cutoff_time: Option<Bound<'_, PyDateTime>>,
        sinks: Option<Vec<Py<PyFeatherSink>>>,
    ) -> PyResult<Self> {
        Self::new_inner(source, collect_stats, concurrent, cutoff_time, sinks)
    }
}
#[cfg(not(feature = "concurrent"))]
#[pymethods]
impl PyReplayContext {
    #[new]
    #[pyo3(signature = (source, *, collect_stats=false, cutoff_time=None, sinks=None))]
    fn new(
        source: &Bound<'_, PyAny>,
        collect_stats: bool,
        cutoff_time: Option<Bound<'_, PyDateTime>>,
        sinks: Option<Vec<Py<PyFeatherSink>>>,
    ) -> PyResult<Self> {
        Self::new_inner(source, collect_stats, false, cutoff_time, sinks)
    }
}

#[pymethods]
impl PyReplayContext {
    fn __enter__(mut slf: PyRefMut<'_, Self>) -> PyResult<PyRefMut<'_, Self>> {
        let py = slf.py();
        if slf.session.is_none() {
            return Err(PyRuntimeError::new_err("ReplayContext is closed"));
        }
        if slf.books.is_none() {
            if let Err(error) = slf.replay(py) {
                let _ = slf.close(py);
                return Err(error);
            }
        }
        Ok(slf)
    }
    fn __exit__(
        &mut self,
        py: Python<'_>,
        _exception_type: &Bound<'_, PyAny>,
        _exception: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        self.close(py)?;
        Ok(false)
    }
    fn close(&mut self, py: Python<'_>) -> PyResult<()> {
        if self.session.is_none() {
            return Ok(());
        }
        if let Some(books) = &self.books {
            for book in books.values() {
                book.try_borrow_mut(py)?.book.disconnect();
            }
        }
        if let Some(session) = self.session.take() {
            session.close(py)?;
        }
        Ok(())
    }
    #[getter]
    fn closed(&self) -> bool {
        self.session.is_none()
    }
    fn __getitem__(&self, py: Python<'_>, ticker: &str) -> PyResult<Py<PyBook>> {
        let ticker = ticker.trim().to_ascii_lowercase();
        self.ready_books()?
            .get(&ticker)
            .map(|book| book.clone_ref(py))
            .ok_or_else(|| PyKeyError::new_err(ticker))
    }
    fn __contains__(&self, ticker: &str) -> PyResult<bool> {
        Ok(self
            .ready_books()?
            .contains_key(&ticker.trim().to_ascii_lowercase()))
    }
    fn __len__(&self) -> PyResult<usize> {
        Ok(self.ready_books()?.len())
    }
    fn keys(&self) -> PyResult<Vec<String>> {
        let mut keys = self.ready_books()?.keys().cloned().collect::<Vec<_>>();
        keys.sort_unstable();
        Ok(keys)
    }
    #[getter]
    fn cutoff_time(&self) -> Option<DateTime<Utc>> {
        self.cutoff_time
    }
    #[getter]
    fn message_count(&self) -> u64 {
        self.stats.map_or(0, |stats| stats.replay_messages)
    }
    #[getter]
    fn skipped_message_count(&self) -> usize {
        0
    }
    fn __repr__(&self) -> String {
        format!(
            "ReplayContext(book_count={}, message_count={}, skipped_message_count=0)",
            self.books.as_ref().map_or(0, HashMap::len),
            self.message_count()
        )
    }
}
impl PyReplayContext {
    fn new_inner(
        source: &Bound<'_, PyAny>,
        collect_stats: bool,
        concurrent: bool,
        cutoff_time: Option<Bound<'_, PyDateTime>>,
        sinks: Option<Vec<Py<PyFeatherSink>>>,
    ) -> PyResult<Self> {
        let cutoff_time = normalize_cutoff(cutoff_time)?;
        let sources = if let Ok(source) = source.extract::<Py<PyItchSource>>() {
            vec![source]
        } else {
            let iterator = source.try_iter().map_err(|_| {
                PyTypeError::new_err("source must be an ItchSource or an iterable of ItchSource")
            })?;
            iterator
                .map(|source| -> PyResult<_> {
                    source?.extract::<Py<PyItchSource>>().map_err(PyErr::from)
                })
                .collect::<PyResult<Vec<_>>>()?
        };
        if sources.is_empty() {
            return Err(PyValueError::new_err(
                "source iterable must contain at least one ItchSource",
            ));
        }
        #[cfg(not(feature = "concurrent"))]
        let _ = concurrent;
        Ok(Self {
            sources,
            collect_stats,
            cutoff_time,
            books: None,
            stats: None,
            session: Some(SinkSession::connect(
                source.py(),
                sinks.unwrap_or_default(),
            )?),
            #[cfg(feature = "concurrent")]
            concurrent,
        })
    }
    fn ready_books(&self) -> PyResult<&HashMap<String, Py<PyBook>>> {
        self.books.as_ref().ok_or_else(|| {
            PyRuntimeError::new_err("ReplayContext must be entered before accessing books")
        })
    }
    fn replay(&mut self, py: Python<'_>) -> PyResult<()> {
        let publisher = self.session.as_ref().and_then(SinkSession::publisher);
        match (publisher, self.cutoff_time.is_some()) {
            (Some(publisher), true) => self.replay_with::<_, true>(py, publisher),
            (Some(publisher), false) => self.replay_with::<_, false>(py, publisher),
            (None, true) => self.replay_with::<_, true>(py, NullPublisher),
            (None, false) => self.replay_with::<_, false>(py, NullPublisher),
        }
    }
    // Publisher and cutoff selectors stay outside the native hot loop.
    #[inline(never)]
    fn replay_with<P, const HAS_CUTOFF: bool>(
        &mut self,
        py: Python<'_>,
        publisher: P,
    ) -> PyResult<()>
    where
        P: BookPublisherFactory<lobo_primitives::CompressedPrice> + Send,
        PyBook: From<NativeBook<P>>,
    {
        let cutoff_time = if HAS_CUTOFF { self.cutoff_time } else { None };
        let sources = self
            .sources
            .iter()
            .map(|source| source.borrow(py).native().clone())
            .collect::<Vec<_>>();
        let collect_stats = self.collect_stats;
        #[cfg(feature = "concurrent")]
        let concurrent = self.concurrent;
        let (books, stats) = py.detach(move || -> PyResult<_> {
            let mut context =
                NativeReplayContext::with_sinks(cutoff_time, ConnectedPublisher(publisher))
                    .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
            let sources = sources.iter().collect::<Vec<_>>();
            let stats = if collect_stats {
                Some(
                    context
                        .replay_from_sources_with_stats(&sources)
                        .map_err(|error| PyRuntimeError::new_err(error.to_string()))?,
                )
            } else {
                #[cfg(not(feature = "concurrent"))]
                context
                    .replay_from_sources(&sources)
                    .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
                #[cfg(feature = "concurrent")]
                if concurrent {
                    context
                        .parallel_replay_from_sources(&sources, 12)
                        .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
                } else {
                    context
                        .replay_from_sources(&sources)
                        .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
                }
                None
            };
            Ok((context.books, stats))
        })?;
        self.books = Some(
            books
                .into_iter()
                .map(|(ticker, book)| Ok((ticker, Py::new(py, PyBook::from(book))?)))
                .collect::<PyResult<_>>()?,
        );
        self.stats = stats;
        Ok(())
    }
}

/// Resolve Python timezone rules once, before entering the native replay loop.
fn normalize_cutoff(cutoff: Option<Bound<'_, PyDateTime>>) -> PyResult<Option<DateTime<Utc>>> {
    cutoff
        .map(|cutoff| {
            // tzinfo alone is insufficient: a custom timezone may return None.
            if cutoff.call_method0("utcoffset")?.is_none() {
                return Err(PyValueError::new_err(
                    "cutoff_time must be a timezone-aware datetime",
                ));
            }
            // Resolve ZoneInfo/DST/fold on the original date before extraction.
            // Direct chrono UTC extraction only accepts Python's UTC timezone.
            cutoff
                .call_method1("astimezone", (PyTzInfo::utc(cutoff.py())?,))?
                .extract()
        })
        .transpose()
}
