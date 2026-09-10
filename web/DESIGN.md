# Lobo terminal design system

Visual references come from **HaX0R Terminal Design System**, Stitch project
`11296489504784899907`:

- **Shader**: `d712b4c232f042ee9a108207faed5ab3`.
- **Book Scope Multi-Ticker Selector**: `07295b58dd064aa3a595a2c02c3ca65d`.
- **iTerm Theme Mapped Engine**: `eeb030354ee54abf9e916840e04061b4`.
- **Nordic Violet Terminal**, source toggle group: `5ccbfb3dff824bd8b5800b03e26407d1`.

These references guide appearance. Preserve existing sources, speeds,
simulations, queue inspection, chart tabs, aggregation and price scrolling.
The scope selector, theme picker, loading animation and bucket quantities are
explicit requested additions; do not infer other controls or data from mockups.

## Terminal palette

The selector lists the complete upstream WezTerm-format directory of
[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes).
Catalog and selected TOML files load directly from GitHub; do not vendor or serve
theme files. `lib/terminal-themes.mjs` validates colors and maps them into the
semantic tokens declared in `app/design-tokens.css`.

| Terminal role               | Dashboard use                                |
| --------------------------- | -------------------------------------------- |
| Background / foreground     | Workspace, text, panels, separators, shadows |
| ANSI green / bright green   | Bids, playback, bid gradients and highlights |
| ANSI red / bright red       | Asks, ask gradients and highlights           |
| ANSI yellow / bright yellow | Primary actions and loading glyph gradients  |
| ANSI magenta                | Simulated orders and upper background glow   |
| Selection                   | Text selection                               |

Panel shades and separators mix background and foreground. Chart low/base/high
ramps derive from the side's ANSI color, background, and bright color. Text roles
adjust toward the theme foreground where needed for contrast. No independent
chart palette or component hex colors. Keep simulated orders visually distinct.

The default theme is **Acid Lime**. The optional **System** selection follows OS
changes: Espresso for dark mode, GitHub Light Default for light. A named selection is remembered locally and
stays selected across OS changes. CSS provides a usable initial/offline palette;
a failed upstream request keeps the last palette and offers Retry.

## Typography and shape

- Dark UI: **Inter**. Light UI: **Hanken Grotesk**.
- Numerical readouts, timestamps, labels, axes and tables: **JetBrains Mono**,
  with tabular figures. Font assets and OFL licenses are hosted locally.
- Controls and panels: **4px** radius. Floating overlays: **6px**. Scope dialog:
  **8px**. Keep chart geometry precise rather than smoothing order quantities.
- Fine **1px** separators; restrained shadows on floating elements.
- Compact 4px spacing increments, 8px gutters, and 10px panel padding. The chart
  gets the available screen area; responsive stacking and scrolling remain.
- Source choices share one bordered toggle group, with one pressed selection,
  a raised selected surface, and a green Active badge. Keep replay file controls
  alongside it; server sessions show only their configured adapters.
- Queue prices, counts, quantities, order IDs, and timestamps use the primary
  accent color and semibold tabular figures, while surrounding labels stay muted.
- GPU acceleration displays the configured device's `wgpu::Backend` variant,
  exported through the WASM session. Metal shows Metal, Vulkan shows Vulkan, and
  BrowserWebGpu shows WebGPU. Browser hardware descriptions never replace the
  reported enum. Missing, lost, software, or Noop devices show Not available in red.

## Charts and loading

Depth areas, quantity history, candles, FIFO segments and canvas backgrounds
use gradients derived from the selected theme. Empty liquidity stays empty;
never add decorative curves or synthetic market data.

`lib/chart-theme.ts` packs thirteen RGBA vectors into the shared WGSL `Theme`:
canvas, panel, grid, bid, ask, bid-low, ask-low, bid-high, ask-high,
background-top, background-bottom, primary, loading-accent. Palette uploads do
not clear history, restart replay, reaggregate books, or read GPU memory.
Raster labels and frozen FIFO views follow the same tokens.

Depth's right edge shows cumulative quantities at eight uniformly spaced
bucket centers, using the same bid/ask split as the shaded depth. Hover shows
both bucket quantity and cumulative depth for the selected side; clicking still
opens the corresponding L3 FIFO queue. Cumulative depth extends left from the
right-hand zero baseline: more depth means more shaded width at a fixed scale.
Label the horizontal quantity scale; it follows the larger visible side total. Quantity labels
use native levels in the same coordinates as GPU aggregation, without readback.

During reconstruction, market computation and axes stay paused. A separate
WebGPU pass draws drifting pixel-font tickers over the themed canvas gradient.
Use only discovered scope tickers, up to 100; sample across larger directories.
Fit text to the viewport, capped at **56 CSS px**, including one-ticker scopes.
Upload glyph instances only when the ticker list or viewport changes. The
animation runs at roughly 12 fps and continues during awaited input; replay
reconstruction remains unpaced in bounded work slices. Show source-time progress.

## Book scope

Place scope beside the data source. The modal spans most of the screen, with a
search field, category shortcuts, selected chips, a scrollable checkbox grid,
and explicit Cancel / Apply actions. It shares the viewing ticker's matching
and ranking. Categories intersect the actual feed directory; show their source
and date rather than suggesting historical index membership.

Hosted Nasdaq sessions start with AAPL only. Local and repository replay files
default to All. The Tech preset remains available in the selector. Replay scope
determines native routing before books are allocated. Applying scope restarts at
the configured Start at time. Live All
retains subscribe-as-visited behavior; explicit selections keep all chosen pairs
subscribed. The full directory stays available to expand scope later.
