//! Compile the order-command schema into field projections and concrete calls.
//! This does not deserialize an intermediate JSON object into Command.
use super::super::compiler::Target;
use super::{emit::Emit, host::host, runtime::Frame};
use crate::custom::definition::{
    expression::{Expr, Operator},
    projection::OrderRecord,
};
use crate::{custom::observer::EventSink, order_messages::ApplyFeedCommand};
use lobo_models::{
    events::Reports,
    server::{
        Command, IcebergOrder, LimitOrder, MarketOrder, Order, OrderFields, RestingOrderState,
    },
};
use lobo_primitives::Price64;
use serde_json::Value;
use std::mem::offset_of;

pub(super) fn symbols<O: EventSink>() -> Vec<(&'static str, *const u8, usize)> {
    vec![
        ("command_add_market", apply_order::<O, 0, 0> as *const u8, 1),
        ("command_add_limit", apply_order::<O, 0, 1> as *const u8, 1),
        (
            "command_add_iceberg",
            apply_order::<O, 0, 2> as *const u8,
            1,
        ),
        (
            "command_fill_market",
            apply_order::<O, 1, 0> as *const u8,
            1,
        ),
        ("command_fill_limit", apply_order::<O, 1, 1> as *const u8, 1),
        (
            "command_fill_iceberg",
            apply_order::<O, 1, 2> as *const u8,
            1,
        ),
        (
            "command_simulate_market",
            apply_order::<O, 2, 0> as *const u8,
            1,
        ),
        (
            "command_simulate_limit",
            apply_order::<O, 2, 1> as *const u8,
            1,
        ),
        (
            "command_simulate_iceberg",
            apply_order::<O, 2, 2> as *const u8,
            1,
        ),
        ("command_execute", apply_change::<O, 0> as *const u8, 1),
        ("command_cancel", apply_change::<O, 1> as *const u8, 1),
        ("command_remove", apply_change::<O, 2> as *const u8, 1),
        ("command_modify", apply_change::<O, 3> as *const u8, 1),
        ("command_restore", apply_restore::<O> as *const u8, 1),
    ]
}
unsafe fn apply<O: EventSink>(
    run: *mut Frame,
    build: impl FnOnce(&OrderRecord) -> Result<Command, String>,
) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let command = build(&r.record)?;
            command
                .route_feed(s, r.symbol.as_ref(), r.record.timestamp, Reports::default())
                .map_err(|e| e.to_string())?;
            r.timestamp(s, r.record.timestamp);
            p.observer.record(||Ok(crate::custom::observer::book(r.symbol.as_ref(),vec![serde_json::json!({"action":"order_command","value":crate::custom::observer::literal(&command),"timestamp":crate::custom::observer::literal(r.record.timestamp)})],false,s.clock_ns,s.synchronized(r.symbol.as_ref()),0)));
            Ok(0)
        })
    }
}
unsafe extern "C" fn apply_order<O: EventSink, const COMMAND: u8, const KIND: u8>(
    run: *mut Frame,
) -> u64 {
    unsafe {
        apply::<O>(run, |d| {
            let fields = OrderFields {
                id: d.id,
                trader: d.trader,
                side: d.side,
                quantity: d.quantity,
            };
            let order = match KIND {
                0 => Order::Market(MarketOrder { fields }),
                1 => Order::Limit(LimitOrder {
                    fields,
                    price: u32::try_from(d.price).map_err(|_| "Order price exceeds u32")?,
                }),
                _ => Order::Iceberg(IcebergOrder {
                    fields,
                    price: u32::try_from(d.price).map_err(|_| "Order price exceeds u32")?,
                    hidden_quantity: d.hidden_quantity,
                    peak_quantity: d.peak_quantity,
                }),
            };
            Ok(match COMMAND {
                0 => Command::Add { order },
                1 => Command::Fill { order },
                _ => Command::Simulate { order },
            })
        })
    }
}
unsafe extern "C" fn apply_change<O: EventSink, const COMMAND: u8>(run: *mut Frame) -> u64 {
    unsafe {
        apply::<O>(run, |d| {
            let price = || {
                if d.has_price != 0 {
                    u32::try_from(d.price)
                        .map(Some)
                        .map_err(|_| "Order price exceeds u32".to_owned())
                } else {
                    Ok(None)
                }
            };
            Ok(match COMMAND {
                0 => Command::Execute {
                    id: d.id,
                    quantity: d.quantity,
                    price: price()?,
                },
                1 => Command::Cancel {
                    id: d.id,
                    quantity: d.quantity,
                },
                2 => Command::Remove { id: d.id },
                _ => Command::Modify {
                    id: d.id,
                    quantity: d.quantity,
                    price: price()?,
                    new_id: (d.has_new_id != 0).then_some(d.new_id),
                },
            })
        })
    }
}
unsafe extern "C" fn apply_restore<O: EventSink>(run: *mut Frame) -> u64 {
    unsafe {
        host::<O>(run, |p, s, r| {
            let d = r.record;
            let snapshot = RestingOrderState {
                id: d.id,
                trader: d.trader,
                side: d.side,
                price: d.price,
                quantity: d.quantity,
                created_at_ns: d.timestamp,
                hidden_quantity: d.hidden_quantity,
                peak_quantity: d.peak_quantity,
            };
            let order = snapshot.clone().into_order::<Price64>()?;
            crate::dispatch_feed_book!(s.book_mut(r.symbol.as_ref()), book, {
                let (storage, mut publish) = book.storage_and_publisher_at(d.timestamp);
                storage
                    .add_order(order, &mut publish)
                    .map_err(crate::custom::definition::error)?;
            });
            p.observer.record(||Ok(crate::custom::observer::book(r.symbol.as_ref(),vec![serde_json::json!({"action":"restore_order","value":crate::custom::observer::literal(snapshot)})],false,s.clock_ns,s.synchronized(r.symbol.as_ref()),0)));
            Ok(0)
        })
    }
}
pub(super) fn literal(value: impl Into<Value>) -> Expr {
    Expr {
        op: Operator::Literal,
        args: Vec::new(),
        path: Vec::new(),
        value: value.into(),
        name: String::new(),
    }
}
pub(super) fn field(value: &Expr, name: &str) -> Expr {
    if matches!(value.op, Operator::Object) {
        return value
            .path
            .iter()
            .position(|key| key.as_str() == Some(name))
            .map(|i| value.args[i].clone())
            .unwrap_or_else(|| literal(Value::Null));
    }
    if matches!(value.op, Operator::Literal) {
        return literal(value.value.get(name).cloned().unwrap_or(Value::Null));
    }
    if matches!(value.op, Operator::Field | Operator::Root) {
        let mut value = value.clone();
        value.path.push(name.into());
        return value;
    }
    Expr {
        op: Operator::Get,
        args: vec![value.clone(), literal(serde_json::json!([name]))],
        path: vec![],
        value: Value::Null,
        name: String::new(),
    }
}
impl Emit<'_, '_> {
    fn is(&mut self, value: &Expr, tag: &str) -> Result<cranelift_codegen::ir::Value, String> {
        let value = self.ref_expr(value)?;
        let tag = self.literal(&Value::String(tag.into()));
        Ok(self.call("typed_equal", &[self.frame, value, tag], true))
    }
    fn projected(&mut self, value: &Expr, fields: &[(&str, Target)]) -> Result<(), String> {
        let fields = fields
            .iter()
            .map(|(name, target)| (field(value, name), *target))
            .collect::<Vec<_>>();
        for (expr, target) in &fields {
            self.project(expr, *target)?;
        }
        Ok(())
    }
    fn default_id(&mut self, value: &Expr, name: &str, target: Target) -> Result<(), String> {
        let expr = field(value, name);
        let val = self.ref_expr(&expr)?;
        let present = self.call("typed_exists", &[val], false);
        self.branch(
            present,
            |e| e.project(&expr, target),
            |e| e.project(&literal(0u64), target),
        )
    }
    pub(super) fn command(&mut self, value: &Expr, timestamp: &Expr) -> Result<(), String> {
        self.project(timestamp, Target::Time)?;
        self.command_case(value, &field(value, "op"), 0)
    }
    fn command_case(&mut self, value: &Expr, tag: &Expr, index: usize) -> Result<(), String> {
        let names = [
            "add", "fill", "simulate", "execute", "cancel", "remove", "modify",
        ];
        if index == names.len() {
            let error = self.text_constant("Unknown order command");
            self.call("typed_fail", &[self.frame, error], true);
            return Ok(());
        }
        let yes = self.is(tag, names[index])?;
        self.branch(
            yes,
            |e| {
                if index < 3 {
                    return e.order_kind(&field(value, "order"), index, 0);
                }
                e.projected(value, &[("id", Target::Id)])?;
                if index != 5 {
                    e.projected(value, &[("quantity", Target::Quantity)])?;
                }
                if index == 3 || index == 6 {
                    e.projected(value, &[("price", Target::OptionalPrice)])?;
                }
                if index == 6 {
                    let expr = field(value, "new_id");
                    let v = e.ref_expr(&expr)?;
                    let present = e.call("typed_exists", &[v], false);
                    e.record_store(offset_of!(OrderRecord, has_new_id), present);
                    e.branch(present, |e| e.project(&expr, Target::NewId), |_| Ok(()))?;
                }
                e.call(
                    [
                        "command_execute",
                        "command_cancel",
                        "command_remove",
                        "command_modify",
                    ][index - 3],
                    &[e.frame],
                    true,
                );
                Ok(())
            },
            |e| e.command_case(value, tag, index + 1),
        )
    }
    fn order_kind(&mut self, value: &Expr, command: usize, kind: usize) -> Result<(), String> {
        let kinds = ["market", "limit", "iceberg"];
        if kind == 3 {
            let error = self.text_constant("Unknown order type");
            self.call("typed_fail", &[self.frame, error], true);
            return Ok(());
        }
        let yes = self.is(&field(value, "type"), kinds[kind])?;
        self.branch(
            yes,
            |e| {
                e.projected(
                    value,
                    &[
                        ("id", Target::Id),
                        ("side", Target::Side),
                        ("quantity", Target::Quantity),
                    ],
                )?;
                e.default_id(value, "trader", Target::Trader)?;
                if kind != 0 {
                    e.projected(value, &[("price", Target::Price)])?;
                }
                if kind == 2 {
                    e.projected(
                        value,
                        &[
                            ("hidden_quantity", Target::Hidden),
                            ("peak_quantity", Target::Peak),
                        ],
                    )?;
                }
                let imports = [
                    [
                        "command_add_market",
                        "command_add_limit",
                        "command_add_iceberg",
                    ],
                    [
                        "command_fill_market",
                        "command_fill_limit",
                        "command_fill_iceberg",
                    ],
                    [
                        "command_simulate_market",
                        "command_simulate_limit",
                        "command_simulate_iceberg",
                    ],
                ];
                e.call(imports[command][kind], &[e.frame], true);
                Ok(())
            },
            |e| e.order_kind(value, command, kind + 1),
        )
    }
    pub(super) fn restore(&mut self, value: &Expr) -> Result<(), String> {
        self.projected(
            value,
            &[
                ("id", Target::Id),
                ("trader", Target::Trader),
                ("side", Target::Side),
                ("price", Target::Price),
                ("quantity", Target::Quantity),
                ("created_at_ns", Target::Time),
                ("hidden_quantity", Target::Hidden),
                ("peak_quantity", Target::Peak),
            ],
        )?;
        self.call("command_restore", &[self.frame], true);
        Ok(())
    }
}
