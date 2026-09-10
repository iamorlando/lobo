#![cfg(feature = "native")]
use lobo_replay::custom::{
    AdapterInfo, BookLevel, CustomAdapter, FeedMode, FeedState, Protocol,
    runtime::{Session, Source},
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

const INFO: AdapterInfo<'static> = AdapterInfo {
    id: "http-test",
    name: "HTTP test",
    mode: FeedMode::Live,
    level: BookLevel::L3,
    endpoint: None,
    default_symbol: "BOOK",
    timezone: "UTC",
    supports_trades: false,
};
struct Collect(Arc<Mutex<Vec<u8>>>);
impl Protocol for Collect {
    fn receive(&mut self, state: &mut FeedState, bytes: &[u8], eof: bool) -> Result<(), String> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        state.consumed += bytes.len() as u64;
        state.complete = eof;
        Ok(())
    }
}
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut writer = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap()
}
fn serve(body: Vec<u8>, status: u16) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/replay", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket.read(&mut [0; 4096]).unwrap();
        let header = format!(
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        if socket.write_all(header.as_bytes()).is_err() {
            return;
        }
        for chunk in body.chunks(1) {
            if socket.write_all(chunk).is_err() {
                return;
            }
        }
    });
    (url, thread)
}
fn session(url: String) -> (Session, Arc<Mutex<Vec<u8>>>) {
    let output = Arc::new(Mutex::new(Vec::new()));
    let copy = output.clone();
    let session = Session::spawn(
        move || CustomAdapter::new(INFO, Collect(copy), "BOOK"),
        Source::Http { url, chunk_size: 3 },
    )
    .unwrap();
    (session, output)
}
#[test]
fn plain_gzip_and_concatenated_gzip_deliver_identical_bytes_and_eof() {
    let expected = b"native records across arbitrary chunk boundaries";
    let mut concatenated = gzip(&expected[..9]);
    concatenated.extend(gzip(&expected[9..]));
    for body in [expected.to_vec(), gzip(expected), concatenated] {
        let (url, server) = serve(body, 200);
        let (mut session, output) = session(url);
        session.wait().unwrap();
        assert_eq!(&*output.lock().unwrap(), expected);
        assert!(session.with(|a| Ok(a.state().complete)).unwrap());
        session.close();
        server.join().unwrap();
    }
}
#[test]
fn truncated_gzip_and_invalid_crc_fail_without_signalling_eof() {
    let mut truncated = gzip(b"native records");
    truncated.truncate(truncated.len() - 5);
    let mut bad_crc = gzip(b"native records");
    let crc_start = bad_crc.len() - 8;
    bad_crc[crc_start] ^= 1;
    for body in [truncated, bad_crc] {
        let (url, server) = serve(body, 200);
        let (mut session, _) = session(url);
        assert!(session.wait().is_err());
        assert!(!session.with(|a| Ok(a.state().complete)).unwrap());
        session.close();
        server.join().unwrap();
    }
}
#[test]
fn http_errors_never_enter_the_decoder() {
    let (url, server) = serve(b"not a replay".to_vec(), 404);
    let (mut session, output) = session(url);
    assert!(session.wait().unwrap_err().contains("404"));
    assert!(output.lock().unwrap().is_empty());
    session.close();
    server.join().unwrap();
}
#[test]
fn closing_cancels_stalled_headers_and_body() {
    for headers in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/replay", listener.local_addr().unwrap());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.read(&mut [0; 4096]).unwrap();
            if headers {
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\na")
                    .unwrap();
            }
            ready_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        let (mut session, _) = session(url);
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        session.close();
        let elapsed = start.elapsed();
        release_tx.send(()).unwrap();
        server.join().unwrap();
        assert!(elapsed < Duration::from_secs(1), "cancel took {elapsed:?}");
        session.wait().unwrap();
    }
}
