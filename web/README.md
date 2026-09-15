# lobo Orderbook Viewer

From the repository root:

```sh
make web
```

Open **http://127.0.0.1:3000** in a browser with WebGPU enabled (localhost is a
secure context). The command installs npm dependencies, builds `lobo_wasm`,
and starts the Next.js App Router app. Rust and Node.js are required;
`wasm-pack` installs its matching binding generator and missing WASM target.

- **Nasdaq sample sessions** is the default source. Its dropdown is populated from
  [Nasdaq's public directory](https://emi.nasdaq.com/ITCH/Nasdaq%20ITCH/), including
  all listed ITCH 5.0 `.gz` sessions. Sidecars, ZIP archives, and the legacy
  `tvagg.gz` layout are excluded. The first listed session loads automatically
  with the Top tech preset in book scope.
  Compressed byte ranges stream through Next.js with bounded parallel read-ahead
  and are incrementally decompressed in the browser before entering the existing
  replay adapter. Nothing is saved to
  disk, and replay starts without waiting for the whole file. Internet access is
  required; restarting or changing sessions opens a new stream from the beginning.
- **Repository file** streams `data/NASDAQ/01302020.NASDAQ_ITCH50`. Set
  `LOBO_ITCH_PATH=/absolute/path/to/file make web` to use another server-local file.
- **Open replay file** reads a selected raw or gzip file with `File.slice`; it is never uploaded.
- **Kraken Spot · Live** connects to the public WebSocket v2 feed without credentials.
  The symbol box lists Kraken's instrument directory. Selecting a pair subscribes
  to its top 100 price levels and public executions; visited pairs remain subscribed and keep their GPU
  histories. Reconnect restores those subscriptions from fresh snapshots.
- The symbol box autocompletes from `FeedSession.scoped_tickers()`, the context's sorted instrument directory restricted to the current scope (including symbols without orders yet).
  Type to filter, use arrow keys/Enter, or click a suggestion. Every active book
  advances and accumulates GPU history simultaneously. Switching selects an
  existing book and history page, preserving playback time and file position.
- Files must contain **NASDAQ ITCH 5.0**, raw or gzip-compressed,
  with their two-byte record lengths, starting before the orders being replayed.
- **Start at** defaults to 09:30:00 exchange time. Earlier records reconstruct
  scoped books before the playback clock starts. Changing the source/start time or
  restarting reconstructs from the beginning; switching tickers does not.
- **Real-time / 5× / 10× / 100× / 1000×** use recorded timestamps. All speeds
  stop at their playback horizon. Short work slices keep controls and rendering
  responsive; very high event rates or slow storage can make replay lag the
  selected speed. Changing speed preserves the replay position. Pause preserves
  the book; hidden tabs pause playback.
- Reconstruction to **Start at** automatically runs without market-chart drawing, axes, or
  queue inspection. A separate WebGPU pass animates up to 100 actual scope tickers
  as directory entries arrive, using bounded pixel-font sizes (at most 56 CSS px). A progress bar shows the source time advancing toward the
  selected start. Books and candle aggregators continue in bounded work slices,
  without animation-frame waits. The first chart is drawn when reconstruction
  finishes, then playback continues at the selected speed. There is no manual
  fast-forward mode. Changing **Start at** and pressing **Apply** uses the same path.
  Browser execution, event aggregation, and streamed gzip input still differ
  from native benchmarks.
- **Book scope** next to the data source defaults to **Top tech** for hosted
  Nasdaq sessions and **All tickers** for local and repository replay files. The wide dialog
  shares the symbol box's autocomplete ranking and offers a short Top tech list
  and an S&P 500 preset from dated SPY holdings, intersected with the feed directory.
  Applying scope reconstructs the replay at **Start at**, allocating native books
  only for selected symbols. The full directory remains available to expand scope.
  For live feeds, All keeps the existing subscribe-as-visited behavior; an explicit
  selection subscribes every chosen pair. Changing sources restores that source's
  default scope; restarting preserves the user's current scope.
- **iTerm theme** loads the complete upstream WezTerm-format catalog and the chosen
  palette directly from [iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes).
  No theme files are hosted or bundled here. The browser caches upstream responses;
  the selection is remembered locally. New visitors start with **Acid Lime**.
  **System** selects Espresso for dark OS mode
  and GitHub Light Default for light mode. Controls, charts, background gradients,
  FIFO segments, and loading glyphs all derive from the chosen ANSI colors.
  Theme changes preserve replay position, chart history, and simulations. If GitHub
  is unavailable, the current palette remains usable and the selector offers Retry.
- Depth's right edge labels cumulative quantities at eight uniformly spaced
  bucket centers, matching the shaded depth at each price. Labels use the same
  bid/ask split as the chart, including buckets containing both sides. Hovering
  shows both the individual bucket quantity and cumulative depth for the selected
  side. The cumulative depth area extends left from its right-hand zero
  baseline. Its horizontal quantity axis shows the current scale (the larger
  visible side total), so changes in scale are explicit. Quantities are summed
  from native levels using the GPU bin coordinates,
  with no GPU readback.
- Each book automatically zooms out with headroom when its best bid or ask reaches
  the visible edge. **Min. range** sets the starting width; automatic zoom keeps
  the center fixed and never shrinks it. Zooming, scrolling, and recentering preserve
  history at its original prices. Changing the history window clears chart history.
  These controls preserve native book state and OHLC bars.
  The depth curves accumulate asks from the lowest price and bids from the highest
  price within the visible range. Both share the heatmap’s vertical price axis;
  the quantity-history strip spans the full width below them.

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

The workspace fills the viewport, with source, quote, transport, and status data
in a compact left rail. Simulation fills and FIFO queues dock below the chart.
The layout and locally hosted JetBrains Mono typography follow `DESIGN.md` and
the HaX0R Terminal Stitch reference; reusable CSS tokens live in
`app/design-tokens.css`. In the orderbook view, scroll over the chart to move through
price levels. Focus the chart and use **↑ / ↓** for the same action; **Home** or
**Recenter** returns to the market at the chosen minimum range and resumes automatic
zoom. Scrolling pauses automatic tracking for that book so you can inspect distant
levels. Each historical column retains its captured price range, so old liquidity
stays at the correct price when the view changes; prices outside a column's original
range have no recorded raster data. Native books and OHLC bars continue unchanged. No pixels or GPU
buffers are read back.

Kraken supports the same four bar aggregations. Its public `trade` channel supplies
execution prices and quantities; the adapter converts taker side to maker side and
publishes `TradedVolumeEvent` through the native book publisher. Trade IDs prevent
duplicate counting across reconnects. Book changes still use native quantity
updates and checksum validation; they never invent executions or reduce liquidity
a second time. Kraken volume sizes use whole base-asset units (default 1), not raw
decimal atoms. Live time bars close with the next trade in a later interval;
book-channel timestamps and wall-clock heartbeats cannot prematurely close them.

`PriceChangeEvent` (also `BestPriceMessageEvent`) is emitted by the sided store
when its best price changes, including transitions to/from an empty side. It
contains that side's new best price, visible quantity and order count; a quantity
change at the same price does not emit it. `TradedVolumeEvent` contains executed
quantity, execution price and maker side. Native fills emit it per executed price
without requiring optional reports; simulated fills do not emit. ITCH Execute
uses `modify_with_fill_event`; an ITCH C record uses its explicit execution price.

The message-producing sink is `OhlcBars::new(aggregation, destination)`;
`VolumeBars::new(bar_size, destination)` remains a compatible volume constructor.
Targets are `Aggregation::{Volume, Time, Ticks, Notional}(NonZeroU64)`; time uses
nanoseconds. For notional, `set_notional_scale(book, scale)` supplies raw price ×
raw quantity units per quote-currency unit. The web feed configures this from each
instrument's price and quantity decimals; integer feeds default to scale 1.
`set_quantity_scale(book, scale)` similarly expresses volume targets in displayed
quantity units while event quantities remain exact raw atoms. Its
`MessageSink` subtrait declares `(PriceChangeEvent, TradedVolumeEvent)` as required
inputs and `OhlcBar` (also `VolumeBar`) as output. Pass this sink to `ReplayContext::with_sinks`:
the context connects those typed routes automatically, with a null route for raw
price-level messages. A single ordered Tokio queue feeds the transform; its
output destination receives completed `BookEvent<OhlcBar>` messages immediately.
`BookEvent::timestamp_ns()` carries source time. ITCH passes timestamps through
`Book::storage_and_publisher_at(timestamp_ns)`; sources without timestamps retain
zero. Time bars require ordered source timestamps. They close on the next trade
in a later bucket, or when `advance_time(watermark_ns)` advances the source clock;
the browser calls this after replay advances, including for inactive symbols.
OHLC output includes `ticks`, `start_ns`, and `end_ns`; time bars use bucket bounds,
while other bars use first/last execution time. Unfinished bars remain previews
when a sink finishes.
Use `BatchedDestination { batcher: OhlcArrowBatcher::new(batch_size),
destination: feather_or_gpu_sink }` to batch those messages into Arrow. Output
sequence numbers identify the closing source event; `bar_index` distinguishes
multiple bars completed by one execution. The browser connects the same transform
inline to its GPU staging destination.

`Publishers<Levels, Prices, Trades>` selects each route statically, defaulting to
`NullPublisher`. Its lazy observers discard unused best-price scans, event
construction, stamping and sends. Existing level-only sinks leave the new routes
disabled. Native mutation code uses `MutationPublisher`; a legacy level callback
still works and has no observers for the other message types.

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

An L3 limit simulation copies the selected native book, including its arena and
FIFO links. It previews with `SimulatedFills`, applies those fills through
`modify_with_fill_event`, and adds any remainder through `add_order` with the
source timestamp. The initial fills and subsequent maker fills appear in the
simulated execution table. The original books, bars and GPU histories continue
separately. **Return to main timeline** selects their current view.

Same-price replay executions match native FIFO. Executions on your resting side
at a strictly worse price fill your order directly at its limit, capped to its
remaining quantity. Live L3 branches use FIFO without this redirect. Adds,
cancels, replacements and deletes use the native adapter APIs; missing branch
references are skipped. Main-timeline validation remains strict. A fully filled
branch stops, including its chart clock/history, until you return manually. An
unfilled remainder at replay EOF stays unfilled. No L3 live adapter is currently
bundled; the shared live policy is covered by native tests.

The **Level queue** below the fills highlights your resting order in coral.
Segment widths represent quantities; hover to inspect IDs, prices and source
arrival timestamps. **Follow my order** selects its exact price. On any L3 heatmap or depth
view, hover to highlight a price band, then click to inspect all constituent levels in that displayed
aggregation range. These orders are sorted by arrival time; matching still uses
price priority across distinct levels. Replacements receive their new source
arrival timestamp. Scroll the depth chart to reach other prices.

Queue snapshots come from `SidedOrderStore::queue_view` through the storage API at
the existing UI refresh cadence, only while the queue is open. This adds no new
publication work to mutation loops and needs no worker thread. Its canvas uses
native metadata; neither it nor the GPU orderbook chart reads pixels back.
`MarketDataAdapter` owns the shared simulation API; execution policies are chosen
at the boundary. `SimulatedExecutionEvent` stays separate from real trade events.

## Data path

`File / HTTP chunks / WebSocket text → MarketDataAdapter → ReplayContext's native
Book → native PriceLevelChangeEvent → frame batch → Arrow → GpuCanvasSink`.

`available_adapters()` supplies source capabilities and endpoints to the web app.
The transport forwards raw text bytes; Rust handles protocol decoding, subscription
commands, snapshots and checksums. The web consumer uses the shared adapter trait.
ITCH uses the existing per-order mutations. Kraken's decoded `KrakenBookUpdate`
implements the same `AdaptForReplay` trait and uses the native sided store's
`set_level_quantity`, which delegates to `PriceLevelContract::update_quantities`.
No adapter-owned orderbook or price-level implementation exists.

Kraken supplies aggregate liquidity without individual orders. Its native levels
have quantities but no FIFO orders or order counts. `visible_price_levels()` reads
those quotes in native side priority; order-based matching/count APIs keep their
existing semantics. Aggregate updates must not be mixed with individual orders
in one book. Snapshot replacement and depth eviction use
`retain_level_quantities`, publishing zero through the same native level mutation.

Each book stamps its own events. The context stages a complete Kraken message,
validates CRC32 over the native top ten asks then bids, and releases its events.
On failure, native levels are cleared before any invalid quantities reach the
frame batch, and a new snapshot is requested. Only original decimal widths are
retained as adapter metadata; prices and quantities for CRC come from native
storage. Exact JSON decimal parsing preserves trailing zeros, following
[Kraken's checksum rules](https://docs.kraken.com/exchange/guides/websockets/book-checksum-v2).
The live controller reports successful validations and checksum-triggered resubscriptions.

The async WASM `FeedSession.create(canvas, adapterId, ticker, startMs)` receives the actual
HTML canvas. `append(bytes, eof)`, `advance(elapsedMs, recordBudget)`, and `render`
drive incremental replay; `tickers()` lists directory symbols and
`select_ticker(symbol)` switches the view. `ticker_count` lets the UI refresh
suggestions only when the directory grows; `active_books` counts books with
mutations. `source_clock_ms` reports the main feed clock for playback pacing even
when `clock_ms` follows a frozen simulation. `free()` releases the session. The web app requests
1 MiB chunks, with an 8 MiB WASM input limit, and never loads a full day's file
into memory. Stock-locate routing sends each mutation to its persistent native
book. The directory comes from the same StockDirectory records used by the
filesystem `ItchReplaySource::from_file_tickers` API; browser/local-file streams
build it incrementally, without a separate file scan.

`advance_without_render(elapsedMs, recordBudget)` advances the same adapter and
compacts deferred presentation outputs after each batch. Deleted prices that
never reached the GPU are discarded; clears for existing GPU slots are retained.
Completed candles are bounded to a 256-entry tail per instrument and timeline.
The next `render` uploads those tails and the latest level changes, clearing
skipped raster columns on the GPU. The app schedules undrawn work through
`MessageChannel` in cooperative slices rather than waiting for animation frames.
During reconstruction it passes a zero elapsed offset, so the adapter stops at
its configured start time before drawing or beginning paced playback.

`/api/nasdaq-sessions` caches only the directory for five minutes and accepts
bounded byte ranges for listed ITCH 5.0 files. It rejects an upstream response
that ignores Range, forwards cancellation, and never creates a local session
file. Gzip decompression uses the browser's `DecompressionStream`, including its
CRC/trailer validation. Decoder input is driven by replay reads, with at most one
64 KiB write pending; buffered output is drained before more input is written.
This preserves backpressure even when the runtime's decoder buffers eagerly.
Transport progress uses compressed bytes received;
decoded/processed bytes are reported separately because gzip's trailer size wraps
at 4 GiB. Hosted sources keep up to eight 1 MiB ranges in flight or buffered,
refilling only as replay consumes them. Responses are decoded in file order even
when downloads finish out of order. This overlaps network latency with scoped
book reconstruction; pausing stops further refills, and restarting or changing
scope cancels the old requests. Local files retain on-demand reads through the
same decoder. Both paths allocate books only for the selected scope. The single
Nasdaq gzip stream still needs every compressed byte up to the replay position,
so a slow connection can still limit reconstruction regardless of scope.

Within a display frame, updates are coalesced to the final state for each
symbol/price/side, preserving each native book’s event count. The Arrow batch has
an additional 16-byte `gpu_route` column (little-endian price-slot/book indices,
normalized display price, padding). Native quantities and prices retain their
exact integer units; display prices are normalized at the GPU upload boundary.
Compute shaders update a shared price table and build 64 visible price bins plus
a 256-column history ring for **every active book**, including unseen symbols.
The existing capture pass also computes cumulative depth once per book.
A single render pipeline displays the selected book’s heatmap, cumulative depth,
and volume history into the canvas. Histories are stored in pages of 128 books;
selection changes the bound page/index without recomputation or readback.
Rendering is capped at 30 Hz. History is sampled at `window / 256` intervals;
missed intervals use the next available frame's state.

**No chart pixels, textures, or GPU buffers are read back to the CPU.** There is no
`MAP_READ`, `mapAsync`, `getImageData`, or pixel-copy rendering path. A separate
Canvas 2D layer rasterizes labels and crosshairs from clock/camera metadata.
DOM quote/counter labels use the CPU book's metadata, not GPU readback.
`chart_layout()` exports the same geometry used to compile the display shader.
`price_band_at()` and queue selection share native price-bin and side calculation,
including bins containing both quotes. The WebGPU palette follows the design
system and accounts for the canvas surface's color space.

To retain the whole market’s history, raster quantities use 16-bit logarithmic
encoding per side (approximately 0.07% relative rounding error for quantities
above zero). Current depth, volume totals, quotes and native quantities are not
compressed. Each active book uses 68 KiB of history (including per-column price
coordinates), allocated in pages; 9,000 books allocate about 604 MiB for history.
Shared buffers grow by GPU-to-GPU copies.
The browser reports an error above 16,384 active books or 8,388,608 distinct
symbol/price pairs (128 MiB shared price table), rather than evicting histories.
Device memory limits can be reached earlier. Native memory scales with all
books’ resting orders; all-market replay needs more resources than one ticker.
The default minimum price range is 0.5% of each book's centered price. Cameras
widen independently for all active books, including unseen tickers; returning to
a ticker presents its existing history and range. A stopped simulation uses its
own final quotes and retains its camera while the main feed continues.

## Build and verify

### Publish to Vercel

Production: **https://lobo-demo.vercel.app**.

Use Node.js 24. From a full checkout of this repository:

```sh
cd web
nvm use                              # if you manage Node.js with nvm
npm ci
npm run vercel -- login              # once per machine
npm run vercel -- link --project lobo-demo  # once per checkout
npm run deploy
```

`npm run deploy` uploads the web and Rust sources as a compressed archive, builds
WASM and Next.js on Vercel, and publishes the result to production. The root
`.vercelignore` excludes datasets, credentials, and local build artifacts. The CLI
prints the public HTTPS URL. Subsequent updates use the same command. The project
link stays in the repository root's ignored `.vercel/` directory. The CLI wrapper
runs from that root so Vercel can resolve the project's `web` root directory
and the adjacent Rust workspace. The Vercel
CLI version is pinned in `package.json`.

The GitHub repository is also connected. Vercel's project root directory is
`web`, with **Include source files outside of the Root Directory** enabled so
the Rust workspace is available. Git deployments run `npm run build:vercel`,
which installs a Rustup toolchain with the WASM target and caches it alongside
Cargo artifacts in `.next/cache`, then runs the existing build. Pushing to
`master` deploys production; other branches create previews.

`npm run deploy` publishes the current checkout directly, including changes
that have not been committed. The public site includes the
Next.js API routes used for Nasdaq sessions and exchange directories. Kraken and
Bitfinex WebSockets connect directly from each visitor's browser.

The repository's large replay data file is not deployed. Use **Nasdaq sample
sessions** or **Open replay file** on the public site; **Repository file** is
available only when hosting beside that data file. Opening a replay file reads
it in the browser without uploading it. Visitors need a WebGPU-capable browser.

### Local checks

```sh
make web-build                       # WASM + production Next.js build
npm --prefix web start               # serve that production build
cargo test -p lobo_wasm             # shader validation, from repository root
cargo test -p lobo_adapters --features itchy,kraken --test kraken --test market_stream
cargo test -p lobo_storage --test aggregate_levels
cargo test -p lobo_adapters --features itchy --test volume_events
cargo test -p lobo_batchers --features feather --test volume_bars
npm --prefix web run typecheck
npm --prefix web test
```

Generated bindings live under `web/public/wasm/` and are ignored by Git. The
WebGPU dependency remains optional on `lobo_batchers`, enabled by this crate.
Only the configured repository file is exposed by the local API; request
parameters cannot select arbitrary filesystem paths. The dev/start scripts bind
to localhost. The standalone viewer does not require authentication.

### Bitfinex R0 · live L3

Run `make web` and select **Bitfinex R0** in the source header. The ticker box
loads the exchange's public spot-pair directory (for example `BTCUSD`, `ETHUSD`
and `AAVE:USD`). Selected pairs stay subscribed when you visit another pair;
the adapter assigns up to 15 pairs to each socket, using two channels per pair
within Bitfinex's 30-channel connection limit. No credentials are required.

The `bitfinex` adapter feature is enabled by `lobo_wasm`. It loads the R0
snapshot into the existing native L3 book, then adapts additions, absolute
quantity changes, price changes and removals through the native mutation API.
Checksums query that same book: the first 25 bids and asks sorted by price and
numeric order ID, alternating ID and signed amount, with signed CRC32. The
adapter requests server timestamps and connection-wide sequence numbers.
Sequence gaps, malformed messages, maintenance and checksum failures invalidate
the affected books and reconnect for fresh snapshots. An active simulation
stops with its report intact; use **Return to main timeline** to resume viewing
market data.

All OHLC aggregation modes use actual public trade prices and quantities.
Historical trade snapshots and duplicate `te`/`tu` notifications do not create
new bars. R0 reductions and cancellations never create traded-volume events.
The main book is updated exclusively by R0, so trades do not reduce it twice.

Choose **Simulate order → Limit** for a separate native FIFO timeline, or
**Market** for a nonmutating fill preview. Click a depth band to inspect its
native order queue; resting simulated orders are highlighted. Completed limit
scenarios freeze for review while the real feed continues.

This is an **estimated public-feed simulation**: R0 exposes up to 250 visible
orders per side, its snapshot omits original creation timestamps, and public
trades omit maker order IDs. Snapshot FIFO is seeded in numeric ID order;
subsequent orders carry the server time when they were observed. The simulator
briefly buffers branch book operations (250 ms) to reconcile asynchronous book
reductions and trade notifications. Actual trades and newly resting orders (including price amendments) use the
native live matching API, with no replay-only execution clamping. A resting
order that crosses the simulated limit can therefore fill it even when it
remains passive on the real exchange. Corrections do not restore
makers already consumed in the scenario. Channel skew beyond that interval,
hidden liquidity, unknown amendments to time priority and orders outside the
published window limit the precision of estimated fills. The main feed streams
immediately after its initial snapshot checksum passes.

The web client remains adapter-driven. `MarketDataAdapter` supplies bootstrap
requests, socket assignments, commands and wire decoding. The small
`/api/market-data` transport proxy only forwards approved public directory URLs
because this endpoint does not provide browser CORS headers; it never parses
orders or trades. GPU charts continue to render directly into the canvas with
no GPU-to-CPU readback.

Protocol references: [raw books](https://docs.bitfinex.com/reference/ws-public-raw-books),
[trades](https://docs.bitfinex.com/reference/ws-public-trades),
[checksums](https://docs.bitfinex.com/docs/ws-websocket-checksum),
[configuration and connection limits](https://docs.bitfinex.com/docs/ws-general),
[decimal precision](https://docs.bitfinex.com/docs/introduction).

## Python-hosted books

`make py` packages this app for the Rust server. Python users can open
`with server_context(...)` and create `Book(...)` objects inside it; the printed
server URL exposes those books in live L3 mode. The source switcher and scope
selector are hidden in that deployment. All charts, queues, and simulations use
the existing adapter interface. See the [server API guide](../rust/crates/lobo_server/README.md).
