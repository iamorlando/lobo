//! Transport schemas. Prices and quantities are integer atoms, like native orders.
//! Matching and mutation belong to books/storage; these types only describe inputs.
use crate::{
    Side,
    orders::{
        core::{OrderCore, ReplenishmentBehavior, RestingOrder},
        order_types::{
            IcebergOrder as NativeIceberg, LimitOrder as NativeLimit, MarketOrder as NativeMarket,
        },
    },
};
use lobo_primitives::{PriceType, time::DateTime, uuid::Uuid};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OrderFields {
    pub id: Uuid,
    #[serde(default)]
    pub trader: Uuid,
    pub side: Side,
    /// Visible quantity, in the book's integer quantity units.
    #[schemars(range(min = 1))]
    pub quantity: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MarketOrder {
    #[serde(flatten)]
    pub fields: OrderFields,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LimitOrder {
    #[serde(flatten)]
    pub fields: OrderFields,
    #[schemars(range(min = 1))]
    pub price: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct IcebergOrder {
    #[serde(flatten)]
    pub fields: OrderFields,
    #[schemars(range(min = 1))]
    pub price: u32,
    pub hidden_quantity: u64,
    #[schemars(range(min = 1))]
    pub peak_quantity: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Order {
    Market(MarketOrder),
    Limit(LimitOrder),
    Iceberg(IcebergOrder),
}
impl Order {
    pub fn fields(&self) -> &OrderFields {
        match self {
            Self::Market(o) => &o.fields,
            Self::Limit(o) => &o.fields,
            Self::Iceberg(o) => &o.fields,
        }
    }
    pub fn price(&self) -> Option<u32> {
        match self {
            Self::Market(_) => None,
            Self::Limit(o) => Some(o.price),
            Self::Iceberg(o) => Some(o.price),
        }
    }
    pub fn requested_quantity(&self) -> Option<u64> {
        self.requested_quantity_with(std::convert::identity)
    }
    pub fn requested_quantity_with(&self, include_hidden: impl FnOnce(u64) -> u64) -> Option<u64> {
        match self {
            Self::Iceberg(o) => o
                .fields
                .quantity
                .checked_add(include_hidden(o.hidden_quantity)),
            _ => Some(self.fields().quantity),
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.fields().quantity == 0 {
            return Err("quantity must be positive");
        }
        if self.price() == Some(0) {
            return Err("price must be positive");
        }
        if let Self::Iceberg(o) = self {
            if o.peak_quantity == 0 || o.fields.quantity > o.peak_quantity {
                return Err("iceberg requires 0 < quantity <= peak_quantity");
            }
        }
        self.requested_quantity()
            .ok_or("total quantity overflows u64")?;
        Ok(())
    }
    pub fn resting<P: PriceType>(
        &self,
        timestamp_ns: u64,
    ) -> Result<RestingOrder<P>, &'static str> {
        match self {
            Self::Market(_) => Err("market orders cannot rest; use fill or simulate"),
            Self::Limit(o) => Ok(o.native(timestamp_ns).into_resting()),
            Self::Iceberg(o) => Ok(o.native(timestamp_ns).into_resting()),
        }
    }
}
impl OrderFields {
    pub fn core<P: PriceType>(&self, price: Option<u32>, timestamp_ns: u64) -> OrderCore<P> {
        OrderCore {
            uuid: self.id,
            trader: self.trader,
            side: self.side,
            quantity: self.quantity,
            price: price.map(P::from),
            creation_time: DateTime::from_timestamp_nanos(timestamp_ns as i64),
        }
    }
    pub fn from_core<P: PriceType>(core: &OrderCore<P>) -> Self {
        Self {
            id: core.uuid,
            trader: core.trader,
            side: core.side,
            quantity: core.quantity,
        }
    }
}
impl MarketOrder {
    pub fn native<P: PriceType>(&self, timestamp_ns: u64) -> NativeMarket<P> {
        NativeMarket {
            common_data: self.fields.core(None, timestamp_ns),
            typed_order_details: crate::orders::order_types::MarketOrderData,
        }
    }
}
impl LimitOrder {
    pub fn native<P: PriceType>(&self, timestamp_ns: u64) -> NativeLimit<P> {
        NativeLimit {
            common_data: self.fields.core(Some(self.price), timestamp_ns),
            typed_order_details: crate::orders::order_types::LimitOrderData,
        }
    }
}
impl IcebergOrder {
    pub fn native<P: PriceType>(&self, timestamp_ns: u64) -> NativeIceberg<P> {
        NativeIceberg {
            common_data: self.fields.core(Some(self.price), timestamp_ns),
            typed_order_details: crate::orders::order_types::IcebergOrderData {
                hidden_quantity: self.hidden_quantity,
                peak_quantity: self.peak_quantity,
            },
        }
    }
}

/// A single decoded command goes directly to the book adapter.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Insert passive liquidity without matching. Market orders are rejected.
    Add {
        order: Order,
    },
    /// Reduce a named resting order and publish executed volume.
    Execute {
        id: Uuid,
        quantity: u64,
        #[serde(default)]
        price: Option<u32>,
    },
    /// Reduce a named resting order without publishing executed volume.
    Cancel {
        id: Uuid,
        quantity: u64,
    },
    Remove {
        id: Uuid,
    },
    /// Absolute visible quantity; price/identity changes or quantity increases lose FIFO priority.
    Modify {
        id: Uuid,
        quantity: u64,
        #[serde(default)]
        price: Option<u32>,
        #[serde(default)]
        new_id: Option<Uuid>,
    },
    /// Match through native fill and rest any eligible remainder.
    Fill {
        order: Order,
    },
    /// Nonmutating preview. Results are explicitly marked simulated.
    Simulate {
        order: Order,
    },
}
impl Command {
    pub fn order_id(&self) -> Uuid {
        match self {
            Self::Add { order } | Self::Fill { order } | Self::Simulate { order } => {
                order.fields().id
            }
            Self::Execute { id, .. }
            | Self::Cancel { id, .. }
            | Self::Remove { id }
            | Self::Modify { id, .. } => *id,
        }
    }
    pub fn simulated(&self) -> bool {
        matches!(self, Self::Simulate { .. })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BookInfo {
    pub symbol: String,
    pub price_decimals: u8,
    pub quantity_decimals: u8,
    #[serde(default)]
    pub policy: BookPolicy,
}
pub use crate::BookPolicy;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RestingOrderSnapshot {
    pub order: Order,
    pub timestamp_ns: u64,
}
impl RestingOrderSnapshot {
    pub fn from_native<P: PriceType>(order: &RestingOrder<P>) -> Result<Self, &'static str> {
        let fields = OrderFields::from_core(&order.common_data);
        let price = order
            .common_data
            .price
            .ok_or("resting order has no price")?
            .into_u128()
            .try_into()
            .map_err(|_| "price exceeds server representation")?;
        let data = match order.typed_order_details.replenishment_behavior {
            ReplenishmentBehavior::Remove => Order::Limit(LimitOrder { fields, price }),
            ReplenishmentBehavior::Iceberg {
                hidden_quantity,
                peak_quantity,
            } => Order::Iceberg(IcebergOrder {
                fields,
                price,
                hidden_quantity,
                peak_quantity,
            }),
        };
        Ok(Self {
            order: data,
            timestamp_ns: order
                .common_data
                .creation_time
                .timestamp_nanos_opt()
                .ok_or("invalid order timestamp")? as u64,
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FeedMessage {
    Directory {
        books: Vec<BookInfo>,
    },
    Snapshot {
        book: BookInfo,
        sequence: u64,
        timestamp_ns: u64,
        orders: Vec<RestingOrderSnapshot>,
    },
    Update {
        book: String,
        sequence: u64,
        timestamp_ns: u64,
        command: Command,
    },
    Heartbeat,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Execution {
    pub maker_id: Uuid,
    pub price: u64,
    pub quantity: u64,
    pub simulated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CommandResponse {
    pub book: String,
    pub sequence: u64,
    pub order_id: Uuid,
    pub simulated: bool,
    pub filled: u64,
    pub remaining: u64,
    pub resting_order_id: Option<Uuid>,
    pub average_price: Option<f64>,
    pub executions: Vec<Execution>,
}

/// A FIFO snapshot entry independent of a transport's order-entry price width.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RestingOrderState {
    pub id: Uuid,
    pub trader: Uuid,
    pub side: Side,
    pub price: u64,
    pub quantity: u64,
    pub created_at_ns: u64,
    pub hidden_quantity: u64,
    pub peak_quantity: u64,
}
impl RestingOrderState {
    pub fn from_order<P: PriceType>(order: &RestingOrder<P>) -> Result<Self, String> {
        let common = &order.common_data;
        let (hidden_quantity, peak_quantity) =
            match order.typed_order_details.replenishment_behavior {
                ReplenishmentBehavior::Remove => (0, 0),
                ReplenishmentBehavior::Iceberg {
                    hidden_quantity,
                    peak_quantity,
                } => (hidden_quantity, peak_quantity),
            };
        Ok(Self {
            id: common.uuid,
            trader: common.trader,
            side: common.side,
            price: u64::try_from(
                common
                    .price
                    .ok_or("Resting order has no price")?
                    .into_u128(),
            )
            .map_err(|_| "Snapshot price exceeds u64")?,
            quantity: common.quantity,
            created_at_ns: u64::try_from(
                common
                    .creation_time
                    .timestamp_nanos_opt()
                    .ok_or("Invalid order timestamp")?,
            )
            .map_err(|_| "Order timestamp is negative")?,
            hidden_quantity,
            peak_quantity,
        })
    }
    pub fn into_order<P: PriceType>(self) -> Result<RestingOrder<P>, String> {
        if self.price == 0
            || self.quantity == 0
            || (self.hidden_quantity > 0 && self.peak_quantity == 0)
        {
            return Err("Invalid resting order snapshot".into());
        }
        let price =
            P::try_from(self.price).map_err(|_| "Snapshot price exceeds book representation")?;
        let timestamp =
            i64::try_from(self.created_at_ns).map_err(|_| "Timestamp exceeds supported range")?;
        let mut order = RestingOrder::new_replay_order(OrderCore {
            uuid: self.id,
            trader: self.trader,
            side: self.side,
            price: Some(price),
            quantity: self.quantity,
            creation_time: DateTime::from_timestamp_nanos(timestamp),
        });
        if self.peak_quantity > 0 {
            order.typed_order_details.replenishment_behavior = ReplenishmentBehavior::Iceberg {
                hidden_quantity: self.hidden_quantity,
                peak_quantity: self.peak_quantity,
            };
        }
        Ok(order)
    }
}
