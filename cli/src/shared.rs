//! A feed belongs to a session, not a terminal pane. Slow panes skip published
//! frames; they never hold the native feed worker or accumulate a frame backlog.
use crate::{
    app::App,
    args::{Args, BarKind, OrderKind, OrderSide, RendererKind, SourceKind, View},
    engine::{DepthFrame, Snapshot},
};
use anyhow::{Context, Result, anyhow, bail};
use lobo_models::server::Command;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::Shutdown,
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const MAX_PACKET: usize = 64 * 1024 * 1024;
const PROTOCOL: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub source: SourceKind,
    pub symbol: Option<String>,
    pub scope: String,
    pub paused: bool,
    pub speed: f64,
    pub aggregation: BarKind,
    pub bar_size: Option<u64>,
    pub history_seconds: f64,
    pub capacity: u32,
    pub side: OrderSide,
    pub queue_price: Option<f64>,
    pub order_kind: OrderKind,
    pub quantity: f64,
    pub price: Option<f64>,
    pub order_endpoint: Option<String>,
}
impl Config {
    fn from_args(a: &Args) -> Self {
        Self {
            source: a.source_kind(),
            symbol: a.symbol.clone(),
            scope: a.scope.clone(),
            paused: a.paused,
            speed: a.speed,
            aggregation: a.aggregation,
            bar_size: a.bar_size,
            history_seconds: a.history_seconds,
            capacity: a.capacity,
            side: a.side,
            queue_price: a.queue_price,
            order_kind: a.order_kind,
            quantity: a.quantity,
            price: a.price,
            order_endpoint: a.order_endpoint.clone(),
        }
    }
    pub fn apply(&self, a: &mut Args) {
        a.source = self.source;
        a.symbol = self.symbol.clone();
        a.scope = self.scope.clone();
        a.paused = self.paused;
        a.speed = self.speed;
        a.aggregation = self.aggregation;
        a.bar_size = self.bar_size;
        a.history_seconds = self.history_seconds;
        a.capacity = self.capacity;
        a.side = self.side;
        a.queue_price = self.queue_price;
        a.order_kind = self.order_kind;
        a.quantity = self.quantity;
        a.price = self.price;
        a.order_endpoint = self.order_endpoint.clone();
    }
}

struct Published {
    sequence: u64,
    revision: u64,
    config: Config,
    snapshot: Snapshot,
    depth: Vec<DepthFrame>,
    message: String,
}
#[derive(Serialize, Deserialize)]
struct WireFrame {
    protocol: u32,
    sequence: u64,
    revision: u64,
    config: Config,
    snapshot: Snapshot,
    reset_depth: bool,
    depth: Vec<DepthFrame>,
    message: String,
}
#[derive(Serialize, Deserialize)]
enum Action {
    Command(String),
    Submit {
        command: Command,
        symbol: String,
        revision: u64,
    },
}
#[derive(Serialize, Deserialize)]
struct Request {
    id: u64,
    action: Action,
}
#[derive(Serialize, Deserialize)]
struct Ack {
    id: u64,
    result: Result<String, String>,
}
#[derive(Serialize, Deserialize)]
enum Reply {
    Frame(Box<WireFrame>),
    Ack(Ack),
}
struct Pending {
    request: Request,
    reply: mpsc::SyncSender<Ack>,
}
type Publication = Arc<(Mutex<Option<Arc<Published>>>, Condvar)>;

fn write_packet<T: Serialize>(stream: &mut UnixStream, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_PACKET {
        bail!("session packet exceeds 64 MiB");
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)?;
    Ok(())
}
fn read_packet<T: for<'a> Deserialize<'a>>(stream: &mut UnixStream) -> Result<T> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_PACKET {
        bail!("session packet exceeds 64 MiB");
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Clone)]
pub struct Update {
    pub config: Config,
    pub snapshot: Snapshot,
    pub message: String,
}
#[derive(Default)]
struct ClientState {
    latest: Option<Update>,
    error: Option<String>,
}
type Responses = Arc<Mutex<BTreeMap<u64, mpsc::SyncSender<Result<String, String>>>>>;
pub struct Client {
    writer: Mutex<UnixStream>,
    state: Arc<Mutex<ClientState>>,
    responses: Responses,
    next_id: AtomicU64,
}
impl Client {
    pub fn connect(path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(path).with_context(|| {
            format!(
                "cannot attach to {}; start `lobo session --socket {}`",
                path.display(),
                path.display()
            )
        })?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut reader = stream.try_clone()?;
        let state = Arc::new(Mutex::new(ClientState::default()));
        let responses = Responses::default();
        let task_state = state.clone();
        let task_responses = responses.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                let mut depth = Vec::new();
                loop {
                    match read_packet(&mut reader)? {
                        Reply::Frame(frame) => {
                            if frame.protocol != PROTOCOL {
                                bail!("incompatible local session protocol");
                            }
                            if frame.reset_depth {
                                depth.clear();
                            }
                            depth.extend(frame.depth);
                            let excess = depth.len().saturating_sub(frame.config.capacity as usize);
                            depth.drain(..excess);
                            let mut snapshot = frame.snapshot;
                            snapshot.depth = depth.clone();
                            snapshot.session_sequence = Some(frame.sequence);
                            snapshot.session_revision = Some(frame.revision);
                            task_state.lock().unwrap().latest = Some(Update {
                                config: frame.config,
                                snapshot,
                                message: frame.message,
                            });
                        }
                        Reply::Ack(ack) => {
                            if let Some(tx) = task_responses.lock().unwrap().remove(&ack.id) {
                                let _ = tx.send(ack.result);
                            }
                        }
                    }
                }
            })();
            task_state.lock().unwrap().error =
                Some(format!("Session disconnected: {}", result.unwrap_err()));
            task_responses.lock().unwrap().clear();
        });
        let client = Self {
            writer: Mutex::new(stream),
            state,
            responses,
            next_id: AtomicU64::new(1),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let state = client.state.lock().unwrap();
            if state.latest.is_some() {
                break;
            }
            if let Some(e) = &state.error {
                bail!("{e}");
            }
            drop(state);
            if Instant::now() >= deadline {
                bail!("session did not publish its initial state within 5 seconds");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(client)
    }
    pub fn latest(&self, after: Option<u64>) -> (Option<Update>, Option<String>) {
        let state = self.state.lock().unwrap();
        (
            state
                .latest
                .as_ref()
                .filter(|f| f.snapshot.session_sequence != after)
                .cloned(),
            state.error.clone(),
        )
    }
    fn request(&self, action: Action) -> Result<String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(1);
        self.responses.lock().unwrap().insert(id, tx);
        let result = (|| {
            write_packet(&mut self.writer.lock().unwrap(), &Request { id, action })?;
            rx.recv_timeout(Duration::from_secs(5))
                .context("session response unavailable; command was not retried")?
                .map_err(|e| anyhow!(e))
        })();
        self.responses.lock().unwrap().remove(&id);
        result
    }
    pub fn command(&self, text: &str) -> Result<String> {
        self.request(Action::Command(text.into()))
    }
    pub fn submit(&self, command: Command, displayed: &Snapshot) -> Result<String> {
        self.request(Action::Submit {
            command,
            symbol: displayed.symbol.clone(),
            revision: displayed
                .session_revision
                .context("wait for the session snapshot before entering an order")?,
        })
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.writer.lock().unwrap().shutdown(Shutdown::Both);
    }
}

fn serve_client(
    stream: UnixStream,
    publication: Publication,
    requests: mpsc::SyncSender<Pending>,
    stop: Arc<AtomicBool>,
    clients: Arc<AtomicUsize>,
) -> Result<()> {
    // Darwin can inherit O_NONBLOCK from the listening socket.
    stream.set_nonblocking(false)?;
    let mut reader = stream.try_clone()?;
    let mut writer = stream;
    writer.set_write_timeout(Some(Duration::from_secs(2)))?;
    let (tx, rx) = mpsc::sync_channel(8);
    std::thread::spawn(move || {
        while let Ok(request) = read_packet(&mut reader) {
            if requests
                .send(Pending {
                    request,
                    reply: tx.clone(),
                })
                .is_err()
            {
                break;
            }
        }
        let _ = reader.shutdown(Shutdown::Both);
    });
    std::thread::spawn(move || {
        let result = (|| -> Result<()> {
            let mut sent = 0;
            let mut revision = u64::MAX;
            let mut last_depth = None;
            loop {
                for ack in rx.try_iter() {
                    write_packet(&mut writer, &Reply::Ack(ack))?;
                }
                if stop.load(Ordering::Acquire) {
                    // A requested stop is published only after its acknowledgement
                    // has been queued. Drain again to cover the race with the first drain.
                    for ack in rx.try_iter() {
                        write_packet(&mut writer, &Reply::Ack(ack))?;
                    }
                    break;
                }
                let (lock, wake) = &*publication;
                let guard = lock.lock().unwrap();
                let (guard, _) = wake
                    .wait_timeout_while(guard, Duration::from_millis(10), |f| {
                        f.as_ref().is_none_or(|f| f.sequence == sent)
                    })
                    .unwrap();
                let frame = guard.clone();
                drop(guard);
                let Some(frame) = frame.filter(|f| f.sequence != sent) else {
                    continue;
                };
                let start = if revision == frame.revision {
                    last_depth
                        .and_then(|timestamp| {
                            frame.depth.iter().position(|f| f.clock_ns == timestamp)
                        })
                        .map(|i| i + 1)
                } else {
                    None
                };
                let wire = WireFrame {
                    protocol: PROTOCOL,
                    sequence: frame.sequence,
                    revision: frame.revision,
                    config: frame.config.clone(),
                    snapshot: frame.snapshot.clone(),
                    reset_depth: start.is_none(),
                    depth: frame.depth[start.unwrap_or(0)..].to_vec(),
                    message: frame.message.clone(),
                };
                write_packet(&mut writer, &Reply::Frame(Box::new(wire)))?;
                sent = frame.sequence;
                revision = frame.revision;
                last_depth = frame.depth.last().map(|f| f.clock_ns);
            }
            Ok(())
        })();
        let _ = result; // A pane disconnect is local; it does not stop the feed.
        let _ = writer.shutdown(Shutdown::Both);
        clients.fetch_sub(1, Ordering::Relaxed);
    });
    Ok(())
}

struct SocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.path)
            && metadata.file_type().is_socket()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn run(args: &Args) -> Result<()> {
    let path = args
        .socket
        .as_ref()
        .context("session requires --socket PATH")?;
    // Only stale sockets may be replaced. Never remove a user's regular file or symlink.
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() {
            bail!("socket path already exists and is not a socket");
        }
        match UnixStream::connect(path) {
            Ok(_) => bail!("session already running at {}", path.display()),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                ) => {}
            Err(e) => return Err(e).context("cannot check the existing session socket"),
        }
        fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    let metadata = fs::symlink_metadata(path)?;
    let _socket = SocketGuard {
        path: path.clone(),
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let stop = Arc::new(AtomicBool::new(false));
    let sigint = signal_hook::flag::register(signal_hook::consts::SIGINT, stop.clone())?;
    let sigterm = signal_hook::flag::register(signal_hook::consts::SIGTERM, stop.clone())?;
    let result = run_session(args, listener, stop.clone());
    stop.store(true, Ordering::Release);
    signal_hook::low_level::unregister(sigint);
    signal_hook::low_level::unregister(sigterm);
    result
}
fn run_session(args: &Args, listener: UnixListener, stop: Arc<AtomicBool>) -> Result<()> {
    let mut local = args.clone();
    local.command = Some(View::Dashboard);
    local.socket = None;
    local.renderer = RendererKind::Cpu;
    local.theme = "Acid Lime".into();
    local.theme_file = None;
    let mut app = App::new(local)?;
    let publication: Publication = Arc::new((Mutex::new(None), Condvar::new()));
    let clients = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::sync_channel::<Pending>(64);
    let period = Duration::from_secs_f64(1.0 / f64::from(args.fps));
    let mut next = Instant::now();
    let mut sequence = 0;
    let mut revision = 0;
    println!(
        "LOBO session ready · {} · one feed / clock / simulation",
        args.socket.as_ref().unwrap().display()
    );
    std::io::stdout().flush()?;
    while !stop.load(Ordering::Relaxed) {
        for _ in 0..32 {
            match listener.accept() {
                Ok((stream, _)) if clients.load(Ordering::Relaxed) < 32 => {
                    clients.fetch_add(1, Ordering::Relaxed);
                    if serve_client(
                        stream,
                        publication.clone(),
                        tx.clone(),
                        stop.clone(),
                        clients.clone(),
                    )
                    .is_err()
                    {
                        clients.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        for pending in rx.try_iter().take(64) {
            let stopping =
                matches!(&pending.request.action, Action::Command(text) if text == "stop");
            let result = match pending.request.action {
                Action::Command(text) if text == "stop" => Ok("Session stopped".into()),
                Action::Command(text) => app.command(&text).map(|()| app.message.clone()),
                Action::Submit {
                    command,
                    symbol,
                    revision: expected,
                } => {
                    if expected != revision || symbol != app.snapshot.symbol {
                        Err(anyhow!(
                            "session context changed; wait for the pane to refresh before entering the order"
                        ))
                    } else {
                        app.submit_order(command)
                    }
                }
            };
            if result.is_ok() {
                revision += 1;
            }
            let _ = pending.reply.try_send(Ack {
                id: pending.request.id,
                result: result.map_err(|e| e.to_string()),
            });
            if stopping {
                stop.store(true, Ordering::Release);
                break;
            }
        }
        if Instant::now() >= next {
            app.refresh()?;
            sequence += 1;
            let mut snapshot = app.snapshot.clone();
            snapshot.session_sequence = Some(sequence);
            snapshot.session_revision = Some(revision);
            let depth = std::mem::take(&mut snapshot.depth);
            *publication.0.lock().unwrap() = Some(Arc::new(Published {
                sequence,
                revision,
                config: Config::from_args(&app.args),
                snapshot,
                depth,
                message: app.message.clone(),
            }));
            publication.1.notify_all();
            next += period;
            if next < Instant::now() {
                next = Instant::now() + period;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // Let the writer deliver the stop acknowledgement before closing its socket.
    publication.1.notify_all();
    std::thread::sleep(Duration::from_millis(20));
    Ok(())
}
