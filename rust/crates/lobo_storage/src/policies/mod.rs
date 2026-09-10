use lobo_models::orders::{
    core::{Order, ReplenishmentBehavior},
    traits::HandlesCompletion,
};
use lobo_primitives::PriceType;

use crate::{price_level::PriceLevelContract, price_sorting::PriceLevelMap};

pub struct UpdateHiddenQuantity;
pub struct DoNotUpdateHiddenQuantity;

pub trait HiddenQuantityPolicy {
    const ENABLED: bool;
    fn prepare_incoming<T, P: PriceType>(order: &mut Order<T, P>)
    where
        Order<T, P>: HandlesCompletion<P>;
    fn included_quantity(quantity: u64) -> u64;
    fn add_hidden_quantity(hidden_quantity: &mut u64, delta: u64);
    fn sub_hidden_quantity(hidden_quantity: &mut u64, delta: u64);
    fn mut_hidden_quantity_in_place(hidden_quantity: &mut u64, new_value: u64);

    fn order_hidden_quantity(replenishment_behavior: &ReplenishmentBehavior) -> u64;
    fn is_depleted(quantity: u64, replenishment_behavior: &ReplenishmentBehavior) -> bool;
    fn aggregate_levels_hidden_quantities<K: Ord, L, Levels>(levels: &Levels) -> u64
    where
        L: PriceLevelContract,
        Levels: PriceLevelMap<K, L>;

    fn update_hidden_quantity_delta_in_place(
        old_hidden_quantity: &mut u64,
        new_hidden_quantity: u64,
    );
}
impl HiddenQuantityPolicy for UpdateHiddenQuantity {
    const ENABLED: bool = true;
    #[inline(always)]
    fn prepare_incoming<T, P: PriceType>(_order: &mut Order<T, P>)
    where
        Order<T, P>: HandlesCompletion<P>,
    {
    }
    #[inline(always)]
    fn included_quantity(quantity: u64) -> u64 {
        quantity
    }
    #[inline(always)]
    fn add_hidden_quantity(hidden_quantity: &mut u64, delta: u64) {
        *hidden_quantity = (*hidden_quantity).strict_add(delta);
    }
    #[inline(always)]
    fn sub_hidden_quantity(hidden_quantity: &mut u64, delta: u64) {
        *hidden_quantity = (*hidden_quantity).strict_sub(delta);
    }
    #[inline(always)]
    fn mut_hidden_quantity_in_place(hidden_quantity: &mut u64, new_value: u64) {
        *hidden_quantity = new_value
    }

    #[inline(always)]
    fn order_hidden_quantity(replenishment_behavior: &ReplenishmentBehavior) -> u64 {
        match replenishment_behavior {
            ReplenishmentBehavior::Iceberg {
                hidden_quantity, ..
            } => *hidden_quantity,
            ReplenishmentBehavior::Remove => 0,
        }
    }
    #[inline(always)]
    fn is_depleted(quantity: u64, replenishment_behavior: &ReplenishmentBehavior) -> bool {
        quantity == 0
            && match replenishment_behavior {
                ReplenishmentBehavior::Iceberg {
                    hidden_quantity, ..
                } => *hidden_quantity == 0,
                ReplenishmentBehavior::Remove => true,
            }
    }

    #[inline(always)]
    fn update_hidden_quantity_delta_in_place(
        old_hidden_quantity: &mut u64,
        new_hidden_quantity: u64,
    ) {
        *old_hidden_quantity += new_hidden_quantity;
    }
    #[inline(always)]
    fn aggregate_levels_hidden_quantities<K: Ord, L, Levels>(levels: &Levels) -> u64
    where
        L: PriceLevelContract,
        Levels: PriceLevelMap<K, L>,
    {
        levels.values().map(|f| f.hidden_quantity()).sum()
    }
}
impl HiddenQuantityPolicy for DoNotUpdateHiddenQuantity {
    const ENABLED: bool = false;
    #[inline(always)]
    fn prepare_incoming<T, P: PriceType>(order: &mut Order<T, P>)
    where
        Order<T, P>: HandlesCompletion<P>,
    {
        order.discard_hidden();
    }
    #[inline(always)]
    fn included_quantity(_quantity: u64) -> u64 {
        0
    }
    #[inline(always)]
    fn add_hidden_quantity(_hidden_quantity: &mut u64, _delta: u64) {}
    #[inline(always)]
    fn sub_hidden_quantity(_hidden_quantity: &mut u64, _delta: u64) {}
    #[inline(always)]
    fn mut_hidden_quantity_in_place(_hidden_quantity: &mut u64, _new_value: u64) {}
    #[inline(always)]
    fn order_hidden_quantity(_replenishment_behavior: &ReplenishmentBehavior) -> u64 {
        0
    }

    #[inline(always)]
    fn is_depleted(quantity: u64, _replenishment_behavior: &ReplenishmentBehavior) -> bool {
        quantity == 0
    }
    #[inline(always)]
    fn update_hidden_quantity_delta_in_place(
        _old_hidden_quantity: &mut u64,
        _new_hidden_quantity: u64,
    ) {
    }
    #[inline(always)]
    fn aggregate_levels_hidden_quantities<K: Ord, L, Levels>(_levels: &Levels) -> u64
    where
        L: PriceLevelContract,
        Levels: PriceLevelMap<K, L>,
    {
        0
    }
}
pub mod checksum;
mod matrix;
