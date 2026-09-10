use lobo_primitives::{
    PriceType,
    time::{DateTime, Utc},
    uuid::Uuid,
};

/// Orders implementing Trades (as in: "this trades against other orders" are able to trade.)
use crate::{Side, events::ExecutionResult, orders::core::RestingOrderData};
pub trait StaticOrderData {
    const CAN_REST: bool = true;
}
pub trait Trades<P: PriceType> {
    fn side(&self) -> Side;
    fn price(&self) -> Option<P>;
    fn quantity(&self) -> u64;
    fn uuid(&self) -> Uuid;
    // fn trade(&mut self, order: impl Trades) -> OrderMatch;
    // fn can_rest(&self) -> bool;
}
pub trait Fills {
    fn fillable_quantity(&self, against: u64) -> Option<u64>;
}
pub trait Expires {
    fn is_expired(&self, now: DateTime<Utc>) -> bool;
}

pub trait Replenishes {
    type Output;

    fn replenish_into(self) -> Option<Self::Output>;
    fn replenish_in_place(&mut self) {
        // Default: do nothing.
    }
}
pub trait IntoRestingOrderData {
    /// Concrete visible-only policies erase reserve state before matching.
    fn discard_hidden(&mut self) {}
    fn into_resting_data(self) -> RestingOrderData;
}
pub trait HandlesCompletion<P: PriceType> {
    fn discard_hidden(&mut self) {}
    fn handle_completion(self, execution_result: &mut ExecutionResult<P>);
}

#[cfg(test)]
mod tests {
    use super::{Replenishes, StaticOrderData};

    struct StaticData;
    impl StaticOrderData for StaticData {}

    struct NonReplenishing;
    impl Replenishes for NonReplenishing {
        type Output = ();

        fn replenish_into(self) -> Option<Self::Output> {
            None
        }
    }

    #[test]
    fn static_order_data_can_rest_by_default() {
        assert!(StaticData::CAN_REST);
    }

    #[test]
    fn default_in_place_replenishment_is_a_noop() {
        let mut value = NonReplenishing;
        value.replenish_in_place();
        assert!(value.replenish_into().is_none());
    }
}
