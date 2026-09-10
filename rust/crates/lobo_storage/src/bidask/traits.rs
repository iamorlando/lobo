use lobo_events::MutationPublisher;
use lobo_models::{Side, orders::core::RestingOrder};
use lobo_primitives::PriceType;

use crate::{
    arena::arenav1::Arena,
    bidask::{Ask, Bid, SidedOrderStore, SubmittedData},
    policies::HiddenQuantityPolicy,
    price_level::PriceLevelContract,
    price_sorting::PriceSortingPolicy,
};

pub trait SidedPrice<Pr: PriceType> {
    const SIDE: Side;
    type PriceKey: Ord + Clone;
    fn into_key(price: Pr) -> Self::PriceKey;
    fn from_key(key: Self::PriceKey) -> Pr;
    fn price_from_key(key: &Self::PriceKey) -> &Pr;
}

pub struct Buy;
pub struct Sell;
pub trait OrderSide {
    const SIDE: Side;
    fn push_order_to_sided_store<L, S, H>(
        bids: &mut SidedOrderStore<Bid, L, S, H>,
        asks: &mut SidedOrderStore<Ask, L, S, H>,
        arena: &mut Arena<RestingOrder<L::Price>>,
        order: RestingOrder<L::Price>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> SubmittedData
    where
        L: PriceLevelContract,
        S: PriceSortingPolicy,
        H: HiddenQuantityPolicy;
}
impl OrderSide for Sell {
    const SIDE: Side = Side::Sell;
    fn push_order_to_sided_store<L, S, H>(
        _: &mut SidedOrderStore<Bid, L, S, H>,
        asks: &mut SidedOrderStore<Ask, L, S, H>,
        arena: &mut Arena<RestingOrder<L::Price>>,
        order: RestingOrder<L::Price>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> SubmittedData
    where
        L: PriceLevelContract,
        S: PriceSortingPolicy,
        H: HiddenQuantityPolicy,
    {
        asks.push_order(arena, order, publish)
    }
}
impl OrderSide for Buy {
    const SIDE: Side = Side::Buy;
    fn push_order_to_sided_store<L, S, H>(
        bids: &mut SidedOrderStore<Bid, L, S, H>,
        _: &mut SidedOrderStore<Ask, L, S, H>,
        arena: &mut Arena<RestingOrder<L::Price>>,
        order: RestingOrder<L::Price>,
        publish: &mut impl MutationPublisher<L::Price>,
    ) -> SubmittedData
    where
        L: PriceLevelContract,
        S: PriceSortingPolicy,
        H: HiddenQuantityPolicy,
    {
        bids.push_order(arena, order, publish)
    }
}
