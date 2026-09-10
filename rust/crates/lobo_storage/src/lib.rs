use crate::policies::checksum::ChecksumPolicy;
#[doc(hidden)]
pub use paste::paste as __paste;
use lobo_events::{MutationPublisher, TradedVolumeEvent};
use std::{
    collections::{HashMap, HashSet},
    marker::PhantomData,
};
pub mod policies;
use lobo_models::{
    Side,
    events::{
        ExecutionResult, FillResult, MarketImpactContext, OrderDetails, OrderTransition, Reports,
        build_report,
    },
    orders::{
        core::{Order, ReplenishmentBehavior, RestingOrder},
        traits::{HandlesCompletion, IntoRestingOrderData, Trades},
    },
};
#[cfg(feature = "python")]
use lobo_primitives::mixins::{DisplayFields, PyDisplay};

use lobo_primitives::{Notional, PriceType, uuid::Uuid};

use crate::{
    arena::arenav1::{Arena, ArenaKey, ArenaLinks},
    bidask::{
        Ask, Bid, SidedOrderStore,
        traits::{Buy, OrderSide, Sell, SidedPrice},
    },
    policies::{HiddenQuantityPolicy, UpdateHiddenQuantity},
    price_level::PriceLevelContract,
    price_sorting::{PriceSortingPolicy, SortedVectorPriceSorting},
};
pub mod arena;

pub mod price_sorting;
pub mod sorted_vector;

pub mod price_level;

pub mod bidask;
#[cfg(feature = "python")]
use pyo3::prelude::*;

// type OrderIndex = rustc_hash::FxHashMap<Uuid, ArenaKey>;
type OrderIndex = rustc_hash::FxHashMap<Uuid, ArenaKey>;

pub struct OrderStorage<
    L: PriceLevelContract,
    Sort: PriceSortingPolicy = SortedVectorPriceSorting,
    U: UserMapUpdatePolicy = UpdateUserMap,
    H: HiddenQuantityPolicy = UpdateHiddenQuantity,
> {
    arena: Arena<RestingOrder<L::Price>>,
    pub user_to_orders_map: HashMap<Uuid, HashSet<ArenaKey>>,
    pub order_to_arena_map: OrderIndex,
    pub bids: SidedOrderStore<Bid, L, Sort, H>,
    pub asks: SidedOrderStore<Ask, L, Sort, H>,
    marker: PhantomData<U>,
    marker_h: PhantomData<H>,
    checksum: <L::Checksum as ChecksumPolicy>::State,
}

impl<L, Sort, U, H> Clone for OrderStorage<L, Sort, U, H>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    SidedOrderStore<Bid, L, Sort, H>: Clone,
    SidedOrderStore<Ask, L, Sort, H>: Clone,
{
    fn clone(&self) -> Self {
        Self {
            arena: self.arena.clone(),
            user_to_orders_map: self.user_to_orders_map.clone(),
            order_to_arena_map: self.order_to_arena_map.clone(),
            bids: self.bids.clone(),
            asks: self.asks.clone(),
            marker: PhantomData,
            marker_h: PhantomData,
            checksum: self.checksum.clone(),
        }
    }
}

// struct ReplacementState<Pr: PriceType> {
//     old_price: Pr,
//     old_quantity: u64,
//     old_hidden_quantity: u64,
//     old_side: Side,
//     new_order_id: Uuid,
//     new_price: Pr,
//     new_quantity: u64,
//     new_hidden_quantity: u64,
//     new_side: Side,
// }

// struct VisibleReplacementState<Pr: PriceType> {
//     old_price: Pr,
//     new_price: Pr,
//     old_quantity: u64,
//     new_quantity: u64,
//     old_side: Side,
//     new_order_id: Uuid,
//     new_side: Side,
// }

struct ModificationState<Pr: PriceType> {
    price: Pr,
    old_quantity: u64,
    old_hidden_quantity: u64,
    // side: Side,
    new_quantity: u64,
    new_hidden_quantity: u64,
}

// struct VisibleModificationState<Pr: PriceType> {
//     price: Pr,
//     old_quantity: u64,
//     // side: Side,
//     new_quantity: u64,
// }

// impl<Pr: PriceType> VisibleModificationState<Pr> {
//     #[inline(always)]
//     fn is_depleted(&self) -> bool {
//         self.new_quantity == 0
//     }
// }

impl<Pr: PriceType> ModificationState<Pr> {
    #[inline(always)]
    fn is_depleted(&self) -> bool {
        self.new_quantity == 0 && self.new_hidden_quantity == 0
    }
}
// trait UpdateQuantity {
//     fn substract_quantities(&self, price_level: &PriceLevel);
//     fn get_hidden_quantity(&self);
// }
// impl UpdateQuantity for ReplacementState {
//     fn substract_quantities(&self, price_level: &mut PriceLevel)where PriceLevel:PriceLevelContract {
//         price_level.substract_order();
//         price_level.visible_quantity -= self.old_quantity;
//         price_level.hidden_quantity -= self.old_hidden_quantity;
//     }
// }
// impl<Pr: PriceType> ReplacementState<Pr> {
//     #[inline(always)]
//     fn side_changed(&self) -> bool {
//         self.old_side != self.new_side
//     }
//     #[inline(always)]
//     fn price_changed(&self) -> bool {
//         self.old_price != self.new_price
//     }
// }
// impl<Pr: PriceType> VisibleReplacementState<Pr> {
//     #[inline(always)]
//     fn side_changed(&self) -> bool {
//         self.old_side != self.new_side
//     }
//     #[inline(always)]
//     fn price_changed(&self) -> bool {
//         self.old_price != self.new_price
//     }
// }

impl<L, Sort, U, H> Default for OrderStorage<L, Sort, U, H>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
{
    fn default() -> Self {
        Self {
            arena: Arena::default(),
            user_to_orders_map: HashMap::new(),
            order_to_arena_map: OrderIndex::default(),
            bids: SidedOrderStore::default(),
            asks: SidedOrderStore::default(),
            marker: PhantomData,
            marker_h: PhantomData,
            checksum: L::Checksum::new_state(),
        }
    }
}

#[derive(Clone, PartialEq)]
#[cfg_attr(feature = "python", pyclass(from_py_object,extends=PyDisplay, module = "lobo.storage"))]
pub struct UserOutstandingLiquidity {
    pub user_id: Uuid,
    pub bid_order_count: usize,
    pub ask_order_count: usize,
    pub bid_visible_quantity: u64,
    pub bid_visible_value: Notional,
    pub bid_visible_vwap: f32,
    pub ask_visible_quantity: u64,
    pub ask_visible_value: Notional,
    pub ask_visible_vwap: f32,
    pub bid_hidden_quantity: u64,
    pub bid_hidden_value: Notional,
    pub bid_hidden_vwap: f32,
    pub ask_hidden_quantity: u64,
    pub ask_hidden_value: Notional,
    pub ask_hidden_vwap: f32,
    visible_value: Notional,
    visible_vwap: f32,
    hidden_value: Notional,
    hidden_vwap: f32,
    value: Notional,
    vwap: f32,
}

impl UserOutstandingLiquidity {
    #[cfg(feature = "python")]
    pub fn into_python(self, py: Python<'_>) -> PyResult<Py<Self>> {
        let initializer = PyClassInitializer::from(PyDisplay).add_subclass(self);

        Py::new(py, initializer)
    }

    fn new(user_id: Uuid) -> Self {
        Self {
            user_id,
            bid_order_count: 0,
            ask_order_count: 0,
            bid_visible_quantity: 0,
            bid_visible_value: 0,
            bid_visible_vwap: 0.0,
            ask_visible_quantity: 0,
            ask_visible_value: 0,
            ask_visible_vwap: 0.0,
            bid_hidden_quantity: 0,
            bid_hidden_value: 0,
            bid_hidden_vwap: 0.0,
            ask_hidden_quantity: 0,
            ask_hidden_value: 0,
            ask_hidden_vwap: 0.0,
            visible_value: 0,
            visible_vwap: 0.0,
            hidden_value: 0,
            hidden_vwap: 0.0,
            value: 0,
            vwap: 0.0,
        }
    }

    pub fn order_count(&self) -> usize {
        self.bid_order_count + self.ask_order_count
    }
    pub fn visible_quantity(&self) -> u64 {
        self.bid_visible_quantity + self.ask_visible_quantity
    }
    pub fn hidden_quantity(&self) -> u64 {
        self.bid_hidden_quantity + self.ask_hidden_quantity
    }
    pub fn quantity(&self) -> u64 {
        self.visible_quantity() + self.hidden_quantity()
    }
    pub fn visible_value(&self) -> Notional {
        self.visible_value
    }
    pub fn visible_vwap(&self) -> f32 {
        self.visible_vwap
    }
    pub fn hidden_value(&self) -> Notional {
        self.hidden_value
    }
    pub fn hidden_vwap(&self) -> f32 {
        self.hidden_vwap
    }
    pub fn value(&self) -> Notional {
        self.value
    }
    pub fn vwap(&self) -> f32 {
        self.vwap
    }
}
pub enum UserStateError {
    UserAlreadyExists,
    UserDoesNotExists,
    ConflictingStateError,
}
#[derive(Debug)]

pub enum OrderStateError {
    OrderAlreadyExists,
    OrderDoesNotExists,
}

#[cfg(feature = "python")]
#[pymethods]
impl UserOutstandingLiquidity {
    #[getter(order_count)]
    fn py_order_count(&self) -> usize {
        self.order_count()
    }

    #[getter(visible_quantity)]
    fn py_visible_quantity(&self) -> u64 {
        self.visible_quantity()
    }

    #[getter(hidden_quantity)]
    fn py_hidden_quantity(&self) -> u64 {
        self.hidden_quantity()
    }

    #[getter(quantity)]
    fn py_quantity(&self) -> u64 {
        self.quantity()
    }

    #[getter(visible_value)]
    fn py_visible_value(&self) -> Notional {
        self.visible_value()
    }

    #[getter(visible_vwap)]
    fn py_visible_vwap(&self) -> f32 {
        self.visible_vwap()
    }

    #[getter(hidden_value)]
    fn py_hidden_value(&self) -> Notional {
        self.hidden_value()
    }

    #[getter(hidden_vwap)]
    fn py_hidden_vwap(&self) -> f32 {
        self.hidden_vwap()
    }

    #[getter(value)]
    fn py_value(&self) -> Notional {
        self.value()
    }

    #[getter(vwap)]
    fn py_vwap(&self) -> f32 {
        self.vwap()
    }
    #[classattr]
    fn __display_fields__() -> Vec<&'static str> {
        Self::display_fields()
    }
}
#[cfg(feature = "python")]
impl DisplayFields for UserOutstandingLiquidity {
    fn display_fields() -> Vec<&'static str> {
        vec![
            "order_count",
            "visible_quantity",
            "hidden_quantity",
            "quantity",
            "visible_value",
            "visible_vwap",
            "hidden_value",
            "hidden_vwap",
            "value",
            "vwap",
        ]
    }
}

pub struct UpdateUserMap;
pub struct DoNotUpdateUserMap;

pub trait UserMapUpdatePolicy {
    const ENABLED: bool;
    fn update_user_map(
        map: &mut HashMap<Uuid, HashSet<ArenaKey>>,
        trader_id: Uuid,
        arena_key: ArenaKey,
    );
    fn remove_from_user_map(
        map: &mut HashMap<Uuid, HashSet<ArenaKey>>,
        trader_id: Uuid,
        arena_key: ArenaKey,
    );
}
impl UserMapUpdatePolicy for UpdateUserMap {
    const ENABLED: bool = true;
    fn update_user_map(
        map: &mut HashMap<Uuid, HashSet<ArenaKey>>,
        trader_id: Uuid,
        arena_key: ArenaKey,
    ) {
        map.entry(trader_id).or_default().insert(arena_key);
    }
    fn remove_from_user_map(
        map: &mut HashMap<Uuid, HashSet<ArenaKey>>,
        trader_id: Uuid,
        arena_key: ArenaKey,
    ) {
        map.entry(trader_id).and_modify(|f| {
            f.remove(&arena_key);
        });
    }
}
impl UserMapUpdatePolicy for DoNotUpdateUserMap {
    const ENABLED: bool = false;
    fn update_user_map(
        _map: &mut HashMap<Uuid, HashSet<ArenaKey>>,
        _trader_id: Uuid,
        _arena_key: ArenaKey,
    ) {
    }
    fn remove_from_user_map(
        _map: &mut HashMap<Uuid, HashSet<ArenaKey>>,
        _trader_id: Uuid,
        _arena_key: ArenaKey,
    ) {
    }
}
pub trait ExecutionPolicy {
    fn execute<S, T, L, Sort, H>(
        &self,
        store: &mut SidedOrderStore<S, L, Sort, H>,
        arena: &mut Arena<RestingOrder<L::Price>>,
        order: Order<T, L::Price>,
        include_match_result: bool,
        on_depleted: &mut dyn FnMut(Uuid, Uuid),
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> ExecutionResult<L::Price>
    where
        S: SidedPrice<L::Price>,
        L: PriceLevelContract,
        Sort: PriceSortingPolicy,
        H: HiddenQuantityPolicy,
        Order<T, L::Price>: HandlesCompletion<L::Price>;

    #[inline(always)]
    fn commit_remaining<S, L, Sort, U, H>(
        &self,
        _user_dict: &mut HashMap<Uuid, HashSet<ArenaKey>>,
        _order_dict: &mut OrderIndex,
        _store: &mut SidedOrderStore<S, L, Sort, H>,
        _arena: &mut Arena<RestingOrder<L::Price>>,
        _remaining: &mut Option<RestingOrder<L::Price>>,
        _publish: &mut impl MutationPublisher<L::Price>,
    ) where
        S: SidedPrice<L::Price>,
        L: PriceLevelContract,
        Sort: PriceSortingPolicy,
        H: HiddenQuantityPolicy,
        U: UserMapUpdatePolicy,
    {
    }
}

fn market_impact_context<S, L, Sort, H>(
    store: &SidedOrderStore<S, L, Sort, H>,
) -> MarketImpactContext<L::Price>
where
    S: SidedPrice<L::Price>,
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    H: HiddenQuantityPolicy,
{
    MarketImpactContext {
        best_price: store.best().map(|(price, _)| *price),
        total_quantity_available: store
            .visible_quantity
            .checked_add(store.hidden_quantity)
            .expect("total available quantity overflowed u64"),
    }
}

pub struct MutatingFills;
pub struct SimulatedFills;
/// Preview L2 quantities through the fill API without inventing maker orders.
pub struct SimulatedAggregateFills;

/// Reuse an execution policy while leaving its remainder uncommitted (IOC).
/// Selected statically; normal fill paths gain no policy checks.
pub struct WithoutRest<E>(pub E);
impl<E: ExecutionPolicy> ExecutionPolicy for WithoutRest<E> {
    #[inline(always)]
    fn execute<S, T, L, Sort, H>(
        &self,
        store: &mut SidedOrderStore<S, L, Sort, H>,
        arena: &mut Arena<RestingOrder<L::Price>>,
        order: Order<T, L::Price>,
        include_match_result: bool,
        on_depleted: &mut dyn FnMut(Uuid, Uuid),
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> ExecutionResult<L::Price>
    where
        S: SidedPrice<L::Price>,
        L: PriceLevelContract,
        Sort: PriceSortingPolicy,
        H: HiddenQuantityPolicy,
        Order<T, L::Price>: HandlesCompletion<L::Price>,
    {
        self.0.execute(
            store,
            arena,
            order,
            include_match_result,
            on_depleted,
            publish,
        )
    }
}

impl<L, Sort> OrderStorage<L, Sort>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
{
    pub fn new() -> Self {
        Self::default()
    }
}

impl ExecutionPolicy for MutatingFills {
    #[inline(always)]
    fn execute<S, T, L, Sort, H>(
        &self,
        store: &mut SidedOrderStore<S, L, Sort, H>,
        arena: &mut Arena<RestingOrder<L::Price>>,
        order: Order<T, L::Price>,
        include_match_result: bool,
        on_depleted: &mut dyn FnMut(Uuid, Uuid),
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> ExecutionResult<L::Price>
    where
        S: SidedPrice<L::Price>,
        L: PriceLevelContract,
        Sort: PriceSortingPolicy,
        H: HiddenQuantityPolicy,
        Order<T, L::Price>: HandlesCompletion<L::Price>,
    {
        store.fill(arena, order, include_match_result, on_depleted, publish)
    }

    #[inline(always)]
    fn commit_remaining<S, L, Sort, U, H>(
        &self,
        user_dict: &mut HashMap<Uuid, HashSet<ArenaKey>>,
        order_dict: &mut OrderIndex,
        store: &mut SidedOrderStore<S, L, Sort, H>,
        arena: &mut Arena<RestingOrder<L::Price>>,
        remaining: &mut Option<RestingOrder<L::Price>>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) where
        S: SidedPrice<L::Price>,
        L: PriceLevelContract,
        Sort: PriceSortingPolicy,
        H: HiddenQuantityPolicy,
        U: UserMapUpdatePolicy,
    {
        let Some(order) = remaining.take() else {
            return;
        };

        let data = store.push_order(arena, order, publish);

        order_dict.insert(data.order_id, data.arena_key);

        U::update_user_map(user_dict, data.user_id, data.arena_key);
    }
}

impl ExecutionPolicy for SimulatedFills {
    #[inline(always)]
    fn execute<S, T, L, Sort, H>(
        &self,
        store: &mut SidedOrderStore<S, L, Sort, H>,
        arena: &mut Arena<RestingOrder<L::Price>>,
        order: Order<T, L::Price>,
        include_match_result: bool,
        _on_depleted: &mut dyn FnMut(Uuid, Uuid),
        _publish: &mut impl MutationPublisher<L::Price>,
    ) -> ExecutionResult<L::Price>
    where
        S: SidedPrice<L::Price>,
        L: PriceLevelContract,
        Sort: PriceSortingPolicy,
        H: HiddenQuantityPolicy,
        Order<T, L::Price>: HandlesCompletion<L::Price>,
    {
        store.simulate(arena, order, include_match_result)
    }
}

impl ExecutionPolicy for SimulatedAggregateFills {
    #[inline(always)]
    fn execute<S, T, L, Sort, H>(
        &self,
        store: &mut SidedOrderStore<S, L, Sort, H>,
        _arena: &mut Arena<RestingOrder<L::Price>>,
        order: Order<T, L::Price>,
        include_match_result: bool,
        _on_depleted: &mut dyn FnMut(Uuid, Uuid),
        _publish: &mut impl MutationPublisher<L::Price>,
    ) -> ExecutionResult<L::Price>
    where
        S: SidedPrice<L::Price>,
        L: PriceLevelContract,
        Sort: PriceSortingPolicy,
        H: HiddenQuantityPolicy,
        Order<T, L::Price>: HandlesCompletion<L::Price>,
    {
        store.simulate_aggregate(order, include_match_result)
    }
}

impl<L, Sort, U, H> OrderStorage<L, Sort, U, H>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
{
    pub fn submit_order<T, E>(
        &mut self,
        order: Order<T, L::Price>,
        ex: E,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> FillResult<L::Price>
    where
        Order<T, L::Price>: HandlesCompletion<L::Price>,
        E: ExecutionPolicy,
    {
        self.submit_order_with_reports(order, ex, Reports::default(), publish)
    }

    pub fn submit_order_with_reports<T, E>(
        &mut self,
        mut order: Order<T, L::Price>,
        ex: E,
        reports: Reports,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> FillResult<L::Price>
    where
        Order<T, L::Price>: HandlesCompletion<L::Price>,
        E: ExecutionPolicy,
    {
        H::prepare_incoming(&mut order);
        let order_id = order.uuid();
        let trader_id = order.common_data.trader;
        let side = order.side();
        let market_context = reports.include_market_impact.then(|| match side {
            Side::Buy => market_impact_context(&self.asks),
            Side::Sell => market_impact_context(&self.bids),
        });
        let arena = &mut self.arena;
        let order_dict = &mut self.order_to_arena_map;
        let mut on_depleted = |maker_order_id, _maker_trader_uid| {
            let Some((_, _arena_key)) = order_dict.remove_entry(&maker_order_id) else {
                panic!("could not remove an exhausted trade, book is corrupted");
            };
        };

        // Incoming buys execute against asks.
        // Incoming sells execute against bids.
        let mut result = match side {
            Side::Buy => ex.execute(
                &mut self.asks,
                arena,
                order,
                reports.any(),
                &mut on_depleted,
                publish,
            ),
            Side::Sell => ex.execute(
                &mut self.bids,
                arena,
                order,
                reports.any(),
                &mut on_depleted,
                publish,
            ),
        };
        // A remaining incoming order rests on its OWN side:
        //
        // Buy  -> bids
        // Sell -> asks
        //
        // MutatingFills consumes remaining_order and inserts it.
        // SimulatedFills leaves remaining_order untouched.
        if let Some(side) = result.remaining_order.as_ref().map(Trades::side) {
            let user_dict = &mut self.user_to_orders_map;
            let order_dict = &mut self.order_to_arena_map;

            match side {
                Side::Buy => ex.commit_remaining::<_, _, _, U, H>(
                    user_dict,
                    order_dict,
                    &mut self.bids,
                    arena,
                    &mut result.remaining_order,
                    publish,
                ),
                Side::Sell => ex.commit_remaining::<_, _, _, U, H>(
                    user_dict,
                    order_dict,
                    &mut self.asks,
                    arena,
                    &mut result.remaining_order,
                    publish,
                ),
            }
        }

        let report = result.match_result.take().map(|match_result| {
            build_report(
                reports,
                match_result,
                order_id,
                trader_id,
                side,
                market_context,
            )
        });

        FillResult {
            remaining_order_id: result.remaining_order_id,
            remaining_order: result.remaining_order,
            report,
        }
    }

    pub fn user_outstanding_liquidity(&mut self, user_id: Uuid) -> UserOutstandingLiquidity {
        let mut liq = UserOutstandingLiquidity::new(user_id);
        let mut bid_visible_value = 0;
        let mut ask_visible_value = 0;
        let mut bid_hidden_value = 0;
        let mut ask_hidden_value = 0;
        let arena = &self.arena;
        let user_orders = self.user_to_orders_map.entry(user_id).or_default();

        // User indexes use the same tombstone discipline as price-level
        // queues: removals leave keys behind, and reads discard keys only when
        // they encounter a missing arena entry.
        user_orders.retain(|key| arena.get(*key).is_some());

        user_orders
            .iter()
            .filter_map(|key| self.arena.get(*key))
            .for_each(|order| {
                let price = order.price().unwrap();
                let visible_quantity = order.quantity();
                let hidden_quantity = match &order.typed_order_details.replenishment_behavior {
                    ReplenishmentBehavior::Iceberg {
                        hidden_quantity, ..
                    } => *hidden_quantity,
                    ReplenishmentBehavior::Remove => 0,
                };
                let visible_value = price.into_u128() * Notional::from(visible_quantity);
                let hidden_value = price.into_u128() * Notional::from(hidden_quantity);

                match order.side() {
                    Side::Buy => {
                        liq.bid_order_count += 1;
                        liq.bid_visible_quantity += visible_quantity;
                        liq.bid_hidden_quantity += hidden_quantity;
                        bid_visible_value += visible_value;
                        bid_hidden_value += hidden_value;
                    }
                    Side::Sell => {
                        liq.ask_order_count += 1;
                        liq.ask_visible_quantity += visible_quantity;
                        liq.ask_hidden_quantity += hidden_quantity;
                        ask_visible_value += visible_value;
                        ask_hidden_value += hidden_value;
                    }
                }
            });

        let vwap = |value: Notional, quantity: u64| {
            if quantity == 0 {
                0.0
            } else {
                value as f32 / quantity as f32
            }
        };
        let visible_value = bid_visible_value + ask_visible_value;
        let hidden_value = bid_hidden_value + ask_hidden_value;
        liq.bid_visible_value = bid_visible_value;
        liq.bid_visible_vwap = vwap(bid_visible_value, liq.bid_visible_quantity);
        liq.ask_visible_value = ask_visible_value;
        liq.ask_visible_vwap = vwap(ask_visible_value, liq.ask_visible_quantity);
        liq.bid_hidden_value = bid_hidden_value;
        liq.bid_hidden_vwap = vwap(bid_hidden_value, liq.bid_hidden_quantity);
        liq.ask_hidden_value = ask_hidden_value;
        liq.ask_hidden_vwap = vwap(ask_hidden_value, liq.ask_hidden_quantity);
        liq.visible_value = visible_value;
        liq.visible_vwap = vwap(visible_value, liq.visible_quantity());
        liq.hidden_value = hidden_value;
        liq.hidden_vwap = vwap(hidden_value, liq.hidden_quantity());
        liq.value = visible_value + hidden_value;
        liq.vwap = vwap(liq.value, liq.quantity());

        liq
    }

    pub fn remove_user_orders(
        &mut self,
        user_id: Uuid,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> Result<Uuid, UserStateError> {
        let arena_keys: Vec<_> = self
            .user_to_orders_map
            .entry(user_id)
            .or_default()
            .iter()
            .copied()
            .collect();
        let mut encountered_stale_keys = Vec::new();

        for arena_key in arena_keys {
            let Some(order) = self.arena.get(arena_key) else {
                encountered_stale_keys.push(arena_key);
                continue;
            };
            let order_id = order.uuid();

            if self.order_to_arena_map.get(&order_id) != Some(&arena_key) {
                return Err(UserStateError::ConflictingStateError);
            }

            self.remove_order(order_id, publish)
                .map_err(|_| UserStateError::ConflictingStateError)?;
        }

        if let Some(user_orders) = self.user_to_orders_map.get_mut(&user_id) {
            for arena_key in encountered_stale_keys {
                user_orders.remove(&arena_key);
            }
        }

        Ok(user_id)
    }

    #[inline(always)]
    pub fn add_order<T>(
        &mut self,
        order: Order<T, L::Price>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> Result<Uuid, OrderStateError>
    where
        T: IntoRestingOrderData,
    {
        let order = order.into_resting();
        match order.side() {
            Side::Buy => self.add_sided_order::<Buy>(order, publish),
            Side::Sell => self.add_sided_order::<Sell>(order, publish),
        }
    }

    #[inline(always)]
    pub fn add_sided_order<S: OrderSide>(
        &mut self,
        order: RestingOrder<L::Price>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> Result<Uuid, OrderStateError> {
        // TODO
        //perform initial checks depending on idempotency policy
        let entry = match self.order_to_arena_map.entry(order.uuid()) {
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(OrderStateError::OrderAlreadyExists);
            }
            std::collections::hash_map::Entry::Vacant(entry) => entry,
        };
        let arena = &mut self.arena;
        let data =
            S::push_order_to_sided_store(&mut self.bids, &mut self.asks, arena, order, publish);
        entry.insert(data.arena_key);
        U::update_user_map(&mut self.user_to_orders_map, data.user_id, data.arena_key);
        Ok(data.order_id)
    }

    pub fn remove_order(
        &mut self,
        order_id: Uuid,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> Result<Uuid, OrderStateError> {
        self.remove_and_return_order(order_id, publish)
            .map(|_| order_id)
    }

    /// Checked read for consumers working with external or counterfactual IDs.
    pub fn order(&self, order_id: Uuid) -> Option<&RestingOrder<L::Price>> {
        let key = self.order_to_arena_map.get(&order_id)?;
        self.arena.get_node(*key).map(|node| &node.value)
    }

    /// Select the byte layout and precision before populating the book.
    pub fn configure_checksum(
        &mut self,
        specification: std::sync::Arc<policies::checksum::Prepared>,
        precision: policies::checksum::Precision,
    ) -> Result<(), policies::checksum::Error> {
        if !self.order_to_arena_map.is_empty()
            || self.bids.visible_quantity != 0 || self.asks.visible_quantity != 0 {
            return Err("Configure checksum before populating the book");
        }
        L::Checksum::configure(&mut self.checksum, specification, precision)
    }

    /// Finish a complete message; the null policy compiles to a constant result.
    #[inline(always)]
    pub fn checksum(&mut self) -> Result<u32, policies::checksum::Error> {
        L::Checksum::calculate(self)
    }
    /// Use a layout validated during adapter construction, with the same input view.
    pub fn checksum_with(&mut self, prepared: &std::sync::Arc<policies::checksum::Prepared>) -> Result<u32, policies::checksum::Error> {
        L::Checksum::calculate_with(self, prepared)
    }


    pub fn queue_view(
        &self,
        side: Side,
        prices: std::ops::RangeInclusive<L::Price>,
    ) -> Vec<bidask::QueueOrder<L::Price>> {
        match side {
            Side::Buy => self.bids.queue_view(&self.arena, prices),
            Side::Sell => self.asks.queue_view(&self.arena, prices),
        }
    }

    #[inline(always)]
    pub fn remove_unchecked(
        &mut self,
        order_id: Uuid,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> Result<Uuid, OrderStateError> {
        Ok(self
            .remove_and_return_order_unchecked(order_id, publish)
            .uuid())
    }

    pub fn remove_and_return_order(
        &mut self,
        order_id: Uuid,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> Result<RestingOrder<L::Price>, OrderStateError> {
        let arena_key = self
            .order_to_arena_map
            .remove(&order_id)
            .ok_or(OrderStateError::OrderDoesNotExists)?;
        let node = self
            .arena
            .remove_and_return_node(arena_key)
            .ok_or(OrderStateError::OrderDoesNotExists)?;
        let links = node.links();
        let order = node.value;
        U::remove_from_user_map(
            &mut self.user_to_orders_map,
            order.common_data.trader,
            arena_key,
        );

        self.mark_order_removed(
            order.side(),
            links,
            order.price().expect("resting order must have a price"),
            order.quantity(),
            H::order_hidden_quantity(&order.typed_order_details.replenishment_behavior),
            publish,
        );

        // Intentionally leave arena_key in both the price-level queue and the
        // user index. Each collection removes the tombstone when its own
        // traversal encounters the missing arena entry.
        Ok(order)
    }

    #[inline(always)]
    pub fn remove_and_return_order_unchecked(
        &mut self,
        order_id: Uuid,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> RestingOrder<L::Price> {
        // SAFETY: replay only removes order IDs previously inserted into the
        // live order index.
        let arena_key = unsafe { self.order_to_arena_map.remove(&order_id).unwrap_unchecked() };

        // SAFETY: the indexed arena node remains live until it is removed below.
        let links = unsafe { self.arena.get_node_unchecked(arena_key) }.links();
        // SAFETY: the arena key came directly from the live order index.
        let order = unsafe { self.arena.remove_and_return_value_unchecked(arena_key) };
        U::remove_from_user_map(
            &mut self.user_to_orders_map,
            order.common_data.trader,
            arena_key,
        );

        self.mark_order_removed_unchecked(
            order.side(),
            links,
            // SAFETY: every indexed resting order has a price.
            unsafe { order.price().unwrap_unchecked() },
            order.quantity(),
            H::order_hidden_quantity(&order.typed_order_details.replenishment_behavior),
            publish,
        );

        // Intentionally leave arena_key in both the price-level queue and the
        // user index. Each collection removes the tombstone when its own
        // traversal encounters the missing arena entry.
        order
    }

    //cannot modify side or id
    pub fn modify_order_in_place(
        &mut self,
        order_id: Uuid,
        order_details: OrderDetails<L::Price>,
        transition: OrderTransition<L::Price>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> Result<Uuid, OrderStateError> {
        self.modify_with_effect(order_id, order_details, transition, publish, |_, _, _| {})
    }

    #[inline(always)]
    fn modify_with_effect<MP: MutationPublisher<L::Price>>(
        &mut self,
        order_id: Uuid,
        order_details: OrderDetails<L::Price>,
        transition: OrderTransition<L::Price>,
        publish: &mut MP,
        effect: impl FnOnce(&ModificationState<L::Price>, Side, &mut MP),
    ) -> Result<Uuid, OrderStateError> {
        // SAFETY: preserve-priority modifications are submitted only for live,
        // indexed resting orders.
        let arena_key = unsafe { *self.order_to_arena_map.get(&order_id).unwrap_unchecked() };
        let (side, trader_id, modification) =
            self.apply_in_place_transition(arena_key, order_details, transition);
        self.finalize_in_place_modification(
            side,
            trader_id,
            order_id,
            arena_key,
            &modification,
            publish,
        );
        // The effect is an inlined generic callback, erased for ordinary modification.
        effect(&modification, side, publish);
        Ok(order_id)
    }
    /// Apply a replay execution, publishing volume from the actual quantity
    /// reduction. The caller supplies a price selector once (resting or explicit
    /// execution price); cancellations continue to use modify_order_in_place.
    #[inline(always)]
    pub fn modify_with_fill_event(
        &mut self,
        order_id: Uuid,
        order_details: OrderDetails<L::Price>,
        transition: OrderTransition<L::Price>,
        execution_price: impl FnOnce(L::Price) -> L::Price,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> Result<Uuid, OrderStateError> {
        self.modify_with_effect(
            order_id,
            order_details,
            transition,
            publish,
            |modification, side, publish| {
                publish.traded_volume(|| {
                    let quantity = modification.old_quantity - modification.new_quantity;
                    (quantity != 0).then(|| TradedVolumeEvent {
                        price: execution_price(modification.price),
                        quantity,
                        side,
                    })
                });
            },
        )
    }

    //replace_in_place
    //must modify id (hence replace.) comes out of level regardless of wheher price changes, and always loses time priority.
    // cannot modify side. This is considered in place in the sense that
    //1. stays in arena
    //2. stays in store (cannot change sides)
    // but it CAN change levels.
    #[inline(always)]
    pub fn replace_in_place(
        &mut self,
        old_order_id: Uuid,
        order_data: OrderDetails<L::Price>,
        transition: OrderTransition<L::Price>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> Result<Uuid, OrderStateError> {
        let mut order: Order<
            lobo_models::orders::core::RestingOrderData,
            <L as PriceLevelContract>::Price,
        > = self.remove_and_return_order_unchecked(old_order_id, publish);
        transition(&mut order, order_data);
        match order.side() {
            Side::Buy => self.add_sided_order::<Buy>(order, publish),
            Side::Sell => self.add_sided_order::<Sell>(order, publish),
        }
    }

    #[inline(always)]
    fn apply_in_place_transition(
        &mut self,
        arena_key: ArenaKey,
        order_details: OrderDetails<L::Price>,
        transition: OrderTransition<L::Price>,
    ) -> (Side, Uuid, ModificationState<L::Price>) {
        // SAFETY: modify_order_in_place obtained this key from the live order index.
        let order = unsafe { self.arena.get_unchecked_mut(arena_key) };
        // SAFETY: every indexed resting order has a price.
        let price = unsafe { order.price().unwrap_unchecked() };
        let old_quantity = order.quantity();
        let old_hidden_quantity =
            H::order_hidden_quantity(&order.typed_order_details.replenishment_behavior);

        transition(order, order_details);

        (
            order.side(),
            order.common_data.trader,
            ModificationState {
                price,
                old_quantity,
                old_hidden_quantity,
                new_quantity: order.quantity(),
                new_hidden_quantity: H::order_hidden_quantity(
                    &order.typed_order_details.replenishment_behavior,
                ),
            },
        )
    }

    #[inline(always)]
    fn finalize_in_place_modification(
        &mut self,
        side: Side,
        trader_id: Uuid,
        order_id: Uuid,
        arena_key: ArenaKey,
        modification: &ModificationState<L::Price>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) {
        if modification.is_depleted() {
            self.remove_depleted_modified_order(
                side,
                trader_id,
                order_id,
                arena_key,
                modification,
                publish,
            );
        } else {
            self.update_order_quantities(
                side,
                modification.price,
                modification.old_quantity,
                modification.old_hidden_quantity,
                modification.new_quantity,
                modification.new_hidden_quantity,
                publish,
            );
        }
    }

    #[inline(always)]
    fn remove_depleted_modified_order(
        &mut self,
        side: Side,
        trader_id: Uuid,
        order_id: Uuid,
        arena_key: ArenaKey,
        modification: &ModificationState<L::Price>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) {
        self.order_to_arena_map.remove(&order_id);
        U::remove_from_user_map(&mut self.user_to_orders_map, trader_id, arena_key);

        // SAFETY: the live indexed node remains in its price level until its
        // links are captured here.
        let links = unsafe { self.arena.get_node_unchecked(arena_key) }.links();
        // SAFETY: the same live indexed node is removed exactly once.
        let _ = unsafe { self.arena.remove_and_return_value_unchecked(arena_key) };

        self.mark_order_removed_unchecked(
            side,
            links,
            modification.price,
            modification.old_quantity,
            modification.old_hidden_quantity,
            publish,
        );
    }

    #[inline(always)]
    fn update_order_quantities(
        &mut self,
        side: Side,
        price: L::Price,
        old_visible_quantity: u64,
        old_hidden_quantity: u64,
        new_visible_quantity: u64,
        new_hidden_quantity: u64,
        publish: &mut impl MutationPublisher<L::Price>,
    ) {
        let _ = match side {
            Side::Buy => self.bids.update_order_quantities(
                price,
                old_visible_quantity,
                old_hidden_quantity,
                new_visible_quantity,
                new_hidden_quantity,
                publish,
            ),
            Side::Sell => self.asks.update_order_quantities(
                price,
                old_visible_quantity,
                old_hidden_quantity,
                new_visible_quantity,
                new_hidden_quantity,
                publish,
            ),
        };
    }

    fn mark_order_removed(
        &mut self,
        side: Side,
        links: ArenaLinks,
        price: L::Price,
        visible_quantity: u64,
        hidden_quantity: u64,
        publish: &mut impl MutationPublisher<L::Price>,
    ) {
        let removed = match side {
            Side::Buy => self.bids.mark_order_removed(
                &mut self.arena,
                links,
                price,
                visible_quantity,
                hidden_quantity,
                publish,
            ),
            Side::Sell => self.asks.mark_order_removed(
                &mut self.arena,
                links,
                price,
                visible_quantity,
                hidden_quantity,
                publish,
            ),
        };
        assert!(removed, "removed order price level is missing");
    }

    #[inline(always)]
    fn mark_order_removed_unchecked(
        &mut self,
        side: Side,
        links: ArenaLinks,
        price: L::Price,
        visible_quantity: u64,
        hidden_quantity: u64,
        publish: &mut impl MutationPublisher<L::Price>,
    ) {
        match side {
            Side::Buy => self.bids.mark_order_removed_unchecked(
                &mut self.arena,
                links,
                price,
                visible_quantity,
                hidden_quantity,
                publish,
            ),
            Side::Sell => self.asks.mark_order_removed_unchecked(
                &mut self.arena,
                links,
                price,
                visible_quantity,
                hidden_quantity,
                publish,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use lobo_models::{
        Side,
        events::{Fill, FillResult, Reports},
        orders::{
            order_types::{IcebergOrder, LimitOrder},
            traits::Trades,
        },
    };
    use lobo_primitives::{CompressedPrice, uuid::Uuid};

    use crate::bidask::traits::{Buy, Sell};
    use crate::policies::UpdateHiddenQuantity;
    use crate::price_level::DeepPriceLevel;

    use super::*;

    fn p(raw: u32) -> CompressedPrice {
        CompressedPrice::from(raw)
    }

    fn new_storage() -> OrderStorage<DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>> {
        OrderStorage::new()
    }

    const FILL_REPORTS: Reports = Reports {
        include_fills: true,
        include_summary: false,
        include_market_impact: false,
    };

    fn reported_fills(result: &FillResult<CompressedPrice>) -> &[Fill<CompressedPrice>] {
        result
            .report
            .as_ref()
            .and_then(|report| report.fills.as_deref())
            .expect("fill report was requested")
    }

    fn fill_count(result: &FillResult<CompressedPrice>) -> usize {
        reported_fills(result).len()
    }

    fn filled_quantity(result: &FillResult<CompressedPrice>) -> u64 {
        reported_fills(result)
            .iter()
            .map(|fill| fill.fill_quantity)
            .sum()
    }

    fn apply_order_details(
        order: &mut RestingOrder<CompressedPrice>,
        details: OrderDetails<CompressedPrice>,
    ) {
        order.update_in_place(details);
    }

    fn order_details(
        quantity: u64,
        price: Option<CompressedPrice>,
        uuid: Option<Uuid>,
    ) -> OrderDetails<CompressedPrice> {
        OrderDetails {
            uuid,
            price,
            creation_time: None,
            quantity: Some(quantity),
            trader: None,
            side: None,
        }
    }

    #[test]
    fn inserts_many_resting_orders() {
        const ORDER_COUNT: usize = 25_000;
        const USER_COUNT: usize = 100;

        let mut storage = new_storage();

        let users: Vec<Uuid> = (0..USER_COUNT).map(|_| Uuid::new_v4()).collect();

        let started = Instant::now();

        for index in 0..ORDER_COUNT {
            let user_id = users[index % USER_COUNT];

            let order = LimitOrder::new(Some(100), 1, user_id, Side::Buy);

            storage.submit_order(order, MutatingFills, &mut |_| {});
        }

        let elapsed = started.elapsed();

        assert_eq!(storage.bids.len(), ORDER_COUNT);
        assert_eq!(storage.bids.price_level_count(), 1);
        assert!(storage.asks.is_empty());

        assert_eq!(storage.order_to_arena_map.len(), ORDER_COUNT,);

        assert_eq!(storage.user_to_orders_map.len(), USER_COUNT,);

        let indexed_order_count: usize = storage
            .user_to_orders_map
            .values()
            .map(|orders| orders.len())
            .sum();

        assert_eq!(indexed_order_count, ORDER_COUNT);

        for orders in storage.user_to_orders_map.values() {
            assert_eq!(orders.len(), ORDER_COUNT / USER_COUNT);
        }

        println!("Inserted {ORDER_COUNT} resting orders in {elapsed:?}");
    }

    #[test]
    fn matches_many_orders_at_one_price() {
        const ORDER_COUNT: usize = 25_000;

        let mut storage = new_storage();
        let buyer = Uuid::new_v4();
        let seller = Uuid::new_v4();

        for _ in 0..ORDER_COUNT {
            let buy = LimitOrder::new(Some(100), 1, buyer, Side::Buy);

            storage.submit_order(buy, MutatingFills, &mut |_| {});
        }

        assert_eq!(storage.bids.len(), ORDER_COUNT);
        assert_eq!(storage.bids.price_level_count(), 1);

        let started = Instant::now();
        let mut total_fills = 0;
        let mut total_filled_quantity = 0;

        for _ in 0..ORDER_COUNT {
            let sell = LimitOrder::new(Some(100), 1, seller, Side::Sell);

            let result =
                storage.submit_order_with_reports(sell, MutatingFills, FILL_REPORTS, &mut |_| {});

            assert_eq!(fill_count(&result), 1);
            assert_eq!(filled_quantity(&result), 1);

            total_fills += fill_count(&result);
            total_filled_quantity += filled_quantity(&result);
        }

        let elapsed = started.elapsed();

        assert_eq!(total_fills, ORDER_COUNT);
        assert_eq!(total_filled_quantity, ORDER_COUNT as u64,);

        assert!(
            storage.bids.is_empty(),
            "all resting bids should have been consumed"
        );

        assert!(
            storage.asks.is_empty(),
            "fully filled sells must not enter the ask book"
        );

        println!("Matched {ORDER_COUNT} order pairs in {elapsed:?}");
    }

    #[test]
    fn one_large_order_sweeps_many_price_levels() {
        const PRICE_LEVELS: usize = 20;
        const ORDERS_PER_LEVEL: usize = 1_000;

        let mut storage = new_storage();
        let seller = Uuid::new_v4();
        let buyer = Uuid::new_v4();

        for level in 0..PRICE_LEVELS {
            let price = p(100 + u32::try_from(level).unwrap());

            for _ in 0..ORDERS_PER_LEVEL {
                let sell = LimitOrder::new(Some(price), 1, seller, Side::Sell);

                storage.submit_order(sell, MutatingFills, &mut |_| {});
            }
        }

        assert_eq!(storage.asks.len(), PRICE_LEVELS * ORDERS_PER_LEVEL);
        assert_eq!(storage.asks.price_level_count(), PRICE_LEVELS);

        let expected_quantity = (PRICE_LEVELS * ORDERS_PER_LEVEL) as u64;

        let highest_price = p(100 + u32::try_from(PRICE_LEVELS - 1).unwrap());

        let buy = LimitOrder::new(Some(highest_price), expected_quantity, buyer, Side::Buy);

        let started = Instant::now();
        let result =
            storage.submit_order_with_reports(buy, MutatingFills, FILL_REPORTS, &mut |_| {});
        let elapsed = started.elapsed();

        assert_eq!(fill_count(&result), PRICE_LEVELS * ORDERS_PER_LEVEL,);

        assert_eq!(filled_quantity(&result), expected_quantity,);

        assert!(
            storage.asks.is_empty(),
            "the large buy should consume every ask"
        );

        assert!(
            storage.bids.is_empty(),
            "the completely filled buy must not rest"
        );

        println!(
            "Swept {} orders across {} price levels in {:?}",
            fill_count(&result),
            PRICE_LEVELS,
            elapsed,
        );
    }

    #[test]
    fn filled_orders_leave_user_tombstones_until_user_index_traversal() {
        const ORDER_COUNT: usize = 10_000;

        let mut storage = new_storage();
        let buyer = Uuid::new_v4();
        let seller = Uuid::new_v4();

        for _ in 0..ORDER_COUNT {
            storage.submit_order(
                LimitOrder::new(Some(100), 1, buyer, Side::Buy),
                MutatingFills,
                &mut |_| {},
            );
        }

        for _ in 0..ORDER_COUNT {
            storage.submit_order(
                LimitOrder::new(Some(100), 1, seller, Side::Sell),
                MutatingFills,
                &mut |_| {},
            );
        }

        assert!(
            storage.order_to_arena_map.is_empty(),
            "filled orders remain in order_to_arena_map"
        );

        let indexed_order_count: usize = storage
            .user_to_orders_map
            .values()
            .map(|orders| orders.len())
            .sum();

        assert_eq!(
            indexed_order_count, ORDER_COUNT,
            "user tombstones should remain until traversal"
        );

        assert_eq!(storage.user_outstanding_liquidity(buyer).order_count(), 0);
        assert!(
            storage
                .user_to_orders_map
                .get(&buyer)
                .is_some_and(HashSet::is_empty),
            "user traversal should discard missing arena keys"
        );
    }

    #[test]
    fn iceberg_maker_is_removed_only_after_hidden_quantity_is_exhausted() {
        let mut storage = new_storage();
        let seller = Uuid::new_v4();
        let buyer = Uuid::new_v4();
        let iceberg = IcebergOrder::new(Some(100), seller, Side::Sell, 4, 2);
        let maker_order_id = iceberg.uuid();

        let resting = storage.submit_order(iceberg, MutatingFills, &mut |_| {});
        assert_eq!(resting.remaining_order_id, Some(maker_order_id));
        assert_eq!(storage.asks.len(), 1);

        let first_fill = storage.submit_order_with_reports(
            LimitOrder::new(Some(100), 1, buyer, Side::Buy),
            MutatingFills,
            FILL_REPORTS,
            &mut |_| {},
        );
        assert_eq!(filled_quantity(&first_fill), 1);
        assert!(!reported_fills(&first_fill)[0].maker_depleted);
        assert!(storage.order_to_arena_map.contains_key(&maker_order_id));

        for quantity in [1, 2] {
            let replenishing_fill = storage.submit_order_with_reports(
                LimitOrder::new(Some(100), quantity, buyer, Side::Buy),
                MutatingFills,
                FILL_REPORTS,
                &mut |_| {},
            );
            assert_eq!(filled_quantity(&replenishing_fill), quantity);
            assert!(!reported_fills(&replenishing_fill)[0].maker_depleted);
            assert!(storage.order_to_arena_map.contains_key(&maker_order_id));
        }

        let final_fill = storage.submit_order_with_reports(
            LimitOrder::new(Some(100), 2, buyer, Side::Buy),
            MutatingFills,
            FILL_REPORTS,
            &mut |_| {},
        );
        assert_eq!(filled_quantity(&final_fill), 2);
        assert!(reported_fills(&final_fill)[0].maker_depleted);
        assert!(storage.asks.is_empty());
        assert!(!storage.order_to_arena_map.contains_key(&maker_order_id));
        assert_eq!(
            storage.user_to_orders_map.get(&seller).map(HashSet::len),
            Some(1),
            "depletion should leave a user-index tombstone"
        );
        assert_eq!(storage.user_outstanding_liquidity(seller).order_count(), 0);
        assert!(
            storage
                .user_to_orders_map
                .get(&seller)
                .is_some_and(HashSet::is_empty)
        );
    }

    #[test]
    fn user_outstanding_liquidity_reports_mixed_limit_and_iceberg_orders() {
        let mut storage = new_storage();
        let trader = Uuid::new_v4();
        let other_trader = Uuid::new_v4();

        storage.submit_order(
            LimitOrder::new(Some(99), 3, trader, Side::Buy),
            MutatingFills,
            &mut |_| {},
        );
        storage.submit_order(
            IcebergOrder::new(Some(105), trader, Side::Sell, 8, 2),
            MutatingFills,
            &mut |_| {},
        );
        storage.submit_order(
            LimitOrder::new(Some(98), 50, other_trader, Side::Buy),
            MutatingFills,
            &mut |_| {},
        );

        let liquidity = storage.user_outstanding_liquidity(trader);

        assert_eq!(liquidity.user_id, trader);
        assert_eq!(liquidity.order_count(), 2);
        assert_eq!(liquidity.bid_order_count, 1);
        assert_eq!(liquidity.ask_order_count, 1);
        assert_eq!(liquidity.bid_visible_quantity, 3);
        assert_eq!(liquidity.ask_visible_quantity, 2);
        assert_eq!(liquidity.visible_quantity(), 5);
        assert_eq!(liquidity.bid_hidden_quantity, 0);
        assert_eq!(liquidity.ask_hidden_quantity, 8);
        assert_eq!(liquidity.hidden_quantity(), 8);
        assert_eq!(liquidity.quantity(), 13);
        assert_eq!(liquidity.bid_visible_value, 297);
        assert_eq!(liquidity.bid_visible_vwap, 99.0);
        assert_eq!(liquidity.ask_visible_value, 210);
        assert_eq!(liquidity.ask_visible_vwap, 105.0);
        assert_eq!(liquidity.ask_hidden_value, 840);
        assert_eq!(liquidity.ask_hidden_vwap, 105.0);
        assert_eq!(liquidity.visible_value(), 507);
        assert_eq!(liquidity.visible_vwap(), 101.4);
        assert_eq!(liquidity.hidden_value(), 840);
        assert_eq!(liquidity.hidden_vwap(), 105.0);
        assert_eq!(liquidity.value(), 1_347);
        assert_eq!(liquidity.vwap(), 1_347.0 / 13.0);

        let missing = storage.user_outstanding_liquidity(Uuid::new_v4());
        assert_eq!(missing.order_count(), 0);
        assert_eq!(missing.bid_order_count, 0);
        assert_eq!(missing.ask_order_count, 0);
        assert_eq!(missing.visible_quantity(), 0);
        assert_eq!(missing.hidden_quantity(), 0);
        assert_eq!(missing.quantity(), 0);
    }

    #[test]
    fn user_outstanding_liquidity_tracks_partial_fills_and_cancellation() {
        let mut storage = new_storage();
        let seller = Uuid::new_v4();
        let buyer = Uuid::new_v4();
        let ask = IcebergOrder::new(Some(100), seller, Side::Sell, 4, 2);
        let ask_id = ask.uuid();
        storage.submit_order(ask, MutatingFills, &mut |_| {});

        storage.submit_order(
            LimitOrder::new(Some(100), 3, buyer, Side::Buy),
            MutatingFills,
            &mut |_| {},
        );
        let after_fill = storage.user_outstanding_liquidity(seller);
        assert_eq!(after_fill.order_count(), 1);
        assert_eq!(after_fill.bid_order_count, 0);
        assert_eq!(after_fill.ask_order_count, 1);
        // assert_eq!(after_fill.ask_visible_quantity, 2);
        // assert_eq!(after_fill.visible_quantity(), 2);
        assert_eq!(after_fill.ask_hidden_quantity, 2);
        assert_eq!(after_fill.hidden_quantity(), 2);
        // assert_eq!(after_fill.quantity(), 4);
        // assert_eq!(after_fill.ask_visible_value, 200);
        assert_eq!(after_fill.ask_visible_vwap, 100.0);
        assert_eq!(after_fill.ask_hidden_value, 200);
        assert_eq!(after_fill.ask_hidden_vwap, 100.0);
        // assert_eq!(after_fill.value(), 400);

        assert!(storage.remove_order(ask_id, &mut |_| {}).is_ok());
        let after_cancel = storage.user_outstanding_liquidity(seller);
        assert_eq!(after_cancel.order_count(), 0);
        assert_eq!(after_cancel.ask_visible_quantity, 0);
        assert_eq!(after_cancel.ask_hidden_quantity, 0);
        assert_eq!(after_cancel.quantity(), 0);
    }

    #[test]
    fn user_outstanding_liquidity_scales_to_thousands_of_orders() {
        const ORDER_COUNT: usize = 5_000;
        let mut storage = new_storage();
        let trader = Uuid::new_v4();

        for index in 0..ORDER_COUNT {
            storage.submit_order(
                LimitOrder::new(
                    Some(p(100 + u32::try_from(index % 25).unwrap())),
                    2,
                    trader,
                    Side::Buy,
                ),
                MutatingFills,
                &mut |_| {},
            );
        }

        let liquidity = storage.user_outstanding_liquidity(trader);
        assert_eq!(liquidity.order_count(), ORDER_COUNT);
        assert_eq!(liquidity.bid_order_count, ORDER_COUNT);
        assert_eq!(liquidity.ask_order_count, 0);
        assert_eq!(liquidity.bid_visible_quantity, (ORDER_COUNT * 2) as u64);
        assert_eq!(liquidity.visible_quantity(), (ORDER_COUNT * 2) as u64);
        assert_eq!(liquidity.hidden_quantity(), 0);
        assert_eq!(liquidity.quantity(), (ORDER_COUNT * 2) as u64);
        assert_eq!(liquidity.bid_visible_value, 1_120_000);
        assert_eq!(liquidity.bid_visible_vwap, 112.0);
    }

    #[test]
    fn benchmark_orderbook_rs_workloads() {
        /*
         * Benchmark 1:
         * Insert 25,000 resting buy orders at one price.
         */
        {
            const ORDER_COUNT: usize = 25_000;
            const USER_COUNT: usize = 100;

            let mut storage = new_storage();

            let users: Vec<Uuid> = (0..USER_COUNT).map(|_| Uuid::new_v4()).collect();

            let started = Instant::now();

            for index in 0..ORDER_COUNT {
                let user_id = users[index % USER_COUNT];

                let order = LimitOrder::new(Some(100), 1, user_id, Side::Buy);

                storage.submit_order(order, MutatingFills, &mut |_| {});
            }

            let elapsed = started.elapsed();

            assert_eq!(storage.bids.len(), ORDER_COUNT);
            assert_eq!(storage.bids.price_level_count(), 1);
            assert!(storage.asks.is_empty());

            assert_eq!(storage.order_to_arena_map.len(), ORDER_COUNT,);

            assert_eq!(storage.user_to_orders_map.len(), USER_COUNT,);

            let indexed_order_count: usize = storage
                .user_to_orders_map
                .values()
                .map(|orders| orders.len())
                .sum();

            assert_eq!(indexed_order_count, ORDER_COUNT);

            for orders in storage.user_to_orders_map.values() {
                assert_eq!(orders.len(), ORDER_COUNT / USER_COUNT,);
            }

            println!("Inserted {ORDER_COUNT} resting orders in {elapsed:?}");
        }

        /*
         * Benchmark 2:
         * Match 25,000 one-unit sells against 25,000 resting buys.
         */
        {
            const ORDER_COUNT: usize = 25_000;

            let mut storage = new_storage();
            let buyer = Uuid::new_v4();
            let seller = Uuid::new_v4();

            for _ in 0..ORDER_COUNT {
                let buy = LimitOrder::new(Some(100), 1, buyer, Side::Buy);

                storage.submit_order(buy, MutatingFills, &mut |_| {});
            }

            assert_eq!(storage.bids.len(), ORDER_COUNT);
            assert_eq!(storage.bids.price_level_count(), 1);

            let started = Instant::now();

            let mut total_fills = 0;
            let mut total_filled_quantity = 0;

            for _ in 0..ORDER_COUNT {
                let sell = LimitOrder::new(Some(100), 1, seller, Side::Sell);

                let result = storage.submit_order_with_reports(
                    sell,
                    MutatingFills,
                    FILL_REPORTS,
                    &mut |_| {},
                );

                assert_eq!(fill_count(&result), 1);
                assert_eq!(filled_quantity(&result), 1);

                total_fills += fill_count(&result);
                total_filled_quantity += filled_quantity(&result);
            }

            let elapsed = started.elapsed();

            assert_eq!(total_fills, ORDER_COUNT);

            assert_eq!(total_filled_quantity, ORDER_COUNT as u64,);

            assert!(
                storage.bids.is_empty(),
                "all resting bids should have been consumed"
            );

            assert!(
                storage.asks.is_empty(),
                "fully filled sells must not enter the ask book"
            );

            println!("Matched {ORDER_COUNT} order pairs in {elapsed:?}");
        }

        /*
         * Benchmark 3:
         * Sweep 20,000 resting sells across 20 price levels.
         */
        {
            const PRICE_LEVELS: usize = 20;
            const ORDERS_PER_LEVEL: usize = 1_000;

            let mut storage = new_storage();
            let seller = Uuid::new_v4();
            let buyer = Uuid::new_v4();

            for level in 0..PRICE_LEVELS {
                let price = p(100 + u32::try_from(level).unwrap());

                for _ in 0..ORDERS_PER_LEVEL {
                    let sell = LimitOrder::new(Some(price), 1, seller, Side::Sell);

                    storage.submit_order(sell, MutatingFills, &mut |_| {});
                }
            }

            assert_eq!(storage.asks.len(), PRICE_LEVELS * ORDERS_PER_LEVEL);
            assert_eq!(storage.asks.price_level_count(), PRICE_LEVELS);

            let expected_quantity = (PRICE_LEVELS * ORDERS_PER_LEVEL) as u64;

            let highest_price = p(100 + u32::try_from(PRICE_LEVELS - 1).unwrap());

            let buy = LimitOrder::new(Some(highest_price), expected_quantity, buyer, Side::Buy);

            let started = Instant::now();

            let result =
                storage.submit_order_with_reports(buy, MutatingFills, FILL_REPORTS, &mut |_| {});

            let elapsed = started.elapsed();

            assert_eq!(fill_count(&result), PRICE_LEVELS * ORDERS_PER_LEVEL,);

            assert_eq!(filled_quantity(&result), expected_quantity,);

            assert!(
                storage.asks.is_empty(),
                "the large buy should consume every ask"
            );

            assert!(
                storage.bids.is_empty(),
                "the completely filled buy must not rest"
            );

            println!(
                "Swept {} orders across {} price levels in {:?}",
                fill_count(&result),
                PRICE_LEVELS,
                elapsed,
            );
        }
    }

    #[test]
    fn nil_user_orders_update_the_user_index() {
        let mut storage = new_storage();
        let order = LimitOrder::new(Some(100), 3, Uuid::nil(), Side::Buy);
        let order_id = order.uuid();

        assert_eq!(
            storage
                .add_sided_order::<Buy>(order.into_resting(), &mut |_| {})
                .ok(),
            Some(order_id)
        );
        assert_eq!(
            storage
                .user_to_orders_map
                .get(&Uuid::nil())
                .map(HashSet::len),
            Some(1)
        );
        assert_eq!(
            storage.remove_order(order_id, &mut |_| {}).ok(),
            Some(order_id)
        );
        assert_eq!(
            storage
                .user_to_orders_map
                .get(&Uuid::nil())
                .map(HashSet::len),
            Some(0)
        );
        assert!(storage.order_to_arena_map.is_empty());
        assert!(storage.bids.is_empty());
    }

    #[test]
    fn direct_add_remove_and_error_paths_keep_indexes_consistent() {
        let mut storage = new_storage();
        let user = Uuid::new_v4();
        let order = LimitOrder::new(Some(100), 3, user, Side::Buy);
        let order_id = order.uuid();

        assert_eq!(
            storage
                .add_sided_order::<Buy>(order.into_resting(), &mut |_| {})
                .ok(),
            Some(order_id)
        );
        assert!(storage.order_to_arena_map.contains_key(&order_id));

        let duplicate = LimitOrder::new(Some(100), 1, user, Side::Buy).with_uuid(order_id);
        assert!(matches!(
            storage.add_sided_order::<Buy>(duplicate.into_resting(), &mut |_| {}),
            Err(OrderStateError::OrderAlreadyExists)
        ));

        assert_eq!(
            storage.remove_order(order_id, &mut |_| {}).ok(),
            Some(order_id)
        );
        assert!(matches!(
            storage.remove_order(order_id, &mut |_| {}),
            Err(OrderStateError::OrderDoesNotExists)
        ));
        assert_eq!(
            storage.user_to_orders_map.get(&user).map(HashSet::len),
            Some(0)
        );
        assert_eq!(storage.user_outstanding_liquidity(user).order_count(), 0);
        assert!(storage.user_to_orders_map.get(&user).unwrap().is_empty());
    }

    #[test]
    fn removal_updates_live_aggregates_and_queue_traversal_skips_tombstone() {
        let mut storage = new_storage();
        let seller = Uuid::new_v4();
        let buyer = Uuid::new_v4();
        let first = LimitOrder::new(Some(100), 3, seller, Side::Sell);
        let first_id = first.uuid();
        let second = LimitOrder::new(Some(100), 4, seller, Side::Sell);
        let second_id = second.uuid();
        storage
            .add_sided_order::<Sell>(first.into_resting(), &mut |_| {})
            .unwrap();
        storage
            .add_sided_order::<Sell>(second.into_resting(), &mut |_| {})
            .unwrap();

        storage.remove_order(first_id, &mut |_| {}).unwrap();

        assert_eq!(storage.asks.len(), 1);
        assert_eq!(storage.asks.visible_quantity, 4);
        assert_eq!(storage.asks.get(100).unwrap().len(), 1);
        assert_eq!(
            storage.user_to_orders_map.get(&seller).map(HashSet::len),
            Some(1)
        );

        let fill = storage.submit_order_with_reports(
            LimitOrder::new(Some(100), 1, buyer, Side::Buy),
            MutatingFills,
            FILL_REPORTS,
            &mut |_| {},
        );
        assert_eq!(reported_fills(&fill)[0].maker_order_id, second_id);
        assert_eq!(storage.asks.len(), 1);
        assert_eq!(storage.asks.visible_quantity, 3);
    }

    #[test]
    fn in_place_quantity_modification_preserves_fifo_priority_and_totals() {
        let mut storage = new_storage();
        let seller = Uuid::new_v4();
        let buyer = Uuid::new_v4();
        let first = LimitOrder::new(Some(100), 5, seller, Side::Sell);
        let first_id = first.uuid();
        let second = LimitOrder::new(Some(100), 5, seller, Side::Sell);
        storage
            .add_sided_order::<Sell>(first.into_resting(), &mut |_| {})
            .unwrap();
        storage
            .add_sided_order::<Sell>(second.into_resting(), &mut |_| {})
            .unwrap();

        storage
            .modify_order_in_place(
                first_id,
                order_details(3, None, None),
                apply_order_details,
                &mut |_| {},
            )
            .expect("in-place modification should succeed");

        assert_eq!(storage.asks.len(), 2);
        assert_eq!(storage.asks.visible_quantity, 8);
        assert_eq!(storage.asks.get(100).unwrap().visible_quantity(), 8);

        let fill = storage.submit_order_with_reports(
            LimitOrder::new(Some(100), 1, buyer, Side::Buy),
            MutatingFills,
            FILL_REPORTS,
            &mut |_| {},
        );
        assert_eq!(reported_fills(&fill)[0].maker_order_id, first_id);
    }

    #[test]
    fn replacement_loses_fifo_priority_even_when_price_is_unchanged() {
        let mut storage = new_storage();
        let seller = Uuid::new_v4();
        let buyer = Uuid::new_v4();
        let first = LimitOrder::new(Some(100), 5, seller, Side::Sell);
        let first_id = first.uuid();
        let second = LimitOrder::new(Some(100), 5, seller, Side::Sell);
        let second_id = second.uuid();
        let replacement_id = Uuid::new_v4();
        storage
            .add_sided_order::<Sell>(first.into_resting(), &mut |_| {})
            .unwrap();
        storage
            .add_sided_order::<Sell>(second.into_resting(), &mut |_| {})
            .unwrap();

        storage
            .replace_in_place(
                first_id,
                order_details(4, Some(p(100)), Some(replacement_id)),
                apply_order_details,
                &mut |_| {},
            )
            .expect("same-price replacement should succeed");
        assert!(!storage.order_to_arena_map.contains_key(&first_id));
        assert!(storage.order_to_arena_map.contains_key(&replacement_id));
        assert_eq!(storage.asks.visible_quantity, 9);

        let fill = storage.submit_order_with_reports(
            LimitOrder::new(Some(100), 1, buyer, Side::Buy),
            MutatingFills,
            FILL_REPORTS,
            &mut |_| {},
        );
        assert_eq!(reported_fills(&fill)[0].maker_order_id, second_id);
    }

    #[test]
    fn replacement_price_change_moves_order_and_updates_level_totals() {
        let mut storage = new_storage();
        let trader = Uuid::new_v4();
        let order = LimitOrder::new(Some(100), 5, trader, Side::Buy);
        let order_id = order.uuid();
        let replacement_id = Uuid::new_v4();
        storage
            .add_sided_order::<Buy>(order.into_resting(), &mut |_| {})
            .unwrap();

        storage
            .replace_in_place(
                order_id,
                order_details(3, Some(p(102)), Some(replacement_id)),
                apply_order_details,
                &mut |_| {},
            )
            .expect("price-changing replacement should succeed");

        assert_eq!(storage.bids.len(), 1);
        assert_eq!(storage.bids.visible_quantity, 3);
        assert_eq!(storage.bids.price_level_count(), 1);
        assert!(storage.bids.get(100).unwrap().is_empty());
        assert_eq!(storage.bids.get(102).unwrap().visible_quantity(), 3);
        assert_eq!(*storage.bids.best().unwrap().0, 102);
    }

    #[test]
    fn replacement_side_change_moves_the_prepared_order_between_stores() {
        let mut storage = new_storage();
        let trader = Uuid::new_v4();
        let first = LimitOrder::new(Some(100), 5, trader, Side::Buy);
        let first_id = first.uuid();
        let second = LimitOrder::new(Some(100), 2, trader, Side::Buy);
        let second_id = second.uuid();
        storage
            .add_sided_order::<Buy>(first.into_resting(), &mut |_| {})
            .unwrap();
        storage
            .add_sided_order::<Buy>(second.into_resting(), &mut |_| {})
            .unwrap();

        storage
            .replace_in_place(
                first_id,
                OrderDetails {
                    uuid: None,
                    price: Some(p(101)),
                    creation_time: None,
                    quantity: Some(3),
                    trader: None,
                    side: Some(Side::Sell),
                },
                apply_order_details,
                &mut |_| {},
            )
            .expect("side-changing replacement should succeed");

        assert_eq!(storage.order_to_arena_map.len(), 2);
        assert!(storage.order_to_arena_map.contains_key(&first_id));
        assert_eq!(storage.bids.len(), 1);
        assert_eq!(storage.bids.visible_quantity, 2);
        assert_eq!(storage.bids.get(100).unwrap().len(), 1);
        assert_eq!(storage.asks.len(), 1);
        assert_eq!(storage.asks.visible_quantity, 3);
        assert_eq!(storage.asks.get(101).unwrap().len(), 1);

        let fill = storage.submit_order_with_reports(
            LimitOrder::new(Some(100), 1, Uuid::new_v4(), Side::Sell),
            MutatingFills,
            FILL_REPORTS,
            &mut |_| {},
        );
        assert_eq!(reported_fills(&fill)[0].maker_order_id, second_id);

        let fill = storage.submit_order_with_reports(
            LimitOrder::new(Some(101), 1, Uuid::new_v4(), Side::Buy),
            MutatingFills,
            FILL_REPORTS,
            &mut |_| {},
        );
        assert_eq!(reported_fills(&fill)[0].maker_order_id, first_id);
    }

    #[test]
    fn removing_all_user_orders_prunes_both_sides() {
        let mut storage = new_storage();
        let user = Uuid::new_v4();
        storage
            .add_sided_order::<Buy>(
                LimitOrder::new(Some(99), 2, user, Side::Buy).into_resting(),
                &mut |_| {},
            )
            .ok()
            .unwrap();
        storage
            .add_sided_order::<Sell>(
                LimitOrder::new(Some(101), 3, user, Side::Sell).into_resting(),
                &mut |_| {},
            )
            .ok()
            .unwrap();

        assert_eq!(
            storage.remove_user_orders(user, &mut |_| {}).ok(),
            Some(user)
        );
        assert!(storage.bids.is_empty());
        assert!(storage.asks.is_empty());
        assert!(storage.order_to_arena_map.is_empty());
    }

    #[test]
    fn removing_user_orders_reports_conflicting_internal_indexes() {
        let mut storage = new_storage();
        let user = Uuid::new_v4();
        let id = storage
            .add_sided_order::<Buy>(
                LimitOrder::new(Some(99), 2, user, Side::Buy).into_resting(),
                &mut |_| {},
            )
            .ok()
            .unwrap();
        storage.order_to_arena_map.remove(&id);

        assert!(matches!(
            storage.remove_user_orders(user, &mut |_| {}),
            Err(UserStateError::ConflictingStateError)
        ));
    }

    #[test]
    fn state_error_variants_are_constructible() {
        assert!(matches!(
            UserStateError::UserAlreadyExists,
            UserStateError::UserAlreadyExists
        ));
        assert!(matches!(
            UserStateError::UserDoesNotExists,
            UserStateError::UserDoesNotExists
        ));
    }

    #[cfg(feature = "python")]
    #[test]
    fn python_liquidity_getters_and_display_fields_match_rust_accessors() {
        let mut storage = new_storage();
        let user = Uuid::new_v4();
        storage.submit_order(
            IcebergOrder::new(Some(100), user, Side::Buy, 4, 2),
            MutatingFills,
            &mut |_| {},
        );
        let liquidity = storage.user_outstanding_liquidity(user);

        assert_eq!(liquidity.py_order_count(), liquidity.order_count());
        assert_eq!(
            liquidity.py_visible_quantity(),
            liquidity.visible_quantity()
        );
        assert_eq!(liquidity.py_hidden_quantity(), liquidity.hidden_quantity());
        assert_eq!(liquidity.py_quantity(), liquidity.quantity());
        assert_eq!(liquidity.py_visible_value(), liquidity.visible_value());
        assert_eq!(liquidity.py_visible_vwap(), liquidity.visible_vwap());
        assert_eq!(liquidity.py_hidden_value(), liquidity.hidden_value());
        assert_eq!(liquidity.py_hidden_vwap(), liquidity.hidden_vwap());
        assert_eq!(liquidity.py_value(), liquidity.value());
        assert_eq!(liquidity.py_vwap(), liquidity.vwap());
        assert_eq!(
            UserOutstandingLiquidity::__display_fields__(),
            UserOutstandingLiquidity::display_fields()
        );
    }
}
