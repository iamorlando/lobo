//! Shared expression tree used by declarations, validation, and compilation.
#![warn(missing_docs)]

use super::super::decimal::Decimal as ExactDecimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{borrow::Cow, collections::BTreeMap};

/// Lookup values indexed by connection ID, table name, and entry key.
pub type Tables = BTreeMap<u32, BTreeMap<String, BTreeMap<String, Value>>>;
/// Protocol variable names and their current values.
pub type Variables = BTreeMap<String, Value>;
/// The calculations supported by the adapter expression language.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    /// Return the stored constant value.
    Literal,
    /// Construct an array from argument expressions.
    Array,
    /// Construct an object from string path keys and argument expressions.
    Object,
    /// Read from the current message or iteration item.
    Field,
    /// Read from the complete incoming message, even inside nested iteration.
    Root,
    /// Read a named protocol or execution-context variable.
    Variable,
    /// Compare values for equality.
    Eq,
    /// Compare values for inequality.
    Ne,
    /// Compare two decimal-compatible values exactly for greater-than.
    Gt,
    /// Compare two decimal-compatible values exactly for less-than.
    Lt,
    /// Combine boolean operands with short-circuit AND.
    And,
    /// Combine boolean operands with short-circuit OR.
    Or,
    /// Test whether an operand is non-null.
    Exists,
    /// Test whether an operand is an array.
    IsArray,
    /// Count elements in an array or bytes in a string.
    Length,
    /// Remove the sign from a numeric value.
    Abs,
    /// Remove surrounding whitespace from a string.
    Trim,
    /// Remove one matching prefix from a string.
    StripPrefix,
    /// Look up a value in a declared mapping, with a fallback.
    Map,
    /// Look up a key in a named connection-local table.
    Lookup,
    /// Read a path from the result of another expression.
    Get,
    /// Convert an exact decimal to integer atoms at the requested precision.
    Decimal,
    /// Convert a timestamp to unsigned nanoseconds.
    Timestamp,
    /// Evaluate one of two expressions according to a boolean condition.
    Choose,
    /// Concatenate the textual representations of argument expressions.
    Concat,
}
/// An immutable calculation used by a message condition or action argument.
///
/// Use Field to read a source value, Variable to read protocol state, and methods
/// such as decimal, eq, or map to transform those values. These objects describe
/// calculations; they do not evaluate incoming messages in Python. Python boolean
/// conversion is rejected: use &, |, and .eq() to build conditions.
///
/// Args:
///     data: For the low-level Expression constructor, an exported expression
///         dictionary with an op name and its operands. Prefer Field, Variable,
///         Choose, Concat, or expression(value) when writing an adapter.
///
/// Examples:
/// ```python
/// from lobo.replay.adapters import expressions as le
///
/// price = le.Field("price").decimal(2)
/// is_update = le.Field("type").eq("update") & le.Field("price").exists()
/// ```
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(
        name = "Expression",
        module = "lobo.replay.adapters.expressions",
        frozen,
        subclass,
        from_py_object
    )
)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Expr {
    /// The operation represented by this node.
    pub op: Operator,
    #[serde(default)]
    /// Ordered operand expressions; the operator determines their required count.
    pub args: Vec<Expr>,
    #[serde(default)]
    /// Field path components, or object keys for the Object operator.
    pub path: Vec<Value>,
    #[serde(default)]
    /// The value held by a Literal node; unused for other operators.
    pub value: Value,
    #[serde(default)]
    /// The protocol variable name for a Variable node.
    pub name: String,
}
/// Input references for the reference evaluator, including its iteration context.
#[repr(C)]
pub struct Input<'a> {
    /// The current message or nested iteration item.
    pub item: &'a Value,
    /// The complete incoming message, unchanged by nested iteration.
    pub root: &'a Value,
    /// The current protocol and execution-context variables.
    pub vars: &'a Variables,
    /// Lookup tables for all connections; the connection variable selects one.
    pub tables: &'a Tables,
}
/// Represent a lookup key as text, borrowing strings and numeric tokens when possible.
pub fn key_ref(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(s) => Cow::Borrowed(s),
        Value::Number(n) => Cow::Borrowed(n.as_str()),
        _ => Cow::Owned(value.to_string()),
    }
}
/// Return an owned textual lookup key with the same representation as key_ref.
pub fn key(value: &Value) -> String {
    key_ref(value).into_owned()
}
/// Read a sequence of string keys or signed array indices from a value.
///
/// Negative indices count from the end. Missing keys, invalid indices, and
/// incompatible container types yield null.
pub fn path<'a>(mut value: &'a Value, path: &[Value]) -> &'a Value {
    for part in path {
        value = match part {
            Value::String(name) => &value[name],
            Value::Number(index) => index
                .as_i64()
                .and_then(|i| {
                    let a = value.as_array()?;
                    let n = if i < 0 { i + a.len() as i64 } else { i };
                    usize::try_from(n).ok().and_then(|i| a.get(i))
                })
                .unwrap_or(&Value::Null),
            _ => &Value::Null,
        };
    }
    value
}
/// Read a nonnegative 64-bit integer from a JSON number or decimal integer text.
///
/// # Errors
/// Returns an error for negative, fractional, overflowing, or incompatible values.
pub fn integer(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| format!("Expected an unsigned integer, got {value}"))
}
impl Expr {
    /// Validate operand counts and object keys recursively during adapter setup.
    ///
    /// # Errors
    /// Returns an error when a node has the wrong number of arguments or a non-text object key.
    pub fn validate(&self) -> Result<(), String> {
        use Operator::*;
        let expected = match self.op {
            Literal | Field | Root | Variable => 0,
            Exists | IsArray | Length | Abs | Trim => 1,
            Eq | Ne | Gt | Lt | And | Or | StripPrefix | Lookup | Get | Decimal | Timestamp => 2,
            Map | Choose => 3,
            Concat | Array => self.args.len(),
            Object => {
                if self.path.iter().any(|p| !p.is_string()) {
                    return Err("Object keys must be text".into());
                }
                self.path.len()
            }
        };
        if self.args.len() != expected {
            return Err(format!("{:?} expects {expected} operands", self.op));
        }
        for arg in &self.args {
            arg.validate()?;
        }
        Ok(())
    }
    /// Evaluate an expression as nonnegative integer atoms or timestamp nanoseconds.
    ///
    /// The input supplies message fields, variables, and lookup tables. Decimal
    /// conversion preserves exact input precision and refuses nonzero discarded digits.
    ///
    /// # Errors
    /// Returns an error for incompatible values, precision loss, or integer overflow.
    pub fn unsigned(&self, input: &Input<'_>) -> Result<u64, String> {
        let arg = |i: usize| self.args[i].eval_ref(input);
        match self.op {
            Operator::Decimal => {
                let places = u8::try_from(integer(arg(1)?.as_ref())?)
                    .map_err(|_| "Decimal precision exceeds 255")?;
                let decimal = self.args[0].exact(input)?;
                decimal.atoms(places)
            }
            Operator::Timestamp => {
                let v = arg(0)?;
                let unit = arg(1)?;
                let n = match unit.as_str().ok_or("timestamp unit must be text")? {
                    "ns" => integer(&v)?,
                    "us" => integer(&v)?
                        .checked_mul(1_000)
                        .ok_or("timestamp overflow")?,
                    "ms" => integer(&v)?
                        .checked_mul(1_000_000)
                        .ok_or("timestamp overflow")?,
                    "s" => integer(&v)?
                        .checked_mul(1_000_000_000)
                        .ok_or("timestamp overflow")?,
                    "rfc3339" => lobo_primitives::time::DateTime::parse_from_rfc3339(
                        v.as_str().ok_or("timestamp requires text")?,
                    )
                    .map_err(|e| e.to_string())?
                    .timestamp_nanos_opt()
                    .and_then(|n| u64::try_from(n).ok())
                    .ok_or("timestamp out of range")?,
                    _ => return Err("Unsupported timestamp unit".into()),
                };
                Ok(n)
            }
            Operator::Choose => self.args[if arg(0)?.as_bool() == Some(true) {
                1
            } else {
                2
            }]
            .unsigned(input),
            _ => integer(self.eval_ref(input)?.as_ref()),
        }
    }
    fn exact(&self, input: &Input<'_>) -> Result<ExactDecimal, String> {
        if matches!(self.op, Operator::Abs) {
            let value = self.args[0].eval_ref(input)?;
            let text = match value.as_ref() {
                Value::Number(n) => n.as_str(),
                Value::String(s) => s.as_str(),
                _ => return Err("Expected a decimal".into()),
            };
            ExactDecimal::parse(text.trim_start_matches('-'))
        } else {
            decimal_value(self.eval_ref(input)?.as_ref())
        }
    }
    /// Return integer atoms together with the source decimal scale.
    ///
    /// An explicit Decimal expression retains its wire scale for checksums. Other
    /// expressions use fallback, the instrument's declared fractional digit count.
    ///
    /// # Errors
    /// Returns an error for invalid decimals, precision loss, or integer overflow.
    pub fn scaled(&self, input: &Input<'_>, fallback: u8) -> Result<(u64, u32), String> {
        if matches!(self.op, Operator::Decimal) {
            let decimal = self.args[0].exact(input)?;
            let places = u8::try_from(self.args[1].unsigned(input)?)
                .map_err(|_| "Decimal precision exceeds 255")?;
            Ok((decimal.atoms(places)?, decimal.scale()))
        } else {
            Ok((self.unsigned(input)?, u32::from(fallback)))
        }
    }
    /// Borrow the source value for a direct Field or Root expression.
    ///
    /// item is the current iteration item; root is the full message. Other operators
    /// return None because their result is a calculation rather than a direct field.
    pub fn source<'a>(&self, item: &'a Value, root: &'a Value) -> Option<&'a Value> {
        match self.op {
            Operator::Field => Some(path(item, &self.path)),
            Operator::Root => Some(path(root, &self.path)),
            _ => None,
        }
    }
    /// Evaluate using the reference implementation and return an owned value.
    ///
    /// # Errors
    /// Returns an error when an operator receives an incompatible or out-of-range value.
    pub fn eval(&self, input: &Input<'_>) -> Result<Value, String> {
        self.eval_ref(input).map(Cow::into_owned)
    }
    /// Evaluate using the reference implementation, borrowing unchanged values.
    ///
    /// input supplies fields, variables, and connection-local tables. Computed
    /// containers and transformed values are returned as owned values.
    ///
    /// # Errors
    /// Returns an error when an operator receives an incompatible or out-of-range value.
    pub fn eval_ref<'a>(&'a self, input: &Input<'a>) -> Result<Cow<'a, Value>, String> {
        use Operator::*;
        match self.op {
            Literal => return Ok(Cow::Borrowed(&self.value)),
            Field => return Ok(Cow::Borrowed(path(input.item, &self.path))),
            Root => return Ok(Cow::Borrowed(path(input.root, &self.path))),
            Variable => {
                return Ok(Cow::Borrowed(
                    input.vars.get(&self.name).unwrap_or(&Value::Null),
                ));
            }
            Lookup => {
                let table = self.args[0].eval_ref(input)?;
                let key = self.args[1].eval_ref(input)?;
                let connection = input
                    .vars
                    .get("connection")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                return Ok(Cow::Borrowed(
                    input
                        .tables
                        .get(&connection)
                        .and_then(|tables| tables.get(key_ref(&table).as_ref()))
                        .and_then(|table| table.get(key_ref(&key).as_ref()))
                        .unwrap_or(&Value::Null),
                ));
            }
            _ => {}
        }
        let arg = |i: usize| self.args[i].eval_ref(input);
        Ok(Cow::Owned(match self.op {
            Literal | Field | Root | Variable | Lookup => unreachable!(),
            Object => Value::Object(
                self.path
                    .iter()
                    .zip(&self.args)
                    .map(|(key, value)| {
                        Ok((
                            key.as_str().ok_or("Object key must be text")?.to_owned(),
                            value.eval(input)?,
                        ))
                    })
                    .collect::<Result<_, String>>()?,
            ),
            Array => Value::Array(
                self.args
                    .iter()
                    .map(|e| e.eval(input))
                    .collect::<Result<_, _>>()?,
            ),
            And => json!(arg(0)?.as_bool() == Some(true) && arg(1)?.as_bool() == Some(true)),
            Or => json!(arg(0)?.as_bool() == Some(true) || arg(1)?.as_bool() == Some(true)),
            Eq | Ne => {
                let a = arg(0)?;
                let b = arg(1)?;
                let equal = if a.is_number() && b.is_number() {
                    compare(&a, &b)?.is_eq()
                } else {
                    a == b
                };
                json!(if matches!(self.op, Eq) { equal } else { !equal })
            }
            Gt | Lt => {
                let order = compare(arg(0)?.as_ref(), arg(1)?.as_ref())?;
                json!(if matches!(self.op, Gt) {
                    order.is_gt()
                } else {
                    order.is_lt()
                })
            }
            Exists => json!(!arg(0)?.is_null()),
            IsArray => json!(arg(0)?.is_array()),
            Length => {
                let v = arg(0)?;
                json!(match v.as_ref() {
                    Value::Array(a) => a.len(),
                    Value::Object(o) => o.len(),
                    Value::String(s) => s.len(),
                    _ => 0,
                })
            }
            Abs => {
                let v = arg(0)?;
                if let Some(i) = v.as_i64() {
                    json!(i.unsigned_abs())
                } else if let Some(s) = v.as_str() {
                    json!(s.trim_start_matches('-'))
                } else {
                    serde_json::from_str(&v.to_string().trim_start_matches('-'))
                        .map_err(|e| e.to_string())?
                }
            }
            Trim => json!(arg(0)?.as_str().ok_or("trim requires text")?.trim()),
            StripPrefix => {
                let v = arg(0)?;
                let p = arg(1)?;
                let s = v.as_str().ok_or("strip_prefix requires text")?;
                json!(
                    s.strip_prefix(p.as_str().ok_or("prefix requires text")?)
                        .unwrap_or(s)
                )
            }
            Map => {
                let k = key(arg(0)?.as_ref());
                let table = arg(1)?;
                match table.get(&k) {
                    Some(value) => value.clone(),
                    None => arg(2)?.into_owned(),
                }
            }
            Get => {
                let v = arg(0)?;
                let p = arg(1)?;
                path(&v, p.as_array().ok_or("get requires a field path")?).clone()
            }
            Decimal | Timestamp => json!(self.unsigned(input)?),
            Choose => {
                if arg(0)?.as_bool() == Some(true) {
                    arg(1)?.into_owned()
                } else {
                    arg(2)?.into_owned()
                }
            }
            Concat => Value::String(
                self.args
                    .iter()
                    .map(|e| e.eval(input).map(|v| key(&v)))
                    .collect::<Result<Vec<_>, _>>()?
                    .concat(),
            ),
        }))
    }
}

pub(super) fn compare(left: &Value, right: &Value) -> Result<std::cmp::Ordering, String> {
    if let (Some(a), Some(b)) = (left.as_u64(), right.as_u64()) {
        return Ok(a.cmp(&b));
    }
    if let (Some(a), Some(b)) = (left.as_i64(), right.as_i64()) {
        return Ok(a.cmp(&b));
    }
    let (Some(left_number), Some(right_number)) = (left.as_number(), right.as_number()) else {
        return Err("Comparison requires numbers".into());
    };
    let sign = |number: &serde_json::Number| {
        let text = number.as_str();
        let mantissa = text.split(['e', 'E']).next().unwrap_or(text);
        if !mantissa.bytes().any(|c| matches!(c, b'1'..=b'9')) {
            std::cmp::Ordering::Equal
        } else if text.starts_with('-') {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    };
    if right.as_u64() == Some(0) {
        return Ok(sign(left_number));
    }
    if left.as_u64() == Some(0) {
        return Ok(sign(right_number).reverse());
    }
    let left = left.to_string();
    let right = right.to_string();
    let a = ExactDecimal::parse(left.trim_start_matches('-'))?;
    let b = ExactDecimal::parse(right.trim_start_matches('-'))?;
    let zero = ExactDecimal::parse("0")?;
    let negative_a = left.starts_with('-') && !a.compare(&zero).is_eq();
    let negative_b = right.starts_with('-') && !b.compare(&zero).is_eq();
    Ok(match (negative_a, negative_b) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (true, true) => b.compare(&a),
        _ => a.compare(&b),
    })
}

fn decimal_value(value: &Value) -> Result<ExactDecimal, String> {
    match value {
        Value::Number(number) => ExactDecimal::parse(number.as_str()),
        Value::String(text) => ExactDecimal::parse(text),
        _ => Err("Expected a decimal price or quantity".into()),
    }
}
