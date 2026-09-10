//! Schema-specialized decoding and packet execution.
mod decode;
mod emit;
mod layout;
pub(super) mod runtime;
mod storage;

#[cfg(test)]
mod tests {
    use super::layout::Layout;
    use crate::{
        custom::{
            AdapterDescriptor, BookLevel, FeedMode, Protocol,
            definition::{DefinedProtocol, schema::Definition},
        },
        feed::FeedState,
    };
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn protocol(actions: Value, connect: Value) -> DefinedProtocol {
        let definition = Definition::parse(&json!({
            "format":{"format":"json","messages":[{"condition":{"op":"literal","value":true},"actions":actions}]},
            "connect":connect,"subscriptions":[],"bootstrap":[],"reconcile_window_ns":1000,"reconcile_capacity":32,"symbols_per_connection":0
        }).to_string()).unwrap();
        let info = AdapterDescriptor {
            id: "test".into(),
            name: "test".into(),
            mode: FeedMode::Live,
            level: BookLevel::L3,
            default_symbol: "BOOK".into(),
            endpoint: None,
            timezone: "UTC".into(),
            supports_trades: true,
        };
        DefinedProtocol::new(Arc::new(definition), info, "BOOK").unwrap()
    }

    #[test]
    fn lifecycle_selection_refreshes_slots_changed_by_messages() {
        let mut p = protocol(
            json!([{"action":"let","name":"symbol","value":{"op":"literal","value":"OTHER"}}]),
            json!([{"action":"send","message":{"op":"variable","name":"symbol"}}]),
        );
        let mut state = FeedState::new("BOOK").unwrap();
        p.set_symbol(&state, "BOOK");
        p.packet(&mut state, 0, b"{}").unwrap();
        p.set_symbol(&state, "BOOK");
        let definition = p.definition.clone();
        p.actions(&definition.connect, &mut state, &Value::Null, &Value::Null)
            .unwrap();
        assert_eq!(p.commands(&mut state), vec!["\"BOOK\"".to_owned()]);
    }

    #[test]
    fn compiled_program_is_shared_but_connection_state_is_not() {
        let actions = json!([
            {"action":"let","name":"stamp","value":{"op":"field","path":["time"]}},
            {"action":"send","message":{"op":"timestamp","args":[{"op":"variable","name":"stamp"},{"op":"literal","value":"us"}]}}
        ]);
        let first = protocol(actions.clone(), json!([]));
        let mut handles = Vec::new();
        for n in 0..8 {
            let mut p = protocol(actions.clone(), json!([]));
            assert!(Arc::ptr_eq(&first.program, &p.program));
            handles.push(std::thread::spawn(move || {
                let mut state = FeedState::new("BOOK").unwrap();
                for i in 0..1000 {
                    let clock = n * 1000 + i;
                    p.packet(&mut state, n, format!(r#"{{"time":{clock}}}"#).as_bytes())
                        .unwrap();
                    assert_eq!(
                        p.commands_on(&mut state, n),
                        vec![(clock * 1000).to_string()]
                    );
                }
            }));
        }
        drop(first);
        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn compiled_levels_preserve_wire_width_and_instrument_fallback() {
        let field = |name| json!({"op":"field","path":[name]});
        let decimal =
            |name| json!({"op":"decimal","args":[field(name),{"op":"literal","value":8}]});
        for (price, quantity, input, precision, expected) in [
            (
                decimal("p"),
                decimal("q"),
                r#"{"p":"10.0000","q":"1.200"}"#,
                (8, 8),
                (1_000_000_000, 120_000_000, 4, 3),
            ),
            (
                field("p"),
                field("q"),
                r#"{"p":1234,"q":50}"#,
                (2, 6),
                (1234, 50, 2, 6),
            ),
        ] {
            let mut p = protocol(
                json!([
                    {"action":"let","name":"price_decimals","value":{"op":"literal","value":9}},
                    {"action":"book","symbol":{"op":"literal","value":"BOOK"},"snapshot":{"op":"literal","value":true},"timestamp":{"op":"literal","value":100},"depth":0,"ready":true,"checksum":null,
                     "actions":[{"action":"level","side":{"op":"literal","value":"buy"},"price":price,"quantity":quantity}]}
                ]),
                json!([
                    {"action":"register","symbol":{"op":"literal","value":"BOOK"},"price_decimals":{"op":"literal","value":precision.0},"quantity_decimals":{"op":"literal","value":precision.1},"key":{"op":"literal","value":null},"policy":{"op":"literal","value":"full"}}
                ]),
            );
            let mut state = FeedState::new("BOOK").unwrap();
            let definition = p.definition.clone();
            p.actions(&definition.connect, &mut state, &Value::Null, &Value::Null)
                .unwrap();
            p.packet(&mut state, 0, input.as_bytes()).unwrap();
            // Inspect the typed operands produced by the real packet entry.
            let record = p.execution.frames.last().unwrap().record;
            assert_eq!(
                (
                    record.price,
                    record.quantity,
                    record.price_width,
                    record.quantity_width
                ),
                expected
            );
            assert_eq!(record.side, lobo_models::Side::Buy);
        }
    }

    #[test]
    fn infer_layouts_from_public_adapter_declarations() {
        for (name, text) in [
            (
                "itch",
                include_str!("../../../../../tests/definitions/itch.json"),
            ),
            (
                "kraken",
                include_str!("../../../../../tests/definitions/kraken.json"),
            ),
            (
                "bitfinex",
                include_str!("../../../../../tests/definitions/bitfinex.json"),
            ),
            (
                "server",
                include_str!("../../../../../tests/definitions/server.json"),
            ),
        ] {
            let definition = Definition::parse(text).unwrap();
            let layout = Layout::new(&definition).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(!layout.shapes.is_empty());
        }
    }

    #[test]
    fn compiled_decoder_reads_reordered_fields_and_exact_numbers() {
        use super::{
            decode::{Buffers, Decoder},
            storage::{NUMBER, Record},
        };
        let definition =
            Definition::parse(include_str!("../../../../../tests/definitions/server.json"))
                .unwrap();
        let layout = Layout::new(&definition).unwrap();
        let decoder = Decoder::compile(&layout).unwrap();
        let mut buffers = Buffers::default();
        let input = br#"{"command":{"order":{"quantity":9007199254740993,"side":"buy","price":123,"id":"ffffffff-ffff-ffff-ffff-ffffffffffff","type":"limit"},"op":"add"},"book":"BOOK","type":"update","sequence":1,"timestamp_ns":2,"unused":{"x":[1,2,3]}}"#;
        let root = decoder.parse(layout.root, input, &mut buffers).unwrap();
        let mut shape = layout.root;
        let mut value = root;
        for name in ["command", "order", "quantity"] {
            let (offset, (_, child)) = layout.shapes[shape]
                .fields
                .iter()
                .enumerate()
                .find(|(_, (field, _))| *field == name)
                .unwrap();
            value = unsafe { *((*value).fields as *const usize).add(offset) as *const Record };
            shape = *child;
        }
        assert_eq!(unsafe { (*value).kind }, NUMBER);
        assert_eq!(unsafe { (*value).number.low }, 9_007_199_254_740_993);
        for invalid in [
            br#"{"command": "#.as_slice(),
            br#"{"unknown":01}"#.as_slice(),
        ] {
            assert!(decoder.parse(layout.root, invalid, &mut buffers).is_err());
        }
    }
}

mod actions;
mod command;
mod host;

pub(super) mod program;
