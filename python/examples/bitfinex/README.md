# Bitfinex R0 custom adapter

## server.py

Starts the adapter and web app inside `with server_context(...) as server`, prints the URL, and waits for Ctrl-C. Leaving the context stops the server and its adapter.

## adapter.py

### Message schema

Bitfinex sends JSON objects for connection/subscription events and arrays for book/trade data. A subscription response assigns a `chanId` to a symbol and channel.

An R0 order row is `[order_id, price, signed_amount]`. Positive amount is a bid; negative is an ask. A snapshot contains a list of rows; an update contains one row. Price zero removes the referenced order. See [Raw Books](https://docs.bitfinex.com/reference/ws-public-raw-books).

The configuration flags explain the extra fields on array messages:

| Flag | Value | Purpose |
| --- | ---: | --- |
| TIMESTAMP | 32768 | Append a millisecond timestamp |
| SEQ_ALL | 65536 | Append a sequence number |
| OB_CHECKSUM | 131072 | Request order-book CRC32 checksums |

The named values are combined with bitwise OR. With these options, the last two array fields are sequence and timestamp. See [WebSocket configuration](https://docs.bitfinex.com/docs/ws-general).

### Mapping

`Bootstrap` loads the instrument directory. `Remember("channels", ...)` associates channel IDs with symbols; later messages use `lookup("channels")` to recover that record. `Variable("symbol")` is the instrument being processed. `Variable("channel")` reads the record saved by `Let("channel", ...)`.

A snapshot uses `Add`; subsequent rows use `Upsert`, with no price for deletions. Prices and absolute amounts use eight decimal places. Snapshot order IDs establish initial FIFO priority; subsequent updates use the library's order mutation rules. `CheckSequence` detects missing messages.

`Checksum` interleaves IDs and signed amounts for the best 25 orders on each side, joined with colons and formatted like the feed. See [Bitfinex checksums](https://docs.bitfinex.com/docs/ws-websocket-checksum). Public trade messages feed `Trade`; trade-history IDs and reconciliation avoid counting duplicate execution notifications.

`build_adapter` combines this declaration with the public WebSocket source and live L3 mode. No credentials are required.

## How to run

Use the repository's installed Poetry environment, and run from the repository root:

```sh
poetry run python python/examples/bitfinex/server.py
```

Open the printed URL to see the source book's heatmap, depth and OHLC bars. The app's simulation controls remain available. Ctrl-C in the server terminal exits the context. Use `--port 8001` to run another server.

This is a public live feed; internet access is required, but credentials are not. Use `--symbol ETHUSD` to select another pair. Click a depth level to inspect its estimated FIFO queue. Simulated fills use visible orders and public trades; they do not place orders on the exchange.
