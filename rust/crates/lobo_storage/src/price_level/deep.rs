use crate::policies::checksum::{ChecksumPolicy, NoChecksum};
use lobo_events::PriceLevelChangeEvent;
use std::{collections::VecDeque, marker::PhantomData};

use lobo_models::{
    Side,
    events::ExecutionResult,
    orders::{
        core::{Order, ReplenishmentBehavior, RestingOrder},
        traits::{Fills, Replenishes, Trades},
    },
};
use lobo_primitives::{PriceType, uuid::Uuid};

use super::traits::PriceLevelContract;
use super::traits::private::Mutate;
use crate::{
    arena::arenav1::{Arena, ArenaKey, ArenaLinks},
    policies::HiddenQuantityPolicy,
    price_level::traits::HasHiddenQuantity,
};

enum PruneAction {
    Keep,
    Requeue,
    Remove,
}

pub struct PriceLevel<P: PriceType, H: HiddenQuantityPolicy, C: ChecksumPolicy = NoChecksum> {
    visible_quantity: u64,
    hidden_quantity: u64,
    order_queue: VecDeque<ArenaKey>,
    len: usize,
    price: P,
    side: Side,
    marker: PhantomData<H>,
    checksum: C::LevelState,
}

impl<P: PriceType, H: HiddenQuantityPolicy, C: ChecksumPolicy> Clone for PriceLevel<P, H, C> {
    fn clone(&self) -> Self {
        Self {
            visible_quantity: self.visible_quantity,
            hidden_quantity: self.hidden_quantity,
            order_queue: self.order_queue.clone(),
            len: self.len,
            price: self.price,
            side: self.side,
            checksum: self.checksum.clone(),
            marker: PhantomData,
        }
    }
}

impl<P, HiddenQuantityUpdater, C: ChecksumPolicy> PriceLevel<P, HiddenQuantityUpdater, C>
where
    HiddenQuantityUpdater: HiddenQuantityPolicy,
    P: PriceType,
{
    fn simulated_fillable_quantity(
        resting: &RestingOrder<P>,
        quantity: u64,
        against: u64,
    ) -> Option<u64> {
        use lobo_models::orders::core::FillBehavior;

        let fill_quantity = quantity.min(against);

        match &resting.typed_order_details.fill_behavior {
            FillBehavior::Standard => Some(fill_quantity),

            FillBehavior::AllOrNothing => {
                if quantity <= against {
                    Some(quantity)
                } else {
                    None
                }
            }

            FillBehavior::MinimumQuantity(q) => {
                if q > &against {
                    Some(fill_quantity)
                } else {
                    None
                }
            }

            FillBehavior::BlockIncrements(b) => {
                let s = against / b;

                if s > *b { Some(fill_quantity) } else { None }
            }
        }
    }

    pub fn new<Q: TryInto<P>>(price: Q, side: Side) -> Self {
        <Self as PriceLevelContract>::new(
            price
                .try_into()
                .ok()
                .expect("price level exceeds configured range"),
            side,
        )
    }

    fn prune(arena: &mut Arena<RestingOrder<P>>, key: ArenaKey) -> PruneAction {
        let action = {
            let Some(order) = arena.get_mut(key) else {
                return PruneAction::Remove;
            };

            if order.quantity() != 0 {
                PruneAction::Keep
            } else {
                order.replenish_in_place();

                if order.quantity() != 0 {
                    // Replenished iceberg: same arena key,
                    // but move it to the back of the queue.
                    PruneAction::Requeue
                } else {
                    // Remove behavior or no hidden quantity.
                    PruneAction::Remove
                }
            }
        };

        if matches!(action, PruneAction::Remove) {
            let _ = arena.remove_and_return_value(key);
        }
        action
    }
}
impl<P: PriceType, HiddenQuantityUpdater: HiddenQuantityPolicy, C: ChecksumPolicy> HasHiddenQuantity
    for PriceLevel<P, HiddenQuantityUpdater, C>
{
    fn hidden_quantity(&self) -> u64 {
        self.hidden_quantity
    }
}

impl<P: PriceType, HiddenQuantityUpdater: HiddenQuantityPolicy, C: ChecksumPolicy> Mutate<P>
    for PriceLevel<P, HiddenQuantityUpdater, C>
{
    #[inline(always)]
    fn link_back_unchecked(&mut self, _arena: &mut Arena<RestingOrder<P>>, key: ArenaKey) {
        self.order_queue.push_back(key);
    }
    #[inline(always)]
    fn unlink_removed(&mut self, _arena: &mut Arena<RestingOrder<P>>, _links: ArenaLinks) {}
    #[inline(always)]
    fn len_mut(&mut self) -> &mut usize {
        &mut self.len
    }
    fn hidden_quantity_mut(&mut self) -> &mut u64 {
        &mut self.hidden_quantity
    }
    #[inline(always)]
    fn visible_quantity_mut(&mut self) -> &mut u64 {
        &mut self.visible_quantity
    }
}
impl<P: PriceType, HiddenQuantityUpdater: HiddenQuantityPolicy, C: ChecksumPolicy> PriceLevelContract
    for PriceLevel<P, HiddenQuantityUpdater, C>
{
    type Price = P;
    type Checksum = C;

    #[inline(always)]
    fn checksum_state(&self) -> &C::LevelState { &self.checksum }
    #[inline(always)]
    fn checksum_state_mut(&mut self) -> &mut C::LevelState { &mut self.checksum }
    type HiddenQuantityUpdater = HiddenQuantityUpdater;

    #[inline(always)]
    fn new(price: P, side: Side) -> Self {
        Self {
            visible_quantity: 0,
            hidden_quantity: 0,
            order_queue: VecDeque::new(),
            len: 0,
            price,
            side,
            checksum: C::LevelState::default(),
            marker: PhantomData,
        }
    }

    #[inline(always)]
    fn price(&self) -> P {
        self.price
    }

    #[inline(always)]
    fn side(&self) -> Side {
        self.side
    }

    #[inline(always)]
    fn visible_quantity(&self) -> u64 {
        self.visible_quantity
    }

    #[inline(always)]
    fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline(always)]
    fn queue_is_empty(&self) -> bool {
        self.order_queue.is_empty()
    }

    fn for_each_order(
        &self,
        arena: &Arena<RestingOrder<P>>,
        mut visit: impl FnMut(&RestingOrder<P>),
    ) {
        for key in &self.order_queue {
            if let Some(order) = arena.get(*key) {
                if order.quantity() != 0 {
                    visit(order);
                }
            }
        }
    }

    #[inline(always)]
    fn simulate_fill<T>(
        &self,
        arena: &Arena<RestingOrder<P>>,
        order: &mut Order<T, P>,
        execution_result: &mut ExecutionResult<P>,
    ) {
        // Read the actual FIFO queue without modifying it.
        let mut untouched = self.order_queue.iter().copied();

        // Local simulation state for replenished iceberg orders:
        //
        // (arena key, simulated visible quantity, simulated hidden quantity)
        //
        // Replenished orders lose priority, so they go to the back here just as
        // they go into `requeued` in `fill`.
        let mut requeued: VecDeque<(ArenaKey, u64, u64)> = VecDeque::new();

        while order.quantity() > 0 {
            let (key, mut visible_quantity, mut hidden_quantity) = loop {
                if let Some(key) = untouched.next() {
                    let Some(resting) = arena.get(key) else {
                        // Same effect on matching as fill(): stale keys provide
                        // no liquidity. Simulation does not remove them because
                        // the book is read-only.
                        continue;
                    };

                    break (
                        key,
                        resting.quantity(),
                        HiddenQuantityUpdater::order_hidden_quantity(
                            &resting.typed_order_details.replenishment_behavior,
                        ),
                    );
                }

                let Some(simulated) = requeued.pop_front() else {
                    return;
                };

                break simulated;
            };

            // This key was either successfully read from the immutable arena
            // above, or was previously put into `requeued` from such an entry.
            let resting = arena
                .get(key)
                .expect("simulated resting order must remain in immutable arena");

            if let Some(fillable_quantity) =
                Self::simulated_fillable_quantity(resting, visible_quantity, order.quantity())
                    .filter(|&quantity| quantity > 0)
            {
                order.common_data.quantity -= fillable_quantity;

                // Only local simulated maker state is changed.
                visible_quantity -= fillable_quantity;

                // This is exactly the same test as fill(), except it uses the
                // simulated quantities rather than mutating the arena order.
                let maker_depleted = visible_quantity == 0
                    && match &resting.typed_order_details.replenishment_behavior {
                        ReplenishmentBehavior::Iceberg { .. } => hidden_quantity == 0,
                        ReplenishmentBehavior::Remove => true,
                    };

                execution_result.add_fill(
                    resting.uuid(),
                    order.uuid(),
                    maker_depleted,
                    resting.common_data.trader,
                    fillable_quantity,
                    resting.price().unwrap(),
                );
            }

            // Equivalent to prune(), but applied entirely to local simulation
            // state rather than mutating RestingOrder or Arena.
            if visible_quantity != 0 {
                // PruneAction::Keep.
                //
                // Real fill moves this maker into `retained`, so it cannot be
                // encountered again during this fill. Nothing more to do here.
                continue;
            }

            match &resting.typed_order_details.replenishment_behavior {
                ReplenishmentBehavior::Iceberg { peak_quantity, .. } => {
                    // Exactly the same calculation as replenish_in_place():
                    //
                    // next_quantity = hidden_quantity.min(peak_quantity)
                    let next_quantity = hidden_quantity.min(*peak_quantity);

                    if next_quantity != 0 {
                        visible_quantity = next_quantity;
                        hidden_quantity -= next_quantity;

                        // PruneAction::Requeue.
                        //
                        // Same arena key, but it loses time priority.
                        requeued.push_back((key, visible_quantity, hidden_quantity));
                    }

                    // next_quantity == 0 corresponds to PruneAction::Remove.
                    // Simulation simply drops the local state.
                }

                ReplenishmentBehavior::Remove => {
                    // PruneAction::Remove.
                    // Nothing to mutate in simulation.
                }
            }
        }
    }

    #[inline(always)]
    fn fill<T, F>(
        &mut self,
        arena: &mut Arena<RestingOrder<P>>,
        order: &mut Order<T, P>,
        execution_result: &mut ExecutionResult<P>,
        on_depleted: &mut F,
        publish: &mut impl FnMut(PriceLevelChangeEvent<P>),
    ) where
        F: FnMut(Uuid, Uuid) + ?Sized,
    {
        let mut retained = VecDeque::new();
        let mut requeued = VecDeque::new();
        while order.quantity() != 0 {
            let Some(key) = self.order_queue.pop_front() else {
                if requeued.is_empty() {
                    break;
                }
                self.order_queue.append(&mut requeued);
                continue;
            };
            let Some(node) = arena.get_node_mut(key) else {
                continue;
            };
            let links = node.links();
            let resting = &mut node.value;
            let old_visible = resting.quantity();
            let old_hidden = HiddenQuantityUpdater::order_hidden_quantity(
                &resting.typed_order_details.replenishment_behavior,
            );
            if let Some(quantity) = resting
                .fillable_quantity(order.quantity())
                .filter(|q| *q != 0)
            {
                order.common_data.quantity -= quantity;
                resting.common_data.quantity -= quantity;
                let depleted = HiddenQuantityUpdater::is_depleted(
                    resting.quantity(),
                    &resting.typed_order_details.replenishment_behavior,
                );
                execution_result.add_fill(
                    resting.uuid(),
                    order.uuid(),
                    depleted,
                    resting.common_data.trader,
                    quantity,
                    self.price,
                );
                if depleted {
                    on_depleted(resting.uuid(), resting.common_data.trader);
                }
            }
            match Self::prune(arena, key) {
                PruneAction::Keep => {
                    // SAFETY: Keep leaves the maker live in the arena.
                    let resting = unsafe { arena.get_unchecked(key) };
                    if old_visible != resting.quantity() {
                        self.update_quantities(
                            old_visible,
                            old_hidden,
                            resting.quantity(),
                            HiddenQuantityUpdater::order_hidden_quantity(
                                &resting.typed_order_details.replenishment_behavior,
                            ),
                            publish,
                        );
                    }
                    retained.push_back(key);
                }
                PruneAction::Requeue => {
                    // SAFETY: Requeue leaves the replenished maker live.
                    let resting = unsafe { arena.get_unchecked(key) };
                    requeued.push_back(key);
                    // Filling and replenishing are one committed level update.
                    self.update_quantities(
                        old_visible,
                        old_hidden,
                        resting.quantity(),
                        HiddenQuantityUpdater::order_hidden_quantity(
                            &resting.typed_order_details.replenishment_behavior,
                        ),
                        publish,
                    );
                }
                PruneAction::Remove => {
                    self.pop_virtual(arena, links, old_visible, old_hidden, publish)
                }
            }
        }
        while let Some(key) = retained.pop_back() {
            self.order_queue.push_front(key);
        }
        self.order_queue.append(&mut requeued);
    }

    #[inline(always)]
    fn for_each_order_mut(
        &mut self,
        arena: &mut Arena<RestingOrder<P>>,
        mut f: impl FnMut(&mut RestingOrder<P>),
        publish: &mut impl FnMut(PriceLevelChangeEvent<P>),
    ) {
        let original_len = self.order_queue.len();
        let mut requeued = VecDeque::new();
        for _ in 0..original_len {
            let Some(key) = self.order_queue.pop_front() else {
                break;
            };
            let Some(node) = arena.get_node_mut(key) else {
                continue;
            };
            let links = node.links();
            let old_visible = node.value.quantity();
            let old_hidden = HiddenQuantityUpdater::order_hidden_quantity(
                &node.value.typed_order_details.replenishment_behavior,
            );
            f(&mut node.value);
            let action = Self::prune(arena, key);
            match action {
                PruneAction::Remove => {
                    self.pop_virtual(arena, links, old_visible, old_hidden, publish)
                }
                PruneAction::Keep | PruneAction::Requeue => {
                    // SAFETY: both branches retain a live arena order.
                    let resting = unsafe { arena.get_unchecked(key) };
                    let visible = resting.quantity();
                    let hidden = HiddenQuantityUpdater::order_hidden_quantity(
                        &resting.typed_order_details.replenishment_behavior,
                    );
                    if matches!(action, PruneAction::Requeue) {
                        requeued.push_back(key);
                    } else {
                        self.order_queue.push_back(key);
                    }
                    if (old_visible, old_hidden) != (visible, hidden)
                        || matches!(action, PruneAction::Requeue)
                    {
                        self.update_quantities(old_visible, old_hidden, visible, hidden, publish);
                    }
                }
            }
        }
        self.order_queue.append(&mut requeued);
    }
}

#[cfg(test)]
mod tests {
    use crate::policies::UpdateHiddenQuantity;

    use super::*;
    use lobo_models::{
        Side,
        orders::{
            order_types::{IcebergOrder, LimitOrder, MarketOrder},
            traits::Trades,
        },
    };
    use lobo_primitives::{CompressedPrice, uuid::Uuid};

    #[test]
    fn new_level_is_empty_at_its_price() {
        let level = PriceLevel::<CompressedPrice, UpdateHiddenQuantity>::new(101, Side::Sell);
        assert_eq!(level.price, 101);
        assert_eq!(level.len(), 0);
        assert!(level.is_empty());
        assert_eq!(level.visible_quantity, 0);
        assert_eq!(level.hidden_quantity, 0);
    }

    #[test]
    fn pushing_limit_and_iceberg_orders_updates_aggregates() {
        let mut arena = Arena::default();

        let mut level = PriceLevel::<CompressedPrice, UpdateHiddenQuantity>::new(100, Side::Sell);
        level.push(
            &mut arena,
            LimitOrder::new(Some(100), 4, Uuid::new_v4(), Side::Sell),
            &mut |_| {},
        );
        level.push(
            &mut arena,
            IcebergOrder::new(Some(100), Uuid::new_v4(), Side::Sell, 7, 3),
            &mut |_| {},
        );

        assert_eq!(level.len(), 2);
        assert_eq!(level.visible_quantity, 7);
        assert_eq!(level.hidden_quantity, 7);
    }

    #[test]
    fn fill_drops_stale_arena_keys_after_live_aggregates_are_updated() {
        let mut arena = Arena::default();

        let mut level = PriceLevel::<CompressedPrice, UpdateHiddenQuantity>::new(100, Side::Sell);
        let key = level.push(
            &mut arena,
            LimitOrder::new(Some(100), 4, Uuid::new_v4(), Side::Sell),
            &mut |_| {},
        );
        let removed = arena.remove_and_return_node(key).unwrap();
        level.pop_virtual(&mut arena, removed.links(), 4, 0, &mut |_| {});
        assert!(level.is_empty());
        assert!(!level.queue_is_empty());

        let mut taker = MarketOrder::new(2, Uuid::new_v4(), Side::Buy);
        let mut result = ExecutionResult::new(true);
        level.fill(
            &mut arena,
            &mut taker,
            &mut result,
            &mut |_, _| {},
            &mut |_| {},
        );

        assert!(result.match_result.unwrap().fills.is_empty());
        assert_eq!(taker.quantity(), 2);
        assert!(level.is_empty());
        assert!(level.queue_is_empty());
        assert_eq!(level.visible_quantity, 0);
        assert_eq!(level.hidden_quantity, 0);
    }

    #[test]
    fn fill_retains_partial_maker_and_requeues_replenished_iceberg() {
        let mut arena = Arena::default();

        let mut level = PriceLevel::<CompressedPrice, UpdateHiddenQuantity>::new(100, Side::Sell);
        let limit_key = level.push(
            &mut arena,
            LimitOrder::new(Some(100), 5, Uuid::new_v4(), Side::Sell),
            &mut |_| {},
        );
        let iceberg_key = level.push(
            &mut arena,
            IcebergOrder::new(Some(100), Uuid::new_v4(), Side::Sell, 2, 2),
            &mut |_| {},
        );

        let mut first_taker = MarketOrder::new(2, Uuid::new_v4(), Side::Buy);
        level.fill(
            &mut arena,
            &mut first_taker,
            &mut ExecutionResult::new(false),
            &mut |_, _| {},
            &mut |_| {},
        );
        assert_eq!(arena.get(limit_key).unwrap().quantity(), 3);

        let mut second_taker = MarketOrder::new(3, Uuid::new_v4(), Side::Buy);
        level.fill(
            &mut arena,
            &mut second_taker,
            &mut ExecutionResult::new(false),
            &mut |_, _| {},
            &mut |_| {},
        );
        assert!(arena.get(limit_key).is_none());
        assert_eq!(arena.get(iceberg_key).unwrap().quantity(), 2);

        let mut third_taker = MarketOrder::new(2, Uuid::new_v4(), Side::Buy);
        level.fill(
            &mut arena,
            &mut third_taker,
            &mut ExecutionResult::new(false),
            &mut |_, _| {},
            &mut |_| {},
        );
        assert_eq!(arena.get(iceberg_key).unwrap().quantity(), 2);
        assert_eq!(level.visible_quantity, 2);
        assert_eq!(level.hidden_quantity, 0);
    }

    #[test]
    fn for_each_order_mut_prunes_removed_orders_and_requeues_icebergs() {
        let mut arena = Arena::default();

        let mut level = PriceLevel::<CompressedPrice, UpdateHiddenQuantity>::new(100, Side::Sell);
        level.push(
            &mut arena,
            LimitOrder::new(Some(100), 2, Uuid::new_v4(), Side::Sell),
            &mut |_| {},
        );
        let iceberg_key = level.push(
            &mut arena,
            IcebergOrder::new(Some(100), Uuid::new_v4(), Side::Sell, 3, 2),
            &mut |_| {},
        );

        level.for_each_order_mut(
            &mut arena,
            |order| {
                order.common_data.quantity = 0;
            },
            &mut |_| {},
        );

        assert_eq!(level.len(), 1);
        assert_eq!(arena.get(iceberg_key).unwrap().quantity(), 2);
        assert_eq!(level.visible_quantity, 2);
        assert_eq!(level.hidden_quantity, 1);
    }

    #[test]
    fn prune_treats_missing_arena_entry_as_removed() {
        let mut arena = Arena::default();
        let key = arena.push(LimitOrder::new(Some(100), 1, Uuid::new_v4(), Side::Sell).into());
        assert!(arena.remove_and_return_value(key).is_some());
        assert!(matches!(
            PriceLevel::<CompressedPrice, UpdateHiddenQuantity>::prune(&mut arena, key),
            PruneAction::Remove
        ));
    }
}
