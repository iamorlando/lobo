//! Binary accessors compiled once from a protocol definition.
use super::{
    expression::{Expr, Operator},
    schema::{Action, Binary, BinaryField, Operation, Record},
};
use lobo_models::Side;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy)]
pub(super) struct Operand {
    pub(super) offset: usize,
    pub(super) width: usize,
    pub(super) little: bool,
    pub(super) value: Option<u64>,
}
impl Operand {
    fn field(f: &BinaryField) -> Result<Self, String> {
        if f.kind != "uint" {
            return Err("Expected an integer field".into());
        }
        Ok(Self {
            offset: f.offset,
            width: f.size,
            little: f.byteorder == "little",
            value: None,
        })
    }
    fn literal(v: u64) -> Self {
        Self {
            offset: 0,
            width: 0,
            little: false,
            value: Some(v),
        }
    }
    #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
    #[inline(always)]
    fn read(self, data: &[u8]) -> u64 {
        if let Some(value) = self.value {
            return value;
        }
        let data = &data[self.offset..self.offset + self.width];
        if self.little {
            data.iter().rev().fold(0, |a, &b| (a << 8) | u64::from(b))
        } else {
            data.iter().fold(0, |a, &b| (a << 8) | u64::from(b))
        }
    }
}
#[derive(Clone)]
pub(super) struct SideOperand {
    pub(super) offset: usize,
    pub(super) values: Box<[Option<Side>; 256]>,
}
impl SideOperand {
    #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
    fn read(&self, bytes: &[u8]) -> Result<Side, String> {
        self.values[usize::from(bytes[self.offset])].ok_or_else(|| "Invalid order side".into())
    }
}
#[derive(Clone)]
pub(super) struct Mapping {
    pub(super) size: usize,
    pub(super) kind: u8,
    pub(super) id: Operand,
    pub(super) quantity: Operand,
    pub(super) price: Operand,
    pub(super) new_id: Operand,
    pub(super) side: Option<SideOperand>,
    pub(super) execution_price: bool,
    registration: Option<Registration>,
    pub(super) streamable: bool,
}
#[derive(Clone)]
pub struct Plan {
    pub spec: Arc<Binary>,
    records: Vec<Option<Mapping>>,
    other: BTreeMap<u64, Mapping>,
    #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
    max_header: usize,
    #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
    pub(super) code: Arc<super::jit::binary::Decoder>,
}
#[derive(Clone, Copy, Default)]
pub struct Message {
    pub key: u64,
    pub timestamp: u64,
    pub kind: u8,
    pub id: u64,
    pub quantity: u64,
    pub price: u64,
    pub new_id: u64,
    pub side: Option<Side>,
    pub execution_price: bool,
}
fn operand(expr: &Expr, record: &Record, binary: &Binary) -> Result<Operand, String> {
    match expr.op {
        Operator::Literal => Ok(Operand::literal(
            expr.value.as_u64().ok_or("Expected an integer constant")?,
        )),
        Operator::Variable if expr.name == "clock" => Operand::field(&binary.timestamp),
        Operator::Field if expr.path.len() == 1 => Operand::field(
            record
                .fields
                .get(expr.path[0].as_str().ok_or("Binary fields have names")?)
                .ok_or("Unknown binary field")?,
        ),
        _ => Err("This binary mapping requires incremental execution; use start()".into()),
    }
}
fn side_operand(expr: &Expr, record: &Record) -> Result<SideOperand, String> {
    let read = |value: &serde_json::Value| match value.as_str() {
        Some("buy") => Ok(Some(Side::Buy)),
        Some("sell") => Ok(Some(Side::Sell)),
        None if value.is_null() => Ok(None),
        _ => Err("Side mapping values must be buy, sell, or None".to_owned()),
    };
    if matches!(expr.op, Operator::Literal) {
        return Ok(SideOperand {
            offset: 0,
            values: Box::new([read(&expr.value)?; 256]),
        });
    }
    if !matches!(expr.op, Operator::Map)
        || expr.args.len() != 3
        || !matches!(expr.args[1].op, Operator::Literal)
        || !matches!(expr.args[2].op, Operator::Literal)
    {
        return Err(
            "Binary side mapping requires constant wire values; use start() for computed mappings"
                .into(),
        );
    }
    let source = &expr.args[0];
    if !matches!(source.op, Operator::Field | Operator::Root) || source.path.len() != 1 {
        return Err("Side mapping requires a field".into());
    }
    let field = record
        .fields
        .get(
            source.path[0]
                .as_str()
                .ok_or("Side requires a named field")?,
        )
        .ok_or("Unknown side field")?;
    if field.size != 1 {
        return Err("Bulk side mappings require a one-byte discriminator".into());
    }
    let mut values = Box::new([read(&expr.args[2].value)?; 256]);
    let mapping = expr.args[1]
        .value
        .as_object()
        .ok_or("Side map must be an object")?;
    for raw in 0..=255u8 {
        let key = if field.kind == "text" {
            let byte = [raw];
            let Ok(value) = std::str::from_utf8(&byte) else {
                values[usize::from(raw)] = None;
                continue;
            };
            value.trim().to_owned()
        } else {
            raw.to_string()
        };
        if let Some(value) = mapping.get(&key) {
            values[usize::from(raw)] = read(value)?;
        }
    }
    Ok(SideOperand {
        offset: field.offset,
        values,
    })
}
#[derive(Clone)]
struct Registration {
    field: BinaryField,
    price_decimals: u8,
    quantity_decimals: u8,
    policy: lobo_models::BookPolicy,
}
pub struct RegistrationView<'a> {
    pub symbol: &'a str,
    pub price_decimals: u8,
    pub quantity_decimals: u8,
    pub policy: lobo_models::BookPolicy,
}
impl Registration {
    fn new(action: &Action, record: &Record) -> Result<Self, ()> {
        let Operation::Register {
            symbol,
            price_decimals,
            quantity_decimals,
            key,
            policy,
        } = &action.operation
        else {
            return Err(());
        };
        if !matches!(symbol.op, Operator::Field)
            || symbol.path.len() != 1
            || !matches!(key.op, Operator::Variable)
            || key.name != "key"
        {
            return Err(());
        }
        let field = record
            .fields
            .get(symbol.path[0].as_str().ok_or(())?)
            .ok_or(())?
            .clone();
        if field.kind != "text" {
            return Err(());
        }
        let constant = |e: &Expr| {
            if matches!(e.op, Operator::Literal) {
                e.value
                    .as_u64()
                    .and_then(|v| u8::try_from(v).ok())
                    .ok_or(())
            } else {
                Err(())
            }
        };
        if !matches!(policy.op, Operator::Literal) {
            return Err(());
        }
        let policy = match policy.value.as_str() {
            Some("full") => lobo_models::BookPolicy::Full,
            Some("no_user_map") => lobo_models::BookPolicy::NoUserMap,
            Some("no_hidden_quantity") => lobo_models::BookPolicy::NoHiddenQuantity,
            Some("no_updates") => lobo_models::BookPolicy::NoUpdates,
            _ => return Err(()),
        };
        Ok(Self {
            field,
            price_decimals: constant(price_decimals)?,
            quantity_decimals: constant(quantity_decimals)?,
            policy,
        })
    }
    fn view<'a>(&self, bytes: &'a [u8]) -> Result<RegistrationView<'a>, String> {
        let symbol =
            std::str::from_utf8(&bytes[self.field.offset..self.field.offset + self.field.size])
                .map_err(|e| e.to_string())?
                .trim();
        Ok(RegistrationView {
            symbol,
            price_decimals: self.price_decimals,
            quantity_decimals: self.quantity_decimals,
            policy: self.policy,
        })
    }
}
impl Plan {
    #[cfg(feature = "native")]
    pub(super) fn metadata_only(spec: Arc<Binary>) -> Result<Self, String> {
        #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
        let max_header = [&spec.tag, &spec.key, &spec.timestamp]
            .into_iter()
            .map(|f| f.offset + f.size)
            .max()
            .unwrap_or(0);
        let records = vec![None; 256];
        let other = BTreeMap::new();
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        let code = super::jit::binary::Decoder::compile(&spec, &records, &other)?;
        Ok(Self {
            spec,
            records,
            other,
            #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
            max_header,
            #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
            code,
        })
    }
    pub fn new(spec: Arc<Binary>) -> Result<Self, String> {
        let mut records = vec![None; 256];
        let mut other = BTreeMap::new();
        #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
        let max_header = [&spec.tag, &spec.key, &spec.timestamp]
            .into_iter()
            .map(|f| f.offset + f.size)
            .max()
            .unwrap_or(0);
        for (&tag, record) in &spec.records {
            if record.variable() {
                return Err(
                    "Variable binary records require incremental execution; use start() and wait()"
                        .into(),
                );
            }
            let mut mapping = Mapping {
                size: record.size.expect("fixed record"),
                kind: 0,
                id: Operand::literal(0),
                quantity: Operand::literal(0),
                price: Operand::literal(0),
                new_id: Operand::literal(0),
                side: None,
                execution_price: false,
                registration: None,
                streamable: true,
            };
            for action in &record.actions {
                use Operation::*;
                if metadata(action) {
                    match Registration::new(action, record) {
                        Ok(registration) if mapping.registration.is_none() => {
                            mapping.registration = Some(registration)
                        }
                        _ => mapping.streamable = false,
                    }
                    continue;
                }
                if mapping.kind != 0 {
                    return Err(
                        "Bulk records require one mutation; use start() for compound records"
                            .into(),
                    );
                }
                let timestamp = match &action.operation {
                    Add { timestamp, .. }
                    | Execute { timestamp, .. }
                    | Cancel { timestamp, .. }
                    | Remove { timestamp, .. }
                    | Replace { timestamp, .. } => timestamp,
                    _ => {
                        return Err(
                            "This mapping requires incremental execution; use start()".into()
                        );
                    }
                };
                let timestamp = operand(timestamp, record, &spec)?;
                if timestamp.value.is_some()
                    || timestamp.offset != spec.timestamp.offset
                    || timestamp.width != spec.timestamp.size
                    || timestamp.little != (spec.timestamp.byteorder == "little")
                {
                    return Err("Bulk records must use the source timestamp; use start() for mapped timestamps".into());
                }
                match &action.operation {
                    Add {
                        id,
                        side,
                        price,
                        quantity,
                        ..
                    } => {
                        mapping.kind = 1;
                        mapping.id = operand(id, record, &spec)?;
                        mapping.side = Some(side_operand(side, record)?);
                        mapping.price = operand(price, record, &spec)?;
                        mapping.quantity = operand(quantity, record, &spec)?;
                    }
                    Execute {
                        id,
                        quantity,
                        price,
                        ..
                    } => {
                        mapping.kind = 2;
                        mapping.id = operand(id, record, &spec)?;
                        mapping.quantity = operand(quantity, record, &spec)?;
                        if !matches!(price.op, Operator::Literal) || !price.value.is_null() {
                            mapping.price = operand(price, record, &spec)?;
                            mapping.execution_price = true;
                        }
                    }
                    Cancel { id, quantity, .. } => {
                        mapping.kind = 3;
                        mapping.id = operand(id, record, &spec)?;
                        mapping.quantity = operand(quantity, record, &spec)?;
                    }
                    Remove { id, .. } => {
                        mapping.kind = 4;
                        mapping.id = operand(id, record, &spec)?;
                    }
                    Replace {
                        id,
                        new_id,
                        quantity,
                        price,
                        ..
                    } => {
                        mapping.kind = 5;
                        mapping.id = operand(id, record, &spec)?;
                        mapping.new_id = operand(new_id, record, &spec)?;
                        mapping.quantity = operand(quantity, record, &spec)?;
                        mapping.price = operand(price, record, &spec)?;
                    }
                    _ => {
                        return Err(
                            "This record mapping requires incremental execution; use start()"
                                .into(),
                        );
                    }
                }
            }
            if mapping.price.width > 4
                || mapping.price.value.is_some_and(|v| v > u64::from(u32::MAX))
            {
                return Err("Use start() for prices wider than 32 bits".into());
            }
            if tag < 256 {
                records[tag as usize] = Some(mapping);
            } else {
                other.insert(tag, mapping);
            }
        }
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        let code = super::jit::binary::Decoder::compile(&spec, &records, &other)?;
        Ok(Self {
            spec,
            records,
            other,
            #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
            max_header,
            #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
            code,
        })
    }
    /// Construction-time eligibility for a record with fixed metadata and a
    /// single typed mutation. Shared with the packet compiler.
    #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
    pub(super) fn streamable(&self, tag: u64) -> bool {
        let mapping = if tag < 256 {
            self.records[tag as usize].as_ref()
        } else {
            self.other.get(&tag)
        };
        mapping.is_some_and(|mapping| mapping.streamable && mapping.kind != 0)
    }
    pub fn streaming<'a>(
        &self,
        tag: u64,
        bytes: &'a [u8],
    ) -> Result<Option<(Message, Option<RegistrationView<'a>>)>, String> {
        let mapping = if tag < 256 {
            self.records[tag as usize].as_ref()
        } else {
            self.other.get(&tag)
        };
        let Some(mapping) = mapping.filter(|m| m.streamable && m.kind != 0) else {
            return Ok(None);
        };
        let message = self.decode(bytes)?;
        Ok(Some((
            message,
            mapping
                .registration
                .as_ref()
                .map(|r| r.view(bytes))
                .transpose()?,
        )))
    }
    pub fn decode(&self, bytes: &[u8]) -> Result<Message, String> {
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        {
            return self.code.decode(bytes);
        }
        #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
        {
            if bytes.len() < self.max_header {
                return Err("Truncated binary header".into());
            }
            let mut result = Message {
                key: self.spec.key.number(bytes)?,
                timestamp: self.spec.timestamp.number(bytes)?,
                ..Message::default()
            };
            let tag = self.spec.tag.number(bytes)?;
            let mapping = if tag < 256 {
                self.records[tag as usize].as_ref()
            } else {
                self.other.get(&tag)
            };
            if let Some(mapping) = mapping {
                if bytes.len() != mapping.size {
                    return Err("Unexpected record size".into());
                }
                result.kind = mapping.kind;
                if mapping.kind != 0 {
                    result.id = mapping.id.read(bytes);
                    result.quantity = mapping.quantity.read(bytes);
                    result.price = mapping.price.read(bytes);
                    result.new_id = mapping.new_id.read(bytes);
                    result.execution_price = mapping.execution_price;
                    if let Some(side) = &mapping.side {
                        result.side = Some(side.read(bytes)?);
                    }
                }
            }
            Ok(result)
        }
    }
}
pub(super) fn metadata(action: &Action) -> bool {
    match &action.operation {
        Operation::Register { .. } | Operation::DirectoryComplete => true,
        Operation::When {
            actions, otherwise, ..
        } => actions.iter().chain(otherwise).all(metadata),
        Operation::ForEach { actions, .. } => actions.iter().all(metadata),
        _ => false,
    }
}

impl Message {
    pub fn mutation(self) -> Result<super::mutation::Mutation, String> {
        use super::mutation::Mutation;
        use crate::custom::orders::*;
        use lobo_primitives::{Price64, uuid::Uuid};
        let id = Uuid::from_u128(u128::from(self.id));
        Ok(match self.kind {
            1 => Mutation::Add(AddOrder {
                timestamp: self.timestamp,
                id,
                side: self.side.ok_or("Missing order side")?,
                price: Price64::from(self.price),
                quantity: self.quantity,
            }),
            2 => Mutation::Execute(ExecuteOrder {
                timestamp: self.timestamp,
                id,
                price: self.execution_price.then(|| Price64::from(self.price)),
                quantity: self.quantity,
            }),
            3 => Mutation::Cancel(CancelOrder {
                timestamp: self.timestamp,
                id,
                quantity: self.quantity,
            }),
            4 => Mutation::Remove(RemoveOrder {
                timestamp: self.timestamp,
                id,
            }),
            5 => Mutation::Replace(ReplaceOrder {
                timestamp: self.timestamp,
                id,
                new_id: Uuid::from_u128(u128::from(self.new_id)),
                price: Price64::from(self.price),
                quantity: self.quantity,
            }),
            _ => return Err("Not a mutation record".into()),
        })
    }
}
