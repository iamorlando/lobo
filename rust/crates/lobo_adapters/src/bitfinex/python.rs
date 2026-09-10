//! Python configuration for the existing Bitfinex adapter.
#![warn(missing_docs)]
use super::Bitfinex;
use crate::adapter::MarketDataAdapter;
use lobo_replay::custom::python::{PyAdapterSession, PySource};
use lobo_replay::custom::runtime::Source;
use pyo3::{exceptions::PyValueError, prelude::*};

/// Configure the Bitfinex WebSocket v2 R0 order book feed.
///
/// Construction prepares configuration without opening a connection. Call start()
/// to receive messages and inspect books through the returned AdapterSession.
/// Previously selected pairs retain their live subscriptions. All messages use the
/// existing Bitfinex decoder, snapshot handling, checksum policy, and execution paths.
///
/// Args:
///     symbol: Initially selected pair, such as "BTCUSD". Defaults to "BTCUSD".
///     source: Optional models.Source describing a WebSocket endpoint or recorded
///         packets/JSON lines. None connects to the exchange's public WebSocket.
///         Recorded input must include the exchange's directory and snapshot messages.
///
/// Raises:
///     ValueError: The symbol or source configuration is invalid.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import bitfinex
///
///     with bitfinex.BitfinexSource("BTCUSD").start() as session:
///         available = session.tickers()
///     ```
#[pyclass(
    name = "BitfinexSource",
    module = "lobo.replay.adapters.bitfinex",
    frozen,
    skip_from_py_object
)]
pub struct PyBitfinexSource {
    symbol: String,
    source: Source,
}
#[pymethods]
impl PyBitfinexSource {
    #[new]
    #[pyo3(signature=(symbol="BTCUSD",*,source=None))]
    fn new(symbol: &str, source: Option<PySource>) -> PyResult<Self> {
        // Reuse adapter construction to validate configuration before any input is read.
        let symbol = Bitfinex::new(symbol)
            .map_err(PyValueError::new_err)?
            .state()
            .selected
            .clone();
        Ok(Self {
            symbol,
            source: source.map_or(
                Source::WebSocket {
                    endpoint: None,
                    heartbeat: None,
                },
                |s| s.transport(),
            ),
        })
    }
    /// The normalized pair selected when a session starts.
    #[getter]
    fn symbol(&self) -> &str {
        &self.symbol
    }
    /// Start receiving this source using the Bitfinex adapter.
    ///
    /// Returns:
    ///     An AdapterSession with ticker selection, book levels, execution bars,
    ///     and the simulations supported by this source's book level. Use it as a
    ///     context manager to stop its worker automatically.
    ///
    /// Raises:
    ///     RuntimeError: The adapter worker cannot start. Source and checksum errors
    ///         encountered after startup are reported by the session's wait() method.
    fn start(&self, py: Python<'_>) -> PyResult<PyAdapterSession> {
        let symbol = self.symbol.clone();
        PyAdapterSession::spawn(py, move || Bitfinex::new(&symbol), self.source.clone())
    }
}
