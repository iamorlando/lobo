# Polymarket live L2 adapter

`adapter.py` discovers the **1,000 most liquid non-empty open markets** and builds
a custom adapter for their public WebSocket feed. Discovery sorts Gamma markets
by liquidity, follows pagination, and filters out closed, archived, or disabled
markets. Each outcome has its own readable ticker and book. No credentials or
manually copied token IDs are required.

## Server

`server.py` starts the app and feed together inside `server_context`. From the
repository root, run:

```sh
poetry run python -m python.examples.polymarkets.server --open
```

The app opens at `http://127.0.0.1:8000` after discovery. Omit `--open` to open
the printed URL yourself, or add `--port 8001` to choose another port. Running
`poetry run python python/examples/polymarkets/server.py` also works.

Type a market name in **SYMBOL** to search the discovered outcomes. **BOOK SCOPE**
uses the same directory: All opens instruments as you visit them; a selection
keeps its chosen instruments live. Existing server subscriptions remain available
to other browser sessions. The heatmap and depth
chart follow level updates, and OHLC bars accumulate reported trades. L2 market
simulation reports fills, their average price, and the unfilled quantity without
changing the book or starting an alternate timeline. Stop the server with Ctrl-C.

Python callers can use `build_adapter()` with no arguments, call
`discover_instruments()` for the label-to-token mapping, or supply an explicit
mapping to `build_adapter(tokens, scope=list(tokens))`. Discovery and Polymarket
protocol handling live entirely in the Python example, using the public adapter
API and the [Gamma market directory](https://docs.polymarket.com/api-reference/markets/list-markets-keyset-pagination).

## Message mapping

The [market WebSocket protocol](https://docs.polymarket.com/market-data/realtime-data)
publishes these messages:

| Message | Relevant fields | Declaration |
| --- | --- | --- |
| `book` | `asset_id`, `timestamp`, `bids` and `asks` containing `price`/`size` | Replace the book with a `Book(snapshot=True)` and its `Level` entries. |
| `price_change` | `timestamp`, `price_changes` containing `asset_id`, `side`, `price`, `size` | Assign each level's absolute quantity. A zero size removes it. |
| `last_trade_price` | `asset_id`, `timestamp`, `side`, `price`, `size` | Report a `Trade` at the execution price without subtracting liquidity twice. |

Initial snapshots can arrive in an array; individual events use the same action
definitions. Each selected token gets its own connection and a fresh snapshot on
reconnect. `TextHeartbeat("PING", "PONG", interval=10)` handles the protocol's
plain-text heartbeat in the transport.

Prices use dollars per share and quantities use shares, both with six decimal
digits. For example, `0.425` becomes `425000` price atoms. Fixed precision also
accommodates tick-size changes without rescaling existing levels. Other event
types do not mutate the book. This example does not validate the feed's `hash`
field.

`FeedMode.Live` and `BookLevel.L2` describe the source's capabilities through the
typed library API. All exchange-specific declarations are in `adapter.py`.

## Tests

The adapter tests belong to the Python test suite:

```sh
poetry run pytest -q python/tests/lobo/test_polymarket_adapter.py
```
