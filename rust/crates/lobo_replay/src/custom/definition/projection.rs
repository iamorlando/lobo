//! Typed in-process boundary between a compiled mapping and book operations.
//! These records never pass through JSON or an IPC queue.
use lobo_models::Side;
use lobo_primitives::{Price64, uuid::Uuid};

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct OrderRecord {
    pub timestamp: u64,
    pub id: Uuid,
    pub new_id: Uuid,
    pub side: Side,
    pub price: u64,
    pub has_price: u64,
    pub quantity: u64,
    pub trade_id: u64,
    pub has_trade_id: u64,
    pub price_width: u64,
    pub quantity_width: u64,
    pub trader: Uuid,
    pub hidden_quantity: u64,
    pub peak_quantity: u64,
    pub has_new_id: u64,
}
impl Default for OrderRecord {
    fn default() -> Self {
        Self {
            timestamp: 0,
            id: Uuid::nil(),
            new_id: Uuid::nil(),
            side: Side::Buy,
            price: 0,
            has_price: 0,
            quantity: 0,
            trade_id: 0,
            has_trade_id: 0,
            price_width: 0,
            quantity_width: 0,
            trader: Uuid::nil(),
            hidden_quantity: 0,
            peak_quantity: 0,
            has_new_id: 0,
        }
    }
}
impl OrderRecord {
    pub fn price(&self) -> Option<Price64> {
        (self.has_price != 0).then(|| Price64::from(self.price))
    }
}
