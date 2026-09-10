use lobo_events::PriceLevelChangeEvent;
use lobo_models::{
    Side,
    orders::{
        order_types::{IcebergOrder, LimitOrder},
        traits::Trades,
    },
};
use lobo_primitives::{Price64, uuid::Uuid};
use lobo_storage::{
    arena::arenav1::Arena,
    policies::UpdateHiddenQuantity,
    price_level::{DeepPriceLevel, IntrusivePriceLevel, PriceLevelContract},
};

fn bulk_mutation<L: PriceLevelContract<Price = Price64>>() {
    let mut arena = Arena::default();
    let mut level = L::new(Price64::from(100_u32), Side::Sell);
    let mut events = Vec::new();
    let mut publish = |event| events.push(event);
    level.push(
        &mut arena,
        LimitOrder::new(Some(100), 2, Uuid::nil(), Side::Sell),
        &mut publish,
    );
    level.push(
        &mut arena,
        IcebergOrder::new(Some(100), Uuid::nil(), Side::Sell, 4, 3),
        &mut publish,
    );
    level.for_each_order_mut(
        &mut arena,
        |order| order.common_data.quantity = 0,
        &mut publish,
    );
    let expected = [(2, 0, 1), (5, 4, 2), (3, 4, 1), (3, 1, 1)];
    assert_eq!(
        events,
        expected.map(|(visible, hidden, count)| PriceLevelChangeEvent::new(
            visible,
            hidden,
            Price64::from(100_u32),
            count,
            Side::Sell
        ))
    );
    assert_eq!(level.visible_quantity(), 3);
    assert_eq!(level.hidden_quantity(), 1);
    assert_eq!(level.len(), 1);
    // A read-only traversal/prune must not manufacture another change event.
    level.for_each_order_mut(
        &mut arena,
        |order| assert_eq!(order.quantity(), 3),
        &mut |_| panic!("unchanged level published"),
    );
}

#[test]
fn deep_bulk_mutations_use_completed_level_events() {
    bulk_mutation::<DeepPriceLevel<Price64, UpdateHiddenQuantity>>();
}
#[test]
fn intrusive_bulk_mutations_use_completed_level_events() {
    bulk_mutation::<IntrusivePriceLevel<Price64, UpdateHiddenQuantity>>();
}
