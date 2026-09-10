use std::fmt::{self, Display, Formatter};

use lobo_primitives::{
    Notional, PriceType,
    time::{DateTime, Utc},
    uuid::Uuid,
};

use crate::{
    Side,
    orders::{
        core::{Order, RestingOrder},
        traits::{IntoRestingOrderData, Trades},
    },
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reports {
    pub include_fills: bool,
    pub include_summary: bool,
    pub include_market_impact: bool,
}

impl Reports {
    pub const fn any(self) -> bool {
        self.include_fills || self.include_summary || self.include_market_impact
    }
}

/// Fill data captured only when at least one report was requested.
#[derive(PartialEq, Eq, Default)]
pub struct MatchResult<P: PriceType> {
    pub fills: Vec<Fill<P>>,
}

/// Minimal matching output needed to commit a request correctly.
#[derive(PartialEq, Eq, Default)]
pub struct ExecutionResult<P: PriceType> {
    pub match_result: Option<MatchResult<P>>,
    pub remaining_order: Option<RestingOrder<P>>,
    pub remaining_order_id: Option<Uuid>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Fill<P: PriceType> {
    pub maker_order_id: Uuid,
    pub taker_order_id: Uuid,
    pub fill_quantity: u64,
    pub maker_depleted: bool,
    pub maker_trader_uid: Uuid,
    pub price: P,
    pub fill_time: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary<P: PriceType> {
    pub order_id: Uuid,
    pub trader_id: Uuid,
    pub side: Side,
    pub filled_quantity: u64,
    pub realized_price: P,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarketImpact<P: PriceType> {
    /// Average execution price across all fills (in price units).
    pub avg_price: f64,
    /// Worst (furthest from best price) execution price (in price units).
    pub worst_price: P,
    /// Absolute slippage from best price (in price units).
    pub slippage: P,
    /// Slippage in basis points.
    pub slippage_bps: f64,
    /// Number of price levels that would be consumed.
    pub levels_consumed: usize,
    /// Total resting depth available on the side being hit (in units),
    /// including visible and hidden iceberg quantity at every non-empty level.
    pub total_quantity_available: u64,
}

impl<P: PriceType> MarketImpact<P> {
    pub fn can_fill(&self, requested_quantity: u64) -> bool {
        self.total_quantity_available >= requested_quantity
    }

    pub fn fill_ratio(&self, requested_quantity: u64) -> f64 {
        if requested_quantity == 0 {
            1.0
        } else {
            (self.total_quantity_available as f64 / requested_quantity as f64).min(1.0)
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Report<P: PriceType> {
    pub fills: Option<Vec<Fill<P>>>,
    pub summary: Option<Summary<P>>,
    pub market_impact: Option<MarketImpact<P>>,
}

#[derive(Clone, PartialEq)]
pub struct FillResult<P: PriceType> {
    pub remaining_order_id: Option<Uuid>,
    /// Uncommitted remainder, present when the execution policy does not rest it.
    pub remaining_order: Option<RestingOrder<P>>,
    pub report: Option<Report<P>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketImpactContext<P: PriceType> {
    pub best_price: Option<P>,
    pub total_quantity_available: u64,
}

impl<P: PriceType> Fill<P> {
    pub fn new(
        maker_order_id: Uuid,
        taker_order_id: Uuid,
        maker_depleted: bool,
        maker_trader_uid: Uuid,
        fill_quantity: u64,
        price: P,
    ) -> Self {
        Self {
            maker_order_id,
            taker_order_id,
            maker_depleted,
            maker_trader_uid,
            fill_quantity,
            price,
            fill_time: Utc::now(),
        }
    }
}

impl<P: PriceType> MatchResult<P> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_fill(&mut self, fill: Fill<P>) {
        self.fills.push(fill);
    }
}

impl<P: PriceType> ExecutionResult<P> {
    pub fn new(include_match_result: bool) -> Self {
        Self {
            match_result: include_match_result.then(MatchResult::new),
            remaining_order: None,
            remaining_order_id: None,
        }
    }

    pub fn add_remaining_order<T>(&mut self, order: Order<T, P>)
    where
        T: IntoRestingOrderData,
    {
        if order.quantity() == 0 {
            return;
        }

        if self.remaining_order.is_some() {
            panic!("Execution result already stored the remaining order")
        }

        self.remaining_order_id = Some(order.uuid());
        self.remaining_order = Some(order.into_resting());
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_fill(
        &mut self,
        maker_order_id: Uuid,
        taker_order_id: Uuid,
        maker_depleted: bool,
        maker_trader_uid: Uuid,
        fill_quantity: u64,
        price: P,
    ) {
        if let Some(match_result) = &mut self.match_result {
            match_result.add_fill(Fill::new(
                maker_order_id,
                taker_order_id,
                maker_depleted,
                maker_trader_uid,
                fill_quantity,
                price,
            ));
        }
    }
}

pub fn summary<P: PriceType>(
    match_result: &MatchResult<P>,
    order_id: Uuid,
    trader_id: Uuid,
    side: Side,
) -> Summary<P> {
    let mut filled_quantity = 0_u64;
    let mut fill_notional: Notional = 0;

    for fill in &match_result.fills {
        filled_quantity = filled_quantity
            .checked_add(fill.fill_quantity)
            .expect("total fill quantity overflowed u64");
        fill_notional = fill_notional
            .checked_add(
                fill.price
                    .into_u128()
                    .checked_mul(Notional::from(fill.fill_quantity))
                    .expect("fill notional overflowed Notional"),
            )
            .expect("total fill notional overflowed Notional");
    }

    Summary {
        order_id,
        trader_id,
        side,
        filled_quantity,
        realized_price: if filled_quantity == 0 {
            P::MIN
        } else {
            P::try_from(fill_notional / Notional::from(filled_quantity))
                .ok()
                .expect("average fill price exceeded the selected representation")
        },
    }
}

pub fn market_impact<P: PriceType>(
    match_result: &MatchResult<P>,
    side: Side,
    context: MarketImpactContext<P>,
) -> MarketImpact<P> {
    if match_result.fills.is_empty() {
        return MarketImpact {
            avg_price: 0.0,
            worst_price: P::MIN,
            slippage: P::MIN,
            slippage_bps: 0.0,
            levels_consumed: 0,
            total_quantity_available: context.total_quantity_available,
        };
    }

    let mut filled_quantity = 0_u64;
    let mut fill_notional = 0.0_f64;
    let mut levels_consumed = 0_usize;
    let mut previous_price = None;
    let mut worst_price = match side {
        Side::Buy => P::MIN,
        Side::Sell => P::MAX,
    };

    for fill in &match_result.fills {
        filled_quantity = filled_quantity
            .checked_add(fill.fill_quantity)
            .expect("total fill quantity overflowed u64");
        fill_notional += fill.price.into_u128() as f64 * fill.fill_quantity as f64;

        if previous_price != Some(fill.price) {
            levels_consumed += 1;
            previous_price = Some(fill.price);
        }

        worst_price = match side {
            Side::Buy => worst_price.max(fill.price),
            Side::Sell => worst_price.min(fill.price),
        };
    }

    let best_price = context.best_price.unwrap_or(worst_price);
    let slippage = worst_price.abs_diff(best_price);

    MarketImpact {
        avg_price: fill_notional / filled_quantity as f64,
        worst_price,
        slippage,
        slippage_bps: if best_price == P::MIN {
            0.0
        } else {
            slippage.into_u128() as f64 / best_price.into_u128() as f64 * 10_000.0
        },
        levels_consumed,
        total_quantity_available: context.total_quantity_available,
    }
}

pub fn build_report<P: PriceType>(
    reports: Reports,
    match_result: MatchResult<P>,
    order_id: Uuid,
    trader_id: Uuid,
    side: Side,
    market_context: Option<MarketImpactContext<P>>,
) -> Report<P> {
    let summary = reports
        .include_summary
        .then(|| summary(&match_result, order_id, trader_id, side));
    let market_impact = reports.include_market_impact.then(|| {
        market_impact(
            &match_result,
            side,
            market_context.expect("market-impact context must be captured before execution"),
        )
    });
    let fills = reports.include_fills.then_some(match_result.fills);

    Report {
        fills,
        summary,
        market_impact,
    }
}

impl<P: PriceType> Display for Fill<P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} units @ {} | buy={} sell={} | {} | {} | {}",
            self.fill_quantity,
            self.price,
            self.maker_order_id,
            self.taker_order_id,
            self.maker_depleted,
            self.maker_trader_uid,
            self.fill_time,
        )
    }
}

impl<P: PriceType> Display for MatchResult<P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "Match result")?;

        if self.fills.is_empty() {
            return write!(f, "  Fills: none");
        }

        writeln!(f, "  Fills: {}", self.fills.len())?;
        for (index, fill) in self.fills.iter().enumerate() {
            writeln!(f, "    {}. {}", index + 1, fill)?;
        }

        Ok(())
    }
}
#[derive(Clone)]
pub struct OrderDetails<P: PriceType> {
    pub uuid: Option<Uuid>,
    pub price: Option<P>,
    pub creation_time: Option<DateTime<Utc>>,
    pub quantity: Option<u64>,
    pub trader: Option<Uuid>,
    pub side: Option<Side>,
}
pub type OrderTransition<P> = fn(&mut RestingOrder<P>, OrderDetails<P>) -> ();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModificationPriority {
    Preserve,
    Lose,
}

#[derive(Clone)]
pub enum ReplayEvent<P: PriceType> {
    Add {
        // timestamp: DateTime<Utc>,
        order: RestingOrder<P>,
    },

    Delete {
        // timestamp: DateTime<Utc>,
        order_id: Uuid,
    },
    Modify {
        old_order_id: Uuid,
        order_data: OrderDetails<P>,
        transition: OrderTransition<P>,
        priority: ModificationPriority,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::order_types::LimitOrder;
    use lobo_primitives::CompressedPrice;

    fn p(raw: u32) -> CompressedPrice {
        CompressedPrice::from(raw)
    }

    fn fill(
        maker_order_id: Uuid,
        taker_order_id: Uuid,
        quantity: u64,
        price: CompressedPrice,
        depleted: bool,
    ) -> Fill<CompressedPrice> {
        Fill::new(
            maker_order_id,
            taker_order_id,
            depleted,
            Uuid::new_v4(),
            quantity,
            price,
        )
    }

    #[test]
    fn reports_detect_requested_output() {
        assert!(!Reports::default().any());
        assert!(
            Reports {
                include_fills: true,
                ..Reports::default()
            }
            .any()
        );
    }

    #[test]
    fn execution_result_skips_match_result_without_reports() {
        let mut result = ExecutionResult::new(false);
        result.add_fill(
            Uuid::new_v4(),
            Uuid::new_v4(),
            true,
            Uuid::new_v4(),
            2,
            p(100),
        );
        assert!(result.match_result.is_none());

        let order = LimitOrder::<CompressedPrice>::new(Some(p(99)), 4, Uuid::new_v4(), Side::Buy);
        let id = order.uuid();
        result.add_remaining_order(order);
        assert_eq!(result.remaining_order_id, Some(id));
        assert_eq!(result.remaining_order.as_ref().unwrap().quantity(), 4);
    }

    #[test]
    #[should_panic(expected = "already stored the remaining order")]
    fn adding_a_second_remaining_order_panics() {
        let mut result = ExecutionResult::<CompressedPrice>::new(false);
        result.add_remaining_order(LimitOrder::<CompressedPrice>::new(
            Some(p(99)),
            4,
            Uuid::new_v4(),
            Side::Buy,
        ));
        result.add_remaining_order(LimitOrder::<CompressedPrice>::new(
            Some(p(98)),
            2,
            Uuid::new_v4(),
            Side::Buy,
        ));
    }

    #[test]
    fn summary_matches_previous_aggregate_output() {
        let order_id = Uuid::new_v4();
        let trader_id = Uuid::new_v4();
        let result = MatchResult {
            fills: vec![
                fill(Uuid::new_v4(), order_id, 2, p(100), true),
                fill(Uuid::new_v4(), order_id, 3, p(110), true),
            ],
        };

        assert_eq!(
            summary(&result, order_id, trader_id, Side::Buy),
            Summary {
                order_id,
                trader_id,
                side: Side::Buy,
                filled_quantity: 5,
                realized_price: p(106),
            }
        );
    }

    #[test]
    fn market_impact_uses_best_worst_and_all_available_depth() {
        let taker = Uuid::new_v4();
        let result = MatchResult {
            fills: vec![
                fill(Uuid::new_v4(), taker, 2, p(100), true),
                fill(Uuid::new_v4(), taker, 1, p(100), true),
                fill(Uuid::new_v4(), taker, 2, p(102), true),
            ],
        };
        let impact = market_impact(
            &result,
            Side::Buy,
            MarketImpactContext {
                best_price: Some(p(100)),
                total_quantity_available: 12,
            },
        );

        assert_eq!(impact.avg_price, 100.8);
        assert_eq!(impact.worst_price, p(102));
        assert_eq!(impact.slippage, p(2));
        assert_eq!(impact.slippage_bps, 200.0);
        assert_eq!(impact.levels_consumed, 2);
        assert_eq!(impact.total_quantity_available, 12);
        assert!(impact.can_fill(12));
        assert!(!impact.can_fill(13));
        assert_eq!(impact.fill_ratio(24), 0.5);
    }

    #[test]
    fn report_building_calculates_only_requested_outputs() {
        let order_id = Uuid::new_v4();
        let report = build_report(
            Reports {
                include_fills: true,
                include_summary: false,
                include_market_impact: true,
            },
            MatchResult {
                fills: vec![fill(Uuid::new_v4(), order_id, 4, p(101), true)],
            },
            order_id,
            Uuid::new_v4(),
            Side::Sell,
            Some(MarketImpactContext {
                best_price: Some(p(101)),
                total_quantity_available: 9,
            }),
        );

        assert_eq!(report.fills.as_ref().unwrap().len(), 1);
        assert!(report.summary.is_none());
        assert_eq!(report.market_impact.unwrap().total_quantity_available, 9);
    }

    #[test]
    fn display_covers_empty_and_filled_results() {
        assert!(
            MatchResult::<CompressedPrice>::new()
                .to_string()
                .contains("Fills: none")
        );
        let displayed = MatchResult {
            fills: vec![fill(Uuid::new_v4(), Uuid::new_v4(), 6, p(101), true)],
        }
        .to_string();
        assert!(displayed.contains("Fills: 1"));
        assert!(displayed.contains("6 units @ 101"));
    }
}
