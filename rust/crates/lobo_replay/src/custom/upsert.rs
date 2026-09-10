//! Absolute visible order updates over storage.
use super::AdaptForReplay;
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
pub struct OrderUpdate {
    pub timestamp: u64,
    pub id: Uuid,
    /// None removes the order; Some upserts absolute visible liquidity.
    pub price: Option<Price64>,
    pub quantity: u64,
    pub side: Side,
}
fn transition<P: PriceType>(order: &mut RestingOrder<P>, details: OrderDetails<P>) {
    order.update_in_place(details);
}
impl<L, S, U, H, Pub> AdaptForReplay<L, S, U, H, Pub> for OrderUpdate
where
    L: PriceLevelContract<Price = Price64>,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    fn process(self, book: &mut Book<L, S, U, H, Pub>) -> Result<(), OrderStateError> {
        let id = self.id;
        // An absolute order feed can evict orders outside its subscribed window.
        let previous = book
            .order_storage
            .order(id)
            .map(|o| (o.price(), o.side(), o.quantity()));
        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        let Some(price) = self.price else {
            if previous.is_some() {
                storage.remove_unchecked(id, &mut publish)?;
            }
            return Ok(());
        };
        let details = OrderDetails {
            quantity: Some(self.quantity),
            price: Some(price),
            side: Some(self.side),
            uuid: None,
            trader: None,
            creation_time: None,
        };
        match previous {
            Some((Some(old_price), side, quantity)) if old_price == price && side == self.side => {
                if quantity != self.quantity {
                    storage.modify_order_in_place(id, details, transition, &mut publish)?;
                }
            }
            Some(_) => {
                storage.replace_in_place(
                    id,
                    OrderDetails {
                        creation_time: Some(DateTime::from_timestamp_nanos(self.timestamp as i64)),
                        ..details
                    },
                    transition,
                    &mut publish,
                )?;
            }
            None => {
                storage.add_order(
                    RestingOrder::new_replay_order(OrderCore {
                        creation_time: DateTime::from_timestamp_nanos(self.timestamp as i64),
                        uuid: id,
                        price: Some(price),
                        quantity: self.quantity,
                        trader: Uuid::nil(),
                        side: self.side,
                    }),
                    &mut publish,
                )?;
            }
        }
        Ok(())
    }
}
