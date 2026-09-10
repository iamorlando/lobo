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

The source reads and decompresses the session on the host. Replay starts paused.
Use Play/Pause, speed, Restart, and START AT / Apply in the terminal, or pass
`--play --speed 10` to start at 10×. `--start-ns 34200000000000` reconstructs through
09:30:00 in a file with nanoseconds-since-midnight timestamps. Seek uses the file's
absolute timestamp convention, so epoch-based recordings require epoch nanoseconds.

Python uses the same shared controls: `adapter.play()`, `adapter.pause()`,
`adapter.set_speed(10)`, `adapter.restart()`, and `adapter.seek(timestamp_ns)`.
`adapter.playback` reports pause, speed, source clock, seeking, errors, and EOF.
Seek and restart return after reconstructing from the beginning, retaining pause
and speed. Seeking earlier than the origin clamps to the origin; seeking beyond
EOF stops at the final timestamp. EOF freezes playback and leaves the final book
available. Click a depth level to inspect its FIFO queue.

`GET /api/server-context` advertises a `playbackEndpoint` for each recorded
adapter. GET that endpoint for state; POST `{"action":"play"}`, `{"action":"pause"}`,
`{"action":"speed","speed":10}`, `{"action":"restart"}`, or
`{"action":"seek","timestamp_ns":34200000000000}` to control the same worker.
These controls are shared by all observers. A partial recording remains PARTIAL;
playback and reconstruction do not supply a missing opening snapshot.
