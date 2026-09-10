use super::*;
use crate::{
    MutatingFills, SimulatedFills,
    policies::UpdateHiddenQuantity as Hidden,
    price_level::{DeepPriceLevel, IntrusivePriceLevel},
    price_sorting::{BTreeMapPriceSorting, SortedVectorPriceSorting},
};
use lobo_models::{
    events::{OrderDetails, Reports},
    orders::order_types::{IcebergOrder, LimitOrder, MarketOrder},
};
use lobo_primitives::{Price64, uuid::Uuid};

fn aggregate<L, Sort>()
where
    L: PriceLevelContract<Price = Price64, Checksum = Kraken>,
    Sort: PriceSortingPolicy,
{
    let mut book = OrderStorage::<L, Sort>::new();
    book.configure_checksum(
        Specification::kraken().prepare().unwrap(),
        Precision {
            price: 1,
            quantity: 2,
        },
    )
    .unwrap();
    let widths = DecimalWidths {
        price: 1,
        quantity: 2,
    };
    for price in 101u64..=111 {
        book.asks
            .set_level_quantity_formatted(Price64::from(price), 120, widths, &mut |_| {});
    }
    book.bids
        .set_level_quantity_formatted(Price64::from(99u64), 250, widths, &mut |_| {});
    let operands = (101..=110).map(|p| format!("{p}120")).collect::<String>() + "99250";
    assert_eq!(
        book.checksum().unwrap(),
        crc32fast::hash(operands.as_bytes())
    );
    assert_eq!(book.checksum.calculations, 1);
    book.checksum().unwrap();
    // A changed level outside the checksum window does not trigger another CRC.
    book.asks
        .set_level_quantity_formatted(Price64::from(111u64), 750, widths, &mut |_| {});
    assert_eq!(
        book.checksum().unwrap(),
        crc32fast::hash(operands.as_bytes())
    );
    assert_eq!(book.checksum.calculations, 1);
    // Identical numeric quantity, different wire decimal width, is significant.
    book.bids.set_level_quantity_formatted(
        Price64::from(99u64),
        250,
        DecimalWidths {
            price: 1,
            quantity: 3,
        },
        &mut |_| {},
    );
    assert_eq!(
        book.checksum().unwrap(),
        crc32fast::hash((operands.clone() + "0").as_bytes())
    );
    assert_eq!(book.checksum.calculations, 2);
    // Removal promotes the previously dirty eleventh level into the prefix.
    book.asks
        .set_level_quantity(Price64::from(101u64), 0, &mut |_| {});
    let operands = (102..=110).map(|p| format!("{p}120")).collect::<String>() + "111750992500";
    assert_eq!(
        book.checksum().unwrap(),
        crc32fast::hash(operands.as_bytes())
    );
    book.asks.retain_level_quantities(0, &mut |_| {});
    book.bids.retain_level_quantities(0, &mut |_| {});
    assert_eq!(book.checksum().unwrap(), 0);
}

fn orders<L, Sort>()
where
    L: PriceLevelContract<Price = Price64, Checksum = BitFinex>,
    Sort: PriceSortingPolicy,
    OrderStorage<L, Sort>: Clone,
{
    let mut book = OrderStorage::<L, Sort>::new();
    book.configure_checksum(
        Specification::bitfinex().prepare().unwrap(),
        Precision {
            price: 0,
            quantity: 8,
        },
    )
    .unwrap();
    for id in (1..=30).rev() {
        book.add_order(
            LimitOrder::new(Some(100u64), 100_000_000, Uuid::nil(), Side::Buy)
                .with_uuid(Uuid::from_u128(id)),
            &mut |_| {},
        )
        .unwrap();
    }
    book.add_order(
        LimitOrder::new(Some(101u64), 99, Uuid::nil(), Side::Sell).with_uuid(Uuid::from_u128(40)),
        &mut |_| {},
    )
    .unwrap();
    let operands = "1:1:40:-9.9e-7:".to_owned()
        + &(2..=25)
            .map(|id| format!("{id}:1"))
            .collect::<Vec<_>>()
            .join(":");
    assert_eq!(
        book.checksum().unwrap(),
        crc32fast::hash(operands.as_bytes())
    );
    let view = book.queue_view(Side::Buy, Price64::from(100u64)..=Price64::from(100u64));
    assert_eq!(view[0].id, Uuid::from_u128(30)); // checksum order cannot change matching order
    book.modify_order_in_place(
        Uuid::from_u128(30),
        OrderDetails {
            quantity: Some(200_000_000),
            price: None,
            side: None,
            uuid: None,
            trader: None,
            creation_time: None,
        },
        |order, details| order.update_in_place(details),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(
        book.checksum().unwrap(),
        crc32fast::hash(operands.as_bytes())
    );
    assert_eq!(book.checksum.calculations, 1);
    book.remove_order(Uuid::from_u128(1), &mut |_| {}).unwrap();
    assert_ne!(
        book.checksum().unwrap(),
        crc32fast::hash(operands.as_bytes())
    );
    let unchanged = book.checksum().unwrap();
    book.submit_order_with_reports(
        MarketOrder::new(50, Uuid::nil(), Side::Buy),
        SimulatedFills,
        Reports::default(),
        &mut |_| {},
    );
    assert_eq!(book.checksum().unwrap(), unchanged);
    book.submit_order_with_reports(
        MarketOrder::new(50, Uuid::nil(), Side::Buy),
        MutatingFills,
        Reports::default(),
        &mut |_| {},
    );
    assert_ne!(book.checksum().unwrap(), unchanged);
    let mut fork = book.clone();
    fork.remove_order(Uuid::from_u128(40), &mut |_| {}).unwrap();
    assert_ne!(fork.checksum().unwrap(), book.checksum().unwrap());
}

fn replenishment<L, Sort>()
where
    L: PriceLevelContract<Price = Price64, Checksum = BitFinex>,
    Sort: PriceSortingPolicy,
{
    let mut book = OrderStorage::<L, Sort>::new();
    book.add_order(
        IcebergOrder::new(Some(100u64), Uuid::nil(), Side::Sell, 6, 3)
            .with_uuid(Uuid::from_u128(1)),
        &mut |_| {},
    )
    .unwrap();
    book.add_order(
        LimitOrder::new(Some(100u64), 4, Uuid::nil(), Side::Sell).with_uuid(Uuid::from_u128(2)),
        &mut |_| {},
    )
    .unwrap();
    let before = book.checksum().unwrap();
    book.submit_order_with_reports(
        MarketOrder::new(4, Uuid::nil(), Side::Buy),
        MutatingFills,
        Reports::default(),
        &mut |_| {},
    );
    let queue = book.queue_view(Side::Sell, Price64::from(100u64)..=Price64::from(100u64));
    assert_eq!(
        queue.iter().map(|r| r.id.as_u128()).collect::<Vec<_>>(),
        [2, 1]
    );
    assert_ne!(book.checksum().unwrap(), before);
    assert_eq!(book.checksum().unwrap(), crc32fast::hash(b"1:-3:2:-3"));
    let mut layout = Specification::bitfinex();
    layout.priority = Priority::Fifo;
    let layout = layout.prepare().unwrap();
    assert_eq!(
        book.checksum_with(&layout).unwrap(),
        crc32fast::hash(b"2:-3:1:-3")
    );
}

#[test]
fn all_storage_backends_preserve_checksum_and_matching_semantics() {
    macro_rules! run {
        ($level:ident,$sort:ty) => {
            aggregate::<$level<Price64, Hidden, Kraken>, $sort>();
            orders::<$level<Price64, Hidden, BitFinex>, $sort>();
            replenishment::<$level<Price64, Hidden, BitFinex>, $sort>();
        };
    }
    run!(DeepPriceLevel, BTreeMapPriceSorting);
    run!(DeepPriceLevel, SortedVectorPriceSorting);
    run!(IntrusivePriceLevel, BTreeMapPriceSorting);
    run!(IntrusivePriceLevel, SortedVectorPriceSorting);
}
#[test]
fn disabled_policy_has_no_state() {
    assert_eq!(
        std::mem::size_of::<<NoChecksum as ChecksumPolicy>::LevelState>(),
        0
    );
    assert_eq!(
        std::mem::size_of::<<NoChecksum as ChecksumPolicy>::State>(),
        0
    );
    let mut book = OrderStorage::<DeepPriceLevel<Price64, Hidden>>::new();
    book.add_order(
        LimitOrder::new(Some(100u64), 3, Uuid::nil(), Side::Buy),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(book.checksum().unwrap(), 0);
}
#[test]
fn encoding_keeps_wire_digits_and_ecmascript_boundaries() {
    for (atoms, places, expected) in [
        (0, 8, "0"),
        (1, 8, "1e-8"),
        (10, 8, "1e-7"),
        (99, 8, "9.9e-7"),
        (100, 8, "0.000001"),
        (123400000, 8, "1.234"),
        (120, 2, "1.2"),
    ] {
        let input = Input {
            id: 1,
            price: 0,
            quantity: atoms,
            side: Side::Buy,
            widths: DecimalWidths::default(),
        };
        let mut bytes = Vec::new();
        encoding::writer(Field::Quantity, NumberFormat::EcmaScript)(
            &input,
            Precision {
                price: 0,
                quantity: places,
            },
            &mut bytes,
        )
        .unwrap();
        assert_eq!(bytes, expected.as_bytes());
    }
    let mut bytes = Vec::new();
    encoding::decimal_digits(120, 2, 4, &mut bytes).unwrap();
    assert_eq!(bytes, b"12000");
    assert!(encoding::decimal_digits(121, 2, 1, &mut Vec::new()).is_err());
}
