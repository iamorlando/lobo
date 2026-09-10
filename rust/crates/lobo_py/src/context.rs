use lobo_books::price_time_priority::python::{NativeBook, PyBook};
use lobo_context::python::{PyFeatherSink, SinkSession};
use pyo3::{
    exceptions::{PyKeyError, PyRuntimeError, PyValueError},
    prelude::*,
};
use std::collections::HashMap;

/// A context for generated orders and interactive fills.
/// Create FeatherSink objects first, pass sinks=[...] at construction, then
/// obtain books with context.book("name"). Exit/close drains every destination.
#[pyclass(name = "Context", module = "lobo.context")]
pub struct PyContext {
    books: HashMap<String, Py<PyBook>>,
    session: Option<SinkSession>,
}

#[pymethods]
impl PyContext {
    #[new]
    #[pyo3(signature = (*, sinks=None))]
    fn new(py: Python<'_>, sinks: Option<Vec<Py<PyFeatherSink>>>) -> PyResult<Self> {
        Ok(Self {
            books: HashMap::new(),
            session: Some(SinkSession::connect(py, sinks.unwrap_or_default())?),
        })
    }
    fn __enter__(slf: PyRef<'_, Self>) -> PyResult<PyRef<'_, Self>> {
        slf.open_session()?;
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
    /// Return a named book, creating it and connecting its publisher if needed.
    fn book(&mut self, py: Python<'_>, name: &str) -> PyResult<Py<PyBook>> {
        let publisher = self.open_session()?.publisher();
        let name = name.trim();
        if name.is_empty() {
            return Err(PyValueError::new_err("book name must not be empty"));
        }
        let book = match self.books.entry(name.to_owned()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let native = NativeBook::new().with_id(name.to_owned());
                let book = match publisher {
                    Some(publisher) => PyBook::from(native.with_publisher(publisher)),
                    None => PyBook::from(native),
                };
                entry.insert(Py::new(py, book)?)
            }
        };
        Ok(book.clone_ref(py))
    }
    fn __getitem__(&self, py: Python<'_>, name: &str) -> PyResult<Py<PyBook>> {
        self.books
            .get(name.trim())
            .map(|book| book.clone_ref(py))
            .ok_or_else(|| PyKeyError::new_err(name.to_owned()))
    }
    fn __len__(&self) -> usize {
        self.books.len()
    }
    fn keys(&self) -> Vec<String> {
        let mut keys = self.books.keys().cloned().collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }
    #[getter]
    fn closed(&self) -> bool {
        self.session.is_none()
    }
    /// Drain pending updates and write every Feather footer. Safe to call twice.
    fn close(&mut self, py: Python<'_>) -> PyResult<()> {
        if self.session.is_none() {
            return Ok(());
        }
        for book in self.books.values() {
            book.try_borrow_mut(py)?.book.disconnect();
        }
        if let Some(session) = self.session.take() {
            session.close(py)?;
        }
        Ok(())
    }
}
impl PyContext {
    fn open_session(&self) -> PyResult<&SinkSession> {
        self.session
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Context is closed"))
    }
}
