#![cfg(all(feature = "json", feature = "native"))]
#[path = "support/definitions.rs"]
mod definitions;
use lobo_replay::custom::{
    CustomAdapter, MarketDataAdapter,
    definition::DefinedProtocol,
    observer::{self, ObservedProtocol, channel::Publisher},
};
use serde_json::json;

#[path = "support/itch.rs"]
mod itch_fixture;

#[test]
fn hosted_replay_observer_keeps_source_clock_and_partial_state() {
    let mut descriptor = definitions::descriptor("server", "BOOK");
    descriptor.mode = lobo_replay::custom::FeedMode::Replay;
    let mut viewer = CustomAdapter::new(
        descriptor.clone(),
        ObservedProtocol::new(descriptor, "BOOK").unwrap(),
        "BOOK",
    )
    .unwrap();
    let origin = 34_200_000_000_000u64;
    for (sequence, clock, complete) in [
        (1, origin + 1_000_000_000, false),
        (2, origin, false),
        (3, origin + 3_000_000_000, true),
    ] {
        let packet = json!({"type":"snapshot","sequence":sequence,"instruments":[{"symbol":"BOOK","price_decimals":0,"quantity_decimals":0,"policy":"full"}],"actions":[
            observer::book("BOOK", vec![json!({"action":"level","side":observer::literal("buy"),"price":observer::literal(100),"quantity":observer::literal(sequence)})], true, clock, false, 0)
        ],"start_ns":origin,"clock_ns":clock,"messages":sequence,"consumed":sequence,"complete":complete});
        viewer
            .receive(packet.to_string().as_bytes(), false)
            .unwrap();
        assert!(!viewer.state().synchronized("BOOK"));
        assert!(!viewer.state().warming);
        viewer.advance(u64::MAX, 1000).unwrap();
        assert_eq!(viewer.state().clock_ns, clock);
        assert_eq!(viewer.state().complete, complete);
        assert!(!viewer.state().synchronized("BOOK"));
    }
}

#[test]
fn unvisited_instruments_survive_observer_snapshots_without_allocating_books() {
    use lobo_context::BookScope;
    use lobo_models::BookPolicy;
    use lobo_replay::custom::Instrument;
    let info = definitions::descriptor("server", "ALPHA");
    let protocol =
        DefinedProtocol::new(definitions::definition("server"), info.clone(), "ALPHA").unwrap();
    let mut source = CustomAdapter::new(info.clone(), protocol, "ALPHA").unwrap();
    for (symbol, policy) in [("ALPHA", BookPolicy::Full), ("BETA", BookPolicy::NoUpdates)] {
        source
            .register_instrument(
                Instrument {
                    symbol: symbol.into(),
                    price_decimals: 6,
                    quantity_decimals: 6,
                },
                policy,
            )
            .unwrap();
    }
    let mut viewer = CustomAdapter::new(
        info.clone(),
        ObservedProtocol::new(info, "BETA").unwrap(),
        "BETA",
    )
    .unwrap()
    .with_scope(BookScope::Selected(["BETA".into()].into()))
    .unwrap();
    let initial = observer::snapshot(&source, 0).unwrap();
    assert_eq!(initial["instruments"].as_array().unwrap().len(), 2);
    assert!(initial["actions"].as_array().unwrap().is_empty());
    viewer
        .receive(initial.to_string().as_bytes(), false)
        .unwrap();
    assert_eq!(viewer.tickers(), ["ALPHA", "BETA"]);
    assert_eq!(viewer.scoped_tickers(), ["BETA"]);
    assert!(viewer.state().context.books.is_empty());
    assert_eq!(viewer.state().context.policy("ALPHA"), BookPolicy::Full);
    assert_eq!(viewer.state().context.policy("BETA"), BookPolicy::NoUpdates);
    assert!(viewer.select_ticker("ALPHA").is_err());
    viewer.select_ticker("BETA").unwrap();
    viewer.state().outputs["BETA"].borrow_mut().generation = 42;
    viewer
        .receive(initial.to_string().as_bytes(), false)
        .unwrap();
    assert_eq!(viewer.state().outputs["BETA"].borrow().generation, 42);
}

#[test]
fn binary_orders_create_observer_books_without_an_exchange_snapshot() {
    let file = itch_fixture::file();
    let info = definitions::descriptor("itch", "ALPHA");
    let publisher = Publisher::new(64);
    let mut events = publisher.events.subscribe();
    let protocol = DefinedProtocol::with_observer(
        definitions::definition("itch"),
        info.clone(),
        "ALPHA",
        publisher.sink(),
    )
    .unwrap();
    let mut source = CustomAdapter::new(info.clone(), protocol, "ALPHA").unwrap();
    let mut viewer = CustomAdapter::new(
        info.clone(),
        ObservedProtocol::new(info, "ALPHA").unwrap(),
        "ALPHA",
    )
    .unwrap();
    viewer
        .receive(
            observer::snapshot(&source, 0)
                .unwrap()
                .to_string()
                .as_bytes(),
            false,
        )
        .unwrap();
    source
        .receive(&std::fs::read(file.0.clone()).unwrap(), true)
        .unwrap();
    while !source.state().complete {
        source.advance(u64::MAX, 1).unwrap();
        while let Ok(event) = events.try_recv() {
            viewer.receive(event.to_string().as_bytes(), false).unwrap();
        }
        assert_eq!(
            observer::snapshot(&source, 0).unwrap()["actions"],
            observer::snapshot(&viewer, 0).unwrap()["actions"]
        );
    }
    assert!(viewer.state().book("ALPHA").is_some());
    assert!(viewer.state().synchronized("ALPHA"));
}

#[test]
fn streamed_operations_and_reconnect_snapshot_preserve_fifo_and_hidden_quantity() {
    let descriptor = definitions::descriptor("server", "BOOK");
    let publisher = Publisher::new(64);
    let mut events = publisher.events.subscribe();
    let protocol = DefinedProtocol::with_observer(
        definitions::definition("server"),
        descriptor.clone(),
        "BOOK",
        publisher.sink(),
    )
    .unwrap();
    let mut source = CustomAdapter::new(descriptor.clone(), protocol, "BOOK").unwrap();
    let mut viewer = CustomAdapter::new(
        descriptor.clone(),
        ObservedProtocol::new(descriptor, "BOOK").unwrap(),
        "BOOK",
    )
    .unwrap();
    let initial = observer::snapshot(&source, publisher.sequence()).unwrap();
    viewer
        .receive(initial.to_string().as_bytes(), false)
        .unwrap();
    let book = json!({"symbol":"BOOK","price_decimals":4,"quantity_decimals":0,"policy":"full"});
    let iceberg = json!({"type":"iceberg","id":"00000000-0000-0000-0000-000000000001","side":"buy","quantity":10,"price":10000,"hidden_quantity":20,"peak_quantity":10});
    let limit = json!({"type":"limit","id":"00000000-0000-0000-0000-000000000002","side":"buy","quantity":25,"price":10000});
    for message in [
        json!({"type":"directory","books":[book.clone()]}),
        json!({"type":"snapshot","book":book,"sequence":0,"timestamp_ns":100,"orders":[]}),
        json!({"type":"update","book":"BOOK","sequence":1,"timestamp_ns":101,"command":{"op":"add","order":iceberg}}),
        json!({"type":"update","book":"BOOK","sequence":2,"timestamp_ns":102,"command":{"op":"add","order":limit}}),
        json!({"type":"update","book":"BOOK","sequence":3,"timestamp_ns":103,"command":{"op":"execute","id":"00000000-0000-0000-0000-000000000001","quantity":10}}),
    ] {
        source
            .receive(message.to_string().as_bytes(), false)
            .unwrap();
        while let Ok(event) = events.try_recv() {
            viewer.receive(event.to_string().as_bytes(), false).unwrap();
        }
        let source_book = source.state().book("BOOK");
        let viewer_book = viewer.state().book("BOOK");
        assert_eq!(
            source_book.map(|b| b.order_count()),
            viewer_book.map(|b| b.order_count())
        );
        if let (Some(a), Some(b)) = (source_book, viewer_book) {
            for side in [lobo_models::Side::Buy, lobo_models::Side::Sell] {
                assert_eq!(
                    format!("{:?}", a.queue_view(side, 0u64.into()..=u64::MAX.into())),
                    format!("{:?}", b.queue_view(side, 0u64.into()..=u64::MAX.into()))
                );
            }
        }
    }
    let snapshot = observer::snapshot(&source, publisher.sequence()).unwrap();
    viewer
        .receive(snapshot.to_string().as_bytes(), false)
        .unwrap();
    let a = source.state().book("BOOK").unwrap();
    let b = viewer.state().book("BOOK").unwrap();
    assert_eq!(
        format!(
            "{:?}",
            a.queue_view(lobo_models::Side::Buy, 0u64.into()..=u64::MAX.into())
        ),
        format!(
            "{:?}",
            b.queue_view(lobo_models::Side::Buy, 0u64.into()..=u64::MAX.into())
        )
    );
    assert_eq!(
        observer::snapshot(&source, 0).unwrap()["actions"],
        observer::snapshot(&viewer, 0).unwrap()["actions"]
    );
    // An update already included in that snapshot must not execute twice.
    viewer
        .receive(
            json!({"type":"update","sequence":publisher.sequence(),"actions":[]})
                .to_string()
                .as_bytes(),
            false,
        )
        .unwrap();
    assert!(
        viewer
            .receive(
                json!({"type":"update","sequence":publisher.sequence()+2,"actions":[]})
                    .to_string()
                    .as_bytes(),
                false
            )
            .is_err()
    );
}
