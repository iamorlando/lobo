> Bundled from `rust/crates/lobo_server/README.md` in https://github.com/iamorlando/loblib.
> This content is included in the installed skill; no checkout or download is needed.
> Check the installed wheel's public `help()` for version-specific signatures.

# Hosted Python books and order API

```python
from uuid import uuid4
from lobo import Book, server_context
from lobo.orders import LimitOrder, Side

with server_context(port=8000, price_decimals=2) as server:
    books = [Book("AAPL"), Book("MSFT")]
    trader = uuid4()
    books[0].fill(LimitOrder(18960, 100, trader, Side.Buy))
    books[0].fill(LimitOrder(18962, 150, trader, Side.Sell))
    print(server.url)
    input("Press Enter to stop the server\n")
```

Open the printed URL. The terminal runs in **live L3** mode, with only the books
registered by this server. The source switcher and book-scope selector are hidden.
Book selection, themes, heatmap/depth, OHLC aggregations, FIFO queues, and isolated
limit-order simulations use the same app and native APIs as other sources.

`server_context` accepts `host="127.0.0.1"`, `port=8000`, `price_decimals=0`,
`quantity_decimals=0`, and `queue_capacity=8192`. `port=0` selects an available port.
`web_root` can point to a separately packaged static terminal. The server starts
on construction and closes on context exit, including exceptional exits.
Existing Python book references remain usable after it closes.

`Book(...)` constructors inside the context register automatically. ContextVar
scopes support nesting and isolate async tasks. Worker threads can explicitly
create/retrieve books with `server.book("AAPL")`; `server["AAPL"]` retrieves an
existing book, and `server.keys()` lists the registered names. Python and HTTP
access the **same native storage**, not mirrored Python/server books.

Choose policies with flags on the single `Book` class:

```python
from lobo import Book, CheckSum

book = Book("AAPL", update_user_map=True, check_sum_spec=CheckSum.Null, update_hidden=True)
```

`CheckSum.Null`, `CheckSum.BitFinex`, and `CheckSum.Kraken` select no checksum,
Bitfinex's order checksum, or Kraken's aggregate-level checksum. Constructors
for all twelve flag combinations come from one Rust policy matrix; the Python
binding selects the constructor by the flag tuple before processing orders.
All combinations return the same Python type, including `server[name]` and
`server.book(name)`. Existing books keep their selected policies.

`update_hidden=True` tracks reserves and replenishes icebergs in FIFO order.
`update_hidden=False` ignores submitted reserves and only visible liquidity
participates. Disabled user maps return empty user summaries. These policies
are selected at construction; matching does not inspect the flags.

Checksum state belongs to the book. Level mutations invalidate its cached inputs;
the adapter finishes the checksum at the protocol's message boundary. The null
policy has no state and empty mutation hooks. `book.checksum()` returns the current
unsigned CRC32. Server books use the context's price and quantity decimal scales;
standalone books default to integer units. The feed carries the user/hidden policy
so browser reconstruction and simulations use the same bookkeeping behavior.

## Observer metrics

Book policies and observer visibility are independent. Price-level events always
carry the native hidden total (zero on a hidden-disabled book). The batcher applies
`PriceLevelMetrics` bit flags after publication; unselected hidden fields are zeroed
without changing the schema or book state.

```python
from lobo.sinks import FeatherSink, GpuSink, PriceLevelMetrics

# Visible depth by default; opt into visible + hidden depth in the hosted app.
with server_context(sinks=[GpuSink(metrics=PriceLevelMetrics.HIDDEN_QUANTITY)]) as server:
    book = Book("RESERVES")
    # Fill books here and keep this scope open while using the terminal.

# For Context/ReplayContext, omit reserve totals from Feather observers.
sink = FeatherSink("visible.feather", metrics=PriceLevelMetrics.NONE)
```

`FeatherSink` keeps its existing default of including hidden totals; `GpuSink`
defaults to visible depth. The GPU depth widths and labels use the same selected
quantities. These flags govern price-level output and chart display; the server's
order-command feed retains the order data needed to reconstruct native matching.

## Order API

`POST /api/books/{symbol}/orders` takes one JSON command. Prices are unsigned
32-bit integer atoms, and quantities are unsigned 64-bit integer atoms, matching
the Python book. Precision controls display only: `18960` with two price decimals
is displayed as `189.60`. IDs are UUIDs; `trader` defaults to the nil UUID.

```json
{
  "op": "add",
  "order": {
    "type": "limit",
    "id": "00000000-0000-0000-0000-000000000001",
    "side": "buy",
    "price": 18960,
    "quantity": 100
  }
}
```

| `op`       | Fields                                                | Behavior                                                                                         |
| ---------- | ----------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `add`      | `order`                                               | Insert limit/iceberg liquidity without matching.                                                 |
| `execute`  | `id`, `quantity`, optional `price`                    | Reduce the named order; publish executed volume at the supplied or resting price.                |
| `cancel`   | `id`, `quantity`                                      | Reduce the named order without reporting a trade.                                                |
| `remove`   | `id`                                                  | Remove the entire resting order.                                                                 |
| `modify`   | `id`, absolute `quantity`, optional `price`, `new_id` | Reductions preserve FIFO priority; increases, price changes, and identity changes lose priority. |
| `fill`     | `order`                                               | Match through the native fill API and rest an eligible remainder.                                |
| `simulate` | `order`                                               | Run a nonmutating native preview; mark the response/executions simulated.                        |

`market` orders have `id`, `side`, and `quantity`. `limit` adds `price`.
`iceberg` adds `price`, `hidden_quantity`, and `peak_quantity`; `quantity` is the
initial visible quantity and must be positive and no greater than the peak.
Replenishment uses the native order policy and rejoins the FIFO queue.

Responses include the book sequence, order ID, filled/unfilled quantities,
average execution price in atoms, execution details, simulation flag, and any
resting order ID. A simulated response never represents a committed resting order.
Unknown books/orders return 404, duplicate IDs return 409, and invalid command
values return 422. Invalid requests do not advance the feed sequence.

- `GET /api/books`: registered book metadata.
- `GET /api/books/{symbol}`: a snapshot in native FIFO order.
- `GET /api/schema`: command, response, and feed JSON Schemas generated from Rust
  structs using [Schemars](https://docs.rs/schemars/latest/schemars/macro.schema_for.html).
- `GET /api/server-context`: terminal mode and registered book metadata.
- `WS /api/feed`: initial snapshots followed by ordered command updates for every
  registered book. The same server adapter reconstructs them in WebAssembly.
