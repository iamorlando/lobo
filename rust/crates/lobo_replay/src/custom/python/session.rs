//! Shared Python access to running adapter sessions.
use super::{
    PyAdapterInfo, PyCustomAdapter,
    adapter::{error, to_python},
};
use crate::custom::{
    MarketDataAdapter,
    runtime::{Driver, Session},
};
use lobo_models::{
    Side,
    orders::order_types::{LimitOrder, MarketOrder},
};
use lobo_primitives::{Price64, PriceType, uuid::Uuid};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};
use serde_json::{Value, json};

/// Access books, executions, and simulations while an adapter consumes its source.
///
/// Obtain a session from a source's start() method. The worker continues receiving
/// messages while Python inspects its books. Use a with block to stop the worker
/// when leaving the scope, including when an exception occurs.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import kraken
///
///     with kraken.KrakenSource("BTC/USD").start() as session:
///         available = session.tickers()
///     ```
#[pyclass(
    name = "AdapterSession",
    module = "lobo.replay.adapters",
    skip_from_py_object
)]
pub struct PyAdapterSession {
    session: Option<Session>,
}
impl PyAdapterSession {
    /// Start the existing adapter on its source worker and return its Python handle.
    pub fn spawn<F, A>(py: Python<'_>, factory: F, source: impl Driver) -> PyResult<Self>
    where
        F: FnOnce() -> Result<A, String> + Send + 'static,
        A: MarketDataAdapter + 'static,
    {
        py.detach(|| Session::spawn(factory, source))
            .map(|session| Self {
                session: Some(session),
            })
            .map_err(error)
    }
    fn active(&self) -> PyResult<&Session> {
        self.session
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Adapter session is closed"))
    }
}
#[pymethods]
impl PyAdapterSession {
    /// The running source's identity, feed mode, and book level.
    #[getter]
    fn info(&self, py: Python<'_>) -> PyResult<PyAdapterInfo> {
        let session = self.active()?;
        py.detach(|| {
            session.with(|a| {
                Ok(PyAdapterInfo {
                    descriptor: a.info().into(),
                })
            })
        })
        .map_err(error)
    }
    /// List the instrument symbols discovered so far.
    ///
    /// Returns:
    ///     Normalized tickers. A live source may still be discovering its directory.
    ///
    /// Raises:
    ///     RuntimeError: The session is closed.
    fn tickers(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        let session = self.active()?;
        py.detach(|| session.with(|a| Ok(a.tickers())))
            .map_err(error)
    }
    /// Wait for source completion and report any decoder or checksum failure.
    ///
    /// Finite recordings retain their books for inspection after this method returns.
    /// A live WebSocket normally continues until close() is called from another thread.
    ///
    /// Raises:
    ///     RuntimeError: The session is closed or the source worker failed.
    fn wait(&self, py: Python<'_>) -> PyResult<()> {
        let session = self.active()?;
        py.detach(|| session.wait()).map_err(error)
    }
    /// Whether the source has finished; wait() reports a completed source's error.
    #[getter]
    fn finished(&self) -> PyResult<bool> {
        Ok(self.active()?.finished())
    }
    /// Stop the worker and release its books. Repeated calls are safe.
    fn close(&mut self, py: Python<'_>) {
        if let Some(mut session) = self.session.take() {
            py.detach(|| session.close());
        }
    }
    /// Return this running session and close it automatically when the scope exits.
    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }
    /// Close this session when its with block exits.
    ///
    /// Args:
    ///     _ty: The exception type supplied by Python, or None.
    ///     _value: The exception instance supplied by Python, or None.
    ///     _trace: The exception traceback supplied by Python, or None.
    ///
    /// Returns:
    ///     None. Exceptions from the context body are not suppressed.
    fn __exit__(
        &mut self,
        py: Python<'_>,
        _ty: &Bound<'_, PyAny>,
        _value: &Bound<'_, PyAny>,
        _trace: &Bound<'_, PyAny>,
    ) {
        self.close(py);
    }
}

// Both Python entry points expose the same book-query implementation and docs.
macro_rules! session_queries {
    ($binding:ty) => {
        #[pymethods]
        impl $binding {
    /// Select the active book and request its live subscription if needed.
    ///
    /// Args:
    ///     symbol: A registered ticker inside the adapter's book scope. Previously
    ///         visited live instruments retain their subscriptions.
    ///
    /// Raises:
    ///     RuntimeError: The adapter has no started session, or the ticker is
    ///         unavailable or outside scope.
    fn select(&self, py: Python<'_>, symbol: String) -> PyResult<()> {
        let session = self.active()?;
        py.detach(|| session.with(move |a| a.select_ticker(&symbol)))
            .map_err(error)
    }
    /// Read session progress and synchronization for the selected book.
    ///
    /// Returns:
    ///     A dictionary containing symbol, messages, bytes, clock_ns, complete,
    ///     synchronized, checksum_checks, checksum_failures, and books. clock_ns uses
    ///     the source timeline; complete describes source completion.
    ///
    /// Raises:
    ///     RuntimeError: The adapter has no started session.
    fn status(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let session = self.active()?;
        let value=py.detach(||session.with(|a| { let s=a.state();Ok(json!({"symbol":s.selected,"messages":s.messages,"bytes":s.consumed,"clock_ns":s.clock_ns,"complete":s.complete,"synchronized":s.synchronized(&s.selected),"checksum_checks":s.checksum_checks,"checksum_failures":s.checksum_failures,"books":s.instruments.keys().filter(|s|a.state().book(s).is_some()).count()})) })).map_err(error)?;
        to_python(py, value)
    }
    /// Read price levels from a book on the currently displayed timeline.
    ///
    /// Args:
    ///     symbol: The registered book symbol to inspect.
    ///
    /// Returns:
    ///     A list of dictionaries with side, price, quantity, hidden, and orders.
    ///     Price is decimal text containing integer atoms; quantities are integer atoms.
    ///     Hidden totals can be zero when their book policy disables maintenance.
    ///
    /// Raises:
    ///     RuntimeError: The adapter has no started session or the book is unavailable.
    fn levels(&self, py: Python<'_>, symbol: String) -> PyResult<Py<PyAny>> {
        use lobo_storage::price_level::{HasHiddenQuantity, PriceLevelContract};
        let session = self.active()?;
        let value=py.detach(||session.with(move |a| {
            let book=a.state().view().book(&symbol).ok_or("Book is not available")?;
            Ok(crate::dispatch_feed_book!(book,native,Value::Array(native.order_storage.bids.visible_price_levels().chain(native.order_storage.asks.visible_price_levels()).map(|(p,l)|json!({"price":p.into_u128().to_string(),"quantity":l.visible_quantity(),"hidden":l.hidden_quantity(),"orders":l.len(),"side":match l.side(){Side::Buy=>"buy",Side::Sell=>"sell"}})).collect())))
        })).map_err(error)?;
        to_python(py, value)
    }
    /// Read resting L3 orders within an inclusive range of price levels.
    ///
    /// The view preserves each level's queue priority, including replenished iceberg
    /// orders. It does not reconstruct priority by sorting original timestamps.
    ///
    /// Args:
    ///     symbol: The registered book symbol to inspect.
    ///     side: "buy" or "sell".
    ///     min_price: The inclusive lower price bound in integer atoms.
    ///     max_price: The inclusive upper price bound in integer atoms.
    ///
    /// Returns:
    ///     Tuples of (order_id, price, visible_quantity, created_at_ns), in the store's
    ///     level and queue order. IDs are integers; prices and quantities use atoms.
    ///
    /// Raises:
    ///     RuntimeError: The adapter has no started session or the book is unavailable.
    ///     ValueError: side is neither "buy" nor "sell".
    fn queue(
        &self,
        py: Python<'_>,
        symbol: String,
        side: &str,
        min_price: u64,
        max_price: u64,
    ) -> PyResult<Vec<(u128, u64, u64, i64)>> {
        let side = parse_side(side)?;
        let session = self.active()?;
        py.detach(|| {
            session.with(move |a| {
                let book = a
                    .state()
                    .view()
                    .book(&symbol)
                    .ok_or("Book is not available")?;
                Ok(book
                    .queue_view(side, Price64::from(min_price)..=Price64::from(max_price))
                    .into_iter()
                    .map(|o| {
                        (
                            o.id.as_u128(),
                            o.price.into_u128() as u64,
                            o.quantity,
                            o.created_at.timestamp_nanos_opt().unwrap_or(0),
                        )
                    })
                    .collect())
            })
        })
        .map_err(error)
    }
    /// Preview a market order or start a simulated L3 resting limit order.
    ///
    /// Market previews report immediate fills without creating an alternate timeline.
    /// L3 limit simulations retain a branch for any resting remainder while source
    /// messages continue. The main timeline remains available through return_to_main().
    ///
    /// Args:
    ///     side: The incoming order side, "buy" or "sell".
    ///     quantity: The requested positive quantity in integer quantity atoms.
    ///     price: An optional limit price in integer price atoms. None creates a market
    ///         preview. A limit price requires an L3 source with a synchronized book.
    ///
    /// Raises:
    ///     RuntimeError: The adapter has no started session, the book is not
    ///         synchronized, or its order and simulation rules reject the request.
    ///     ValueError: side is neither "buy" nor "sell".
    #[pyo3(signature=(side,quantity,*,price=None))]
    fn simulate(
        &self,
        py: Python<'_>,
        side: &str,
        quantity: u64,
        price: Option<u64>,
    ) -> PyResult<()> {
        let side = parse_side(side)?;
        let session = self.active()?;
        py.detach(|| {
            session.with(move |a| match price {
                Some(price) => a.simulate(LimitOrder::new(
                    Some(Price64::from(price)),
                    quantity,
                    Uuid::nil(),
                    side,
                )),
                None => a.simulate_market(MarketOrder::new(quantity, Uuid::nil(), side)),
            })
        })
        .map_err(error)
    }
    /// Read simulated fills and the remaining quantity on the selected timeline.
    ///
    /// Returns:
    ///     None before a simulation, otherwise a dictionary with simulated, order_id,
    ///     symbol, requested, filled, remaining, average_price, complete,
    ///     alternate_timeline, ignored, stopped, and executions. Prices use price atoms.
    ///     Each execution contains simulated, timestamp_ns, price, quantity, and sequence.
    ///     A stopped alternate timeline remains selected until return_to_main().
    ///
    /// Raises:
    ///     RuntimeError: The adapter has no started session.
    fn simulation_report(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let session = self.active()?;
        let value=py.detach(||session.with(|a| {
            let state=a.state();let branch=state.simulation.as_ref();
            let Some(report)=branch.map(|b|&b.report).or(state.market_preview.as_ref()) else { return Ok(Value::Null); };
            Ok(json!({"simulated":true,"order_id":report.order_id.to_string(),"symbol":report.symbol.as_ref(),"requested":report.requested,"filled":report.filled,"remaining":report.remaining(),"average_price":report.average_price(),"complete":report.complete(),"alternate_timeline":branch.is_some(),"ignored":branch.map_or(0,|b|b.ignored),"stopped":branch.map_or(report.complete(),|b|b.stopped()),"executions":report.executions.iter().map(|e|json!({"simulated":true,"timestamp_ns":e.timestamp_ns(),"price":u64::from(e.event().execution.price),"quantity":e.event().execution.quantity,"sequence":e.sequence_number()})).collect::<Vec<_>>()}))
        })).map_err(error)?;
        to_python(py, value)
    }
    /// Choose how reported executions accumulate into OHLC bars.
    ///
    /// Changing aggregation resets the bar accumulator, including the simulation's
    /// accumulator when an alternate timeline exists.
    ///
    /// Args:
    ///     kind: "volume" for quantity, "ticks" for execution count, "time" for
    ///         source-time intervals, or "notional" for execution price times quantity.
    ///     size: The positive bar threshold. Units are quantity atoms for volume,
    ///         executions for ticks, nanoseconds for time, and price-atoms times
    ///         quantity-atoms for notional.
    ///
    /// Raises:
    ///     RuntimeError: The adapter has no started session.
    ///     ValueError: kind is unknown or size is zero.
    fn set_bar_aggregation(&self, py: Python<'_>, kind: &str, size: u64) -> PyResult<()> {
        use lobo_batchers::volume::Aggregation;
        let size = std::num::NonZeroU64::new(size)
            .ok_or_else(|| PyValueError::new_err("Bar size must be positive"))?;
        let aggregation = match kind {
            "volume" => Aggregation::Volume(size),
            "ticks" => Aggregation::Ticks(size),
            "time" => Aggregation::Time(size),
            "notional" => Aggregation::Notional(size),
            _ => {
                return Err(PyValueError::new_err(
                    "Aggregation must be volume, ticks, time or notional",
                ));
            }
        };
        let session = self.active()?;
        py.detach(|| {
            session.with(move |a| {
                let state = a.state_mut();
                state.set_bar_aggregation(aggregation);
                if let Some(branch) = state.simulation.as_mut() {
                    branch.feed.set_bar_aggregation(aggregation);
                }
                Ok(())
            })
        })
        .map_err(error)
    }
    /// Take completed OHLC bars from the current timeline's output queue.
    ///
    /// Returns:
    ///     A list of dictionaries with symbol, open, high, low, close, volume, ticks,
    ///     index, start_ns, and end_ns. Price and volume use instrument atoms. Bars
    ///     are removed from the queue, so a subsequent call returns only new output.
    ///
    /// Raises:
    ///     RuntimeError: The adapter has no started session.
    fn drain_bars(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let session = self.active()?;
        let value=py.detach(||session.with(|a| {
            let bars=a.state().view().volume_bars().borrow();
            let events=std::mem::take(&mut *bars.destination.0.borrow_mut());
            Ok(Value::Array(events.into_iter().map(|e| {let b=e.event();json!({"symbol":e.book_id(),"open":u64::from(b.open),"high":u64::from(b.high),"low":u64::from(b.low),"close":u64::from(b.close),"volume":b.volume,"ticks":b.ticks,"index":b.index,"start_ns":b.start_ns,"end_ns":b.end_ns})}).collect()))
        })).map_err(error)?;
        to_python(py, value)
    }
    /// Discard the simulation branch and select the continuing main timeline.
    ///
    /// The main book has continued receiving source messages during the simulation.
    /// Calling this method does not restart the source or replay its earlier messages.
    ///
    /// Raises:
    ///     RuntimeError: The adapter has no started session.
    fn return_to_main(&self, py: Python<'_>) -> PyResult<()> {
        let session = self.active()?;
        py.detach(|| {
            session.with(|a| {
                a.return_to_main();
                Ok(())
            })
        })
        .map_err(error)
    }
        }
    };
}
session_queries!(PyAdapterSession);
session_queries!(PyCustomAdapter);

fn parse_side(side: &str) -> PyResult<Side> {
    match side {
        "buy" | "bid" => Ok(Side::Buy),
        "sell" | "ask" => Ok(Side::Sell),
        _ => Err(PyValueError::new_err("side must be buy or sell")),
    }
}
