//! HTTP bootstrap and WebSocket IO. Socket tasks pass bounded raw frames
//! to the protocol.
use super::{MarketDataAdapter, runtime::ControlReceiver};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_tungstenite::tungstenite::Message;

/// Send and consume a source's application-level text heartbeat.
///
/// Configure this on Source.websocket when the server expects a plain text frame,
/// rather than a JSON message or a WebSocket Ping control frame. Matching replies
/// are handled by the transport before market-data decoding.
///
/// Args:
///     request: The exact text sent to the server, without JSON quotation marks.
///     reply: The exact reply text consumed by the transport.
///     interval: Seconds between requests. Must be finite and positive. Defaults to 10.
///
/// Raises:
///     ValueError: The interval is invalid or either text is empty.
///
/// Examples:
/// ```python
/// from lobo.replay.adapters import models as lm
///
/// heartbeat = lm.TextHeartbeat("PING", "PONG", interval=10)
/// source = lm.Source.websocket("wss://feed.example/ws", heartbeat=heartbeat)
/// ```
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(frozen, from_py_object, module = "lobo.replay.adapters.models")
)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TextHeartbeat {
    /// The exact request frame, sent without JSON encoding.
    pub request: String,
    /// The exact response frame consumed outside the decoder.
    pub reply: String,
    /// The interval between requests, measured in seconds.
    pub interval: f64,
}
impl TextHeartbeat {
    /// Validate transport settings once before a connection starts.
    pub fn validate(&self) -> Result<Duration, &'static str> {
        let interval = Duration::try_from_secs_f64(self.interval)
            .map_err(|_| "heartbeat interval must be finite and positive")?;
        if interval.is_zero() || self.request.is_empty() || self.reply.is_empty() {
            return Err("heartbeat interval must be positive and text must not be empty");
        }
        Ok(interval)
    }
}
#[cfg(feature = "python")]
#[pyo3::pymethods]
impl TextHeartbeat {
    #[new]
    #[pyo3(signature=(request,reply,*,interval=10.0))]
    fn new(request: String, reply: String, interval: f64) -> pyo3::PyResult<Self> {
        let value = Self {
            request,
            reply,
            interval,
        };
        value
            .validate()
            .map_err(pyo3::exceptions::PyValueError::new_err)?;
        Ok(value)
    }
}

// Select the control-frame policy once per connection. Sources without text
// heartbeats have no reply comparison or timer wakeups in their packet loop.
trait ControlFrames: Send + 'static {
    fn connected(&mut self) {}
    fn tick(&mut self) -> impl std::future::Future<Output = ()> + Send;
    fn request(&self) -> &str;
    fn accepts(&self, text: &str) -> bool;
}
struct NoControlFrames;
impl ControlFrames for NoControlFrames {
    fn tick(&mut self) -> impl std::future::Future<Output = ()> + Send {
        std::future::pending()
    }
    fn request(&self) -> &str {
        ""
    }
    #[inline(always)]
    fn accepts(&self, _: &str) -> bool {
        false
    }
}
struct TextControlFrames {
    config: TextHeartbeat,
    timer: tokio::time::Interval,
}
impl ControlFrames for TextControlFrames {
    fn connected(&mut self) {
        self.timer.reset();
    }
    async fn tick(&mut self) {
        self.timer.tick().await;
    }
    fn request(&self) -> &str {
        &self.config.request
    }
    fn accepts(&self, text: &str) -> bool {
        text == self.config.reply
    }
}

enum Payload {
    Connected,
    Data(Vec<u8>),
    Disconnected,
}
struct Event {
    id: u32,
    generation: u64,
    payload: Payload,
}
struct Connection {
    generation: u64,
    outgoing: mpsc::UnboundedSender<String>,
    task: JoinHandle<()>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn connect<H: ControlFrames>(
    id: u32,
    generation: u64,
    endpoint: String,
    events: mpsc::Sender<Event>,
    mut heartbeat: H,
) -> Connection {
    let (outgoing, mut commands) = mpsc::unbounded_channel::<String>();
    let task = tokio::spawn(async move {
        let send = |payload| {
            events.send(Event {
                id,
                generation,
                payload,
            })
        };
        loop {
            // Subscriptions are rebuilt by connected_on after each reconnect.
            while commands.try_recv().is_ok() {}
            if commands.is_closed() {
                break;
            }
            if let Ok((mut socket, _)) = tokio_tungstenite::connect_async(&endpoint).await {
                heartbeat.connected();
                if send(Payload::Connected).await.is_err() {
                    break;
                }
                loop {
                    tokio::select! {
                        _=heartbeat.tick()=>if socket.send(Message::Text(heartbeat.request().to_owned().into())).await.is_err() { break; },
                        command=commands.recv()=>match command {
                            Some(command)=>if socket.send(Message::Text(command.into())).await.is_err() { break; },
                            None=> { let _=socket.close(None).await; return; },
                        },
                        message=socket.next()=>match message {
                            Some(Ok(Message::Text(bytes)))=>if !heartbeat.accepts(&bytes) && send(Payload::Data(bytes.as_bytes().to_vec())).await.is_err() { return; },
                            Some(Ok(Message::Binary(bytes)))=>if send(Payload::Data(bytes.to_vec())).await.is_err() { return; },
                            Some(Ok(Message::Ping(bytes)))=>if socket.send(Message::Pong(bytes)).await.is_err() { break; },
                            Some(Ok(Message::Close(_)))|Some(Err(_))|None=>break,
                            _=>(),
                        },
                    }
                }
                if send(Payload::Disconnected).await.is_err() {
                    break;
                }
            }
            tokio::select! {
                _=tokio::time::sleep(Duration::from_millis(500))=>(),
                command=commands.recv()=>if command.is_none() { break; },
            }
        }
    });
    Connection {
        generation,
        outgoing,
        task,
    }
}

fn synchronize(
    adapter: &mut dyn MarketDataAdapter,
    connections: &mut BTreeMap<u32, Connection>,
    ready: &mut BTreeSet<u32>,
    events: &mpsc::Sender<Event>,
    endpoint: &Option<String>,
    generation: &mut u64,
    heartbeat: &Option<(TextHeartbeat, Duration)>,
) -> Result<(), String> {
    let mut requested = adapter
        .connections()
        .into_iter()
        .map(|c| (c.id, endpoint.as_deref().unwrap_or(c.endpoint).to_owned()))
        .collect::<Vec<_>>();
    // Some protocols receive a user-owned endpoint instead of advertising one.
    if requested.is_empty() && adapter.info().endpoint.is_none() {
        if let Some(endpoint) = endpoint {
            requested.push((0, endpoint.clone()));
        }
    }
    let wanted = requested.iter().map(|(id, _)| *id).collect::<BTreeSet<_>>();
    connections.retain(|id, _| wanted.contains(id));
    ready.retain(|id| wanted.contains(id));
    for (id, url) in requested {
        connections.entry(id).or_insert_with(|| {
            *generation += 1;
            match heartbeat {
                Some((config, interval)) => {
                    let mut timer = tokio::time::interval(*interval);
                    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    connect(
                        id,
                        *generation,
                        url,
                        events.clone(),
                        TextControlFrames {
                            config: config.clone(),
                            timer,
                        },
                    )
                }
                None => connect(id, *generation, url, events.clone(), NoControlFrames),
            }
        });
        if ready.contains(&id) {
            for command in adapter.commands_on(id) {
                connections[&id]
                    .outgoing
                    .send(command)
                    .map_err(|_| "Socket sender closed")?;
            }
        }
    }
    Ok(())
}

pub(super) fn run(
    adapter: &mut dyn MarketDataAdapter,
    controls: &ControlReceiver,
    endpoint: Option<String>,
    text_heartbeat: Option<TextHeartbeat>,
) -> Result<(), String> {
    let text_heartbeat = text_heartbeat
        .map(|config| config.validate().map(|interval| (config, interval)))
        .transpose()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let client=reqwest::Client::builder().timeout(Duration::from_secs(30)).build().map_err(|e|e.to_string())?;
        let requests=adapter.bootstrap_requests().into_iter().map(|r|(r.id.to_owned(),r.url.to_owned())).collect::<Vec<_>>();
        for (id,url) in requests {
            let request=async { client.get(url).send().await?.error_for_status()?.bytes().await };
            tokio::pin!(request);
            let bytes=loop {
                tokio::select! {
                    response=&mut request=>break response.map_err(|e|e.to_string())?,
                    _=tokio::time::sleep(Duration::from_millis(10))=>if !controls.poll(adapter) { return Ok(()); },
                }
            };
            adapter.bootstrap(&id,&bytes)?;
        }
        let (tx,mut rx)=mpsc::channel(64);
        let mut connections=BTreeMap::new();
        let mut ready=BTreeSet::new();
        let mut generation=0;
        let mut clock=super::runtime::LiveClock::default();
        let mut controls_tick=tokio::time::interval(Duration::from_millis(10));
        let mut heartbeat=tokio::time::interval(Duration::from_secs(15));
        loop {
            if !controls.poll(adapter) { break; }
            synchronize(adapter,&mut connections,&mut ready,&tx,&endpoint,&mut generation,&text_heartbeat)?;
            tokio::select! {
                event=rx.recv()=>match event {
                    Some(Event{id,generation,payload})=> {
                        if !connections.get(&id).is_some_and(|c|c.generation==generation) { continue; }
                        match payload {
                            Payload::Connected=> { ready.insert(id);adapter.connected_on(id)?; },
                            Payload::Data(bytes)=>if adapter.receive_on(id,&bytes).is_err() {
                                // Checksum/sequence failures require a new snapshot.
                                // Ignore already queued frames from this generation.
                                adapter.disconnected_on(id);
                                ready.remove(&id);
                                connections.remove(&id);
                            },
                            Payload::Disconnected=> { ready.remove(&id);adapter.disconnected_on(id); },
                        }
                    },
                    None=>break,
                },
                _=heartbeat.tick()=>for id in &ready { adapter.keepalive_on(*id); },
                _=controls_tick.tick()=>clock.advance(adapter)?,
            }
        }
        // Abort sockets, including pending connects, before dropping state.
        drop(connections);
        Ok(())
    })
}
