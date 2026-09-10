//! ITCH adaptations for an isolated native replay branch.
use super::messages::{ItchAdd, ItchCancel, ItchDelete, ItchExecute, ItchReplace};
use crate::adapter::market::{AdaptToFeed, FeedMessage};
use lobo_models::orders::traits::Trades;
use lobo_primitives::{Price64, uuid::Uuid};
use lobo_replay::{feed::FeedBook, simulation::Simulation};
use lobo_storage::OrderStateError;

impl FeedMessage for ItchAdd {
    fn simulate(&self, branch: &mut Simulation, _: &FeedBook) -> Result<(), OrderStateError> {
        self.process_feed(branch.book_mut())
    }
}
impl FeedMessage for ItchExecute {
    fn simulate(&self, branch: &mut Simulation, source: &FeedBook) -> Result<(), OrderStateError> {
        // Wire identity remains valid in the main book even if that maker was
        // consumed in the scenario. Treat this record as taker flow in the fork.
        let order = source
            .order(Uuid::from_u128(self.reference as u128))
            .ok_or(OrderStateError::OrderDoesNotExists)?;
        let price = self
            .execution_price
            .map(Price64::from)
            .or(order.price())
            .ok_or(OrderStateError::OrderDoesNotExists)?;
        branch.execute(self.timestamp, order.side(), price, self.shares as u64);
        Ok(())
    }
}
/// Counterfactual fills can consume an order before its later cancel/delete.
/// Check only at this branch boundary, before calling native unchecked adapters.
fn quantity(branch: &mut Simulation, reference: u64) -> Option<u64> {
    let quantity = branch
        .book_mut()
        .order(Uuid::from_u128(reference as u128))
        .map(Trades::quantity);
    if quantity.is_none() {
        branch.ignored += 1;
    }
    quantity
}
impl FeedMessage for ItchCancel {
    fn simulate(&self, branch: &mut Simulation, _: &FeedBook) -> Result<(), OrderStateError> {
        if let Some(remaining) = quantity(branch, self.reference) {
            Self {
                shares: self.shares.min(remaining as u32),
                ..*self
            }
            .process_feed(branch.book_mut())?;
        }
        Ok(())
    }
}
impl FeedMessage for ItchDelete {
    fn simulate(&self, branch: &mut Simulation, _: &FeedBook) -> Result<(), OrderStateError> {
        if quantity(branch, self.reference).is_some() {
            self.process_feed(branch.book_mut())?;
        }
        Ok(())
    }
}
impl FeedMessage for ItchReplace {
    fn simulate(&self, branch: &mut Simulation, _: &FeedBook) -> Result<(), OrderStateError> {
        if quantity(branch, self.old_reference).is_some() {
            self.process_feed(branch.book_mut())?;
        }
        Ok(())
    }
}
