//! Wire layouts, book operations, and connection lifecycle declarations.
//!
//! Construct a Definition during setup and call Definition::validate before
//! preparing an adapter. The Python Protocol constructor performs this check.
#![warn(missing_docs)]

use super::expression::Expr;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// An immutable declaration of a book change or source lifecycle operation.
///
/// Use the named Python constructors, such as Add, Book, and Send, to describe
/// what happens when a message matches. Creating an action does not execute it.
///
/// Args:
///     kind: For the low-level Action constructor, the serialized operation name.
///     **fields: That operation's arguments, using exported expression dictionaries
///         or expression objects. Prefer the named constructors for new definitions.
///
/// Examples:
/// ```python
/// from lobo.replay.adapters import expressions as le
/// from lobo.replay.adapters import models as lm
///
/// action = lm.Add(id=le.Field("id"), side="buy", price=100, quantity=20)
/// ```
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(
        name = "Action",
        module = "lobo.replay.adapters.models",
        frozen,
        from_py_object,
        subclass
    )
)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Action {
    #[serde(flatten)]
    /// The operation and operands to execute when this action is reached.
    pub operation: Operation,
}
/// The operations understood by the adapter compiler.
///
/// Expressions are evaluated in the current message, item, variable, and routing
/// context. Nested operations execute in declaration order.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Operation {
    /// Mark the end of the instrument directory during replay discovery.
    DirectoryComplete,
    /// Replace the known directory from an array and handle removed instruments.
    Directory {
        /// The expression producing the array to iterate.
        items: Expr,
        /// The expression producing the instrument symbol to register or select.
        symbol: Expr,
        /// Nested actions executed in declaration order for the current item or book.
        actions: Vec<Action>,
        /// Actions run for directory symbols absent from a replacement directory.
        on_remove: Vec<Action>,
    },
    /// Apply an order API command to the selected book.
    OrderCommand {
        /// The expression producing this operation's input value.
        value: Expr,
        /// The expression producing source time in nanoseconds.
        timestamp: Expr,
    },
    /// Restore one order from a server snapshot in its supplied queue position.
    RestoreOrder {
        /// The expression producing this operation's input value.
        value: Expr,
    },
    /// Insert a visible resting order without matching.
    Add {
        /// The expression producing the source order or public trade identifier.
        id: Expr,
        /// The expression producing "buy" or "sell"; Trade uses the resting maker side.
        side: Expr,
        /// The expression producing the price in integer atoms; nullable where the operation permits it.
        price: Expr,
        /// The expression producing quantity atoms; the operation determines delta or absolute semantics.
        quantity: Expr,
        /// The expression producing source time in nanoseconds.
        timestamp: Expr,
    },
    /// Consume resting quantity and publish an execution.
    Execute {
        /// The expression producing the source order or public trade identifier.
        id: Expr,
        /// The expression producing quantity atoms; the operation determines delta or absolute semantics.
        quantity: Expr,
        /// The expression producing the price in integer atoms; nullable where the operation permits it.
        price: Expr,
        /// The expression producing source time in nanoseconds.
        timestamp: Expr,
    },
    /// Subtract a cancellation quantity without reporting a trade.
    Cancel {
        /// The expression producing the source order or public trade identifier.
        id: Expr,
        /// The expression producing quantity atoms; the operation determines delta or absolute semantics.
        quantity: Expr,
        /// The expression producing source time in nanoseconds.
        timestamp: Expr,
    },
    /// Delete a resting order without reporting a trade.
    Remove {
        /// The expression producing the source order or public trade identifier.
        id: Expr,
        /// The expression producing source time in nanoseconds.
        timestamp: Expr,
    },
    /// Set an existing order's absolute visible quantity.
    Modify {
        /// The expression producing the source order or public trade identifier.
        id: Expr,
        /// The expression producing quantity atoms; the operation determines delta or absolute semantics.
        quantity: Expr,
        /// The expression producing source time in nanoseconds.
        timestamp: Expr,
    },
    /// Replace a resting order with a new identifier, price, and quantity.
    Replace {
        /// The expression producing the source order or public trade identifier.
        id: Expr,
        /// The expression producing the replacement order identifier.
        new_id: Expr,
        /// The expression producing the price in integer atoms; nullable where the operation permits it.
        price: Expr,
        /// The expression producing quantity atoms; the operation determines delta or absolute semantics.
        quantity: Expr,
        /// The expression producing source time in nanoseconds.
        timestamp: Expr,
    },
    /// Create or update an order from absolute state; a null price removes it.
    Upsert {
        /// The expression producing the source order or public trade identifier.
        id: Expr,
        /// The expression producing "buy" or "sell"; Trade uses the resting maker side.
        side: Expr,
        /// The expression producing the price in integer atoms; nullable where the operation permits it.
        price: Expr,
        /// The expression producing quantity atoms; the operation determines delta or absolute semantics.
        quantity: Expr,
        /// The expression producing source time in nanoseconds.
        timestamp: Expr,
    },
    /// Set the absolute visible quantity of a price level; zero removes it.
    Level {
        /// The expression producing "buy" or "sell"; Trade uses the resting maker side.
        side: Expr,
        /// The expression producing the price in integer atoms; nullable where the operation permits it.
        price: Expr,
        /// The expression producing quantity atoms; the operation determines delta or absolute semantics.
        quantity: Expr,
    },
    /// Record a historical trade ID without publishing an execution.
    TradeHistory {
        /// The expression producing the source order or public trade identifier.
        id: Expr,
    },
    /// Publish a public execution without also reducing the main book.
    Trade {
        /// The expression producing the source order or public trade identifier.
        id: Expr,
        /// The expression producing "buy" or "sell"; Trade uses the resting maker side.
        side: Expr,
        /// The expression producing the price in integer atoms; nullable where the operation permits it.
        price: Expr,
        /// The expression producing quantity atoms; the operation determines delta or absolute semantics.
        quantity: Expr,
        /// The expression producing source time in nanoseconds.
        timestamp: Expr,
    },
    /// Register an instrument and choose its book policy at construction.
    Register {
        /// The expression producing the instrument symbol to register or select.
        symbol: Expr,
        /// The expression producing the instrument's number of fractional price digits.
        price_decimals: Expr,
        /// The expression producing the instrument's number of fractional quantity digits.
        quantity_decimals: Expr,
        /// The expression producing a routing, lookup, or sequence key for this operation.
        key: Expr,
        /// The expression selecting full, no_user_map, no_hidden_quantity, or no_updates at book creation.
        policy: Expr,
    },
    /// Apply nested actions to array items, with optional ordering and deduplication.
    ForEach {
        /// The expression producing the array to iterate.
        items: Expr,
        /// Nested actions executed in declaration order for the current item or book.
        actions: Vec<Action>,
        /// An optional expression used to sort array items before applying actions.
        order_by: Option<Expr>,
        /// An optional item key used to keep one item per key before applying actions.
        unique_by: Option<Expr>,
    },
    /// Select one of two groups of actions from a boolean expression.
    When {
        /// The boolean expression selecting whether these actions execute.
        condition: Expr,
        /// Nested actions executed in declaration order for the current item or book.
        actions: Vec<Action>,
        /// Actions executed when the condition is false.
        otherwise: Vec<Action>,
    },
    /// Assign a protocol variable.
    Let {
        /// The variable name assigned by this operation.
        name: String,
        /// The expression producing this operation's input value.
        value: Expr,
    },
    /// Assign a connection-local lookup-table entry.
    Remember {
        /// The connection-local lookup table updated by this operation.
        table: String,
        /// The expression producing a routing, lookup, or sequence key for this operation.
        key: Expr,
        /// The expression producing this operation's input value.
        value: Expr,
    },
    /// Queue an outgoing control message with expression values substituted.
    Send {
        /// The outgoing message template or failure description.
        message: Expr,
    },
    /// Enable subscriptions and send pending requests on the current connection.
    Subscribe,
    /// Verify contiguous source sequencing or establish a snapshot baseline.
    Sequence {
        /// The expression producing this operation's input value.
        value: Expr,
        /// The expression producing a routing, lookup, or sequence key for this operation.
        key: Expr,
        /// Whether this message establishes a new sequence baseline.
        reset: bool,
    },
    /// Reject the message with a description and require source recovery.
    Fail {
        /// The outgoing message template or failure description.
        message: String,
    },
    /// Select a book, apply a snapshot or update, then finish checks and publication.
    Book {
        /// The expression producing the instrument symbol to register or select.
        symbol: Expr,
        /// Nested actions executed in declaration order for the current item or book.
        actions: Vec<Action>,
        /// The expression indicating replacement of the selected book before applying actions.
        snapshot: Expr,
        /// The expression producing source time in nanoseconds.
        timestamp: Expr,
        /// The retained level depth; zero leaves the book untrimmed.
        depth: usize,
        /// Optional checksum configuration applied after all nested actions complete.
        checksum: Option<Checksum>,
        /// Whether the completed update marks the book synchronized.
        ready: bool,
    },
}
/// Compare a source-provided CRC32 with a configured view of the completed book.
///
/// Pass this object to Book.checksum. The book's checksum policy maintains the
/// required state; Book compares the result after its actions finish. A missing
/// or null expected value skips comparison for that message. This describes the
/// source's checksum convention, rather than choosing a different hash algorithm.
///
/// Args:
///     expected: An expression yielding the checksum sent by the source, or None
///         for messages that do not carry one.
///     view: "levels" aggregates by price; "orders" includes individual orders.
///     depth: The positive maximum number of entries per side, starting at the best
///         price. Defaults to ten. Choose the value required by the source protocol.
///     sides: The order of sides to include. Defaults to ("sell", "buy") when None.
///     fields: Fields emitted for each entry. Defaults to ("price", "quantity")
///         when None. Supported fields are "id", "price", "quantity", and
///         "signed_quantity". Signed quantities are negative for asks.
///     separator: Text placed between emitted values. Defaults to no separator.
///     interleave: If True, alternate entries between sides at each rank. If False,
///         emit all entries from one side before the next.
///     format: "decimal_digits" removes the decimal point and leading zeroes;
///         "decimal" emits decimal text; "ecmascript" uses JavaScript number
///         formatting. Match the source's representation to reproduce its checksum.
///     signed: Whether the expected checksum is a signed 32-bit integer. False
///         compares it as an unsigned CRC32.
///     priority: For individual orders at a price, "fifo" uses queue order and
///         "id" sorts by order ID. Defaults to "fifo".
///     on_failure: Actions run when the checksum differs, such as Send requests to
///         unsubscribe and resubscribe. Defaults to no recovery messages. A mismatch
///         invalidates synchronization; it does not roll back the preceding changes.
///
/// Examples:
/// ```python
/// from lobo.replay.adapters import expressions as le
/// from lobo.replay.adapters import models as lm
///
/// check = lm.Checksum(le.Field("checksum"), depth=10)
/// update = lm.Book(le.Field("symbol"), checksum=check)
/// ```
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(
        name = "Checksum",
        module = "lobo.replay.adapters.models",
        frozen,
        from_py_object
    )
)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Checksum {
    #[serde(skip)]
    /// The prepared checksum policy, populated during adapter construction and excluded from serialization.
    pub prepared: std::sync::Arc<lobo_storage::policies::checksum::Prepared>,
    /// The source-provided checksum expression; null skips comparison.
    pub expected: Expr,
    /// Whether checksum entries represent price levels or individual orders.
    pub view: String,
    /// The positive maximum number of entries included per side, starting at the best price.
    pub depth: usize,
    /// The order in which buy and sell sides participate in the checksum.
    pub sides: Vec<String>,
    /// The ordered entry fields included in the checksum input.
    pub fields: Vec<String>,
    /// The text placed between serialized checksum values.
    pub separator: String,
    /// Whether entries alternate between sides at each rank.
    pub interleave: bool,
    /// The wire-number formatting convention used in the checksum input.
    pub format: String,
    /// Whether the expected checksum is interpreted as a signed 32-bit integer.
    pub signed: bool,
    /// The FIFO or ID ordering of orders at each price for a checksum.
    pub priority: String,
    #[serde(default)]
    /// Recovery actions run after a checksum mismatch invalidates synchronization.
    pub on_failure: Vec<Action>,
}
/// Apply a group of actions when a JSON message satisfies a condition.
///
/// Json evaluates rules in order. All matching rules run, so use mutually exclusive
/// conditions when a packet should follow only one path. Unmatched packets are ignored.
///
/// Args:
///     condition: A boolean expression evaluated against the incoming JSON message.
///     *actions: Actions to execute in order when condition is true.
///
/// Examples:
/// ```python
/// from lobo.replay.adapters import expressions as le
/// from lobo.replay.adapters import models as lm
///
/// rule = lm.Message(le.Field("type").eq("heartbeat"), le.Let("connected", True))
/// ```
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(
        name = "Message",
        module = "lobo.replay.adapters.models",
        frozen,
        from_py_object
    )
)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Message {
    /// The boolean expression selecting whether these actions execute.
    pub condition: Expr,
    /// Nested actions executed in declaration order for the current item or book.
    pub actions: Vec<Action>,
}
/// A byte range within a binary record or its length prefix.
///
/// Use UInt(offset, size, byteorder="big") to read an unsigned integer, or
/// Text(offset, size) to read trimmed UTF-8 text. These constructors provide the
/// field kind; this shared base is not constructed directly from Python.
///
/// Examples:
/// ```python
/// from lobo.replay.adapters import models as lm
///
/// order_id = lm.UInt(offset=0, size=8)
/// symbol = lm.Text(offset=8, size=8)
/// ```
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(
        name = "BinaryField",
        module = "lobo.replay.adapters.models",
        frozen,
        from_py_object,
        subclass
    )
)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BinaryField {
    #[serde(rename = "type")]
    /// The field representation: uint or text.
    pub kind: String,
    /// The zero-based byte offset within the payload, or within the length prefix for the length field.
    pub offset: usize,
    /// The positive number of bytes occupied by this field.
    pub size: usize,
    #[serde(default = "big")]
    /// The unsigned integer byte order: big or little.
    pub byteorder: String,
}
fn big() -> String {
    "big".into()
}
/// Describe one binary record's payload and the actions it triggers.
///
/// Named fields become available through Field("name") while its actions run.
/// The enclosing Binary format supplies the record tag, routing key, and clock.
///
/// Args:
///     size: Exact payload size, excluding the prefix, or None for variable size.
///         Fixed-size declarations retain strict equality checks.
///     fields: A dictionary from field names to UInt or Text declarations. Offsets
///         are relative to the start of the payload, not to the length prefix.
///     actions: Actions to execute in order for this record type. An empty sequence
///         can describe a record that affects no book state.
///     block_length: Optional UInt containing the transmitted root block length.
///         The root ends at block_offset + its value; scalar offsets still start
///         at the payload. This skips unknown fixed-block extension bytes.
///     block_offset: Bytes before the root block; requires block_length.
///     groups: Group declarations in wire order. Without block_length the first
///         group follows the last scalar/header field, or its explicit offset.
///     allow_trailing: With size=None, allow uninterpreted bytes after the declared
///         layout. Defaults to False to catch schema mismatches and bad counts.
///
/// Within actions Field reads the decoded object; nested ForEach changes its
/// current item, while Root reads the message object. Variable layouts execute
/// natively through CustomAdapter.start()/wait(), including hosted adapters.
/// CustomAdapter.run() supports fixed-record bulk mappings only.
///
/// Raises:
///     ValueError: A field extends outside the payload or has an unsupported layout.
///     TypeError: A field is not a UInt or Text, or an action is not an Action.
///
/// Examples:
/// ```python
/// from lobo.replay.adapters import models as lm
///
/// heartbeat = lm.Record(size=11, fields={"clock": lm.UInt(3, 8)}, actions=())
/// ```
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(
        name = "Record",
        module = "lobo.replay.adapters.models",
        frozen,
        from_py_object
    )
)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Record {
    /// The exact record payload size in bytes, excluding the length prefix.
    /// None accepts a variable payload, checked against its fields and groups.
    pub size: Option<usize>,
    /// Named payload fields made available to Field expressions while this record executes.
    pub fields: BTreeMap<String, BinaryField>,
    /// Nested actions executed in declaration order for the current item or book.
    pub actions: Vec<Action>,
    /// Optional transmitted root block length, read relative to the payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_length: Option<BinaryField>,
    /// Bytes preceding the root block (for example, a message header).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub block_offset: usize,
    /// Ordered repeating groups following the root block.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    /// Permit uninterpreted bytes after the declared layout in variable records.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_trailing: bool,
}

/// Decode a repeating group into an array for ForEach and Field expressions.
///
/// Args:
///     name: Field name of the resulting array in its containing record or entry.
///     header_size: Size of the group's dimension header, in bytes.
///     count: UInt relative to the dimension header containing the entry count.
///     block_length: UInt relative to the dimension header containing each entry's
///         fixed block size. Unknown extension bytes in that block are skipped.
///     fields: Named UInt/Text fields, with offsets relative to each entry.
///     offset: Explicit group-header offset relative to its containing payload or
///         entry. None follows the root block or the preceding group.
///     groups: Nested groups following each entry's fixed block, in wire order.
///     max_count: Maximum accepted entries in this group (default 65535).
///     alignment: Align the group-header offset to this power of two, relative to
///         the containing payload or entry (default 1, no padding).
///
/// Field("orders") accesses a top-level group. Inside ForEach it is the entry
/// object: Field("id") reads that entry; Root("version") reads a declared root
/// field. Variable("clock") supplies the enclosing Binary.timestamp value.
/// All entries are decoded and bounds-checked in Rust before any record action
/// runs. A zero count produces an empty array. Truncated headers, undersized
/// blocks, excessive counts and overlapping explicit offsets fail with RuntimeError
/// from CustomAdapter.wait(). Invalid declarations raise ValueError at setup.
/// Nesting is limited to 16 levels, and at most 65535 entries may be decoded
/// across all groups in a single record, including nested entries.
///
/// Examples:
/// ```python
/// from lobo.replay.adapters import models as lm
///
/// orders = lm.Group("orders", header_size=4,
///     count=lm.UInt(2, 2, byteorder="little"),
///     block_length=lm.UInt(0, 2, byteorder="little"),
///     fields={"id": lm.UInt(0, 8, byteorder="little")})
/// ```
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "lobo.replay.adapters.models", frozen, from_py_object)
)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Group {
    /// The array field name.
    pub name: String,
    /// Length of the dimension header.
    pub header_size: usize,
    /// Entry count within the dimension header.
    pub count: BinaryField,
    /// Fixed entry block length within the dimension header.
    pub block_length: BinaryField,
    /// Entry-relative scalar fields.
    pub fields: BTreeMap<String, BinaryField>,
    /// Optional containing-block-relative header location.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Nested groups in wire order.
    #[serde(default)]
    pub groups: Vec<Group>,
    /// Maximum accepted entry count.
    #[serde(default = "group_limit")]
    pub max_count: usize,
    /// Header alignment relative to the containing record or entry.
    #[serde(default = "one")]
    pub alignment: usize,
}
fn group_limit() -> usize {
    65535
}
fn one() -> usize {
    1
}
fn is_zero(value: &usize) -> bool {
    *value == 0
}
fn is_false(value: &bool) -> bool {
    !*value
}
/// Length-prefix framing and the payload layouts selected by numeric record tags.
///
/// The length prefix is excluded from all payload offsets. Unknown tags can be
/// skipped from their length without inventing a record layout.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Binary {
    /// The unsigned length prefix at offset zero; its value excludes the prefix itself.
    pub length: BinaryField,
    /// The unsigned payload field selecting a record layout.
    pub tag: BinaryField,
    /// The payload field containing the routing key registered for an instrument.
    pub key: BinaryField,
    /// The payload field containing the source timestamp in nanoseconds.
    pub timestamp: BinaryField,
    #[serde(deserialize_with = "record_map")]
    /// Payload layouts and actions indexed by numeric record tag.
    pub records: BTreeMap<u64, Record>,
    /// The largest accepted record payload, in bytes, between one and 65535.
    pub max_record_size: usize,
    /// If true the transmitted length includes the prefix itself. Payload offsets
    /// still exclude the prefix, and max_record_size still bounds payload bytes.
    #[serde(default, skip_serializing_if = "is_false")]
    pub length_includes_prefix: bool,
}
fn record_map<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<u64, Record>, D::Error> {
    BTreeMap::<String, Record>::deserialize(deserializer)?
        .into_iter()
        .map(|(tag, record)| {
            tag.parse::<u64>()
                .map(|tag| (tag, record))
                .map_err(serde::de::Error::custom)
        })
        .collect()
}
/// Choose binary record decoding or JSON message rules for one protocol.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "format", rename_all = "snake_case")]
pub enum Format {
    /// Decode length-prefixed records using fixed byte ranges.
    Binary(Binary),
    /// Decode JSON packets using ordered conditional rules.
    Json {
        /// Rules evaluated in declaration order; every matching rule runs.
        messages: Vec<Message>,
    },
}
/// Fetch one HTTP JSON response before opening a live source's connections.
///
/// A bootstrap request is a startup discovery step. For example, an exchange's
/// instrument endpoint can provide symbols and decimal precision needed to register
/// books before subscription. It is not automatically an order-book snapshot:
/// actions explicitly decide what to do with the response. A feed that sends its
/// snapshot over WebSocket should handle that snapshot with a Message and Book.
///
/// Args:
///     name: A unique name for this request within the protocol. Source.packets and
///         Source.json_lines use this name to associate supplied discovery responses
///         with their actions when replaying recorded messages.
///     url: The HTTP or HTTPS URL to GET for a live WebSocket source. The response
///         must be JSON and is available through Field and Root expressions.
///     *actions: Actions applied to the response in order. ForEach and Register can
///         turn an array of instruments into the adapter's ticker directory.
///
/// Examples:
/// ```python
/// from lobo.replay.adapters import expressions as le
/// from lobo.replay.adapters import models as lm
///
/// directory = lm.Bootstrap(
///     "instruments", "https://feed.example/instruments",
///     le.ForEach(le.Field("symbols"), lm.Register(symbol=le.Field("name"),
///             price_decimals=le.Field("price_dp"), quantity_decimals=0)),
/// )
/// ```
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(
        name = "Bootstrap",
        module = "lobo.replay.adapters.models",
        frozen,
        from_py_object
    )
)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Bootstrap {
    /// The unique request name used to match recorded discovery responses.
    pub name: String,
    /// The HTTP GET URL whose JSON response supplies startup discovery data.
    pub url: String,
    /// Actions applied to the discovery response before opening live connections.
    pub actions: Vec<Action>,
}
/// Describe message decoding and the lifecycle of a custom adapter.
///
/// Protocol is the complete declaration passed to CustomAdapter. It combines a
/// wire format with optional discovery requests and connection actions. Its
/// constructor validates the declaration; CustomAdapter prepares it for execution.
/// No connection is opened merely by constructing a Protocol.
///
/// Args:
///     format: Binary record layouts or Json message rules describing incoming data.
///     connect: Actions run each time a source connection opens, including reconnects.
///         Use Send for a handshake and Subscribe if subscription can start immediately.
///         Defaults to no actions. Connection lookup tables and sequence state are
///         cleared before these actions run.
///     subscriptions: Actions run once for each requested, available instrument on a
///         ready connection. Variable("symbol") is that instrument. These actions
///         usually contain Send requests. Subscribe marks the connection ready and
///         sends pending subscriptions; call it after any required handshake reply.
///         Defaults to no actions.
///     bootstrap: Bootstrap requests processed in declaration order before a live
///         connection opens. These HTTP JSON responses commonly discover symbols and
///         precision. They are separate from book snapshots carried by the message
///         stream. Recorded sources can supply responses under each request's name.
///         Defaults to no requests.
///     keepalive: Actions run on each connected WebSocket at the transport's periodic
///         heartbeat, currently every fifteen seconds. Use Send if the source needs
///         an application-level ping. Defaults to no actions.
///     simulation_note: Optional text describing source-specific simulation limits,
///         exposed with the adapter's simulation information. Defaults to None.
///     reconcile_window_ns: Maximum source-time interval for associating separate
///         public trades with absolute L3 order updates in an alternate simulation.
///         Defaults to 250,000,000 nanoseconds (250 milliseconds). It does not delay
///         the main book or apply to feeds with explicit order execution messages.
///     reconcile_capacity: Maximum pending reconciliation entries retained for a
///         simulation. Defaults to 4096 and must be positive. Exceeding the bound
///         interrupts the simulation rather than retaining unbounded pending state.
///     symbols_per_connection: Maximum subscribed instruments assigned to one
///         connection. Zero, the default, puts all requested symbols on one connection.
///         A positive value opens additional connections as symbols are requested;
///         it does not limit the adapter's ticker directory or book scope.
///
/// Raises:
///     ValueError: An expression, action, checksum configuration, framing layout, or
///         reconciliation capacity is invalid.
///     TypeError: An argument is not the required declaration or sequence of declarations.
///
/// Examples:
/// ```python
/// from lobo.replay.adapters import Protocol
/// from lobo.replay.adapters import expressions as le
/// from lobo.replay.adapters import models as lm
///
/// protocol = Protocol(
///     lm.Json(lm.Message(le.Field("type").eq("level"),
///         lm.Book(le.Field("symbol"), lm.Level(side=le.Field("side"),
///             price=le.Field("price").decimal(2), quantity=le.Field("size"))))),
///     connect=(lm.Subscribe(),),
///     subscriptions=(lm.Send({"method": "subscribe", "symbol": le.Variable("symbol")}),),
/// )
/// ```
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(
        name = "Protocol",
        module = "lobo.replay.adapters",
        frozen,
        from_py_object
    )
)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Definition {
    #[serde(default)]
    /// Actions run by the transport heartbeat for each connected source.
    pub keepalive: Vec<Action>,
    #[serde(default)]
    /// Optional source-specific simulation guidance shown to consumers.
    pub simulation_note: Option<String>,
    /// Binary record layouts or JSON rules describing incoming messages.
    pub format: Format,
    /// Actions run whenever a source connection opens or reconnects.
    pub connect: Vec<Action>,
    /// Actions run per requested instrument after Subscribe enables the connection.
    pub subscriptions: Vec<Action>,
    /// HTTP JSON discovery requests processed in order before opening live connections.
    pub bootstrap: Vec<Bootstrap>,
    /// The simulation trade/order association window, in source-time nanoseconds.
    pub reconcile_window_ns: u64,
    /// The positive bound on pending simulation reconciliation entries.
    pub reconcile_capacity: usize,
    /// Maximum symbols assigned per connection; zero uses a single connection.
    pub symbols_per_connection: usize,
}
impl BinaryField {
    /// Check that the field fits within max bytes and has a supported representation.
    ///
    /// # Errors
    /// Returns an error for an empty or out-of-bounds range, unsupported kind, integer
    /// width above eight bytes, or invalid byte order.
    pub fn validate(&self, max: usize) -> Result<(), String> {
        if self.size == 0 || self.offset.checked_add(self.size).is_none_or(|n| n > max) {
            return Err("Binary field is outside its record".into());
        }
        if self.kind != "uint" && self.kind != "text" {
            return Err("Binary field type must be uint or text".into());
        }
        if self.kind == "uint" && self.size > 8 {
            return Err("Integer binary fields support at most 8 bytes".into());
        }
        if self.byteorder != "big" && self.byteorder != "little" {
            return Err("byteorder must be big or little".into());
        }
        Ok(())
    }
    /// Read an unsigned field from a payload or length prefix.
    ///
    /// Call validate during setup before reading records. bytes begins at the same
    /// origin used for offset.
    ///
    /// # Errors
    /// Returns an error if the byte slice ends before the field.
    pub fn number(&self, bytes: &[u8]) -> Result<u64, String> {
        let s = bytes
            .get(self.offset..self.offset + self.size)
            .ok_or("Truncated binary field")?;
        Ok(if self.byteorder == "little" {
            s.iter().rev().fold(0, |v, &b| (v << 8) | u64::from(b))
        } else {
            s.iter().fold(0, |v, &b| (v << 8) | u64::from(b))
        })
    }
    /// Read a numeric value or trimmed UTF-8 string from a record payload.
    ///
    /// # Errors
    /// Returns an error for truncated input or invalid UTF-8 text.
    pub fn value(&self, bytes: &[u8]) -> Result<Value, String> {
        if self.kind == "uint" {
            Ok(self.number(bytes)?.into())
        } else {
            Ok(std::str::from_utf8(
                bytes
                    .get(self.offset..self.offset + self.size)
                    .ok_or("Truncated text field")?,
            )
            .map_err(|e| e.to_string())?
            .trim()
            .into())
        }
    }
}
impl Definition {
    /// Deserialize and validate an exported protocol definition.
    ///
    /// Use this when loading saved JSON. Python construction supplies Definition
    /// directly and does not serialize its arguments through this function.
    ///
    /// # Errors
    /// Returns an error for malformed JSON or an invalid declaration.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let definition: Self = serde_json::from_str(spec).map_err(|e| e.to_string())?;
        definition.validate()?;
        Ok(definition)
    }
    /// Check expression structure, layouts, and fixed protocol options during setup.
    ///
    /// This does not consume source messages or change a book. Call it after building
    /// a Rust definition and before preparing the adapter.
    ///
    /// # Errors
    /// Returns the first invalid field range, expression operand count, checksum
    /// configuration, or capacity encountered.
    ///
    /// # Examples
    ///
    /// ```
    /// use lobo_replay::custom::definition::schema::{Definition, Format};
    /// let protocol = Definition {
    ///     format: Format::Json { messages: vec![] },
    ///     connect: vec![], subscriptions: vec![], bootstrap: vec![], keepalive: vec![],
    ///     simulation_note: None, reconcile_window_ns: 250_000_000,
    ///     reconcile_capacity: 4096, symbols_per_connection: 0,
    /// };
    /// assert!(protocol.validate().is_ok());
    /// ```

    pub fn validate(&self) -> Result<(), String> {
        let definition = self;
        if definition.reconcile_capacity == 0 {
            return Err("reconcile_capacity must be positive".into());
        }
        match &definition.format {
            Format::Binary(binary) => {
                if binary.max_record_size == 0 || binary.max_record_size > 65535 {
                    return Err("max_record_size must be between 1 and 65535".into());
                }
                binary.length.validate(8)?;
                if binary.length.offset != 0 || binary.length.kind != "uint" {
                    return Err("Record length must be an integer prefix at offset zero".into());
                }
                for field in [&binary.tag, &binary.key, &binary.timestamp] {
                    super::binary_layout::uint(field, binary.max_record_size)?;
                }
                for record in binary.records.values() {
                    record.validate_layout(binary.max_record_size, binary.minimum_header())?;
                    validate_actions(&record.actions)?;
                }
            }
            Format::Json { messages } => {
                for message in messages {
                    message.condition.validate()?;
                    validate_actions(&message.actions)?;
                }
            }
        }
        validate_actions(&definition.keepalive)?;
        validate_actions(&definition.connect)?;
        validate_actions(&definition.subscriptions)?;
        for bootstrap in &definition.bootstrap {
            validate_actions(&bootstrap.actions)?;
        }
        Ok(())
    }
}
fn validate_actions(actions: &[Action]) -> Result<(), String> {
    use Operation::*;
    for a in actions {
        let expressions: Vec<&Expr> = match &a.operation {
            Add {
                id,
                side,
                price,
                quantity,
                timestamp,
            }
            | Upsert {
                id,
                side,
                price,
                quantity,
                timestamp,
            } => vec![id, side, price, quantity, timestamp],
            Execute {
                id,
                price,
                quantity,
                timestamp,
            } => vec![id, price, quantity, timestamp],
            Replace {
                id,
                new_id,
                price,
                quantity,
                timestamp,
            } => vec![id, new_id, price, quantity, timestamp],
            Cancel {
                id,
                quantity,
                timestamp,
            }
            | Modify {
                id,
                quantity,
                timestamp,
            } => vec![id, quantity, timestamp],
            Remove { id, timestamp } => vec![id, timestamp],
            Level {
                side,
                price,
                quantity,
            } => vec![side, price, quantity],
            TradeHistory { id } => vec![id],
            Trade {
                id,
                side,
                price,
                quantity,
                timestamp,
            } => vec![id, side, price, quantity, timestamp],
            Register {
                symbol,
                price_decimals,
                quantity_decimals,
                key,
                policy,
            } => {
                vec![symbol, price_decimals, quantity_decimals, key, policy]
            }
            Directory {
                items,
                symbol,
                actions,
                on_remove,
            } => {
                validate_actions(actions)?;
                validate_actions(on_remove)?;
                vec![items, symbol]
            }
            ForEach {
                items,
                actions,
                order_by,
                unique_by,
            } => {
                validate_actions(actions)?;
                let mut e = vec![items];
                e.extend(order_by);
                e.extend(unique_by);
                e
            }
            When {
                condition,
                actions,
                otherwise,
            } => {
                validate_actions(actions)?;
                validate_actions(otherwise)?;
                vec![condition]
            }
            Let { value, .. } | RestoreOrder { value } => vec![value],
            OrderCommand { value, timestamp } => vec![value, timestamp],
            Sequence { value, key, .. } => vec![value, key],
            Remember { key, value, .. } => vec![key, value],
            Book {
                symbol,
                actions,
                snapshot,
                timestamp,
                checksum,
                ..
            } => {
                validate_actions(actions)?;
                let mut expressions = vec![symbol, snapshot, timestamp];
                if let Some(c) = checksum {
                    validate_actions(&c.on_failure)?;
                    if c.depth == 0
                        || !["levels", "orders"].contains(&c.view.as_str())
                        || !["fifo", "id"].contains(&c.priority.as_str())
                        || !["decimal_digits", "decimal", "ecmascript"].contains(&c.format.as_str())
                    {
                        return Err("Invalid checksum view, depth, priority or format".into());
                    }
                    for side in &c.sides {
                        if !["buy", "sell"].contains(&side.as_str()) {
                            return Err("Invalid checksum side".into());
                        }
                    }
                    for field in &c.fields {
                        if !["id", "price", "quantity", "signed_quantity"].contains(&field.as_str())
                        {
                            return Err("Invalid checksum field".into());
                        }
                    }
                    expressions.push(&c.expected);
                }
                expressions
            }
            Send { message } => vec![message],
            Subscribe | DirectoryComplete | Fail { .. } => vec![],
        };
        for expression in expressions {
            expression.validate()?;
        }
    }
    Ok(())
}

pub(crate) fn visit_operations(
    def: &mut Definition,
    visit: &mut impl FnMut(&mut Action) -> Result<(), String>,
) -> Result<(), String> {
    fn walk(
        actions: &mut [Action],
        visit: &mut impl FnMut(&mut Action) -> Result<(), String>,
    ) -> Result<(), String> {
        for action in actions {
            visit(action)?;
            match &mut action.operation {
                Operation::Directory {
                    actions, on_remove, ..
                } => {
                    walk(actions, visit)?;
                    walk(on_remove, visit)?;
                }
                Operation::ForEach { actions, .. } => walk(actions, visit)?,
                Operation::When {
                    actions, otherwise, ..
                } => {
                    walk(actions, visit)?;
                    walk(otherwise, visit)?;
                }
                Operation::Book {
                    actions, checksum, ..
                } => {
                    walk(actions, visit)?;
                    if let Some(checksum) = checksum {
                        walk(&mut checksum.on_failure, visit)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    match &mut def.format {
        Format::Json { messages } => {
            for message in messages {
                walk(&mut message.actions, visit)?;
            }
        }
        Format::Binary(binary) => {
            for record in binary.records.values_mut() {
                walk(&mut record.actions, visit)?;
            }
        }
    }
    walk(&mut def.connect, visit)?;
    walk(&mut def.subscriptions, visit)?;
    walk(&mut def.keepalive, visit)?;
    for request in &mut def.bootstrap {
        walk(&mut request.actions, visit)?;
    }
    Ok(())
}
