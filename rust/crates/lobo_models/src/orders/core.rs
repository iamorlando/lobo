use crate::Side;
use crate::events::{ExecutionResult, OrderDetails};
use crate::orders::core::FillBehavior::{BlockIncrements, MinimumQuantity};
use crate::orders::order_types::{IcebergOrder, IcebergOrderData, LimitOrder, MarketOrder};
use crate::orders::traits::{
    Expires, Fills, HandlesCompletion, IntoRestingOrderData, Replenishes, Trades,
};
use lobo_primitives::PriceType;
#[cfg(feature = "python")]
use lobo_primitives::mixins::DisplayFields;
#[cfg(feature = "python")]
use lobo_primitives::mixins::PyDisplay;
use lobo_primitives::{time::DateTime, time::Utc, uuid::Uuid};
#[cfg(feature = "python")]
use pyo3::prelude::*;

#[derive(Clone)]
pub struct OrderCore<P: PriceType> {
    pub uuid: Uuid,
    pub price: Option<P>,
    pub creation_time: DateTime<Utc>,
    pub quantity: u64,
    pub trader: Uuid,
    pub side: Side,
}

pub struct Order<T, P: PriceType> {
    pub common_data: OrderCore<P>,
    pub typed_order_details: T,
}
#[cfg(feature = "python")]
#[pyclass(name = "Order", module = "lobo.orders", extends = PyDisplay,
subclass)]
pub struct PyOrder;

#[cfg(feature = "python")]
#[pymethods]
impl PyOrder {
    #[new]
    fn new() -> PyClassInitializer<Self> {
        PyClassInitializer::from(PyDisplay).add_subclass(Self)
    }
    #[classattr]
    fn __display_fields__() -> Vec<&'static str> {
        Self::display_fields()
    }
}
#[cfg(feature = "python")]
impl DisplayFields for PyOrder {
    fn display_fields() -> Vec<&'static str> {
        vec!["quantity", "trader", "side"]
    }
}

impl<T, P: PriceType> Ord for Order<T, P> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.common_data
            .price
            .cmp(&other.common_data.price)
            .then_with(|| self.common_data.uuid.cmp(&other.common_data.uuid))
    }
}
impl<T, P: PriceType> PartialOrd for Order<T, P> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T, P: PriceType> PartialEq for Order<T, P> {
    fn eq(&self, other: &Self) -> bool {
        self.common_data.price.cmp(&other.common_data.price) == std::cmp::Ordering::Equal
    }
}
impl<T, P: PriceType> Eq for Order<T, P> {}
impl<T, P: PriceType> Order<T, P> {
    // Available for every Order<T>.

    pub fn with_uuid(mut self, uuid: Uuid) -> Self {
        self.common_data.uuid = uuid;
        self
    }

    pub fn with_creation_time(mut self, creation_time: DateTime<Utc>) -> Self {
        self.common_data.creation_time = creation_time;
        self
    }
}
impl IntoRestingOrderData for RestingOrderData {
    #[inline(always)]
    fn discard_hidden(&mut self) {
        self.replenishment_behavior = ReplenishmentBehavior::Remove;
    }
    #[inline(always)]
    fn into_resting_data(self) -> RestingOrderData {
        self
    }
}
impl<P: PriceType> Order<RestingOrderData, P> {
    pub fn new_replay_order(common_data: OrderCore<P>) -> Self {
        Self {
            common_data,
            typed_order_details: RestingOrderData {
                fill_behavior: FillBehavior::Standard,
                replenishment_behavior: ReplenishmentBehavior::Remove,
                expiration_behavior: ExpirationBehavior::DoesNotExpire,
            },
        }
    }
    pub fn update_in_place(&mut self, order_details: OrderDetails<P>) {
        self.common_data = OrderCore {
            uuid: order_details.uuid.unwrap_or_else(|| self.uuid()),
            price: Some(order_details.price.unwrap_or_else(|| self.price().unwrap())),
            creation_time: order_details
                .creation_time
                .unwrap_or(self.common_data.creation_time),
            quantity: order_details.quantity.unwrap_or_else(|| self.quantity()),
            trader: order_details.trader.unwrap_or(self.common_data.trader),
            side: order_details.side.unwrap_or_else(|| self.side()),
        };
    }

    pub fn update_from(mut self, order_details: OrderDetails<P>) -> Self {
        self.update_in_place(order_details);
        self
    }
}
impl<T, P: PriceType> Order<T, P>
where
    T: IntoRestingOrderData,
{
    // Available only when T implements IntoRestingOrderData.

    pub fn into_resting(self) -> RestingOrder<P> {
        let Order {
            common_data,
            typed_order_details,
        } = self;

        Order {
            common_data,
            typed_order_details: typed_order_details.into_resting_data(),
        }
    }
}
impl<T, P: PriceType> HandlesCompletion<P> for Order<T, P>
where
    T: IntoRestingOrderData,
{
    #[inline(always)]
    fn discard_hidden(&mut self) {
        self.typed_order_details.discard_hidden();
    }
    fn handle_completion(self, execution_result: &mut ExecutionResult<P>) {
        execution_result.add_remaining_order(self);
    }
}
impl<T, P: PriceType> Trades<P> for Order<T, P> {
    fn price(&self) -> Option<P> {
        self.common_data.price
    }
    fn quantity(&self) -> u64 {
        self.common_data.quantity
    }
    fn side(&self) -> Side {
        self.common_data.side
    }
    fn uuid(&self) -> Uuid {
        self.common_data.uuid
    }
}
#[derive(Clone)]
pub struct RestingOrderData {
    pub fill_behavior: FillBehavior,
    pub replenishment_behavior: ReplenishmentBehavior,
    pub expiration_behavior: ExpirationBehavior,
}
#[derive(Clone)]
pub enum ExpirationBehavior {
    DoesNotExpire,
    RemoveAfterUtc(DateTime<Utc>),
}
#[derive(Clone)]
pub enum FillBehavior {
    Standard,
    AllOrNothing,
    MinimumQuantity(u64),
    BlockIncrements(u64),
}

impl<P: PriceType> Expires for RestingOrder<P> {
    fn is_expired(&self, now: DateTime<Utc>) -> bool {
        match self.typed_order_details.expiration_behavior {
            ExpirationBehavior::DoesNotExpire => false,
            ExpirationBehavior::RemoveAfterUtc(t) => now > t,
        }
    }
}
impl FillBehavior {
    pub(crate) fn fill_calc(quantity: u64, against: u64) -> u64 {
        if quantity >= against {
            against
        } else {
            quantity
        }
    }
    fn fillable_quantity(&self, quantity: u64, against: u64) -> Option<u64> {
        match self {
            FillBehavior::Standard => Some(Self::fill_calc(quantity, against)),
            FillBehavior::AllOrNothing => {
                if quantity <= against {
                    Some(quantity)
                } else {
                    None
                }
            }
            MinimumQuantity(q) => {
                if q > &against {
                    Some(Self::fill_calc(quantity, against))
                } else {
                    None
                }
            }
            BlockIncrements(b) => {
                let s = against / b;
                if s > *b {
                    Some(Self::fill_calc(quantity, against))
                } else {
                    None
                }
            }
        }
    }
}
#[derive(Clone)]
pub enum ReplenishmentBehavior {
    Remove,
    Iceberg {
        hidden_quantity: u64,
        peak_quantity: u64,
    },
}

pub type RestingOrder<P> = Order<RestingOrderData, P>;
impl<P: PriceType> Clone for RestingOrder<P> {
    fn clone(&self) -> Self {
        Self {
            common_data: self.common_data.clone(),
            typed_order_details: self.typed_order_details.clone(),
        }
    }
}
impl<P: PriceType> Fills for RestingOrder<P> {
    fn fillable_quantity(&self, against: u64) -> Option<u64> {
        self.typed_order_details
            .fill_behavior
            .fillable_quantity(self.quantity(), against)
    }
}
pub enum ReplenishedOrder<P: PriceType> {
    Market(MarketOrder<P>),
    Limit(LimitOrder<P>),
    Iceberg(IcebergOrder<P>),
}
impl<P: PriceType> Replenishes for RestingOrder<P> {
    type Output = ReplenishedOrder<P>;
    // this will replenish anything when run. only run when quantity is at 0
    fn replenish_into(self) -> Option<ReplenishedOrder<P>> {
        match self.typed_order_details.replenishment_behavior {
            ReplenishmentBehavior::Iceberg {
                hidden_quantity,
                peak_quantity,
            } => {
                let next_quantity = hidden_quantity.min(peak_quantity);

                if next_quantity == 0 {
                    return None;
                }

                let mut order = Order {
                    common_data: self.common_data,
                    typed_order_details: IcebergOrderData {
                        hidden_quantity: hidden_quantity - next_quantity,
                        peak_quantity,
                    },
                };
                order.common_data.quantity = next_quantity;

                Some(ReplenishedOrder::Iceberg(order))
            }
            ReplenishmentBehavior::Remove => None,
        }
    }
    fn replenish_in_place(&mut self) {
        debug_assert_eq!(
            self.common_data.quantity, 0,
            "only replenish depleted orders"
        );

        match &mut self.typed_order_details.replenishment_behavior {
            ReplenishmentBehavior::Iceberg {
                hidden_quantity,
                peak_quantity,
            } => {
                let next_quantity = (*hidden_quantity).min(*peak_quantity);

                self.common_data.quantity = next_quantity;
                *hidden_quantity -= next_quantity;
            }

            ReplenishmentBehavior::Remove => {
                // Leave quantity at zero.
                // The pruning code will remove it.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::{
        order_types::{IcebergOrder, LimitOrder},
        traits::{Expires, Fills, HandlesCompletion, Replenishes, Trades},
    };
    use lobo_primitives::CompressedPrice;
    use std::time::Duration;

    fn p(raw: u32) -> CompressedPrice {
        CompressedPrice::from(raw)
    }

    fn resting_with(
        quantity: u64,
        fill_behavior: FillBehavior,
        replenishment_behavior: ReplenishmentBehavior,
        expiration_behavior: ExpirationBehavior,
    ) -> RestingOrder<CompressedPrice> {
        Order {
            common_data: OrderCore {
                uuid: Uuid::new_v4(),
                price: Some(p(101)),
                creation_time: Utc::now(),
                quantity,
                trader: Uuid::new_v4(),
                side: Side::Buy,
            },
            typed_order_details: RestingOrderData {
                fill_behavior,
                replenishment_behavior,
                expiration_behavior,
            },
        }
    }

    #[test]
    fn order_builders_and_trade_accessors_preserve_common_data() {
        let id = Uuid::new_v4();
        let trader = Uuid::new_v4();
        let created = Utc::now() - Duration::from_secs(5);
        let order = LimitOrder::<CompressedPrice>::new(Some(p(99)), 12, trader, Side::Sell)
            .with_uuid(id)
            .with_creation_time(created);

        assert_eq!(order.uuid(), id);
        assert_eq!(order.price(), Some(p(99)));
        assert_eq!(order.quantity(), 12);
        assert_eq!(order.side(), Side::Sell);
        assert_eq!(order.common_data.trader, trader);
        assert_eq!(order.common_data.creation_time, created);
    }

    #[test]
    fn ordering_uses_price_then_uuid_and_equality_uses_price() {
        let low_id = Uuid::from_u128(1);
        let high_id = Uuid::from_u128(2);
        let low = LimitOrder::<CompressedPrice>::new(Some(p(99)), 1, Uuid::new_v4(), Side::Buy)
            .with_uuid(high_id);
        let high = LimitOrder::<CompressedPrice>::new(Some(p(100)), 1, Uuid::new_v4(), Side::Buy)
            .with_uuid(low_id);
        let same_price =
            LimitOrder::<CompressedPrice>::new(Some(p(100)), 5, Uuid::new_v4(), Side::Sell)
                .with_uuid(high_id);

        assert!(low < high);
        assert!(high == same_price);
        assert_eq!(high.partial_cmp(&same_price), Some(low_id.cmp(&high_id)));
    }

    #[test]
    fn limit_orders_convert_to_standard_non_replenishing_resting_orders() {
        let id = Uuid::new_v4();
        let limit = LimitOrder::<CompressedPrice>::new(Some(p(42)), 7, Uuid::new_v4(), Side::Buy)
            .with_uuid(id);
        let resting = limit.into_resting();

        assert_eq!(resting.uuid(), id);
        assert!(matches!(
            resting.typed_order_details.fill_behavior,
            FillBehavior::Standard
        ));
        assert!(matches!(
            resting.typed_order_details.replenishment_behavior,
            ReplenishmentBehavior::Remove
        ));
    }

    #[test]
    fn completion_places_resting_remainder_in_execution_result() {
        let order = LimitOrder::<CompressedPrice>::new(Some(p(88)), 4, Uuid::new_v4(), Side::Sell);
        let id = order.uuid();
        let mut result = ExecutionResult::new(false);

        order.handle_completion(&mut result);

        assert_eq!(result.remaining_order_id, Some(id));
        assert_eq!(result.remaining_order.as_ref().unwrap().quantity(), 4);
    }

    #[test]
    fn standard_and_all_or_nothing_fill_rules_cover_partial_and_rejected_fills() {
        let standard = resting_with(
            10,
            FillBehavior::Standard,
            ReplenishmentBehavior::Remove,
            ExpirationBehavior::DoesNotExpire,
        );
        assert_eq!(standard.fillable_quantity(4), Some(4));
        assert_eq!(standard.fillable_quantity(40), Some(10));

        let all_or_nothing = resting_with(
            10,
            FillBehavior::AllOrNothing,
            ReplenishmentBehavior::Remove,
            ExpirationBehavior::DoesNotExpire,
        );
        assert_eq!(all_or_nothing.fillable_quantity(9), None);
        assert_eq!(all_or_nothing.fillable_quantity(10), Some(10));
    }

    #[test]
    fn expiration_is_strictly_after_the_deadline() {
        let deadline = Utc::now();
        let expiring = resting_with(
            1,
            FillBehavior::Standard,
            ReplenishmentBehavior::Remove,
            ExpirationBehavior::RemoveAfterUtc(deadline),
        );
        let permanent = resting_with(
            1,
            FillBehavior::Standard,
            ReplenishmentBehavior::Remove,
            ExpirationBehavior::DoesNotExpire,
        );

        assert!(!expiring.is_expired(deadline));
        assert!(expiring.is_expired(deadline + Duration::from_nanos(1)));
        assert!(!permanent.is_expired(deadline + Duration::from_secs(86_400)));
    }

    #[test]
    fn iceberg_resting_order_replenishes_into_successive_peaks() {
        let iceberg =
            IcebergOrder::<CompressedPrice>::new(Some(p(100)), Uuid::new_v4(), Side::Sell, 7, 3);
        let resting: RestingOrder<CompressedPrice> = iceberg.into();

        let replenished = resting.replenish_into().unwrap();
        let ReplenishedOrder::Iceberg(replenished) = replenished else {
            panic!("expected iceberg replenishment")
        };
        assert_eq!(replenished.quantity(), 3);
        assert_eq!(replenished.typed_order_details.hidden_quantity, 4);
    }

    #[test]
    fn depleted_or_non_replenishing_resting_orders_return_none() {
        let depleted = resting_with(
            0,
            FillBehavior::Standard,
            ReplenishmentBehavior::Iceberg {
                hidden_quantity: 0,
                peak_quantity: 3,
            },
            ExpirationBehavior::DoesNotExpire,
        );
        assert!(depleted.replenish_into().is_none());

        let removed = resting_with(
            0,
            FillBehavior::Standard,
            ReplenishmentBehavior::Remove,
            ExpirationBehavior::DoesNotExpire,
        );
        assert!(removed.replenish_into().is_none());
    }

    #[test]
    fn in_place_replenishment_updates_iceberg_and_leaves_remove_at_zero() {
        let mut iceberg = resting_with(
            0,
            FillBehavior::Standard,
            ReplenishmentBehavior::Iceberg {
                hidden_quantity: 5,
                peak_quantity: 3,
            },
            ExpirationBehavior::DoesNotExpire,
        );
        iceberg.replenish_in_place();
        assert_eq!(iceberg.quantity(), 3);
        assert!(matches!(
            iceberg.typed_order_details.replenishment_behavior,
            ReplenishmentBehavior::Iceberg {
                hidden_quantity: 2,
                peak_quantity: 3
            }
        ));

        let mut removed = resting_with(
            0,
            FillBehavior::Standard,
            ReplenishmentBehavior::Remove,
            ExpirationBehavior::DoesNotExpire,
        );
        removed.replenish_in_place();
        assert_eq!(removed.quantity(), 0);
    }

    #[cfg(feature = "python")]
    #[test]
    fn python_order_display_fields_are_stable() {
        assert_eq!(PyOrder::display_fields(), ["quantity", "trader", "side"]);
        assert_eq!(PyOrder::__display_fields__(), PyOrder::display_fields());
        let _initializer = PyOrder::new();
    }
}
