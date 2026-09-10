#![cfg(feature = "itchy")]
use lobo_adapters::{
    adapter::AdaptForReplay,
    itch::messages::{ItchAdd, ItchCancel, ItchDelete, ItchExecute},
};
use lobo_books::price_time_priority::{Book, Command};
use lobo_events::{
    MpscPublisher, MutationPublisher, NullPublisher, PriceChangeEvent, PriceLevelChangeEvent,
    PublisherFactory, Publishers, Receiver, TradedVolumeEvent,
};
use lobo_models::{
    Side,
    events::Reports,
    orders::order_types::{LimitOrderData, MarketOrder, MarketOrderData},
};
use lobo_primitives::{Price64, uuid::Uuid};
use lobo_storage::{
    DoNotUpdateUserMap, MutatingFills, SimulatedFills, policies::DoNotUpdateHiddenQuantity,
    price_level::IntrusivePriceLevel, price_sorting::BTreeMapPriceSorting,
};
type Level = IntrusivePriceLevel<Price64, DoNotUpdateHiddenQuantity>;
type Routes = Publishers<
    NullPublisher,
    MpscPublisher<PriceChangeEvent<Price64>>,
    MpscPublisher<TradedVolumeEvent<Price64>>,
>;
type TestBook =
    Book<Level, BTreeMapPriceSorting, DoNotUpdateUserMap, DoNotUpdateHiddenQuantity, Routes>;
fn book() -> (
    TestBook,
    Receiver<PriceChangeEvent<Price64>>,
    Receiver<TradedVolumeEvent<Price64>>,
) {
    let (prices, price_rx) = tokio::sync::mpsc::unbounded_channel();
    let (trades, trade_rx) = tokio::sync::mpsc::unbounded_channel();
    let routes = Publishers {
        levels: NullPublisher,
        prices: MpscPublisher::new(vec![prices]),
        trades: MpscPublisher::new(vec![trades]),
    };
    (
        Book::<Level, BTreeMapPriceSorting, DoNotUpdateUserMap, DoNotUpdateHiddenQuantity>::new()
            .with_id("TEST".into())
            .with_publisher(routes),
        price_rx,
        trade_rx,
    )
}
fn add(book: &mut TestBook, id: u64, price: u32, quantity: u32, side: itchy::Side) {
    ItchAdd {
        timestamp: 0,
        reference: id,
        side,
        shares: quantity,
        price: itchy::Price4::from(price),
    }
    .process(book)
    .unwrap();
}
#[test]
fn sided_best_price_changes_include_post_mutation_data_and_empty_sides() {
    let (mut book, mut prices, mut trades) = book();
    add(&mut book, 1, 100, 5, itchy::Side::Buy);
    add(&mut book, 2, 99, 4, itchy::Side::Buy); // not best
    add(&mut book, 3, 101, 3, itchy::Side::Buy);
    add(&mut book, 4, 100, 2, itchy::Side::Buy);
    ItchCancel {
        timestamp: 0,
        reference: 3,
        shares: 1,
    }
    .process(&mut book)
    .unwrap(); // same price
    ItchDelete {
        timestamp: 0,
        reference: 3,
    }
    .process(&mut book)
    .unwrap();
    let mut seen = Vec::new();
    while let Ok(event) = prices.try_recv() {
        seen.push(*event.event());
    }
    assert_eq!(
        seen,
        [
            PriceChangeEvent {
                price: Some(100u64.into()),
                quantity: 5,
                number_of_orders: 1,
                side: Side::Buy
            },
            PriceChangeEvent {
                price: Some(101u64.into()),
                quantity: 3,
                number_of_orders: 1,
                side: Side::Buy
            },
            PriceChangeEvent {
                price: Some(100u64.into()),
                quantity: 7,
                number_of_orders: 2,
                side: Side::Buy
            },
        ]
    );
    for id in [1, 4, 2] {
        ItchDelete {
            timestamp: 0,
            reference: id,
        }
        .process(&mut book)
        .unwrap();
    }
    assert_eq!(prices.try_recv().unwrap().event().price, Some(99u64.into()));
    assert_eq!(
        *prices.try_recv().unwrap().event(),
        PriceChangeEvent {
            price: None,
            quantity: 0,
            number_of_orders: 0,
            side: Side::Buy
        }
    );
    add(&mut book, 5, 103, 2, itchy::Side::Sell);
    add(&mut book, 6, 102, 4, itchy::Side::Sell);
    add(&mut book, 7, 104, 6, itchy::Side::Sell);
    assert_eq!(
        prices.try_recv().unwrap().event().price,
        Some(103u64.into())
    );
    assert_eq!(
        prices.try_recv().unwrap().event().price,
        Some(102u64.into())
    );
    assert!(prices.try_recv().is_err());
    assert!(trades.try_recv().is_err());
}
#[test]
fn execute_reports_actual_prices_and_cancels_do_not_report_trades() {
    let (mut book, _, mut trades) = book();
    add(&mut book, 1, 100, 10, itchy::Side::Sell);
    ItchCancel {
        timestamp: 0,
        reference: 1,
        shares: 2,
    }
    .process(&mut book)
    .unwrap();
    assert!(trades.try_recv().is_err());
    ItchExecute {
        timestamp: 12_345,
        reference: 1,
        shares: 3,
        execution_price: None,
    }
    .process(&mut book)
    .unwrap();
    ItchExecute {
        timestamp: 12_346,
        reference: 1,
        shares: 5,
        execution_price: Some(99),
    }
    .process(&mut book)
    .unwrap();
    let first = trades.try_recv().unwrap();
    let second = trades.try_recv().unwrap();
    assert_eq!(first.timestamp_ns(), 12_345);
    assert_eq!(second.timestamp_ns(), 12_346);
    assert_eq!(
        *first.event(),
        TradedVolumeEvent {
            price: 100u64.into(),
            quantity: 3,
            side: Side::Sell
        }
    );
    assert_eq!(
        *second.event(),
        TradedVolumeEvent {
            price: 99u64.into(),
            quantity: 5,
            side: Side::Sell
        }
    );
    assert!(trades.try_recv().is_err());
}
#[test]
fn native_fill_emits_each_executed_price_without_optional_reports() {
    let (mut book, _, mut trades) = book();
    add(&mut book, 1, 100, 3, itchy::Side::Sell);
    add(&mut book, 2, 101, 5, itchy::Side::Sell);
    let before = book.sequence();
    assert!(
        book.submit::<MarketOrderData, LimitOrderData, SimulatedFills>(Command::Fill {
            order: MarketOrder::new(8, Uuid::nil(), Side::Buy),
            execution: SimulatedFills,
            reports: Reports::default(),
        })
        .is_ok()
    );
    assert_eq!(book.sequence(), before);
    assert!(trades.try_recv().is_err());
    assert!(
        book.submit::<MarketOrderData, LimitOrderData, MutatingFills>(Command::Fill {
            order: MarketOrder::new(8, Uuid::nil(), Side::Buy),
            execution: MutatingFills,
            reports: Reports::default(),
        })
        .is_ok()
    );
    assert_eq!(
        *trades.try_recv().unwrap().event(),
        TradedVolumeEvent {
            price: 100u64.into(),
            quantity: 3,
            side: Side::Sell
        }
    );
    assert_eq!(
        *trades.try_recv().unwrap().event(),
        TradedVolumeEvent {
            price: 101u64.into(),
            quantity: 5,
            side: Side::Sell
        }
    );
    assert!(trades.try_recv().is_err());
}
#[test]
fn unsupported_types_do_not_evaluate_observations_or_event_construction() {
    let (sender, _) = tokio::sync::mpsc::unbounded_channel();
    let mut book = Book::<Level>::new().with_publisher(MpscPublisher::<
        PriceLevelChangeEvent<Price64>,
    >::new(vec![sender]));
    let (_, mut publisher) = book.storage_and_publisher();
    assert_eq!(
        publisher.observe_price(|| panic!("best-price scan ran")),
        None
    );
    assert_eq!(
        publisher.observe_trade(|| panic!("trade observation ran")),
        0
    );
    publisher.price_change(|| panic!("constructed unsupported price event"));
    publisher.traded_volume(|| panic!("constructed unsupported trade event"));
    let _: () = <NullPublisher as PublisherFactory<TradedVolumeEvent<Price64>>>::observe(
        &NullPublisher,
        || panic!("null observation ran"),
    );
}

#[tokio::test]
async fn volume_sink_declares_inputs_and_connects_at_context_creation() {
    use lobo_batchers::{VolumeBars, traits::Sink};
    use lobo_context::Context;
    use lobo_events::{BookEvent, VolumeBar};
    use lobo_replay::ReplayContext;
    use std::{
        convert::Infallible,
        num::NonZeroU64,
        sync::{Arc, Mutex},
    };
    #[derive(Clone, Default)]
    struct Output(Arc<Mutex<Vec<BookEvent<VolumeBar<Price64>>>>>);
    impl Sink<BookEvent<VolumeBar<Price64>>> for Output {
        type SinkResult = ();
        type SinkError = Infallible;
        fn write(&mut self, event: &BookEvent<VolumeBar<Price64>>) -> Result<(), Infallible> {
            self.0.lock().unwrap().push(event.clone());
            Ok(())
        }
        fn flush(&mut self) -> Result<(), Infallible> {
            Ok(())
        }
        fn finish(&mut self) -> Result<(), Infallible> {
            Ok(())
        }
    }
    let output = Output::default();
    let sink = VolumeBars::new(NonZeroU64::new(10).unwrap(), output.clone());
    let mut context = ReplayContext::<
        Level,
        BTreeMapPriceSorting,
        DoNotUpdateUserMap,
        DoNotUpdateHiddenQuantity,
    >::with_sinks(None, sink)
    .unwrap();
    // Only the transform's required typed inputs are enabled.
    let _: () = <_ as PublisherFactory<PriceLevelChangeEvent<Price64>>>::observe(
        context.publisher_factory(),
        || panic!("raw levels enabled by volume sink"),
    );
    let mut book =
        Book::<Level, BTreeMapPriceSorting, DoNotUpdateUserMap, DoNotUpdateHiddenQuantity>::new()
            .with_id("TEST".into())
            .with_publisher(context.publisher_factory().clone());
    ItchAdd {
        timestamp: 0,
        reference: 1,
        side: itchy::Side::Sell,
        shares: 35,
        price: itchy::Price4::from(100),
    }
    .process(&mut book)
    .unwrap();
    ItchCancel {
        timestamp: 0,
        reference: 1,
        shares: 5,
    }
    .process(&mut book)
    .unwrap();
    ItchExecute {
        timestamp: 0,
        reference: 1,
        shares: 30,
        execution_price: None,
    }
    .process(&mut book)
    .unwrap();
    context.books.insert("test".into(), book);
    context.finish().await.unwrap();
    let bars = output.0.lock().unwrap();
    assert_eq!(bars.len(), 3);
    assert!(bars.iter().all(|bar| bar.book_id() == "TEST"
        && bar.event().volume == 10
        && bar.event().open == 100u64.into()));
}
