#[cfg(feature = "python")]
use lobo_primitives::mixins::{DisplayFields, PyDisplay};
#[cfg(feature = "python")]
use pyo3::prelude::*;

#[cfg(feature = "python")]
use crate::orders::core::PyOrder;

use std::cmp::min;

#[cfg(any(test, feature = "python"))]
use lobo_primitives::CompressedPrice;
use lobo_primitives::{PriceType, time::Utc, uuid::Uuid};

use crate::{
    Side,
    orders::{
        core::{
            ExpirationBehavior, FillBehavior, Order, OrderCore, ReplenishedOrder,
            ReplenishmentBehavior, RestingOrder, RestingOrderData,
        },
        traits::{Fills, HandlesCompletion, IntoRestingOrderData, Replenishes, Trades},
    },
};

pub struct MarketOrderData;
pub type MarketOrder<P> = Order<MarketOrderData, P>;

impl<P: PriceType> Order<MarketOrderData, P> {
    pub fn new(quantity: u64, trader: Uuid, side: Side) -> Self {
        let uuid = Uuid::new_v4();
        let creation_time = Utc::now();
        Self {
            common_data: OrderCore {
                creation_time,
                price: None,
                uuid,
                quantity,
                trader,
                side,
            },
            typed_order_details: MarketOrderData,
        }
    }
}

#[cfg(feature = "python")]
#[pyclass(
    name = "MarketOrder",
    module = "lobo.orders",
    extends = PyOrder,
    from_py_object,
    get_all
)]
#[derive(Clone)] // #[cfg(feature = "python")]
pub struct PyMarketOrder {
    quantity: u64,
    trader: Uuid,
    side: Side,
}
#[cfg(feature = "python")]
#[pymethods]
impl PyMarketOrder {
    #[new]
    fn new(quantity: u64, trader: Uuid, side: Side) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PyDisplay)
            .add_subclass(PyOrder)
            .add_subclass(Self {
                quantity,
                trader,
                side,
            })
    }
    #[classattr]
    fn __display_fields__() -> Vec<&'static str> {
        PyOrder::display_fields()
    }
}
#[cfg(feature = "python")]
impl From<&PyMarketOrder> for MarketOrder<CompressedPrice> {
    fn from(value: &PyMarketOrder) -> Self {
        Self::new(value.quantity, value.trader, value.side)
    }
}
#[cfg(feature = "python")]
impl From<PyMarketOrder> for MarketOrder<CompressedPrice> {
    fn from(value: PyMarketOrder) -> Self {
        (&value).into()
    }
}

#[cfg(feature = "python")]
impl<'a, 'py> FromPyObject<'a, 'py> for MarketOrder<CompressedPrice> {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
        let py_order: PyMarketOrder = obj.extract()?;
        Ok(py_order.into())
    }
}

impl<P: PriceType> Replenishes for MarketOrder<P> {
    type Output = ReplenishedOrder<P>;
    fn replenish_into(self) -> Option<ReplenishedOrder<P>> {
        None
    }
}
impl<P: PriceType> Fills for MarketOrder<P> {
    fn fillable_quantity(&self, against: u64) -> Option<u64> {
        Some(FillBehavior::fill_calc(self.quantity(), against))
    }
}
impl<P: PriceType> HandlesCompletion<P> for MarketOrder<P> {
    fn handle_completion(self, _: &mut crate::events::ExecutionResult<P>) {}
}

#[derive(Default)]
pub struct LimitOrderData;

pub type LimitOrder<P> = Order<LimitOrderData, P>;

impl<P: PriceType> Clone for LimitOrder<P> {
    fn clone(&self) -> Self {
        Self {
            common_data: self.common_data.clone(),
            typed_order_details: LimitOrderData,
        }
    }
}
impl<P: PriceType> Order<LimitOrderData, P> {
    pub fn new<Q>(price: Option<Q>, quantity: u64, trader: Uuid, side: Side) -> Self
    where
        Q: TryInto<P>,
    {
        let uuid = Uuid::new_v4();
        let creation_time = Utc::now();
        let price = price.map(|price| {
            price
                .try_into()
                .ok()
                .expect("limit-order price exceeds configured range")
        });
        Self {
            common_data: OrderCore {
                creation_time,
                price,
                uuid,
                quantity,
                trader,
                side,
            },
            typed_order_details: LimitOrderData,
        }
    }
}

#[cfg(feature = "python")]
impl From<&PyLimitOrder> for LimitOrder<CompressedPrice> {
    fn from(value: &PyLimitOrder) -> Self {
        Self::new(Some(value.price), value.quantity, value.trader, value.side)
    }
}
#[cfg(feature = "python")]
impl From<PyLimitOrder> for LimitOrder<CompressedPrice> {
    fn from(value: PyLimitOrder) -> Self {
        (&value).into()
    }
}

#[cfg(feature = "python")]
#[pyclass(name = "LimitOrder", module = "lobo.orders",extends = PyOrder, from_py_object,get_all)]
#[derive(Clone)] // #[cfg(feature = "python")]
pub struct PyLimitOrder {
    price: CompressedPrice,
    quantity: u64,
    trader: Uuid,
    side: Side,
}

#[cfg(feature = "python")]
impl<'a, 'py> FromPyObject<'a, 'py> for LimitOrder<CompressedPrice> {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
        let py_order: PyLimitOrder = obj.extract()?;
        Ok(py_order.into())
    }
}

#[cfg(feature = "python")]
#[pymethods]
impl PyLimitOrder {
    #[new]
    fn new(
        price: CompressedPrice,
        quantity: u64,
        trader: Uuid,
        side: Side,
    ) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PyDisplay)
            .add_subclass(PyOrder)
            .add_subclass(Self {
                price,
                quantity,
                trader,
                side,
            })
    }
    #[classattr]
    fn __display_fields__() -> Vec<&'static str> {
        let mut fields = PyOrder::display_fields();
        fields.insert(0, "price");
        fields
    }
}

impl<P: PriceType> Replenishes for LimitOrder<P> {
    type Output = ReplenishedOrder<P>; // if you want o replenish into something, new type required
    fn replenish_into(self) -> Option<ReplenishedOrder<P>> {
        None
    }
}
impl<P: PriceType> Fills for LimitOrder<P> {
    fn fillable_quantity(&self, against: u64) -> Option<u64> {
        Some(FillBehavior::fill_calc(self.quantity(), against))
    }
}
impl IntoRestingOrderData for LimitOrderData {
    fn into_resting_data(self) -> RestingOrderData {
        RestingOrderData {
            fill_behavior: FillBehavior::Standard,
            replenishment_behavior: ReplenishmentBehavior::Remove,
            expiration_behavior: ExpirationBehavior::DoesNotExpire,
        }
    }
}
impl<P: PriceType> From<LimitOrder<P>> for RestingOrder<P> {
    fn from(order: LimitOrder<P>) -> Self {
        order.into_resting()
    }
}

pub struct IcebergOrderData {
    pub hidden_quantity: u64,
    pub peak_quantity: u64,
}
pub type IcebergOrder<P> = Order<IcebergOrderData, P>;
impl<P: PriceType> Order<IcebergOrderData, P> {
    pub fn new<Q>(
        price: Option<Q>,
        trader: Uuid,
        side: Side,
        hidden_quantity: u64,
        peak_quantity: u64,
    ) -> Self
    where
        Q: TryInto<P>,
    {
        let uuid = Uuid::new_v4();
        let creation_time = Utc::now();
        let price = price.map(|price| {
            price
                .try_into()
                .ok()
                .expect("iceberg-order price exceeds configured range")
        });
        Self {
            common_data: OrderCore {
                creation_time,
                price,
                uuid,
                quantity: peak_quantity,
                trader,
                side,
            },
            typed_order_details: IcebergOrderData {
                hidden_quantity,
                peak_quantity,
            },
        }
    }
}
impl<P: PriceType> From<IcebergOrder<P>> for RestingOrder<P> {
    fn from(order: IcebergOrder<P>) -> Self {
        order.into_resting()
    }
}
impl<P: PriceType> From<IcebergOrder<P>> for ReplenishedOrder<P> {
    fn from(order: IcebergOrder<P>) -> Self {
        Self::Iceberg(order)
    }
}
impl<P: PriceType> Replenishes for IcebergOrder<P> {
    type Output = ReplenishedOrder<P>;

    fn replenish_into(mut self) -> Option<Self::Output> {
        let next_quantity = min(
            self.typed_order_details.peak_quantity,
            self.typed_order_details.hidden_quantity,
        );

        if next_quantity == 0 {
            return None;
        }

        self.common_data.quantity = next_quantity;
        self.typed_order_details.hidden_quantity -= next_quantity;

        Some(self.into())
    }
}
impl<P: PriceType> Fills for IcebergOrder<P> {
    fn fillable_quantity(&self, against: u64) -> Option<u64> {
        Some(FillBehavior::fill_calc(self.quantity(), against))
    }
}
impl IntoRestingOrderData for IcebergOrderData {
    #[inline(always)]
    fn discard_hidden(&mut self) {
        self.hidden_quantity = 0;
    }
    fn into_resting_data(self) -> RestingOrderData {
        RestingOrderData {
            fill_behavior: FillBehavior::Standard,
            expiration_behavior: ExpirationBehavior::DoesNotExpire,
            replenishment_behavior: ReplenishmentBehavior::Iceberg {
                hidden_quantity: self.hidden_quantity,
                peak_quantity: self.peak_quantity,
            },
        }
    }
}
#[cfg(feature = "python")]
#[pyclass(name = "IcebergOrder", module = "lobo.orders",extends = PyOrder, from_py_object,get_all)]
#[derive(Clone)] // #[cfg(feature = "python")]
pub struct PyIcebergOrder {
    price: CompressedPrice,
    quantity: u64,
    trader: Uuid,
    side: Side,
    hidden_quantity: u64,
}

#[cfg(feature = "python")]
impl<'a, 'py> FromPyObject<'a, 'py> for IcebergOrder<CompressedPrice> {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
        let py_order: PyIcebergOrder = obj.extract()?;
        Ok(py_order.into())
    }
}

#[cfg(feature = "python")]
impl From<&PyIcebergOrder> for IcebergOrder<CompressedPrice> {
    fn from(value: &PyIcebergOrder) -> Self {
        Self::new(
            Some(value.price),
            value.trader,
            value.side,
            value.hidden_quantity,
            value.quantity,
        )
    }
}
#[cfg(feature = "python")]
impl From<PyIcebergOrder> for IcebergOrder<CompressedPrice> {
    fn from(value: PyIcebergOrder) -> Self {
        (&value).into()
    }
}
#[cfg(feature = "python")]
#[pymethods]
impl PyIcebergOrder {
    #[new]
    fn new(
        price: CompressedPrice,
        quantity: u64,
        trader: Uuid,
        side: Side,
        hidden_quantity: u64,
    ) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PyDisplay)
            .add_subclass(PyOrder)
            .add_subclass(Self {
                price,
                quantity,
                trader,
                side,
                hidden_quantity,
            })
    }
    #[classattr]
    fn __display_fields__() -> Vec<&'static str> {
        let mut fields = PyOrder::display_fields();
        fields.insert(0, "price");
        fields.push("hidden_quantity");
        fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::ExecutionResult,
        orders::{
            core::{ReplenishedOrder, ReplenishmentBehavior, RestingOrder},
            traits::{Fills, HandlesCompletion, Replenishes, Trades},
        },
    };

    fn p(raw: u32) -> CompressedPrice {
        CompressedPrice::from(raw)
    }

    #[test]
    fn market_order_exposes_market_semantics() {
        let trader = Uuid::new_v4();
        let order = MarketOrder::<CompressedPrice>::new(8, trader, Side::Buy);

        assert_eq!(order.price(), None);
        assert_eq!(order.quantity(), 8);
        assert_eq!(order.trader_id(), trader);
        assert_eq!(order.side(), Side::Buy);
        assert_eq!(order.fillable_quantity(3), Some(3));
        assert_eq!(order.fillable_quantity(20), Some(8));
        assert!(order.replenish_into().is_none());
    }

    #[test]
    fn completed_market_order_does_not_leave_a_remainder() {
        let order = MarketOrder::<CompressedPrice>::new(5, Uuid::new_v4(), Side::Sell);
        let mut result = ExecutionResult::<CompressedPrice>::new(false);
        order.handle_completion(&mut result);
        assert!(result.remaining_order.is_none());
        assert!(result.remaining_order_id.is_none());
    }

    #[test]
    fn limit_order_fills_and_converts_to_resting_data() {
        let id = Uuid::new_v4();
        let order = LimitOrder::<CompressedPrice>::new(Some(p(101)), 9, Uuid::new_v4(), Side::Sell)
            .with_uuid(id);
        assert_eq!(order.fillable_quantity(4), Some(4));
        assert!(
            LimitOrder::<CompressedPrice>::new(Some(p(101)), 9, Uuid::new_v4(), Side::Sell)
                .replenish_into()
                .is_none()
        );

        let resting: RestingOrder<CompressedPrice> = order.into();
        assert_eq!(resting.uuid(), id);
        assert!(matches!(
            resting.typed_order_details.replenishment_behavior,
            ReplenishmentBehavior::Remove
        ));
    }

    #[test]
    fn iceberg_replenishment_consumes_hidden_quantity_by_peak() {
        let order =
            IcebergOrder::<CompressedPrice>::new(Some(p(103)), Uuid::new_v4(), Side::Buy, 8, 3);
        assert_eq!(order.fillable_quantity(2), Some(2));

        let first = order.replenish_into().unwrap();
        let ReplenishedOrder::Iceberg(first) = first else {
            panic!("expected iceberg")
        };
        assert_eq!(first.quantity(), 3);
        assert_eq!(first.typed_order_details.hidden_quantity, 5);

        let resting: RestingOrder<CompressedPrice> = first.into();
        assert!(matches!(
            resting.typed_order_details.replenishment_behavior,
            ReplenishmentBehavior::Iceberg {
                hidden_quantity: 5,
                peak_quantity: 3
            }
        ));
    }

    #[test]
    fn iceberg_with_no_hidden_quantity_does_not_replenish() {
        let order =
            IcebergOrder::<CompressedPrice>::new(Some(p(103)), Uuid::new_v4(), Side::Buy, 0, 3);
        assert!(order.replenish_into().is_none());
    }

    #[test]
    fn iceberg_converts_directly_to_replenished_variant() {
        let order =
            IcebergOrder::<CompressedPrice>::new(Some(p(100)), Uuid::new_v4(), Side::Sell, 4, 2);
        let replenished: ReplenishedOrder<CompressedPrice> = order.into();
        assert!(matches!(replenished, ReplenishedOrder::Iceberg(_)));
    }

    trait TraderId {
        fn trader_id(&self) -> Uuid;
    }

    impl<T, P: PriceType> TraderId for Order<T, P> {
        fn trader_id(&self) -> Uuid {
            self.common_data.trader
        }
    }

    #[cfg(feature = "python")]
    #[test]
    fn python_order_value_conversions_preserve_constructor_fields() {
        let trader = Uuid::new_v4();
        let market = PyMarketOrder {
            quantity: 7,
            trader,
            side: Side::Buy,
        };
        let rust_market: MarketOrder<CompressedPrice> = (&market).into();
        assert_eq!(rust_market.quantity(), 7);
        assert_eq!(rust_market.trader_id(), trader);

        let limit = PyLimitOrder {
            price: p(99),
            quantity: 4,
            trader,
            side: Side::Sell,
        };
        let rust_limit: LimitOrder<CompressedPrice> = limit.into();
        assert_eq!(rust_limit.price(), Some(p(99)));
        assert_eq!(rust_limit.quantity(), 4);

        let iceberg = PyIcebergOrder {
            price: p(102),
            quantity: 3,
            trader,
            side: Side::Buy,
            hidden_quantity: 11,
        };
        let rust_iceberg: IcebergOrder<CompressedPrice> = iceberg.into();
        assert_eq!(rust_iceberg.quantity(), 3);
        assert_eq!(rust_iceberg.typed_order_details.hidden_quantity, 11);
    }

    #[cfg(feature = "python")]
    #[test]
    fn python_order_display_fields_extend_the_base_order() {
        assert_eq!(
            PyMarketOrder::__display_fields__(),
            ["quantity", "trader", "side"]
        );
        assert_eq!(
            PyLimitOrder::__display_fields__(),
            ["price", "quantity", "trader", "side"]
        );
        assert_eq!(
            PyIcebergOrder::__display_fields__(),
            ["price", "quantity", "trader", "side", "hidden_quantity"]
        );

        let trader = Uuid::new_v4();
        let _market = PyMarketOrder::new(1, trader, Side::Buy);
        let _limit = PyLimitOrder::new(p(10), 1, trader, Side::Buy);
        let _iceberg = PyIcebergOrder::new(p(10), 1, trader, Side::Buy, 2);
    }
}
