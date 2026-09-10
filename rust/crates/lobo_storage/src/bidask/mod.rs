use lobo_events::{MutationPublisher, PriceChangeEvent, TradedVolumeEvent};
use lobo_models::Side;
use crate::policies::checksum::{self, ChecksumPolicy, DecimalWidths};
use std::{cmp::Reverse, marker::PhantomData};
pub mod traits;
use lobo_models::{
    events::ExecutionResult,
    orders::{
        core::{Order, RestingOrder},
        traits::{HandlesCompletion, Trades},
    },
};

use lobo_primitives::{PriceType, uuid::Uuid};

use crate::{
    arena::arenav1::{Arena, ArenaKey, ArenaLinks},
    bidask::traits::SidedPrice,
    policies::{HiddenQuantityPolicy, UpdateHiddenQuantity},
    price_level::PriceLevelContract,
    price_sorting::{PriceLevelMap, PriceSortingPolicy, SortedVectorPriceSorting},
};
#[cfg(feature = "python-polars")]
use pyo3::prelude::*;

#[cfg(feature = "python-polars")]
use polars::prelude::{Column, DataFrame};

#[cfg(feature = "python-polars")]
use pyo3::exceptions::PyRuntimeError;

#[cfg(feature = "python-polars")]
use pyo3_polars::PyDataFrame;
pub struct Bid;
pub struct Ask;

pub struct SidedOrderStore<
    S: SidedPrice<L::Price>,
    L: PriceLevelContract,
    Sort: PriceSortingPolicy = SortedVectorPriceSorting,
    H: HiddenQuantityPolicy = UpdateHiddenQuantity,
> {
    pub visible_quantity: u64,
    pub hidden_quantity: u64,
    orders: Sort::PriceLevels<S::PriceKey, L>,
    len: usize,
    side: PhantomData<(S, Sort)>,
    hidden_quantity_policy: PhantomData<H>,
}

pub struct SubmittedData {
    pub order_id: Uuid,
    pub user_id: Uuid,
    pub arena_key: ArenaKey,
}

/// An immutable order view, built only for the requested display range.
#[derive(Clone, Debug)]
pub struct QueueOrder<P> {
    pub id: Uuid,
    pub price: P,
    pub quantity: u64,
    pub created_at: lobo_primitives::time::DateTime<lobo_primitives::time::Utc>,
}

impl<S, L, Sort, H> Clone for SidedOrderStore<S, L, Sort, H>
where
    S: SidedPrice<L::Price>,
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    H: HiddenQuantityPolicy,
    Sort::PriceLevels<S::PriceKey, L>: Clone,
{
    fn clone(&self) -> Self {
        Self {
            visible_quantity: self.visible_quantity,
            hidden_quantity: self.hidden_quantity,
            orders: self.orders.clone(),
            len: self.len,
            side: PhantomData,
            hidden_quantity_policy: PhantomData,
        }
    }
}

pub(crate) struct FillDeltas<K> {
    old_visible_quantity: u64,
    new_visible_quantity: u64,
    old_hidden_quantity: u64,
    new_hidden_quantity: u64,
    old_len: usize,
    new_len: usize,
    physically_empty_levels: Vec<K>,
}

impl<K> Default for FillDeltas<K> {
    fn default() -> Self {
        Self {
            old_visible_quantity: 0,
            new_visible_quantity: 0,
            old_hidden_quantity: 0,
            new_hidden_quantity: 0,
            old_len: 0,
            new_len: 0,
            physically_empty_levels: Vec::new(),
        }
    }
}
impl<S, L, Sort, H> Default for SidedOrderStore<S, L, Sort, H>
where
    S: SidedPrice<L::Price>,
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    H: HiddenQuantityPolicy,
{
    fn default() -> Self {
        Self {
            visible_quantity: 0,
            hidden_quantity: 0,
            len: 0,
            orders: Sort::PriceLevels::default(),
            side: PhantomData,
            hidden_quantity_policy: PhantomData,
        }
    }
}

impl<S, L, Sort, H> SidedOrderStore<S, L, Sort, H>
where
    S: SidedPrice<L::Price>,
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    H: HiddenQuantityPolicy,
{
    #[inline(always)]
    fn checked_price<Q: TryInto<L::Price>>(price: Q) -> L::Price {
        price
            .try_into()
            .ok()
            .expect("price exceeds configured range")
    }

    fn select_price_levels_for_order<'a, T>(
        &'a mut self,
        order: &Order<T, L::Price>,
    ) -> impl Iterator<Item = (&'a S::PriceKey, &'a mut L)> + 'a {
        let limit_key = order.price().map(S::into_key);

        self.orders
            .prefix_mut(limit_key)
            .filter(|(_, level)| !level.is_empty())
    }

    fn select_price_levels_for_simulation<'a, T>(
        &'a self,
        order: &Order<T, L::Price>,
    ) -> impl Iterator<Item = (&'a S::PriceKey, &'a L)> + 'a {
        let limit_key = order.price().map(S::into_key);

        self.orders
            .prefix(limit_key)
            .filter(|(_, level)| !level.is_empty())
    }

    fn attempt_fill<'a, T>(
        pls: impl Iterator<Item = (&'a S::PriceKey, &'a mut L)>,
        arena: &mut Arena<RestingOrder<L::Price>>,
        mut order: Order<T, L::Price>,
        include_match_result: bool,
        on_depleted: &mut dyn FnMut(Uuid, Uuid),
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> (ExecutionResult<L::Price>, bool, FillDeltas<S::PriceKey>)
    where
        S::PriceKey: 'a,
        L: 'a,
        Order<T, L::Price>: HandlesCompletion<L::Price>,
    {
        let initial_quantity = order.quantity();
        let mut result = ExecutionResult::<L::Price>::new(include_match_result);
        let mut deltas = FillDeltas::default();

        for (key, level) in pls {
            if order.quantity() == 0 {
                break;
            }

            let old_visible_quantity = level.visible_quantity();
            let old_hidden_quantity = level.hidden_quantity();

            let old_len = level.len();

            let before_fill = publish.observe_trade(|| order.quantity());
            level.fill(arena, &mut order, &mut result, on_depleted, &mut |event| {
                publish.level(event)
            });
            publish.traded_volume(|| {
                let quantity = before_fill - order.quantity();
                (quantity != 0).then(|| TradedVolumeEvent {
                    price: level.price(),
                    quantity,
                    side: S::SIDE,
                })
            });

            deltas.old_visible_quantity += old_visible_quantity;
            deltas.new_visible_quantity += level.visible_quantity();

            H::update_hidden_quantity_delta_in_place(
                &mut deltas.old_hidden_quantity,
                old_hidden_quantity,
            );
            H::update_hidden_quantity_delta_in_place(
                &mut deltas.new_hidden_quantity,
                level.hidden_quantity(),
            );

            deltas.old_len += old_len;
            deltas.new_len += level.len();

            // TODO put this behind a Depletion Policy guard
            if level.queue_is_empty() {
                deltas.physically_empty_levels.push(key.clone());
            }
        }

        let matched = order.quantity() != initial_quantity;
        order.handle_completion(&mut result);

        (result, matched, deltas)
    }

    fn attempt_simulate<'a, T>(
        pls: impl Iterator<Item = (&'a S::PriceKey, &'a L)>,
        arena: &Arena<RestingOrder<L::Price>>,
        mut order: Order<T, L::Price>,
        include_match_result: bool,
    ) -> ExecutionResult<L::Price>
    where
        S::PriceKey: 'a,
        L: 'a,
        Order<T, L::Price>: HandlesCompletion<L::Price>,
    {
        let mut result = ExecutionResult::<L::Price>::new(include_match_result);

        for (_, level) in pls {
            if order.quantity() == 0 {
                break;
            }

            level.simulate_fill(arena, &mut order, &mut result);
        }

        order.handle_completion(&mut result);

        result
    }

    pub fn fill<T>(
        &mut self,
        arena: &mut Arena<RestingOrder<L::Price>>,
        order: Order<T, L::Price>,
        include_match_result: bool,
        on_depleted: &mut dyn FnMut(Uuid, Uuid),
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> ExecutionResult<L::Price>
    where
        Order<T, L::Price>: HandlesCompletion<L::Price>,
    {
        let before = publish.observe_price(|| self.top_level().map(PriceLevelContract::price));
        let iterator = self.select_price_levels_for_order(&order);
        let (result, _, deltas) = Self::attempt_fill(
            iterator,
            arena,
            order,
            include_match_result,
            on_depleted,
            publish,
        );

        self.visible_quantity = self
            .visible_quantity
            .wrapping_sub(deltas.old_visible_quantity)
            .wrapping_add(deltas.new_visible_quantity);
        H::sub_hidden_quantity(&mut self.hidden_quantity, deltas.old_hidden_quantity);
        H::add_hidden_quantity(&mut self.hidden_quantity, deltas.new_hidden_quantity);
        self.len = Self::replace_len(self.len, deltas.old_len, deltas.new_len);

        // Empty-level observations stay in `deltas`; their levels remain resident.
        self.publish_price_change(before, publish);
        result
    }

    pub fn simulate<T>(
        &self,
        arena: &Arena<RestingOrder<L::Price>>,
        order: Order<T, L::Price>,
        include_match_result: bool,
    ) -> ExecutionResult<L::Price>
    where
        Order<T, L::Price>: HandlesCompletion<L::Price>,
    {
        let iterator = self.select_price_levels_for_simulation(&order);

        Self::attempt_simulate(iterator, arena, order, include_match_result)
    }

    /// Preview aggregate liquidity directly. L2 has no maker IDs; nil denotes an
    /// unidentified aggregate counterparty, never a fabricated resting order.
    pub fn simulate_aggregate<T>(
        &self,
        mut order: Order<T, L::Price>,
        include_match_result: bool,
    ) -> ExecutionResult<L::Price>
    where
        Order<T, L::Price>: HandlesCompletion<L::Price>,
    {
        let mut result = ExecutionResult::new(include_match_result);
        let limit = order.price().map(S::into_key);
        for (_, level) in self.orders.prefix(limit) {
            if order.quantity() == 0 {
                break;
            }
            let quantity = order.quantity().min(level.visible_quantity());
            if quantity == 0 {
                continue;
            }
            order.common_data.quantity -= quantity;
            result.add_fill(
                Uuid::nil(),
                order.uuid(),
                quantity == level.visible_quantity(),
                Uuid::nil(),
                quantity,
                level.price(),
            );
        }
        order.handle_completion(&mut result);
        result
    }

    /// Visit selected levels in matching price priority, preserving each native
    /// FIFO. Creation time is metadata: replenishment moves an order to the tail
    /// without changing its original timestamp.
    pub fn queue_view(
        &self,
        arena: &Arena<RestingOrder<L::Price>>,
        prices: std::ops::RangeInclusive<L::Price>,
    ) -> Vec<QueueOrder<L::Price>> {
        if prices.is_empty() {
            return Vec::new();
        }
        let first = S::into_key(*prices.start());
        let last = S::into_key(*prices.end());
        let bounds = if first <= last {
            first..=last
        } else {
            last..=first
        };
        let mut orders = Vec::new();
        for (_, level) in self.orders.range(bounds) {
            level.for_each_order(arena, |order| {
                orders.push(QueueOrder {
                    id: order.uuid(),
                    price: level.price(),
                    quantity: order.quantity(),
                    created_at: order.common_data.creation_time,
                })
            });
        }
        orders
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Number of occupied price levels on this side of the book.
    pub fn price_level_count(&self) -> usize {
        self.orders
            .values()
            .filter(|level| !level.is_empty())
            .count()
    }

    /// Price levels in matching priority order (best price first).
    pub fn price_levels(&self) -> impl Iterator<Item = (&L::Price, &L)> {
        self.orders
            .iter()
            .filter(|(_, level)| !level.is_empty())
            .map(|(key, level)| (S::price_from_key(key), level))
    }

    fn top_level(&self) -> Option<&L> {
        self.orders
            .values()
            .find(|level| !level.is_empty() || level.visible_quantity() != 0)
    }
    #[inline(always)]
    fn publish_price_change(
        &self,
        before: Option<L::Price>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) {
        publish.price_change(|| {
            let best = self.top_level();
            let price = best.map(PriceLevelContract::price);
            (price != before).then(|| PriceChangeEvent {
                price,
                quantity: best.map_or(0, PriceLevelContract::visible_quantity),
                number_of_orders: best.map_or(0, PriceLevelContract::len),
                side: S::SIDE,
            })
        });
    }

    /// Visible quotes, best price first, including aggregate feeds with no
    /// individual orders. `len`, `is_empty`, and `price_levels` describe orders;
    /// this view describes displayed liquidity independently of the order FIFO.
    pub fn visible_price_levels(&self) -> impl Iterator<Item = (&L::Price, &L)> {
        self.orders
            .iter()
            .filter(|(_, level)| level.visible_quantity() != 0)
            .map(|(key, level)| (S::price_from_key(key), level))
    }

    /// Gather only the checksum window in this side's matching price order.
    /// Cached level contributions are rebuilt only after a mutation.
    pub(crate) fn checksum_data<Selection: checksum::Selection>(
        &mut self,
        arena: &Arena<RestingOrder<L::Price>>,
        prepared: &checksum::Prepared,
        precision: checksum::Precision,
        generation: u64,
        cache: &mut checksum::SideCache,
    ) -> Result<bool, checksum::Error>
    where L::Checksum: ChecksumPolicy<LevelState = checksum::LevelCache> {
        cache.begin();
        let depth = prepared.specification.depth;
        for level in self.orders.values_mut().filter(|level| level.visible_quantity() != 0) {
            checksum::refresh_level::<Selection, L>(level, arena, prepared, precision, generation)?;
            cache.append(level.checksum_state(), depth);
            if cache.len() == depth { break; }
        }
        Ok(cache.finish())
    }

    /// Replace the displayed quantity of an aggregate feed's price level.
    ///
    /// Use on a store populated exclusively by aggregate updates, never on a
    /// store containing individual orders. Aggregate feeds do not supply FIFO
    /// orders or order counts; those remain empty/zero. Prices must already be
    /// converted to the configured native price type by the adapter.
    ///
    /// The existing level mutation owns publication. Side dispatch, price type,
    /// and publisher are static; no feed-mode checks enter order matching.
    #[inline(always)]
    pub fn set_level_quantity(
        &mut self,
        price: L::Price,
        quantity: u64,
        publish: &mut impl MutationPublisher<L::Price>,
    ) {
        self.set_level_quantity_with(price, quantity, |_| {}, publish);
    }

    /// Apply wire formatting metadata with the aggregate quantity in one lookup.
    #[inline(always)]
    pub fn set_level_quantity_formatted(
        &mut self, price: L::Price, quantity: u64, widths: DecimalWidths,
        publish: &mut impl MutationPublisher<L::Price>,
    ) {
        self.set_level_quantity_with(price, quantity,
            |level| L::Checksum::wire_widths(level.checksum_state_mut(), widths), publish);
    }

    #[inline(always)]
    fn set_level_quantity_with(
        &mut self, price: L::Price, quantity: u64, format: impl FnOnce(&mut L),
        publish: &mut impl MutationPublisher<L::Price>,
    ) {
        let before = publish.observe_price(|| self.top_level().map(PriceLevelContract::price));
        let level = self
            .orders
            .get_or_insert_with(S::into_key(price), || L::new(price, S::SIDE));
        let old = level.visible_quantity();
        format(level);
        self.visible_quantity = self
            .visible_quantity
            .wrapping_sub(old)
            .wrapping_add(quantity);
        level.update_quantities(old, 0, quantity, 0, &mut |event| publish.level(event));
        self.publish_price_change(before, publish);
    }

    /// Keep the best `depth` visible aggregate quotes, publishing zero for each
    /// eviction through the same level mutation. Depth zero clears a snapshot.
    /// Use only with `set_level_quantity`; this does not remove arena orders.
    pub fn retain_level_quantities(
        &mut self,
        depth: usize,
        publish: &mut impl MutationPublisher<L::Price>,
    ) {
        let before = publish.observe_price(|| self.top_level().map(PriceLevelContract::price));
        for level in self
            .orders
            .values_mut()
            .filter(|level| level.visible_quantity() != 0)
            .skip(depth)
        {
            let old = level.visible_quantity();
            self.visible_quantity = self.visible_quantity.wrapping_sub(old);
            level.update_quantities(old, 0, 0, 0, &mut |event| publish.level(event));
        }
        self.orders.retain(|_, level| level.visible_quantity() != 0);
        self.publish_price_change(before, publish);
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn prune(
        &mut self,
        arena: &mut Arena<RestingOrder<L::Price>>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) {
        let before = publish.observe_price(|| self.top_level().map(PriceLevelContract::price));
        for level in self.orders.values_mut() {
            level.for_each_order_mut(arena, |_| {}, &mut |event| publish.level(event));
        }
        // Pruning cleans each level in place without removing its map entry.
        self.recalculate_aggregates();
        self.publish_price_change(before, publish);
    }

    fn recalculate_aggregates(&mut self) {
        self.visible_quantity = self
            .orders
            .values()
            .map(|level| level.visible_quantity())
            .sum();

        self.hidden_quantity = H::aggregate_levels_hidden_quantities(&self.orders);

        self.len = self.orders.values().map(|level| level.len()).sum();
    }
    #[inline(always)]
    fn replace_quantity_unchecked(total: u64, old: u64, new: u64) -> u64 {
        total.wrapping_sub(old).wrapping_add(new)
    }

    fn replace_len(total: usize, old: usize, new: usize) -> usize {
        total
            .checked_sub(old)
            .and_then(|len| len.checked_add(new))
            .expect("side live order count update overflowed or underflowed")
    }
    pub fn insert<Q: TryInto<L::Price>>(&mut self, price: Q) {
        let price = Self::checked_price(price);
        self.orders
            .get_or_insert_with(S::into_key(price), || L::new(price, S::SIDE));
    }
    pub fn contains_key<Q: TryInto<L::Price>>(&self, price: Q) -> bool {
        let price = Self::checked_price(price);
        self.orders.contains_key(&S::into_key(price))
    }

    pub fn get<Q: TryInto<L::Price>>(&self, price: Q) -> Option<&L> {
        let price = Self::checked_price(price);
        self.orders.get(&S::into_key(price))
    }

    pub fn get_mut<Q: TryInto<L::Price>>(&mut self, price: Q) -> Option<&mut L> {
        let price = Self::checked_price(price);
        self.orders.get_mut(&S::into_key(price))
    }
    #[inline(always)]
    fn mutate_visible(&mut self, order: &RestingOrder<L::Price>) {
        self.len += 1;
        self.visible_quantity += order.quantity();
    }

    pub fn push_order(
        &mut self,
        arena: &mut Arena<RestingOrder<L::Price>>,
        order: RestingOrder<L::Price>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> SubmittedData {
        let before = publish.observe_price(|| self.top_level().map(PriceLevelContract::price));
        H::add_hidden_quantity(
            &mut self.hidden_quantity,
            H::order_hidden_quantity(&order.typed_order_details.replenishment_behavior),
        );
        self.mutate_visible(&order);
        let order_id = order.uuid();
        let user_id = order.common_data.trader;
        let price = order.price().unwrap();
        let arena_key = self
            .orders
            .get_or_insert_with(S::into_key(price), || L::new(price, S::SIDE))
            .push(arena, order, &mut |event| publish.level(event));

        self.publish_price_change(before, publish);
        SubmittedData {
            order_id,
            user_id,
            arena_key,
        }
    }

    pub fn update_order_quantities(
        &mut self,
        price: L::Price,
        old_visible_quantity: u64,
        old_hidden_quantity: u64,
        new_visible_quantity: u64,
        new_hidden_quantity: u64,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> bool {
        let before = publish.observe_price(|| self.top_level().map(PriceLevelContract::price));
        {
            let Some(level) = self.orders.get_mut(&S::into_key(price)) else {
                return false;
            };
            // SAFETY: replay obtains the order from its live index, so its price
            // level is resident on the order's side.
            // TODO TEST IF THIS IS FASTER:
            // let level = unsafe { self.orders.get_mut(&S::into_key(price)).unwrap_unchecked() };

            level.update_quantities(
                old_visible_quantity,
                old_hidden_quantity,
                new_visible_quantity,
                new_hidden_quantity,
                &mut |event| publish.level(event),
            );
        }

        H::sub_hidden_quantity(&mut self.hidden_quantity, old_hidden_quantity);
        H::add_hidden_quantity(&mut self.hidden_quantity, new_hidden_quantity);

        self.visible_quantity = self
            .visible_quantity
            .wrapping_sub(old_visible_quantity)
            .wrapping_add(new_visible_quantity);
        self.publish_price_change(before, publish);
        true
    }

    /// Remove one order from live aggregates without searching the FIFO for
    /// its arena key. Queue traversal owns physical tombstone cleanup.
    pub(crate) fn mark_order_removed(
        &mut self,
        arena: &mut Arena<RestingOrder<L::Price>>,
        links: ArenaLinks,
        price: L::Price,
        visible_quantity: u64,
        hidden_quantity: u64,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> bool {
        let before = publish.observe_price(|| self.top_level().map(PriceLevelContract::price));
        let Some(level) = self.orders.get_mut(&S::into_key(price)) else {
            return false;
        };

        level.pop_virtual(
            arena,
            links,
            visible_quantity,
            hidden_quantity,
            &mut |event| publish.level(event),
        );
        self.visible_quantity =
            Self::replace_quantity_unchecked(self.visible_quantity, visible_quantity, 0);
        self.hidden_quantity =
            Self::replace_quantity_unchecked(self.hidden_quantity, hidden_quantity, 0);
        self.len = self
            .len
            .checked_sub(1)
            .expect("side live order count underflowed");
        self.publish_price_change(before, publish);
        true
    }

    #[inline(always)]
    pub(crate) fn mark_order_removed_unchecked(
        &mut self,
        arena: &mut Arena<RestingOrder<L::Price>>,
        links: ArenaLinks,
        price: L::Price,
        visible_quantity: u64,
        hidden_quantity: u64,
        publish: &mut impl MutationPublisher<L::Price>,
    ) {
        let before = publish.observe_price(|| self.top_level().map(PriceLevelContract::price));
        // SAFETY: unchecked removal is only called for a live indexed order,
        // whose price level is therefore resident on this side.
        let level = unsafe { self.orders.get_mut(&S::into_key(price)).unwrap_unchecked() };
        level.pop_virtual(
            arena,
            links,
            visible_quantity,
            hidden_quantity,
            &mut |event| publish.level(event),
        );
        self.visible_quantity -= visible_quantity;
        H::sub_hidden_quantity(&mut self.hidden_quantity, hidden_quantity);
        self.len -= 1;
        self.publish_price_change(before, publish);
    }

    pub fn best(&self) -> Option<(&L::Price, &L)> {
        let (key, level) = self.orders.iter().find(|(_, level)| !level.is_empty())?;
        Some((S::price_from_key(key), level))
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[cfg(feature = "python-polars")]
    pub fn price_levels_df(&self, n_rows: Option<usize>) -> PyResult<PyDataFrame>
    where
        u64: From<L::Price>,
    {
        let active_level_count = self.price_level_count();
        let n = n_rows.unwrap_or(active_level_count).min(active_level_count);

        let mut prices = Vec::with_capacity(n);
        let mut visible_quantities = Vec::with_capacity(n);
        let mut hidden_quantities = Vec::with_capacity(n);
        let mut order_counts = Vec::with_capacity(n);

        for (_, level) in self
            .orders
            .iter()
            .filter(|(_, level)| !level.is_empty())
            .take(n)
        {
            // Replace as_i64() with whatever primitive representation
            // your Price type exposes.
            prices.push(u64::from(level.price()));

            visible_quantities.push(level.visible_quantity());
            hidden_quantities.push(level.hidden_quantity());
            order_counts.push(level.len() as u64);
        }

        let df = DataFrame::new_infer_height(vec![
            Column::new("price".into(), prices),
            Column::new("quantity".into(), visible_quantities),
            Column::new("hidden_quantity".into(), hidden_quantities),
            Column::new("order_count".into(), order_counts),
        ])
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

        Ok(PyDataFrame(df))
    }
}
impl<Pr: PriceType> SidedPrice<Pr> for Bid {
    const SIDE: Side = Side::Buy;
    type PriceKey = Reverse<Pr>;
    fn into_key(price: Pr) -> Self::PriceKey {
        Reverse(price)
    }
    fn from_key(Reverse(price): Self::PriceKey) -> Pr {
        price
    }

    fn price_from_key(key: &Self::PriceKey) -> &Pr {
        &key.0
    }
}

impl<Pr: PriceType> SidedPrice<Pr> for Ask {
    const SIDE: Side = Side::Sell;
    type PriceKey = Pr;
    fn into_key(price: Pr) -> Self::PriceKey {
        price
    }
    fn from_key(price: Self::PriceKey) -> Pr {
        price
    }

    fn price_from_key(key: &Self::PriceKey) -> &Pr {
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lobo_models::{
        Side,
        orders::{
            order_types::{IcebergOrder, LimitOrder, MarketOrder},
            traits::Trades,
        },
    };

    use lobo_primitives::{CompressedPrice, uuid::Uuid};

    use crate::policies::UpdateHiddenQuantity;
    use crate::price_level::DeepPriceLevel;

    type Level = DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>;

    fn p(raw: u32) -> CompressedPrice {
        CompressedPrice::from(raw)
    }

    #[test]
    fn side_key_conversions_define_best_price_ordering() {
        assert_eq!(Bid::from_key(Bid::into_key(p(101))), 101);
        assert_eq!(Ask::from_key(Ask::into_key(p(99))), 99);
        assert_eq!(*Bid::price_from_key(&Bid::into_key(p(102))), 102);
        assert_eq!(*Ask::price_from_key(&Ask::into_key(p(98))), 98);

        let mut arena = Arena::default();
        let mut bids = SidedOrderStore::<Bid, Level>::new();
        bids.push_order(
            &mut arena,
            LimitOrder::new(Some(99), 1, Uuid::new_v4(), Side::Buy).into(),
            &mut |_| {},
        );
        bids.push_order(
            &mut arena,
            LimitOrder::new(Some(101), 1, Uuid::new_v4(), Side::Buy).into(),
            &mut |_| {},
        );
        assert_eq!(*bids.best().unwrap().0, 101);

        let mut asks = SidedOrderStore::<Ask, Level>::new();
        asks.push_order(
            &mut arena,
            LimitOrder::new(Some(101), 1, Uuid::new_v4(), Side::Sell).into(),
            &mut |_| {},
        );
        asks.push_order(
            &mut arena,
            LimitOrder::new(Some(99), 1, Uuid::new_v4(), Side::Sell).into(),
            &mut |_| {},
        );
        assert_eq!(*asks.best().unwrap().0, 99);
    }

    #[test]
    fn map_operations_preserve_existing_levels_and_aggregates() {
        let mut arena = Arena::default();
        let mut store = SidedOrderStore::<Bid, Level>::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        store.insert(100);
        assert!(store.contains_key(100));
        assert_eq!(store.get(100).unwrap().price(), 100);
        assert_eq!(store.get_mut(100).unwrap().price(), 100);

        store.push_order(
            &mut arena,
            LimitOrder::new(Some(100), 3, Uuid::new_v4(), Side::Buy).into(),
            &mut |_| {},
        );
        assert_eq!(store.len(), 1);
        assert_eq!(store.visible_quantity, 3);

        store.insert(100);
        assert_eq!(store.get(100).unwrap().len(), 1);
        assert_eq!(store.len(), 1);
        assert_eq!(store.visible_quantity, 3);
    }

    #[test]
    fn push_order_tracks_hidden_quantity_and_submission_ids() {
        let mut arena = Arena::default();
        let mut store = SidedOrderStore::<Ask, Level>::new();
        let user = Uuid::new_v4();
        let order = IcebergOrder::new(Some(105), user, Side::Sell, 9, 3);
        let order_id = order.uuid();

        let submitted = store.push_order(&mut arena, order.into(), &mut |_| {});
        assert_eq!(submitted.order_id, order_id);
        assert_eq!(submitted.user_id, user);
        assert_eq!(arena.get(submitted.arena_key).unwrap().uuid(), order_id);
        assert_eq!(store.visible_quantity, 3);
        assert_eq!(store.hidden_quantity, 9);
        assert_eq!(store.price_level_count(), 1);
    }

    #[test]
    fn change_order_price_relocates_fifo_key_and_updates_aggregates() {
        let mut arena = Arena::default();
        let mut bids = SidedOrderStore::<Bid, Level>::new();
        let moved = bids.push_order(
            &mut arena,
            LimitOrder::new(Some(100), 3, Uuid::new_v4(), Side::Buy).into(),
            &mut |_| {},
        );
        bids.push_order(
            &mut arena,
            LimitOrder::new(Some(100), 2, Uuid::new_v4(), Side::Buy).into(),
            &mut |_| {},
        );

        let order = arena.get_mut(moved.arena_key).unwrap();
        order.common_data.price = Some(p(101));
        order.common_data.quantity = 5;
    }

    #[test]
    fn price_levels_iterate_in_matching_priority_order() {
        let mut arena = Arena::default();
        let mut bids = SidedOrderStore::<Bid, Level>::new();
        for price in [100, 102, 101] {
            bids.push_order(
                &mut arena,
                LimitOrder::new(Some(price), 1, Uuid::new_v4(), Side::Buy).into(),
                &mut |_| {},
            );
        }
        let prices: Vec<_> = bids.price_levels().map(|(price, _)| *price).collect();
        assert_eq!(prices, [102, 101, 100]);

        let mut asks = SidedOrderStore::<Ask, Level>::new();
        for price in [100, 102, 101] {
            asks.push_order(
                &mut arena,
                LimitOrder::new(Some(price), 1, Uuid::new_v4(), Side::Sell).into(),
                &mut |_| {},
            );
        }
        let prices: Vec<_> = asks.price_levels().map(|(price, _)| *price).collect();
        assert_eq!(prices, [100, 101, 102]);
    }

    #[test]
    fn selection_respects_limit_and_skips_empty_levels() {
        let mut arena = Arena::default();
        let mut store = SidedOrderStore::<Ask, Level>::new();
        store.insert(99);
        for price in [100, 101, 102] {
            store.push_order(
                &mut arena,
                LimitOrder::new(Some(price), 1, Uuid::new_v4(), Side::Sell).into(),
                &mut |_| {},
            );
        }
        let limit = LimitOrder::new(Some(101), 1, Uuid::new_v4(), Side::Buy);
        let selected = store.select_price_levels_for_order(&limit).count();
        assert_eq!(selected, 2);

        let market = MarketOrder::new(1, Uuid::new_v4(), Side::Buy);
        let selected = store.select_price_levels_for_order(&market).count();
        assert_eq!(selected, 3);

        let mut bids = SidedOrderStore::<Bid, Level>::new();
        bids.insert(103);
        for price in [100, 101, 102] {
            bids.push_order(
                &mut arena,
                LimitOrder::new(Some(price), 1, Uuid::new_v4(), Side::Buy).into(),
                &mut |_| {},
            );
        }
        let limit = LimitOrder::new(Some(101), 1, Uuid::new_v4(), Side::Sell);
        let selected = bids.select_price_levels_for_order(&limit).count();
        assert_eq!(selected, 2);

        let market = MarketOrder::new(1, Uuid::new_v4(), Side::Sell);
        let selected = bids.select_price_levels_for_order(&market).count();
        assert_eq!(selected, 3);
    }

    #[test]
    fn fill_retains_depleted_levels_and_updates_totals() {
        let mut arena = Arena::default();
        let mut asks = SidedOrderStore::<Ask, Level>::new();
        asks.push_order(
            &mut arena,
            LimitOrder::new(Some(100), 2, Uuid::new_v4(), Side::Sell).into(),
            &mut |_| {},
        );
        asks.push_order(
            &mut arena,
            LimitOrder::new(Some(101), 4, Uuid::new_v4(), Side::Sell).into(),
            &mut |_| {},
        );

        let result = asks.fill(
            &mut arena,
            LimitOrder::new(Some(100), 2, Uuid::new_v4(), Side::Buy),
            true,
            &mut |_, _| {},
            &mut |_| {},
        );
        assert_eq!(result.match_result.unwrap().fills[0].fill_quantity, 2);
        assert_eq!(asks.len(), 1);
        assert_eq!(asks.visible_quantity, 4);
        assert_eq!(asks.price_level_count(), 1);
        assert!(asks.get(100).unwrap().is_empty());
        assert!(asks.get(100).unwrap().queue_is_empty());
        assert_eq!(*asks.best().unwrap().0, 101);

        let no_fill = asks.fill(
            &mut arena,
            LimitOrder::new(Some(99), 1, Uuid::new_v4(), Side::Buy),
            false,
            &mut |_, _| {},
            &mut |_| {},
        );
        assert!(no_fill.match_result.is_none());
        assert_eq!(no_fill.remaining_order.unwrap().quantity(), 1);
    }

    #[test]
    fn prune_removes_stale_orders_but_retains_empty_levels() {
        let mut arena = Arena::default();
        let mut bids = SidedOrderStore::<Bid, Level>::new();
        let submitted = bids.push_order(
            &mut arena,
            LimitOrder::new(Some(100), 2, Uuid::new_v4(), Side::Buy).into(),
            &mut |_| {},
        );
        let removed = arena.remove_and_return_node(submitted.arena_key).unwrap();
        bids.mark_order_removed_unchecked(&mut arena, removed.links(), p(100), 2, 0, &mut |_| {});

        bids.prune(&mut arena, &mut |_| {});
        assert!(bids.is_empty());
        assert_eq!(bids.len(), 0);
        assert_eq!(bids.visible_quantity, 0);
        assert!(bids.get(100).unwrap().is_empty());
        assert!(bids.get(100).unwrap().queue_is_empty());
    }

    #[test]
    fn relocating_last_order_retains_empty_source_level() {
        let mut arena = Arena::default();
        let mut bids = SidedOrderStore::<Bid, Level>::new();
        let moved = bids.push_order(
            &mut arena,
            LimitOrder::new(Some(100), 2, Uuid::new_v4(), Side::Buy).into(),
            &mut |_| {},
        );

        arena.get_mut(moved.arena_key).unwrap().common_data.price = Some(p(101));
    }

    #[cfg(feature = "python-polars")]
    #[test]
    fn price_levels_dataframe_respects_side_order_and_row_limit() {
        let mut arena = Arena::default();
        let mut bids = SidedOrderStore::<Bid, Level>::new();
        for price in [100, 102, 101] {
            bids.push_order(
                &mut arena,
                LimitOrder::new(Some(price), 2, Uuid::new_v4(), Side::Buy).into(),
                &mut |_| {},
            );
        }

        let frame = bids.price_levels_df(Some(2)).unwrap().0;
        assert_eq!(frame.height(), 2);
        assert_eq!(frame.width(), 4);
        assert_eq!(
            frame.get_column_names(),
            ["price", "quantity", "hidden_quantity", "order_count"]
        );

        let all = bids.price_levels_df(None).unwrap().0;
        assert_eq!(all.height(), 3);
    }
}
