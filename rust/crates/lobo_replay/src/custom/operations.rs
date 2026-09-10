//! Venue-independent execution notifications over publishers.
use super::AdaptForReplay;
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

/// One decoded aggregate message. The quote type selects metadata handling.
pub struct LevelBatch<Q> {
    pub timestamp: u64,
    pub bids: Vec<Q>,
    pub asks: Vec<Q>,
    pub depth: usize,
}
pub type LevelUpdate<P> = LevelBatch<(P, u64)>;
pub type FormattedLevelUpdate<P> =
    LevelBatch<(P, u64, lobo_storage::policies::checksum::DecimalWidths)>;
impl<Q, L, S, U, H, Pub> AdaptForReplay<L, S, U, H, Pub> for LevelBatch<Q>
where
    Q: LevelQuote<Price = L::Price>,
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    fn process(self, book: &mut Book<L, S, U, H, Pub>) -> Result<(), OrderStateError> {
        let (storage, mut publish) = book.storage_and_publisher_at(self.timestamp);
        for quote in self.asks {
            quote.apply(&mut storage.asks, &mut publish);
        }
        for quote in self.bids {
            quote.apply(&mut storage.bids, &mut publish);
        }
        storage
            .asks
            .retain_level_quantities(self.depth, &mut publish);
        storage
            .bids
            .retain_level_quantities(self.depth, &mut publish);
        Ok(())
    }
}
/// Formatting metadata is selected by the message type, never inspected in matching.
pub trait LevelQuote {
    type Price: PriceType;
    fn apply<L, S, Sort, H, Pub>(
        self,
        store: &mut lobo_storage::bidask::SidedOrderStore<S, L, Sort, H>,
        publish: &mut Pub,
    ) where
        L: PriceLevelContract<Price = Self::Price>,
        S: lobo_storage::bidask::traits::SidedPrice<Self::Price>,
        Sort: PriceSortingPolicy,
        H: HiddenQuantityPolicy,
        Pub: lobo_events::MutationPublisher<Self::Price>;
}
impl<P: PriceType> LevelQuote for (P, u64) {
    type Price = P;
    #[inline(always)]
    fn apply<L, S, Sort, H, Pub>(
        self,
        store: &mut lobo_storage::bidask::SidedOrderStore<S, L, Sort, H>,
        publish: &mut Pub,
    ) where
        L: PriceLevelContract<Price = P>,
        S: lobo_storage::bidask::traits::SidedPrice<P>,
        Sort: PriceSortingPolicy,
        H: HiddenQuantityPolicy,
        Pub: lobo_events::MutationPublisher<P>,
    {
        store.set_level_quantity(self.0, self.1, publish);
    }
}
impl<P: PriceType> LevelQuote for (P, u64, lobo_storage::policies::checksum::DecimalWidths) {
    type Price = P;
    #[inline(always)]
    fn apply<L, S, Sort, H, Pub>(
        self,
        store: &mut lobo_storage::bidask::SidedOrderStore<S, L, Sort, H>,
        publish: &mut Pub,
    ) where
        L: PriceLevelContract<Price = P>,
        S: lobo_storage::bidask::traits::SidedPrice<P>,
        Sort: PriceSortingPolicy,
        H: HiddenQuantityPolicy,
        Pub: lobo_events::MutationPublisher<P>,
    {
        store.set_level_quantity_formatted(self.0, self.1, self.2, publish);
    }
}
