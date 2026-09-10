//! Python constructors for wire layouts and source lifecycle declarations.
use super::super::schema;
use super::{error, export, expressions::operand};
use crate::custom::python::{Iterable, Mapping};
use pyo3::{
    prelude::*,
    types::{PyDict, PyString, PyTuple},
};
use std::sync::Arc;

/// The wire format stored by Binary or Json.
///
/// Construct Binary for length-prefixed records or Json for JSON message rules.
/// Both hold the same Rust format used by Protocol and the adapter compiler.
/// This shared base is not constructed directly from Python.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import models as lm
///
///     messages = lm.Json()
///     ```
#[pyclass(
    name = "Format",
    module = "lobo.replay.adapters.models",
    frozen,
    subclass
)]
pub struct PyFormat {
    pub(super) inner: schema::Format,
}
#[pymethods]
impl PyFormat {
    /// Return a detached dictionary containing this format and its message rules.
    ///
    /// Returns:
    ///     The serializable format definition; editing it does not change the format.
    #[getter]
    fn data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        export(py, &self.inner)
    }
}

/// Read an unsigned integer from a fixed range of bytes.
///
/// Args:
///     offset: Zero-based offset from the containing record payload, group entry,
///         or group dimension header, according to where the field is declared.
///         For Binary.length it is relative to the prefix and must be zero.
///     size: The number of bytes occupied by the integer, from one through eight.
///     byteorder: "big" for most-significant byte first, or "little" for the reverse.
///
/// Raises:
///     ValueError: The byte order or size is unsupported.
///
/// Values remain exact through 2**64 - 1. Signed integers, floating point, null
/// sentinels and decimal scales are not inferred. Declare sentinel handling and
/// unit conversions with expressions using the protocol's schema.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import models as lm
///
///     order_id = lm.UInt(11, 8)
///     ```
#[pyclass(extends=schema::BinaryField, frozen, module="lobo.replay.adapters.models")]
pub struct UInt;
#[pymethods]
impl UInt {
    #[new]
    #[pyo3(signature=(offset,size,*,byteorder="big"))]
    fn new(offset: usize, size: usize, byteorder: &str) -> PyResult<PyClassInitializer<Self>> {
        let field = schema::BinaryField {
            kind: "uint".into(),
            offset,
            size,
            byteorder: byteorder.into(),
        };
        field.validate(usize::MAX).map_err(error)?;
        Ok(PyClassInitializer::from(field).add_subclass(Self))
    }
}
/// Read UTF-8 text from a fixed range of bytes and trim surrounding whitespace.
///
/// Args:
///     offset: Zero-based offset from the record payload (excluding its prefix)
///         or the containing group entry.
///     size: The positive number of bytes occupied by the text, including padding.
///
/// Raises:
///     ValueError: The size is zero or the byte range overflows.
///
/// Decoding is strict UTF-8. Trimming removes Unicode whitespace, including ASCII
/// space padding; it does not remove NUL bytes or translate other encodings.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import models as lm
///
///     stock = lm.Text(24, 8)
///     ```
#[pyclass(extends=schema::BinaryField, frozen, module="lobo.replay.adapters.models")]
pub struct Text;
#[pymethods]
impl Text {
    #[new]
    fn new(offset: usize, size: usize) -> PyResult<PyClassInitializer<Self>> {
        let field = schema::BinaryField {
            kind: "text".into(),
            offset,
            size,
            byteorder: "big".into(),
        };
        field.validate(usize::MAX).map_err(error)?;
        Ok(PyClassInitializer::from(field).add_subclass(Self))
    }
}
#[pymethods]
impl schema::BinaryField {
    /// Return this field's kind, offset, byte size, and byte order.
    ///
    /// Returns:
    ///     A detached dictionary suitable for saving with the record definition.
    #[getter]
    fn data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        export(py, self)
    }
}
#[pymethods]
impl schema::Record {
    #[new]
    #[pyo3(signature=(*,size=None,fields,actions,block_length=None,block_offset=0,groups=None,allow_trailing=false))]
    fn new(
        size: Option<usize>,
        fields: Mapping<String, schema::BinaryField>,
        actions: Iterable<schema::Action>,
        block_length: Option<PyRef<'_, schema::BinaryField>>,
        block_offset: usize,
        groups: Option<Iterable<schema::Group>>,
        allow_trailing: bool,
    ) -> PyResult<Self> {
        let fields = fields.0;
        let record = Self {
            size,
            fields,
            actions: actions.0,
            block_length: block_length.map(|field| field.clone()),
            block_offset,
            groups: Iterable::or_empty(groups),
            allow_trailing,
        };
        record.validate_layout(65535, 0).map_err(error)?;
        Ok(record)
    }
    /// Return the payload size, named fields, and ordered actions as a dictionary.
    ///
    /// Returns:
    ///     A detached representation of this record layout.
    #[getter]
    fn data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        export(py, self)
    }
}

#[pymethods]
impl schema::Group {
    #[new]
    #[pyo3(signature=(name,*,header_size,count,block_length,fields,offset=None,groups=None,max_count=65535,alignment=1))]
    fn new(
        name: String,
        header_size: usize,
        count: PyRef<'_, schema::BinaryField>,
        block_length: PyRef<'_, schema::BinaryField>,
        fields: Mapping<String, schema::BinaryField>,
        offset: Option<usize>,
        groups: Option<Iterable<schema::Group>>,
        max_count: usize,
        alignment: usize,
    ) -> PyResult<Self> {
        let group = Self {
            name,
            header_size,
            count: count.clone(),
            block_length: block_length.clone(),
            fields: fields.0,
            offset,
            groups: Iterable::or_empty(groups),
            max_count,
            alignment,
        };
        let record = schema::Record {
            size: None,
            fields: Default::default(),
            actions: vec![],
            block_length: None,
            block_offset: 0,
            groups: vec![group.clone()],
            allow_trailing: false,
        };
        record.validate_layout(65535, 0).map_err(error)?;
        Ok(group)
    }

    /// Return a detached declaration, including nested group layouts.
    #[getter]
    fn data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        export(py, self)
    }
}

/// Describe the framing and layouts of a binary message stream.
///
/// Each record starts with a length prefix followed by its payload. The length
/// counts payload bytes by default; length_includes_prefix=True counts the entire
/// frame, including that prefix. All other offsets remain relative to the payload.
/// Records whose tags are absent from records are skipped using their length.
///
/// Args:
///     length: A UInt at offset zero describing the length prefix.
///     tag: A UInt identifying which Record layout to use.
///     key: A UInt containing the routing key registered with Register.key.
///     timestamp: A UInt containing the source timestamp in nanoseconds.
///     records: A dictionary from integer tags or single-character strings to
///         Record objects. A character tag is converted to its Unicode code point.
///     max_record_size: The largest accepted payload size, in bytes. Defaults to
///         512 and must be between 1 and 65535. This bounds framing buffers.
///     length_includes_prefix: Set True when the wire length includes its own
///         bytes. For a two-byte prefix storing 10, True reads an eight-byte
///         payload; False reads a ten-byte payload. The default is False.
///
/// Record fields and groups are decoded in Rust. For binary sources construct
/// CustomAdapter with mode=lm.FeedMode.Replay. Variable records use start() and
/// wait(); run() is a fixed-record bulk optimization and rejects them.
/// key supplies the default instrument route. For messages containing several
/// instruments, use explicit Book(symbol, ...) actions inside ForEach, with
/// fields or table lookups to select each entry's instrument. Register.key is
/// only needed for the default header route. Declare version/schema fields in
/// Record.fields and use When to guard the applicable layout's actions.
/// Binary.key and Binary.timestamp are available as Variable("key") and
/// Variable("clock"); they are not automatically added to the Field object.
///
/// Source.file and Source.http supply a continuous stream (gzip is detected for
/// files). Source.packets supplies byte chunks; chunk boundaries are not message
/// or capture-envelope boundaries. Every chunk must contain this declared stream.
/// Capture-file, network, and packet headers are not automatically detected or
/// stripped. Unknown record tags are skipped, but still need a readable common
/// tag/key/timestamp header. Binary.timestamp must already be unsigned nanoseconds
/// on a common time axis for replay scheduling. Converting an action's timestamp
/// does not change the framing clock used to pace the stream.
///
/// Raises:
///     TypeError: A field or record has the wrong declaration type.
///     ValueError: A string tag contains more than one character. Protocol checks
///         the completed layout's field ranges and record sizes during setup.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import Protocol
///     from lobo.replay.adapters import models as lm
///
///     wire = lm.Binary(length=lm.UInt(0, 2), tag=lm.UInt(0, 1), key=lm.UInt(1, 2),
///                   timestamp=lm.UInt(3, 8),
///                   records={"H": lm.Record(size=11, fields={}, actions=())})
///     protocol = Protocol(wire)
///     ```
///
///     This complete example decodes two entries from one inclusive-length frame:
///
///     ```python
///     from lobo.replay.adapters import CustomAdapter, Protocol
///     from lobo.replay.adapters import models as lm, expressions as le
///
///     orders = lm.Group("orders", header_size=2,
///         block_length=lm.UInt(0, 1), count=lm.UInt(1, 1),
///         fields={"id": lm.UInt(0, 1), "quantity": lm.UInt(1, 1)})
///     wire = lm.Binary(length=lm.UInt(0, 2, byteorder="little"),
///         length_includes_prefix=True,
///         tag=lm.UInt(0, 1), key=lm.UInt(1, 1), timestamp=lm.UInt(2, 1),
///         records={1: lm.Record(fields={}, groups=[orders], actions=[
///             lm.Book("XYZ", le.ForEach(le.Field("orders"),
///                 lm.Add(id=le.Field("id"), side="buy", price=100,
///                     quantity=le.Field("quantity"))), snapshot=True)])})
///     # Prefix: total size 11. Payload: tag, key, clock, block size, count,
///     # then two (id, quantity) entries. The prefix is excluded from offsets.
///     source = lm.Source.packets([bytes([11, 0, 1, 0, 5, 2, 2, 7, 9, 8, 3])])
///     with CustomAdapter(Protocol(wire), source, symbol="XYZ",
///             mode=lm.FeedMode.Replay, instruments=[lm.Instrument("XYZ", 0, 0)]) as feed:
///         feed.start()
///         feed.wait()
///         assert feed.levels("XYZ")[0]["quantity"] == 12
///     ```
#[pyclass(extends=PyFormat, frozen, module="lobo.replay.adapters.models")]
pub struct Binary;
#[pymethods]
impl Binary {
    #[new]
    #[pyo3(signature=(*,length,tag,key,timestamp,records: "dict[int | str, Record]",max_record_size=512,length_includes_prefix=false))]
    fn new(
        length: PyRef<'_, schema::BinaryField>,
        tag: PyRef<'_, schema::BinaryField>,
        key: PyRef<'_, schema::BinaryField>,
        timestamp: PyRef<'_, schema::BinaryField>,
        records: &Bound<'_, PyDict>,
        max_record_size: usize,
        length_includes_prefix: bool,
    ) -> PyResult<PyClassInitializer<Self>> {
        let records = records
            .iter()
            .map(|(tag, record)| {
                let key = if let Ok(tag) = tag.cast::<PyString>() {
                    let text = tag.to_str()?;
                    let mut chars = text.chars();
                    let first = chars
                        .next()
                        .ok_or_else(|| error("Record tags must be single characters"))?;
                    if chars.next().is_some() {
                        return Err(error("Record tags must be single characters"));
                    }
                    u64::from(u32::from(first))
                } else {
                    tag.extract::<u64>()?
                };
                Ok((key, record.extract::<PyRef<'_, schema::Record>>()?.clone()))
            })
            .collect::<PyResult<_>>()?;
        Ok(PyClassInitializer::from(PyFormat {
            inner: schema::Format::Binary(schema::Binary {
                length: length.clone(),
                tag: tag.clone(),
                key: key.clone(),
                timestamp: timestamp.clone(),
                records,
                max_record_size,
                length_includes_prefix,
            }),
        })
        .add_subclass(Self))
    }
}
/// Describe a JSON stream as an ordered collection of message rules.
///
/// Every matching Message runs its actions, in declaration order. A message that
/// matches no rule is ignored. Rules can distinguish control messages, snapshots,
/// updates, and trades without assigning those shapes to separate adapters.
///
/// Args:
///     *messages: Message objects containing a condition and the actions it enables.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import expressions as le
///     from lobo.replay.adapters import models as lm
///
///     wire = lm.Json(lm.Message(le.Field("type").eq("heartbeat"), le.Let("alive", True)))
///     ```
#[pyclass(extends=PyFormat, frozen, module="lobo.replay.adapters.models")]
pub struct Json;
#[pymethods]
impl Json {
    #[new]
    #[pyo3(signature=(*messages: "Message"))]
    fn new(messages: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        let messages = messages
            .iter()
            .map(|m| Ok(m.extract::<PyRef<'_, schema::Message>>()?.clone()))
            .collect::<PyResult<_>>()?;
        Ok(PyClassInitializer::from(PyFormat {
            inner: schema::Format::Json { messages },
        })
        .add_subclass(Self))
    }
}
#[pymethods]
impl schema::Message {
    #[new]
    #[pyo3(signature=(condition,*actions: "Action"))]
    fn new(condition: &Bound<'_, PyAny>, actions: &Bound<'_, PyTuple>) -> PyResult<Self> {
        Ok(Self {
            condition: operand(condition)?,
            actions: super::actions::actions(Some(actions.as_any()))?,
        })
    }
    /// Return this rule's condition and ordered actions as a detached dictionary.
    ///
    /// Returns:
    ///     The serializable rule definition.
    #[getter]
    fn data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        export(py, self)
    }
}
#[pymethods]
impl schema::Bootstrap {
    #[new]
    #[pyo3(signature=(name,url,*actions: "Action"))]
    fn new(name: String, url: String, actions: &Bound<'_, PyTuple>) -> PyResult<Self> {
        Ok(Self {
            name,
            url,
            actions: super::actions::actions(Some(actions.as_any()))?,
        })
    }
    /// Return the discovery request's name, URL, and response actions.
    ///
    /// Returns:
    ///     A detached dictionary representing this startup request.
    #[getter]
    fn data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        export(py, self)
    }
}
#[pymethods]
impl schema::Checksum {
    #[new]
    #[pyo3(signature=(expected,*,view="levels",depth=10,sides=None,fields=None,separator="",interleave=false,format="decimal_digits",signed=false,priority="fifo",on_failure=None))]
    fn new(
        expected: &Bound<'_, PyAny>,
        view: &str,
        depth: usize,
        sides: Option<Vec<String>>,
        fields: Option<Vec<String>>,
        separator: &str,
        interleave: bool,
        format: &str,
        signed: bool,
        priority: &str,
        on_failure: Option<Iterable<schema::Action>>,
    ) -> PyResult<Self> {
        Ok(Self {
            prepared: Arc::default(),
            expected: operand(expected)?,
            view: view.into(),
            depth,
            sides: sides.unwrap_or_else(|| vec!["sell".into(), "buy".into()]),
            fields: fields.unwrap_or_else(|| vec!["price".into(), "quantity".into()]),
            separator: separator.into(),
            interleave,
            format: format.into(),
            signed,
            priority: priority.into(),
            on_failure: Iterable::or_empty(on_failure),
        })
    }
    /// Return checksum operands and formatting options as a detached dictionary.
    ///
    /// Returns:
    ///     The public checksum definition, excluding prepared book policy state.
    #[getter]
    fn data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        export(py, self)
    }
}
#[pymethods]
impl schema::Definition {
    #[new]
    #[pyo3(signature=(format,*,connect=None,subscriptions=None,bootstrap=None,keepalive=None,simulation_note=None,reconcile_window_ns=250_000_000,reconcile_capacity=4096,symbols_per_connection=0))]
    fn new(
        format: PyRef<'_, PyFormat>,
        connect: Option<Iterable<schema::Action>>,
        subscriptions: Option<Iterable<schema::Action>>,
        bootstrap: Option<Iterable<schema::Bootstrap>>,
        keepalive: Option<Iterable<schema::Action>>,
        simulation_note: Option<String>,
        reconcile_window_ns: u64,
        reconcile_capacity: usize,
        symbols_per_connection: usize,
    ) -> PyResult<Self> {
        let bootstrap = Iterable::or_empty(bootstrap);
        let definition = Self {
            format: format.inner.clone(),
            connect: Iterable::or_empty(connect),
            subscriptions: Iterable::or_empty(subscriptions),
            bootstrap,
            keepalive: Iterable::or_empty(keepalive),
            simulation_note,
            reconcile_window_ns,
            reconcile_capacity,
            symbols_per_connection,
        };
        definition.validate().map_err(error)?;
        Ok(definition)
    }
    /// Return the complete protocol as a detached dictionary.
    ///
    /// Returns:
    ///     A serializable definition for inspection or storage. CustomAdapter
    ///     accepts Protocol directly and does not require this conversion.
    #[getter]
    fn data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        export(py, self)
    }
}
