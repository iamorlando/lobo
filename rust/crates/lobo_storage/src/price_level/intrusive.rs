use crate::policies::checksum::{ChecksumPolicy, NoChecksum};
use super::traits::private::Mutate;
use lobo_events::PriceLevelChangeEvent;
use lobo_models::{
    Side,
    events::ExecutionResult,
    orders::{
        core::{FillBehavior, Order, ReplenishmentBehavior, RestingOrder},
        traits::{Fills, Replenishes, Trades},
    },
};
use lobo_primitives::{PriceType, uuid::Uuid};
use std::marker::PhantomData;

use crate::{
    arena::arenav1::{Arena, ArenaKey, ArenaLinks},
    policies::HiddenQuantityPolicy,
    price_level::traits::HasHiddenQuantity,
};

use super::traits::PriceLevelContract;

enum PruneAction {
    Keep {
        visible_quantity: u64,
        hidden_quantity: u64,
    },
    Requeue {
        visible_quantity: u64,
        hidden_quantity: u64,
    },
    Remove,
}

pub struct PriceLevel<P: PriceType, HiddenQuantityUpdater: HiddenQuantityPolicy, C: ChecksumPolicy = NoChecksum> {
    visible_quantity: u64,
    hidden_quantity: u64,
    head: Option<ArenaKey>,
    tail: Option<ArenaKey>,
    len: usize,
    price: P,
    side: Side,
    marker: PhantomData<HiddenQuantityUpdater>,
    checksum: C::LevelState,
}

impl<P: PriceType, H: HiddenQuantityPolicy, C: ChecksumPolicy> Clone for PriceLevel<P, H, C> {
    fn clone(&self) -> Self {
        Self {
            visible_quantity: self.visible_quantity,
            hidden_quantity: self.hidden_quantity,
            head: self.head,
            tail: self.tail,
            len: self.len,
            price: self.price,
            side: self.side,
            checksum: self.checksum.clone(),
            marker: PhantomData,
        }
    }
}

impl<P: PriceType, HiddenQuantityUpdater: HiddenQuantityPolicy, C: ChecksumPolicy>
    PriceLevel<P, HiddenQuantityUpdater, C>
{
    pub fn new<Q: TryInto<P>>(price: Q, side: Side) -> Self {
        <Self as PriceLevelContract>::new(
            price
                .try_into()
                .ok()
                .expect("price level exceeds configured range"),
            side,
        )
    }
}

impl<P: PriceType, HiddenQuantityUpdater: HiddenQuantityPolicy, C: ChecksumPolicy>
    PriceLevel<P, HiddenQuantityUpdater, C>
{
    #[inline(always)]
    fn simulated_fillable_quantity(
        resting: &RestingOrder<P>,
        quantity: u64,
        against: u64,
    ) -> Option<u64> {
        let fill_quantity = quantity.min(against);

        match &resting.typed_order_details.fill_behavior {
            FillBehavior::Standard => Some(fill_quantity),
            FillBehavior::AllOrNothing => (quantity <= against).then_some(quantity),
            FillBehavior::MinimumQuantity(minimum) => (minimum > &against).then_some(fill_quantity),
            FillBehavior::BlockIncrements(block) => {
                (against / block > *block).then_some(fill_quantity)
            }
        }
    }

    #[inline(always)]
    fn prune(order: &mut RestingOrder<P>) -> PruneAction {
        if order.quantity() != 0 {
            return PruneAction::Keep {
                visible_quantity: order.quantity(),
                hidden_quantity: HiddenQuantityUpdater::order_hidden_quantity(
                    &order.typed_order_details.replenishment_behavior,
                ),
            };
        }

        order.replenish_in_place();
        if order.quantity() != 0 {
            PruneAction::Requeue {
                visible_quantity: order.quantity(),
                hidden_quantity: HiddenQuantityUpdater::order_hidden_quantity(
                    &order.typed_order_details.replenishment_behavior,
                ),
            }
        } else {
            PruneAction::Remove
        }
    }

    #[inline(always)]
    fn order_price(order: &RestingOrder<P>) -> P {
        // SAFETY: only priced orders can rest in a price level.
        unsafe { order.price().unwrap_unchecked() }
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
    fn link_back_unchecked(&mut self, arena: &mut Arena<RestingOrder<P>>, key: ArenaKey) {
        // SAFETY: push has just created a live, detached arena node.
        unsafe { arena.push_back_unchecked(&mut self.head, &mut self.tail, key) };
    }
    #[inline(always)]
    fn unlink_removed(&mut self, arena: &mut Arena<RestingOrder<P>>, links: ArenaLinks) {
        // SAFETY: the caller captured these links immediately before removing the node.
        unsafe { arena.relink_remaining_nodes_unchecked(&mut self.head, &mut self.tail, links) };
    }
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
    type HiddenQuantityUpdater = HiddenQuantityUpdater;
    type Price = P;
    type Checksum = C;

    #[inline(always)]
    fn checksum_state(&self) -> &C::LevelState { &self.checksum }
    #[inline(always)]
    fn checksum_state_mut(&mut self) -> &mut C::LevelState { &mut self.checksum }

    #[inline(always)]
    fn new(price: P, side: Side) -> Self {
        Self {
            visible_quantity: 0,
            hidden_quantity: 0,
            head: None,
            tail: None,
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
        self.head.is_none()
    }

    fn for_each_order(
        &self,
        arena: &Arena<RestingOrder<P>>,
        mut visit: impl FnMut(&RestingOrder<P>),
    ) {
        let mut current = self.head;
        while let Some(key) = current {
            // SAFETY: the native intrusive list contains live keys in this arena.
            let node = unsafe { arena.get_node_unchecked(key) };
            if node.value.quantity() != 0 {
                visit(&node.value);
            }
            current = node.next;
        }
    }

    #[inline(always)]
    fn simulate_fill<T>(
        &self,
        arena: &Arena<RestingOrder<P>>,
        order: &mut Order<T, P>,
        execution_result: &mut ExecutionResult<P>,
    ) {
        let mut current = self.head;
        let mut requeued = Vec::new();
        let mut requeued_index = 0;

        while order.quantity() != 0 {
            let (key, mut visible_quantity, mut hidden_quantity) = match current {
                Some(key) => {
                    // SAFETY: intrusive links contain only live arena keys.
                    current = unsafe { arena.linked_next_unchecked(key) };
                    // SAFETY: key came directly from the live intrusive list.
                    let resting = unsafe { arena.get_unchecked(key) };
                    (
                        key,
                        resting.quantity(),
                        HiddenQuantityUpdater::order_hidden_quantity(
                            &resting.typed_order_details.replenishment_behavior,
                        ),
                    )
                }
                None => {
                    if requeued_index == requeued.len() {
                        return;
                    }
                    // SAFETY: the equality check above proves the index is in bounds.
                    let state = unsafe { *requeued.get_unchecked(requeued_index) };
                    requeued_index += 1;
                    state
                }
            };

            // SAFETY: both the intrusive list and simulated requeue entries hold live keys.
            let resting = unsafe { arena.get_unchecked(key) };
            if let Some(fillable_quantity) =
                Self::simulated_fillable_quantity(resting, visible_quantity, order.quantity())
                    .filter(|quantity| *quantity != 0)
            {
                order.common_data.quantity -= fillable_quantity;
                visible_quantity -= fillable_quantity;

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
                    Self::order_price(resting),
                );
            }

            if visible_quantity != 0 {
                continue;
            }

            if let ReplenishmentBehavior::Iceberg { peak_quantity, .. } =
                &resting.typed_order_details.replenishment_behavior
            {
                let next_quantity = hidden_quantity.min(*peak_quantity);
                if next_quantity != 0 {
                    hidden_quantity -= next_quantity;
                    requeued.push((key, next_quantity, hidden_quantity));
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
        let mut current = self.head;
        while order.quantity() != 0 {
            let Some(key) = current else {
                break;
            };
            // SAFETY: every linked key belongs to a live node in this level.
            let node = unsafe { arena.get_node_unchecked_mut(key) };
            let links = node.links();
            let next = node.next;
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
            let action = Self::prune(resting);
            current = match action {
                PruneAction::Keep {
                    visible_quantity,
                    hidden_quantity,
                } => {
                    if (old_visible, old_hidden) != (visible_quantity, hidden_quantity) {
                        self.update_quantities(
                            old_visible,
                            old_hidden,
                            visible_quantity,
                            hidden_quantity,
                            publish,
                        );
                    }
                    next
                }
                PruneAction::Requeue {
                    visible_quantity,
                    hidden_quantity,
                } => {
                    // SAFETY: the node is live and linked, then live and detached.
                    unsafe { arena.unlink_unchecked(&mut self.head, &mut self.tail, key) };
                    unsafe { arena.push_back_unchecked(&mut self.head, &mut self.tail, key) };
                    self.update_quantities(
                        old_visible,
                        old_hidden,
                        visible_quantity,
                        hidden_quantity,
                        publish,
                    );
                    next.or(Some(key))
                }
                PruneAction::Remove => {
                    // SAFETY: capture links before removing this live node. pop_virtual
                    // repairs its neighbors and updates aggregates exactly once.
                    unsafe { arena.remove_and_return_value_unchecked(key) };
                    self.pop_virtual(arena, links, old_visible, old_hidden, publish);
                    next
                }
            };
        }
    }

    #[inline(always)]
    fn for_each_order_mut(
        &mut self,
        arena: &mut Arena<RestingOrder<P>>,
        mut f: impl FnMut(&mut RestingOrder<P>),
        publish: &mut impl FnMut(PriceLevelChangeEvent<P>),
    ) {
        let Some(stop) = self.tail else {
            return;
        };
        let mut current = self.head;
        while let Some(key) = current {
            // SAFETY: traversal follows live intrusive links through the original tail.
            let node = unsafe { arena.get_node_unchecked_mut(key) };
            let links = node.links();
            let next = node.next;
            let order = &mut node.value;
            let old_visible = order.quantity();
            let old_hidden = HiddenQuantityUpdater::order_hidden_quantity(
                &order.typed_order_details.replenishment_behavior,
            );
            f(order);
            let action = Self::prune(order);
            match action {
                PruneAction::Remove => {
                    // SAFETY: links were captured before removing this live node.
                    unsafe { arena.remove_and_return_value_unchecked(key) };
                    self.pop_virtual(arena, links, old_visible, old_hidden, publish);
                }
                PruneAction::Keep {
                    visible_quantity,
                    hidden_quantity,
                }
                | PruneAction::Requeue {
                    visible_quantity,
                    hidden_quantity,
                } => {
                    if matches!(action, PruneAction::Requeue { .. }) {
                        // SAFETY: the node is live and linked, then live and detached.
                        unsafe { arena.unlink_unchecked(&mut self.head, &mut self.tail, key) };
                        unsafe { arena.push_back_unchecked(&mut self.head, &mut self.tail, key) };
                    }
                    if (old_visible, old_hidden) != (visible_quantity, hidden_quantity)
                        || matches!(action, PruneAction::Requeue { .. })
                    {
                        self.update_quantities(
                            old_visible,
                            old_hidden,
                            visible_quantity,
                            hidden_quantity,
                            publish,
                        );
                    }
                }
            }
            if key == stop {
                break;
            }
            current = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::policies::{DoNotUpdateHiddenQuantity, UpdateHiddenQuantity};

    use super::*;
    use lobo_models::{
        Side,
        orders::{
            order_types::{IcebergOrder, LimitOrder, MarketOrder},
            traits::Trades,
        },
    };
    use lobo_primitives::CompressedPrice;

    #[test]
    fn new_level_is_empty_at_its_price() {
        let level = PriceLevel::<CompressedPrice, UpdateHiddenQuantity>::new(
            CompressedPrice::from_raw(101),
            Side::Sell,
        );
        assert_eq!(level.price, 101);
        assert_eq!(level.len(), 0);
        assert!(level.is_empty());
        assert_eq!(level.visible_quantity, 0);
        assert_eq!(level.hidden_quantity, 0);
        assert!(level.queue_is_empty());
        assert!(level.head.is_none());
        assert!(level.tail.is_none());
    }

    #[test]
    fn hidden_quantity_policy_is_generic() {
        let mut arena = Arena::default();
        let mut level =
            PriceLevel::<CompressedPrice, DoNotUpdateHiddenQuantity>::new(100, Side::Sell);

        level.push(
            &mut arena,
            IcebergOrder::new(Some(100), Uuid::new_v4(), Side::Sell, 7, 3),
            &mut |_| {},
        );

        assert_eq!(level.visible_quantity, 3);
        assert_eq!(level.hidden_quantity, 0);
    }

    #[test]
    fn push_links_arena_nodes_and_updates_aggregates() {
        let mut arena = Arena::default();
        let mut level = PriceLevel::<CompressedPrice, UpdateHiddenQuantity>::new(100, Side::Sell);
        let first = level.push(
            &mut arena,
            LimitOrder::new(Some(100), 4, Uuid::new_v4(), Side::Sell),
            &mut |_| {},
        );
        let second = level.push(
            &mut arena,
            IcebergOrder::new(Some(100), Uuid::new_v4(), Side::Sell, 7, 3),
            &mut |_| {},
        );

        assert_eq!(level.head, Some(first));
        assert_eq!(level.tail, Some(second));
        assert_eq!(arena.get_node(first).unwrap().next, Some(second));
        assert_eq!(arena.get_node(second).unwrap().prev, Some(first));
        assert_eq!(level.len(), 2);
        assert_eq!(level.visible_quantity, 7);
        assert_eq!(level.hidden_quantity, 7);
    }

    #[test]
    fn fill_preserves_fifo_and_requeues_replenished_icebergs() {
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
        assert_eq!(level.head, Some(iceberg_key));

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
    fn simulation_uses_link_order_without_mutating_arena() {
        let mut arena = Arena::default();
        let mut level = PriceLevel::<CompressedPrice, UpdateHiddenQuantity>::new(100, Side::Sell);
        let first = LimitOrder::new(Some(100), 2, Uuid::new_v4(), Side::Sell);
        let first_id = first.uuid();
        let second = LimitOrder::new(Some(100), 3, Uuid::new_v4(), Side::Sell);
        let second_id = second.uuid();
        let first_key = level.push(&mut arena, first, &mut |_| {});
        let second_key = level.push(&mut arena, second, &mut |_| {});
        let mut taker = MarketOrder::new(4, Uuid::new_v4(), Side::Buy);
        let mut result = ExecutionResult::new(true);

        level.simulate_fill(&arena, &mut taker, &mut result);

        let fills = &result.match_result.unwrap().fills;
        assert_eq!(fills.len(), 2);
        assert_eq!(fills[0].maker_order_id, first_id);
        assert_eq!(fills[1].maker_order_id, second_id);
        assert_eq!(arena.get(first_key).unwrap().quantity(), 2);
        assert_eq!(arena.get(second_key).unwrap().quantity(), 3);
    }

    #[test]
    fn for_each_order_mut_prunes_and_requeues_without_auxiliary_queue() {
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
        assert_eq!(level.head, Some(iceberg_key));
        assert_eq!(level.tail, Some(iceberg_key));
        assert_eq!(arena.get(iceberg_key).unwrap().quantity(), 2);
        assert_eq!(level.visible_quantity, 2);
        assert_eq!(level.hidden_quantity, 1);
    }

    #[test]
    fn removed_node_links_can_be_spliced_after_arena_removal() {
        let mut arena = Arena::default();
        let mut level = PriceLevel::<CompressedPrice, UpdateHiddenQuantity>::new(100, Side::Sell);
        let first = level.push(
            &mut arena,
            LimitOrder::new(Some(100), 1, Uuid::new_v4(), Side::Sell),
            &mut |_| {},
        );
        let middle = level.push(
            &mut arena,
            LimitOrder::new(Some(100), 2, Uuid::new_v4(), Side::Sell),
            &mut |_| {},
        );
        let last = level.push(
            &mut arena,
            LimitOrder::new(Some(100), 3, Uuid::new_v4(), Side::Sell),
            &mut |_| {},
        );
        let removed = arena.remove_and_return_node(middle).unwrap();

        level.pop_virtual(&mut arena, removed.links(), 2, 0, &mut |_| {});

        assert_eq!(arena.get_node(first).unwrap().next, Some(last));
        assert_eq!(arena.get_node(last).unwrap().prev, Some(first));
        assert_eq!(level.len(), 2);
        assert_eq!(level.visible_quantity, 4);
    }
}
