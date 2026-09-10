#![cfg(all(feature = "jit", not(target_arch = "wasm32")))]
use lobo_models::orders::traits::Trades;
use lobo_primitives::uuid::Uuid;
use lobo_replay::custom::{
    AdapterDescriptor, BookLevel, CustomAdapter, FeedMode, MarketDataAdapter,
    definition::{DefinedProtocol, schema::Definition},
};
use serde_json::{Value, json};
use std::sync::Arc;

fn literal(value: Value) -> Value {
    json!({"op":"literal","value":value})
}
fn field(name: &str) -> Value {
    json!({"op":"field","path":[name]})
}
fn op(name: &str, args: Vec<Value>) -> Value {
    json!({"op":name,"args":args})
}
fn variable(name: &str) -> Value {
    json!({"op":"variable","name":name})
}
fn adapter(actions: Value) -> CustomAdapter<DefinedProtocol> {
    adapter_with_precision(actions, 2, 0)
}
fn adapter_with_precision(
    actions: Value,
    price: u8,
    quantity: u8,
) -> CustomAdapter<DefinedProtocol> {
    let spec = json!({
        "format":{"format":"json","messages":[{"condition":literal(json!(true)),"actions":actions}]},
        "connect":[{"action":"register","symbol":literal(json!("BOOK")),"price_decimals":literal(json!(price)),"quantity_decimals":literal(json!(quantity)),"key":literal(Value::Null),"policy":literal(json!("full"))}],
        "subscriptions":[],"bootstrap":[],"reconcile_window_ns":1000000,"reconcile_capacity":32,"symbols_per_connection":0
    });
    let info = AdapterDescriptor {
        id: "test".into(),
        name: "Test".into(),
        mode: FeedMode::Live,
        level: BookLevel::L3,
        default_symbol: "BOOK".into(),
        endpoint: None,
        timezone: "UTC".into(),
        supports_trades: true,
    };
    let protocol = DefinedProtocol::new(
        Arc::new(Definition::parse(&spec.to_string()).unwrap()),
        info.clone(),
        "BOOK",
    )
    .unwrap();
    let mut adapter = CustomAdapter::new(info, protocol, "BOOK").unwrap();
    adapter.connected().unwrap();
    adapter
}
fn book(actions: Value) -> Value {
    json!({"action":"book","symbol":literal(json!("BOOK")),"snapshot":literal(json!(true)),"timestamp":literal(json!(100)),"depth":0,"ready":true,"checksum":null,"actions":actions})
}
fn add() -> Value {
    json!({"action":"add","id":field("id"),"side":literal(json!("buy")),"quantity":field("quantity"),"price":field("price"),"timestamp":literal(json!(100))})
}

#[test]
fn changing_a_variable_does_not_invalidate_an_active_collection() {
    let mut adapter = adapter(json!([
        {"action":"let","name":"rows","value":field("rows")},
        book(json!([{"action":"for_each","items":variable("rows"),"order_by":null,"unique_by":null,"actions":[
            {"action":"let","name":"rows","value":literal(Value::Null)},add()
        ]}]))
    ]));
    adapter
        .receive(
            br#"{"rows":[{"id":1,"quantity":7,"price":100},{"id":2,"quantity":9,"price":101}]}"#,
            false,
        )
        .unwrap();
    let book = adapter.state().book("BOOK").unwrap();
    assert_eq!(book.order(Uuid::from_u128(1)).unwrap().quantity(), 7);
    assert_eq!(book.order(Uuid::from_u128(2)).unwrap().quantity(), 9);
}

#[test]
fn fixed_item_lists_are_evaluated_before_their_actions() {
    let mut adapter = adapter(json!([
        {"action":"let","name":"second","value":field("second")},
        book(json!([{"action":"for_each","items":{"op":"array","args":[field("first"),variable("second")]},"order_by":null,"unique_by":null,"actions":[
            {"action":"let","name":"second","value":literal(Value::Null)},add()
        ]}]))
    ]));
    adapter.receive(br#"{"first":{"id":1,"quantity":7,"price":100},"second":{"id":2,"quantity":9,"price":101}}"#,false).unwrap();
    assert_eq!(
        adapter
            .state()
            .book("BOOK")
            .unwrap()
            .order(Uuid::from_u128(2))
            .unwrap()
            .quantity(),
        9
    );
}

#[test]
fn raw_command_projection_preserves_wide_ids_and_quantities() {
    let mut adapter = adapter(json!([book(
        json!([{"action":"order_command","value":field("command"),"timestamp":literal(json!(100))}])
    )]));
    adapter.receive(br#" { "ignored": "escaped \" text", "command": { "order": { "quantity": 9007199254740993, "side":"buy", "price":100, "id":"ffffffff-ffff-ffff-ffff-ffffffffffff", "type":"limit" }, "op":"add" } } "#,false).unwrap();
    let order = adapter
        .state()
        .book("BOOK")
        .unwrap()
        .order(Uuid::from_u128(u128::MAX))
        .unwrap();
    assert_eq!(order.quantity(), 9_007_199_254_740_993);
}

#[test]
fn raw_parser_rejects_truncated_input_and_invalid_number_syntax() {
    for bytes in [
        br#"{"command": "#.as_slice(),
        br#"{"number":01}"#.as_slice(),
    ] {
        let mut adapter = adapter(json!([]));
        assert!(adapter.receive(bytes, false).is_err());
    }
}

#[test]
fn discarding_events_does_not_allow_an_unregistered_order_symbol() {
    let mut adapter = adapter(json!([
        {"action":"let","name":"symbol","value":literal(json!("UNKNOWN"))},
        add()
    ]));
    let error = adapter
        .receive(br#"{"id":1,"quantity":7,"price":100}"#, false)
        .unwrap_err();
    assert!(error.contains("Order before instrument"), "{error}");
    assert!(adapter.state().book("UNKNOWN").is_none());
}

#[test]
fn generated_scalar_expressions_match_reference_semantics() {
    use lobo_replay::custom::definition::expression::{Expr, Input};
    use std::collections::BTreeMap;
    let input = json!({"p":" 1.25 ", "n":-7, "items":[2,4,8], "object":{"a":1,"b":2}, "id":u64::MAX,"price":-0.00000001,"scale":8,"text":"  tBTCUSD  ","time":"2026-09-08T13:30:00Z","name":"known"});
    let mut exprs = vec![
        field("object"),
        field("items"),
        op("eq", vec![field("object"), literal(json!({"a":1,"b":2}))]),
        op("eq", vec![field("items"), literal(json!([2, 4, 8]))]),
        op("eq", vec![field("items"), literal(json!([2, 5, 8]))]),
        op(
            "decimal",
            vec![op("trim", vec![field("p")]), literal(json!(2))],
        ),
        op("abs", vec![literal(json!("--12"))]),
        op("abs", vec![field("n")]),
        op(
            "concat",
            vec![
                literal(json!("a")),
                op("concat", vec![literal(json!("b")), literal(json!("c"))]),
                literal(json!("d")),
            ],
        ),
        op("length", vec![field("items")]),
        op("length", vec![field("object")]),
        op("get", vec![field("items"), literal(json!([-1]))]),
        op(
            "map",
            vec![
                literal(json!("known")),
                literal(json!({"known":null})),
                op(
                    "decimal",
                    vec![literal(json!("invalid")), literal(json!(2))],
                ),
            ],
        ),
        op(
            "choose",
            vec![
                literal(json!(false)),
                op(
                    "decimal",
                    vec![literal(json!("invalid")), literal(json!(2))],
                ),
                literal(json!(9)),
            ],
        ),
    ];
    let bad_number = op("decimal", vec![literal(json!("bad")), literal(json!(8))]);
    let absolute = op("abs", vec![field("price")]);
    exprs.extend(vec![
        literal(json!(null)),
        literal(json!(u64::MAX)),
        field("id"),
        field("missing"),
        json!({"op":"field","path":["items",-1]}),
        json!({"op":"field","path":["items",-99]}),
        json!({"op":"root","path":["id"]}),
        json!({"op":"variable","name":"symbol"}),
        op(
            "lookup",
            vec![literal(json!("channels")), literal(json!(9))],
        ),
        op("get", vec![field("items"), literal(json!([-1]))]),
        op(
            "map",
            vec![
                field("name"),
                literal(json!({"known":null})),
                bad_number.clone(),
            ],
        ),
        op(
            "map",
            vec![
                literal(json!("absent")),
                literal(json!({"known":null})),
                literal(json!("fallback")),
            ],
        ),
        op("and", vec![literal(json!(false)), bad_number.clone()]),
        op("or", vec![literal(json!(true)), bad_number.clone()]),
        op(
            "choose",
            vec![
                literal(json!(false)),
                bad_number.clone(),
                literal(json!(u64::MAX)),
            ],
        ),
        op(
            "choose",
            vec![literal(json!(true)), literal(json!(7)), bad_number.clone()],
        ),
        op("eq", vec![field("id"), literal(json!(u64::MAX))]),
        op(
            "ne",
            vec![field("id"), literal(json!(u64::MAX - 1))],
        ),
        op(
            "gt",
            vec![field("id"), literal(json!(u64::MAX - 1))],
        ),
        op("lt", vec![field("price"), literal(json!(0))]),
        op("exists", vec![field("missing")]),
        op("is_array", vec![field("items")]),
        op("length", vec![field("text")]),
        absolute.clone(),
        op("decimal", vec![absolute, field("scale")]),
        op("trim", vec![field("text")]),
        op(
            "strip_prefix",
            vec![literal(json!("tBTCUSD")), literal(json!("t"))],
        ),
        op(
            "timestamp",
            vec![field("time"), literal(json!("rfc3339"))],
        ),
        op(
            "timestamp",
            vec![literal(json!(12345)), literal(json!("ms"))],
        ),
        op("timestamp", vec![field("id"), literal(json!("s"))]),
        op("array", vec![field("id"), literal(json!(true))]),
        json!({"op":"object","path":["id","symbol"],"args":[field("id"), literal(json!("BTCUSD"))]}),
        op(
            "concat",
            vec![
                literal(json!("t")),
                json!({"op":"variable","name":"symbol"}),
            ],
        ),
        bad_number,
    ]);
    for name in ["eq", "ne", "lt", "gt"] {
        for (left, right) in [
            ("1.00", "1e0"),
            ("-1e-30", "-0.000000000000000000000000000002"),
            ("-0.0", "0"),
        ] {
            exprs.push(op(
                name,
                vec![
                    literal(serde_json::from_str(left).unwrap()),
                    literal(serde_json::from_str(right).unwrap()),
                ],
            ));
        }
    }
    let vars = BTreeMap::from([
        ("connection".into(), json!(0)),
        ("symbol".into(), json!("BTCUSD")),
    ]);
    let tables = BTreeMap::from([(
        0,
        BTreeMap::from([(
            "channels".into(),
            BTreeMap::from([("9".into(), json!({"symbol":"BTCUSD"}))]),
        )]),
    )]);
    for expr in exprs {
        let parsed: Expr = serde_json::from_value(expr.clone()).unwrap();
        let expected = parsed.eval(&Input {
            item: &input,
            root: &input,
            vars: &vars,
            tables: &tables,
        });
        let mut adapter = adapter(json!([
            {"action":"let","name":"symbol","value":literal(json!("BTCUSD"))},
            {"action":"remember","table":"channels","key":literal(json!(9)),"value":literal(json!({"symbol":"BTCUSD"}))},
            {"action":"send","message":expr}
        ]));
        let actual = adapter
            .receive(input.to_string().as_bytes(), false)
            .map(|_| {
                let messages = adapter.commands();
                serde_json::from_str::<Value>(&messages[0]).unwrap()
            });
        assert_eq!(actual.ok(), expected.ok(), "{expr}");
    }
}

#[test]
fn compiled_upserts_preserve_exact_fields_and_optional_prices() {
    let mut adapter = adapter_with_precision(
        json!([{"action":"upsert", "id":field("ticket"),
                "side":op("choose",vec![op("gt",vec![field("amount"),literal(json!(0))]),literal(json!("buy")),literal(json!("sell"))]),
                "price":op("choose",vec![op("eq",vec![field("rate"),literal(json!(0))]),literal(Value::Null),op("decimal",vec![field("rate"),literal(json!(8))])]),
                "quantity":op("decimal",vec![op("abs",vec![field("amount")]),literal(json!(8))]),
                "timestamp":op("timestamp",vec![field("time"),literal(json!("ms"))])
        }]),
        8,
        8,
    );
    for (price, expected_price) in [("79419.12345678", Some(7_941_912_345_678u64)), ("0", None)] {
        let bytes = format!(
            r#"{{"ticket":"ffffffff-ffff-ffff-ffff-ffffffffffff","amount":-0.00000001,"rate":{price},"time":1788874200123}}"#
        );
        adapter.receive(bytes.as_bytes(), false).unwrap();
        let order = adapter
            .state()
            .book("BOOK")
            .unwrap()
            .order(Uuid::from_u128(u128::MAX));
        if let Some(price) = expected_price {
            let order = order.unwrap();
            assert_eq!(order.uuid().as_u128(), u128::MAX);
            assert_eq!(order.side(), lobo_models::Side::Sell);
            assert_eq!(order.quantity(), 1);
            assert_eq!(order.price().map(u64::from), Some(price));
            assert_eq!(
                order.common_data.creation_time.timestamp_nanos_opt(),
                Some(1_788_874_200_123_000_000)
            );
        } else {
            assert!(order.is_none());
        }
    }
}
