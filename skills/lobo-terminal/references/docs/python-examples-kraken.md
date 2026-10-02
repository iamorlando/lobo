# Kraken custom adapter

## server.py

Starts the adapter and web app inside `with server_context(...) as server`, prints the URL, and waits for Ctrl-C. Leaving the context stops the server and its adapter.

## adapter.py

### Message schema

Kraken's public v2 WebSocket sends JSON objects:

- `instrument`: `data.pairs` supplies symbols and price/quantity precision.
- `book`: `type` is `snapshot` or `update`; each `data` item contains a symbol, bids, asks and checksum. Level rows contain `price` and `qty`.
- `trade`: each data item contains a trade ID, symbol, execution price, quantity, taker side and timestamp.

A book update assigns an absolute quantity at a price; zero removes that level. The checksum covers the best ten bids and asks even when subscribing to a deeper book. See [Kraken's checksum guide](https://docs.kraken.com/exchange/guides/websockets/book-checksum-v2).

### Mapping

`Message` selects the channel. `ForEach` makes each pair or book item the current record. `Field(...)` reads that record; `Root("type")` still reads the enclosing message to distinguish snapshots from updates.

`Register` supplies decimal precision. `Level` changes a price level inside `Book`, which groups the update and checks its checksum before publishing. A mismatch requests a fresh snapshot. Decimal conversion uses the registered precision instead of assuming every pair has the same scale.

Trade messages map the taker's side to the opposite maker side and pass the execution price/quantity to `Trade`. `Send` declares the instrument, book and trade subscriptions. `build_adapter` uses `Source.websocket`, one selected pair, and live L2 mode. No credentials are required.

## How to run

Use the repository's installed Poetry environment, and run from the repository root:

```sh
poetry run python python/examples/kraken/server.py
```

Open the printed URL to see the source book's heatmap, depth and OHLC bars. The app's simulation controls remain available. Ctrl-C in the server terminal exits the context. Use `--port 8001` to run another server.

This is a public live feed; internet access is required, but credentials are not. Use `--symbol ETH/USD --depth 25` to choose another pair or depth. Kraken is L2, so the app supports market-order previews rather than order-level FIFO inspection.
