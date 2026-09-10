//! Decode once at the transport boundary, then use the mutation API.
use lobo_books::price_time_priority::{Book, CommandResult};
use lobo_events::BookPublisherFactory;
use lobo_models::{
    Side,
    events::{Fill, MatchResult, OrderDetails, Reports, build_report},
    orders::{
        core::{ReplenishedOrder, ReplenishmentBehavior, RestingOrder},
        traits::{Replenishes, Trades},
    },
    server::{Command, Order},
};
use lobo_primitives::{PriceType, time::DateTime};
use lobo_storage::{
    ExecutionPolicy, MutatingFills, SimulatedFills, UserMapUpdatePolicy,
    policies::HiddenQuantityPolicy, price_level::PriceLevelContract,
    price_sorting::PriceSortingPolicy,
};

#[derive(Debug, PartialEq, Eq)]
pub enum CommandError {
    Invalid(&'static str),
    MissingOrder,
    DuplicateOrder,
}
impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::MissingOrder => f.write_str("order does not exist"),
            Self::DuplicateOrder => f.write_str("order already exists"),
        }
    }
}
impl std::error::Error for CommandError {}
impl From<lobo_storage::OrderStateError> for CommandError {
    fn from(error: lobo_storage::OrderStateError) -> Self {
        match error {
            lobo_storage::OrderStateError::OrderAlreadyExists => Self::DuplicateOrder,
            lobo_storage::OrderStateError::OrderDoesNotExists => Self::MissingOrder,
        }
    }
}

/// Command consumers share adaptation, validation and result handling. Matching
/// policies and publishers remain concrete types inside the hot loops.
pub trait ApplyOrderCommand<L, S, U, H, Pub>
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: BookPublisherFactory<L::Price>,
{
    fn apply(
        &self,
        book: &mut Book<L, S, U, H, Pub>,
        timestamp_ns: u64,
        reports: Reports,
    ) -> Result<CommandResult<L::Price>, CommandError>;
}
fn details<P: PriceType>(quantity: u64) -> OrderDetails<P> {
    OrderDetails {
        quantity: Some(quantity),
        price: None,
        uuid: None,
        trader: None,
        side: None,
        creation_time: None,
    }
}
fn transition<P: PriceType>(order: &mut RestingOrder<P>, details: OrderDetails<P>) {
    order.update_in_place(details);
}
fn fill<L, S, U, H, Pub, E>(
    book: &mut Book<L, S, U, H, Pub>,
    order: &Order,
    timestamp: u64,
    reports: Reports,
    execution: E,
) -> CommandResult<L::Price>
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: BookPublisherFactory<L::Price>,
    E: ExecutionPolicy,
{
    let (storage, mut publish) = book.storage_and_publisher_at(timestamp);
    let result = match order {
        Order::Market(order) => storage.submit_order_with_reports(
            order.native::<L::Price>(timestamp),
            execution,
            reports,
            &mut publish,
        ),
        Order::Limit(order) => storage.submit_order_with_reports(
            order.native::<L::Price>(timestamp),
            execution,
            reports,
            &mut publish,
        ),
        Order::Iceberg(order) => storage.submit_order_with_reports(
            order.native::<L::Price>(timestamp),
            execution,
            reports,
            &mut publish,
        ),
    };
    CommandResult::Filled {
        remaining_order_id: result.remaining_order_id,
        report: result.report,
    }
}
impl<L, S, U, H, Pub> ApplyOrderCommand<L, S, U, H, Pub> for Command
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: BookPublisherFactory<L::Price>,
{
    fn apply(
        &self,
        book: &mut Book<L, S, U, H, Pub>,
        timestamp: u64,
        reports: Reports,
    ) -> Result<CommandResult<L::Price>, CommandError> {
        // All untrusted identity/quantity checks happen here, before storage's
        // unchecked mutation entry points. Failed requests leave the book intact.
        match self {
            Self::Add { order } | Self::Fill { order } | Self::Simulate { order } => {
                order.validate().map_err(CommandError::Invalid)?;
                if book.order_storage.order(order.fields().id).is_some() {
                    return Err(CommandError::DuplicateOrder);
                }
                if !self.simulated() {
                    let (visible, hidden) = match order.fields().side {
                        Side::Buy => (
                            book.order_storage.bids.visible_quantity,
                            book.order_storage.bids.hidden_quantity,
                        ),
                        Side::Sell => (
                            book.order_storage.asks.visible_quantity,
                            book.order_storage.asks.hidden_quantity,
                        ),
                    };
                    visible
                        .checked_add(hidden)
                        .and_then(|total| {
                            total.checked_add(order.requested_quantity_with(H::included_quantity)?)
                        })
                        .ok_or(CommandError::Invalid(
                            "book-side quantity would overflow u64",
                        ))?;
                }
                match self {
                    Self::Add { .. } => {
                        let resting = order.resting(timestamp).map_err(CommandError::Invalid)?;
                        let (storage, mut publish) = book.storage_and_publisher_at(timestamp);
                        Ok(CommandResult::Added {
                            order_id: storage.add_order(resting, &mut publish)?,
                        })
                    }
                    Self::Simulate { .. } => {
                        Ok(fill(book, order, timestamp, reports, SimulatedFills))
                    }
                    _ => Ok(fill(book, order, timestamp, reports, MutatingFills)),
                }
            }
            Self::Execute { id, quantity, .. } | Self::Cancel { id, quantity } => {
                let current = book
                    .order_storage
                    .order(*id)
                    .ok_or(CommandError::MissingOrder)?;
                if *quantity == 0 {
                    return Err(CommandError::Invalid("quantity must be positive"));
                }
                let remaining =
                    current
                        .quantity()
                        .checked_sub(*quantity)
                        .ok_or(CommandError::Invalid(
                            "reduction exceeds visible resting quantity",
                        ))?;
                // Replenishment is the order's policy. Only a depleted
                // iceberg needs a replacement; ordinary reductions stay in place.
                let replenished = if remaining == 0 {
                    current.clone().replenish_into()
                } else {
                    None
                };
                let execution = match self {
                    Self::Execute { price, .. } => {
                        if *price == Some(0) {
                            return Err(CommandError::Invalid("execution price must be positive"));
                        }
                        reports.any().then(|| {
                            let fill = Fill {
                                maker_order_id: *id,
                                taker_order_id: lobo_primitives::uuid::Uuid::nil(),
                                fill_quantity: *quantity,
                                maker_depleted: remaining == 0 && replenished.is_none(),
                                maker_trader_uid: current.common_data.trader,
                                price: price
                                    .map(L::Price::from)
                                    .or(current.price())
                                    .expect("resting price"),
                                fill_time: DateTime::from_timestamp_nanos(timestamp as i64),
                            };
                            build_report(
                                reports,
                                MatchResult { fills: vec![fill] },
                                *id,
                                current.common_data.trader,
                                current.side(),
                                None,
                            )
                        })
                    }
                    _ => None,
                };
                let (storage, mut publish) = book.storage_and_publisher_at(timestamp);
                match self {
                    Self::Execute { price, .. } => {
                        storage.modify_with_fill_event(
                            *id,
                            details(remaining),
                            transition,
                            |resting| price.map_or(resting, L::Price::from),
                            &mut publish,
                        )?;
                    }
                    _ => {
                        storage.modify_order_in_place(
                            *id,
                            details(remaining),
                            transition,
                            &mut publish,
                        )?;
                    }
                }
                if let Some(ReplenishedOrder::Iceberg(order)) = replenished {
                    if storage.order(*id).is_some() {
                        storage.remove_order(*id, &mut publish)?;
                    }
                    storage.add_order(order.into_resting(), &mut publish)?;
                }
                if matches!(self, Self::Execute { .. }) {
                    Ok(CommandResult::Filled {
                        remaining_order_id: storage.order(*id).map(|_| *id),
                        report: execution,
                    })
                } else {
                    Ok(CommandResult::Canceled { order_id: *id })
                }
            }
            Self::Remove { id } => {
                let (storage, mut publish) = book.storage_and_publisher_at(timestamp);
                Ok(CommandResult::Canceled {
                    order_id: storage.remove_order(*id, &mut publish)?,
                })
            }
            Self::Modify {
                id,
                quantity,
                price,
                new_id,
            } => {
                let current = book
                    .order_storage
                    .order(*id)
                    .ok_or(CommandError::MissingOrder)?;
                if *quantity == 0 || *price == Some(0) {
                    return Err(CommandError::Invalid(
                        "modify requires positive quantity and price; use remove to delete",
                    ));
                }
                if let ReplenishmentBehavior::Iceberg { peak_quantity, .. } =
                    current.typed_order_details.replenishment_behavior
                {
                    if *quantity > peak_quantity {
                        return Err(CommandError::Invalid(
                            "iceberg visible quantity cannot exceed its peak",
                        ));
                    }
                }
                let (visible, hidden) = match current.side() {
                    Side::Buy => (
                        book.order_storage.bids.visible_quantity,
                        book.order_storage.bids.hidden_quantity,
                    ),
                    Side::Sell => (
                        book.order_storage.asks.visible_quantity,
                        book.order_storage.asks.hidden_quantity,
                    ),
                };
                visible
                    .checked_sub(current.quantity())
                    .and_then(|v| v.checked_add(hidden))
                    .and_then(|v| v.checked_add(*quantity))
                    .ok_or(CommandError::Invalid(
                        "book-side quantity would overflow u64",
                    ))?;
                if new_id.is_some_and(|new| new != *id && book.order_storage.order(new).is_some()) {
                    return Err(CommandError::DuplicateOrder);
                }
                let loses_priority = *quantity > current.quantity()
                    || price.is_some_and(|price| Some(L::Price::from(price)) != current.price())
                    || new_id.is_some_and(|new| new != *id);
                let mut update = details(*quantity);
                update.price = price.map(L::Price::from);
                update.uuid = *new_id;
                let (storage, mut publish) = book.storage_and_publisher_at(timestamp);
                let order_id = if loses_priority {
                    update.creation_time = Some(DateTime::from_timestamp_nanos(timestamp as i64));
                    storage.replace_in_place(*id, update, transition, &mut publish)?
                } else {
                    storage.modify_order_in_place(*id, update, transition, &mut publish)?
                };
                Ok(CommandResult::Added { order_id })
            }
        }
    }
}

use crate::{feed::FeedBook, simulation::Simulation};
use lobo_models::server::RestingOrderSnapshot;
pub trait ApplyFeedCommand {
    fn simulate_feed(
        &self,
        timestamp: u64,
        branch: &mut Simulation,
        source: &FeedBook,
    ) -> Result<(), String>;
    /// Counterfactual failures stop that branch while the source keeps advancing.
    fn route_feed(
        &self,
        state: &mut crate::feed::FeedState,
        symbol: &str,
        timestamp: u64,
        reports: Reports,
    ) -> Result<CommandResult<lobo_primitives::Price64>, CommandError> {
        if let Some(branch) = &mut state.simulation {
            if branch.feed.selected == symbol {
                if let Some(source) = state.context.get(symbol) {
                    if let Err(error) = self.simulate_feed(timestamp, branch, source) {
                        branch.interrupt(&error);
                    }
                }
            }
        }
        self.apply_feed(state.book_mut(symbol), timestamp, reports)
    }
    fn apply_feed(
        &self,
        book: &mut FeedBook,
        timestamp: u64,
        reports: Reports,
    ) -> Result<
        lobo_books::price_time_priority::CommandResult<lobo_primitives::Price64>,
        CommandError,
    >;
}
impl ApplyFeedCommand for Command {
    fn simulate_feed(
        &self,
        timestamp: u64,
        branch: &mut Simulation,
        source: &FeedBook,
    ) -> Result<(), String> {
        simulate_command(self, timestamp, branch, source)
    }
    fn apply_feed(
        &self,
        book: &mut FeedBook,
        timestamp: u64,
        reports: Reports,
    ) -> Result<
        lobo_books::price_time_priority::CommandResult<lobo_primitives::Price64>,
        CommandError,
    > {
        crate::dispatch_feed_book!(book, native, self.apply(native, timestamp, reports))
    }
}
const FILL_REPORTS: Reports = Reports {
    include_fills: true,
    include_summary: false,
    include_market_impact: false,
};
pub fn simulate_command(
    command: &Command,
    timestamp: u64,
    branch: &mut Simulation,
    source: &FeedBook,
) -> Result<(), String> {
    if branch.stopped() {
        return Ok(());
    }
    if let Command::Execute {
        id,
        quantity,
        price,
    } = command
    {
        let maker = source
            .order(*id)
            .ok_or("execution references missing maker")?;
        let price = price
            .map(lobo_primitives::Price64::from)
            .or(maker.price())
            .ok_or("maker has no price")?;
        branch.execute(timestamp, maker.side(), price, *quantity);
        return Ok(());
    }
    let command = match command {
        Command::Add { order } => Command::Fill {
            order: order.clone(),
        },
        Command::Cancel { id, quantity } => {
            let Some(current) = branch.book_mut().order(*id).map(Trades::quantity) else {
                branch.ignored += 1;
                return Ok(());
            };
            Command::Cancel {
                id: *id,
                quantity: (*quantity).min(current),
            }
        }
        Command::Modify {
            id,
            quantity,
            price,
            new_id,
        } => {
            let Some(current) = branch.book_mut().order(*id) else {
                branch.ignored += 1;
                return Ok(());
            };
            let previous = source
                .order(*id)
                .ok_or("amendment references missing maker")?
                .quantity();
            // Preserve the difference produced by counterfactual executions.
            let adjusted = if *quantity > previous {
                current
                    .quantity()
                    .checked_add(*quantity - previous)
                    .ok_or("simulation amendment quantity overflow")?
            } else {
                current.quantity().saturating_sub(previous - *quantity)
            };
            if adjusted == 0 {
                Command::Remove { id: *id }
            } else if price.is_some_and(|p| current.price() != Some(p.into())) {
                let mut amended = current.clone();
                amended.update_in_place(OrderDetails {
                    quantity: Some(adjusted),
                    price: price.map(Into::into),
                    uuid: *new_id,
                    trader: None,
                    side: None,
                    creation_time: None,
                });
                let order = RestingOrderSnapshot::from_native(&amended)?.order;
                Command::Remove { id: *id }
                    .apply_feed(branch.book_mut(), timestamp, Reports::default())
                    .map_err(|e| e.to_string())?;
                // A price amendment can cross the counterfactual resting order.
                Command::Fill { order }
            } else {
                Command::Modify {
                    id: *id,
                    quantity: adjusted,
                    price: *price,
                    new_id: *new_id,
                }
            }
        }
        Command::Remove { id } => {
            if branch.book_mut().order(*id).is_none() {
                branch.ignored += 1;
                return Ok(());
            }
            command.clone()
        }
        _ => command.clone(),
    };
    let result = command
        .apply_feed(branch.book_mut(), timestamp, FILL_REPORTS)
        .map_err(|e| e.to_string())?;
    if let lobo_books::price_time_priority::CommandResult::Filled {
        report: Some(report),
        ..
    } = result
    {
        branch
            .report
            .record(timestamp, report.fills.unwrap_or_default());
    }
    branch.feed.clock_ns = branch.feed.clock_ns.max(timestamp);
    Ok(())
}
