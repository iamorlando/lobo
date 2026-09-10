use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use lobo_server::{Registry, router};
use std::sync::Arc;
use tokio::sync::watch;
use tower::ServiceExt;

#[tokio::test]
async fn http_commands_schemas_and_assets_use_the_registered_book() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("index.html"),
        "<title>Native terminal</title>",
    )
    .unwrap();
    let registry = Arc::new(Registry::new(32, watch::channel(false).1));
    let book = registry.register(Some("AAPL".into()), 2, 0).unwrap();
    let app = router(registry.clone(), root.path().into());
    let command = serde_json::json!({"op":"add", "order":{"type":"limit", "id":"00000000-0000-0000-0000-000000000001", "side":"buy", "price":10000, "quantity":10}});
    let request = || {
        Request::post("/api/books/AAPL/orders")
            .header("content-type", "application/json")
            .body(Body::from(command.to_string()))
            .unwrap()
    };
    let response = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(response["sequence"], 1);
    assert_eq!(book.native.lock().order_storage.bids.visible_quantity, 10);
    assert_eq!(
        app.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    let invalid = Request::post("/api/books/AAPL/orders")
        .header("content-type", "application/json")
        .body(Body::from(
            "{\"op\":\"remove\",\"id\":\"00000000-0000-0000-0000-000000000099\"}",
        ))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(invalid).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    for path in [
        "/api/server-context",
        "/api/books",
        "/api/books/AAPL",
        "/api/schema",
        "/",
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        if path == "/api/schema" {
            let schema: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                schema["command"]["$schema"],
                "https://json-schema.org/draft/2020-12/schema"
            );
            for name in ["MarketOrder", "LimitOrder", "IcebergOrder"] {
                assert!(
                    schema["command"]["$defs"].get(name).is_some(),
                    "missing {name}"
                );
            }
        }
        if path == "/api/server-context" {
            let config: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(config["mode"], "server");
            assert_eq!(config["books"][0]["symbol"], "AAPL");
            assert_eq!(config["adapters"][0]["id"], "server");
            assert_eq!(config["adapters"][0]["endpoint"], "/api/feed");
            assert!(config["adapters"][0].get("observer").is_none());
        }
    }
}

#[test]
fn server_binds_an_available_port_and_closes_it() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("index.html"), "terminal").unwrap();
    let mut server =
        lobo_server::Server::start("127.0.0.1:0".parse().unwrap(), root.path().into(), 32)
            .unwrap();
    let address = server.address();
    assert_ne!(address.port(), 0);
    assert!(std::net::TcpStream::connect(address).is_ok());
    server.close();
    server.close();
    assert!(std::net::TcpStream::connect(address).is_err());
}
