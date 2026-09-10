//! Source workers, lifecycle controls, and serialized book access.
use super::{FeedMode, MarketDataAdapter};
use std::{
    cell::Cell,
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, mpsc},
    thread::JoinHandle,
};

type Query = Box<dyn FnOnce(&mut dyn MarketDataAdapter) + Send>;
pub(crate) enum Control {
    Query(Query),
    Stop,
}

/// Input transports. Additional transports implement `Driver`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    File {
        path: PathBuf,
        chunk_size: usize,
    },
    /// Stream a plain or gzip-compressed replay over HTTP(S), without a local file.
    Http {
        url: String,
        chunk_size: usize,
    },
    JsonLines {
        path: PathBuf,
        bootstrap: Vec<(String, Vec<u8>)>,
    },
    WebSocket {
        endpoint: Option<String>,
        /// Optional application-level text control frames, handled by the transport.
        #[serde(default)]
        heartbeat: Option<super::TextHeartbeat>,
    },
    /// Preloaded wire packets, useful for captured feeds and deterministic tests.
    Packets {
        packets: Vec<Vec<u8>>,
        bootstrap: Vec<(String, Vec<u8>)>,
    },
}

/// A source owns its IO loop and invokes the adapter. This trait
/// also permits application-specific APIs without adding them to the library.
pub trait Driver: Send + 'static {
    fn run(
        self: Box<Self>,
        adapter: &mut dyn MarketDataAdapter,
        controls: &ControlReceiver,
    ) -> Result<(), String>;
}
pub struct ControlReceiver {
    pub(crate) rx: mpsc::Receiver<Control>,
    stopped: Cell<bool>,
}
impl ControlReceiver {
    /// Service controls between input batches, never inside a book mutation.
    pub fn poll(&self, adapter: &mut dyn MarketDataAdapter) -> bool {
        if self.stopped.get() {
            return false;
        }
        loop {
            match self.rx.try_recv() {
                Ok(Control::Query(query)) => query(adapter),
                Ok(Control::Stop) | Err(mpsc::TryRecvError::Disconnected) => {
                    self.stopped.set(true);
                    return false;
                }
                Err(mpsc::TryRecvError::Empty) => return true,
            }
        }
    }
    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped.get()
    }
}

type Completion = Arc<(Mutex<Option<Result<(), String>>>, Condvar)>;
/// The feed and its non-Send frame buffers stay on one worker thread.
/// All handles and responses are Send; no lock is taken by book mutations.
pub struct Session {
    tx: mpsc::Sender<Control>,
    completion: Completion,
    thread: Option<JoinHandle<()>>,
}
#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::Sender<Control>,
}
impl SessionHandle {
    pub fn with<R: Send + 'static>(
        &self,
        query: impl FnOnce(&mut dyn MarketDataAdapter) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx
            .send(Control::Query(Box::new(move |adapter| {
                let _ = tx.send(query(adapter));
            })))
            .map_err(|_| "Adapter is closed")?;
        rx.recv().map_err(|_| "Adapter stopped during a request")?
    }
}
impl Session {
    pub fn handle(&self) -> SessionHandle {
        SessionHandle {
            tx: self.tx.clone(),
        }
    }
    pub fn spawn<F, A>(factory: F, source: impl Driver) -> Result<Self, String>
    where
        F: FnOnce() -> Result<A, String> + Send + 'static,
        A: MarketDataAdapter + 'static,
    {
        Self::spawn_boxed(
            move || factory().map(|a| Box::new(a) as Box<dyn MarketDataAdapter>),
            Box::new(source),
        )
    }
    pub fn spawn_boxed(
        factory: impl FnOnce() -> Result<Box<dyn MarketDataAdapter>, String> + Send + 'static,
        source: Box<dyn Driver>,
    ) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let completion: Completion = Arc::new((Mutex::new(None), Condvar::new()));
        let done = completion.clone();
        let thread = std::thread::Builder::new()
            .name("lobo-feed".into())
            .spawn(move || {
                let mut adapter = match factory() {
                    Ok(a) => a,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                let controls = ControlReceiver {
                    rx,
                    stopped: Cell::new(false),
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    source.run(adapter.as_mut(), &controls)
                }))
                .unwrap_or_else(|_| Err("Input worker panicked".into()));
                let (lock, notify) = &*done;
                *lock.lock().expect("Completion lock poisoned") = Some(result);
                notify.notify_all();
                // Finite replays retain their books for queries/simulation.
                while !controls.stopped.get() {
                    match controls.rx.recv() {
                        Ok(Control::Query(query)) => query(adapter.as_mut()),
                        _ => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        match ready_rx
            .recv()
            .map_err(|_| "Adapter initialization panicked".to_string())?
        {
            Ok(()) => Ok(Self {
                tx,
                completion,
                thread: Some(thread),
            }),
            Err(e) => {
                let _ = thread.join();
                Err(e)
            }
        }
    }
    pub fn with<R: Send + 'static>(
        &self,
        query: impl FnOnce(&mut dyn MarketDataAdapter) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        self.handle().with(query)
    }

    pub fn finished(&self) -> bool {
        self.completion
            .0
            .lock()
            .expect("Completion lock poisoned")
            .is_some()
    }
    pub fn wait(&self) -> Result<(), String> {
        let (lock, notify) = &*self.completion;
        let result = notify
            .wait_while(lock.lock().expect("Completion lock poisoned"), |r| {
                r.is_none()
            })
            .expect("Completion lock poisoned");
        result
            .as_ref()
            .expect("completion predicate was satisfied")
            .clone()
    }
    pub fn close(&mut self) {
        let _ = self.tx.send(Control::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

fn reader(path: &PathBuf) -> Result<Box<dyn Read + Send>, String> {
    let mut reader = BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    let gzip = reader
        .fill_buf()
        .map_err(|e| e.to_string())?
        .starts_with(&[0x1f, 0x8b]);
    if gzip {
        Ok(Box::new(flate2::read::MultiGzDecoder::new(reader)))
    } else {
        Ok(Box::new(reader))
    }
}
pub(super) fn advance(
    adapter: &mut dyn MarketDataAdapter,
    controls: &ControlReceiver,
) -> Result<bool, String> {
    if adapter.info().mode != FeedMode::Replay {
        return Ok(controls.poll(adapter));
    }
    while !adapter.state().needs_input && !adapter.state().complete {
        if !controls.poll(adapter) {
            return Ok(false);
        }
        adapter.advance(u64::MAX, 65_536)?;
        if adapter.buffered_bytes() == 0 {
            break;
        }
    }
    Ok(true)
}

/// Advance the feed clock between packets so delayed execution reconciliation
/// and time bars also complete while the network is quiet.
#[derive(Default)]
pub(crate) struct LiveClock {
    anchor: Option<(u64, std::time::Instant)>,
}
impl LiveClock {
    pub fn advance(&mut self, adapter: &mut dyn MarketDataAdapter) -> Result<(), String> {
        let Some(start) = adapter.state().start_ns else {
            return Ok(());
        };
        let now = std::time::Instant::now();
        let (clock, instant) = self.anchor.get_or_insert((adapter.state().clock_ns, now));
        let mut target = clock.saturating_add(now.duration_since(*instant).as_nanos() as u64);
        if adapter.state().clock_ns > target {
            *clock = adapter.state().clock_ns;
            *instant = now;
            target = *clock;
        }
        adapter.advance(target.saturating_sub(start), 65_536)?;
        let state = adapter.state_mut();
        state
            .volume_bars()
            .borrow_mut()
            .advance_time(state.clock_ns)
            .map_err(|e| e.to_string())?;
        if let Some(branch) = state.simulation.as_mut() {
            if !branch.stopped() {
                branch
                    .feed
                    .volume_bars()
                    .borrow_mut()
                    .advance_time(branch.feed.clock_ns)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}
impl Driver for Source {
    fn run(
        self: Box<Self>,
        adapter: &mut dyn MarketDataAdapter,
        controls: &ControlReceiver,
    ) -> Result<(), String> {
        match *self {
            Source::File { path, chunk_size } => {
                if chunk_size == 0 || chunk_size > 4 * 1024 * 1024 {
                    return Err("chunk_size must be between 1 and 4194304".into());
                }
                let mut reader = reader(&path)?;
                let mut buffer = vec![0; chunk_size];
                loop {
                    if !controls.poll(adapter) {
                        return Ok(());
                    }
                    let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
                    adapter.receive(&buffer[..count], count == 0)?;
                    if !advance(adapter, controls)? || count == 0 {
                        break;
                    }
                }
            }
            Source::JsonLines { path, bootstrap } => {
                let mut reader = BufReader::new(reader(&path)?);
                let mut bytes = Vec::new();
                for (id, bytes) in bootstrap {
                    adapter.bootstrap(&id, &bytes)?;
                }
                adapter.connected()?;
                loop {
                    if !controls.poll(adapter) {
                        return Ok(());
                    }
                    bytes.clear();
                    if reader
                        .read_until(b'\n', &mut bytes)
                        .map_err(|e| e.to_string())?
                        == 0
                    {
                        break;
                    }
                    if bytes.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    adapter.receive(&bytes, false)?;
                    if !advance(adapter, controls)? {
                        break;
                    }
                }
            }
            Source::Packets { packets, bootstrap } => {
                for (id, bytes) in bootstrap {
                    adapter.bootstrap(&id, &bytes)?;
                }
                adapter.connected()?;
                for bytes in packets {
                    if !controls.poll(adapter) {
                        return Ok(());
                    }
                    adapter.receive(&bytes, false)?;
                    if !advance(adapter, controls)? {
                        return Ok(());
                    }
                }
                if adapter.info().mode == FeedMode::Replay {
                    adapter.receive(&[], true)?;
                    advance(adapter, controls)?;
                }
            }
            Source::WebSocket { endpoint, heartbeat } => super::transport::run(adapter, controls, endpoint, heartbeat)?,
            Source::Http { url, chunk_size } => {
                super::http::run(adapter, controls, url, chunk_size)?
            }
        }
        Ok(())
    }
}
