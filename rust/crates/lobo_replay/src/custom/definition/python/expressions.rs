//! Python expression constructors backed by the compiler's Rust expression type.
use super::super::expression::{Expr, Operator};
use super::{error, export, literal_value, specification_value};
use pyo3::{
    exceptions::PyTypeError,
    prelude::*,
    types::{PyDict, PyList, PyTuple},
};
use serde_json::Value;

pub(super) fn node(op: Operator, args: Vec<Expr>) -> Expr {
    Expr {
        op,
        args,
        path: vec![],
        value: Value::Null,
        name: String::new(),
    }
}
pub(super) fn literal(value: impl Into<Value>) -> Expr {
    Expr {
        value: value.into(),
        ..node(Operator::Literal, vec![])
    }
}
pub(super) fn variable(name: &str) -> Expr {
    Expr {
        name: name.into(),
        ..node(Operator::Variable, vec![])
    }
}
pub(super) fn operand(value: &Bound<'_, PyAny>) -> PyResult<Expr> {
    if let Ok(value) = value.extract::<PyRef<'_, Expr>>() {
        return Ok(value.clone());
    }
    if let Ok(dict) = value.cast::<PyDict>() {
        let mut keys = Vec::with_capacity(dict.len());
        let mut args = Vec::with_capacity(dict.len());
        for (key, value) in dict.iter() {
            keys.push(key.str()?.to_str()?.to_owned());
            args.push(operand(&value)?);
        }
        return Ok(if args.iter().all(|e| matches!(e.op, Operator::Literal)) {
            literal(Value::Object(
                keys.into_iter()
                    .zip(args)
                    .map(|(key, arg)| (key, arg.value))
                    .collect(),
            ))
        } else {
            Expr {
                path: keys.into_iter().map(Value::String).collect(),
                ..node(Operator::Object, args)
            }
        });
    }
    if value.is_instance_of::<PyList>() || value.is_instance_of::<PyTuple>() {
        let args = value
            .try_iter()?
            .map(|v| operand(&v?))
            .collect::<PyResult<Vec<_>>>()?;
        return Ok(if args.iter().all(|e| matches!(e.op, Operator::Literal)) {
            literal(Value::Array(args.into_iter().map(|e| e.value).collect()))
        } else {
            node(Operator::Array, args)
        });
    }
    Ok(literal(literal_value(value)?))
}
pub(super) fn optional(value: Option<&Bound<'_, PyAny>>) -> PyResult<Expr> {
    value
        .map(operand)
        .transpose()
        .map(|e| e.unwrap_or_else(|| literal(Value::Null)))
}
pub(super) fn clock(value: Option<&Bound<'_, PyAny>>) -> PyResult<Expr> {
    value
        .map(operand)
        .transpose()
        .map(|e| e.unwrap_or_else(|| variable("clock")))
}
fn path(parts: &Bound<'_, PyTuple>) -> PyResult<Vec<Value>> {
    parts
        .iter()
        .map(|part| {
            let value = literal_value(&part)?;
            if value.is_string() || value.as_i64().is_some() {
                Ok(value)
            } else {
                Err(PyTypeError::new_err(
                    "Field paths contain string keys or signed integer indices",
                ))
            }
        })
        .collect()
}

/// Turn a constant, container, or existing expression into an expression.
///
/// Dictionaries and lists can contain expressions. For example, a Send action
/// can combine fixed JSON keys with Variable("symbol") in a subscription request.
/// Entirely constant containers remain constants in the compiled definition.
///
/// Args:
///     value: An expression, None, a boolean, integer, finite float, string,
///         ASCII bytes, or a list, tuple, or dictionary containing these values.
///         Dictionary keys are converted to strings.
///
/// Returns:
///     An immutable Expression describing the value to compute for a message.
///
/// Raises:
///     TypeError: The value is not supported by the declaration language.
///     ValueError: A float is not finite or a byte string is not ASCII.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters.expressions import expression, Variable
///     request = expression({"method": "subscribe", "symbol": Variable("symbol")})
///     ```
#[pyfunction]
pub fn expression(value: &Bound<'_, PyAny>) -> PyResult<Expr> {
    operand(value)
}

#[pymethods]
impl Expr {
    /// Read an expression from its exported dictionary representation.
    ///
    /// Prefer Field, Variable, Choose, and the expression methods when writing an
    /// adapter. This constructor is for restoring a previously exported expression.
    ///
    /// Args:
    ///     data: The dictionary returned by specification(expression), containing
    ///         an op name and that operator's operands or field path.
    ///
    /// Raises:
    ///     ValueError: The operator, operands, or dictionary structure is invalid.
    #[new]
    fn python_new(data: &Bound<'_, PyDict>) -> PyResult<Self> {
        let expr: Self =
            serde_json::from_value(specification_value(data.as_any())?).map_err(error)?;
        expr.validate().map_err(error)?;
        Ok(expr)
    }
    /// Return a detached dictionary describing this expression.
    ///
    /// Returns:
    ///     A serializable dictionary. Editing it does not mutate this expression.
    #[getter]
    fn data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        export(py, self)
    }
    /// Compare this expression with another value for equality.
    ///
    /// Args:
    ///     other: An expression or constant to compare with this expression.
    ///
    /// Returns:
    ///     A boolean expression. Numeric comparisons preserve integer and decimal
    ///     precision; this method does not evaluate a Python boolean immediately.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     is_add = le.Field("event").eq("add")
    ///     ```
    fn eq(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(node(Operator::Eq, vec![self.clone(), operand(other)?]))
    }
    /// Test whether this expression differs from another value.
    ///
    /// Args:
    ///     other: An expression or constant to compare with this expression.
    ///
    /// Returns:
    ///     A boolean expression that is true when the values are unequal.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     has_quantity = le.Field("quantity").ne(0)
    ///     ```
    fn ne(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(node(Operator::Ne, vec![self.clone(), operand(other)?]))
    }
    /// Test whether this numeric expression is greater than another number.
    ///
    /// Args:
    ///     other: A numeric constant or expression producing a number.
    ///
    /// Returns:
    ///     A boolean expression using exact numeric comparison. Invalid numeric
    ///     input is reported when the corresponding message is processed.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     is_bid = le.Field("signed_amount").gt(0)
    ///     ```
    fn gt(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(node(Operator::Gt, vec![self.clone(), operand(other)?]))
    }
    /// Test whether this numeric expression is less than another number.
    ///
    /// Args:
    ///     other: A numeric constant or expression producing a number.
    ///
    /// Returns:
    ///     A boolean expression using exact numeric comparison.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     is_ask = le.Field("signed_amount").lt(0)
    ///     ```
    fn lt(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(node(Operator::Lt, vec![self.clone(), operand(other)?]))
    }
    /// Require both conditions, evaluating the right condition only if needed.
    ///
    /// Args:
    ///     other: The second boolean expression or boolean constant.
    ///
    /// Returns:
    ///     A boolean expression. Use parentheses around comparisons combined by &.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     valid = le.Field("price").exists() & le.Field("price").gt(0)
    ///     ```
    fn __and__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(node(Operator::And, vec![self.clone(), operand(other)?]))
    }
    /// Accept either condition, evaluating the right condition only if needed.
    ///
    /// Args:
    ///     other: The second boolean expression or boolean constant.
    ///
    /// Returns:
    ///     A boolean expression to use in Message, When, or Choose.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     is_order = le.Field("event").eq("add") | le.Field("event").eq("update")
    ///     ```
    fn __or__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(node(Operator::Or, vec![self.clone(), operand(other)?]))
    }
    /// Reject immediate Python truth testing of an unevaluated expression.
    ///
    /// Raises:
    ///     TypeError: Expressions describe future messages. Use &, |, When, or
    ///         Choose instead of Python and, or, if, or bool(expression).
    fn __bool__(&self) -> PyResult<bool> {
        Err(PyTypeError::new_err(
            "Use &, |, and .eq() to combine adapter expressions",
        ))
    }
    /// Test whether the selected value exists and is not JSON null.
    ///
    /// Returns:
    ///     A boolean expression. Zero, false, and an empty string count as present.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     has_execution_price = le.Field("execution_price").exists()
    ///     ```
    fn exists(&self) -> Self {
        node(Operator::Exists, vec![self.clone()])
    }
    /// Test whether the selected value is an array.
    ///
    /// Returns:
    ///     A boolean expression, useful for distinguishing snapshot arrays from
    ///     control messages before entering ForEach.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     contains_rows = le.Field("orders").is_array()
    ///     ```
    fn is_array(&self) -> Self {
        node(Operator::IsArray, vec![self.clone()])
    }
    /// Count the selected array's items, object's keys, or text's bytes.
    ///
    /// Returns:
    ///     An integer expression. Missing values and other scalar types produce 0.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     nonempty = le.Field("orders").length().gt(0)
    ///     ```
    fn length(&self) -> Self {
        node(Operator::Length, vec![self.clone()])
    }
    /// Remove the sign of a numeric value without converting it to a float.
    ///
    /// Returns:
    ///     A nonnegative numeric expression. Apply decimal(places) afterwards
    ///     when the source uses fractional quantities.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     quantity = le.Field("signed_amount").absolute().decimal(8)
    ///     ```
    fn absolute(&self) -> Self {
        node(Operator::Abs, vec![self.clone()])
    }
    /// Remove whitespace from both ends of a text value.
    ///
    /// Returns:
    ///     A text expression. Non-text input produces an error during processing.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     symbol = le.Field("symbol").trim()
    ///     ```
    fn trim(&self) -> Self {
        node(Operator::Trim, vec![self.clone()])
    }
    /// Remove a leading prefix from text when the prefix matches.
    ///
    /// Args:
    ///     prefix: The literal prefix to remove. Other text remains unchanged.
    ///
    /// Returns:
    ///     A text expression with at most one matching prefix removed.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     symbol = le.Field("symbol").strip_prefix("t")
    ///     ```
    fn strip_prefix(&self, prefix: &str) -> Self {
        node(Operator::StripPrefix, vec![self.clone(), literal(prefix)])
    }
    /// Translate a source value through a fixed lookup dictionary.
    ///
    /// Args:
    ///     values: Source values mapped to output values. Keys are converted to
    ///         text, so integer wire codes can be used as dictionary keys.
    ///     default: The expression or constant returned if no key matches.
    ///         Defaults to None. A matching key mapped to None still matches.
    ///
    /// Returns:
    ///     An expression yielding the mapped value, or the fallback for an unknown key.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     side = le.Field("side").map({"B": "buy", "S": "sell"})
    ///     ```
    #[pyo3(signature=(values: "dict[Any, Any]",*,default=None))]
    fn map(
        &self,
        values: &Bound<'_, PyDict>,
        default: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        Ok(node(
            Operator::Map,
            vec![self.clone(), operand(values.as_any())?, optional(default)?],
        ))
    }
    /// Retrieve a value previously stored by Remember for this connection.
    ///
    /// Args:
    ///     table: The name of the table used by Remember. This expression supplies
    ///         the key, for example a channel ID assigned by the server.
    ///
    /// Returns:
    ///     The remembered value, or None when the connection has no matching key.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     symbol = le.Field("channel").lookup("channels").get("symbol")
    ///     ```
    fn lookup(&self, table: &str) -> Self {
        node(Operator::Lookup, vec![literal(table), self.clone()])
    }
    /// Select a nested field from the value produced by this expression.
    ///
    /// Args:
    ///     *path: String object keys or integer array indices. Negative indices
    ///         count backwards from the end of an array.
    ///
    /// Returns:
    ///     An expression yielding the selected value, or None for a missing path.
    ///
    /// Raises:
    ///     TypeError: A path component is neither a string nor an integer.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     last_price = le.Variable("rows").get(-1, "price")
    ///     ```
    #[pyo3(signature=(*path: "str | int"))]
    fn get(&self, path: &Bound<'_, PyTuple>) -> PyResult<Self> {
        Ok(node(
            Operator::Get,
            vec![self.clone(), literal(Value::Array(self::path(path)?))],
        ))
    }
    /// Convert a decimal value to unsigned integer atoms at a chosen precision.
    ///
    /// Args:
    ///     places: A decimal count or expression yielding that count, normally
    ///         the instrument's price_decimals or quantity_decimals. For places=2,
    ///         a source value of "12.34" becomes 1234 book atoms.
    ///
    /// Returns:
    ///     An integer expression. Decimal conversion preserves the original wire
    ///     width for level checksums. Nonzero digits beyond the declared precision
    ///     produce a source error.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     price = le.Field("price").decimal(le.Variable("price_decimals"))
    ///     ```
    fn decimal(&self, places: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(node(
            Operator::Decimal,
            vec![self.clone(), operand(places)?],
        ))
    }
    /// Convert a source timestamp to nonnegative nanoseconds.
    ///
    /// Args:
    ///     unit: "ns", "us", "ms", or "s" for unsigned source-time counts; use
    ///         "rfc3339" for text such as "2026-01-01T09:30:00Z". Defaults to "ns".
    ///
    /// Returns:
    ///     An integer expression suitable for an action's timestamp argument.
    ///     Invalid values and overflow are errors when their messages are processed.
    ///
    /// Raises:
    ///     ValueError: The declared unit is unsupported.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     execution_time = le.Field("time_ms").timestamp("ms")
    ///     ```
    #[pyo3(signature=(unit="ns"))]
    fn timestamp(&self, unit: &str) -> PyResult<Self> {
        if !["ns", "us", "ms", "s", "rfc3339"].contains(&unit) {
            return Err(error("Unsupported timestamp unit"));
        }
        Ok(node(Operator::Timestamp, vec![self.clone(), literal(unit)]))
    }
}

macro_rules! expression_class {
    ($(#[$doc:meta])* $name:ident, ($($arg:ident : $ty:ty),*), $signature:tt, $body:expr) => {
        $(#[$doc])*
        #[pyclass(extends=Expr, frozen, module="lobo.replay.adapters.expressions")]
        pub struct $name;
        #[pymethods]
        impl $name {
            $(#[$doc])*
            #[new]
            #[pyo3(signature=$signature)]
            fn new($($arg:$ty),*) -> PyResult<PyClassInitializer<Self>> { Ok(PyClassInitializer::from($body).add_subclass(Self)) }
        }
    };
}
expression_class! {
    /// Select a value from the message or the current ForEach item.
    ///
    /// Args:
    ///     *path: String object keys or integer array indices, in traversal order.
    ///         Negative indices count from the end. An empty path selects the
    ///         entire current item. Missing paths produce None.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     price = le.Field("data", 0, "price")
    ///     row = le.Field()  # Inside ForEach, this selects the current row.
    ///     ```
    Field, (path: &Bound<'_, PyTuple>), (*path: "str | int"), Expr { path:self::path(path)?, ..node(Operator::Field,vec![]) }
}
expression_class! {
    /// Select a value from the complete incoming message, even inside a loop.
    ///
    /// Args:
    ///     *path: String object keys or integer array indices. The traversal always
    ///         starts at the root message. An empty path selects the whole message.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     timestamp = le.Root("timestamp").timestamp("ms")
    ///     ```
    Root, (path: &Bound<'_, PyTuple>), (*path: "str | int"), Expr { path:self::path(path)?, ..node(Operator::Root,vec![]) }
}
expression_class! {
    /// Refer to a named value supplied by the session or assigned by Let.
    ///
    /// Built-in names are symbol, connection, clock, price_decimals,
    /// quantity_decimals, and key. Book and subscription scopes refresh instrument
    /// metadata. clock is nanoseconds; binary records supply their source clock,
    /// while JSON packets initially use the session's wall clock unless overridden.
    /// Remember stores connection-specific lookup data; use it instead of Let
    /// for channel maps that must be cleared and rebuilt on reconnect.
    ///
    /// Args:
    ///     name: The variable name. An unassigned variable yields None.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     symbol = le.Variable("symbol")
    ///     price = le.Field("price").decimal(le.Variable("price_decimals"))
    ///     ```
    Variable, (name: &str), (name), variable(name)
}
expression_class! {
    /// Select between two values using a condition evaluated for each message.
    ///
    /// Args:
    ///     condition: A boolean expression or constant. Only true selects yes.
    ///     yes: The expression or constant to use when the condition is true.
    ///     no: The expression or constant to use otherwise. The unselected branch
    ///         is not evaluated, so it may refer to fields absent from that message.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     side = le.Choose(le.Field("amount").gt(0), "buy", "sell")
    ///     ```
    Choose, (condition: &Bound<'_, PyAny>, yes: &Bound<'_, PyAny>, no: &Bound<'_, PyAny>), (condition,yes,no), node(Operator::Choose,vec![operand(condition)?,operand(yes)?,operand(no)?])
}
expression_class! {
    /// Join text and expression results to form a text value.
    ///
    /// Args:
    ///     *values: Expressions or constants, concatenated in order. Text is used
    ///         directly; other values use their JSON representation.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     wire_symbol = le.Concat("t", le.Variable("symbol"))
    ///     ```
    Concat, (values: &Bound<'_, PyTuple>), (*values), node(Operator::Concat,values.iter().map(|v|operand(&v)).collect::<PyResult<Vec<_>>>()?)
}
