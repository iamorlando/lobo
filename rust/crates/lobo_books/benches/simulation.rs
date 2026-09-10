use std::{hint::black_box, time::Duration};

use criterion::{
    BatchSize, BenchmarkId, Criterion, SamplingMode, Throughput, criterion_group, criterion_main,
};
use lobo_books::price_time_priority::{Book, Command, CommandResult};
use lobo_models::{
    Side,
    events::Reports,
    orders::order_types::{
        IcebergOrder, IcebergOrderData, LimitOrder, LimitOrderData, MarketOrder, MarketOrderData,
    },
};
use lobo_primitives::{CompressedPrice, uuid::Uuid};
use lobo_storage::{MutatingFills, policies::UpdateHiddenQuantity, price_level::DeepPriceLevel};

const BENCHMARK_ROUNDS: usize = 5;
const POPULATION_ORDERS: usize = 10_000;
const SWEEP_ORDERS: usize = 2_000;
const REPORT_BENCH_FILLS: usize = 1_000;
const CANCEL_ALL_ORDERS: usize = 2_000;
const QUERY_USER_ORDERS: usize = 2_000;
const PRICE_LEVELS: usize = 100;
const BID_MAX_RAW: u32 = 10_099;
const ASK_MIN_RAW: u32 = 10_100;

type BenchmarkBook<P> = Book<DeepPriceLevel<P, UpdateHiddenQuantity>>;

const MAKER: Uuid = Uuid::from_u128(1);
const TAKER: Uuid = Uuid::from_u128(2);
const TARGET_USER: Uuid = Uuid::from_u128(3);
const MISSING_USER: Uuid = Uuid::from_u128(u128::MAX);

fn bid_max() -> CompressedPrice {
    CompressedPrice::from(BID_MAX_RAW)
}

fn ask_min() -> CompressedPrice {
    CompressedPrice::from(ASK_MIN_RAW)
}

#[derive(Clone, Copy)]
enum SimulatedOrder {
    Limit {
        uuid: Uuid,
        price: CompressedPrice,
        quantity: u64,
        trader: Uuid,
        side: Side,
    },
    Iceberg {
        uuid: Uuid,
        price: CompressedPrice,
        hidden_quantity: u64,
        peak_quantity: u64,
        trader: Uuid,
        side: Side,
    },
}

fn round_label(round: usize) -> String {
    format!("round_{round}")
}

fn order_uuid(namespace: u128, index: usize) -> Uuid {
    Uuid::from_u128(namespace * 1_000_000 + index as u128 + 1)
}

fn trader_uuid(index: usize) -> Uuid {
    Uuid::from_u128(10_000 + (index % 64) as u128)
}

fn limit_specs(order_count: usize) -> Vec<SimulatedOrder> {
    (0..order_count)
        .map(|index| {
            let (side, price) = if index % 2 == 0 {
                (
                    Side::Buy,
                    CompressedPrice::from(
                        BID_MAX_RAW - u32::try_from(index % PRICE_LEVELS).unwrap(),
                    ),
                )
            } else {
                (
                    Side::Sell,
                    CompressedPrice::from(
                        ASK_MIN_RAW + u32::try_from(index % PRICE_LEVELS).unwrap(),
                    ),
                )
            };
            SimulatedOrder::Limit {
                uuid: order_uuid(1, index),
                price,
                quantity: (index % 100 + 1) as u64,
                trader: trader_uuid(index),
                side,
            }
        })
        .collect()
}

fn mixed_specs(order_count: usize) -> Vec<SimulatedOrder> {
    (0..order_count)
        .map(|index| {
            let (side, price) = if index % 2 == 0 {
                (
                    Side::Buy,
                    CompressedPrice::from(
                        BID_MAX_RAW - u32::try_from(index % PRICE_LEVELS).unwrap(),
                    ),
                )
            } else {
                (
                    Side::Sell,
                    CompressedPrice::from(
                        ASK_MIN_RAW + u32::try_from(index % PRICE_LEVELS).unwrap(),
                    ),
                )
            };
            let trader = trader_uuid(index);
            let uuid = order_uuid(2, index);

            if index % 3 == 0 {
                let peak_quantity = (index % 25 + 1) as u64;
                SimulatedOrder::Iceberg {
                    uuid,
                    price,
                    hidden_quantity: peak_quantity * 3,
                    peak_quantity,
                    trader,
                    side,
                }
            } else {
                SimulatedOrder::Limit {
                    uuid,
                    price,
                    quantity: (index % 100 + 1) as u64,
                    trader,
                    side,
                }
            }
        })
        .collect()
}

fn populate_book(orders: &[SimulatedOrder]) -> BenchmarkBook<CompressedPrice> {
    let mut book = BenchmarkBook::<CompressedPrice>::default();

    for order in orders {
        let result = match *order {
            SimulatedOrder::Limit {
                uuid,
                price,
                quantity,
                trader,
                side,
            } => book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Fill {
                order: LimitOrder::new(Some(price), quantity, trader, side).with_uuid(uuid),
                execution: MutatingFills,
                reports: Reports::default(),
            }),
            SimulatedOrder::Iceberg {
                uuid,
                price,
                hidden_quantity,
                peak_quantity,
                trader,
                side,
            } => book.submit::<IcebergOrderData, IcebergOrderData, MutatingFills>(Command::Fill {
                order: IcebergOrder::new(Some(price), trader, side, hidden_quantity, peak_quantity)
                    .with_uuid(uuid),
                execution: MutatingFills,
                reports: Reports::default(),
            }),
        };
        assert!(result.is_ok(), "generated order submission failed");
    }

    book
}

fn add_limit(
    book: &mut BenchmarkBook<CompressedPrice>,
    uuid: Uuid,
    price: CompressedPrice,
    quantity: u64,
    trader: Uuid,
    side: Side,
) {
    let result = book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Add {
        order: LimitOrder::new(Some(price), quantity, trader, side).with_uuid(uuid),
    });
    assert!(result.is_ok(), "benchmark fixture insertion failed");
}

fn add_iceberg(
    book: &mut BenchmarkBook<CompressedPrice>,
    uuid: Uuid,
    price: CompressedPrice,
    hidden_quantity: u64,
    peak_quantity: u64,
) {
    let result = book.submit::<IcebergOrderData, IcebergOrderData, MutatingFills>(Command::Add {
        order: IcebergOrder::new(
            Some(price),
            MAKER,
            Side::Sell,
            hidden_quantity,
            peak_quantity,
        )
        .with_uuid(uuid),
    });
    assert!(result.is_ok(), "benchmark iceberg fixture insertion failed");
}

fn exact_cross_book() -> BenchmarkBook<CompressedPrice> {
    let mut book = BenchmarkBook::<CompressedPrice>::default();
    add_limit(
        &mut book,
        order_uuid(10, 0),
        ask_min(),
        100,
        MAKER,
        Side::Sell,
    );
    book
}

fn same_price_ask_book(order_count: usize) -> BenchmarkBook<CompressedPrice> {
    let mut book = BenchmarkBook::<CompressedPrice>::default();
    for index in 0..order_count {
        add_limit(
            &mut book,
            order_uuid(11, index),
            ask_min(),
            1,
            trader_uuid(index),
            Side::Sell,
        );
    }
    book
}

fn multi_level_ask_book(order_count: usize) -> BenchmarkBook<CompressedPrice> {
    let mut book = BenchmarkBook::<CompressedPrice>::default();
    for index in 0..order_count {
        add_limit(
            &mut book,
            order_uuid(12, index),
            CompressedPrice::from(ASK_MIN_RAW + u32::try_from(index % PRICE_LEVELS).unwrap()),
            1,
            trader_uuid(index),
            Side::Sell,
        );
    }
    book
}

fn iceberg_ask_book(order_count: usize) -> BenchmarkBook<CompressedPrice> {
    let mut book = BenchmarkBook::<CompressedPrice>::default();
    for index in 0..order_count {
        add_iceberg(&mut book, order_uuid(13, index), ask_min(), 90, 10);
    }
    book
}

fn one_order_book() -> (BenchmarkBook<CompressedPrice>, Uuid) {
    let mut book = BenchmarkBook::<CompressedPrice>::default();
    let order_id = order_uuid(14, 0);
    add_limit(&mut book, order_id, bid_max(), 10, MAKER, Side::Buy);
    (book, order_id)
}

fn one_user_book(order_count: usize) -> BenchmarkBook<CompressedPrice> {
    let mut book = BenchmarkBook::<CompressedPrice>::default();
    for index in 0..order_count {
        let (side, price) = if index % 2 == 0 {
            (
                Side::Buy,
                CompressedPrice::from(BID_MAX_RAW - u32::try_from(index % PRICE_LEVELS).unwrap()),
            )
        } else {
            (
                Side::Sell,
                CompressedPrice::from(ASK_MIN_RAW + u32::try_from(index % PRICE_LEVELS).unwrap()),
            )
        };
        add_limit(
            &mut book,
            order_uuid(15, index),
            price,
            (index % 50 + 1) as u64,
            TARGET_USER,
            side,
        );
    }
    book
}

fn limit_cross(book: &mut BenchmarkBook<CompressedPrice>) -> CommandResult<CompressedPrice> {
    book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Fill {
        order: LimitOrder::new(Some(ask_min()), 100, TAKER, Side::Buy),
        execution: MutatingFills,
        reports: Reports::default(),
    })
    .ok()
    .expect("limit cross failed")
}

fn market_cross(
    book: &mut BenchmarkBook<CompressedPrice>,
    quantity: u64,
) -> CommandResult<CompressedPrice> {
    market_cross_with_reports(book, quantity, Reports::default())
}

fn market_cross_with_reports(
    book: &mut BenchmarkBook<CompressedPrice>,
    quantity: u64,
    reports: Reports,
) -> CommandResult<CompressedPrice> {
    book.submit::<MarketOrderData, LimitOrderData, MutatingFills>(Command::Fill {
        order: MarketOrder::new(quantity, TAKER, Side::Buy),
        execution: MutatingFills,
        reports,
    })
    .ok()
    .expect("market cross failed")
}

fn bench_population(criterion: &mut Criterion) {
    let limit_orders = limit_specs(POPULATION_ORDERS);
    let mixed_orders = mixed_specs(POPULATION_ORDERS);
    let mut group = criterion.benchmark_group("book_population");
    group.sampling_mode(SamplingMode::Flat);
    group.throughput(Throughput::Elements(POPULATION_ORDERS as u64));

    for round in 1..=BENCHMARK_ROUNDS {
        group.bench_with_input(
            BenchmarkId::new("limit_orders_10000", round_label(round)),
            &limit_orders,
            |bencher, orders| bencher.iter(|| black_box(populate_book(black_box(orders)))),
        );
        group.bench_with_input(
            BenchmarkId::new("mixed_limit_iceberg_10000", round_label(round)),
            &mixed_orders,
            |bencher, orders| bencher.iter(|| black_box(populate_book(black_box(orders)))),
        );
    }

    group.finish();
}

fn bench_reports(criterion: &mut Criterion) {
    let report_modes = [
        ("no_report", Reports::default()),
        (
            "fills_only",
            Reports {
                include_fills: true,
                ..Reports::default()
            },
        ),
        (
            "summary_only",
            Reports {
                include_summary: true,
                ..Reports::default()
            },
        ),
        (
            "market_impact_only",
            Reports {
                include_market_impact: true,
                ..Reports::default()
            },
        ),
    ];
    let mut group = criterion.benchmark_group("book_reports");
    group.sampling_mode(SamplingMode::Flat);
    group.throughput(Throughput::Elements(REPORT_BENCH_FILLS as u64));

    for round in 1..=BENCHMARK_ROUNDS {
        for (name, reports) in report_modes {
            group.bench_function(BenchmarkId::new(name, round_label(round)), |bencher| {
                bencher.iter_batched(
                    || multi_level_ask_book(REPORT_BENCH_FILLS),
                    |mut book| {
                        black_box(market_cross_with_reports(
                            &mut book,
                            REPORT_BENCH_FILLS as u64,
                            reports,
                        ))
                    },
                    BatchSize::LargeInput,
                );
            });
        }
    }

    group.finish();
}

fn bench_matching(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("book_matching");
    group.sampling_mode(SamplingMode::Flat);

    for round in 1..=BENCHMARK_ROUNDS {
        group.throughput(Throughput::Elements(1));
        group.bench_function(
            BenchmarkId::new("exact_limit_cross", round_label(round)),
            |bencher| {
                bencher.iter_batched(
                    exact_cross_book,
                    |mut book| black_box(limit_cross(&mut book)),
                    BatchSize::SmallInput,
                );
            },
        );
        group.bench_function(
            BenchmarkId::new("exact_market_cross", round_label(round)),
            |bencher| {
                bencher.iter_batched(
                    exact_cross_book,
                    |mut book| black_box(market_cross(&mut book, 100)),
                    BatchSize::SmallInput,
                );
            },
        );

        group.throughput(Throughput::Elements(SWEEP_ORDERS as u64));
        group.bench_function(
            BenchmarkId::new("fifo_same_price_sweep_2000", round_label(round)),
            |bencher| {
                bencher.iter_batched(
                    || same_price_ask_book(SWEEP_ORDERS),
                    |mut book| black_box(market_cross(&mut book, SWEEP_ORDERS as u64)),
                    BatchSize::LargeInput,
                );
            },
        );
        group.bench_function(
            BenchmarkId::new("market_100_level_sweep_2000", round_label(round)),
            |bencher| {
                bencher.iter_batched(
                    || multi_level_ask_book(SWEEP_ORDERS),
                    |mut book| black_box(market_cross(&mut book, SWEEP_ORDERS as u64)),
                    BatchSize::LargeInput,
                );
            },
        );
        group.bench_function(
            BenchmarkId::new("bounded_limit_50_level_sweep", round_label(round)),
            |bencher| {
                bencher.iter_batched(
                    || multi_level_ask_book(SWEEP_ORDERS),
                    |mut book| {
                        let result = book
                            .submit::<LimitOrderData, LimitOrderData, MutatingFills>(
                                Command::Fill {
                                    order: LimitOrder::new(
                                        Some(CompressedPrice::from(ASK_MIN_RAW + 49)),
                                        SWEEP_ORDERS as u64,
                                        TAKER,
                                        Side::Buy,
                                    ),
                                    execution: MutatingFills,
                                    reports: Reports::default(),
                                },
                            )
                            .ok()
                            .expect("bounded limit sweep failed");
                        black_box(result)
                    },
                    BatchSize::LargeInput,
                );
            },
        );

        group.throughput(Throughput::Elements(500));
        group.bench_function(
            BenchmarkId::new("iceberg_replenishment_sweep_500", round_label(round)),
            |bencher| {
                bencher.iter_batched(
                    || iceberg_ask_book(500),
                    |mut book| black_box(market_cross(&mut book, 5_000)),
                    BatchSize::LargeInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_cancellation(criterion: &mut Criterion) {
    let missing_order = Uuid::from_u128(u128::MAX - 1);
    let mut group = criterion.benchmark_group("book_cancellation");
    group.sampling_mode(SamplingMode::Flat);

    for round in 1..=BENCHMARK_ROUNDS {
        group.throughput(Throughput::Elements(1));
        group.bench_function(
            BenchmarkId::new("single_existing_order", round_label(round)),
            |bencher| {
                bencher.iter_batched(
                    one_order_book,
                    |(mut book, order_id)| {
                        black_box(
                            book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(
                                Command::Cancel { order_id },
                            )
                            .is_ok(),
                        )
                    },
                    BatchSize::SmallInput,
                );
            },
        );

        group.throughput(Throughput::Elements(CANCEL_ALL_ORDERS as u64));
        group.bench_function(
            BenchmarkId::new("all_for_user_2000", round_label(round)),
            |bencher| {
                bencher.iter_batched(
                    || one_user_book(CANCEL_ALL_ORDERS),
                    |mut book| {
                        black_box(
                            book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(
                                Command::CancelAllForUser {
                                    user_id: TARGET_USER,
                                },
                            )
                            .is_ok(),
                        )
                    },
                    BatchSize::LargeInput,
                );
            },
        );

        group.throughput(Throughput::Elements(1));
        group.bench_function(
            BenchmarkId::new("missing_order_noop", round_label(round)),
            |bencher| {
                let mut book = BenchmarkBook::<CompressedPrice>::default();
                bencher.iter(|| {
                    black_box(
                        book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(
                            Command::Cancel {
                                order_id: missing_order,
                            },
                        )
                        .is_err(),
                    )
                });
            },
        );
        group.bench_function(
            BenchmarkId::new("empty_user_noop", round_label(round)),
            |bencher| {
                let mut book = BenchmarkBook::<CompressedPrice>::default();
                bencher.iter(|| {
                    black_box(
                        book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(
                            Command::CancelAllForUser {
                                user_id: MISSING_USER,
                            },
                        )
                        .is_ok(),
                    )
                });
            },
        );
    }

    group.finish();
}

fn bench_non_crossing(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("book_non_crossing");
    group.sampling_mode(SamplingMode::Flat);

    for round in 1..=BENCHMARK_ROUNDS {
        group.throughput(Throughput::Elements(1));
        group.bench_function(
            BenchmarkId::new("resting_limit_on_empty_book", round_label(round)),
            |bencher| {
                bencher.iter_batched(
                    BenchmarkBook::<CompressedPrice>::new,
                    |mut book| {
                        black_box(
                            book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(
                                Command::Fill {
                                    order: LimitOrder::new(Some(bid_max()), 100, TAKER, Side::Buy),
                                    execution: MutatingFills,
                                    reports: Reports::default(),
                                },
                            )
                            .is_ok(),
                        )
                    },
                    BatchSize::SmallInput,
                );
            },
        );
        group.bench_function(
            BenchmarkId::new("market_on_empty_book", round_label(round)),
            |bencher| {
                let mut book = BenchmarkBook::<CompressedPrice>::default();
                bencher.iter(|| {
                    black_box(
                        book.submit::<MarketOrderData, LimitOrderData, MutatingFills>(
                            Command::Fill {
                                order: MarketOrder::new(100, TAKER, Side::Buy),
                                execution: MutatingFills,
                                reports: Reports::default(),
                            },
                        )
                        .is_ok(),
                    )
                });
            },
        );
    }

    group.finish();
}

fn bench_queries(criterion: &mut Criterion) {
    let mut query_book = one_user_book(QUERY_USER_ORDERS);
    let levels_book = populate_book(&limit_specs(POPULATION_ORDERS));
    let mut group = criterion.benchmark_group("book_queries");
    group.sampling_mode(SamplingMode::Flat);

    for round in 1..=BENCHMARK_ROUNDS {
        group.throughput(Throughput::Elements(QUERY_USER_ORDERS as u64));
        group.bench_function(
            BenchmarkId::new("user_liquidity_2000", round_label(round)),
            |bencher| {
                bencher.iter(|| {
                    black_box(
                        query_book
                            .order_storage
                            .user_outstanding_liquidity(TARGET_USER),
                    )
                });
            },
        );

        group.throughput(Throughput::Elements(2));
        group.bench_function(
            BenchmarkId::new("best_bid_and_ask", round_label(round)),
            |bencher| {
                bencher.iter(|| {
                    black_box((
                        levels_book.order_storage.bids.best(),
                        levels_book.order_storage.asks.best(),
                    ))
                });
            },
        );
    }

    group.finish();
}

fn benchmark_configuration() -> Criterion {
    Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_millis(250))
        .measurement_time(Duration::from_secs(2))
        .nresamples(10_000)
        .noise_threshold(0.03)
        .significance_level(0.01)
        .confidence_level(0.95)
}

criterion_group! {
    name = benches;
    config = benchmark_configuration();
    targets =
        bench_population,
        bench_matching,
        bench_reports,
        bench_cancellation,
        bench_non_crossing,
        bench_queries
}
criterion_main!(benches);
