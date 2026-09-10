# ITCH custom adapter

## server.py

Starts the adapter and web app inside `with server_context(...) as server`, prints the URL, and waits for Ctrl-C. Leaving the context stops the server and its adapter.

## adapter.py

### Message schema

NASDAQ ITCH 5.0 files contain binary messages preceded by a two-byte big-endian length. Offsets in `Record.fields` start at the message tag, excluding that length prefix. The common header is 11 bytes: tag, stock-locate number, tracking number and a six-byte nanosecond timestamp.

| Tag | Meaning | Book action |
| --- | --- | --- |
| R | Stock directory | Register the symbol and its stock-locate route |
| A / F | Add order / add with attribution | Add |
| E / C | Execute / execute at a supplied price | Execute |
| X | Cancel part of an order | Cancel |
| D | Delete an order | Remove |
| U | Replace with a new order ID | Replace |
| S | System event | Mark directory completion at start of system hours |

An A record contains an order ID, B/S side, share quantity, padded symbol and price. Prices have four decimal places: `10000` means $1.0000. For the complete wire specification, see [NASDAQ TotalView-ITCH 5.0](https://www.nasdaqtrader.com/content/technicalsupport/specifications/dataproducts/NQTVITCHSpecification.pdf).

### Mapping

`Binary` declares framing and the routing/clock fields. Each `Record` declares its size, field locations and actions. `Field("quantity")` refers to a declared field; `Variable("key")` is the stock-locate value read by `Binary`. `Register` connects that number to the symbol. The library's `Add`, `Execute`, `Cancel`, `Remove` and `Replace` actions update the book.

`build_adapter` combines the declaration with a `Source`, the selected symbol, L3 replay mode and the New York timezone. Pass `scope=["AAPL"]` for one book or `scope=None` for all symbols. `server.py` scopes the books to its `--symbol` argument. The file contains the whole definition; it imports only the public library API.

## How to run

Use the repository's installed Poetry environment, and run from the repository root:

```sh
poetry run python python/examples/itch/server.py
```

Open the printed URL to see the source book's heatmap, depth and OHLC bars. The app's simulation controls remain available. Ctrl-C in the server terminal exits the context. Use `--port 8001` to run another server.

The default file is `data/NASDAQ/01302020.NASDAQ_ITCH50`. For another file, use `--source /path/to/session.gz --symbol MSFT`. For an online gzip session:

```sh
poetry run python python/examples/itch/server.py \
  --source 'https://emi.nasdaq.com/ITCH/Nasdaq%20ITCH/01302020.NASDAQ_ITCH50.gz'
```

The source reads and decompresses the session. Server replay advances as fast as input processing allows; it is not paced by the browser's replay-speed control. The completed book remains available while the server runs. Click a depth level to inspect its FIFO queue.
