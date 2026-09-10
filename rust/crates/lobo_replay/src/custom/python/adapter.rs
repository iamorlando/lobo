//! Python adapter construction, source lifecycle, and book queries.
use crate::custom::{
    AdapterDescriptor, MarketDataAdapter,
    runtime::{Session, Source},
};
use lobo_context::{
    BookLevel, BookScope, FeedMode,
    python::{Publisher, PyFeatherSink, SinkSession},
};
use lobo_models::BookPolicy;
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyDict, PyList},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// The instruments, filtering, and sink policy used by a complete file replay.
#[derive(Clone)]
pub struct ReplayOptions {
    /// Instruments already known before directory discovery.
    pub instruments: Vec<Instrument>,
    /// The set of symbols whose books may be constructed.
    pub scope: BookScope,
    /// Whether independent books replay concurrently.
    pub concurrent: bool,
    /// Whether replay collects counts and timings.
    pub collect_stats: bool,
    /// Optional source-time cutoff converted to UTC.
    pub cutoff: Option<lobo_primitives::time::DateTime<lobo_primitives::time::Utc>>,
    /// Optional event publisher connected to the requested sinks.
    pub publisher: Option<Publisher>,
}
/// Convert the completed result after detaching from the hot loop.
/// Implementations may return the existing Python book wrappers.
pub trait ReplayOutput: Send {
    /// Convert completed books to their Python wrappers while holding the Python token.
    fn into_python(self: Box<Self>, py: Python<'_>) -> PyResult<PythonReplayResult>;
}
/// Completed Python books, optional statistics, and publisher cleanup.
pub struct PythonReplayResult {
    /// The Python dictionary containing the completed book wrappers.
    pub value: Py<PyAny>,
    /// The replay statistics exported to Python.
    pub stats: Value,
    /// Rust cleanup for Python books that retain sink publishers.
    pub disconnect: Box<dyn FnOnce(Python<'_>) -> PyResult<()> + Send + Sync>,
}

/// Specify where an adapter obtains its incoming messages.
///
/// Use file for a binary replay file, http for an HTTP binary replay stream,
/// websocket for a live feed, json_lines for recorded JSON messages, or packets for
/// an in-memory collection of raw messages. Constructing a Source does not open it.
/// CustomAdapter owns the source's lifecycle and obtains all book changes through
/// its Protocol.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import models as lm
///
///     source = lm.Source.file("01302020.NASDAQ_ITCH50.gz")
///     live = lm.Source.websocket("wss://feed.example/ws")
///     ```
#[pyclass(
    name = "Source",
    module = "lobo.replay.adapters.models",
    frozen,
    from_py_object
)]
#[derive(Clone)]
pub struct PySource {
    native: Source,
}
#[pymethods]
impl PySource {
    /// Describe a local binary replay file, optionally gzip-compressed.
    ///
    /// Args:
    ///     path: A string or path-like filename. Date-based intraday formats need the
    ///         session date in the filename, such as 01302020.NASDAQ_ITCH50.
    ///     chunk_size: Bytes read per input chunk, between 1 and 4,194,304. Defaults
    ///         to 1,048,576. This controls input buffering, not message or sink batch size.
    ///
    /// Returns:
    ///     A Source that opens the file when its adapter starts or runs.
    ///
    /// Raises:
    ///     ValueError: chunk_size is outside the supported range.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     source = lm.Source.file("01302020.NASDAQ_ITCH50.gz")
    ///     ```
    #[staticmethod]
    #[pyo3(signature=(path,*,chunk_size=1048576))]
    fn file(path: std::path::PathBuf, chunk_size: usize) -> PyResult<Self> {
        if chunk_size == 0 || chunk_size > 4 * 1024 * 1024 {
            return Err(PyValueError::new_err(
                "chunk_size must be between 1 and 4194304",
            ));
        }
        Ok(Self {
            native: Source::File { path, chunk_size },
        })
    }
    /// Describe a binary replay streamed from an HTTP or HTTPS URL.
    ///
    /// The reader decompresses gzip content as it arrives. It does not save a local
    /// copy. Use start() and wait() for this streaming source; run() requires a local file.
    ///
    /// Args:
    ///     url: The complete HTTP or HTTPS URL. Intraday date-based formats need a
    ///         session date in the URL filename, just as a local replay file does.
    ///     chunk_size: Maximum reader chunk size in bytes, from 1 through 4,194,304.
    ///         Defaults to 1,048,576.
    ///
    /// Returns:
    ///     A Source that requests the URL when the adapter starts.
    ///
    /// Raises:
    ///     ValueError: The URL scheme or chunk_size is unsupported.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     source = lm.Source.http("https://feed.example/01302020.NASDAQ_ITCH50.gz")
    ///     ```
    #[staticmethod]
    #[pyo3(signature=(url,*,chunk_size=1048576))]
    fn http(url: String, chunk_size: usize) -> PyResult<Self> {
        let parsed = reqwest::Url::parse(&url).map_err(|e| PyValueError::new_err(e.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(PyValueError::new_err("Replay URL must use HTTP or HTTPS"));
        }
        if chunk_size == 0 || chunk_size > 4 * 1024 * 1024 {
            return Err(PyValueError::new_err(
                "chunk_size must be between 1 and 4194304",
            ));
        }
        Ok(Self {
            native: Source::Http { url, chunk_size },
        })
    }
    /// Describe a recording with one complete JSON message on each line.
    ///
    /// Args:
    ///     path: The string or path-like filename containing the recorded messages.
    ///     bootstrap: Optional (request_name, response_bytes) pairs supplying startup
    ///         discovery responses. Names must match Protocol.bootstrap declarations.
    ///         These responses are applied before recorded messages; no discovery HTTP
    ///         requests are made for this source. Defaults to an empty collection.
    ///
    /// Returns:
    ///     A Source for streaming the recorded messages in file order.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     source = lm.Source.json_lines("updates.jsonl")
    ///     ```
    #[staticmethod]
    #[pyo3(signature=(path,*,bootstrap=Vec::new()))]
    fn json_lines(
        py: Python<'_>,
        path: std::path::PathBuf,
        bootstrap: Vec<(String, Py<pyo3::types::PyBytes>)>,
    ) -> Self {
        Self {
            native: Source::JsonLines {
                path,
                bootstrap: bootstrap
                    .into_iter()
                    .map(|(name, bytes)| (name, bytes.bind(py).as_bytes().to_vec()))
                    .collect(),
            },
        }
    }
    /// Describe a live WebSocket source.
    ///
    /// Args:
    ///     endpoint: The ws:// or wss:// URL used for the live connection. None defers
    ///         to an endpoint already provided by the adapter descriptor; a custom
    ///         adapter constructed directly from Python should supply the URL here.
    ///     heartbeat: Optional TextHeartbeat for a server that sends plain text
    ///         control frames. The transport consumes matching replies before decoding.
    ///         Omit it for feeds whose heartbeat is expressed by Protocol.keepalive.
    ///
    /// Returns:
    ///     A Source that opens live connections when its adapter starts. The protocol's
    ///     discovery, handshake, subscription, and keepalive actions control the feed.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     source = lm.Source.websocket("wss://feed.example/ws")
    ///     ```
    #[staticmethod]
    #[pyo3(signature=(endpoint=None,*,heartbeat=None))]
    fn websocket(
        endpoint: Option<String>,
        heartbeat: Option<crate::custom::TextHeartbeat>,
    ) -> Self {
        Self {
            native: Source::WebSocket {
                endpoint,
                heartbeat,
            },
        }
    }
    /// Describe a finite sequence of raw messages already available in memory.
    ///
    /// Args:
    ///     packets: A sequence of bytes objects containing complete JSON messages or
    ///         chunks of a length-prefixed binary stream, in arrival order. Python
    ///         supplies bytes; Protocol describes how they become book operations.
    ///     bootstrap: Optional (request_name, response_bytes) pairs for the protocol's
    ///         startup discovery requests, applied before packets. Defaults to none.
    ///
    /// Returns:
    ///     A finite Source useful for examples, recordings, and deterministic tests.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     source = lm.Source.packets([b'{"type":"heartbeat"}'])
    ///     ```
    #[staticmethod]
    #[pyo3(signature=(packets,*,bootstrap=Vec::new()))]
    fn packets(
        py: Python<'_>,
        packets: Vec<Py<pyo3::types::PyBytes>>,
        bootstrap: Vec<(String, Py<pyo3::types::PyBytes>)>,
    ) -> Self {
        Self {
            native: Source::Packets {
                packets: packets
                    .into_iter()
                    .map(|bytes| bytes.bind(py).as_bytes().to_vec())
                    .collect(),
                bootstrap: bootstrap
                    .into_iter()
                    .map(|(name, bytes)| (name, bytes.bind(py).as_bytes().to_vec()))
                    .collect(),
            },
        }
    }
    #[getter]
    fn _source_spec(&self) -> PyResult<String> {
        serde_json::to_string(&self.native).map_err(error)
    }
}

/// Instrument metadata retained for dispatching a complete file replay.
#[derive(Clone)]
pub struct Instrument {
    /// The normalized symbol used for routing.
    pub symbol: String,
    /// The number of fractional price digits.
    pub price_decimals: u8,
    /// The number of fractional quantity digits.
    pub quantity_decimals: u8,
    /// The book policy selected before consuming orders.
    pub policy: BookPolicy,
}

/// Connect a user-defined Protocol to a source and expose its books.
///
/// The declaration is prepared during construction. Use start() for a streaming
/// session and run() for a complete local binary replay. A context manager closes
/// the session and drains attached sinks on exit; entering it alone does not start
/// reading. To expose the books in the app, pass this adapter to server_context.
///
/// Args:
///     protocol: A Protocol containing message layouts, actions, and lifecycle rules.
///     source: A Source describing a file, HTTP stream, WebSocket, or recorded packets.
///     instruments: An optional iterable or generator of Instrument objects. It is
///         consumed once during construction. Omit it when Register actions discover
///         instruments from the source.
///     symbol: The initially selected ticker. Defaults to "BOOK" and is normalized
///         like an Instrument symbol. It must be inside scope when scope is supplied.
///     scope: An optional list of ticker symbols whose books may be created. None
///         allows all discovered symbols. Scope restricts book construction, while
///         symbol chooses which permitted book is currently displayed.
///     name: The source name shown in the app. Defaults to "Custom adapter".
///     mode: FeedMode.Live for an ongoing feed or FeedMode.Replay for recorded data.
///         Defaults to FeedMode.Live.
///     level: BookLevel.L1 for best quotes, BookLevel.L2 for aggregate levels, or
///         BookLevel.L3 for individual orders. Defaults to BookLevel.L3.
///         L2 supports market previews; L3 additionally supports queue views and
///         alternate timelines for simulated resting limit orders.
///     timezone: The IANA timezone used to interpret recorded source times. Defaults
///         to "UTC". For a date-named file with intraday timestamps, use its exchange's
///         timezone, such as "America/New_York".
///     supports_trades: Whether actions report actual executions. Defaults to True.
///         Set False for a source that supplies quantity updates without trade data.
///
/// Raises:
///     ValueError: The declaration cannot be prepared, metadata is invalid, or
///         the selected symbol is outside scope.
///     RuntimeError: The source cannot be prepared or its directory cannot be read.
///     TypeError: The source, instruments, or declaration arguments have invalid types.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import CustomAdapter, Protocol
///     from lobo.replay.adapters import expressions as le
///     from lobo.replay.adapters import models as lm
///
///     protocol = Protocol(lm.Json(lm.Message(le.Field("type").eq("add"),
///         lm.Book("BOOK", lm.Add(id=le.Field("id"), side=le.Field("side"),
///             price=le.Field("price"), quantity=le.Field("quantity")), snapshot=True))))
///     source = lm.Source.packets([
///         b'{"type":"add","id":1,"side":"buy","price":10000,"quantity":20}'
///     ])
///     with CustomAdapter(protocol, source,
///                        instruments=[lm.Instrument("BOOK", 2, 0)]) as adapter:
///         adapter.start()
///         adapter.wait()
///         assert adapter.levels("BOOK")[0]["quantity"] == 20
///     ```
#[pyclass(name = "CustomAdapter", module = "lobo.replay.adapters.custom")]
pub struct PyCustomAdapter {
    definition: Arc<DeclaredAdapter>,
    source: Option<Source>,
    symbol: String,
    scope: BookScope,
    instruments: Vec<Instrument>,
    session: Option<Session>,
    sinks: Option<SinkSession>,
    result: Option<PythonReplayResult>,
}
impl PyCustomAdapter {
    fn new(definition: DeclaredAdapter) -> Self {
        let definition = Arc::new(definition);
        let symbol = definition.info().default_symbol;
        Self {
            definition,
            source: None,
            symbol,
            scope: BookScope::All,
            instruments: Vec::new(),
            session: None,
            sinks: None,
            result: None,
        }
    }
    fn with_source(mut self, source: Source) -> Result<Self, String> {
        self.definition.prepare(&source)?;
        self.source = Some(source);
        Ok(self)
    }
    pub(super) fn active(&self) -> PyResult<&Session> {
        self.session
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Start the feed first"))
    }
}
impl PyCustomAdapter {
    fn start_with_publisher(
        &mut self,
        py: Python<'_>,
        observer: Option<crate::custom::observer::channel::Publisher>,
    ) -> PyResult<()> {
        if self.session.is_some() || self.result.is_some() {
            return Err(PyRuntimeError::new_err("Feed is already started"));
        }
        let source = self
            .source
            .clone()
            .ok_or_else(|| PyValueError::new_err("Configure a Source first"))?;
        let definition = self.definition.clone();
        let symbol = self.symbol.clone();
        let scope = self.scope.clone();
        let instruments = self.instruments.clone();
        self.session = Some(
            py.detach(move || {
                Session::spawn_boxed(
                    move || {
                        let mut adapter = match observer {
                            Some(publisher) => definition.create_observed(&symbol, publisher)?,
                            None => definition.create(&symbol)?,
                        };
                        adapter.set_book_scope(scope)?;
                        for instrument in instruments {
                            adapter.register_instrument(
                                crate::feed::Instrument {
                                    symbol: instrument.symbol.clone(),
                                    price_decimals: instrument.price_decimals,
                                    quantity_decimals: instrument.quantity_decimals,
                                },
                                instrument.policy,
                            )?;
                        }
                        Ok(adapter)
                    },
                    Box::new(source),
                )
            })
            .map_err(error)?,
        );
        Ok(())
    }
    /// Start a session with an observer publisher for the server.
    ///
    /// The server uses the returned session handle to route commands to these books.
    ///
    /// Args:
    ///     py: The Python token used to detach while starting the session.
    ///     capacity: The observer channel capacity for queued completed updates.
    ///
    /// Returns:
    ///     The session handle, observer publisher, and descriptor used by the server.
    ///
    /// Errors:
    ///     Returns an error if the source or session cannot be started.
    pub fn attach(
        &mut self,
        py: Python<'_>,
        capacity: usize,
    ) -> PyResult<crate::custom::observer::HostedAdapter> {
        let publisher = crate::custom::observer::channel::Publisher::new(capacity);
        self.start_with_publisher(py, Some(publisher.clone()))?;
        Ok(crate::custom::observer::HostedAdapter {
            session: self.active()?.handle(),
            publisher,
            descriptor: self.definition.info(),
            observer_definition: json!({
                "format": {"format": "json", "messages": []}, "connect": [], "subscriptions": [], "bootstrap": [],
                "reconcile_window_ns": self.definition.plan.reconcile_window_ns,
                "reconcile_capacity": self.definition.plan.reconcile_capacity,
                "simulation_note": self.definition.plan.simulation_note,
                "symbols_per_connection": 0,
            }),
        })
    }
    /// Close a server-attached adapter using the same cleanup as Python close().
    pub fn close_hosted(&mut self, py: Python<'_>) -> PyResult<()> {
        self.close(py)
    }
}

#[pymethods]
impl PyCustomAdapter {
    #[new]
    #[pyo3(signature=(protocol,source,*,instruments=None,symbol="BOOK",scope=None,name="Custom adapter",mode=FeedMode::Live,level=BookLevel::L3,timezone="UTC",supports_trades=true))]
    fn construct(
        py: Python<'_>,
        protocol: PyRef<'_, crate::custom::definition::schema::Definition>,
        source: PyRef<'_, PySource>,
        instruments: Option<super::Iterable<super::PyInstrument>>,
        symbol: &str,
        scope: Option<Vec<String>>,
        name: &str,
        mode: FeedMode,
        level: BookLevel,
        timezone: &str,
        supports_trades: bool,
    ) -> PyResult<Self> {
        let definition = Arc::new(protocol.clone());
        let info = super::PyAdapterInfo::new(
            "custom".into(),
            name.into(),
            mode,
            level,
            symbol,
            None,
            timezone,
            supports_trades,
        )?;
        custom_adapter(
            py,
            definition,
            source,
            &info,
            Some(symbol.into()),
            scope,
            instruments,
        )
    }
    /// A dictionary describing the adapter's identity, source mode, and capabilities.
    ///
    /// Keys are id, name, mode, level, endpoint, default_symbol, timezone, and
    /// supports_trades. Mode and level are FeedMode and BookLevel enum values.
    /// This property is available before start().
    #[getter]
    fn info(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let i = self.definition.info();
        let info = PyDict::new(py);
        info.set_item("id", i.id)?;
        info.set_item("name", i.name)?;
        info.set_item("mode", i.mode)?;
        info.set_item("level", i.level)?;
        info.set_item("endpoint", i.endpoint)?;
        info.set_item("default_symbol", i.default_symbol)?;
        info.set_item("timezone", i.timezone)?;
        info.set_item("supports_trades", i.supports_trades)?;
        Ok(info.into_any().unbind())
    }
    /// Start consuming the source on the adapter's background session.
    ///
    /// The method returns after starting the worker; it does not wait for replay or
    /// live discovery to finish. Use wait() for finite input and close() or a context
    /// manager to stop a live source. A session already started is left running.
    ///
    /// Raises:
    ///     RuntimeError: The source cannot be prepared or a worker cannot be started.
    fn start(&mut self, py: Python<'_>) -> PyResult<()> {
        self.start_with_publisher(py, None)
    }
    /// Replay a complete local binary file into books, optionally publishing to sinks.
    ///
    /// This call waits for replay completion while allowing other Python threads to
    /// run. Create a fresh adapter for each run. Use a context manager so sink writers
    /// finish and release their publishers when the adapter leaves scope.
    ///
    /// Args:
    ///     concurrent: Whether books replay concurrently using the existing worker
    ///         pool. Defaults to True. False uses the sequential replay path.
    ///     collect_stats: Whether to collect replay timing and count statistics,
    ///         available through stats. Defaults to False.
    ///     cutoff_time: Optional timezone-aware datetime at which replay stops. None
    ///         processes the full session; naive datetime values are rejected.
    ///     sinks: Optional sequence of FeatherSink instances receiving price-level
    ///         events during replay. Defaults to no event sinks.
    ///
    /// Returns:
    ///     A dictionary mapping ticker symbols to their completed book objects.
    ///
    /// Raises:
    ///     RuntimeError: The adapter has already started, the source is not a
    ///         supported local binary file, or replay encounters an input error.
    ///     ValueError: cutoff_time has no timezone.
    ///
    /// Examples:
    ///     ```python
    ///     # With a declared binary protocol and its local Source:
    ///     # with CustomAdapter(protocol, source, symbol="AAPL", mode="replay") as adapter:
    ///     #     books = adapter.run(concurrent=False)
    ///     ```
    #[pyo3(signature=(*,concurrent=true,collect_stats=false,cutoff_time=None,sinks=None))]
    fn run(
        &mut self,
        py: Python<'_>,
        concurrent: bool,
        collect_stats: bool,
        cutoff_time: Option<&Bound<'_, PyAny>>,
        sinks: Option<Vec<Py<PyFeatherSink>>>,
    ) -> PyResult<Py<PyAny>> {
        if self.result.is_some() || self.session.is_some() {
            return Err(PyRuntimeError::new_err(
                "Use a fresh adapter for each replay",
            ));
        }
        let cutoff = match cutoff_time {
            Some(value) => {
                if value.call_method0("utcoffset")?.is_none() {
                    return Err(PyValueError::new_err("cutoff_time must be timezone-aware"));
                }
                let iso: String = value.call_method0("isoformat")?.extract()?;
                Some(
                    lobo_primitives::time::DateTime::parse_from_rfc3339(&iso)
                        .map_err(error)?
                        .with_timezone(&lobo_primitives::time::Utc),
                )
            }
            None => None,
        };
        let sink_session = SinkSession::connect(py, sinks.unwrap_or_default())?;
        let options = ReplayOptions {
            instruments: self.instruments.clone(),
            scope: self.scope.clone(),
            concurrent,
            collect_stats,
            cutoff,
            publisher: sink_session.publisher(),
        };
        let definition = self.definition.clone();
        let output = match py.detach(move || definition.replay(options)) {
            Ok(output) => output,
            Err(e) => {
                sink_session.close(py)?;
                return Err(error(e));
            }
        };
        let output = match output.into_python(py) {
            Ok(output) => output,
            Err(e) => {
                sink_session.close(py)?;
                return Err(e);
            }
        };
        let value = output.value.clone_ref(py);
        self.sinks = Some(sink_session);
        self.result = Some(output);
        Ok(value)
    }
    /// Wait for a started finite source to finish, releasing Python while waiting.
    ///
    /// A live WebSocket normally runs until it is closed; waiting is therefore mainly
    /// useful for files and packet recordings.
    ///
    /// Raises:
    ///     RuntimeError: start() has not created a session, or the worker reports
    ///         an input or protocol error.
    fn wait(&self, py: Python<'_>) -> PyResult<()> {
        let session = self.active()?;
        py.detach(|| session.wait()).map_err(error)
    }
    /// Replay statistics from run(), or None before a bulk replay completes.
    ///
    /// With collect_stats=True, the dictionary includes counts and timing measurements;
    /// with False it contains only the result supplied by the replay path.
    #[getter]
    fn stats(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        to_python(
            py,
            self.result
                .as_ref()
                .map_or(Value::Null, |r| r.stats.clone()),
        )
    }
    /// Whether the session created by start() has stopped consuming its source.
    ///
    /// Use wait() to surface an error from a completed worker. Access before start()
    /// raises RuntimeError. This property does not describe the separate run() result.
    #[getter]
    fn finished(&self) -> PyResult<bool> {
        Ok(self.active()?.finished())
    }
    /// List the symbols discovered or explicitly registered by this adapter.
    ///
    /// For a running session, read the current directory. Before start(), a local
    /// binary source can scan its directory records; explicit instruments are merged
    /// into the result. Live discovery requires starting the source first.
    ///
    /// Returns:
    ///     A list of normalized symbols, suitable for ticker selection and autocomplete.
    ///
    /// Raises:
    ///     RuntimeError: Directory discovery encounters an input or protocol error.
    fn tickers(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        if let Some(session) = &self.session {
            return py
                .detach(|| session.with(|a| Ok(a.tickers())))
                .map_err(error);
        }
        let definition = self.definition.clone();
        let mut tickers = py
            .detach(move || definition.instruments())
            .map_err(error)?
            .into_iter()
            .map(|i| i.symbol)
            .collect::<Vec<_>>();
        tickers.extend(self.instruments.iter().map(|i| i.symbol.clone()));
        tickers.sort_unstable();
        tickers.dedup();
        Ok(tickers)
    }
    /// Stop the background session and finish any attached sink writers.
    ///
    /// The method also disconnects publishers retained by bulk replay result books.
    /// Repeated calls are safe. Context-manager exit calls this method automatically.
    ///
    /// Raises:
    ///     RuntimeError: Disconnecting a publisher or closing a sink fails.
    fn close(&mut self, py: Python<'_>) -> PyResult<()> {
        if let Some(mut session) = self.session.take() {
            py.detach(|| session.close());
        }
        if let Some(result) = self.result.take() {
            (result.disconnect)(py)?;
        }
        if let Some(session) = self.sinks.take() {
            session.close(py)?;
        }
        Ok(())
    }
    /// Enter a scope that closes this adapter automatically on exit.
    ///
    /// Returns:
    ///     This adapter. Call start() or run() explicitly inside the scope.
    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }
    /// Close the adapter and attached sinks when leaving its context.
    ///
    /// Args:
    ///     _ty: The exception type supplied by Python, or None on normal exit.
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
    ) -> PyResult<()> {
        self.close(py)
    }
}

#[cfg(feature = "python-polars")]
#[pymethods]
impl PyCustomAdapter {
    /// Read named binary fields into a Polars LazyFrame for one instrument.
    ///
    /// Args:
    ///     symbol: The ticker to extract. None uses the initially selected ticker.
    ///         The symbol must be inside the adapter's book scope.
    ///
    /// Returns:
    ///     A Polars LazyFrame containing the declared record fields for that ticker.
    ///     This is a data extraction operation, rather than a replay into books.
    ///
    /// Raises:
    ///     ValueError: symbol is invalid or outside scope.
    ///     RuntimeError: The source or format cannot supply rows.
    #[pyo3(signature=(symbol=None))]
    fn to_lazy_frame(
        &self,
        py: Python<'_>,
        symbol: Option<String>,
    ) -> PyResult<pyo3_polars::PyLazyFrame> {
        let definition = self.definition.clone();
        let symbol = crate::feed::normalize_symbol(symbol.as_deref().unwrap_or(&self.symbol))
            .map_err(error)?;
        if !self.scope.contains(&symbol) {
            return Err(PyValueError::new_err("Ticker is outside the book scope"));
        }
        let scope = BookScope::Selected([symbol].into());
        py.detach(move || definition.rows(scope))
            .map(pyo3_polars::PyLazyFrame)
            .map_err(error)
    }
}

/// Construct an adapter directly from a user's protocol declaration.
fn custom_adapter(
    py: Python<'_>,
    plan: Arc<crate::custom::definition::schema::Definition>,
    source: PyRef<'_, PySource>,
    info: &super::PyAdapterInfo,
    symbol: Option<String>,
    scope: Option<Vec<String>>,
    instruments: Option<super::Iterable<super::PyInstrument>>,
) -> PyResult<PyCustomAdapter> {
    let mut descriptor = info.descriptor.clone();
    if let Source::WebSocket {
        endpoint: Some(endpoint),
        ..
    } = &source.native
    {
        descriptor.endpoint = Some(endpoint.clone());
    }
    let plan = py
        .detach(|| crate::custom::definition::jit::prepare_protocol(plan, descriptor.mode))
        .map_err(PyValueError::new_err)?;
    let definition = DeclaredAdapter {
        descriptor,
        plan,
        prepared: Mutex::new(None),
    };
    let mut adapter = PyCustomAdapter::new(definition);
    if let Some(symbol) = symbol {
        adapter.symbol = crate::feed::normalize_symbol(&symbol).map_err(PyValueError::new_err)?;
    }
    adapter.scope = match scope {
        Some(symbols) => BookScope::Selected(
            symbols
                .into_iter()
                .map(|s| crate::feed::normalize_symbol(&s))
                .collect::<Result<_, _>>()
                .map_err(PyValueError::new_err)?,
        ),
        None => BookScope::All,
    };
    if !adapter.scope.contains(&adapter.symbol) {
        return Err(PyValueError::new_err(
            "Selected symbol must belong to the book scope",
        ));
    }
    if let Some(instruments) = instruments {
        for item in instruments.0 {
            adapter.instruments.push(Instrument {
                symbol: item.native.symbol.clone(),
                price_decimals: item.native.price_decimals,
                quantity_decimals: item.native.quantity_decimals,
                policy: item.policy,
            });
        }
    }
    let source = source.native.clone();
    py.detach(move || adapter.with_source(source))
        .map_err(error)
}
struct DeclaredAdapter {
    descriptor: AdapterDescriptor,
    plan: Arc<crate::custom::definition::schema::Definition>,
    prepared: Mutex<
        Option<(
            crate::custom::definition::binary::FileFormat,
            Vec<crate::custom::definition::binary::DirectoryEntry>,
        )>,
    >,
}
impl DeclaredAdapter {
    fn prepare(&self, source: &Source) -> Result<(), String> {
        if let Source::File { path, .. } = source {
            // Some declarations use compound records. They remain available to
            // the incremental runner; preparing a bulk plan is an optimization.
            if let Ok(format) = crate::custom::definition::binary::FileFormat::new(
                path.clone(),
                &self.plan,
                &self.descriptor.timezone,
            ) {
                let directory = format.directory(self.plan.clone(), self.descriptor.clone())?;
                *self.prepared.lock().map_err(|e| e.to_string())? = Some((format, directory));
            }
        }
        Ok(())
    }
    fn instruments(&self) -> Result<Vec<crate::feed::Instrument>, String> {
        let prepared = self.prepared.lock().map_err(|e| e.to_string())?;
        Ok(prepared
            .as_ref()
            .map(|(_, entries)| entries.iter().map(|e| e.instrument.clone()).collect())
            .unwrap_or_default())
    }
    #[cfg(feature = "python-polars")]
    fn rows(&self, scope: BookScope) -> Result<polars::prelude::LazyFrame, String> {
        let prepared = self.prepared.lock().map_err(|e| e.to_string())?;
        let (format, directory) = prepared
            .as_ref()
            .ok_or("Tabular conversion requires a binary file source")?;
        format.table(
            &directory
                .iter()
                .filter(|entry| scope.contains(&entry.instrument.symbol))
                .map(|entry| entry.key)
                .collect(),
        )
    }
    fn replay(&self, options: ReplayOptions) -> Result<Box<dyn ReplayOutput>, String> {
        let (format, directory) = self
            .prepared
            .lock()
            .map_err(|e| e.to_string())?
            .clone()
            .ok_or("This definition has no bulk file plan; use start() for streaming execution")?;
        if let Some(error) = &format.bulk_error {
            return Err(error.clone());
        }
        let mut groups: Vec<(
            BookPolicy,
            Vec<
                crate::custom::source::CustomReplaySource<
                    crate::custom::definition::binary::FileFormat,
                >,
            >,
        )> = Vec::new();
        for entry in directory {
            if !options.scope.contains(&entry.instrument.symbol) {
                continue;
            }
            let policy = options
                .instruments
                .iter()
                .find(|i| i.symbol == entry.instrument.symbol)
                .map_or(entry.policy, |i| i.policy);
            let index = match groups.iter().position(|(p, _)| *p == policy) {
                Some(index) => index,
                None => {
                    groups.push((policy, Vec::new()));
                    groups.len() - 1
                }
            };
            groups[index]
                .1
                .push(crate::custom::source::CustomReplaySource::new(
                    format.clone(),
                    entry.key,
                    entry.instrument.symbol,
                ));
        }
        if groups.is_empty() {
            return Err("No instruments match the book scope".into());
        }
        let mut outputs = Vec::with_capacity(groups.len());
        for (policy, sources) in groups {
            outputs.push(super::bulk::replay(options.clone(), sources, policy)?);
        }
        Ok(Box::new(super::bulk::Combined(outputs)))
    }
    fn info(&self) -> AdapterDescriptor {
        self.descriptor.clone()
    }
    fn create_observed(
        &self,
        symbol: &str,
        publisher: crate::custom::observer::channel::Publisher,
    ) -> Result<Box<dyn MarketDataAdapter>, String> {
        let protocol = crate::custom::definition::DefinedProtocol::with_observer(
            self.plan.clone(),
            self.descriptor.clone(),
            symbol,
            publisher.sink(),
        )?;
        Ok(Box::new(crate::custom::CustomAdapter::new(
            self.descriptor.clone(),
            protocol,
            symbol,
        )?))
    }
    fn create(&self, symbol: &str) -> Result<Box<dyn MarketDataAdapter>, String> {
        let protocol = crate::custom::definition::DefinedProtocol::new(
            self.plan.clone(),
            self.descriptor.clone(),
            symbol,
        )?;
        Ok(Box::new(crate::custom::CustomAdapter::new(
            self.descriptor.clone(),
            protocol,
            symbol,
        )?))
    }
}
pub(super) fn error(e: impl std::fmt::Display) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}
pub(super) fn to_python(py: Python<'_>, value: Value) -> PyResult<Py<PyAny>> {
    use pyo3::IntoPyObjectExt;
    match value {
        Value::Null => Ok(py.None()),
        Value::Bool(v) => v.into_py_any(py),
        Value::String(v) => v.into_py_any(py),
        Value::Number(v) => {
            if let Some(n) = v.as_u64() {
                n.into_py_any(py)
            } else if let Some(n) = v.as_i64() {
                n.into_py_any(py)
            } else {
                v.as_f64().into_py_any(py)
            }
        }
        Value::Array(values) => {
            let list = PyList::empty(py);
            for v in values {
                list.append(to_python(py, v)?)?;
            }
            Ok(list.into_any().unbind())
        }
        Value::Object(values) => {
            let dict = PyDict::new(py);
            for (k, v) in values {
                dict.set_item(k, to_python(py, v)?)?;
            }
            Ok(dict.into_any().unbind())
        }
    }
}

impl PySource {
    /// Clone the transport configuration for an adapter that owns its decoder.
    pub fn transport(&self) -> Source {
        self.native.clone()
    }
}
