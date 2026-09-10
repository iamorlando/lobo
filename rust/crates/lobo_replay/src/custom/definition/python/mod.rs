//! Construct adapter declarations as Rust values from Python.
//!
//! Expressions describe how to read a message. Actions describe the changes to
//! apply to a book. A protocol combines those declarations with connection and
//! discovery behavior. Constructors run during adapter setup; the completed
//! [`super::schema::Definition`] goes directly to the compiler.
#![warn(missing_docs)]

mod actions;
mod expressions;
mod formats;

use super::{expression::Expr, schema};
use pyo3::{
    exceptions::{PyTypeError, PyValueError},
    prelude::*,
    types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple},
};
use serde::Serialize;
use serde_json::Value;

fn error(error: impl ToString) -> PyErr {
    PyValueError::new_err(error.to_string())
}
fn export(py: Python<'_>, value: &impl Serialize) -> PyResult<Py<PyAny>> {
    to_python(py, serde_json::to_value(value).map_err(error)?)
}
// Export exact integer constants, including IDs larger than u64, without a float.
fn to_python(py: Python<'_>, value: Value) -> PyResult<Py<PyAny>> {
    use pyo3::IntoPyObjectExt;
    match value {
        Value::Null => Ok(py.None()),
        Value::Bool(value) => value.into_py_any(py),
        Value::String(value) => value.into_py_any(py),
        Value::Number(value) => {
            let text = value.as_str();
            if text.bytes().any(|b| matches!(b, b'.' | b'e' | b'E')) {
                text.parse::<f64>().map_err(error)?.into_py_any(py)
            } else {
                Ok(py.get_type::<PyInt>().call1((text,))?.unbind())
            }
        }
        Value::Array(values) => {
            let result = PyList::empty(py);
            for value in values {
                result.append(to_python(py, value)?)?;
            }
            Ok(result.into_any().unbind())
        }
        Value::Object(values) => {
            let result = PyDict::new(py);
            for (key, value) in values {
                result.set_item(key, to_python(py, value)?)?;
            }
            Ok(result.into_any().unbind())
        }
    }
}
fn literal_value(value: &Bound<'_, PyAny>) -> PyResult<Value> {
    if value.is_none() {
        return Ok(Value::Null);
    }
    if let Ok(value) = value.cast::<PyBool>() {
        return Ok(Value::Bool(value.is_true()));
    }
    if value.is_instance_of::<PyInt>() {
        return serde_json::from_str(value.str()?.to_str()?).map_err(error);
    }
    if let Ok(value) = value.cast::<PyFloat>() {
        return serde_json::Number::from_f64(value.value())
            .map(Value::Number)
            .ok_or_else(|| error("Expression constants must be finite numbers"));
    }
    if let Ok(value) = value.cast::<PyString>() {
        return Ok(Value::String(value.to_str()?.into()));
    }
    if let Ok(value) = value.cast::<PyBytes>() {
        let bytes = value.as_bytes();
        if !bytes.is_ascii() {
            return Err(error("Declaration byte strings must contain ASCII text"));
        }
        return Ok(Value::String(
            std::str::from_utf8(bytes).map_err(error)?.into(),
        ));
    }
    Err(PyTypeError::new_err(
        "Expected an expression, a JSON-compatible constant, or ASCII bytes",
    ))
}
fn specification_value(value: &Bound<'_, PyAny>) -> PyResult<Value> {
    macro_rules! declaration {
        ($($ty:ty),* $(,)?) => {$(
            if let Ok(value) = value.extract::<PyRef<'_, $ty>>() {
                return serde_json::to_value(&*value).map_err(error);
            }
        )*};
    }
    declaration!(
        Expr,
        schema::Action,
        schema::Checksum,
        schema::Message,
        schema::BinaryField,
        schema::Record,
        schema::Bootstrap,
        schema::Definition
    );
    if let Ok(value) = value.extract::<PyRef<'_, formats::PyFormat>>() {
        return serde_json::to_value(&value.inner).map_err(error);
    }
    if let Ok(dict) = value.cast::<PyDict>() {
        return dict
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.str()?.to_str()?.to_owned(),
                    specification_value(&value)?,
                ))
            })
            .collect::<PyResult<serde_json::Map<_, _>>>()
            .map(Value::Object);
    }
    if value.is_instance_of::<PyList>() || value.is_instance_of::<PyTuple>() {
        return value
            .try_iter()?
            .map(|value| specification_value(&value?))
            .collect::<PyResult<Vec<_>>>()
            .map(Value::Array);
    }
    literal_value(value)
}
/// Export a declaration as ordinary Python dictionaries, lists, and constants.
///
/// This is useful for saving a protocol definition or comparing it with another
/// definition. Adapter construction accepts the Rust-backed objects directly;
/// callers do not need to export a protocol before passing it to CustomAdapter.
///
/// Args:
///     value: A protocol, expression, action, format, or a list or dictionary
///         containing declarations and JSON-compatible constants.
///
/// Returns:
///     A detached Python value that can be passed to json.dumps(). Mutating the
///     returned value does not change the original declaration.
///
/// Raises:
///     TypeError: A value cannot be represented in a declaration.
///     ValueError: A number is not finite or a byte string is not ASCII.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import expressions as le
///     from lobo.replay.adapters import models as lm
///     assert lm.specification(le.Field("price"))["path"] == ["price"]
///     ```
#[pyfunction]
pub fn specification(value: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    to_python(value.py(), specification_value(value)?)
}

/// AUTOGENERATED FROM RUST. DO NOT EDIT.
///
/// Describe wire formats, book changes, instruments, and source capabilities.
/// Use these declarations inside Protocol and supply data-reading expressions
/// from lobo.replay.adapters.expressions for their incoming values.
#[pymodule]
#[pyo3(name = "models", submodule, module = "lobo.replay.adapters")]
pub mod models {
    #[pymodule_export]
    use super::super::schema::{Action, BinaryField, Bootstrap, Checksum, Message, Record};
    #[pymodule_export]
    use super::actions::{
        Add, Book, Cancel, Directory, DirectoryComplete, Execute, Fail, Level, Modify,
        OrderCommand, Register, Remove, Replace, RestoreOrder, Send, Subscribe, Trade,
        TradeHistory, Upsert,
    };
    #[pymodule_export]
    use super::formats::{Binary, Json, PyFormat, Text, UInt};
    #[pymodule_export]
    use super::specification;
    #[pymodule_export]
    use crate::custom::TextHeartbeat;
    #[pymodule_export]
    use crate::custom::python::{PyAdapterInfo, PyInstrument, PySource};
    #[pymodule_export]
    use lobo_context::{BookLevel, FeedMode};
}

/// AUTOGENERATED FROM RUST. DO NOT EDIT.
///
/// Read incoming fields, transform values, and describe control flow.
/// Combine these expressions with the book operations in models when defining
/// a Protocol.
#[pymodule]
#[pyo3(name = "expressions", submodule, module = "lobo.replay.adapters")]
pub mod expression_exports {
    #[pymodule_export]
    use super::super::expression::Expr;
    #[pymodule_export]
    use super::actions::{CheckSequence, ForEach, Let, Remember, When};
    #[pymodule_export]
    use super::expressions::{Choose, Concat, Field, Root, Variable, expression};
}
