#![cfg(all(feature = "json", feature = "native"))]
//! Generic wire fixtures deliberately have no exchange-specific decoder.
#[path = "support/definitions.rs"]
mod definitions;
use lobo_replay::custom::{
    CustomAdapter, MarketDataAdapter,
    definition::{DefinedProtocol, schema::Definition},
};
use serde_json::{Value, json};
use std::sync::Arc;

fn uint(offset: usize) -> Value {
    json!({"type":"uint", "offset":offset, "size":1, "byteorder":"little"})
}
fn group(name: &str) -> Value {
    json!({"name":name, "header_size":2, "block_length":uint(0), "count":uint(1),
        "fields":{"value":uint(0)}})
}
fn spec(record: Value) -> Definition {
    Definition::parse(
        &json!({
            "format":{"format":"binary", "length":uint(0), "length_includes_prefix":true,
                "tag":uint(0), "key":uint(1), "timestamp":uint(2),
                "max_record_size":255, "records":{"1":record}},
            "connect":[], "subscriptions":[], "bootstrap":[], "reconcile_capacity":32,
            "reconcile_window_ns":1000, "symbols_per_connection":0
        })
        .to_string(),
    )
    .unwrap()
}
fn adapter(record: Value) -> CustomAdapter<DefinedProtocol> {
    let info = definitions::descriptor("binary-test", "BOOK");
    let protocol = DefinedProtocol::new(Arc::new(spec(record)), info.clone(), "BOOK").unwrap();
    CustomAdapter::new(info, protocol, "BOOK").unwrap()
}
fn base_record(groups: Value) -> Value {
    json!({"size":null, "fields":{"root":uint(3)}, "groups":groups,
        "actions":[{"action":"send", "message":{"op":"field", "path":[]}}]})
}
fn run(record: Value, payload: &[u8]) -> Result<Vec<Value>, String> {
    let mut adapter = adapter(record);
    let mut bytes = vec![(payload.len() + 1) as u8];
    bytes.extend(payload);
    // Exercise prefix fragmentation and bounded work without an exchange adapter.
    for byte in bytes {
        adapter.receive(&[byte], false)?;
        adapter.advance(u64::MAX, 1)?;
    }
    adapter.receive(&[], true)?;
    adapter.advance(u64::MAX, 100)?;
    Ok(adapter
        .commands()
        .into_iter()
        .map(|s| serde_json::from_str(&s).unwrap())
        .collect())
}

#[test]
fn sequential_nested_groups_use_transmitted_block_lengths() {
    let mut outer = group("outer");
    outer["groups"] = json!([group("inner")]);
    let record = base_record(json!([outer, group("next")]));
    let result = run(
        record,
        &[
            1, 0, 1, 42, 2, 2, 10, 255, 1, 2, 11, 12, 20, 255, 1, 0, 1, 1, 30,
        ],
    )
    .unwrap();
    assert_eq!(
        result,
        vec![json!({"root":42,
        "outer":[{"value":10,"inner":[{"value":11},{"value":12}]},
                 {"value":20,"inner":[]}], "next":[{"value":30}]})]
    );
}

#[test]
fn zero_groups_and_alignment_preserve_following_offsets() {
    let mut a = group("a");
    a["offset"] = json!(5);
    a["alignment"] = json!(4);
    let result = run(
        base_record(json!([a, group("b")])),
        &[1, 0, 1, 42, 255, 255, 255, 255, 1, 0, 1, 1, 90],
    )
    .unwrap();
    assert_eq!(result, vec![json!({"root":42,"a":[],"b":[{"value":90}]})]);
}

#[test]
fn malformed_dimensions_and_trailing_data_fail_without_actions() {
    for (payload, error) in [
        (vec![1, 0, 1, 42, 1], "Truncated binary group header"),
        (vec![1, 0, 1, 42, 0, 1], "block length is too small"),
        (
            vec![1, 0, 1, 42, 1, 2, 10],
            "Truncated binary group entries",
        ),
        (vec![1, 0, 1, 42, 1, 0, 10], "undeclared trailing bytes"),
    ] {
        let mut adapter = adapter(base_record(json!([group("a")])));
        let mut bytes = vec![(payload.len() + 1) as u8];
        bytes.extend(payload);
        adapter.receive(&bytes, true).unwrap();
        assert!(adapter.advance(u64::MAX, 100).unwrap_err().contains(error));
        assert!(adapter.commands().is_empty());
    }
}

#[test]
fn explicit_group_limits_and_offsets_are_enforced() {
    let mut limited = group("a");
    limited["max_count"] = json!(1);
    assert!(
        run(base_record(json!([limited])), &[1, 0, 1, 42, 1, 2, 10, 20])
            .unwrap_err()
            .contains("count exceeds limit")
    );
    let mut overlap = group("a");
    overlap["offset"] = json!(2);
    assert!(
        run(base_record(json!([overlap])), &[1, 0, 1, 42, 1, 0])
            .unwrap_err()
            .contains("overlaps")
    );
}

#[test]
fn variable_root_extensions_and_explicit_trailing_policy() {
    let mut record = base_record(json!([group("a")]));
    record["block_length"] = uint(3);
    record["block_offset"] = json!(4);
    assert_eq!(
        run(record.clone(), &[1, 0, 1, 2, 255, 255, 1, 1, 10]).unwrap(),
        vec![json!({"root":2,"a":[{"value":10}]})]
    );
    assert!(
        run(record.clone(), &[1, 0, 1, 50])
            .unwrap_err()
            .contains("root block")
    );
    record["allow_trailing"] = json!(true);
    assert!(run(record, &[1, 0, 1, 0, 1, 0, 255]).is_ok());
}

#[test]
fn prefix_limits_use_payload_size_and_checked_subtraction() {
    let definition = spec(base_record(json!([])));
    let lobo_replay::custom::definition::schema::Format::Binary(binary) = definition.format else {
        panic!()
    };
    assert_eq!(binary.payload_length(256).unwrap(), 255);
    for encoded in [0, 1, 257, u64::MAX] {
        assert!(binary.payload_length(encoded).is_err());
    }
}

#[test]
fn later_groups_resolve_references_and_retain_the_message_root() {
    let mut levels = group("levels");
    levels["fields"] = json!({"reference":uint(0), "price":uint(1)});
    let mut orders = group("orders");
    orders["fields"] = json!({"reference":uint(0), "quantity":uint(1)});
    let mut record = base_record(json!([levels, orders]));
    let field = |name| json!({"op":"field", "path":[name]});
    let lookup =
        json!({"op":"lookup", "args":[{"op":"literal","value":"levels"},field("reference")]});
    record["actions"] = json!([
        {"action":"for_each", "items":field("levels"), "actions":[
            {"action":"remember", "table":"levels", "key":field("reference"), "value":field("price")}
        ]},
        {"action":"for_each", "items":field("orders"), "actions":[
            {"action":"send", "message":{"op":"array", "args":[
                lookup, field("quantity"), {"op":"root","path":["root"]}
            ]}}
        ]}
    ]);
    assert_eq!(
        run(
            record,
            &[1, 0, 1, 42, 2, 2, 1, 100, 2, 200, 2, 2, 2, 9, 1, 3]
        )
        .unwrap(),
        vec![json!([200, 9, 42]), json!([100, 3, 42])]
    );
}
