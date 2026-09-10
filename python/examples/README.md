# Python examples

| Example | Purpose |
| --- | --- |
| [ITCH](itch/README.md) | Custom adapter for local or HTTP gzip Nasdaq sessions; L3 replay |
| [Kraken](kraken/README.md) | Custom adapter for the public v2 WebSocket; L2 live |
| [Bitfinex](bitfinex/README.md) | Custom adapter for the public R0 WebSocket; L3 live |
| [Polymarket](polymarkets/README.md) | Discovers the 1,000 most liquid markets; public L2 live |
| [Order API](order_api/README.md) | Standalone L3 book accepting HTTP orders; no replay or upstream feed |
| [Order feed](order_feed/README.md) | Custom adapter consuming an existing order server's snapshots and updates |

The feed examples contain their full declaration in `adapter.py`, a context-managed `server.py`, and a README. From the repository root, run them as modules or directly as files. For example:

```sh
poetry run python -m python.examples.bitfinex.server
```

Open the printed URL to observe the source book.

Only the standalone Order API example includes a trader script:

```sh
poetry run python python/examples/order_api/server.py
```

In another terminal:

```sh
bash python/examples/order_api/trader_sim.sh http://127.0.0.1:8000
```

Open the server URL to watch its TRADER book. Run several trader scripts for more activity. Ctrl-C stops a trader or exits the server context.

The existing [serve_books.py](serve_books.py) also demonstrates serving books created directly in Python.

The notebook connections and custom replay benchmarks use these same adapter definitions. [export_definitions.py](export_definitions.py) exports them as inputs for the Rust parity tests:

```sh
poetry run python python/examples/export_definitions.py
```

[Validation results and benchmark commands](VALIDATION.md) document those comparisons.
