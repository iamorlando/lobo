//! Tabular access to declared fields, preserving the complete wire record.
use super::binary::{FileFormat, Reader};
use polars::{frame::row::Row, prelude::*};
use std::collections::{BTreeMap, BTreeSet};
impl FileFormat {
    pub fn table(&self, keys: &BTreeSet<u64>) -> Result<LazyFrame, String> {
        let spec = &self.plan.spec;
        let reserved = ["record_type", "routing_key", "timestamp_ns", "record"];
        let mut fields = BTreeMap::new();
        for record in spec.records.values() {
            if record.variable() {
                return Err("Variable binary records require start() and wait(); table() supports fixed scalar layouts".into());
            }
            for (name, field) in &record.fields {
                if reserved.contains(&name.as_str()) {
                    return Err(format!(
                        "Field {name} conflicts with a table metadata column"
                    ));
                }
                let dtype = if field.kind == "text" {
                    DataType::String
                } else {
                    DataType::UInt64
                };
                if let Some(previous) = fields.insert(name.clone(), dtype.clone()) {
                    if previous != dtype {
                        return Err(format!("Field {name} has conflicting column types"));
                    }
                }
            }
        }
        let schema = Schema::from_iter(
            [
                Field::new("record_type".into(), DataType::UInt64),
                Field::new("routing_key".into(), DataType::UInt64),
                Field::new("timestamp_ns".into(), DataType::UInt64),
                Field::new("record".into(), DataType::Binary),
            ]
            .into_iter()
            .chain(
                fields
                    .iter()
                    .map(|(name, dtype)| Field::new(name.as_str().into(), dtype.clone())),
            ),
        );
        let mut reader = Reader::open(&self.path, self.plan.clone())?;
        let mut rows = Vec::new();
        while reader
            .record(|bytes| {
                let key = spec.key.number(bytes)?;
                if !keys.contains(&key) {
                    return Ok(());
                }
                let tag = spec.tag.number(bytes)?;
                let mut values = vec![
                    AnyValue::UInt64(tag),
                    AnyValue::UInt64(key),
                    AnyValue::UInt64(spec.timestamp.number(bytes)?),
                    AnyValue::BinaryOwned(bytes.to_vec()),
                ];
                let record = spec.records.get(&tag);
                if record.is_some_and(|record| record.size != Some(bytes.len())) {
                    return Err("Unexpected record size".into());
                }
                for name in fields.keys() {
                    values.push(match record.and_then(|r| r.fields.get(name)) {
                        Some(field) if field.kind == "text" => AnyValue::StringOwned(
                            field
                                .value(bytes)?
                                .as_str()
                                .ok_or("Expected text field")?
                                .into(),
                        ),
                        Some(field) => AnyValue::UInt64(field.number(bytes)?),
                        None => AnyValue::Null,
                    });
                }
                rows.push(Row::new(values));
                Ok(())
            })?
            .is_some()
        {}
        DataFrame::from_rows_and_schema(&rows, &schema)
            .map(IntoLazy::lazy)
            .map_err(|e| e.to_string())
    }
}
