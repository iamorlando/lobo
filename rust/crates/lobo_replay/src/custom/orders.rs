//! Typed feed mutations. Matching and publication use the storage API.
use super::{AdaptForReplay, AdaptToFeed, FeedMessage};
use crate::{feed::FeedBook, simulation::Simulation};
use lobo_books::price_time_priority::Book;
use lobo_models::{
    Side,
    events::OrderDetails,
    orders::{
        core::{OrderCore, RestingOrder},
        traits::Trades,
    },
};
use lobo_primitives::{Price64, PriceType, time::DateTime, uuid::Uuid};
use lobo_storage::{
    OrderStateError, UserMapUpdatePolicy, policies::HiddenQuantityPolicy,
    price_level::PriceLevelContract, price_sorting::PriceSortingPolicy,
};

#[derive(Clone, Copy, Debug)]
pub struct AddOrder<P> {
    pub timestamp: u64,
    pub id: Uuid,
    pub side: Side,
    pub quantity: u64,
    pub price: P,
}
#[derive(Clone, Copy, Debug)]
pub struct ExecuteOrder<P> {
    pub timestamp: u64,
    pub id: Uuid,
    pub quantity: u64,
    pub price: Option<P>,
}
#[derive(Clone, Copy, Debug)]
pub struct CancelOrder {
    pub timestamp: u64,
    pub id: Uuid,
    pub quantity: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct RemoveOrder {
    pub timestamp: u64,
    pub id: Uuid,
}
#[derive(Clone, Copy, Debug)]
pub struct ReplaceOrder<P> {
    pub timestamp: u64,
    pub id: Uuid,
    pub new_id: Uuid,
    pub quantity: u64,
    pub price: P,
}

fn details<P: PriceType>(quantity: u64) -> OrderDetails<P> {
    OrderDetails {
        quantity: Some(quantity),
        price: None,
        uuid: None,
        side: None,
        trader: None,
        creation_time: None,
    }
}
// The transition ABI uses OrderDetails. Both callers below construct
// quantity=Some before entering storage; no optional quantity is accepted here.
fn reduce<P: PriceType>(order: &mut RestingOrder<P>, details: OrderDetails<P>) {
    // SAFETY: this private transition is used only with details(quantity).
    let quantity = unsafe { details.quantity.unwrap_unchecked() };
    order.common_data.quantity = order.common_data.quantity.wrapping_sub(quantity);
}
impl<P, L, S, U, H, Pub> AdaptForReplay<L, S, U, H, Pub> for AddOrder<P>
where
    P: PriceType,
    L: PriceLevelContract<Price = P>,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<P>,
{
    #[inline(always)]
    fn process(self, book: &mut Book<L, S, U, H, Pub>) -> Result<(), OrderStateError> {
        let order = RestingOrder::new_replay_order(OrderCore {
            creation_time: DateTime::from_timestamp_nanos(self.timestamp as i64),
            uuid: self.id,
            side: self.side,
            price: Some(self.price),
            quantity: self.quantity,
            trader: Uuid::nil(),
        });
        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        storage.add_order(order, &mut publish).map(|_| ())
    }
}
impl<P, L, S, U, H, Pub> AdaptForReplay<L, S, U, H, Pub> for ExecuteOrder<P>
where
    P: PriceType,
    L: PriceLevelContract<Price = P>,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<P>,
{
    #[inline(always)]
    fn process(self, book: &mut Book<L, S, U, H, Pub>) -> Result<(), OrderStateError> {
        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        storage
            .modify_with_fill_event(
                self.id,
                details(self.quantity),
                reduce,
                |resting| self.price.unwrap_or(resting),
                &mut publish,
            )
            .map(|_| ())
    }
}
impl<L, S, U, H, Pub> AdaptForReplay<L, S, U, H, Pub> for CancelOrder
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    #[inline(always)]
    fn process(self, book: &mut Book<L, S, U, H, Pub>) -> Result<(), OrderStateError> {
        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        storage
            .modify_order_in_place(self.id, details(self.quantity), reduce, &mut publish)
            .map(|_| ())
    }
}
impl<L, S, U, H, Pub> AdaptForReplay<L, S, U, H, Pub> for RemoveOrder
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    #[inline(always)]
    fn process(self, book: &mut Book<L, S, U, H, Pub>) -> Result<(), OrderStateError> {
        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        storage.remove_unchecked(self.id, &mut publish).map(|_| ())
    }
}
impl<P, L, S, U, H, Pub> AdaptForReplay<L, S, U, H, Pub> for ReplaceOrder<P>
where
    P: PriceType,
    L: PriceLevelContract<Price = P>,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<P>,
{
    #[inline(always)]
    fn process(self, book: &mut Book<L, S, U, H, Pub>) -> Result<(), OrderStateError> {
        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        storage
            .replace_in_place(
                self.id,
                OrderDetails {
                    uuid: Some(self.new_id),
                    price: Some(self.price),
                    creation_time: Some(DateTime::from_timestamp_nanos(self.timestamp as i64)),
                    ..details(self.quantity)
                },
                |order, details| order.update_in_place(details),
                &mut publish,
            )
            .map(|_| ())
    }
}
impl FeedMessage for AddOrder<Price64> {
    fn simulate(&self, branch: &mut Simulation, _: &FeedBook) -> Result<(), OrderStateError> {
        self.process_feed(branch.book_mut())
    }
}
impl FeedMessage for ExecuteOrder<Price64> {
    fn simulate(&self, branch: &mut Simulation, source: &FeedBook) -> Result<(), OrderStateError> {
        let order = source
            .order(self.id)
            .ok_or(OrderStateError::OrderDoesNotExists)?;
        let price = self
            .price
            .or(order.price())
            .ok_or(OrderStateError::OrderDoesNotExists)?;
        branch.execute(self.timestamp, order.side(), price, self.quantity);
        Ok(())
    }
}
fn remaining(branch: &mut Simulation, id: Uuid) -> Option<u64> {
    let quantity = branch.book_mut().order(id).map(Trades::quantity);
    if quantity.is_none() {
        branch.ignored += 1;
    }
    quantity
}
impl FeedMessage for CancelOrder {
    fn simulate(&self, branch: &mut Simulation, _: &FeedBook) -> Result<(), OrderStateError> {
        if let Some(quantity) = remaining(branch, self.id) {
            Self {
                quantity: self.quantity.min(quantity),
                ..*self
            }
            .process_feed(branch.book_mut())?;
        }
        Ok(())
    }
}
impl FeedMessage for RemoveOrder {
    fn simulate(&self, branch: &mut Simulation, _: &FeedBook) -> Result<(), OrderStateError> {
        if remaining(branch, self.id).is_some() {
            self.process_feed(branch.book_mut())?;
        }
        Ok(())
    }
}
impl FeedMessage for ReplaceOrder<Price64> {
    fn simulate(&self, branch: &mut Simulation, _: &FeedBook) -> Result<(), OrderStateError> {
        if remaining(branch, self.id).is_some() {
            self.process_feed(branch.book_mut())?;
        }
        Ok(())
    }
}
