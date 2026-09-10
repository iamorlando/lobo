//! Thin bindings: Rust owns the runtime, registry, commands and HTTP schemas.
use crate::registry::RegisteredBooks;
use crate::{RegisteredBook, Server};
use lobo_books::price_time_priority::{
    PyBook,
    python::{
        hosted::{self, HostedBook, HostedCommands},
    },
};
use pyo3::{
    exceptions::{PyKeyError, PyRuntimeError, PyValueError},
    prelude::*,
};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};

impl<
    U: lobo_storage::UserMapUpdatePolicy + Send,
    H: lobo_storage::policies::HiddenQuantityPolicy + Send,
    C: lobo_storage::policies::checksum::ChecksumPolicy,
> HostedCommands for RegisteredBook<U, H, C>
{
    fn apply(
        &self,
        command: lobo_models::server::Command,
        reports: lobo_models::events::Reports,
    ) -> Result<
        lobo_books::price_time_priority::CommandResult<lobo_primitives::CompressedPrice>,
        String,
    > {
        self.apply(command, reports)
            .map(|(result, _)| result)
            .map_err(|e| e.to_string())
    }
}
fn registration(book: RegisteredBooks) -> PyBook {
    crate::registry::dispatch_registered!(book, book, PyBook::from(HostedBook {
        native: book.native.clone(), commands: book,
    }))
}
fn python_book(py: Python<'_>, book: RegisteredBooks) -> PyResult<Py<PyAny>> {
    Py::new(py, registration(book)).map(Py::into_any)
}
#[pyclass(name = "ServerContext", module = "lobo.server")]
pub struct PyServerContext {
    adapters: Vec<Py<lobo_replay::custom::python::PyCustomAdapter>>,
    server: Option<Server>,
    token: Option<Py<PyAny>>,
    price_decimals: u8,
    quantity_decimals: u8,
}
#[pymethods]
impl PyServerContext {
    #[new]
    #[pyo3(signature = (web_root, *, host="127.0.0.1", port=8000, price_decimals=0, quantity_decimals=0, queue_capacity=8192, sinks=None, adapters=None, replay_paused=true, replay_speed=1.0))]
    fn new(
        py: Python<'_>,
        web_root: PathBuf,
        host: &str,
        port: u16,
        price_decimals: u8,
        quantity_decimals: u8,
        queue_capacity: usize,
        sinks: Option<Vec<Py<lobo_context::python::PyGpuSink>>>,
        adapters: Option<Vec<Py<lobo_replay::custom::python::PyCustomAdapter>>>,
        replay_paused: bool,
        replay_speed: f64,
    ) -> PyResult<Self> {
        if price_decimals > 9 || quantity_decimals > 18 {
            return Err(PyValueError::new_err("invalid book precision"));
        }
        let host: IpAddr = host
            .parse()
            .map_err(|_| PyValueError::new_err("host must be an IP address"))?;
        let sinks = sinks.unwrap_or_default();
        if sinks.len() > 1 {
            return Err(PyValueError::new_err(
                "a server hosts one GPU terminal; pass at most one GpuSink",
            ));
        }
        let metrics = sinks
            .first()
            .map_or_else(Default::default, |sink| sink.borrow(py).metrics);
        let server = py
            .detach(|| {
                Server::start_with_metrics(
                    SocketAddr::new(host, port),
                    web_root,
                    queue_capacity,
                    metrics,
                )
            })
            .map_err(PyRuntimeError::new_err)?;
        let adapters = adapters.unwrap_or_default();
        for adapter in &adapters {
            match adapter.try_borrow_mut(py)?.attach(
                py,
                queue_capacity,
                replay_paused,
                replay_speed,
            ) {
                Ok(hosted) => server.registry.adapters.write().push(hosted),
                Err(error) => {
                    for adapter in &adapters {
                        let _ = adapter.try_borrow_mut(py).map(|mut a| a.close_hosted(py));
                    }
                    return Err(error);
                }
            }
        }
        Ok(Self {
            adapters,
            server: Some(server),
            token: None,
            price_decimals,
            quantity_decimals,
        })
    }
    fn __enter__(mut slf: PyRefMut<'_, Self>) -> PyResult<PyRefMut<'_, Self>> {
        if slf.token.is_some() {
            return Err(PyRuntimeError::new_err(
                "this server context is already entered",
            ));
        }
        let registry = slf.open()?.registry.clone();
        let price_decimals = slf.price_decimals;
        let quantity_decimals = slf.quantity_decimals;
        slf.token = Some(hosted::enter(
            slf.py(),
            Arc::new(move |name, policy, checksum| {
                registry
                    .register_options(name, price_decimals, quantity_decimals, policy, checksum)
                    .map(registration)
                    .map_err(PyValueError::new_err)
            }),
        )?);
        Ok(slf)
    }
    fn __exit__(
        &mut self,
        py: Python<'_>,
        _kind: &Bound<'_, PyAny>,
        _value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        self.close(py)?;
        Ok(false)
    }
    #[getter]
    fn url(&self) -> PyResult<String> {
        Ok(self.open()?.url())
    }
    #[getter]
    fn port(&self) -> PyResult<u16> {
        Ok(self.open()?.address().port())
    }
    #[getter]
    fn closed(&self) -> bool {
        self.server.is_none()
    }
    /// Create or retrieve a named book, including from a worker thread.
    fn book(&self, py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
        let registry = &self.open()?.registry;
        let book = match registry.get(name) {
            Some(book) => book,
            None => registry
                .register_policy(
                    Some(name.into()),
                    self.price_decimals,
                    self.quantity_decimals,
                    lobo_models::server::BookPolicy::Full,
                )
                .map_err(PyValueError::new_err)?,
        };
        python_book(py, book)
    }
    fn __getitem__(&self, py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
        let book = self
            .open()?
            .registry
            .get(name)
            .ok_or_else(|| PyKeyError::new_err(name.to_owned()))?;
        python_book(py, book)
    }
    fn keys(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        let registry = self.open()?.registry.clone();
        py.detach(move || {
            let mut symbols = registry
                .directory()
                .into_iter()
                .map(|b| b.symbol)
                .collect::<std::collections::BTreeSet<_>>();
            for adapter in registry.adapters.read().iter() {
                for book in adapter.books()? {
                    if let Some(symbol) = book["symbol"].as_str() {
                        symbols.insert(symbol.into());
                    }
                }
            }
            Ok::<_, String>(symbols.into_iter().collect())
        })
        .map_err(PyRuntimeError::new_err)
    }

    fn close(&mut self, py: Python<'_>) -> PyResult<()> {
        for adapter in self.adapters.drain(..) {
            adapter.try_borrow_mut(py)?.close_hosted(py)?;
        }
        if let Some(token) = self.token.take() {
            hosted::exit(py, token)?;
        }
        if let Some(mut server) = self.server.take() {
            py.detach(move || server.close());
        }
        Ok(())
    }
}
impl PyServerContext {
    fn open(&self) -> PyResult<&Server> {
        self.server
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("server context is closed"))
    }
}
