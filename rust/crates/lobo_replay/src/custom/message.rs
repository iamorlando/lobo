use lobo_books::price_time_priority::Book;
use lobo_events::NullPublisher;
use lobo_models::events::ReplayEvent;
use lobo_primitives::PriceType;
use lobo_storage::{
    OrderStateError, UserMapUpdatePolicy, policies::HiddenQuantityPolicy,
    price_level::PriceLevelContract, price_sorting::PriceSortingPolicy,
};

pub trait AdaptForReplay<L, S, U, H, Pub = NullPublisher>
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    fn process(self, book: &mut Book<L, S, U, H, Pub>) -> Result<(), OrderStateError>;
}

pub struct ReplayEvents<P: PriceType>([Option<ReplayEvent<P>>; 2]);

impl<P: PriceType> ReplayEvents<P> {
    pub fn none() -> Self {
        Self([None, None])
    }

    pub fn one(event: ReplayEvent<P>) -> Self {
        Self([Some(event), None])
    }

    pub fn two(first: ReplayEvent<P>, second: ReplayEvent<P>) -> Self {
        Self([Some(first), Some(second)])
    }
}

impl<P: PriceType> IntoIterator for ReplayEvents<P> {
    type Item = ReplayEvent<P>;
    type IntoIter = std::iter::Flatten<std::array::IntoIter<Option<ReplayEvent<P>>, 2>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter().flatten()
    }
}
