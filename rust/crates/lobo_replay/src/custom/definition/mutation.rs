//! Validated L3 transitions shared by compiled and expression-based mappings.
use super::super::{FeedMessage, orders::*};
use crate::feed::FeedState;
use lobo_models::orders::traits::Trades;
use lobo_primitives::Price64;
use serde_json::{Value, json};

pub enum Mutation {
    Add(AddOrder<Price64>),
    Execute(ExecuteOrder<Price64>),
    Cancel(CancelOrder),
    Remove(RemoveOrder),
    Replace(ReplaceOrder<Price64>),
}
impl Mutation {
    pub fn apply(&self, state: &mut FeedState, symbol: &str) -> Result<(), String> {
        let result = match *self {
            Self::Add(m) => {
                if m.quantity == 0 || u64::from(m.price) == 0 {
                    return Err("Add requires positive quantity and price".into());
                }
                if state.book(symbol).is_some_and(|b| b.order(m.id).is_some()) {
                    return Err("Duplicate order ID".into());
                }
                m.route(state, symbol)
            }
            Self::Execute(m) => {
                let order = state
                    .book(symbol)
                    .and_then(|b| b.order(m.id))
                    .ok_or("Execution refers to missing order")?;
                if m.quantity == 0 || m.quantity > order.quantity() {
                    return Err("Invalid execution quantity".into());
                }
                m.route(state, symbol)
            }
            Self::Cancel(m) => {
                let order = state
                    .book(symbol)
                    .and_then(|b| b.order(m.id))
                    .ok_or("Cancel refers to missing order")?;
                if m.quantity == 0 || m.quantity > order.quantity() {
                    return Err("Invalid cancellation quantity".into());
                }
                m.route(state, symbol)
            }
            Self::Remove(m) => {
                if state.book(symbol).and_then(|b| b.order(m.id)).is_none() {
                    return Err("Remove refers to missing order".into());
                }
                m.route(state, symbol)
            }
            Self::Replace(m) => {
                let book = state.book(symbol).ok_or("Replace before book")?;
                if book.order(m.id).is_none()
                    || (m.id != m.new_id && book.order(m.new_id).is_some())
                    || m.quantity == 0
                    || u64::from(m.price) == 0
                {
                    return Err("Invalid replacement".into());
                }
                m.route(state, symbol)
            }
        };
        result.map_err(|e| format!("Order operation failed: {e:?}"))
    }
    pub fn event(&self) -> Value {
        use super::super::observer::literal as v;
        match self {
            Self::Add(m) => {
                json!({"action":"add","id":v(m.id),"side":v(m.side),"price":v(u64::from(m.price)),"quantity":v(m.quantity),"timestamp":v(m.timestamp)})
            }
            Self::Execute(m) => {
                json!({"action":"execute","id":v(m.id),"price":v(m.price.map(u64::from)),"quantity":v(m.quantity),"timestamp":v(m.timestamp)})
            }
            Self::Cancel(m) => {
                json!({"action":"cancel","id":v(m.id),"quantity":v(m.quantity),"timestamp":v(m.timestamp)})
            }
            Self::Remove(m) => json!({"action":"remove","id":v(m.id),"timestamp":v(m.timestamp)}),
            Self::Replace(m) => {
                json!({"action":"replace","id":v(m.id),"new_id":v(m.new_id),"price":v(u64::from(m.price)),"quantity":v(m.quantity),"timestamp":v(m.timestamp)})
            }
        }
    }
}
