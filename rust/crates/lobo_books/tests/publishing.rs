use lobo_books::price_time_priority::{Book, Command};
use lobo_events::{MpscPublisher, PriceLevelChangeEvent};
use lobo_models::{
    Side,
    events::Reports,
    orders::order_types::{
        IcebergOrder, IcebergOrderData, LimitOrder, LimitOrderData, MarketOrder, MarketOrderData,
    },
};
use lobo_primitives::{Price64, uuid::Uuid};
use lobo_storage::{
    MutatingFills, SimulatedFills,
    policies::UpdateHiddenQuantity,
    price_level::{DeepPriceLevel, IntrusivePriceLevel, PriceLevelContract},
};
use tokio::sync::mpsc;

fn fill_events<L: PriceLevelContract<Price = Price64>>() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut book = Book::<L>::new()
        .with_id("test-book".into())
        .with_publisher(MpscPublisher::new(vec![tx]));
    book.submit::<IcebergOrderData, IcebergOrderData, MutatingFills>(Command::Add {
        order: IcebergOrder::new(Some(100), Uuid::nil(), Side::Sell, 4, 2),
    })
    .ok()
    .unwrap();
    book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Add {
        order: LimitOrder::new(Some(100), 3, Uuid::nil(), Side::Sell),
    })
    .ok()
    .unwrap();
    book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Add {
        order: LimitOrder::new(Some(101), 2, Uuid::nil(), Side::Sell),
    })
    .ok()
    .unwrap();
    // Simulation produces no level events and never advances the sequence.
    book.submit::<MarketOrderData, LimitOrderData, SimulatedFills>(Command::Fill {
        order: MarketOrder::new(10, Uuid::nil(), Side::Buy),
        execution: SimulatedFills,
        reports: Reports::default(),
    })
    .ok()
    .unwrap();
    assert_eq!(book.sequence(), 3);
    book.submit::<MarketOrderData, LimitOrderData, MutatingFills>(Command::Fill {
        order: MarketOrder::new(10, Uuid::nil(), Side::Buy),
        execution: MutatingFills,
        reports: Reports::default(),
    })
    .ok()
    .unwrap();
    let expected = [
        (100, 2, 4, 1),
        (100, 5, 4, 2),
        (101, 2, 0, 1),
        // Replenished iceberg, next maker removed, replenished again,
        // exhausted iceberg, then a partial fill at the next price.
        (100, 5, 2, 2),
        (100, 2, 2, 1),
        (100, 2, 0, 1),
        (100, 0, 0, 0),
        (101, 1, 0, 1),
    ];
    for (i, (price, visible, hidden, count)) in expected.into_iter().enumerate() {
        let event = rx.try_recv().unwrap();
        assert_eq!(event.book_id(), "test-book");
        assert_eq!(event.sequence_number(), i as u64 + 1);
        assert_eq!(
            *event.event(),
            PriceLevelChangeEvent::new(
                visible,
                hidden,
                Price64::from(price as u32),
                count,
                Side::Sell
            )
        );
    }
    assert!(rx.try_recv().is_err());
    assert_eq!(book.sequence(), 8);
}

#[test]
fn deep_fills_publish_completed_levels_once_per_maker_mutation() {
    fill_events::<DeepPriceLevel<Price64, UpdateHiddenQuantity>>();
}

#[test]
fn intrusive_fills_publish_completed_levels_once_per_maker_mutation() {
    fill_events::<IntrusivePriceLevel<Price64, UpdateHiddenQuantity>>();
}

#[test]
fn default_book_never_stamps_messages() {
    let mut book = Book::<DeepPriceLevel<Price64, UpdateHiddenQuantity>>::new();
    book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Add {
        order: LimitOrder::new(Some(100), 3, Uuid::nil(), Side::Buy),
    })
    .ok()
    .unwrap();
    book.publish(PriceLevelChangeEvent::new(
        3,
        0,
        Price64::from(100_u32),
        1,
        Side::Buy,
    ));
    assert_eq!(book.sequence(), 0);
    assert_eq!(book.order_storage.bids.visible_quantity, 3);
}
