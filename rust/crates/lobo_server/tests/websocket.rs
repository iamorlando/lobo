use futures_util::StreamExt;
use lobo_models::{
    Side,
    server::{Command, FeedMessage, LimitOrder, Order, OrderFields},
};
use lobo_primitives::uuid::Uuid;
use lobo_server::Server;
use std::time::Duration;

#[tokio::test]
async fn snapshot_precedes_updates_new_books_arrive_and_shutdown_closes_clients() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("index.html"), "terminal").unwrap();
    let mut server = Server::start("127.0.0.1:0".parse().unwrap(), root.path().into(), 32).unwrap();
    let book = server.registry.register(Some("TEST".into()), 0, 0).unwrap();
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{}/api/feed", server.address()))
            .await
            .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        serde_json::from_str::<FeedMessage>(next.to_text().unwrap()).unwrap(),
        FeedMessage::Directory { .. }
    ));
    let next = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        serde_json::from_str::<FeedMessage>(next.to_text().unwrap()).unwrap(),
        FeedMessage::Snapshot { sequence: 0, .. }
    ));
    book.submit(Command::Add {
        order: Order::Limit(LimitOrder {
            fields: OrderFields {
                id: Uuid::from_u128(1),
                trader: Uuid::nil(),
                side: Side::Buy,
                quantity: 5,
            },
            price: 100,
        }),
    })
    .unwrap();
    loop {
        let next = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match serde_json::from_str::<FeedMessage>(next.to_text().unwrap()).unwrap() {
            FeedMessage::Heartbeat => continue,
            FeedMessage::Update { sequence, .. } => {
                assert_eq!(sequence, 1);
                break;
            }
            _ => panic!("expected ordered update"),
        }
    }
    server
        .registry
        .register(Some("LATER".into()), 0, 0)
        .unwrap();
    loop {
        let next = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match serde_json::from_str::<FeedMessage>(next.to_text().unwrap()).unwrap() {
            FeedMessage::Heartbeat => continue,
            FeedMessage::Snapshot { book, .. } => {
                assert_eq!(book.symbol, "LATER");
                break;
            }
            _ => panic!("expected new-book snapshot"),
        }
    }
    server.close();
    tokio::time::timeout(Duration::from_secs(3), async {
        // A heartbeat already sent before shutdown may precede the close frame.
        while let Some(next) = socket.next().await {
            let next = next.unwrap();
            if next.is_close() {
                break;
            }
            assert!(matches!(
                serde_json::from_str::<FeedMessage>(next.to_text().unwrap()).unwrap(),
                FeedMessage::Heartbeat
            ));
        }
    })
    .await
    .unwrap();
    let _ = socket.close(None).await;
}
