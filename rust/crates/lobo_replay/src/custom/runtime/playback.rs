//! Paced, restartable finite sources. Book mutations and controls share a worker.
use super::*;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlaybackCommand {
    Status,
    Play,
    Pause,
    Speed {
        speed: f64,
    },
    Restart,
    /// Absolute source timestamp, in nanoseconds. Earlier positions clamp to origin.
    Seek {
        timestamp_ns: u64,
    },
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PlaybackStatus {
    pub paused: bool,
    pub speed: f64,
    pub clock_ns: u64,
    pub start_ns: Option<u64>,
    pub complete: bool,
    pub seeking: bool,
    pub error: Option<String>,
}

type Reply = mpsc::SyncSender<Result<PlaybackStatus, String>>;
pub(super) struct Playback {
    paused: bool,
    speed: f64,
    elapsed: u64,
    anchor: Instant,
    seek: Option<u64>,
    reply: Option<Reply>,
    error: Option<String>,
    #[cfg(feature = "json")]
    publisher: Option<crate::custom::observer::channel::Publisher>,
}
impl Playback {
    fn status(&self, adapter: &dyn MarketDataAdapter) -> PlaybackStatus {
        let state = adapter.state();
        PlaybackStatus {
            paused: self.paused,
            speed: self.speed,
            clock_ns: state.clock_ns,
            start_ns: state.start_ns,
            complete: state.complete,
            seeking: self.seek.is_some(),
            error: self.error.clone(),
        }
    }
    fn anchor(&mut self, adapter: &dyn MarketDataAdapter) {
        self.elapsed = adapter
            .state()
            .clock_ns
            .saturating_sub(adapter.state().start_ns.unwrap_or(0));
        self.anchor = Instant::now();
    }
    fn target(&self, adapter: &dyn MarketDataAdapter) -> u64 {
        if let Some(seek) = self.seek {
            return seek.saturating_sub(adapter.state().start_ns.unwrap_or(seek));
        }
        if self.paused || adapter.state().start_ns.is_none() {
            return self.elapsed;
        }
        self.elapsed
            .saturating_add((self.anchor.elapsed().as_nanos() as f64 * self.speed) as u64)
    }
    fn finish_seek(&mut self, adapter: &dyn MarketDataAdapter) -> Result<(), String> {
        if self.seek.take().is_some() {
            self.anchor(adapter);
            #[cfg(feature = "json")]
            if let Some(publisher) = &self.publisher {
                publisher.resume_snapshot(adapter)?;
            }
            if let Some(reply) = self.reply.take() {
                let _ = reply.send(Ok(self.status(adapter)));
            }
        }
        Ok(())
    }
    fn clock(&self, _adapter: &dyn MarketDataAdapter) {
        #[cfg(feature = "json")]
        if let Some(publisher) = &self.publisher {
            publisher.publish_clock(_adapter.state());
        }
    }
}
pub(super) fn validate_speed(speed: f64) -> Result<(), String> {
    if !speed.is_finite() || speed <= 0.0 || speed > 1000.0 {
        return Err("Playback speed must be finite and in (0, 1000]".into());
    }
    Ok(())
}

impl ControlReceiver {
    pub(super) fn control_playback(
        &self,
        adapter: &mut dyn MarketDataAdapter,
        command: PlaybackCommand,
        reply: Reply,
    ) {
        let mut playback = self.playback.borrow_mut();
        let Some(p) = playback.as_mut() else {
            let _ = reply.send(Err("This session has no playback controls".into()));
            return;
        };
        let result = match command {
            PlaybackCommand::Status => Ok(()),
            PlaybackCommand::Play if p.error.is_some() => {
                Err("Restart or seek to recover the failed replay".into())
            }
            PlaybackCommand::Play => {
                p.anchor(adapter);
                p.paused = adapter.state().complete && p.seek.is_none();
                Ok(())
            }
            PlaybackCommand::Pause => {
                p.anchor(adapter);
                p.paused = true;
                Ok(())
            }
            PlaybackCommand::Speed { speed } => validate_speed(speed).map(|()| {
                p.anchor(adapter);
                p.speed = speed;
            }),
            PlaybackCommand::Restart | PlaybackCommand::Seek { .. } => {
                if p.reply.is_some() {
                    let _ = reply.send(Err("A seek is already in progress".into()));
                    return;
                }
                p.seek = Some(match command {
                    PlaybackCommand::Seek { timestamp_ns } => timestamp_ns,
                    _ => 0,
                });
                p.reply = Some(reply);
                p.error = None;
                self.rewind.set(true);
                #[cfg(feature = "json")]
                if let Some(publisher) = &p.publisher {
                    publisher.suspend();
                }
                return;
            }
        };
        let _ = reply.send(result.map(|()| p.status(adapter)));
    }
    /// Wait before consuming input, while retaining responsive queries and close().
    pub(crate) fn ready(&self, adapter: &mut dyn MarketDataAdapter) -> Result<bool, String> {
        loop {
            if !self.poll(adapter) {
                return Ok(false);
            }
            let waiting = self
                .playback
                .borrow()
                .as_ref()
                .is_some_and(|p| p.paused && p.seek.is_none());
            if !waiting {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    pub(crate) fn receive(
        &self,
        adapter: &mut dyn MarketDataAdapter,
        bytes: &[u8],
        eof: bool,
    ) -> Result<bool, String> {
        // JSON protocols can depend on arbitrary earlier protocol state. A private
        // decoder discovers the next packet's timestamp without touching visible
        // books. It retains one packet of lookahead, never a recording-sized cache.
        let timestamp = if let Some(probe) = self.probe.borrow_mut().as_mut() {
            probe.state_mut().selected = adapter.state().selected.clone();
            probe.receive(bytes, eof)?;
            probe.commands();
            probe
                .state()
                .start_ns
                .map(|start| (start, probe.state().clock_ns))
        } else {
            None
        };
        if let Some((origin, timestamp)) = timestamp {
            if adapter.state().start_ns.is_none() {
                adapter.state_mut().start_ns = Some(origin);
                adapter.state_mut().clock_ns = origin;
                if let Some(p) = self.playback.borrow_mut().as_mut() {
                    p.anchor(adapter);
                }
            }
            loop {
                if !self.ready(adapter)? {
                    return Ok(false);
                }
                let target = self
                    .playback
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .target(adapter)
                    .saturating_add(origin);
                if eof || timestamp <= target {
                    break;
                }
                adapter.state_mut().clock_ns = target;
                let mut playback = self.playback.borrow_mut();
                let p = playback.as_mut().unwrap();
                if p.seek.is_some() {
                    p.finish_seek(adapter)?;
                } else {
                    p.clock(adapter);
                }
                drop(playback);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        adapter.receive(bytes, eof)?;
        Ok(true)
    }
    pub(super) fn advance_replay(
        &self,
        adapter: &mut dyn MarketDataAdapter,
    ) -> Result<bool, String> {
        if self.probe.borrow().is_some() {
            return Ok(!self.is_stopped());
        }
        loop {
            if !self.ready(adapter)? {
                return Ok(false);
            }
            let had_origin = adapter.state().start_ns.is_some();
            let target = self.playback.borrow().as_ref().unwrap().target(adapter);
            let consumed = adapter.state().consumed;
            adapter.advance(target, if had_origin { 4096 } else { 1 })?;
            let mut playback = self.playback.borrow_mut();
            let p = playback.as_mut().unwrap();
            if !had_origin && adapter.state().start_ns.is_some() {
                p.anchor(adapter);
            }
            if adapter.state().complete {
                p.paused = true;
                p.finish_seek(adapter)?;
                return Ok(true);
            }
            if adapter.state().needs_input {
                return Ok(true);
            }
            if adapter.state().consumed == consumed {
                if p.seek.is_some() {
                    p.finish_seek(adapter)?;
                }
                drop(playback);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

impl Session {
    /// Start a finite replay with public controls; ordinary spawn remains unpaced.
    pub fn spawn_playback(
        factory: impl Fn(bool) -> Result<Box<dyn MarketDataAdapter>, String> + Send + 'static,
        source: Source,
        paused: bool,
        speed: f64,
        json_packets: bool,
        #[cfg(feature = "json")] publisher: Option<crate::custom::observer::channel::Publisher>,
    ) -> Result<Self, String> {
        validate_speed(speed)?;
        if matches!(source, Source::WebSocket { .. }) {
            return Err("Playback requires a finite file, HTTP stream, or packet source".into());
        }
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let completion: Completion = Arc::new((Mutex::new(None), Condvar::new()));
        let done = completion.clone();
        let thread = std::thread::Builder::new()
            .name("lobo-playback".into())
            .spawn(move || {
                let mut adapter = match factory(true) {
                    Ok(adapter) => adapter,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let probe = if json_packets {
                    match factory(false) {
                        Ok(probe) => Some(probe),
                        Err(error) => {
                            let _ = ready_tx.send(Err(error));
                            return;
                        }
                    }
                } else {
                    None
                };
                let controls = ControlReceiver {
                    rx,
                    stopped: Cell::new(false),
                    rewind: Cell::new(false),
                    probe: std::cell::RefCell::new(probe),
                    playback: std::cell::RefCell::new(Some(Playback {
                        paused,
                        speed,
                        elapsed: 0,
                        anchor: Instant::now(),
                        seek: None,
                        reply: None,
                        error: None,
                        #[cfg(feature = "json")]
                        publisher,
                    })),
                };
                let _ = ready_tx.send(Ok(()));
                loop {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                        || -> Result<(), String> {
                            if controls.rewind.replace(false) {
                                let selected = adapter.state().selected.clone();
                                adapter = factory(true)?;
                                // Selection can refer to a directory discovered from the file.
                                adapter.state_mut().selected = selected;
                                *controls.probe.borrow_mut() = if json_packets {
                                    Some(factory(false)?)
                                } else {
                                    None
                                };
                                *done.0.lock().expect("Completion lock poisoned") = None;
                            }
                            Box::new(source.clone()).run(adapter.as_mut(), &controls)
                        },
                    ))
                    .unwrap_or_else(|_| Err("Replay worker panicked".into()));
                    if controls.stopped.get() {
                        break;
                    }
                    if controls.rewind.get() && result.is_ok() {
                        continue;
                    }
                    {
                        let mut playback = controls.playback.borrow_mut();
                        let p = playback.as_mut().unwrap();
                        p.paused = true;
                        if let Err(error) = &result {
                            p.error = Some(error.clone());
                            p.seek = None;
                            if let Some(reply) = p.reply.take() {
                                let _ = reply.send(Err(error.clone()));
                            }
                            #[cfg(feature = "json")]
                            if let Some(publisher) = &p.publisher {
                                let _ = publisher.resume_snapshot(adapter.as_ref());
                            }
                        } else if let Err(error) = p.finish_seek(adapter.as_ref()) {
                            p.error = Some(error);
                        }
                        p.clock(adapter.as_ref());
                    }
                    *done.0.lock().expect("Completion lock poisoned") = Some(result);
                    done.1.notify_all();
                    while controls.poll(adapter.as_mut()) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    if controls.stopped.get() {
                        break;
                    }
                }
                // Wake waiters when close interrupts a paused replay or reconstruction.
                *done.0.lock().expect("Completion lock poisoned") = Some(Ok(()));
                done.1.notify_all();
            })
            .map_err(|e| e.to_string())?;
        match ready_rx
            .recv()
            .map_err(|_| "Replay initialization panicked")?
        {
            Ok(()) => Ok(Self {
                tx,
                completion,
                thread: Some(thread),
            }),
            Err(error) => {
                let _ = thread.join();
                Err(error)
            }
        }
    }
}
