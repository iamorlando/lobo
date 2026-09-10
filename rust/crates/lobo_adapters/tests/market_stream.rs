#![cfg(feature = "itchy")]
#[path = "support/feed.rs"]
mod support;
use lobo_adapters::{adapter::MarketDataAdapter, itch::stream::ItchStream as Replay};
use lobo_storage::price_level::PriceLevelContract;
use support::{native, native_mut};
fn bid(replay: &Replay) -> Option<u64> {
    replay
        .selected_book()
        .map(native)?
        .order_storage
        .bids
        .visible_price_levels()
        .next()
        .map(|(_, l)| l.visible_quantity())
}

fn record(tag: u8, timestamp: u64, body: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&((11 + body.len()) as u16).to_be_bytes());
    bytes.push(tag);
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(&timestamp.to_be_bytes()[2..]);
    bytes.extend_from_slice(body);
    bytes
}
fn add(timestamp: u64) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&1u64.to_be_bytes());
    body.push(b'B');
    body.extend_from_slice(&100u32.to_be_bytes());
    body.extend_from_slice(b"AAPL    ");
    body.extend_from_slice(&1000000u32.to_be_bytes());
    record(b'A', timestamp, &body)
}
fn cancel(timestamp: u64, quantity: u32) -> Vec<u8> {
    let mut body = 1u64.to_be_bytes().to_vec();
    body.extend_from_slice(&quantity.to_be_bytes());
    record(b'X', timestamp, &body)
}

#[test]
fn simulated_maker_survives_multiple_partial_executions() {
    use lobo_models::{
        Side,
        orders::{order_types::LimitOrder, traits::Trades},
    };
    use lobo_primitives::{Price64, uuid::Uuid};
    let mut ask = add(200);
    ask[13..21].copy_from_slice(&2u64.to_be_bytes());
    ask[21] = b'S';
    let execute = |timestamp, quantity: u32| {
        let mut body = 2u64.to_be_bytes().to_vec();
        body.extend(quantity.to_be_bytes());
        body.extend(1u64.to_be_bytes());
        record(b'E', timestamp, &body)
    };
    let mut replay = Replay::new("AAPL", 0).unwrap();
    replay
        .append(
            &[
                add(100),
                ask,
                execute(300, 20),
                execute(400, 20),
                execute(500, 60),
            ]
            .concat(),
            true,
        )
        .unwrap();
    replay.advance(0, 100).unwrap();
    let order = LimitOrder::<Price64>::new(Some(1000000u64), 150, Uuid::new_v4(), Side::Sell);
    let id = order.uuid();
    replay.simulate(order).unwrap();
    for (elapsed, remaining) in [(200, 30), (300, 10)] {
        replay.advance(elapsed, 100).unwrap();
        let simulation = replay.state().simulation.as_ref().unwrap();
        assert_eq!(simulation.remaining(), remaining);
        assert_eq!(
            simulation
                .feed
                .selected_book()
                .map(native)
                .unwrap()
                .order_storage
                .order(id)
                .map(Trades::quantity),
            Some(remaining)
        );
    }
    replay.advance(400, 100).unwrap();
    let simulation = replay.state().simulation.as_ref().unwrap();
    assert!(simulation.complete());
    assert_eq!(simulation.report.executions.len(), 4);
    assert!(
        simulation
            .feed
            .selected_book()
            .map(native)
            .unwrap()
            .order_storage
            .order(id)
            .is_none()
    );
}

#[test]
fn simulation_forks_native_fifo_matches_future_flow_and_preserves_main_replay() {
    use lobo_models::{
        Side,
        orders::{order_types::LimitOrder, traits::Trades},
    };
    use lobo_primitives::{Price64, uuid::Uuid};
    fn limit(time: u64, id: u64, side: u8, price: u32, quantity: u32) -> Vec<u8> {
        let mut body = id.to_be_bytes().to_vec();
        body.push(side);
        body.extend(quantity.to_be_bytes());
        body.extend(b"AAPL    ");
        body.extend(price.to_be_bytes());
        record(b'A', time, &body)
    }
    let mut execution = 4u64.to_be_bytes().to_vec();
    execution.extend(50u32.to_be_bytes());
    execution.extend(1u64.to_be_bytes());
    let feed = [
        limit(100, 1, b'S', 1010000, 100),
        limit(100, 2, b'B', 990000, 100),
        record(b'D', 200, &1u64.to_be_bytes()),
        limit(210, 3, b'S', 1020000, 100),
        limit(220, 4, b'B', 1010000, 60),
        record(b'E', 300, &execution),
    ]
    .concat();
    let mut replay = Replay::new("AAPL", 0).unwrap();
    replay.append(&feed, true).unwrap();
    replay.advance(0, 100).unwrap();
    let sequence = replay
        .state()
        .selected_book()
        .map(native)
        .unwrap()
        .sequence();
    let order = LimitOrder::<Price64>::new(Some(1010000u64), 150, Uuid::new_v4(), Side::Buy);
    let id = order.uuid();
    replay.simulate(order).unwrap();
    let state = replay.state();
    let main = state.selected_book().map(native).unwrap();
    assert_eq!(main.sequence(), sequence);
    assert_eq!(main.order_storage.asks.visible_quantity, 100);
    assert!(main.order_storage.order(id).is_none());
    let simulation = state.simulation.as_ref().unwrap();
    assert_eq!(simulation.report.filled, 100);
    assert_eq!(simulation.remaining(), 50);
    assert_eq!(simulation.report.executions[0].timestamp_ns(), 100);
    assert_eq!(
        simulation.report.executions[0].event().execution.quantity,
        100
    );
    assert_eq!(
        simulation
            .feed
            .selected_book()
            .map(native)
            .unwrap()
            .order_storage
            .order(id)
            .unwrap()
            .quantity(),
        50
    );
    assert_eq!(state.volume_bars().borrow().progress("AAPL", 100), 0.0);
    assert!(replay.select_ticker("AAPL").is_err());
    replay.advance(100, 100).unwrap(); // A later delete refers to the consumed maker.
    assert_eq!(replay.state().simulation.as_ref().unwrap().ignored, 1);
    replay.advance(200, 100).unwrap();
    let simulation = replay.state().simulation.as_ref().unwrap();
    assert!(simulation.complete());
    assert_eq!(simulation.report.executions.len(), 2);
    assert_eq!(simulation.report.executions[1].timestamp_ns(), 300);
    assert_eq!(simulation.report.executions[1].event().maker_order_id, id);
    assert_eq!(
        simulation.report.executions[1].event().execution.price,
        1010000u64.into()
    );
    assert_eq!(
        replay.state().volume_bars().borrow().progress("AAPL", 300),
        50.0
    );
    replay.return_to_main();
    assert_eq!(
        replay
            .selected_book()
            .map(native)
            .unwrap()
            .order_storage
            .bids
            .get(1010000u64)
            .unwrap()
            .visible_quantity(),
        10
    );
    assert!(replay.state().simulation.is_none());
    assert!(replay.state().complete);
}
#[test]
fn fragmented_input_and_recorded_clock_preserve_mutation_order() {
    let mut replay = Replay::new("AAPL", 0).unwrap();
    let feed = [
        add(100),
        cancel(200, 25),
        record(b'D', 300, &1u64.to_be_bytes()),
    ]
    .concat();
    for byte in &feed {
        replay.append(&[*byte], false).unwrap();
        replay.advance(0, 10).unwrap();
    }
    assert_eq!(
        replay.state().outputs[&replay.state().selected]
            .borrow()
            .count,
        1
    );
    assert_eq!(bid(&replay), Some(100));
    replay.append(&[], true).unwrap();
    replay.advance(100, 10).unwrap();
    assert_eq!(
        replay.state().outputs[&replay.state().selected]
            .borrow()
            .count,
        2
    );
    assert_eq!(bid(&replay), Some(75));
    replay.advance(200, 10).unwrap();
    assert!(replay.state().complete);
    assert_eq!(
        replay.state().outputs[&replay.state().selected]
            .borrow()
            .count,
        3
    );
    assert!(bid(&replay).is_none());
    assert_eq!(
        replay.state().outputs[&replay.state().selected]
            .borrow()
            .pending
            .len(),
        1
    );
    assert_eq!(
        replay.state().outputs[&replay.state().selected]
            .borrow()
            .pending
            .values()
            .next()
            .unwrap()
            .sequence_number(),
        4
    );
}
#[test]
fn preroll_reconstructs_before_requested_start() {
    let mut replay = Replay::new("AAPL", 250).unwrap();
    replay
        .append(
            &[
                add(100),
                cancel(200, 25),
                record(b'D', 300, &1u64.to_be_bytes()),
            ]
            .concat(),
            true,
        )
        .unwrap();
    replay.advance(0, 100).unwrap();
    assert!(!replay.state().warming);
    assert_eq!(replay.state().clock_ns, 250);
    assert_eq!(
        replay.state().outputs[&replay.state().selected]
            .borrow()
            .count,
        2
    );
    replay.advance(50, 100).unwrap();
    assert!(replay.state().complete);
}
#[test]
fn malformed_input_is_rejected_before_unsafe_native_transitions() {
    let mut replay = Replay::new("AAPL", 0).unwrap();
    replay
        .append(&[add(100), cancel(200, 101)].concat(), true)
        .unwrap();
    assert!(replay.advance(1000, 100).unwrap_err().contains("exceeds"));
    let mut replay = Replay::new("AAPL", 0).unwrap();
    replay.append(&add(100)[..15], true).unwrap();
    assert!(replay.advance(0, 10).is_err());
    let mut replay = Replay::new("MSFT", 0).unwrap();
    replay.append(&add(100), true).unwrap();
    replay.advance(0, 10).unwrap();
    assert_eq!(replay.tickers(), ["AAPL"]);
    assert!(replay.selected_book().is_none());
    replay.select_ticker("AAPL").unwrap();
    assert_eq!(
        replay.state().outputs[&replay.state().selected]
            .borrow()
            .count,
        1
    );
}

#[test]
fn switching_books_preserves_all_mutations_clock_and_file_cursor() {
    let mut msft_add = add(150);
    msft_add[3..5].copy_from_slice(&2u16.to_be_bytes());
    msft_add[26..34].copy_from_slice(b"MSFT    ");
    let mut msft_cancel = cancel(250, 40);
    msft_cancel[3..5].copy_from_slice(&2u16.to_be_bytes());
    let mut replay = Replay::new("AAPL", 0).unwrap();
    replay
        .append(
            &[
                add(100),
                msft_add,
                cancel(200, 25),
                msft_cancel,
                cancel(400, 10),
            ]
            .concat(),
            true,
        )
        .unwrap();
    replay.advance(200, 100).unwrap();
    let checkpoint = (
        replay.state().clock_ns,
        replay.state().messages,
        replay.state().consumed,
        replay.buffered(),
        replay.state().start_ns,
    );
    assert_eq!(replay.tickers(), ["AAPL", "MSFT"]);
    assert_eq!(replay.state().instruments.len(), 2);
    replay.select_ticker(" msft ").unwrap();
    assert_eq!(bid(&replay).unwrap(), 60);
    assert_eq!(
        replay.state().outputs[&replay.state().selected]
            .borrow()
            .count,
        2
    );
    assert_eq!(
        checkpoint,
        (
            replay.state().clock_ns,
            replay.state().messages,
            replay.state().consumed,
            replay.buffered(),
            replay.state().start_ns
        )
    );
    assert!(replay.select_ticker("MISSING").is_err());
    assert_eq!(replay.state().selected, "MSFT");
    replay.advance(300, 100).unwrap();
    replay.select_ticker("AAPL").unwrap();
    assert_eq!(bid(&replay).unwrap(), 65);
    assert!(replay.state().complete);
}
#[test]
fn directory_lists_inactive_tickers_and_selection_does_not_require_orders() {
    let mut body = vec![0; 28];
    body[..8].copy_from_slice(b"ZZZ     ");
    let mut directory = record(b'R', 50, &body);
    directory[3..5].copy_from_slice(&3u16.to_be_bytes());
    let mut replay = Replay::new("AAPL", 0).unwrap();
    replay
        .append(&[directory, add(100)].concat(), true)
        .unwrap();
    replay.advance(0, 100).unwrap();
    assert_eq!(replay.tickers(), ["AAPL", "ZZZ"]);
    replay.select_ticker("ZZZ").unwrap();
    assert!(!replay.state().synchronized("ZZZ"));
    assert_eq!(replay.state().instruments.len(), 2);
}

#[test]
fn priced_execution_bytes_feed_actual_prices_into_volume_bars() {
    let mut body = 1u64.to_be_bytes().to_vec();
    body.extend_from_slice(&25u32.to_be_bytes());
    body.extend_from_slice(&1u64.to_be_bytes());
    body.push(b'Y');
    body.extend_from_slice(&990000u32.to_be_bytes());
    let mut replay = Replay::new("AAPL", 0).unwrap();
    replay
        .append(&[add(100), record(b'C', 200, &body)].concat(), true)
        .unwrap();
    replay.advance(1000, 100).unwrap();
    let bars = replay.state().volume_bars().borrow();
    let bar = bars.forming("AAPL").unwrap();
    assert_eq!(u64::from(bar.open), 990000);
    assert_eq!(bar.volume, 25);
    assert_eq!(bid(&replay), Some(75));
}

#[test]
fn l3_market_preview_reports_unfilled_and_preserves_main_book_and_events() {
    use lobo_models::{Side, orders::order_types::MarketOrder};
    use lobo_primitives::{Price64, uuid::Uuid};
    let mut replay = Replay::new("AAPL", 0).unwrap();
    replay.append(&add(100), true).unwrap();
    replay.advance(0, 100).unwrap();
    let sequence = replay.selected_book().map(native).unwrap().sequence();
    replay
        .simulate_market(MarketOrder::<Price64>::new(150, Uuid::nil(), Side::Sell))
        .unwrap();
    let report = replay.state().market_preview.as_ref().unwrap();
    assert_eq!(
        (report.filled, report.remaining(), report.average_price()),
        (100, 50, Some(1000000.0))
    );
    assert_eq!(report.executions[0].timestamp_ns(), 100);
    assert!(report.price.is_none());
    assert!(replay.state().simulation.is_none());
    assert_eq!(bid(&replay), Some(100));
    assert_eq!(
        replay.selected_book().map(native).unwrap().sequence(),
        sequence
    );
    assert_eq!(replay.state().volume_bars().borrow().completed("AAPL"), 0);
    replay.return_to_main();
    assert!(replay.state().market_preview.is_none());
}

#[test]
fn replay_redirects_worse_prices_but_live_respects_native_fifo() {
    use lobo_context::FeedMode;
    use lobo_models::{
        Side,
        orders::{order_types::LimitOrder, traits::Trades},
    };
    use lobo_primitives::{Price64, uuid::Uuid};
    use lobo_replay::simulation::Simulation;
    for side in [Side::Buy, Side::Sell] {
        let mut record = add(100);
        record[21] = if side == Side::Buy { b'B' } else { b'S' };
        let mut replay = Replay::new("AAPL", 0).unwrap();
        replay.append(&record, true).unwrap();
        replay.advance(0, 100).unwrap();
        let order = LimitOrder::<Price64>::new(Some(1000000u64), 50, Uuid::nil(), side);
        let id = order.uuid();
        let mut live = Simulation::start(replay.state(), order.clone(), FeedMode::Live).unwrap();
        let mut branch = Simulation::start(replay.state(), order, FeedMode::Replay).unwrap();
        let worse = if side == Side::Buy {
            999900u64
        } else {
            1000100u64
        };
        // Same-price flow honors the queue, including orders ahead of the user.
        branch.execute(200, side, 1000000u64.into(), 10);
        assert_eq!(branch.remaining(), 50);
        branch.execute(300, side, worse.into(), 20);
        assert_eq!(branch.remaining(), 30);
        assert_eq!(
            branch.report.executions[0].event().execution.price,
            1000000u64.into()
        );
        assert_eq!(
            branch
                .book_mut()
                .order(Uuid::from_u128(1))
                .unwrap()
                .quantity(),
            90
        );
        // Live does not bypass older makers when a trade arrives at a worse price.
        live.execute(300, side, worse.into(), 20);
        assert_eq!(live.remaining(), 50);
        assert_eq!(
            live.book_mut()
                .order(Uuid::from_u128(1))
                .unwrap()
                .quantity(),
            80
        );
        branch.execute(400, side, worse.into(), 100);
        assert!(branch.complete());
        assert!(
            native_mut(branch.book_mut())
                .order_storage
                .order(id)
                .is_none()
        );
        branch.execute(500, side, worse.into(), 100);
        assert_eq!(branch.feed.clock_ns, 400);
        assert_eq!(branch.report.filled, 50);
        assert_eq!(
            branch
                .book_mut()
                .order(Uuid::from_u128(1))
                .unwrap()
                .quantity(),
            90
        );
        live.execute(400, side, 1000000u64.into(), 100);
        assert_eq!(live.remaining(), 30);
        live.execute(500, side, 1000000u64.into(), 30);
        assert!(live.complete());
        // Neither branch touched the original book.
        assert_eq!(
            replay
                .selected_book()
                .map(native)
                .unwrap()
                .order_storage
                .order(Uuid::from_u128(1))
                .unwrap()
                .quantity(),
            100
        );
    }
}

#[test]
fn replacement_moves_to_queue_tail_with_source_arrival_timestamp() {
    use lobo_models::Side;
    use lobo_primitives::{Price64, uuid::Uuid};
    let first = add(100);
    let mut second = add(200);
    second[13..21].copy_from_slice(&2u64.to_be_bytes());
    let mut body = 1u64.to_be_bytes().to_vec();
    body.extend(3u64.to_be_bytes());
    body.extend(100u32.to_be_bytes());
    body.extend(1000000u32.to_be_bytes());
    let mut replay = Replay::new("AAPL", 0).unwrap();
    replay
        .append(&[first, second, record(b'U', 300, &body)].concat(), true)
        .unwrap();
    replay.advance(1000, 100).unwrap();
    let queue = replay
        .selected_book()
        .map(native)
        .unwrap()
        .order_storage
        .queue_view(
            Side::Buy,
            Price64::from(1000000u64)..=Price64::from(1000000u64),
        );
    assert_eq!(
        queue.iter().map(|o| o.id).collect::<Vec<_>>(),
        vec![Uuid::from_u128(2), Uuid::from_u128(3)]
    );
    assert_eq!(
        queue
            .iter()
            .map(|o| o.created_at.timestamp_nanos_opt())
            .collect::<Vec<_>>(),
        vec![Some(200), Some(300)]
    );
}

#[test]
fn scope_routes_only_selected_books_but_preserves_directory_and_source_clock() {
    use lobo_context::BookScope;
    let mut other = add(100);
    other[3..5].copy_from_slice(&2u16.to_be_bytes());
    other[26..34].copy_from_slice(b"MSFT    ");
    let bytes = [other, add(200), cancel(300, 40)].concat();
    let mut replay = Replay::new("AAPL", 250).unwrap();
    replay
        .set_book_scope(BookScope::Selected(["AAPL".into()].into()))
        .unwrap();
    replay.append(&bytes, true).unwrap();
    replay.advance(0, 100).unwrap();
    assert_eq!(replay.state().start_ns, Some(250));
    assert_eq!(replay.state().clock_ns, 250);
    assert_eq!(replay.tickers(), ["AAPL", "MSFT"]);
    assert_eq!(replay.scoped_tickers(), ["AAPL"]);
    assert!(replay.state().context.get("MSFT").is_none());
    assert_eq!(bid(&replay), Some(100));
    assert!(replay.select_ticker("MSFT").is_err());
    assert!(replay.set_book_scope(BookScope::All).is_err());
    replay.advance(1000, 100).unwrap();
    assert_eq!(bid(&replay), Some(60));
    assert_eq!(replay.state().messages, 3);
    assert_eq!(replay.state().consumed, bytes.len() as u64);
    assert!(replay.state().complete);
}
