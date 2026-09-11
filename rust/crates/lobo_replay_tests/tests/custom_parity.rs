#![cfg(all(feature = "json", feature = "order-api"))]
#[path = "../../lobo_replay/tests/support/definitions.rs"]
mod examples;
use lobo_adapters::adapter::MarketDataAdapter as Original;
use lobo_models::{
    Side,
    orders::order_types::{LimitOrder, MarketOrder},
};
use lobo_primitives::{Price64, uuid::Uuid};
use lobo_replay::custom::MarketDataAdapter;
use serde_json::{Value, json};

#[path = "../../lobo_replay/tests/support/observe.rs"]
mod observe;
use observe::market as state;
struct Pair {
    old: Box<dyn Original>,
    custom: Box<dyn MarketDataAdapter>,
}
impl Pair {
    fn check(&self) {
        assert_eq!(state(self.old.state()), state(self.custom.state()));
    }
    fn receive(&mut self, bytes: &[u8], eof: bool) {
        assert_eq!(
            self.old.receive(bytes, eof).is_ok(),
            self.custom.receive(bytes, eof).is_ok()
        );
        self.check();
    }
    fn json(&mut self, value: Value) {
        self.receive(value.to_string().as_bytes(), false);
    }
    fn commands(&mut self) {
        assert_eq!(self.old.commands(), self.custom.commands());
    }
    fn connected(&mut self) {
        assert_eq!(self.old.connected(), self.custom.connected());
        self.check();
        self.commands();
    }
    fn disconnected(&mut self) {
        self.old.disconnected();
        self.custom.disconnected();
        self.check();
    }
    fn advance(&mut self, elapsed: u64, budget: usize) {
        assert_eq!(
            self.old.advance(elapsed, budget),
            self.custom.advance(elapsed, budget)
        );
        self.check();
    }
    fn limit(&mut self, price: u64, quantity: u64, side: Side) {
        let order = LimitOrder::new(Some(Price64::from(price)), quantity, Uuid::nil(), side);
        assert_eq!(
            self.old.simulate(order.clone()),
            self.custom.simulate(order)
        );
        self.check();
    }
    fn market(&mut self, quantity: u64, side: Side) {
        let order = || {
            let mut o = MarketOrder::<Price64>::new(quantity, Uuid::nil(), side);
            o.common_data.uuid = Uuid::from_u128(999);
            o
        };
        assert_eq!(
            self.old.simulate_market(order()),
            self.custom.simulate_market(order())
        );
        self.check();
    }
}
fn record(tag: u8, timestamp: u64, body: &[u8]) -> Vec<u8> {
    let mut bytes = ((11 + body.len()) as u16).to_be_bytes().to_vec();
    bytes.push(tag);
    bytes.extend(1u16.to_be_bytes());
    bytes.extend(0u16.to_be_bytes());
    bytes.extend(&timestamp.to_be_bytes()[2..]);
    bytes.extend(body);
    bytes
}
fn add(timestamp: u64, id: u64, side: u8, quantity: u32, price: u32) -> Vec<u8> {
    let mut body = id.to_be_bytes().to_vec();
    body.push(side);
    body.extend(quantity.to_be_bytes());
    body.extend(b"AAPL    ");
    body.extend(price.to_be_bytes());
    record(b'A', timestamp, &body)
}
#[test]
fn itch_fragmentation_clock_events_fifo_and_counterfactual_execution() {
    let mut pair = Pair {
        old: Box::new(lobo_adapters::itch::stream::ItchStream::new("AAPL", 0).unwrap()),
        custom: Box::new(examples::itch::new("AAPL", 0).unwrap()),
    };
    let mut execution = 2u64.to_be_bytes().to_vec();
    execution.extend(60u32.to_be_bytes());
    execution.extend(1u64.to_be_bytes());
    let bytes = [
        add(100, 1, b'B', 100, 10000),
        add(100, 2, b'S', 100, 10100),
        record(b'E', 200, &execution),
        record(
            b'E',
            300,
            &[
                2u64.to_be_bytes().as_slice(),
                40u32.to_be_bytes().as_slice(),
                2u64.to_be_bytes().as_slice(),
            ]
            .concat(),
        ),
    ]
    .concat();
    for fragment in bytes.chunks(7) {
        pair.receive(fragment, false);
        pair.advance(0, 3);
    }
    pair.receive(&[], true);
    pair.advance(0, 100);
    pair.market(50, Side::Buy);
    pair.limit(10100, 150, Side::Sell);
    pair.advance(100, 100);
    pair.advance(200, 100);
    pair.advance(u64::MAX, 100);
    pair.old.return_to_main();
    pair.custom.return_to_main();
    pair.check();
}
#[test]
fn kraken_snapshot_precision_checksum_resubscription_and_trade_bars() {
    let mut pair = Pair {
        old: Box::new(lobo_adapters::kraken::Kraken::new("BTC/USD", 10).unwrap()),
        custom: Box::new(examples::kraken::new("BTC/USD", 10).unwrap()),
    };
    pair.connected();
    pair.json(json!({"channel":"instrument","type":"snapshot","data":{"pairs":[{"symbol":"BTC/USD","price_precision":1,"qty_precision":8,"status":"online"},{"symbol":"ETH/USD","price_precision":2,"qty_precision":8,"status":"online"}]}}));
    pair.commands();
    let snapshot = include_bytes!("../../lobo_adapters/tests/fixtures/kraken_snapshot.json");
    pair.receive(snapshot, false);
    pair.market(100_000_000, Side::Buy);
    pair.receive(
        include_bytes!("../../lobo_adapters/tests/fixtures/kraken_update.json"),
        false,
    );
    pair.json(json!({"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","trade_id":1,"timestamp":"2024-01-01T00:00:01Z","side":"buy","price":"45283.5","qty":"5001.0"}]}));
    let mut corrupt: Value = serde_json::from_slice(snapshot).unwrap();
    corrupt["data"][0]["checksum"] = 0.into();
    pair.json(corrupt);
    pair.commands();
    pair.receive(snapshot, false);
    pair.disconnected();
    pair.connected();
}
#[test]
fn bitfinex_fifo_checksum_trade_reconciliation_and_sequence_gap() {
    let mut pair = Pair {
        old: Box::new(lobo_adapters::bitfinex::Bitfinex::new("BTCUSD").unwrap()),
        custom: Box::new(examples::bitfinex::new("BTCUSD").unwrap()),
    };
    let directory = br#"[["BTCUSD","ETHUSD"]]"#;
    assert_eq!(
        pair.old.bootstrap("instruments", directory),
        pair.custom.bootstrap("instruments", directory)
    );
    pair.check();
    pair.connected();
    for message in [
        json!({"event":"conf","status":"OK","flags":229376}),
        json!({"event":"subscribed","chanId":1,"channel":"book","prec":"R0","symbol":"tBTCUSD"}),
        json!({"event":"subscribed","chanId":2,"channel":"trades","symbol":"tBTCUSD"}),
    ] {
        pair.json(message);
    }
    pair.commands();
    let t = 1_700_000_000_000u64;
    pair.json(json!([
        1,
        [[20, 100, 2], [30, 101, -3], [10, 100, 1]],
        1,
        t
    ]));
    pair.json(json!([
        1,
        "cs",
        crc32fast::hash(b"10:1:30:-3:20:2") as i32,
        2,
        t
    ]));
    pair.limit(100 * 100_000_000, 100_000_000, Side::Buy);
    pair.json(json!([1, [10, 0, 1], 3, t + 10]));
    pair.json(json!([2, "te", [1, t + 10, -4, 100], 4, t + 10]));
    pair.json(json!([2, "tu", [1, t + 10, -4, 100], 5, t + 10]));
    pair.advance(1_000_000_000, 100);
    pair.json(json!([1, "hb", 9, t + 1000]));
    pair.disconnected();
}
#[test]
fn server_all_policies_snapshot_replenishment_and_sequence() {
    use lobo_models::{
        BookPolicy,
        server::{BookInfo, Command, FeedMessage, IcebergOrder, Order, OrderFields},
    };
    for policy in [
        BookPolicy::Full,
        BookPolicy::NoUserMap,
        BookPolicy::NoHiddenQuantity,
        BookPolicy::NoUpdates,
    ] {
        let mut pair = Pair {
            old: Box::new(lobo_replay::order_messages::OrderMessages::new("BOOK").unwrap()),
            custom: Box::new(examples::server::new("BOOK").unwrap()),
        };
        let book = BookInfo {
            symbol: "BOOK".into(),
            price_decimals: 4,
            quantity_decimals: 0,
            policy,
        };
        pair.json(
            serde_json::to_value(FeedMessage::Directory {
                books: vec![book.clone()],
            })
            .unwrap(),
        );
        pair.json(
            serde_json::to_value(FeedMessage::Snapshot {
                book,
                sequence: 0,
                timestamp_ns: 100,
                orders: vec![],
            })
            .unwrap(),
        );
        let order = Order::Iceberg(IcebergOrder {
            fields: OrderFields {
                id: Uuid::from_u128(1),
                trader: Uuid::nil(),
                side: Side::Buy,
                quantity: 10,
            },
            price: 10000,
            hidden_quantity: 20,
            peak_quantity: 10,
        });
        for (sequence, command) in [
            Command::Add { order },
            Command::Execute {
                id: Uuid::from_u128(1),
                quantity: 10,
                price: None,
            },
        ]
        .into_iter()
        .enumerate()
        {
            pair.json(
                serde_json::to_value(FeedMessage::Update {
                    book: "BOOK".into(),
                    sequence: sequence as u64 + 1,
                    timestamp_ns: 101 + sequence as u64,
                    command,
                })
                .unwrap(),
            );
        }
        pair.disconnected();
    }
}

#[test]
fn server_directory_replacement_removes_old_books_and_accepts_new_generations() {
    let mut pair = Pair {
        old: Box::new(lobo_replay::order_messages::OrderMessages::new("BOOK").unwrap()),
        custom: Box::new(examples::server::new("BOOK").unwrap()),
    };
    let book = json!({"symbol":"BOOK","price_decimals":4,"quantity_decimals":0,"policy":"full"});
    for sequence in [100, 0] {
        pair.json(json!({"type":"directory","books":[book.clone()]}));
        pair.json(json!({"type":"snapshot","book":book.clone(),"sequence":sequence,"timestamp_ns":100,"orders":[]}));
        pair.json(json!({"type":"directory","books":[]}));
        assert!(pair.custom.state().instruments.is_empty());
        assert!(pair.custom.state().book("BOOK").is_none());
    }
}
