use lobo_models::{
    Side,
    events::Reports,
    orders::{
        order_types::{IcebergOrder, LimitOrder, MarketOrder},
        traits::Trades,
    },
};
use lobo_primitives::{Price64, time::DateTime, uuid::Uuid};
use lobo_storage::{
    DoNotUpdateUserMap, MutatingFills, OrderStorage, SimulatedFills, UpdateUserMap,
    UserMapUpdatePolicy,
    policies::{DoNotUpdateHiddenQuantity, UpdateHiddenQuantity},
    price_level::{DeepPriceLevel, IntrusivePriceLevel, PriceLevelContract},
    price_sorting::{BTreeMapPriceSorting, PriceSortingPolicy, SortedVectorPriceSorting},
};

fn exercise<L: PriceLevelContract<Price = Price64>, Sort: PriceSortingPolicy>(side: Side) {
    let mut storage = OrderStorage::<L, Sort>::new();
    let mut ids = Vec::new();
    for (price, time, quantity) in [
        (101u64, 10, 10),
        (100, 20, 20),
        (101, 30, 30),
        (100, 20, 40),
        (102, 5, 50),
    ] {
        let order = LimitOrder::new(Some(price), quantity, Uuid::nil(), side)
            .with_creation_time(DateTime::from_timestamp_nanos(time));
        ids.push(order.uuid());
        storage.add_order(order, &mut |_| {}).unwrap();
    }
    let range = Price64::from(100u64)..=Price64::from(101u64);
    let view = storage.queue_view(side, range.clone());
    let priority = if side == Side::Buy {
        [0, 2, 1, 3]
    } else {
        [1, 3, 0, 2]
    };
    assert_eq!(
        view.iter().map(|o| o.id).collect::<Vec<_>>(),
        priority.map(|index| ids[index])
    );
    assert_eq!(
        view.iter().map(|o| o.quantity).collect::<Vec<_>>(),
        priority.map(|index| [10, 20, 30, 40][index])
    );
    assert_eq!(
        view.iter().map(|o| u64::from(o.price)).collect::<Vec<_>>(),
        priority.map(|index| [101, 100, 101, 100][index])
    );
    assert_eq!(
        view[0].created_at.timestamp_nanos_opt(),
        Some(if side == Side::Buy { 10 } else { 20 })
    );
    assert!(
        storage
            .queue_view(side, Price64::from(101u64)..=Price64::from(100u64))
            .is_empty()
    );
    storage.remove_order(ids[1], &mut |_| {}).unwrap();
    assert_eq!(
        storage
            .queue_view(side, range.clone())
            .iter()
            .map(|o| o.id)
            .collect::<Vec<_>>(),
        priority
            .into_iter()
            .filter(|index| *index != 1)
            .map(|index| ids[index])
            .collect::<Vec<_>>()
    );
    storage.remove_order(ids[4], &mut |_| {}).unwrap();
    let taker_side = if side == Side::Buy {
        Side::Sell
    } else {
        Side::Buy
    };
    let reports = Reports {
        include_fills: true,
        ..Reports::default()
    };
    let result = storage.submit_order_with_reports(
        MarketOrder::new(5, Uuid::nil(), taker_side),
        MutatingFills,
        reports,
        &mut |_| {},
    );
    let expected_maker = if side == Side::Buy { ids[0] } else { ids[3] };
    assert_eq!(
        result.report.unwrap().fills.unwrap()[0].maker_order_id,
        expected_maker
    );
    let view = storage.queue_view(side, range);
    assert_eq!(
        view.iter()
            .find(|o| o.id == expected_maker)
            .unwrap()
            .quantity,
        if side == Side::Buy { 5 } else { 35 }
    );
}
fn both<L: PriceLevelContract<Price = Price64>, Sort: PriceSortingPolicy>() {
    exercise::<L, Sort>(Side::Buy);
    exercise::<L, Sort>(Side::Sell);
}
#[test]
fn selected_ranges_preserve_price_then_fifo_priority_and_follow_native_mutations() {
    both::<IntrusivePriceLevel<Price64, UpdateHiddenQuantity>, BTreeMapPriceSorting>();
    both::<IntrusivePriceLevel<Price64, UpdateHiddenQuantity>, SortedVectorPriceSorting>();
    both::<DeepPriceLevel<Price64, UpdateHiddenQuantity>, BTreeMapPriceSorting>();
    both::<DeepPriceLevel<Price64, UpdateHiddenQuantity>, SortedVectorPriceSorting>();
}

fn replenishment<
    L: PriceLevelContract<Price = Price64>,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
>(
    side: Side,
    supports_hidden: bool,
) {
    let mut storage = OrderStorage::<L, Sort, U, L::HiddenQuantityUpdater>::default();
    let iceberg = IcebergOrder::new(Some(100u64), Uuid::nil(), side, 6, 3)
        .with_creation_time(DateTime::from_timestamp_nanos(10));
    let limit = LimitOrder::new(Some(100u64), 4, Uuid::nil(), side)
        .with_creation_time(DateTime::from_timestamp_nanos(20));
    let other_iceberg = IcebergOrder::new(Some(100u64), Uuid::nil(), side, 4, 2)
        .with_creation_time(DateTime::from_timestamp_nanos(30));
    let later = LimitOrder::new(Some(100u64), 2, Uuid::nil(), side)
        .with_creation_time(DateTime::from_timestamp_nanos(5));
    let ids = [
        iceberg.uuid(),
        limit.uuid(),
        other_iceberg.uuid(),
        later.uuid(),
    ];
    storage.add_order(iceberg, &mut |_| {}).unwrap();
    storage.add_order(limit, &mut |_| {}).unwrap();
    storage.add_order(other_iceberg, &mut |_| {}).unwrap();
    let taker_side = if side == Side::Buy {
        Side::Sell
    } else {
        Side::Buy
    };
    let reports = Reports {
        include_fills: true,
        ..Reports::default()
    };
    let range = Price64::from(100u64)..=Price64::from(100u64);
    let queue = |storage: &OrderStorage<L, Sort, U, L::HiddenQuantityUpdater>| {
        storage
            .queue_view(side, range.clone())
            .into_iter()
            .map(|order| {
                assert!(
                    storage.order(order.id).is_some(),
                    "replenished order lost its native index"
                );
                (
                    order.id,
                    order.quantity,
                    order.created_at.timestamp_nanos_opt().unwrap(),
                )
            })
            .collect::<Vec<_>>()
    };
    let consume = |storage: &mut OrderStorage<L, Sort, U, L::HiddenQuantityUpdater>, quantity| {
        storage
            .submit_order_with_reports(
                MarketOrder::new(quantity, Uuid::nil(), taker_side),
                MutatingFills,
                reports,
                &mut |_| {},
            )
            .report
            .unwrap()
            .fills
            .unwrap()
            .into_iter()
            .map(|fill| (fill.maker_order_id, fill.fill_quantity))
            .collect::<Vec<_>>()
    };
    assert_eq!(consume(&mut storage, 3), [(ids[0], 3)]);
    if !supports_hidden {
        assert_eq!(queue(&storage), [(ids[1], 4, 20), (ids[2], 2, 30)]);
        assert!(storage.order(ids[0]).is_none());
        storage.add_order(later, &mut |_| {}).unwrap();
        assert_eq!(
            consume(&mut storage, 8),
            [(ids[1], 4), (ids[2], 2), (ids[3], 2)]
        );
        assert!(queue(&storage).is_empty());
        assert!(storage.order_to_arena_map.is_empty());
        return;
    }
    // Replenishment loses priority even though the original creation time stays.
    assert_eq!(
        queue(&storage),
        [(ids[1], 4, 20), (ids[2], 2, 30), (ids[0], 3, 10)]
    );
    storage.add_order(later, &mut |_| {}).unwrap();
    // A backdated incoming order still joins the native tail.
    let before = queue(&storage);
    assert_eq!(before.last(), Some(&(ids[3], 2, 5)));
    let sweep = storage
        .submit_order_with_reports(
            MarketOrder::new(18, Uuid::nil(), taker_side),
            SimulatedFills,
            reports,
            &mut |_| panic!("simulation changed a level"),
        )
        .report
        .unwrap()
        .fills
        .unwrap();
    assert_eq!(queue(&storage), before);
    assert_eq!(
        sweep
            .iter()
            .map(|fill| (fill.maker_order_id, fill.fill_quantity, fill.maker_depleted))
            .collect::<Vec<_>>(),
        [
            (ids[1], 4, true),
            (ids[2], 2, false),
            (ids[0], 3, false),
            (ids[3], 2, true),
            (ids[2], 2, false),
            (ids[0], 3, true),
            (ids[2], 2, true),
        ]
    );
    let preview = storage
        .submit_order_with_reports(
            MarketOrder::new(7, Uuid::nil(), taker_side),
            SimulatedFills,
            reports,
            &mut |_| panic!("simulation changed a level"),
        )
        .report
        .unwrap()
        .fills
        .unwrap()
        .into_iter()
        .map(|fill| (fill.maker_order_id, fill.fill_quantity))
        .collect::<Vec<_>>();
    assert_eq!(queue(&storage), before);
    assert_eq!(preview, [(ids[1], 4), (ids[2], 2), (ids[0], 1)]);
    assert_eq!(consume(&mut storage, 7), preview);
    assert_eq!(
        queue(&storage),
        [(ids[0], 2, 10), (ids[3], 2, 5), (ids[2], 2, 30)]
    );
    assert_eq!(consume(&mut storage, 4), [(ids[0], 2), (ids[3], 2)]);
    assert_eq!(queue(&storage), [(ids[2], 2, 30), (ids[0], 3, 10)]);
    assert_eq!(consume(&mut storage, 2), [(ids[2], 2)]);
    assert_eq!(queue(&storage), [(ids[0], 3, 10), (ids[2], 2, 30)]);
    assert_eq!(consume(&mut storage, 5), [(ids[0], 3), (ids[2], 2)]);
    assert!(queue(&storage).is_empty());
    assert!(storage.order_to_arena_map.is_empty());
}

macro_rules! replenishment_test {
    ($name:ident, $level:ident, $sort:ty) => {
        #[test]
        fn $name() {
            for side in [Side::Buy, Side::Sell] {
                replenishment::<$level<Price64, UpdateHiddenQuantity>, $sort, UpdateUserMap>(
                    side, true,
                );
                replenishment::<$level<Price64, UpdateHiddenQuantity>, $sort, DoNotUpdateUserMap>(
                    side, true,
                );
                replenishment::<$level<Price64, DoNotUpdateHiddenQuantity>, $sort, UpdateUserMap>(
                    side, false,
                );
                replenishment::<
                    $level<Price64, DoNotUpdateHiddenQuantity>,
                    $sort,
                    DoNotUpdateUserMap,
                >(side, false);
            }
        }
    };
}
replenishment_test!(
    deep_vector_replenishment_loses_priority,
    DeepPriceLevel,
    SortedVectorPriceSorting
);
replenishment_test!(
    deep_btree_replenishment_loses_priority,
    DeepPriceLevel,
    BTreeMapPriceSorting
);
replenishment_test!(
    intrusive_vector_replenishment_loses_priority,
    IntrusivePriceLevel,
    SortedVectorPriceSorting
);
replenishment_test!(
    intrusive_btree_replenishment_loses_priority,
    IntrusivePriceLevel,
    BTreeMapPriceSorting
);
