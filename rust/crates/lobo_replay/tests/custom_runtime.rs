#![cfg(feature = "native")]
use lobo_replay::custom::{
    AdapterInfo, BookLevel, CustomAdapter, FeedMode, FeedState, Protocol,
    runtime::{ControlReceiver, Driver, Session, Source},
};
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const INFO: AdapterInfo<'static> = AdapterInfo {
    id: "test",
    name: "Native input test",
    mode: FeedMode::Live,
    level: BookLevel::L3,
    endpoint: None,
    default_symbol: "BOOK",
    timezone: "UTC",
    supports_trades: true,
};
struct Counter;
impl Protocol for Counter {
    fn receive(&mut self, state: &mut FeedState, bytes: &[u8], _: bool) -> Result<(), String> {
        if bytes == b"bad" {
            return Err("invalid checksum".into());
        }
        state.messages += 1;
        state.consumed += bytes.len() as u64;
        state.start_ns.get_or_insert(100);
        state.clock_ns = state.clock_ns.max(100);
        Ok(())
    }
    fn disconnected(&mut self, state: &mut FeedState) {
        state.checksum_failures += 1;
    }
}
fn counter() -> Result<CustomAdapter<Counter>, String> {
    CustomAdapter::new(INFO, Counter, "BOOK")
}

#[test]
fn text_heartbeat_replies_never_reach_the_protocol() {
    use futures_util::{SinkExt, StreamExt};
    use lobo_replay::custom::TextHeartbeat;
    use tokio_tungstenite::tungstenite::Message;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            for _ in 0..3 {
                let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert_eq!(frame, Message::Text("PING".into()));
                socket.send(Message::Text("PONG".into())).await.unwrap();
            }
            // The sentinel follows all replies on the same ordered stream.
            socket.send(Message::Text("valid".into())).await.unwrap();
            while let Ok(Some(Ok(message))) =
                tokio::time::timeout(Duration::from_secs(5), socket.next()).await
            {
                if matches!(message, Message::Close(_)) {
                    break;
                }
            }
        });
    });
    let mut session = Session::spawn(
        counter,
        Source::WebSocket {
            endpoint: Some(endpoint),
            heartbeat: Some(TextHeartbeat {
                request: "PING".into(),
                reply: "PONG".into(),
                interval: 0.01,
            }),
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let consumed = session.with(|a| Ok(a.state().consumed)).unwrap();
        if consumed > 0 {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        session
            .with(|a| Ok((a.state().messages, a.state().consumed)))
            .unwrap(),
        (1, 5)
    );
    session.close();
    server.join().unwrap();
}

#[test]
fn gzip_input_and_completed_feed_stay_native_and_queryable() {
    let path = std::env::temp_dir().join(format!(
        "lobo-custom-{}.gz",
        lobo_primitives::uuid::Uuid::new_v4()
    ));
    let file = std::fs::File::create(&path).unwrap();
    let mut gzip = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    gzip.write_all(b"one\ntwo\nthree\n").unwrap();
    gzip.finish().unwrap();
    let mut session = Session::spawn(
        counter,
        Source::JsonLines {
            path: path.clone(),
            bootstrap: vec![],
        },
    )
    .unwrap();
    session.wait().unwrap();
    assert!(session.finished());
    assert_eq!(
        session
            .with(|a| Ok((a.state().messages, a.state().consumed)))
            .unwrap(),
        (3, 14)
    );
    session.close();
    std::fs::remove_file(path).unwrap();
}
struct UntilStopped;
impl Driver for UntilStopped {
    fn run(
        self: Box<Self>,
        adapter: &mut dyn lobo_replay::custom::MarketDataAdapter,
        controls: &ControlReceiver,
    ) -> Result<(), String> {
        while controls.poll(adapter) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
}
#[test]
fn closing_a_running_native_source_cancels_and_joins_it() {
    let mut session = Session::spawn(counter, UntilStopped).unwrap();
    session
        .with(|a| {
            a.receive(b"one", false)?;
            Ok(())
        })
        .unwrap();
    session.close();
    assert!(session.finished());
    session.wait().unwrap();
}
struct Panics;
impl Driver for Panics {
    fn run(
        self: Box<Self>,
        _: &mut dyn lobo_replay::custom::MarketDataAdapter,
        _: &ControlReceiver,
    ) -> Result<(), String> {
        panic!("test native failure")
    }
}
#[test]
fn native_failure_wakes_waiters() {
    let session = Session::spawn(counter, Panics).unwrap();
    assert!(session.wait().unwrap_err().contains("panicked"));
}
#[test]
fn websocket_reconnect_discards_failed_generation_and_advances_quiet_clock() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let server = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            for turn in 0..2 {
                let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                observed.fetch_add(1, Ordering::SeqCst);
                if turn == 0 {
                    socket.send(Message::Text("bad".into())).await.unwrap();
                    let _ = socket.send(Message::Text("stale".into())).await;
                } else {
                    socket.send(Message::Text("valid".into())).await.unwrap();
                    let _ = tokio::time::timeout(Duration::from_secs(5), socket.next()).await;
                }
            }
        });
    });
    let mut session = Session::spawn(
        counter,
        Source::WebSocket {
            heartbeat: None,
            endpoint: Some(endpoint),
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (messages, clock) = session
            .with(|a| Ok((a.state().messages, a.state().clock_ns)))
            .unwrap();
        if messages == 1 && clock > 1_000_000 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "native websocket did not recover"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(session.with(|a| Ok(a.state().consumed)).unwrap(), 5);
    session.close();
    server.join().unwrap();
}
