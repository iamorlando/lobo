> Bundled from `rust/crates/lobo_replay/src/custom/README.md` in https://github.com/iamorlando/lobo.
> This content is included in the installed skill; no checkout or download is needed.
> Check the installed wheel's public `help()` for version-specific signatures.

# Custom adapters

Define a protocol in Python and connect it to a source. `CustomAdapter` applies its message mappings to books and supplies the same directory, charts, execution bars, and simulation interface used by other adapters.

```python
from lobo import server_context
from lobo.replay.adapters import CustomAdapter, Protocol
from lobo.replay.adapters import expressions as le
from lobo.replay.adapters import models as lm

protocol = Protocol(lm.Json(
    lm.Message(le.Field("event").eq("snapshot"),
        lm.Book(le.Field("symbol"),
            lm.Add(id=le.Field("id"), side=le.Field("side"),
                price=le.Field("price"), quantity=le.Field("quantity")),
            snapshot=True, timestamp=le.Field("time_ns"))),
    lm.Message(le.Field("event").eq("quantity"),
        lm.Book(le.Field("symbol"),
            lm.Modify(id=le.Field("id"), quantity=le.Field("quantity")),
            timestamp=le.Field("time_ns"))),
))

adapter = CustomAdapter(
    protocol,
    lm.Source.packets([
        b'{"event":"snapshot","symbol":"EXAMPLE","id":1,"side":"sell","price":12500,"quantity":100,"time_ns":1000}',
        b'{"event":"quantity","symbol":"EXAMPLE","id":1,"quantity":75,"time_ns":2000}',
    ]),
    instruments=(lm.Instrument(symbol, 2, 0) for symbol in ["EXAMPLE"]),
    symbol="EXAMPLE", mode=lm.FeedMode.Live, level=lm.BookLevel.L3,
)

with server_context(adapters=[adapter], port=0) as terminal:
    adapter.wait()  # This example source is finite.
    print(adapter.levels("EXAMPLE"))
    print(terminal.url)
    input("Press Enter to close the terminal\n")
```

The source above can be replaced by `lm.Source.websocket(url)`, `lm.Source.json_lines(path)`, or another source appropriate to the declared wire format. A protocol is a definition; it does not wrap an existing adapter.

## Python modules

Use `lobo.replay.adapters` as the entry point:

- `models` contains wire formats, book actions, source configurations, and the
  `FeedMode` and `BookLevel` enums. Import it as `lm`.
- `expressions` contains field selectors, transformations, variables, and control
  flow such as `When` and `ForEach`. Import it as `le`.
- `CustomAdapter` and `Protocol` are available directly. `CustomAdapter` also has
  its own `custom` module.
- `itch.ItchSource`, `kraken.KrakenSource`, and `bitfinex.BitfinexSource` expose
  the shipped adapters. An ITCH source is passed to `ReplayContext`; live sources
  return an `AdapterSession` from `start()`.

For example, start the shipped Kraken adapter and inspect its directory:

```python
from lobo.replay.adapters import kraken

with kraken.KrakenSource("BTC/USD", depth=100).start() as session:
    available = session.tickers()
```

Live directory discovery continues in the background. Once a book is synchronized,
use `session.levels(symbol)`, `session.simulate(...)`, and the shared bar and queue
methods. Bitfinex uses `bitfinex.BitfinexSource("BTCUSD")` in the same way. An optional
`source=lm.Source.json_lines(path)` reads a recorded exchange stream through the
same adapter. Closing the session stops its worker. The custom examples below
remain declarations of the wire formats rather than wrappers around these sources.

## Reference documentation

The declaration types, constructors, and documentation are defined in Rust. PyO3
exposes those same objects and docstrings to Python, and the normal Maturin build
generates their type stubs. Python's public package only re-exports them.

Use `help(Protocol)`, `help(lm.Bootstrap)`, `help(lm.Book)`, and `help(CustomAdapter)` for
complete argument descriptions, defaults, units, errors, and examples. Individual
methods also have documentation, such as `help(le.Field.decimal)` and
`help(lm.Source.http)`. Prices and quantities are measured in **integer atoms**, not
display units; every conversion should use the instrument's declared precision.

Creating a declaration does not execute it or open a connection. `Protocol(...)`
validates the assembled definition, and `CustomAdapter(...)` prepares it. Pass the
Protocol object directly; `lm.specification(protocol)` exports a detached dictionary
only when you want to inspect or save it. Editing an exported dictionary does not
change the original declaration.

## Fields and messages

- `Field(...)` reads the current record or `ForEach` item. `Root(...)` reads the enclosing message. Paths support object keys and array indices, including negative indices.
- `Variable(...)` reads a value bound with `Let`. The session also provides `symbol`, `connection`, `clock`, `price_decimals`, and `quantity_decimals`.
- Expressions support exact numeric comparisons, decimal scaling, timestamps, mapping, conditional choice, and connection-local table lookup. Use `&` and `|` to combine conditions.
- `Json(Message(condition, *actions), ...)` maps JSON packets. `Binary(...)` declares a length prefix, record tag, routing key, timestamp, and fixed record layouts using `Record`, `UInt`, and `Text`.
- `Add`, `Execute`, `Cancel`, `Remove`, `Modify`, and `Replace` use L3 order operations. `Upsert` maps absolute order updates, including deletion when its price is `None`.
- `Level` sets an L2 level's absolute quantity. `Trade` reports executions for bars and live simulation reconciliation. `OrderCommand` accepts the library's order API schema, including fills and icebergs.

Prices and quantities are integer atoms. For example, `Field("price").decimal(2)` converts `"125.00"` into `12500`. Timestamp conversion accepts `ns`, `us`, `ms`, `s`, and `rfc3339`.

## Directories, snapshots, and connections

`Register` declares instrument precision, policy, and an optional routing key. Supply an `instruments=` iterable at construction, or register instruments through protocol messages and bootstrap requests. `Directory` replaces a complete directory, with optional cleanup actions for removed instruments. `DirectoryComplete` marks the end of a replay directory.

`Book(symbol, *actions, snapshot=..., timestamp=...)` groups a completed update. It can enforce retained depth and a `Checksum` before marking the book ready. Set `ready=False` for messages that should not establish snapshot readiness, such as a trade arriving before the first checksum.

A checksum declares its input view, depth, side ordering, fields, decimal formatting, separators, and interleaving. The declaration configures the book’s checksum policy during adapter construction. Changed levels invalidate cached operands; the adapter compares the book’s result with the supplied value after the complete update. `on_failure` can request another snapshot; otherwise a mismatch fails the connection. `CheckSequence` detects gaps. Snapshots, checksum validation, order mutations, and simulations all use the existing book APIs.

### Startup discovery and subscriptions

A **bootstrap** is an HTTP JSON request made before opening a live connection.
It commonly supplies a list of instruments and their precision. It is separate
from the book snapshot sent by a WebSocket: handle that snapshot with a `Message`
rule containing `Book(..., snapshot=True)`.

For example, suppose the source's instrument endpoint returns
`{"symbols": [{"name": "ABC", "price_dp": 2}]}` and its WebSocket accepts
`{"method": "subscribe", "symbol": "ABC"}`. The lifecycle declaration is:

```python
from lobo.replay.adapters import Protocol
from lobo.replay.adapters import expressions as le
from lobo.replay.adapters import models as lm

discovery = lm.Bootstrap(
    "instruments", "https://feed.example/instruments",
    le.ForEach(le.Field("symbols"),
        lm.Register(symbol=le.Field("name"), price_decimals=le.Field("price_dp"),
                 quantity_decimals=0)),
)
protocol = Protocol(
    lm.Json(),  # Add this source's snapshot and update Message rules here.
    bootstrap=(discovery,),
    connect=(lm.Subscribe(),),
    subscriptions=(lm.Send({"method": "subscribe", "symbol": le.Variable("symbol")}),),
)
```

The URL above illustrates an endpoint shape. Use your source's actual endpoint.
If the instrument directory is known in advance, supply `Instrument` objects to
`CustomAdapter(instruments=...)` and omit the bootstrap. When replaying a recorded
JSON stream, provide discovery response bytes under the same bootstrap name via
`Source.json_lines(..., bootstrap=[("instruments", response_bytes)])`.

| Protocol argument | When it runs and why it exists |
| --- | --- |
| `format` | Describes how each incoming record or JSON packet changes the books. |
| `bootstrap` | Fetches startup JSON responses in order before live connections open. Defaults to none. |
| `connect` | Runs on every connection opening or reopening. Send a handshake here, or call `Subscribe()` when no handshake is needed. |
| `subscriptions` | Runs once per requested, available ticker after `Subscribe()` marks that connection ready. `Variable("symbol")` is the ticker being subscribed. |
| `keepalive` | Runs for each connected WebSocket on the transport heartbeat, currently every fifteen seconds. Usually sends an application-level ping. |
| `simulation_note` | Supplies explanatory text about this source's simulation limitations. |
| `reconcile_window_ns` | Associates separate public trades and absolute L3 order updates during a simulation. Defaults to 250 milliseconds in nanoseconds; it does not delay the main book. |
| `reconcile_capacity` | Bounds pending simulation reconciliation entries. Defaults to 4096; it must be positive. |
| `symbols_per_connection` | Splits requested subscriptions across connections when positive. Zero uses one connection. It does not restrict the ticker directory. |

If a source requires a handshake response before subscribing, put `Subscribe()`
in the `Message` rule that accepts that response, instead of in `connect`.
Selecting another ticker keeps visited subscriptions running. `scope=` restricts
which books may exist; it does not select a displayed ticker.

`Remember(table, key, value)` stores a connection-local lookup entry, for example
an exchange channel ID mapped to a symbol. `Field("channel").lookup("channels")`
retrieves it. These lookup entries and sequence baselines are cleared when their
connection reopens. `Let` assigns protocol variables; `Variable` reads them.

Built-in variables give declarations their execution context:

| Variable | Meaning |
| --- | --- |
| `symbol` | The currently routed book or ticker being subscribed. |
| `connection` | The connection ID, used to isolate channel maps and sequence state. |
| `clock` | Source time in nanoseconds, supplied by the binary header or current session clock. A `Book` timestamp sets the clock for its nested actions. |
| `key` | The binary record's instrument routing key. |
| `price_decimals` / `quantity_decimals` | The selected instrument's precision, for converting wire decimals to integer atoms. |

Use `Root` to access the original packet inside nested `ForEach` actions; `Field`
then refers to the current array item. `Choose`, `When`, `&`, and `|` describe
conditional processing. Python's `and` and `or` cannot be used on expressions.

- `start()` runs a source in the background; `wait()` waits for finite input, and `close()` stops it. The context manager closes resources on exit.
- `run(concurrent=True, sinks=[...])` performs grouped binary file replay into Python books. It supports cutoff times and statistics. Definitions needing compound record processing use `start()`.
- `Source.file(path)` accepts plain or gzip files. `Source.http(url)` streams plain or gzip HTTP input without creating a downloaded replay file.
- `server_context(adapters=[adapter])` starts the adapter and serves its books and the app. Attach it before calling `start()` or `run()`. API commands for an attached adapter use `/api/adapters/{index}/orders` with `{"book": "SYMBOL", "command": ...}`.
- `tickers`, `levels`, `queue`, `simulate`, and `simulation_report` expose running books. `set_bar_aggregation` and `drain_bars` control execution bars.

An instrument's `book_policy` chooses `full`, `no_user_map`, `no_hidden_quantity`, or `no_updates` before its book is constructed. Hidden-quantity display remains a sink setting, independent of matching policy.
