//! Declarative checksum setup and response handling. Books own calculation.
use super::{
    DefinedProtocol,
    schema::{self, Checksum, Definition, Operation},
};
use crate::feed::FeedState;
use lobo_models::Side;
use lobo_storage::policies::checksum::{
    Field, NumberFormat, Prepared, Priority, Specification, View,
};
use serde_json::Value;
use std::sync::Arc;

pub(super) fn prepare(definition: &mut Definition) -> Result<Option<Arc<Prepared>>, String> {
    let mut selected: Option<Arc<Prepared>> = None;
    schema::visit_operations(definition, &mut |action| {
        let Operation::Book {
            checksum: Some(config),
            ..
        } = &mut action.operation
        else {
            return Ok(());
        };
        let spec = Specification {
            view: match config.view.as_str() {
                "levels" => View::Levels,
                "orders" => View::Orders,
                _ => return Err("Invalid checksum view".into()),
            },
            depth: config.depth,
            sides: config
                .sides
                .iter()
                .map(|s| match s.as_str() {
                    "buy" => Ok(Side::Buy),
                    "sell" => Ok(Side::Sell),
                    _ => Err("Invalid checksum side".to_owned()),
                })
                .collect::<Result<_, _>>()?,
            fields: config
                .fields
                .iter()
                .map(|s| match s.as_str() {
                    "id" => Ok(Field::Id),
                    "price" => Ok(Field::Price),
                    "quantity" => Ok(Field::Quantity),
                    "signed_quantity" => Ok(Field::SignedQuantity),
                    _ => Err("Invalid checksum field".to_owned()),
                })
                .collect::<Result<_, _>>()?,
            format: match config.format.as_str() {
                "decimal_digits" => NumberFormat::DecimalDigits,
                "decimal" => NumberFormat::Decimal,
                "ecmascript" => NumberFormat::EcmaScript,
                _ => return Err("Invalid checksum number format".into()),
            },
            priority: match config.priority.as_str() {
                "id" => Priority::Id,
                "fifo" => Priority::Fifo,
                _ => return Err("Invalid checksum priority".into()),
            },
            separator: config.separator.clone(),
            interleave: config.interleave,
            signed: config.signed,
        };
        if let Some(first) = &selected {
            if first.specification.view != spec.view {
                return Err("A book's checksum policy must select one input view".into());
            }
            if first.specification == spec {
                config.prepared = first.clone();
                return Ok(());
            }
        }
        config.prepared = spec.prepare().map_err(str::to_owned)?;
        selected.get_or_insert_with(|| config.prepared.clone());
        Ok(())
    })?;
    Ok(selected)
}

impl<O: super::super::observer::EventSink> DefinedProtocol<O> {
    pub(super) fn checksum(
        &mut self,
        config: &Checksum,
        state: &mut FeedState,
        item: &Value,
        root: &Value,
        symbol: &str,
    ) -> Result<bool, String> {
        let expected = self.evaluate(&config.expected, item, root)?;
        let valid = self.checksum_value(config, state, &expected, symbol)?;
        if !valid {
            if config.on_failure.is_empty() {
                return Err("Book checksum mismatch; a fresh snapshot is required".into());
            }
            self.actions(&config.on_failure, state, item, root)?;
        }
        Ok(valid)
    }
    pub(super) fn checksum_value(
        &mut self,
        config: &Checksum,
        state: &mut FeedState,
        expected: &Value,
        symbol: &str,
    ) -> Result<bool, String> {
        let checksum = state
            .context
            .get_mut(symbol)
            .ok_or("Checksum arrived before snapshot")?
            .checksum_with(&config.prepared)
            .map_err(str::to_owned)?;
        let valid = if config.signed {
            expected.as_i64() == Some(i64::from(checksum as i32))
        } else {
            expected.as_u64() == Some(u64::from(checksum))
        };
        state.checksum_checks += 1;
        if !valid {
            state.checksum_failures += 1;
            self.invalidate(state, symbol);
            state.warming = !state.synchronized(&state.selected);
        }
        Ok(valid)
    }
}
