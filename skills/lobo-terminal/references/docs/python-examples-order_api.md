# Standalone order API

## server.py

Creates a `TRADER` book inside `with server_context(...) as server`, seeds 500 shares bid at $99.95 and offered at $100.05, prints the URL, and waits for Ctrl-C. Exiting the context shuts down the server.

This book has no replay, exchange connection or upstream feed. Python creates it directly using the library; subsequent orders arrive through the server's HTTP API. There is no custom adapter to configure in this example.

## Order endpoint

Send commands to `POST /api/books/TRADER/orders`. The `op` field selects `add`, `execute`, `cancel`, `remove`, `modify`, `fill` or `simulate`. Add/fill/simulate carry an `order` with type `market`, `limit` or `iceberg`; other commands identify an existing order.

For example, this command fills crossing quantity and rests any remainder:

```json
{
  "op": "fill",
  "order": {
    "type": "limit",
    "id": "00000000-0000-0000-0000-000000000001",
    "side": "buy",
    "quantity": 25,
    "price": 10005
  }
}
```

Prices are integer cents and quantities are whole shares. `/api/schema` exposes the full schemas. The library handles the command and publishes book updates to the app.

## trader_sim.sh

Sends a limit order every 0.1 seconds using curl. Each order independently chooses buy or sell, 1–100 shares, and a price between $99.50 and $100.50. A fresh UUID lets multiple copies run against the same book.

`"op": "fill"` matches crossing quantity and leaves the remainder resting. Curl prints the response, including fills and the resting order ID. The script demonstrates the order endpoint and creates activity to observe in the app.

## How to run

From the repository root, in the installed Poetry environment:

```sh
poetry run python python/examples/order_api/server.py
```

Open the printed URL; the app displays the TRADER book. In another terminal:

```sh
bash python/examples/order_api/trader_sim.sh http://127.0.0.1:8000
```

The script needs Bash, curl and `uuidgen`. Run several copies to create more activity. Watch the heatmap and depth, click a level for its FIFO queue, or choose OHLC tick bars to see executions become candles. The app's simulation controls remain available.

Ctrl-C stops a trader. Ctrl-C in the server terminal exits the context. Use `--port 8001` and the corresponding URL to run another instance.
