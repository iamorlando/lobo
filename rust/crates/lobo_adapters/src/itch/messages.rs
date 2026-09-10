use lobo_events::NullPublisher;
#[cfg(test)]
use lobo_events::PriceLevelChangeEvent;
use std::num::NonZeroU32;

use itchy::Body;

use lobo_books::price_time_priority::Book;
use lobo_models::{
    events::OrderDetails,
    orders::core::{OrderCore, RestingOrder},
};

use lobo_primitives::{
    PriceType,
    time::{DateTime, Utc},
    uuid::Uuid,
};
use lobo_storage::{
    DoNotUpdateUserMap, OrderStateError,
    bidask::traits::{Buy, OrderSide, Sell},
    policies::DoNotUpdateHiddenQuantity,
    price_level::PriceLevelContract,
    price_sorting::PriceSortingPolicy,
};

use crate::adapter::AdaptForReplay;
pub(crate) type ItchBook<L, Sort, Pub = NullPublisher> =
    Book<L, Sort, DoNotUpdateUserMap, DoNotUpdateHiddenQuantity, Pub>;

// -----------------------------------------------------------------------------
// Common conversions
// -----------------------------------------------------------------------------

#[inline(always)]
fn adapt_reference(reference: u64) -> Uuid {
    Uuid::from_u128(reference as u128)
}

// -----------------------------------------------------------------------------
// Replay transitions
// -----------------------------------------------------------------------------

/// Absolute replacement.
///
/// Any Some(...) value in OrderDetails replaces the corresponding value
/// on the existing resting order. None preserves the existing value.
///
/// Used by ITCH Replace because ITCH supplies the new absolute quantity,
/// new price, and new reference.
#[inline(always)]
fn replace_transition<P: PriceType>(order: &mut RestingOrder<P>, order_details: OrderDetails<P>) {
    order.update_in_place(order_details);
}

/// Delta replacement.
///
/// OrderDetails.quantity is interpreted as an amount to REMOVE from the
/// existing resting quantity. The replay-only transition subtracts it
/// directly from the visible resting quantity.
///
/// Used by both ITCH Execute and ITCH Cancel because their `shares` fields
/// are reductions, not new absolute quantities.
#[inline(always)]
fn quantity_delta_transition<P: PriceType>(
    order: &mut RestingOrder<P>,
    order_details: OrderDetails<P>,
) {
    // SAFETY: ITCH Execute and Cancel adapters always provide a share delta.
    let delta = unsafe { order_details.quantity.unwrap_unchecked() };
    order.common_data.quantity = order.common_data.quantity.wrapping_sub(delta);
}

// -----------------------------------------------------------------------------
// Add
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct ItchAdd {
    pub timestamp: u64,
    pub reference: u64,
    pub side: itchy::Side,
    pub shares: u32,
    pub price: itchy::Price4,
}

impl ItchAdd {
    #[inline(always)]
    fn process_sided<S, L, Sort, U, H, Pub>(
        self,
        book: &mut Book<L, Sort, U, H, Pub>,
    ) -> Result<(), OrderStateError>
    where
        S: OrderSide,
        L: PriceLevelContract,
        Sort: PriceSortingPolicy,
        U: lobo_storage::UserMapUpdatePolicy,
        H: lobo_storage::policies::HiddenQuantityPolicy,
        Pub: lobo_events::BookPublisherFactory<L::Price>,
    {
        let order = RestingOrder::<L::Price>::new_replay_order(OrderCore {
            creation_time: DateTime::<Utc>::from_timestamp_nanos(self.timestamp as i64),
            price: Some(L::Price::from(self.price.raw())),
            uuid: adapt_reference(self.reference),
            quantity: self.shares as u64,
            trader: Uuid::nil(),
            side: S::SIDE,
        });

        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        storage
            .add_sided_order::<S>(order, &mut publish)
            .map(|_| ())
    }
}
impl<L, Sort, U, H, Pub> AdaptForReplay<L, Sort, U, H, Pub> for ItchAdd
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: lobo_storage::UserMapUpdatePolicy,
    H: lobo_storage::policies::HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    #[inline(always)]
    fn process(self, book: &mut Book<L, Sort, U, H, Pub>) -> Result<(), OrderStateError> {
        match self.side {
            itchy::Side::Buy => self.process_sided::<Buy, L, Sort, U, H, Pub>(book),
            itchy::Side::Sell => self.process_sided::<Sell, L, Sort, U, H, Pub>(book),
        }
    }
}

// -----------------------------------------------------------------------------
// Execute
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct ItchExecute {
    pub timestamp: u64,
    pub reference: u64,
    pub shares: u32,
    pub execution_price: Option<u32>,
}

impl<L, Sort, U, H, Pub> AdaptForReplay<L, Sort, U, H, Pub> for ItchExecute
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: lobo_storage::UserMapUpdatePolicy,
    H: lobo_storage::policies::HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    #[inline(always)]
    fn process(self, book: &mut Book<L, Sort, U, H, Pub>) -> Result<(), OrderStateError> {
        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        storage
            .modify_with_fill_event(
                adapt_reference(self.reference),
                OrderDetails {
                    uuid: None,
                    price: None,
                    creation_time: None,
                    quantity: Some(self.shares as u64),
                    trader: None,
                    side: None,
                },
                quantity_delta_transition::<L::Price>,
                |resting| self.execution_price.map_or(resting, L::Price::from),
                &mut publish,
            )
            .map(|_| ())
    }
}

// -----------------------------------------------------------------------------
// Cancel
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct ItchCancel {
    pub timestamp: u64,
    pub reference: u64,
    pub shares: u32,
}

impl<L, Sort, U, H, Pub> AdaptForReplay<L, Sort, U, H, Pub> for ItchCancel
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: lobo_storage::UserMapUpdatePolicy,
    H: lobo_storage::policies::HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    #[inline(always)]
    fn process(self, book: &mut Book<L, Sort, U, H, Pub>) -> Result<(), OrderStateError> {
        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        storage
            .modify_order_in_place(
                adapt_reference(self.reference),
                OrderDetails {
                    uuid: None,
                    price: None,
                    creation_time: None,
                    quantity: Some(self.shares as u64),
                    trader: None,
                    side: None,
                },
                quantity_delta_transition::<L::Price>,
                &mut publish,
            )
            .map(|_| ())
    }
}

// -----------------------------------------------------------------------------
// Delete
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct ItchDelete {
    pub timestamp: u64,
    pub reference: u64,
}

impl<L, Sort, U, H, Pub> AdaptForReplay<L, Sort, U, H, Pub> for ItchDelete
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: lobo_storage::UserMapUpdatePolicy,
    H: lobo_storage::policies::HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    #[inline(always)]
    fn process(self, book: &mut Book<L, Sort, U, H, Pub>) -> Result<(), OrderStateError> {
        let order_id = adapt_reference(self.reference);
        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        storage.remove_unchecked(order_id, &mut publish).map(|_| ())
    }
}

// -----------------------------------------------------------------------------
// Replace
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct ItchReplace {
    pub timestamp: u64,
    pub old_reference: u64,
    pub new_reference: u64,
    pub shares: u32,
    pub price: u32,
}

impl<L, Sort, U, H, Pub> AdaptForReplay<L, Sort, U, H, Pub> for ItchReplace
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: lobo_storage::UserMapUpdatePolicy,
    H: lobo_storage::policies::HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    #[inline(always)]
    fn process(self, book: &mut Book<L, Sort, U, H, Pub>) -> Result<(), OrderStateError> {
        let old_order_id = adapt_reference(self.old_reference);

        // Unlike Execute/Cancel, ITCH Replace supplies absolute
        // replacement values.
        let order_data = OrderDetails {
            uuid: Some(adapt_reference(self.new_reference)),
            price: Some(L::Price::from(self.price)),
            creation_time: Some(DateTime::<Utc>::from_timestamp_nanos(self.timestamp as i64)),
            quantity: Some(self.shares as u64),
            trader: None,
            side: None,
        };

        let transition = replace_transition::<L::Price>;
        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        storage
            .replace_in_place(old_order_id, order_data, transition, &mut publish)
            .map(|_| ())
    }
}

// -----------------------------------------------------------------------------
// itchy::Message -> ITCH adapter message -> canonical ReplayEvent
// -----------------------------------------------------------------------------

#[inline(always)]
pub fn adapt_message<L, S, Pub>(
    message: itchy::Message,
    book: &mut ItchBook<L, S, Pub>,
) -> Option<Result<(), OrderStateError>>
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    let timestamp = message.timestamp;

    let result = match message.body {
        Body::AddOrder(add) => ItchAdd {
            timestamp,
            reference: add.reference,
            side: add.side,
            shares: add.shares,
            price: add.price,
        }
        .process(book),

        Body::OrderExecuted {
            reference,
            executed,
            ..
        } => ItchExecute {
            timestamp,
            reference,
            shares: executed,
            execution_price: None,
        }
        .process(book),

        Body::OrderExecutedWithPrice {
            reference,
            executed,
            price,
            ..
        } => ItchExecute {
            timestamp,
            reference,
            shares: executed,
            execution_price: Some(price.raw()),
        }
        .process(book),

        Body::OrderCancelled {
            reference,
            cancelled,
        } => ItchCancel {
            timestamp,
            reference,
            shares: cancelled,
        }
        .process(book),

        Body::DeleteOrder { reference } => ItchDelete {
            timestamp,
            reference,
        }
        .process(book),

        Body::ReplaceOrder(replace) => {
            let Some(shares) = NonZeroU32::new(replace.shares) else {
                panic!("replace got a 0 quantity value")
            };
            ItchReplace {
                timestamp,
                old_reference: replace.old_reference,
                new_reference: replace.new_reference,
                shares: shares.get(),
                price: replace.price.raw(),
            }
            .process(book)
        }

        _ => return None,
    };
    Some(result)
}

#[cfg(test)]
mod publishing_tests {
    use super::*;
    use lobo_events::MpscPublisher;
    use lobo_models::Side;
    use lobo_primitives::Price64;
    use lobo_storage::{
        price_level::{DeepPriceLevel, IntrusivePriceLevel},
        price_sorting::BTreeMapPriceSorting,
    };
    use tokio::sync::mpsc;

    fn mutations<L: PriceLevelContract<Price = Price64>>() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut book = ItchBook::<L, BTreeMapPriceSorting>::new()
            .with_id("ITCH".into())
            .with_publisher(MpscPublisher::new(vec![tx]));
        ItchAdd {
            timestamp: 1,
            reference: 1,
            side: itchy::Side::Buy,
            shares: 10,
            price: itchy::Price4::from(100),
        }
        .process(&mut book)
        .unwrap();
        ItchExecute {
            timestamp: 2,
            reference: 1,
            shares: 2,
            execution_price: None,
        }
        .process(&mut book)
        .unwrap();
        ItchCancel {
            timestamp: 3,
            reference: 1,
            shares: 3,
        }
        .process(&mut book)
        .unwrap();
        ItchReplace {
            timestamp: 4,
            old_reference: 1,
            new_reference: 2,
            shares: 4,
            price: 101,
        }
        .process(&mut book)
        .unwrap();
        ItchDelete {
            timestamp: 5,
            reference: 2,
        }
        .process(&mut book)
        .unwrap();
        ItchAdd {
            timestamp: 6,
            reference: 3,
            side: itchy::Side::Sell,
            shares: 1,
            price: itchy::Price4::from(100),
        }
        .process(&mut book)
        .unwrap();
        ItchExecute {
            timestamp: 7,
            reference: 3,
            shares: 1,
            execution_price: Some(100),
        }
        .process(&mut book)
        .unwrap();
        ItchAdd {
            timestamp: 8,
            reference: 4,
            side: itchy::Side::Buy,
            shares: 2,
            price: itchy::Price4::from(100),
        }
        .process(&mut book)
        .unwrap();
        ItchCancel {
            timestamp: 9,
            reference: 4,
            shares: 2,
        }
        .process(&mut book)
        .unwrap();
        let expected = [
            (100, 10, 1, Side::Buy),
            (100, 8, 1, Side::Buy),
            (100, 5, 1, Side::Buy),
            (100, 0, 0, Side::Buy),
            (101, 4, 1, Side::Buy),
            (101, 0, 0, Side::Buy),
            (100, 1, 1, Side::Sell),
            (100, 0, 0, Side::Sell),
            (100, 2, 1, Side::Buy),
            (100, 0, 0, Side::Buy),
        ];
        for (i, (price, quantity, count, side)) in expected.into_iter().enumerate() {
            let event = rx.try_recv().unwrap();
            assert_eq!(event.book_id(), "ITCH");
            assert_eq!(event.sequence_number(), i as u64 + 1);
            assert_eq!(
                *event.event(),
                PriceLevelChangeEvent::new(quantity, 0, Price64::from(price as u32), count, side)
            );
        }
        assert!(rx.try_recv().is_err());
        assert!(book.order_storage.order_to_arena_map.is_empty());
    }

    #[test]
    fn deep_replay_mutations_publish_once_after_completion() {
        mutations::<DeepPriceLevel<Price64, DoNotUpdateHiddenQuantity>>();
    }
    #[test]
    fn intrusive_replay_mutations_publish_once_after_completion() {
        mutations::<IntrusivePriceLevel<Price64, DoNotUpdateHiddenQuantity>>();
    }
}
