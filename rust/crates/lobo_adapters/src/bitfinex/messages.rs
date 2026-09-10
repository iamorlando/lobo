//! R0 operations adapt into native order storage. No parallel book or level state.
use crate::adapter::AdaptForReplay;
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
pub struct RawOrder {
    pub timestamp: u64,
    pub reference: u64,
    /// Zero is the wire deletion marker, not a resting price.
    pub price: u64,
    pub quantity: u64,
    pub side: Side,
}
fn transition<P: PriceType>(order: &mut RestingOrder<P>, details: OrderDetails<P>) {
    order.update_in_place(details);
}
impl<L, S, U, H, Pub> AdaptForReplay<L, S, U, H, Pub> for RawOrder
where
    L: PriceLevelContract<Price = Price64>,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    fn process(self, book: &mut Book<L, S, U, H, Pub>) -> Result<(), OrderStateError> {
        let id = Uuid::from_u128(self.reference.into());
        // Validate external identities here; unchecked native hot APIs receive
        // only live IDs. R0 may evict an order outside the subscribed window.
        let previous = book
            .order_storage
            .order(id)
            .map(|o| (o.price(), o.side(), o.quantity()));
        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        if self.price == 0 {
            if previous.is_some() {
                storage.remove_unchecked(id, &mut publish)?;
            }
            return Ok(());
        }
        let price = L::Price::from(self.price);
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
