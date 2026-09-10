//! Python constructors for the shared Rust book and lifecycle operations.
use super::super::schema::{Action, Checksum, Operation};
use super::expressions::{clock, literal, operand, optional};
use super::{error, export, specification_value};
use crate::custom::python::Iterable;
use pyo3::{
    prelude::*,
    types::{PyDict, PyTuple},
};
use serde_json::Value;

pub(super) fn actions(value: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<Action>> {
    value
        .map(|v| {
            v.try_iter()?
                .map(|a| Ok(a?.extract::<PyRef<'_, Action>>()?.clone()))
                .collect()
        })
        .transpose()
        .map(Option::unwrap_or_default)
}
#[pymethods]
impl Action {
    /// Restore an action using an operation name and exported argument values.
    ///
    /// Prefer the named constructors such as Add, Execute, and Book for a new
    /// adapter. This low-level constructor accepts the same fields as an exported
    /// operation; expression arguments must be declaration objects or exported
    /// expression dictionaries, rather than unwrapped numeric values.
    ///
    /// Args:
    ///     kind: The operation name, such as "add" or "let".
    ///     **fields: The fields of that operation in its exported representation.
    ///
    /// Raises:
    ///     ValueError: The operation name or its field structure is invalid.
    #[new]
    #[pyo3(signature=(kind,**fields))]
    fn python_new(kind: &str, fields: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
        let mut fields = fields
            .map(|v| {
                v.iter()
                    .map(|(key, value)| {
                        Ok((key.extract::<String>()?, specification_value(&value)?))
                    })
                    .collect::<PyResult<serde_json::Map<_, _>>>()
            })
            .transpose()?
            .unwrap_or_default();
        fields.insert("action".into(), kind.into());
        serde_json::from_value(Value::Object(fields)).map_err(error)
    }
    /// Return a detached dictionary describing this operation and its arguments.
    ///
    /// Returns:
    ///     A serializable dictionary. Editing it does not mutate this action.
    #[getter]
    fn data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        export(py, self)
    }
}
macro_rules! action_class {
    ($(#[$doc:meta])* $name:ident, $module:literal, ($($arg:ident : $ty:ty),*), $signature:tt, $body:expr) => {
        $(#[$doc])*
        #[pyclass(extends=Action, frozen, module=$module)]
        pub struct $name;
        #[pymethods]
        impl $name {
            $(#[$doc])*
            #[new]
            #[pyo3(signature=$signature)]
            fn new($($arg:$ty),*) -> PyResult<PyClassInitializer<Self>> { Ok(PyClassInitializer::from(Action{operation:$body}).add_subclass(Self)) }
        }
    };
}

action_class! {
    /// Insert a visible resting order without matching it against the other side.
    ///
    /// Use this for feeds that publish individual order additions. Register the
    /// instrument first and select it with Book or the current symbol variable.
    ///
    /// Args:
    ///     id: An expression or constant yielding the order's unsigned integer ID
    ///         or UUID text. Later changes must use the same ID.
    ///     side: "buy" or "sell", or an expression producing that side.
    ///     price: The positive limit price in integer price atoms.
    ///     quantity: The order's visible quantity in integer quantity atoms.
    ///     timestamp: Source time in nanoseconds. None or omission uses Variable("clock").
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     add = lm.Add(id=le.Field("id"), side="buy", price=le.Field("price"), quantity=le.Field("size"))
    ///     ```
    Add, "lobo.replay.adapters.models", (id:&Bound<'_,PyAny>, side:&Bound<'_,PyAny>, price:&Bound<'_,PyAny>, quantity:&Bound<'_,PyAny>, timestamp:Option<&Bound<'_,PyAny>>), (*,id,side,price,quantity,timestamp=None),
    Operation::Add{id:operand(id)?,side:operand(side)?,price:operand(price)?,quantity:operand(quantity)?,timestamp:clock(timestamp)?}
}
action_class! {
    /// Consume quantity from a known resting order and publish an execution.
    ///
    /// Use this when the source explicitly identifies a fill. It supplies traded
    /// volume and execution prices to the existing bar and simulation machinery.
    ///
    /// Args:
    ///     id: The ID of the resting order that traded.
    ///     quantity: The quantity executed by this message, not the quantity left.
    ///     price: The execution price in integer atoms. None uses the resting
    ///         order's price; provide an expression when the feed reports another price.
    ///     timestamp: Source time in nanoseconds. None or omission uses Variable("clock").
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     execute = lm.Execute(id=le.Field("id"), quantity=le.Field("executed_size"))
    ///     ```
    Execute, "lobo.replay.adapters.models", (id:&Bound<'_,PyAny>, quantity:&Bound<'_,PyAny>, price:Option<&Bound<'_,PyAny>>, timestamp:Option<&Bound<'_,PyAny>>), (*,id,quantity,price=None,timestamp=None),
    Operation::Execute{id:operand(id)?,quantity:operand(quantity)?,price:optional(price)?,timestamp:clock(timestamp)?}
}
action_class! {
    /// Subtract a cancellation quantity from a resting order without a trade event.
    ///
    /// Args:
    ///     id: The ID of the order being reduced.
    ///     quantity: The amount cancelled by this message, not its new total.
    ///     timestamp: Source time in nanoseconds. None or omission uses Variable("clock").
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     cancel = lm.Cancel(id=le.Field("id"), quantity=le.Field("cancelled_size"))
    ///     ```
    Cancel, "lobo.replay.adapters.models", (id:&Bound<'_,PyAny>, quantity:&Bound<'_,PyAny>, timestamp:Option<&Bound<'_,PyAny>>), (*,id,quantity,timestamp=None),
    Operation::Cancel{id:operand(id)?,quantity:operand(quantity)?,timestamp:clock(timestamp)?}
}
action_class! {
    /// Delete a resting order in full without reporting a trade.
    ///
    /// Args:
    ///     id: The ID of the order to remove.
    ///     timestamp: Source time in nanoseconds. None or omission uses Variable("clock").
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     remove = lm.Remove(id=le.Field("order_id"))
    ///     ```
    Remove, "lobo.replay.adapters.models", (id:&Bound<'_,PyAny>, timestamp:Option<&Bound<'_,PyAny>>), (*,id,timestamp=None),
    Operation::Remove{id:operand(id)?,timestamp:clock(timestamp)?}
}
action_class! {
    /// Set a known order's absolute visible quantity, retaining its price and side.
    ///
    /// Args:
    ///     id: The ID of an existing resting order.
    ///     quantity: The new visible total in integer atoms, not a delta. A
    ///         decrease alone is not proof of a trade; use Execute for known fills.
    ///     timestamp: Source time in nanoseconds. None or omission uses Variable("clock").
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     modify = lm.Modify(id=le.Field("id"), quantity=le.Field("remaining_size"))
    ///     ```
    Modify, "lobo.replay.adapters.models", (id:&Bound<'_,PyAny>, quantity:&Bound<'_,PyAny>, timestamp:Option<&Bound<'_,PyAny>>), (*,id,quantity,timestamp=None),
    Operation::Modify{id:operand(id)?,quantity:operand(quantity)?,timestamp:clock(timestamp)?}
}
action_class! {
    /// Replace an order with a new identity, limit price, and visible quantity.
    ///
    /// Args:
    ///     id: The ID of the order being replaced.
    ///     new_id: The ID assigned to the replacement order.
    ///     price: The replacement's positive limit price in integer atoms.
    ///     quantity: The replacement's complete visible quantity in integer atoms.
    ///     timestamp: Source time in nanoseconds. None or omission uses Variable("clock").
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     replace = lm.Replace(id=le.Field("old_id"), new_id=le.Field("new_id"),
    ///                       price=le.Field("price"), quantity=le.Field("size"))
    ///     ```
    Replace, "lobo.replay.adapters.models", (id:&Bound<'_,PyAny>, new_id:&Bound<'_,PyAny>, price:&Bound<'_,PyAny>, quantity:&Bound<'_,PyAny>, timestamp:Option<&Bound<'_,PyAny>>), (*,id,new_id,price,quantity,timestamp=None),
    Operation::Replace{id:operand(id)?,new_id:operand(new_id)?,price:operand(price)?,quantity:operand(quantity)?,timestamp:clock(timestamp)?}
}
action_class! {
    /// Apply an absolute order update, creating an order if its ID is new.
    ///
    /// A null price removes the ID. Live L3 feeds often publish this form of update
    /// and report trades separately. During an alternate simulation, the existing
    /// reconciliation policy coordinates these updates with Trade actions.
    ///
    /// Args:
    ///     id: The source's persistent order ID.
    ///     side: "buy" or "sell", or an expression producing that value.
    ///     price: The limit price in integer atoms, or an expression producing None
    ///         for removal. Translate a feed's deletion sentinel with Choose.
    ///     quantity: The complete visible quantity in integer atoms.
    ///     timestamp: Source time in nanoseconds. None or omission uses Variable("clock").
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     update = lm.Upsert(id=le.Field("id"), side="buy", quantity=le.Field("size"),
    ///                     price=le.Choose(le.Field("price").eq(0), None, le.Field("price")))
    ///     ```
    Upsert, "lobo.replay.adapters.models", (id:&Bound<'_,PyAny>, side:&Bound<'_,PyAny>, price:&Bound<'_,PyAny>, quantity:&Bound<'_,PyAny>, timestamp:Option<&Bound<'_,PyAny>>), (*,id,side,price,quantity,timestamp=None),
    Operation::Upsert{id:operand(id)?,side:operand(side)?,price:operand(price)?,quantity:operand(quantity)?,timestamp:clock(timestamp)?}
}
action_class! {
    /// Apply a message already shaped like the library's order API command.
    ///
    /// Args:
    ///     value: An expression yielding the command object, including its op and
    ///         the fields required by add, execute, cancel, remove, modify, fill,
    ///         or simulate. The selected Book supplies the target instrument.
    ///     timestamp: Source time in nanoseconds. None or omission uses Variable("clock").
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     command = lm.OrderCommand(le.Field("command"), timestamp=le.Field("timestamp_ns"))
    ///     ```
    OrderCommand, "lobo.replay.adapters.models", (value:&Bound<'_,PyAny>, timestamp:Option<&Bound<'_,PyAny>>), (value,*,timestamp=None),
    Operation::OrderCommand{value:operand(value)?,timestamp:clock(timestamp)?}
}
action_class! {
    /// Restore one order from the server's resting-order snapshot representation.
    ///
    /// Apply these actions in the snapshot's FIFO order. No timestamp sorting is
    /// added, so replenished iceberg orders retain their correct queue positions.
    ///
    /// Args:
    ///     value: An expression yielding an object with id, trader, side, price,
    ///         quantity, created_at_ns, hidden_quantity, and peak_quantity. These
    ///         are the fields of the server's RestingOrderState schema.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     snapshot_rows = le.ForEach(le.Field("orders"), lm.RestoreOrder(le.Field()))
    ///     ```
    RestoreOrder, "lobo.replay.adapters.models", (value:&Bound<'_,PyAny>), (value), Operation::RestoreOrder{value:operand(value)?}
}
action_class! {
    /// Set the total displayed quantity at one L2 price level.
    ///
    /// Args:
    ///     side: "buy" or "sell", or an expression producing that side.
    ///     price: The positive level price in integer atoms. decimal() expressions
    ///         also preserve the source's decimal width for checksum calculation.
    ///     quantity: The new absolute level quantity in integer atoms. Zero removes
    ///         the level. This action does not invent executions from reductions.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     level = lm.Level(side="buy", price=le.Field("price").decimal(2),
    ///                   quantity=le.Field("quantity").decimal(8))
    ///     ```
    Level, "lobo.replay.adapters.models", (side:&Bound<'_,PyAny>, price:&Bound<'_,PyAny>, quantity:&Bound<'_,PyAny>), (*,side,price,quantity),
    Operation::Level{side:operand(side)?,price:operand(price)?,quantity:operand(quantity)?}
}
action_class! {
    /// Report a public trade at its actual execution price.
    ///
    /// This feeds traded-volume bars and an active simulation. It does not reduce
    /// the main book again when a separate order or level update already did so.
    /// Trades are ignored until the instrument has a synchronized snapshot.
    ///
    /// Args:
    ///     price: The execution price in integer price atoms.
    ///     quantity: The executed quantity in integer quantity atoms.
    ///     maker_side: The resting side that was consumed: "buy" for a resting bid
    ///         or "sell" for a resting ask. This is opposite to the taker's side.
    ///     timestamp: The trade's source time in nanoseconds.
    ///     id: An optional monotonic unsigned trade ID. When supplied, IDs at or
    ///         below the last accepted ID are ignored, preventing duplicate trades.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     trade = lm.Trade(price=le.Field("price"), quantity=le.Field("size"), maker_side="sell",
    ///                   timestamp=le.Field("time").timestamp("ms"), id=le.Field("trade_id"))
    ///     ```
    Trade, "lobo.replay.adapters.models", (price:&Bound<'_,PyAny>, quantity:&Bound<'_,PyAny>, maker_side:&Bound<'_,PyAny>, timestamp:&Bound<'_,PyAny>, id:Option<&Bound<'_,PyAny>>), (*,price,quantity,maker_side,timestamp,id=None),
    Operation::Trade{price:operand(price)?,quantity:operand(quantity)?,side:operand(maker_side)?,timestamp:operand(timestamp)?,id:optional(id)?}
}
action_class! {
    /// Replace the list of available instruments using a complete directory response.
    ///
    /// Instruments omitted from the replacement are removed. Use ForEach instead
    /// for a partial list that must not remove other instruments.
    ///
    /// Args:
    ///     items: An expression yielding the array of directory entries.
    ///     *actions: Actions to run for each entry, normally Register. Field reads
    ///         from that entry; Root still reads the complete directory response.
    ///     symbol: An expression identifying the symbol of the current entry.
    ///     on_remove: Actions for each omitted instrument. Variable("symbol") names
    ///         that instrument, allowing remembered protocol state to be cleared.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     directory = lm.Directory(le.Field("instruments"),
    ///         lm.Register(symbol=le.Field("symbol"), price_decimals=2, quantity_decimals=0),
    ///         symbol=le.Field("symbol"))
    ///     ```
    Directory, "lobo.replay.adapters.models", (items:&Bound<'_,PyAny>, actions:&Bound<'_,PyTuple>, symbol:&Bound<'_,PyAny>, on_remove:Option<Iterable<Action>>), (items,*actions: "Action",symbol,on_remove=None),
    Operation::Directory{items:operand(items)?,symbol:operand(symbol)?,actions:self::actions(Some(actions.as_any()))?,on_remove:self::Iterable::or_empty(on_remove)}
}
action_class! {
    /// Mark an explicitly delimited instrument directory as complete.
    ///
    /// Place this on the source's directory-end message when instruments arrive
    /// individually, for example as Register actions in successive binary records.
    /// The directory scanner can then stop collecting instrument metadata.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     end_directory = lm.Message(le.Field("event").eq("directory_end"), lm.DirectoryComplete())
    ///     ```
    DirectoryComplete, "lobo.replay.adapters.models", (), (), Operation::DirectoryComplete
}
action_class! {
    /// Advance the known trade ID without publishing a historical execution.
    ///
    /// Args:
    ///     id: An expression or constant yielding an unsigned monotonic trade ID.
    ///         The largest observed value is retained. Subsequent Trade actions
    ///         ignore IDs at or below this value.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     history = le.ForEach(le.Field("recent_trades"), lm.TradeHistory(le.Field("id")))
    ///     ```
    TradeHistory, "lobo.replay.adapters.models", (id:&Bound<'_,PyAny>), (id), Operation::TradeHistory{id:operand(id)?}
}
action_class! {
    /// Register an instrument and the units and policies used to construct its book.
    ///
    /// Args:
    ///     symbol: The instrument's ticker or pair name. Symbols are normalized by
    ///         the context and also supply ticker autocomplete in the app.
    ///     price_decimals: Decimal places represented by integer price atoms.
    ///         A value of 2 means 12345 atoms represent 123.45.
    ///     quantity_decimals: Decimal places represented by integer quantity atoms.
    ///         Use 0 for whole shares and an appropriate scale for fractional units.
    ///     key: An optional unsigned binary routing key, such as a stock locator.
    ///         Later records carrying that key can select this book directly.
    ///     policy: "full", "no_user_map", "no_hidden_quantity", or "no_updates".
    ///         These choose user-map and hidden-quantity bookkeeping at book
    ///         construction. "no_updates" disables both and is the default.
    ///         Policies without hidden updates treat submitted reserves as absent.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     register = lm.Register(symbol=le.Field("symbol"), price_decimals=2,
    ///                         quantity_decimals=0, key=le.Variable("key"))
    ///     ```
    Register, "lobo.replay.adapters.models", (symbol:&Bound<'_,PyAny>, price_decimals:&Bound<'_,PyAny>, quantity_decimals:&Bound<'_,PyAny>, key:Option<&Bound<'_,PyAny>>, policy:Option<&Bound<'_,PyAny>>), (*,symbol,price_decimals,quantity_decimals,key=None,policy=None),
    Operation::Register{symbol:operand(symbol)?,price_decimals:operand(price_decimals)?,quantity_decimals:operand(quantity_decimals)?,key:optional(key)?,policy:policy.map(operand).transpose()?.unwrap_or_else(||literal("no_updates"))}
}
action_class! {
    /// Run a sequence of actions for every element of an array.
    ///
    /// Args:
    ///     items: An expression yielding the array to traverse. Field reads from
    ///         each item inside the loop; Root continues to read the full message.
    ///     *actions: Actions to run in order for each item.
    ///     order_by: An optional expression evaluated per item for an ascending
    ///         sort before applying actions. Omit it when source order is already FIFO.
    ///     unique_by: An optional per-item key for removing duplicate entries.
    ///         Use it only when repeated snapshot rows represent the same item.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     bids = le.ForEach(le.Field("bids"),
    ///         lm.Level(side="buy", price=le.Field(0).decimal(2), quantity=le.Field(1).decimal(8)))
    ///     ```
    ForEach, "lobo.replay.adapters.expressions", (items:&Bound<'_,PyAny>, actions:&Bound<'_,PyTuple>, order_by:Option<&Bound<'_,PyAny>>, unique_by:Option<&Bound<'_,PyAny>>), (items,*actions: "Action",order_by=None,unique_by=None),
    Operation::ForEach{items:operand(items)?,actions:self::actions(Some(actions.as_any()))?,order_by:order_by.map(operand).transpose()?,unique_by:unique_by.map(operand).transpose()?}
}
action_class! {
    /// Choose which group of actions to run for the current message or loop item.
    ///
    /// Args:
    ///     condition: An expression or constant; true selects actions.
    ///     *actions: Actions to run, in order, when the condition is true.
    ///     otherwise: Actions to run when the condition is not true. Defaults to
    ///         an empty group. Only the selected branch executes.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     update = le.When(le.Field("deleted").eq(True), lm.Remove(id=le.Field("id")),
    ///                   otherwise=(lm.Modify(id=le.Field("id"), quantity=le.Field("size")),))
    ///     ```
    When, "lobo.replay.adapters.expressions", (condition:&Bound<'_,PyAny>, actions:&Bound<'_,PyTuple>, otherwise:Option<Iterable<Action>>), (condition,*actions: "Action",otherwise=None),
    Operation::When{condition:operand(condition)?,actions:self::actions(Some(actions.as_any()))?,otherwise:self::Iterable::or_empty(otherwise)}
}
action_class! {
    /// Assign an expression result to a named protocol variable.
    ///
    /// Args:
    ///     name: The name used later by Variable(name). Assignment replaces the
    ///         previous value. Variables belong to the adapter; use Remember for
    ///         a lookup table isolated to each connection and reset on reconnect.
    ///     value: The expression or constant to store when this action executes.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     save_symbol = le.Let("wire_symbol", le.Field("symbol"))
    ///     ```
    Let, "lobo.replay.adapters.expressions", (name:String, value:&Bound<'_,PyAny>), (name,value), Operation::Let{name,value:operand(value)?}
}
action_class! {
    /// Store a value under a key in the current connection's named table.
    ///
    /// Args:
    ///     table: The table name used later by expression.lookup(table).
    ///     key: An expression yielding the lookup key, often a channel ID.
    ///     value: The expression or constant to remember. Remembered values survive
    ///         later packets; the connection's tables are cleared on reconnect.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     remember = le.Remember("channels", le.Field("channel_id"), {"symbol": le.Field("symbol")})
    ///     ```
    Remember, "lobo.replay.adapters.expressions", (table:String, key:&Bound<'_,PyAny>, value:&Bound<'_,PyAny>), (table,key,value),
    Operation::Remember{table,key:operand(key)?,value:operand(value)?}
}
action_class! {
    /// Queue a JSON control message for the current source connection.
    ///
    /// Args:
    ///     message: A dictionary, list, constant, or expression describing the
    ///         outgoing JSON value. Embedded expressions are evaluated when the
    ///         action executes. This is commonly a subscription or ping request.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     subscribe = lm.Send({"method": "subscribe", "symbol": le.Variable("symbol")})
    ///     ```
    Send, "lobo.replay.adapters.models", (message:&Bound<'_,PyAny>), (message), Operation::Send{message:operand(message)?}
}
action_class! {
    /// Allow subscriptions and send them for selected instruments that are known.
    ///
    /// This runs Protocol.subscriptions with Variable("symbol") and precision
    /// variables set for each requested instrument. Previously sent subscriptions
    /// are not repeated until reconnect. Place it in connect when the source is
    /// immediately ready, or after an acknowledgement when negotiation is required.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import Protocol
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     protocol = Protocol(lm.Json(), connect=(lm.Subscribe(),),
    ///         subscriptions=(lm.Send({"subscribe": le.Variable("symbol")}),))
    ///     ```
    Subscribe, "lobo.replay.adapters.models", (), (), Operation::Subscribe
}
action_class! {
    /// Check that source sequence numbers increase by exactly one.
    ///
    /// Args:
    ///     value: An expression yielding the message's unsigned sequence number.
    ///     key: An optional expression selecting a sequence stream within the
    ///         connection, for example a channel ID. None uses the connection's
    ///         default sequence stream.
    ///     reset: If true, establish a new starting sequence without checking the
    ///         previous number. Use it for a snapshot or other reset message.
    ///
    /// A gap fails the message and triggers the source's recovery behavior. State
    /// is isolated per connection and cleared on reconnect. The public custom
    /// module also exports this constructor as CheckSequence.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///
    ///     sequence = le.CheckSequence(le.Field("sequence"), key=le.Field("channel"))
    ///     ```
    CheckSequence, "lobo.replay.adapters.expressions", (value:&Bound<'_,PyAny>, key:Option<&Bound<'_,PyAny>>, reset:bool), (value,*,key=None,reset=false),
    Operation::Sequence{value:operand(value)?,key:optional(key)?,reset}
}
action_class! {
    /// Fail the current message with an explanation from the protocol definition.
    ///
    /// Args:
    ///     message: The error text reported to the adapter caller. A live socket
    ///         source reconnects after a processing error; a finite source reports
    ///         the error through wait() or run().
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     reject = le.When(le.Field("status").eq("error"), lm.Fail("The feed rejected the subscription"))
    ///     ```
    Fail, "lobo.replay.adapters.models", (message:String), (message), Operation::Fail{message}
}
action_class! {
    /// Apply related actions to one instrument as a snapshot or an incremental update.
    ///
    /// Selects the book and its precision variables, runs the nested actions, then
    /// checks the completed checksum and synchronization state. A failed message
    /// invalidates the snapshot; this operation does not promise rollback of each
    /// individual mutation already applied before the failure.
    ///
    /// Args:
    ///     symbol: An expression or constant naming a registered instrument.
    ///     *actions: Operations applied in order to this instrument. In a snapshot,
    ///         provide every retained order or level in the appropriate source order.
    ///     snapshot: A boolean expression or constant. True clears the previous
    ///         book before applying actions; false applies an incremental update.
    ///     timestamp: Source time in nanoseconds. None or omission uses Variable("clock").
    ///     depth: The retained L2 depth per side after processing. Zero means no
    ///         depth trimming. This is separate from Checksum.depth.
    ///     checksum: An optional Checksum declaration checked after all actions.
    ///     ready: Whether successful processing may mark the book synchronized.
    ///         Use false while assembling an incomplete snapshot; the default is true.
    ///
    /// Examples:
    ///     ```python
    ///     from lobo.replay.adapters import expressions as le
    ///     from lobo.replay.adapters import models as lm
    ///
    ///     snapshot = lm.Book("XYZ", le.ForEach(le.Field("orders"),
    ///         lm.Add(id=le.Field("id"), side=le.Field("side"), price=le.Field("price"), quantity=le.Field("size"))),
    ///         snapshot=True, timestamp=le.Field("time").timestamp("ms"))
    ///     ```
    Book, "lobo.replay.adapters.models", (symbol:&Bound<'_,PyAny>, actions:&Bound<'_,PyTuple>, snapshot:Option<&Bound<'_,PyAny>>, timestamp:Option<&Bound<'_,PyAny>>, depth:usize, checksum:Option<PyRef<'_,Checksum>>, ready:bool), (symbol,*actions: "Action",snapshot=None,timestamp=None,depth=0,checksum=None,ready=true),
    Operation::Book{symbol:operand(symbol)?,actions:self::actions(Some(actions.as_any()))?,snapshot:snapshot.map(operand).transpose()?.unwrap_or_else(||literal(false)),timestamp:clock(timestamp)?,depth,checksum:checksum.map(|c|c.clone()),ready}
}
