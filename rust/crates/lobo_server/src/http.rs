use crate::Registry;
use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use lobo_models::server::{Command, CommandResponse, FeedMessage};
use lobo_replay::order_messages::CommandError;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tower_http::services::ServeDir;

pub fn router(registry: Arc<Registry>, web_root: PathBuf) -> Router {
    Router::new()
        .route("/api/server-context", get(configuration))
        .route("/api/books", get(books))
        .route("/api/books/{book}", get(snapshot))
        .route("/api/books/{book}/orders", post(command))
        .route("/api/schema", get(schema))
        .route("/api/feed", get(feed))
        .route("/api/adapters/{index}/feed", get(adapter_feed))
        .route("/api/adapters/{index}/orders", post(adapter_command))
        .route(
            "/api/adapters/{index}/subscriptions",
            post(adapter_subscribe),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
        .fallback_service(ServeDir::new(web_root))
        .with_state(registry)
}
async fn configuration(
    State(registry): State<Arc<Registry>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let adapters = registry.adapters.read().clone();
    let configuration = tokio::task::spawn_blocking(move || {
        let mut feeds = Vec::new();
        let mut books = Vec::new();
        for (index, adapter) in adapters.iter().enumerate() {
            let directory = adapter.books()?;
            let mut info = lobo_replay::custom::observer::descriptor(adapter.descriptor.info());
            info["observer"] = true.into();
            info["id"] = format!("custom-{index}").into();
            info["endpoint"] = format!("/api/adapters/{index}/feed").into();
            info["subscriptionsEndpoint"] = format!("/api/adapters/{index}/subscriptions").into();
            info["books"] = directory.clone().into();
            books.extend(directory);
            feeds.push(info);
        }
        Ok::<_, String>((feeds, books))
    })
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Adapter configuration task failed".into(),
        )
    })?
    .map_err(|e| ApiError(StatusCode::SERVICE_UNAVAILABLE, e))?;
    let (mut feeds, mut books) = configuration;
    let local = registry
        .directory()
        .into_iter()
        .map(|b| serde_json::json!(b))
        .collect::<Vec<_>>();
    if !local.is_empty() {
        feeds.push(serde_json::json!({"id":"server","name":"Python books","mode":"live","level":"l3","defaultSymbol":local[0]["symbol"],"endpoint":"/api/feed","timezone":"UTC","supportsTrades":true,"books":local}));
    }
    books.extend(local);
    Ok(Json(
        serde_json::json!({"mode":"server","name":"Python books","books":books,"adapters":feeds,"metrics":registry.metrics.bits()}),
    ))
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AdapterSubscription {
    #[serde(default)]
    symbols: Vec<String>,
    selected: String,
}
async fn adapter_subscribe(
    State(registry): State<Arc<Registry>>,
    Path(index): Path<usize>,
    Json(request): Json<AdapterSubscription>,
) -> Result<StatusCode, ApiError> {
    let adapter = registry
        .adapters
        .read()
        .get(index)
        .cloned()
        .ok_or(ApiError(
            StatusCode::NOT_FOUND,
            "Adapter does not exist".into(),
        ))?;
    tokio::task::spawn_blocking(move || adapter.subscribe(request.symbols, request.selected))
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Subscription task failed".into(),
            )
        })?
        .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn books(
    State(registry): State<Arc<Registry>>,
) -> Json<Vec<lobo_models::server::BookInfo>> {
    Json(registry.directory())
}
async fn snapshot(
    State(registry): State<Arc<Registry>>,
    Path(book): Path<String>,
) -> Result<Json<FeedMessage>, ApiError> {
    let book = registry.get(&book).ok_or(ApiError(
        StatusCode::NOT_FOUND,
        "book does not exist".into(),
    ))?;
    tokio::task::spawn_blocking(move || book.snapshot())
        .await
        .map(Json)
        .map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "snapshot task failed".into(),
            )
        })
}
async fn command(
    State(registry): State<Arc<Registry>>,
    Path(book): Path<String>,
    Json(command): Json<Command>,
) -> Result<Json<CommandResponse>, ApiError> {
    let book = registry.get(&book).ok_or(ApiError(
        StatusCode::NOT_FOUND,
        "book does not exist".into(),
    ))?;
    // Matching is CPU work; it does not occupy the Tokio network workers.
    tokio::task::spawn_blocking(move || book.submit(command))
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "command task failed".into(),
            )
        })?
        .map(Json)
        .map_err(Into::into)
}
async fn schema() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "command": schemars::schema_for!(Command),
        "adapter_command": schemars::schema_for!(AdapterCommand),
        "adapter_subscription": schemars::schema_for!(AdapterSubscription),
        "response": schemars::schema_for!(CommandResponse),
        "feed": schemars::schema_for!(FeedMessage),
        "market": schemars::schema_for!(lobo_models::server::MarketOrder),
        "limit": schemars::schema_for!(lobo_models::server::LimitOrder),
        "iceberg": schemars::schema_for!(lobo_models::server::IcebergOrder),
    }))
}
struct ApiError(StatusCode, String);
impl From<CommandError> for ApiError {
    fn from(error: CommandError) -> Self {
        let status = match error {
            CommandError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
            CommandError::MissingOrder => StatusCode::NOT_FOUND,
            CommandError::DuplicateOrder => StatusCode::CONFLICT,
        };
        Self(status, error.to_string())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"error": self.1}))).into_response()
    }
}
async fn feed(ws: WebSocketUpgrade, State(registry): State<Arc<Registry>>) -> Response {
    ws.max_message_size(16 * 1024)
        .on_upgrade(move |socket| stream(socket, registry))
}
async fn send(socket: &mut WebSocket, message: &FeedMessage) -> Result<(), ()> {
    let text = serde_json::to_string(message).map_err(|_| ())?;
    // A stalled browser gets a fresh snapshot on reconnect; it cannot stall books.
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(Message::Text(text.into())),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}
async fn stream(mut socket: WebSocket, registry: Arc<Registry>) {
    // Subscribe before taking snapshots. Per-book sequence numbers discard any
    // queued updates already included in a snapshot, without losing later ones.
    let mut events = registry.events.subscribe();
    let mut shutdown = registry.shutdown.clone();
    if send(
        &mut socket,
        &FeedMessage::Directory {
            books: registry.directory(),
        },
    )
    .await
    .is_err()
    {
        return;
    }
    for book in registry.books() {
        let Ok(snapshot) = tokio::task::spawn_blocking(move || book.snapshot()).await else {
            return;
        };
        if send(&mut socket, &snapshot).await.is_err() {
            return;
        }
    }
    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            _ = async { let _ = shutdown.wait_for(|stopped| *stopped).await; } => break,
            event = events.recv() => {
                let Ok(event) = event else { break; };
                if send(&mut socket, &event).await.is_err() { break; }
            },
            _ = heartbeat.tick() => { if send(&mut socket, &FeedMessage::Heartbeat).await.is_err() { break; } },
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(_))) | Some(Ok(Message::Ping(_))) => {
                        if send(&mut socket, &FeedMessage::Heartbeat).await.is_err() { break; }
                    },
                    Some(Ok(Message::Pong(_))) => {},
                    _ => break,
                }
            },
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.send(Message::Close(None))).await;
}

async fn adapter_feed(
    ws: WebSocketUpgrade,
    State(registry): State<Arc<Registry>>,
    Path(index): Path<usize>,
) -> Result<Response, ApiError> {
    let adapter = registry
        .adapters
        .read()
        .get(index)
        .cloned()
        .ok_or(ApiError(
            StatusCode::NOT_FOUND,
            "Adapter does not exist".into(),
        ))?;
    let shutdown = registry.shutdown.clone();
    Ok(ws
        .max_message_size(16 * 1024)
        .on_upgrade(move |socket| adapter_stream(socket, adapter, shutdown)))
}
async fn adapter_send(socket: &mut WebSocket, value: &serde_json::Value) -> Result<(), ()> {
    let text = serde_json::to_string(value).map_err(|_| ())?;
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(Message::Text(text.into())),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}
async fn adapter_stream(
    mut socket: WebSocket,
    adapter: lobo_replay::custom::observer::HostedAdapter,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut events = adapter.publisher.events.subscribe();
    let Ok(Ok(snapshot)) = tokio::task::spawn_blocking(move || adapter.snapshot()).await else {
        return;
    };
    if adapter_send(&mut socket, &snapshot).await.is_err() {
        return;
    }
    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            _=async{let _=shutdown.wait_for(|stopped|*stopped).await;}=>break,
            event=events.recv()=>{let Ok(event)=event else{break;};if adapter_send(&mut socket,&event).await.is_err(){break;}},
            _=heartbeat.tick()=>{if adapter_send(&mut socket,&serde_json::json!({"type":"heartbeat"})).await.is_err(){break;}},
            message=socket.recv()=>match message{Some(Ok(Message::Pong(_)|Message::Ping(_)|Message::Text(_)))=>{},_=>break},
        }
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AdapterCommand {
    book: String,
    command: Command,
}
async fn adapter_command(
    State(registry): State<Arc<Registry>>,
    Path(index): Path<usize>,
    Json(request): Json<AdapterCommand>,
) -> Result<Json<CommandResponse>, ApiError> {
    let adapter = registry
        .adapters
        .read()
        .get(index)
        .cloned()
        .ok_or(ApiError(
            StatusCode::NOT_FOUND,
            "Adapter does not exist".into(),
        ))?;
    let symbol = lobo_replay::custom::normalize_symbol(&request.book)
        .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    tokio::task::spawn_blocking(move || adapter.submit(symbol, request.command))
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Order task failed".into(),
            )
        })?
        .map(Json)
        .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, e))
}
