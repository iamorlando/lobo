use super::super::compiler::Target;
use super::emit::{Emit, View};
use crate::custom::definition::{
    expression::{Expr, Operator},
    schema::{Action, Operation},
};

impl Emit<'_, '_> {
    pub fn actions(
        &mut self,
        actions: &[Action],
        in_book: bool,
        collect_levels: bool,
    ) -> Result<(), String> {
        use Operation::*;
        for action in actions {
            match &action.operation {
                When {
                    condition,
                    actions,
                    otherwise,
                } => {
                    let condition = self.condition(condition)?;
                    self.branch(
                        condition,
                        |e| e.actions(actions, in_book, collect_levels),
                        |e| e.actions(otherwise, in_book, collect_levels),
                    )?;
                }
                ForEach {
                    items,
                    actions,
                    order_by,
                    unique_by,
                } => {
                    if matches!(items.op, Operator::Array)
                        && order_by.is_none()
                        && unique_by.is_none()
                    {
                        let parent = self.item;
                        let shape = self.owner.layout.shapes[self.result(items)?]
                            .element
                            .ok_or("Missing element layout")?;
                        let mut values = Vec::new();
                        for expr in &items.args {
                            let value = self.record(expr)?;
                            values.push(self.freeze(value));
                        }
                        let levels = matches!(
                            actions.as_slice(),
                            [Action {
                                operation: Level { .. },
                                ..
                            }]
                        );
                        for value in values {
                            self.item = View { shape, ..value };
                            self.actions(actions, in_book, levels)?;
                        }
                        self.item = parent;
                        if levels {
                            self.call("packet_levels_flush", &[self.frame], true);
                        }
                    } else {
                        let value = self.record(items)?;
                        let value = self.freeze(value);
                        self.foreach(
                            value,
                            actions,
                            order_by.as_ref(),
                            unique_by.as_ref(),
                            in_book,
                        )?;
                    }
                }
                Directory {
                    items,
                    symbol,
                    actions,
                    on_remove,
                } => {
                    let value = self.record(items)?;
                    let value = self.freeze(value);
                    self.call("packet_directory_start", &[self.frame], false);
                    self.each(value, |e| {
                        let symbol = e.ref_expr(symbol)?;
                        e.call("packet_directory_symbol", &[e.frame, symbol], true);
                        Ok(())
                    })?;
                    let length = self.call("packet_directory_begin", &[self.frame], true);
                    self.counted(length, |e, index| {
                        e.call("packet_directory_remove_begin", &[e.frame, index], true);
                        e.actions(on_remove, in_book, false)?;
                        e.call("packet_directory_remove_end", &[e.frame, index], true);
                        Ok(())
                    })?;
                    self.foreach(value, actions, None, None, in_book)?;
                    self.call("packet_directory_end", &[self.frame], true);
                }
                Book {
                    symbol,
                    snapshot,
                    timestamp,
                    depth,
                    checksum,
                    ready,
                    actions,
                } => {
                    let symbol = self.ref_expr(symbol)?;
                    let snapshot = self.expression(snapshot)?;
                    let snapshot = self.boolean(snapshot);
                    let timestamp = self.uint(timestamp)?;
                    let entered = self.call(
                        "packet_book_begin",
                        &[self.frame, symbol, snapshot, timestamp],
                        true,
                    );
                    self.branch(entered,|e|{
                        e.actions(actions,true,false)?;
                        let has_depth=*depth!=0;
                        let depth=e.word(*depth as u64);
                        if has_depth{e.call("packet_book_depth",&[e.frame,depth],true);}
                        let finish=if *ready{"packet_ready"}else{"packet_not_ready"};
                        if let Some(checksum)=checksum {let expected=e.ref_expr(&checksum.expected)?;
                            let config=e.word(checksum as *const _ as u64);
                            let valid=e.call(if checksum.signed { "packet_checksum_signed" } else { "packet_checksum" },&[e.frame,config,expected],true);
                            e.branch(valid,|e|{e.call(finish,&[e.frame,depth],true);Ok(())},|e|{
                                if checksum.on_failure.is_empty(){
                                    let message=e.text_constant("Book checksum mismatch; a fresh snapshot is required");
                                    e.call("typed_fail",&[e.frame,message],true);
                                }else{e.actions(&checksum.on_failure,true,false)?;e.call("packet_book_skip",&[e.frame],false);}
                                Ok(())
                            })?;
                        }else{e.call(finish,&[e.frame,depth],true);}
                        Ok(())
                    },|_|Ok(()))?;
                }
                Let { name, value } => {
                    let value = self.record(value)?;
                    self.own(value);
                    let index = self
                        .owner
                        .layout
                        .variables
                        .keys()
                        .position(|v| v == name)
                        .ok_or("Unknown variable")?;
                    let index = self.word(index as u64);
                    self.call("typed_bind", &[self.frame, index], false);
                }
                Remember { table, key, value } => {
                    let key = self.record(key)?;
                    let key = self.freeze(key);
                    let value = self.record(value)?;
                    self.own(value);
                    let index = self
                        .owner
                        .layout
                        .tables
                        .keys()
                        .position(|v| v == table)
                        .ok_or("Unknown table")?;
                    let index = self.word(index as u64);
                    self.call("typed_remember", &[self.frame, index, key.address], false);
                }
                Send { message: value } => {
                    let value = self.record(value)?;
                    self.serialize(value)?;
                    self.call("packet_send", &[self.frame], true);
                }
                Subscribe => {
                    self.call("packet_subscribe", &[self.frame], true);
                }
                DirectoryComplete => {
                    self.call("packet_directory_complete", &[self.frame], true);
                }
                Sequence { value, key, reset } => {
                    let value = self.uint(value)?;
                    let key = self.ref_expr(key)?;
                    self.call(
                        if *reset {
                            "packet_sequence_reset"
                        } else {
                            "packet_sequence"
                        },
                        &[self.frame, value, key],
                        true,
                    );
                }
                Fail { message } => {
                    let message = self.text_constant(message);
                    self.call("typed_fail", &[self.frame, message], true);
                }
                Register {
                    symbol,
                    price_decimals,
                    quantity_decimals,
                    key,
                    policy,
                } => {
                    let symbol = self.ref_expr(symbol)?;
                    let price = self.uint(price_decimals)?;
                    let quantity = self.uint(quantity_decimals)?;
                    let key = self.ref_expr(key)?;
                    let policy = self.ref_expr(policy)?;
                    self.call(
                        "packet_register",
                        &[self.frame, symbol, price, quantity, key, policy],
                        true,
                    );
                }
                _ => {
                    if in_book {
                        self.order(&action.operation, collect_levels)?;
                    } else {
                        let entered = self.call("packet_order_begin", &[self.frame], true);
                        self.branch(
                            entered,
                            |e| e.order(&action.operation, collect_levels),
                            |_| Ok(()),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
    pub fn order(&mut self, operation: &Operation, collect_levels: bool) -> Result<(), String> {
        use Operation::*;
        let (name, fields): (&str, Vec<(&Expr, Target)>) = match operation {
            Add {
                timestamp,
                id,
                side,
                price,
                quantity,
            } => (
                if self.owner.fast_binary {
                    "packet_add_fast"
                } else {
                    "packet_add"
                },
                vec![
                    (timestamp, Target::Time),
                    (id, Target::Id),
                    (side, Target::Side),
                    (price, Target::Price),
                    (quantity, Target::Quantity),
                ],
            ),
            Execute {
                id,
                quantity,
                price,
                timestamp,
            } => (
                "packet_apply_execute",
                vec![
                    (id, Target::Id),
                    (quantity, Target::Quantity),
                    (price, Target::OptionalPrice),
                    (timestamp, Target::Time),
                ],
            ),
            Cancel {
                id,
                quantity,
                timestamp,
            } => (
                "packet_apply_cancel",
                vec![
                    (id, Target::Id),
                    (quantity, Target::Quantity),
                    (timestamp, Target::Time),
                ],
            ),
            Remove { id, timestamp } => (
                "packet_apply_remove",
                vec![(id, Target::Id), (timestamp, Target::Time)],
            ),
            Replace {
                id,
                new_id,
                quantity,
                price,
                timestamp,
            } => (
                "packet_apply_replace",
                vec![
                    (id, Target::Id),
                    (new_id, Target::NewId),
                    (quantity, Target::Quantity),
                    (price, Target::Price),
                    (timestamp, Target::Time),
                ],
            ),
            Modify {
                id,
                quantity,
                timestamp,
            } => (
                "packet_modify",
                vec![
                    (id, Target::Id),
                    (quantity, Target::Quantity),
                    (timestamp, Target::Time),
                ],
            ),
            Upsert {
                price,
                timestamp,
                id,
                side,
                quantity,
            } => (
                "packet_upsert",
                vec![
                    (price, Target::OptionalPrice),
                    (timestamp, Target::Time),
                    (id, Target::Id),
                    (side, Target::Side),
                    (quantity, Target::Quantity),
                ],
            ),
            Trade {
                id,
                side,
                price,
                quantity,
                timestamp,
            } => (
                "packet_apply_trade",
                vec![
                    (timestamp, Target::Time),
                    (price, Target::Price),
                    (quantity, Target::Quantity),
                    (side, Target::Side),
                    (id, Target::TradeId),
                ],
            ),
            TradeHistory { id } => ("packet_apply_history", vec![(id, Target::HistoryId)]),
            Level {
                side,
                price,
                quantity,
            } => (
                "packet_level",
                vec![
                    (side, Target::Side),
                    (price, Target::LevelPrice),
                    (quantity, Target::LevelQuantity),
                ],
            ),
            OrderCommand { value, timestamp } => return self.command(value, timestamp),
            RestoreOrder { value } => return self.restore(value),
            _ => return Err("Expected an order operation during compilation".into()),
        };
        let emit = |e: &mut Self| {
            for (expr, target) in &fields {
                e.project(expr, *target)?;
            }
            e.call(name, &[e.frame], true);
            if matches!(operation, Level { .. }) && !collect_levels {
                e.call("packet_levels_flush", &[e.frame], true);
            }
            Ok(())
        };
        if matches!(operation, Trade { .. }) {
            let ready = self.call("packet_trade_ready", &[self.frame], true);
            self.branch(ready, emit, |_| Ok(()))
        } else {
            emit(self)
        }
    }
}
