#![cfg(feature = "kraken")]
#[path = "support/feed.rs"]
mod support;
use lobo_adapters::{adapter::MarketDataAdapter, kraken::Kraken};
use lobo_storage::price_level::PriceLevelContract;
use support::kraken as native;
const SNAPSHOT: &str = include_str!("fixtures/kraken_snapshot.json");
const UPDATE: &str = include_str!("fixtures/kraken_update.json");
const INSTRUMENTS: &str = r#"{"channel":"instrument","type":"snapshot","data":{"pairs":[{"symbol":"BTC/USD","price_precision":1,"qty_precision":8,"status":"online"},{"symbol":"ETH/USD","price_precision":2,"qty_precision":8,"status":"online"}]}}"#;
fn send(adapter: &mut dyn MarketDataAdapter, message: &str) {
    adapter.receive(message.as_bytes(), false).unwrap();
}
fn ready() -> Kraken {
    let mut adapter = Kraken::new("BTC/USD", 10).unwrap();
    adapter.connected().unwrap();
    adapter.commands();
    send(&mut adapter, INSTRUMENTS);
    adapter.commands();
    adapter
}

fn trade(id: u64, time: u64, price: &str, qty: &str) -> String {
    format!(
        r#"{{"channel":"trade","type":"update","data":[{{"symbol":"BTC/USD","trade_id":{id},"timestamp":"2024-01-01T00:00:{time:02}Z","side":"buy","price":{price},"qty":{qty}}}]}}"#
    )
}
#[test]
fn public_trades_drive_all_bar_types_without_changing_native_liquidity() {
    use lobo_batchers::Aggregation;
    use std::num::NonZeroU64;
    for aggregation in [
        Aggregation::Volume(NonZeroU64::new(3).unwrap()),
        Aggregation::Ticks(NonZeroU64::new(2).unwrap()),
        Aggregation::Time(NonZeroU64::new(5_000_000_000).unwrap()),
        Aggregation::Notional(NonZeroU64::new(100000).unwrap()),
    ] {
        let mut adapter = ready();
        adapter.state_mut().set_bar_aggregation(aggregation);
        send(&mut adapter, SNAPSHOT);
        send(&mut adapter, UPDATE);
        assert_eq!(
            adapter.state().volume_bars().borrow().completed("BTC/USD"),
            0
        );
        let liquidity = adapter
            .selected_book()
            .map(native)
            .unwrap()
            .order_storage
            .asks
            .visible_quantity;
        send(&mut adapter, &trade(1, 1, "45283.5", "1.0"));
        send(&mut adapter, &trade(2, 2, "45284.0", "2.0"));
        // Source time, not a cross-channel book timestamp, closes time bars.
        if matches!(aggregation, Aggregation::Time(_)) {
            send(&mut adapter, &trade(3, 6, "45284.0", "0.1"));
        }
        let bars = adapter.state().volume_bars().borrow();
        assert_eq!(bars.completed("BTC/USD"), 1);
        let messages = bars.destination.0.borrow();
        let event = &messages[0];
        let bar = event.event();
        assert_eq!(
            (bar.open, bar.high, bar.low, bar.close),
            (
                452835u64.into(),
                452840u64.into(),
                452835u64.into(),
                452840u64.into()
            )
        );
        assert_eq!(bar.volume, 300_000_000);
        assert_eq!(bar.ticks, 2);
        assert_ne!(event.timestamp_ns(), 0);
        assert_eq!(
            adapter
                .selected_book()
                .map(native)
                .unwrap()
                .order_storage
                .asks
                .visible_quantity,
            liquidity
        );
        assert_eq!(adapter.state().checksum_failures, 0);
    }
}
#[test]
fn duplicate_trades_and_snapshots_do_not_double_count_on_reconnect() {
    let mut adapter = ready();
    let message = trade(1, 1, "45283.5", "1.0");
    send(&mut adapter, &message);
    send(&mut adapter, &message);
    send(&mut adapter, &message.replace("update", "snapshot"));
    assert_eq!(
        adapter
            .state()
            .volume_bars()
            .borrow()
            .progress("BTC/USD", 0),
        1.0
    );
    adapter.disconnected();
    adapter.connected().unwrap();
    send(&mut adapter, INSTRUMENTS);
    send(&mut adapter, &message);
    send(&mut adapter, &trade(2, 2, "45284.0", "1.0"));
    assert_eq!(
        adapter
            .state()
            .volume_bars()
            .borrow()
            .progress("BTC/USD", 0),
        2.0
    );
}
fn bid(adapter: &Kraken, price: u64) -> Option<u64> {
    adapter
        .selected_book()
        .map(native)?
        .order_storage
        .bids
        .get(price)
        .map(|level| level.visible_quantity())
}
#[test]
fn snapshot_matches_krakens_published_crc_using_native_levels() {
    // Reference: https://docs.kraken.com/exchange/guides/websockets/book-checksum-v2
    let mut adapter = ready();
    send(&mut adapter, SNAPSHOT);
    let state = adapter.state();
    let book = state.selected_book().map(native).unwrap();
    assert_eq!(state.checksum_checks, 1);
    assert_eq!(state.checksum_failures, 0);
    assert!(!state.warming);
    assert_eq!(bid(&adapter, 452835), Some(10_000_000));
    assert_eq!(
        book.order_storage
            .asks
            .get(452852u64)
            .unwrap()
            .visible_quantity(),
        100_000
    );
    assert_eq!(book.order_storage.bids.visible_price_levels().count(), 10);
    assert_eq!(book.order_storage.bids.len(), 0);
    assert_eq!(book.sequence(), 22);
    let output = state.outputs["BTC/USD"].borrow();
    assert_eq!(output.pending.len(), 20);
    for event in output.pending.values() {
        assert_eq!(event.book_id(), "BTC/USD");
        assert_eq!(event.event().number_of_orders(), 0);
        assert!((1..=book.sequence()).contains(&event.sequence_number()));
    }
    assert_eq!(
        book.order_storage.bids.visible_quantity,
        book.order_storage
            .bids
            .visible_price_levels()
            .map(|(_, level)| level.visible_quantity())
            .sum::<u64>()
    );
}
#[test]
fn numeric_wire_decimals_keep_trailing_zero_precision() {
    let mut adapter = ready();
    let mut wire = SNAPSHOT.to_owned();
    let value: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    for side in ["bids", "asks"] {
        for level in value["data"][0][side].as_array().unwrap() {
            for field in ["price", "qty"] {
                let text = level[field].as_str().unwrap();
                wire = wire.replace(&format!("\"{text}\""), text);
            }
        }
    }
    send(&mut adapter, &wire);
    assert_eq!(adapter.state().checksum_failures, 0);
    assert_eq!(adapter.state().checksum_checks, 1);
    assert!(adapter.state().synchronized("BTC/USD"));
}
#[test]
fn deltas_are_sequential_and_depth_evictions_publish_native_deletions() {
    let mut adapter = ready();
    send(&mut adapter, SNAPSHOT);
    adapter.state().outputs["BTC/USD"]
        .borrow_mut()
        .pending
        .clear();
    send(&mut adapter, UPDATE);
    let book = adapter.selected_book().map(native).unwrap();
    assert_eq!(adapter.state().checksum_checks, 2);
    assert_eq!(adapter.state().checksum_failures, 0);
    assert_eq!(bid(&adapter, 452835), Some(20_000_000));
    assert_eq!(bid(&adapter, 452840), Some(30_000_000));
    assert_eq!(book.order_storage.bids.visible_price_levels().count(), 10);
    assert_eq!(bid(&adapter, 452766), None);
    assert_eq!(book.sequence(), 29); // wire changes, native eviction, and best-price events
    let output = adapter.state().outputs["BTC/USD"].borrow();
    assert_eq!(output.pending[&(452766, 0)].event().visible_quantity(), 0);
    assert_eq!(output.pending[&(452835, 0)].sequence_number(), 25);
}
#[test]
fn checksum_failure_never_releases_bad_levels_and_requires_new_snapshot() {
    let mut adapter = ready();
    send(&mut adapter, SNAPSHOT);
    let mut bad: serde_json::Value = serde_json::from_str(UPDATE).unwrap();
    bad["data"][0]["checksum"] = 0.into();
    send(&mut adapter, &bad.to_string());
    assert_eq!(adapter.state().checksum_failures, 1);
    assert!(adapter.state().warming);
    let book = adapter.selected_book().map(native).unwrap();
    assert_eq!(book.order_storage.bids.visible_price_levels().count(), 0);
    assert_eq!(book.order_storage.bids.visible_quantity, 0);
    assert_eq!(book.order_storage.asks.visible_quantity, 0);
    assert!(
        adapter.state().outputs["BTC/USD"]
            .borrow()
            .pending
            .values()
            .all(|event| event.event().visible_quantity() == 0)
    );
    let commands = adapter.commands();
    assert_eq!(commands.len(), 2);
    assert!(commands[0].contains("unsubscribe"));
    assert!(commands[1].contains("subscribe"));
    send(&mut adapter, UPDATE);
    assert_eq!(adapter.state().checksum_checks, 2);
    send(&mut adapter, SNAPSHOT);
    assert!(!adapter.state().warming);
    assert_eq!(adapter.state().checksum_checks, 3);
}
#[test]
fn reconnect_and_symbol_switch_retain_subscriptions_but_require_snapshots() {
    let mut adapter = ready();
    send(&mut adapter, SNAPSHOT);
    assert_eq!(adapter.tickers(), ["BTC/USD", "ETH/USD"]);
    adapter.select_ticker("eth/usd").unwrap();
    assert!(adapter.state().warming);
    assert_eq!(adapter.commands().len(), 2);
    adapter.select_ticker("BTC/USD").unwrap();
    assert!(!adapter.state().warming);
    assert!(adapter.commands().is_empty());
    adapter.disconnected();
    assert!(adapter.state().warming);
    assert_eq!(
        adapter
            .selected_book()
            .map(native)
            .unwrap()
            .order_storage
            .bids
            .visible_quantity,
        0
    );
    adapter.connected().unwrap();
    assert_eq!(adapter.commands().len(), 1);
    send(&mut adapter, INSTRUMENTS);
    assert_eq!(adapter.commands().len(), 4);
    send(&mut adapter, UPDATE);
    assert!(adapter.state().warming);
    send(&mut adapter, SNAPSHOT);
    assert!(!adapter.state().warming);
}
#[test]
fn replacement_snapshot_removes_old_levels() {
    let mut adapter = ready();
    send(&mut adapter, SNAPSHOT);
    send(&mut adapter, UPDATE);
    let old_sequence = adapter.selected_book().map(native).unwrap().sequence();
    send(&mut adapter, SNAPSHOT);
    assert_eq!(bid(&adapter, 452840), None);
    assert_eq!(bid(&adapter, 452766), Some(15_445_238));
    assert_eq!(
        adapter.selected_book().map(native).unwrap().sequence(),
        old_sequence + 44
    );
}
#[test]
fn unsupported_precision_is_rejected_before_any_mutation() {
    let mut adapter = ready();
    let bad = SNAPSHOT.replace("45283.5", "45283.55");
    assert!(
        adapter
            .receive(bad.as_bytes(), false)
            .unwrap_err()
            .contains("precision")
    );
    assert!(adapter.selected_book().is_none());
    assert_eq!(adapter.state().outputs["BTC/USD"].borrow().count, 0);
}
#[test]
fn native_null_publisher_elides_sequences_for_aggregate_adapter() {
    use lobo_adapters::{adapter::AdaptForReplay, kraken::messages::KrakenBookUpdate};
    use lobo_books::price_time_priority::Book;
    use lobo_primitives::Price64;
    use lobo_storage::price_level::IntrusivePriceLevel;
    let mut book =
        Book::<IntrusivePriceLevel<Price64, lobo_storage::policies::UpdateHiddenQuantity>>::new();
    KrakenBookUpdate {
        timestamp: 0,
        bids: vec![(452835u64.into(), 100)],
        asks: vec![],
        depth: 10,
    }
    .process(&mut book)
    .unwrap();
    assert_eq!(book.sequence(), 0);
    assert_eq!(book.order_storage.bids.visible_quantity, 100);
}

#[test]
fn l2_market_previews_use_aggregate_depth_without_mutation_or_branch() {
    use lobo_models::{
        Side,
        orders::order_types::{LimitOrder, MarketOrder},
    };
    use lobo_primitives::{Price64, uuid::Uuid};
    for side in [Side::Buy, Side::Sell] {
        let mut adapter = ready();
        send(&mut adapter, SNAPSHOT);
        let book = adapter.selected_book().map(native).unwrap();
        let levels: Vec<_> = if side == Side::Buy {
            book.order_storage
                .asks
                .visible_price_levels()
                .map(|(p, l)| (*p, l.visible_quantity()))
                .collect()
        } else {
            book.order_storage
                .bids
                .visible_price_levels()
                .map(|(p, l)| (*p, l.visible_quantity()))
                .collect()
        };
        let quantity = levels.iter().map(|(_, q)| q).sum::<u64>();
        let notional: u128 = levels
            .iter()
            .map(|(p, q)| u128::from(u64::from(*p)) * u128::from(*q))
            .sum();
        let sequence = book.sequence();
        adapter
            .simulate_market(MarketOrder::<Price64>::new(quantity + 1, Uuid::nil(), side))
            .unwrap();
        let report = adapter.state().market_preview.as_ref().unwrap();
        assert_eq!((report.filled, report.remaining()), (quantity, 1));
        assert_eq!(
            report.average_price(),
            Some(notional as f64 / quantity as f64)
        );
        assert_eq!(report.executions.len(), levels.len());
        assert!(
            report
                .executions
                .iter()
                .all(|m| m.event().maker_order_id.is_nil())
        );
        assert!(adapter.state().simulation.is_none());
        assert_eq!(
            adapter.selected_book().map(native).unwrap().sequence(),
            sequence
        );
        assert_eq!(
            adapter
                .selected_book()
                .map(native)
                .unwrap()
                .order_storage
                .asks
                .len(),
            0
        );
        assert!(
            adapter
                .simulate(LimitOrder::<Price64>::new(
                    Some(452835u64),
                    1,
                    Uuid::nil(),
                    side
                ))
                .is_err()
        );
        // A real update still passes CRC after the preview; it didn't consume depth.
        send(&mut adapter, UPDATE);
        assert_eq!(adapter.state().checksum_failures, 0);
        assert_eq!(adapter.state().checksum_checks, 2);
        assert_eq!(
            adapter.state().volume_bars().borrow().completed("BTC/USD"),
            0
        );
        adapter
            .simulate_market(MarketOrder::<Price64>::new(1, Uuid::nil(), side))
            .unwrap();
        let report = adapter.state().market_preview.as_ref().unwrap();
        assert_eq!((report.filled, report.remaining()), (1, 0));
        assert_eq!(report.executions.len(), 1);
    }
}

#[test]
fn explicit_scope_subscribes_all_selected_pairs_and_excludes_others() {
    use lobo_context::BookScope;
    let mut adapter = Kraken::new("BTC/USD", 10).unwrap();
    adapter
        .set_book_scope(BookScope::Selected(
            ["BTC/USD".into(), "ETH/USD".into()].into(),
        ))
        .unwrap();
    adapter.connected().unwrap();
    adapter.commands();
    send(&mut adapter, INSTRUMENTS);
    let commands = adapter.commands();
    assert_eq!(commands.len(), 4); // book + executions for each selected pair
    assert_eq!(commands.iter().filter(|c| c.contains("ETH/USD")).count(), 2);
    assert!(adapter.subscribe("SOL/USD").is_err());
    assert!(adapter.selected_book().is_none()); // directory/subscriptions allocate no book
}
