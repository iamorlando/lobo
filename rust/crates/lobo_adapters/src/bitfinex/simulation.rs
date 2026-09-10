//! Reconcile asynchronous R0 deltas with the public trade channel in a native
//! branch. This queue retains wire operations briefly, never another order book.
use super::messages::RawOrder;
use crate::adapter::{market::AdaptToFeed, messages::PublicTrade};
use lobo_models::orders::{order_types::LimitOrder, traits::Trades};
use lobo_primitives::{Price64, time::DateTime, uuid::Uuid};
use lobo_replay::simulation::Simulation;
use std::collections::VecDeque;

// R0 does not identify which reductions were executions. Give the trade channel
// a short opportunity to arrive first. Unmatched deltas become book corrections,
// never execution events. This is explicitly an estimated public-feed simulation.
const RECONCILE_NS: u64 = 250_000_000;
const MAX_PENDING: usize = 4096;
#[derive(Default)]
pub(super) struct LiveSimulation {
    branch: Option<Uuid>,
    pending: VecDeque<(RawOrder, Option<u64>)>,
}
impl LiveSimulation {
    fn prepare(&mut self, branch: &Simulation) {
        if self.branch != Some(branch.report.order_id) {
            self.pending.clear();
            self.branch = Some(branch.report.order_id);
        }
    }
    pub fn update(
        &mut self,
        branch: &mut Simulation,
        update: RawOrder,
        previous: Option<u64>,
    ) -> Result<(), String> {
        self.prepare(branch);
        if branch.stopped() {
            return Ok(());
        }
        self.flush(branch, update.timestamp)?;
        if self.pending.len() >= MAX_PENDING {
            branch.interrupt("Simulation fell behind the live feed; return to the main timeline");
            self.pending.clear();
            return Ok(());
        }
        self.pending.push_back((update, previous));
        Ok(())
    }
    pub fn flush(&mut self, branch: &mut Simulation, timestamp: u64) -> Result<(), String> {
        self.prepare(branch);
        while self
            .pending
            .front()
            .is_some_and(|(u, _)| u.timestamp.saturating_add(RECONCILE_NS) <= timestamp)
        {
            if let Some((update, previous)) = self.pending.pop_front() {
                Self::apply(branch, update, previous)?;
            }
        }
        Ok(())
    }
    pub fn trade(
        &mut self,
        branch: &mut Simulation,
        trade: PublicTrade<Price64>,
    ) -> Result<(), String> {
        self.prepare(branch);
        if branch.stopped() || trade.timestamp < branch.report.started_ns {
            return Ok(());
        }
        self.flush(branch, trade.timestamp)?;
        // New liquidity observed before the trade participates in native FIFO.
        // Decreases wait until after matching, avoiding duplicate consumption
        // when the book delta precedes its trade notification on the socket.
        let mut reductions = Vec::new();
        while let Some((update, previous)) = self.pending.pop_front() {
            if update.price != 0 && previous.is_none_or(|q| update.quantity > q) {
                Self::apply(branch, update, previous)?;
            } else {
                reductions.push((update, previous));
            }
        }
        branch.execute(
            trade.timestamp,
            trade.maker_side,
            trade.price,
            trade.quantity,
        );
        for (update, previous) in reductions {
            Self::apply(branch, update, previous)?;
        }
        branch.feed.commit();
        Ok(())
    }
    fn apply(
        branch: &mut Simulation,
        mut update: RawOrder,
        previous: Option<u64>,
    ) -> Result<(), String> {
        if branch.stopped() {
            return Ok(());
        }
        let id = Uuid::from_u128(update.reference.into());
        let current = branch
            .book_mut()
            .order(id)
            .map(|order| (order.quantity(), order.price(), order.side()));
        let crosses_new_price = current.is_some_and(|(_, price, side)| {
            update.price != 0 && (price != Some(Price64::from(update.price)) || side != update.side)
        });
        match (current, previous) {
            (None, Some(_)) => {
                branch.ignored += 1;
                return Ok(());
            }
            (Some((current, _, _)), Some(previous)) if update.price != 0 => {
                // Native matching may already have consumed this maker. Absolute
                // exchange corrections cannot restore that consumed quantity.
                update.quantity = if update.quantity > previous {
                    current.saturating_add(update.quantity - previous)
                } else {
                    current.min(update.quantity)
                };
            }
            _ => {}
        }
        if update.price != 0 && (previous.is_none() || crosses_new_price) {
            if current.is_some() {
                lobo_replay::dispatch_feed_book!(branch.book_mut(), native, {
                    let (storage, mut publish) = native.storage_and_publisher_at(update.timestamp);
                    storage
                        .remove_unchecked(id, &mut publish)
                        .map_err(|e| format!("Bitfinex simulation amend: {e:?}"))?;
                });
            }
            let mut order = LimitOrder::new(
                Some(Price64::from(update.price)),
                update.quantity,
                Uuid::nil(),
                update.side,
            )
            .with_creation_time(DateTime::from_timestamp_nanos(update.timestamp as i64));
            order.common_data.uuid = id;
            branch.submit(update.timestamp, order);
        } else {
            update
                .process_feed(branch.book_mut())
                .map_err(|e| format!("Bitfinex simulation: {e:?}"))?;
        }
        branch.feed.clock_ns = branch.feed.clock_ns.max(update.timestamp);
        Ok(())
    }
}
