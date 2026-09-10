> Bundled from `web/README.md` in https://github.com/iamorlando/loblib.
> This content is included in the installed skill; no checkout or download is needed.
> Check the installed wheel's public `help()` for version-specific signatures.

# Terminal behavior and adapter checks

- The symbol box autocompletes from `FeedSession.scoped_tickers()`, the context's sorted instrument directory restricted to the current scope (including symbols without orders yet).
  Type to filter, use arrow keys/Enter, or click a suggestion. Every active book
  advances and accumulates GPU history simultaneously. Switching selects an
  existing book and history page, preserving playback time and file position.

- **Book scope** next to the data source defaults to **AAPL only** for hosted
  Nasdaq sessions and **All tickers** for local and repository replay files. The wide dialog
  shares the symbol box's autocomplete ranking and offers a short Top tech list
  and an S&P 500 preset from dated SPY holdings, intersected with the feed directory.
  Applying scope reconstructs the replay at **Start at**, allocating native books
  only for selected symbols. The full directory remains available to expand scope.
  For live feeds, All keeps the existing subscribe-as-visited behavior; an explicit
  selection subscribes every chosen pair. Changing sources restores that source's
  default scope; restarting preserves the user's current scope.

## OHLC bars

Select **OHLC bars**, then choose **Aggregate by** and the adjacent size dropdown:

| Aggregation | Size                                   | Closes when                                                                                                               |
| ----------- | -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| Volume      | Quantity (default 5,000)               | Exactly the target quantity executes; crossing executions are split.                                                      |
| Time        | 1 second to 5 minutes                  | The aligned exchange-time interval ends. Empty intervals emit no candle.                                                  |
| Ticks       | Execution count (default 100)          | That many execution messages arrive, regardless of quantity.                                                              |
| Notional    | Quote currency value (default 100,000) | Execution price × quantity reaches the target. The entire crossing execution is included, so value may exceed the target. |

Both views and all active tickers keep advancing. Switching tabs or tickers does
not restart replay. Changing aggregation or size starts fresh bars for all books
at the current replay position; each aggregation remembers its chosen size.
A translucent candle shows the unfinished bar. Only completed bars reach the
final sink. The lower panel always shows executed quantity, scaled to the visible
bars. Time bars have exchange-time labels; other bars use sequential indexes.
The view fits the latest 80 candles; GPU history retains 256 slots per book.

Kraken supports the same four bar aggregations. Its public `trade` channel supplies
execution prices and quantities; the adapter converts taker side to maker side and
publishes `TradedVolumeEvent` through the native book publisher. Trade IDs prevent
duplicate counting across reconnects. Book changes still use native quantity
updates and checksum validation; they never invent executions or reduce liquidity
a second time. Kraken volume sizes use whole base-asset units (default 1), not raw
decimal atoms. Live time bars close with the next trade in a later interval;
book-channel timestamps and wall-clock heartbeats cannot prematurely close them.

## Order simulation and level queues

The adapter describes two independent capabilities: `mode` (`replay` or `live`)
and `level` (`l2` or `l3`). ITCH is L3 replay; Kraken is L2 live.

| Book               | Market simulation                   | Limit simulation                                                    |
| ------------------ | ----------------------------------- | ------------------------------------------------------------------- |
| L2, live or replay | Nonmutating aggregate-depth preview | Unavailable                                                         |
| L3, replay         | Nonmutating native FIFO preview     | Separate native book; worse-price executions redirect to your limit |
| L3, live           | Nonmutating native FIFO preview     | Separate native book; natural FIFO matching                         |

Choose **Simulate order**, a side, order type and quantity. Market previews report
simulated fills, their weighted average price and any unfilled quantity, without
creating a timeline or changing liquidity. Kraken supports fractional quantities
at the instrument's precision. L2 counterparties have no maker order ID; they
are never turned into fabricated native orders.

**No chart pixels, textures, or GPU buffers are read back to the CPU.** There is no
`MAP_READ`, `mapAsync`, `getImageData`, or pixel-copy rendering path. A separate
Canvas 2D layer rasterizes labels and crosshairs from clock/camera metadata.
DOM quote/counter labels use the CPU book's metadata, not GPU readback.
`chart_layout()` exports the same geometry used to compile the display shader.
`price_band_at()` and queue selection share native price-bin and side calculation,
including bins containing both quotes. The WebGPU palette follows the design
system and accounts for the canvas surface's color space.
