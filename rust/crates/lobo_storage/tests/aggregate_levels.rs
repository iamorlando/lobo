use lobo_primitives::Price64;
use lobo_storage::{
    bidask::{Ask, Bid, SidedOrderStore, traits::SidedPrice},
    price_level::{DeepPriceLevel, IntrusivePriceLevel, PriceLevelContract},
    price_sorting::{BTreeMapPriceSorting, PriceSortingPolicy, SortedVectorPriceSorting},
};

fn exercise<L, Sort, S>(expected_best: u64)
where
    L: PriceLevelContract<Price = Price64>,
    Sort: PriceSortingPolicy,
    S: SidedPrice<Price64>,
{
    let mut side = SidedOrderStore::<S, L, Sort>::new();
    let mut events = Vec::new();
    {
        let mut publish = |event| events.push(event);
        side.set_level_quantity(100u64.into(), 5, &mut publish);
        side.set_level_quantity(101u64.into(), 7, &mut publish);
        side.set_level_quantity(100u64.into(), 3, &mut publish);
    }
    assert_eq!(side.visible_quantity, 10);
    assert_eq!(side.len(), 0); // no manufactured orders
    assert_eq!(side.visible_price_levels().count(), 2);
    assert_eq!(
        u64::from(*side.visible_price_levels().next().unwrap().0),
        expected_best
    );
    assert_eq!(events.len(), 3);
    assert_eq!(events[2].visible_quantity(), 3);
    assert_eq!(events[2].number_of_orders(), 0);
    assert_eq!(events[2].side(), S::SIDE);
    assert!(side.get(100u64).unwrap().queue_is_empty());
    side.retain_level_quantities(1, &mut |event| events.push(event));
    assert_eq!(side.visible_price_levels().count(), 1);
    assert_eq!(events.last().unwrap().visible_quantity(), 0);
    assert_eq!(
        side.visible_quantity,
        if expected_best == 101 { 7 } else { 3 }
    );
    side.set_level_quantity(expected_best.into(), 0, &mut |event| events.push(event));
    side.retain_level_quantities(1, &mut |event| events.push(event));
    assert_eq!(side.visible_quantity, 0);
    assert!(!side.contains_key(expected_best));
    side.set_level_quantity(102u64.into(), 9, &mut |event| events.push(event));
    side.retain_level_quantities(0, &mut |event| events.push(event));
    assert_eq!(side.visible_quantity, 0);
    assert_eq!(side.visible_price_levels().count(), 0);
    assert!(!side.contains_key(102u64));
    assert_eq!(events.last().unwrap().visible_quantity(), 0);
}
fn both_sides<L: PriceLevelContract<Price = Price64>, S: PriceSortingPolicy>() {
    exercise::<L, S, Bid>(101);
    exercise::<L, S, Ask>(100);
}
#[test]
fn native_backends_support_aggregate_updates_and_snapshot_reset() {
    both_sides::<
        IntrusivePriceLevel<Price64, lobo_storage::policies::UpdateHiddenQuantity>,
        BTreeMapPriceSorting,
    >();
    both_sides::<
        IntrusivePriceLevel<Price64, lobo_storage::policies::UpdateHiddenQuantity>,
        SortedVectorPriceSorting,
    >();
    both_sides::<
        DeepPriceLevel<Price64, lobo_storage::policies::UpdateHiddenQuantity>,
        BTreeMapPriceSorting,
    >();
    both_sides::<
        DeepPriceLevel<Price64, lobo_storage::policies::UpdateHiddenQuantity>,
        SortedVectorPriceSorting,
    >();
}
