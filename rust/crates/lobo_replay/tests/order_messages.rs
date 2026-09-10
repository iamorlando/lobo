#![cfg(feature = "order-api")]
use lobo_books::price_time_priority::{Book, CommandResult};
use lobo_events::{MpscPublisher, NullPublisher, Publishers, TradedVolumeEvent};
use lobo_models::{
    Side,
    events::Reports,
    server::{Command, IcebergOrder, LimitOrder, MarketOrder, Order, OrderFields},
};
use lobo_primitives::{CompressedPrice, uuid::Uuid};
use lobo_replay::order_messages::ApplyOrderCommand;
use lobo_storage::{
    UpdateUserMap, policies::UpdateHiddenQuantity, price_level::DeepPriceLevel,
    price_sorting::SortedVectorPriceSorting,
};
type NativeBook = Book<
    DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>,
    SortedVectorPriceSorting,
    UpdateUserMap,
    UpdateHiddenQuantity,
>;
const REPORTS: Reports = Reports {
    include_fills: true,
    include_summary: false,
    include_market_impact: false,
};
fn fields(id: u128, side: Side, quantity: u64) -> OrderFields {
    OrderFields {
        id: Uuid::from_u128(id),
        trader: Uuid::nil(),
        side,
        quantity,
    }
}
fn add(id: u128, side: Side, price: u32, quantity: u64) -> Command {
    Command::Add {
        order: Order::Limit(LimitOrder {
            fields: fields(id, side, quantity),
            price,
        }),
    }
}

#[test]
fn direct_json_commands_follow_native_mutations_and_execute_alone_publishes_trades() {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut book = NativeBook::new().with_publisher(Publishers {
        levels: NullPublisher,
        prices: NullPublisher,
        trades: MpscPublisher::<TradedVolumeEvent<CompressedPrice>>::new(vec![sender]),
    });
    let request = r#"{"op":"add","order":{"type":"limit","id":"00000000-0000-0000-0000-000000000001","side":"buy","price":100,"quantity":10}}"#;
    let command: Command = serde_json::from_str(request).unwrap();
    command.apply(&mut book, 1, REPORTS).unwrap();
    Command::Cancel {
        id: Uuid::from_u128(1),
        quantity: 2,
    }
    .apply(&mut book, 2, REPORTS)
    .unwrap();
    assert!(receiver.try_recv().is_err());
    let execution = Command::Execute {
        id: Uuid::from_u128(1),
        quantity: 3,
        price: Some(101),
    }
    .apply(&mut book, 3, REPORTS)
    .unwrap();
    assert_eq!(receiver.try_recv().unwrap().event().quantity, 3);
    let CommandResult::Filled {
        report: Some(report),
        ..
    } = execution
    else {
        panic!("execution report missing")
    };
    assert_eq!(
        report.fills.unwrap()[0].price,
        CompressedPrice::from(101u32)
    );
    Command::Modify {
        id: Uuid::from_u128(1),
        quantity: 6,
        price: Some(99),
        new_id: Some(Uuid::from_u128(2)),
    }
    .apply(&mut book, 4, REPORTS)
    .unwrap();
    assert_eq!(book.order_storage.bids.visible_quantity, 6);
    assert!(book.order_storage.order(Uuid::from_u128(1)).is_none());
    Command::Remove {
        id: Uuid::from_u128(2),
    }
    .apply(&mut book, 5, REPORTS)
    .unwrap();
    assert_eq!(book.order_storage.bids.visible_quantity, 0);
    assert!(receiver.try_recv().is_err());
}

#[test]
fn native_iceberg_replenishment_rejoins_fifo_and_market_preview_preserves_state() {
    let mut book = NativeBook::new();
    Command::Add {
        order: Order::Iceberg(IcebergOrder {
            fields: fields(1, Side::Sell, 5),
            price: 100,
            hidden_quantity: 10,
            peak_quantity: 5,
        }),
    }
    .apply(&mut book, 1, REPORTS)
    .unwrap();
    add(2, Side::Sell, 100, 7)
        .apply(&mut book, 2, REPORTS)
        .unwrap();
    Command::Execute {
        id: Uuid::from_u128(1),
        quantity: 5,
        price: None,
    }
    .apply(&mut book, 3, REPORTS)
    .unwrap();
    assert_eq!(book.order_storage.asks.visible_quantity, 12);
    assert_eq!(book.order_storage.asks.hidden_quantity, 5);
    let order = Order::Market(MarketOrder {
        fields: fields(3, Side::Buy, 9),
    });
    let preview = Command::Simulate {
        order: order.clone(),
    }
    .apply(&mut book, 4, REPORTS)
    .unwrap();
    assert_eq!(book.order_storage.asks.visible_quantity, 12);
    let CommandResult::Filled {
        report: Some(preview),
        ..
    } = preview
    else {
        panic!("preview report missing")
    };
    let fills = preview.fills.unwrap();
    assert_eq!(
        fills.iter().map(|f| f.maker_order_id).collect::<Vec<_>>(),
        [Uuid::from_u128(2), Uuid::from_u128(1)]
    );
    Command::Fill { order }
        .apply(&mut book, 5, REPORTS)
        .unwrap();
    assert_eq!(book.order_storage.asks.visible_quantity, 3);
    assert_eq!(book.order_storage.asks.hidden_quantity, 5);
}

#[test]
fn invalid_ids_reductions_and_overflows_do_not_enter_unchecked_storage() {
    let mut book = NativeBook::new();
    add(1, Side::Buy, 100, u64::MAX)
        .apply(&mut book, 1, REPORTS)
        .unwrap();
    for command in [
        add(1, Side::Buy, 100, 1),
        add(2, Side::Buy, 100, 1),
        Command::Cancel {
            id: Uuid::from_u128(2),
            quantity: 1,
        },
        Command::Execute {
            id: Uuid::from_u128(1),
            quantity: 0,
            price: None,
        },
        Command::Modify {
            id: Uuid::from_u128(2),
            quantity: 1,
            price: None,
            new_id: None,
        },
    ] {
        assert!(command.apply(&mut book, 2, REPORTS).is_err());
    }
    assert_eq!(book.order_storage.bids.visible_quantity, u64::MAX);
    assert_eq!(book.order_storage.order_to_arena_map.len(), 1);
}
