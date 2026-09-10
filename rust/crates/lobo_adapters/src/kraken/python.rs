//! Python configuration for the existing Kraken adapter.
#![warn(missing_docs)]
use super::Kraken;
use crate::adapter::MarketDataAdapter;
use lobo_replay::custom::python::{PyAdapterSession, PySource};
use lobo_replay::custom::runtime::Source;
use pyo3::{exceptions::PyValueError, prelude::*};

/// Configure the Kraken Spot WebSocket v2 aggregate book feed.
///
/// Construction prepares configuration without opening a connection. Call start()
/// to receive messages and inspect books through the returned AdapterSession.
/// Previously selected pairs retain their live subscriptions. All messages use the
/// existing Kraken decoder, snapshot handling, checksum policy, and execution paths.
///
/// Args:
///     symbol: Initially selected pair, such as "BTC/USD". Defaults to "BTC/USD".
///     depth: Number of levels per side: 10, 25, 100, 500, or 1000. Defaults to 10.
///     source: Optional models.Source describing a WebSocket endpoint or recorded
///         packets/JSON lines. None connects to the exchange's public WebSocket.
///         Recorded input must include the exchange's directory and snapshot messages.
///
/// Raises:
///     ValueError: The symbol or source configuration is invalid.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import kraken
///
///     with kraken.KrakenSource("BTC/USD").start() as session:
///         available = session.tickers()
///     ```
#[pyclass(
    name = "KrakenSource",
    module = "lobo.replay.adapters.kraken",
    frozen,
    skip_from_py_object
)]
pub struct PyKrakenSource {
    symbol: String,
    depth: usize,
    source: Source,
}
#[pymethods]
impl PyKrakenSource {
    #[new]
    #[pyo3(signature=(symbol="BTC/USD",depth=10,*,source=None))]
    fn new(symbol: &str, depth: usize, source: Option<PySource>) -> PyResult<Self> {
        // Reuse adapter construction to validate configuration before any input is read.
        let symbol = Kraken::new(symbol, depth)
            .map_err(PyValueError::new_err)?
            .state()
            .selected
            .clone();
        Ok(Self {
            symbol,
            depth,
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
    /// The number of subscribed aggregate price levels per side.
    #[getter]
    fn depth(&self) -> usize {
        self.depth
    }
    /// Start receiving this source using the Kraken adapter.
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
        let depth = self.depth;
        PyAdapterSession::spawn(py, move || Kraken::new(&symbol, depth), self.source.clone())
    }
}
