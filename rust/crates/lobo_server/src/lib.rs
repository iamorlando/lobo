//! Axum transport over native books and the shared order-command adapter.
mod http;
#[cfg(feature = "python")]
pub mod python;
pub mod registry;
pub use http::router;
pub use registry::{RegisteredBook, Registry};
use std::{
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    sync::Arc,
    thread::JoinHandle,
};
use tokio::sync::watch;

pub struct Server {
    pub registry: Arc<Registry>,
    address: SocketAddr,
    shutdown: watch::Sender<bool>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    pub fn start(
        address: SocketAddr,
        web_root: PathBuf,
        queue_capacity: usize,
    ) -> Result<Self, String> {
        Self::start_with_metrics(address, web_root, queue_capacity, Default::default())
    }
    pub fn start_with_metrics(
        address: SocketAddr,
        web_root: PathBuf,
        queue_capacity: usize,
        metrics: lobo_batchers::PriceLevelMetrics,
    ) -> Result<Self, String> {
        if queue_capacity == 0 {
            return Err("queue_capacity must be positive".into());
        }
        if !web_root.join("index.html").is_file() {
            return Err(format!(
                "Terminal assets missing at {}. Run make web-server-assets before building the Python package.",
                web_root.display()
            ));
        }
        let listener = TcpListener::bind(address).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let (shutdown, rx) = watch::channel(false);
        let registry = Arc::new(Registry::with_metrics(queue_capacity, rx.clone(), metrics));
        let app = router(registry.clone(), web_root);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let thread = std::thread::Builder::new()
            .name("lobo-server".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener)
                        .expect("configured TCP listener");
                    let mut rx = rx;
                    let result = axum::serve(listener, app)
                        .with_graceful_shutdown(async move {
                            let _ = rx.wait_for(|stopped| *stopped).await;
                        })
                        .await;
                    if let Err(error) = result {
                        eprintln!("lobo server: {error}");
                    }
                });
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            registry,
            address,
            shutdown,
            thread: Some(thread),
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }
    pub fn close(&mut self) {
        self.shutdown.send_replace(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.close();
    }
}
