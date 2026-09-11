//! Exercise the facade as a downstream Rust user, importing only `lobo`.
use lobo::prelude::*;

#[test]
fn order_book_adds_fills_and_cancels_using_public_types() {
    let mut book: OrderBook = OrderBook::new();
    let id = Uuid::new_v4();
    assert!(
        book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Add {
            order: LimitOrder::new(Some(100_u64), 25, Uuid::new_v4(), Side::Sell).with_uuid(id),
        })
        .is_ok()
    );
    assert!(
        book.submit::<MarketOrderData, LimitOrderData, MutatingFills>(Command::Fill {
            order: MarketOrder::new(10, Uuid::new_v4(), Side::Buy),
            execution: MutatingFills,
            reports: Reports::default(),
        })
        .is_ok()
    );
    assert_eq!(book.order_storage.asks.visible_quantity, 15);
    assert!(
        book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Cancel {
            order_id: id,
        })
        .is_ok()
    );
    assert!(book.order_storage.asks.is_empty());
}

#[test]
fn book_presets_allow_other_prices_and_publishers() {
    use lobo::events::{NullPublisher, PriceLevelChangeEvent, PublisherFactory};
    use lobo::storage::{policies::UpdateHiddenQuantity, price_level::DeepPriceLevel};

    let book: OrderBook<Price128> = OrderBook::new();
    let _: Book<DeepPriceLevel<Price128, UpdateHiddenQuantity>> = book;
    let book: ReplayBook<CompressedPrice> = ReplayBook::new();
    let book = book.with_publisher(NullPublisher);
    assert!(book.order_storage.bids.is_empty());
    fn accepts_publisher<T: PublisherFactory<PriceLevelChangeEvent<Price64>>>() {}
    accepts_publisher::<NullPublisher>();
}

#[cfg(feature = "replay")]
#[test]
fn replay_applies_typed_events_through_the_facade() {
    use lobo::replay::custom::{
        AdaptForReplay,
        orders::{AddOrder, CancelOrder, RemoveOrder},
    };
    let mut book: ReplayBook = ReplayBook::new();
    let id = Uuid::new_v4();
    assert!(
        AddOrder {
            timestamp: 1,
            id,
            side: Side::Buy,
            quantity: 12,
            price: Price64::from(100_u64)
        }
        .process(&mut book)
        .is_ok()
    );
    assert!(
        CancelOrder {
            timestamp: 2,
            id,
            quantity: 5
        }
        .process(&mut book)
        .is_ok()
    );
    assert_eq!(book.order_storage.bids.visible_quantity, 7);
    assert!(RemoveOrder { timestamp: 3, id }.process(&mut book).is_ok());
    assert!(book.order_storage.bids.is_empty());
}

#[cfg(feature = "kraken")]
#[test]
fn kraken_adapter_is_available_without_importing_implementation_crates() {
    use lobo::adapters::{adapter::MarketDataAdapter, kraken::Kraken};
    let mut adapter = Kraken::new("BTC/USD", 10).unwrap();
    adapter.connected().unwrap();
    assert!(!adapter.commands().is_empty());
}
