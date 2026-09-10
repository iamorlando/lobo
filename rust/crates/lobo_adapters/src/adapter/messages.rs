//! Venue-independent execution notifications over native publishers.
use crate::adapter::AdaptForReplay;
use lobo_books::price_time_priority::Book;
use lobo_primitives::PriceType;
use lobo_storage::{
    OrderStateError, UserMapUpdatePolicy, policies::HiddenQuantityPolicy,
    price_level::PriceLevelContract, price_sorting::PriceSortingPolicy,
};

/// An execution reported by the public trade channel. Book deltas already
/// account for its liquidity; publishing it must not reduce the book again.
#[derive(Clone, Copy, Debug)]
pub struct PublicTrade<P> {
    pub timestamp: u64,
    pub price: P,
    pub quantity: u64,
    pub maker_side: lobo_models::Side,
}
impl<P, L, S, U, H, Pub> AdaptForReplay<L, S, U, H, Pub> for PublicTrade<P>
where
    P: PriceType,
    L: PriceLevelContract<Price = P>,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<P>,
{
    fn process(self, book: &mut Book<L, S, U, H, Pub>) -> Result<(), OrderStateError> {
        use lobo_events::{MutationPublisher, TradedVolumeEvent};
        let (_, mut publish) = book.storage_and_publisher_at(self.timestamp);
        publish.traded_volume(|| {
            Some(TradedVolumeEvent {
                price: self.price,
                quantity: self.quantity,
                side: self.maker_side,
            })
        });
        Ok(())
    }
}
