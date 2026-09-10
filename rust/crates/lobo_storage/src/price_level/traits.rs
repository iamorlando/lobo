use crate::arena::arenav1::{Arena, ArenaKey, ArenaLinks};
use crate::policies::HiddenQuantityPolicy;
use crate::policies::checksum::ChecksumPolicy;
use lobo_events::PriceLevelChangeEvent;
use lobo_models::{
    Side,
    events::ExecutionResult,
    orders::{
        core::{Order, RestingOrder},
        traits::Trades,
    },
};
use lobo_primitives::{PriceType, uuid::Uuid};

pub trait HasHiddenQuantity {
    fn hidden_quantity(&self) -> u64;
}

pub(super) mod private {
    use super::*;

    pub trait Mutate<P: PriceType>: HasHiddenQuantity {
        fn len_mut(&mut self) -> &mut usize;
        fn hidden_quantity_mut(&mut self) -> &mut u64;
        fn visible_quantity_mut(&mut self) -> &mut u64;
        fn link_back_unchecked(&mut self, arena: &mut Arena<RestingOrder<P>>, key: ArenaKey);
        fn unlink_removed(&mut self, arena: &mut Arena<RestingOrder<P>>, links: ArenaLinks);
    }
}

pub trait PriceLevelContract: private::Mutate<Self::Price> {
    type HiddenQuantityUpdater: HiddenQuantityPolicy;
    type Checksum: ChecksumPolicy;
    type Price: PriceType;
    fn checksum_state(&self) -> &<Self::Checksum as ChecksumPolicy>::LevelState;
    fn checksum_state_mut(&mut self) -> &mut <Self::Checksum as ChecksumPolicy>::LevelState;
    fn new(price: Self::Price, side: Side) -> Self;
    fn price(&self) -> Self::Price;
    fn side(&self) -> Side;
    fn visible_quantity(&self) -> u64;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool;
    fn queue_is_empty(&self) -> bool;

    /// Read live makers in native FIFO order without pruning or mutation.
    fn for_each_order(
        &self,
        arena: &Arena<RestingOrder<Self::Price>>,
        visit: impl FnMut(&RestingOrder<Self::Price>),
    );

    fn simulate_fill<T>(
        &self,
        arena: &Arena<RestingOrder<Self::Price>>,
        order: &mut Order<T, Self::Price>,
        execution_result: &mut ExecutionResult<Self::Price>,
    );

    fn fill<T, F>(
        &mut self,
        arena: &mut Arena<RestingOrder<Self::Price>>,
        order: &mut Order<T, Self::Price>,
        execution_result: &mut ExecutionResult<Self::Price>,
        on_depleted: &mut F,
        publish: &mut impl FnMut(PriceLevelChangeEvent<Self::Price>),
    ) where
        F: FnMut(Uuid, Uuid) + ?Sized;

    /// Replace one order's contribution, then publish the completed level.
    #[inline(always)]
    fn update_quantities(
        &mut self,
        old_visible: u64,
        old_hidden: u64,
        new_visible: u64,
        new_hidden: u64,
        publish: &mut impl FnMut(PriceLevelChangeEvent<Self::Price>),
    ) {
        Self::HiddenQuantityUpdater::sub_hidden_quantity(self.hidden_quantity_mut(), old_hidden);
        Self::HiddenQuantityUpdater::add_hidden_quantity(self.hidden_quantity_mut(), new_hidden);
        let visible = self.visible_quantity_mut();
        *visible = visible.wrapping_sub(old_visible).wrapping_add(new_visible);
        Self::Checksum::changed(self.checksum_state_mut());
        self.publish_change(publish);
    }

    /// Add one resting order and publish after its count and quantities agree.
    #[inline(always)]
    fn push<O>(
        &mut self,
        arena: &mut Arena<RestingOrder<Self::Price>>,
        order: O,
        publish: &mut impl FnMut(PriceLevelChangeEvent<Self::Price>),
    ) -> ArenaKey
    where
        O: Into<RestingOrder<Self::Price>>,
    {
        let mut order = order.into();
        Self::HiddenQuantityUpdater::prepare_incoming(&mut order);
        debug_assert_eq!(order.side(), self.side());
        debug_assert_eq!(order.price(), Some(self.price()));
        let visible = order.quantity();
        let hidden = Self::HiddenQuantityUpdater::order_hidden_quantity(
            &order.typed_order_details.replenishment_behavior,
        );
        let key = arena.push(order);
        self.link_back_unchecked(arena, key);
        *self.len_mut() += 1;
        Self::HiddenQuantityUpdater::add_hidden_quantity(self.hidden_quantity_mut(), hidden);
        *self.visible_quantity_mut() += visible;
        Self::Checksum::changed(self.checksum_state_mut());
        self.publish_change(publish);
        key
    }

    /// Remove an already-removed arena node's contribution and publish, including
    /// the final zero-quantity, zero-order state of a depleted level.
    #[inline(always)]
    fn pop_virtual(
        &mut self,
        arena: &mut Arena<RestingOrder<Self::Price>>,
        links: ArenaLinks,
        visible: u64,
        hidden: u64,
        publish: &mut impl FnMut(PriceLevelChangeEvent<Self::Price>),
    ) {
        self.unlink_removed(arena, links);
        *self.len_mut() -= 1;
        *self.visible_quantity_mut() -= visible;
        Self::HiddenQuantityUpdater::sub_hidden_quantity(self.hidden_quantity_mut(), hidden);
        Self::Checksum::changed(self.checksum_state_mut());
        self.publish_change(publish);
    }

    #[inline(always)]
    fn publish_change(&self, publish: &mut impl FnMut(PriceLevelChangeEvent<Self::Price>)) {
        publish(PriceLevelChangeEvent::new(
            self.visible_quantity(),
            self.hidden_quantity(),
            self.price(),
            self.len(),
            self.side(),
        ));
    }

    /// Apply quantity changes through `update_quantities` or `pop_virtual`.
    /// The callback must preserve the order's price, side, ID, and trader.
    fn for_each_order_mut(
        &mut self,
        arena: &mut Arena<RestingOrder<Self::Price>>,
        f: impl FnMut(&mut RestingOrder<Self::Price>),
        publish: &mut impl FnMut(PriceLevelChangeEvent<Self::Price>),
    );
}
