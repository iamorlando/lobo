#![cfg(feature = "bitfinex")]
#[path = "support/feed.rs"]
mod support;
use lobo_adapters::{
    adapter::{
        MarketDataAdapter,
        market::{adapters, create_adapter},
    },
    bitfinex::Bitfinex,
};
use lobo_models::{
    Side,
    orders::{
        order_types::{LimitOrder, MarketOrder},
        traits::Trades,
    },
};
use lobo_primitives::{Price64, uuid::Uuid};
use lobo_storage::price_level::PriceLevelContract;
use serde_json::{Value, json};
use std::num::NonZeroU64;
use support::bitfinex as native;
const SCALE: u64 = 100_000_000;
const START: u64 = 1_700_000_000_000;
struct Feed {
    adapter: Bitfinex,
    sequence: u64,
    time: u64,
}
impl Feed {
    fn new() -> Self {
        let mut adapter = Bitfinex::new("BTCUSD").unwrap();
        adapter
            .bootstrap("instruments", br#"[["BTCUSD","ETHUSD","AAVE:USD"]]"#)
            .unwrap();
        adapter.connected().unwrap();
        assert_eq!(adapter.commands(), [r#"{"event":"conf","flags":229376}"#]);
        for message in [
            json!({"event":"conf","status":"OK","flags":229376}),
            json!({"event":"subscribed","chanId":1,"channel":"book","prec":"R0","symbol":"tBTCUSD"}),
            json!({"event":"subscribed","chanId":2,"channel":"trades","symbol":"tBTCUSD"}),
        ] {
            adapter
                .receive(message.to_string().as_bytes(), false)
                .unwrap();
        }
        assert_eq!(adapter.commands().len(), 2);
        Self {
            adapter,
            sequence: 0,
            time: START,
        }
    }
    fn send(&mut self, mut frame: Value) -> Result<(), String> {
        self.sequence += 1;
        let row = frame.as_array_mut().unwrap();
        row.push(self.sequence.into());
        row.push(self.time.into());
        self.adapter.receive(frame.to_string().as_bytes(), false)
    }
    fn snapshot(&mut self) {
        self.send(json!([1, [[20, 100, 2], [30, 101, -3], [10, 100, 1]]]))
            .unwrap();
        assert!(self.adapter.state().warming);
        self.crc("10:1:30:-3:20:2");
        assert!(!self.adapter.state().warming);
    }
    fn crc(&mut self, operands: &str) {
        self.send(json!([
            1,
            "cs",
            crc32fast::hash(operands.as_bytes()) as i32
        ]))
        .unwrap();
    }
    fn update(&mut self, order: Value) {
        self.send(json!([1, order])).unwrap();
    }
    fn trade(&mut self, id: u64, kind: &str, amount: f64, price: f64) {
        self.send(json!([2, kind, [id, self.time, amount, price]]))
            .unwrap();
    }
    fn limit(&mut self, quantity: u64) {
        self.adapter
            .simulate(LimitOrder::new(
                Some(Price64::from(100 * SCALE)),
                quantity,
                Uuid::nil(),
                Side::Buy,
            ))
            .unwrap();
    }
}
fn queue(feed: &Feed, side: Side) -> Vec<(u128, u64)> {
    feed.adapter
        .state()
        .selected_book()
        .map(native)
        .unwrap()
        .order_storage
        .queue_view(side, 0u64.into()..=u64::MAX.into())
        .into_iter()
        .map(|o| (o.id.as_u128(), o.quantity))
        .collect()
}
#[test]
fn recorded_public_snapshot_deltas_and_signed_checksums_use_native_orders() {
    let mut adapter: Box<dyn MarketDataAdapter> = create_adapter("bitfinex", "BTCUSD", 0).unwrap();
    adapter
        .bootstrap("instruments", br#"[["BTCUSD"]]"#)
        .unwrap();
    adapter.connected().unwrap();
    for line in include_str!("fixtures/bitfinex_r0.jsonl").lines() {
        adapter.receive(line.as_bytes(), false).unwrap();
    }
    assert_eq!(adapter.state().checksum_checks, 2);
    assert_eq!(adapter.state().checksum_failures, 0);
    assert!(!adapter.state().warming);
    let book = adapter.selected_book().map(native).unwrap();
    assert_eq!(book.order_storage.order_to_arena_map.len(), 500);
    assert!(
        book.order_storage
            .bids
            .visible_price_levels()
            .all(|(_, l)| l.len() > 0)
    );
    assert!(adapters().iter().any(|a| a.id == "bitfinex"
        && a.level == lobo_context::BookLevel::L3
        && a.mode == lobo_context::FeedMode::Live));
}
#[test]
fn snapshot_fifo_native_mutations_and_exact_checksum_decimals() {
    let mut f = Feed::new();
    f.snapshot();
    assert_eq!(queue(&f, Side::Buy), [(10, SCALE), (20, 2 * SCALE)]);
    f.time += 1;
    f.update(json!([20, 100, 1.5]));
    assert_eq!(queue(&f, Side::Buy), [(10, SCALE), (20, 150_000_000)]);
    f.crc("10:1:30:-3:20:1.5");
    f.time += 1;
    f.update(json!([10, 99, 1]));
    f.crc("20:1.5:30:-3:10:1");
    let order = f
        .adapter
        .state()
        .selected_book()
        .map(native)
        .unwrap()
        .order_storage
        .order(Uuid::from_u128(10))
        .unwrap();
    assert_eq!(
        order.common_data.creation_time.timestamp_millis(),
        f.time as i64
    );
    f.update(json!([20, 0, 1]));
    f.update(json!([44, 100, 0.00000001]));
    f.update(json!([45, 100, 0.00000099]));
    f.crc("44:1e-8:30:-3:45:9.9e-7:10:1");
    assert_eq!(
        f.adapter.state().volume_bars().borrow().completed("BTCUSD"),
        0
    );
}
#[test]
fn checksum_covers_25_orders_per_side_with_same_price_ids_sorted() {
    let mut f = Feed::new();
    let rows: Vec<_> = (1..=30).rev().map(|id| json!([id, 100, 1])).collect();
    f.send(json!([1, rows])).unwrap();
    let operands = (1..=25)
        .map(|id| format!("{id}:1"))
        .collect::<Vec<_>>()
        .join(":");
    f.crc(&operands);
    let crc = f.adapter.state_mut().book_mut("BTCUSD").checksum().unwrap();
    f.update(json!([30, 100, 2]));
    assert_eq!(f.adapter.state_mut().book_mut("BTCUSD").checksum().unwrap(), crc);
    f.update(json!([1, 100, 2]));
    assert_ne!(f.adapter.state_mut().book_mut("BTCUSD").checksum().unwrap(), crc);
}
#[test]
fn trade_notifications_drive_all_bars_once_without_changing_main_liquidity() {
    use lobo_batchers::Aggregation;
    for aggregation in [
        Aggregation::Volume(NonZeroU64::new(3).unwrap()),
        Aggregation::Ticks(NonZeroU64::new(2).unwrap()),
        Aggregation::Time(NonZeroU64::new(5_000_000_000).unwrap()),
        Aggregation::Notional(NonZeroU64::new(302).unwrap()),
    ] {
        let mut f = Feed::new();
        f.snapshot();
        f.adapter.state_mut().set_bar_aggregation(aggregation);
        let before = queue(&f, Side::Sell);
        f.send(json!([2, [[100, START - 1000, 50, 100]]])).unwrap();
        f.trade(100, "tu", 50., 100.); // snapshot floor
        f.time += 1000;
        f.trade(101, "te", 1., 100.);
        f.trade(101, "tu", 1., 100.);
        f.time += 1000;
        f.trade(102, "tu", 2., 101.);
        f.trade(102, "te", 2., 101.);
        if matches!(aggregation, Aggregation::Time(_)) {
            f.time += 5000;
            f.trade(103, "te", 0.1, 102.);
        }
        let bars = f.adapter.state().volume_bars().borrow();
        assert_eq!(bars.completed("BTCUSD"), 1);
        let events = bars.destination.0.borrow();
        let bar = events[0].event();
        assert_eq!(
            (bar.open, bar.close, bar.volume, bar.ticks),
            (
                Price64::from(100 * SCALE),
                Price64::from(101 * SCALE),
                3 * SCALE,
                2
            )
        );
        assert_eq!(queue(&f, Side::Sell), before);
    }
}
#[test]
fn live_limit_reconciles_book_before_or_after_trade_without_double_consumption() {
    for book_first in [false, true] {
        let mut f = Feed::new();
        f.snapshot();
        f.limit(SCALE / 5);
        let user = f
            .adapter
            .state()
            .simulation
            .as_ref()
            .unwrap()
            .report
            .order_id;
        f.time += 10;
        if book_first {
            f.update(json!([10, 0, 1]));
            f.update(json!([20, 0, 1]));
        }
        f.trade(1, "te", -3.1, 100.);
        if !book_first {
            f.update(json!([10, 0, 1]));
            f.update(json!([20, 0, 1]));
        }
        f.adapter.advance(1_000_000_000, 1000).unwrap();
        let branch = f.adapter.state().simulation.as_ref().unwrap();
        assert_eq!(branch.report.filled, SCALE / 10);
        assert_eq!(branch.remaining(), SCALE / 10);
        assert_eq!(
            branch
                .feed
                .selected_book()
                .map(native)
                .unwrap()
                .order_storage
                .order(user)
                .unwrap()
                .quantity(),
            SCALE / 10
        );
        assert!(queue(&f, Side::Buy).is_empty());
        f.time += 1010;
        f.trade(2, "te", -0.1, 100.);
        let branch = f.adapter.state().simulation.as_ref().unwrap();
        assert!(branch.complete());
        let clock = branch.feed.clock_ns;
        f.time += 1000;
        f.update(json!([40, 100, 1]));
        f.trade(3, "te", -1., 100.);
        let branch = f.adapter.state().simulation.as_ref().unwrap();
        assert_eq!(branch.feed.clock_ns, clock);
        assert_eq!(branch.report.executions.len(), 2);
        f.adapter.return_to_main();
        assert!(f.adapter.state().simulation.is_none());
        assert_eq!(queue(&f, Side::Buy), [(40, SCALE)]);
    }
}
#[test]
fn cancellations_never_fill_and_market_preview_never_mutates() {
    let mut f = Feed::new();
    f.snapshot();
    f.adapter
        .simulate_market(MarketOrder::new(4 * SCALE, Uuid::nil(), Side::Buy))
        .unwrap();
    let report = f.adapter.state().market_preview.as_ref().unwrap();
    assert_eq!(report.filled, 3 * SCALE);
    assert_eq!(report.remaining(), SCALE);
    assert_eq!(queue(&f, Side::Sell), [(30, 3 * SCALE)]);
    f.adapter.return_to_main();
    f.limit(SCALE);
    f.time += 10;
    f.update(json!([10, 0, 1]));
    f.update(json!([20, 0, 1]));
    f.adapter.advance(1_000_000_000, 1000).unwrap();
    let branch = f.adapter.state().simulation.as_ref().unwrap();
    assert_eq!(branch.report.filled, 0);
    let orders = branch
        .feed
        .selected_book()
        .map(native)
        .unwrap()
        .order_storage
        .queue_view(Side::Buy, 0u64.into()..=u64::MAX.into());
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].id, branch.report.order_id);
}
#[test]
fn gaps_bad_checksums_and_maintenance_invalidate_main_and_freeze_branch() {
    for kind in ["gap", "crc", "maintenance", "bad-json"] {
        let mut f = Feed::new();
        f.snapshot();
        f.limit(SCALE);
        let result = match kind {
            "gap" => {
                f.sequence += 1;
                f.send(json!([1, "hb"]))
            }
            "crc" => f.send(json!([1, "cs", 123])),
            "maintenance" => f
                .adapter
                .receive(br#"{"event":"info","code":20060}"#, false),
            _ => f.adapter.receive(b"[", false),
        };
        assert!(result.is_err());
        assert!(f.adapter.state().warming);
        assert!(queue(&f, Side::Buy).is_empty());
        let branch = f.adapter.state().simulation.as_ref().unwrap();
        assert!(branch.stopped());
        assert!(!branch.complete());
        assert!(branch.stopped_reason.is_some());
        f.adapter.connected().unwrap();
        assert!(f.adapter.commands().iter().any(|c| c.contains("conf")));
    }
}
#[test]
fn directory_is_complete_and_visited_pairs_shard_without_losing_subscriptions() {
    let symbols: Vec<_> = (0..31).map(|n| format!("COIN{n}:USD")).collect();
    let mut adapter = Bitfinex::new(&symbols[0]).unwrap();
    adapter
        .bootstrap("instruments", json!([symbols]).to_string().as_bytes())
        .unwrap();
    assert_eq!(adapter.tickers().len(), 31);
    assert!(adapter.state().context.books.is_empty());
    for symbol in &symbols {
        adapter.select_ticker(symbol).unwrap();
    }
    assert_eq!(adapter.connections().len(), 3);
    for id in 0..3 {
        adapter.connected_on(id).unwrap();
        adapter.commands_on(id);
        adapter
            .receive_on(id, br#"{"event":"conf","flags":229376,"status":"OK"}"#)
            .unwrap();
        assert_eq!(adapter.commands_on(id).len(), if id == 2 { 2 } else { 30 });
    }
    assert!(adapter.connections()[2].selected);
    adapter.select_ticker(&symbols[0]).unwrap();
    assert!(adapter.connections()[0].selected);
    assert!(adapter.commands_on(0).is_empty());
    assert_eq!(adapter.connections().len(), 3);
}

#[test]
fn socket_sequences_and_channel_ids_are_isolated_and_reconnect_deduplicates_trades() {
    let symbols: Vec<_> = (0..16).map(|n| format!("COIN{n}:USD")).collect();
    let mut adapter = Bitfinex::new(&symbols[0]).unwrap();
    adapter
        .bootstrap("instruments", json!([symbols]).to_string().as_bytes())
        .unwrap();
    for symbol in &symbols {
        adapter.select_ticker(symbol).unwrap();
    }
    for (connection, symbol) in [(0, &symbols[0]), (1, &symbols[15])] {
        adapter.connected_on(connection).unwrap();
        for frame in [
            json!({"event":"conf","status":"OK","flags":229376}),
            json!({"event":"subscribed","channel":"book","prec":"R0","symbol":format!("t{symbol}"),"chanId":1}),
            json!([1, [[10, 100, 1]], 1, START]),
            json!([1, "cs", crc32fast::hash(b"10:1") as i32, 2, START]),
        ] {
            adapter
                .receive_on(connection, frame.to_string().as_bytes())
                .unwrap();
        }
    }
    adapter
        .receive_on(
            0,
            json!([1, [10, 100, 2], 3, START + 1])
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    assert_eq!(
        adapter
            .state()
            .book(&symbols[0])
            .map(native)
            .unwrap()
            .order_storage
            .bids
            .visible_quantity,
        2 * SCALE
    );
    assert_eq!(
        adapter
            .state()
            .book(&symbols[15])
            .map(native)
            .unwrap()
            .order_storage
            .bids
            .visible_quantity,
        SCALE
    );
    adapter.disconnected_on(0);
    assert!(adapter.state().synchronized(&symbols[15]));
    assert!(!adapter.state().warming);

    let mut f = Feed::new();
    f.snapshot();
    f.trade(100, "te", 1., 100.);
    f.adapter.disconnected();
    f.adapter.connected().unwrap();
    f.adapter
        .receive(br#"{"event":"conf","status":"OK","flags":229376}"#, false)
        .unwrap();
    for message in [
        json!({"event":"subscribed","channel":"book","prec":"R0","symbol":"tBTCUSD","chanId":1}),
        json!({"event":"subscribed","channel":"trades","symbol":"tBTCUSD","chanId":2}),
    ] {
        f.adapter
            .receive(message.to_string().as_bytes(), false)
            .unwrap();
    }
    f.sequence = 0;
    f.snapshot();
    f.trade(100, "tu", 1., 100.);
    assert_eq!(
        f.adapter
            .state()
            .volume_bars()
            .borrow()
            .forming("BTCUSD")
            .unwrap()
            .volume,
        SCALE
    );
}

#[test]
fn raw_adapter_and_shared_trades_keep_null_publisher_erased() {
    use lobo_adapters::{
        adapter::{AdaptForReplay, messages::PublicTrade},
        bitfinex::messages::RawOrder,
    };
    use lobo_books::price_time_priority::Book;
    use lobo_storage::{policies::UpdateHiddenQuantity, price_level::IntrusivePriceLevel};
    let mut book = Book::<IntrusivePriceLevel<Price64, UpdateHiddenQuantity>>::new();
    for (price, quantity) in [(100, 2), (100, 1), (101, 3), (0, 1)] {
        RawOrder {
            timestamp: 0,
            reference: 10,
            price,
            quantity,
            side: Side::Buy,
        }
        .process(&mut book)
        .unwrap();
    }
    PublicTrade {
        timestamp: 0,
        price: Price64::from(100u64),
        quantity: 1,
        maker_side: Side::Buy,
    }
    .process(&mut book)
    .unwrap();
    assert!(book.order_storage.order_to_arena_map.is_empty());
    assert_eq!(book.sequence(), 0);
}

#[test]
fn newly_resting_orders_and_price_amendments_match_the_simulated_limit_only() {
    for amend in [false, true] {
        let mut f = Feed::new();
        f.snapshot();
        f.adapter
            .simulate(LimitOrder::new(
                Some(Price64::from(10_050_000_000u64)),
                SCALE / 5,
                Uuid::nil(),
                Side::Buy,
            ))
            .unwrap();
        f.time += 10;
        f.update(json!([if amend { 30 } else { 40 }, 100.25, -0.1]));
        f.adapter.advance(1_000_000_000, 1000).unwrap();
        let branch = f.adapter.state().simulation.as_ref().unwrap();
        assert_eq!(branch.report.filled, SCALE / 10);
        assert_eq!(branch.report.average_price(), Some(10_050_000_000.));
        f.time += 1010;
        f.update(json!([41, 100.25, -0.1]));
        f.adapter.advance(2_000_000_000, 1000).unwrap();
        let branch = f.adapter.state().simulation.as_ref().unwrap();
        assert!(branch.complete());
        assert_eq!(branch.report.executions.len(), 2);
        let asks = f
            .adapter
            .state()
            .selected_book()
            .map(native)
            .unwrap()
            .order_storage
            .asks
            .visible_quantity;
        assert_eq!(
            asks,
            if amend {
                SCALE / 5
            } else {
                3 * SCALE + SCALE / 5
            }
        );
        assert_eq!(
            f.adapter
                .state()
                .volume_bars()
                .borrow()
                .forming("BTCUSD")
                .map_or(0, |b| b.volume),
            0
        );
    }
}

#[test]
fn explicit_scope_uses_common_subscription_routing() {
    use lobo_context::BookScope;
    let mut adapter = Bitfinex::new("BTCUSD").unwrap();
    adapter
        .set_book_scope(BookScope::Selected(
            ["BTCUSD".into(), "ETHUSD".into()].into(),
        ))
        .unwrap();
    adapter
        .bootstrap("instruments", br#"[["BTCUSD","ETHUSD","AAVE:USD"]]"#)
        .unwrap();
    assert_eq!(adapter.tickers(), ["AAVE:USD", "BTCUSD", "ETHUSD"]);
    assert_eq!(adapter.scoped_tickers(), ["BTCUSD", "ETHUSD"]);
    assert!(adapter.select_ticker("AAVE:USD").is_err());
    adapter.connected().unwrap();
    adapter.commands();
    adapter
        .receive(br#"{"event":"conf","status":"OK","flags":229376}"#, false)
        .unwrap();
    let commands = adapter.commands();
    assert_eq!(commands.len(), 4);
    assert_eq!(commands.iter().filter(|c| c.contains("ETHUSD")).count(), 2);
    assert!(adapter.selected_book().is_none());
}
