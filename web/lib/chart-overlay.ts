import type { ChartLayout, PriceBand, Session } from "./wasm";
import type { chartTheme } from "./chart-theme";
import { depthProfile } from "./depth-profile";

const clock = (ms: number) =>
  new Date(Math.max(0, ms)).toISOString().slice(11, 19);
const count = new Intl.NumberFormat("en-US");

/** Labels and hit highlights use native metadata; this canvas never reads the GPU. */
export function drawChartOverlay(
  context: CanvasRenderingContext2D,
  native: Session,
  layout: ChartLayout,
  controls: {
    volumeView: boolean;
    aggregation: string;
    windowSeconds: number;
    hiddenDepth?: boolean;
  },
  mouse: { x: number; y: number } | null,
  dpr: number,
  theme: ReturnType<typeof chartTheme>,
): boolean {
  const w = context.canvas.width,
    h = context.canvas.height;
  const font = Math.min(10, Math.max(8, w / dpr / 110)) * dpr;
  const decimals = Math.min(native.price_decimals, 9);
  context.clearRect(0, 0, w, h);
  context.font = `${font}px ${theme.font}`;
  context.fillStyle = theme.muted;
  context.textBaseline = "middle";
  context.lineWidth = dpr;
  if (controls.volumeView) {
    context.textAlign = "right";
    for (let i = 0; i <= 8; i++) {
      context.fillText(
        (native.center + native.span * (0.5 - i / 8)).toFixed(decimals),
        w - 6 * dpr,
        Math.max(font, (0.76 * h * i) / 8),
      );
    }
    context.textAlign = "left";
    context.fillText("EXECUTED QUANTITY", 6 * dpr, 0.81 * h);
    const total = native.volume_bar_count + (native.forming_volume > 0 ? 1 : 0);
    const timestamps =
      controls.aggregation === "time" ? native.bar_timestamps() : null;
    const first = Math.max(0, total - 80),
      visible = Math.min(80, total);
    const step = Math.max(
      1,
      Math.ceil(visible / Math.max(1, Math.floor(w / dpr / 120))),
    );
    for (let i = 0; i < visible; i += step) {
      context.fillText(
        timestamps && i < timestamps.length
          ? clock(timestamps[i])
          : `#${count.format(first + i + 1)}`,
        ((i + 0.2) / Math.max(visible, 20)) * w * 0.92,
        0.79 * h,
      );
    }
    return false;
  }
  const heatmapWidth = layout.heatmapWidth * w;
  const top = layout.priceTop * h,
    bottom = layout.priceBottom * h;
  const priceHeight = bottom - top;
  const quantities = native.depth_quantities();
  const depth = depthProfile(quantities, native.quantity_decimals);
  const quantity = new Intl.NumberFormat("en-US", {
    maximumFractionDigits: Math.min(native.quantity_decimals, 9),
    notation: "compact",
    maximumSignificantDigits: 4,
  });
  const exactQuantity = new Intl.NumberFormat("en-US", {
    maximumFractionDigits: Math.min(native.quantity_decimals, 18),
  });
  // Match the GPU's cumulative depth and quote-midpoint split, including bins
  // containing both sides. Labels describe the shaded width at their price.
  const midpoint =
    native.bid > 0 && native.ask > 0
      ? (native.bid + native.ask) / 2
      : native.bid > 0
        ? Infinity
        : native.ask > 0
          ? -Infinity
          : 0;
  const priceSplit = Math.fround(
    (midpoint - native.center) / native.span + 0.5,
  );
  context.textAlign = "right";
  for (let bin = 4; bin < layout.bins; bin += 8) {
    const y = top + (1 - (bin + 0.5) / layout.bins) * priceHeight;
    context.strokeStyle = theme.line;
    context.globalAlpha = 0.45;
    context.beginPath();
    context.moveTo(heatmapWidth, y);
    context.lineTo(w, y);
    context.stroke();
    context.globalAlpha = 1;
    const side = Number((bin + 0.5) / layout.bins >= priceSplit);
    const label = quantity.format(depth.cumulative[bin * 2 + side]);
    context.fillStyle = theme.panel;
    context.fillRect(
      w - context.measureText(label).width - 10 * dpr,
      y - font * 0.8,
      context.measureText(label).width + 10 * dpr,
      font * 1.6,
    );
    context.fillStyle = theme.muted;
    context.fillText(label, w - 6 * dpr, y);
  }
  context.fillText("CUMULATIVE QTY", w - 6 * dpr, Math.max(font, top - font));
  const priceY = (price: number) =>
    top + (0.5 - (price - native.center) / native.span) * priceHeight;
  const quotes = [
    { price: native.bid, name: "BID", color: theme.bid },
    { price: native.ask, name: "ASK", color: theme.ask },
  ]
    .filter((quote) => quote.price > 0)
    .map((quote) => ({ ...quote, y: priceY(quote.price) }))
    .filter((quote) => quote.y >= top && quote.y < bottom);
  context.textAlign = "left";
  for (let i = 0; i <= 8; i++) {
    const y = top + (priceHeight * i) / 8;
    if (quotes.some((quote) => Math.abs(quote.y - y) < font * 1.5)) continue;
    context.fillText(
      (native.center + native.span * (0.5 - i / 8)).toFixed(decimals),
      heatmapWidth + 6 * dpr,
      Math.min(bottom - font / 2, Math.max(top + font / 2, y)),
    );
  }
  // Quote guides align exactly with price, including after scrolling/recentering.
  const combined =
    quotes.length === 2 && Math.abs(quotes[0].y - quotes[1].y) < font * 2;
  for (const quote of quotes) {
    context.strokeStyle = quote.color;
    context.setLineDash([2 * dpr, 4 * dpr]);
    context.beginPath();
    context.moveTo(0, quote.y);
    context.lineTo(w, quote.y);
    context.stroke();
  }
  context.setLineDash([]);
  const labels = combined
    ? [
        {
          price: (native.bid + native.ask) / 2,
          name: "MID",
          color: theme.primary,
          y: priceY((native.bid + native.ask) / 2),
        },
      ]
    : quotes;
  for (const label of labels) {
    const scaled = label.price * 10 ** decimals;
    const halfTick =
      label.name === "MID" && Math.abs(scaled - Math.round(scaled)) > 0.1;
    const text = `${label.name} ${label.price.toFixed(decimals + Number(halfTick))}`;
    context.fillStyle = theme.panel;
    context.fillRect(
      heatmapWidth + 2 * dpr,
      label.y - font,
      context.measureText(text).width + 10 * dpr,
      font * 2,
    );
    context.fillStyle = label.color;
    context.fillText(text, heatmapWidth + 6 * dpr, label.y);
  }
  const timeTicks = (width: number, y: number) => {
    const steps = Math.max(1, Math.min(4, Math.floor(width / dpr / 120)));
    context.fillStyle = theme.muted;
    for (let i = 0; i <= steps; i++) {
      context.textAlign = i === 0 ? "left" : i === steps ? "right" : "center";
      context.fillText(
        clock(
          native.clock_ms - controls.windowSeconds * 1000 * (1 - i / steps),
        ),
        Math.max(5 * dpr, Math.min(width - 5 * dpr, (width * i) / steps)),
        y,
      );
    }
  };
  timeTicks(heatmapWidth, bottom + (layout.volumeTop * h - bottom) * 0.36);
  context.textBaseline = "bottom";
  timeTicks(w, h - dpr);
  context.textAlign = "left";
  context.font = `${font * 0.85}px ${theme.font}`;
  context.fillStyle = theme.muted;
  context.fillText(
    "VISIBLE RANGE VOLUME",
    6 * dpr,
    layout.volumeTop * h - 2 * dpr,
  );
  context.textAlign = "right";
  const depthWidth = w - heatmapWidth;
  const axisY = bottom + (layout.volumeTop * h - bottom) * 0.32;
  context.fillStyle = theme.muted;
  const steps = depthWidth < 180 * dpr ? 2 : 4;
  for (let step = 0; step <= steps; step++) {
    const fraction = step / steps;
    context.textAlign =
      step === 0 ? "left" : step === steps ? "right" : "center";
    context.fillText(
      quantity.format(depth.maximum * (1 - fraction)),
      heatmapWidth +
        depthWidth *
          (layout.depthFull + (layout.depthZero - layout.depthFull) * fraction),
      axisY,
    );
  }
  context.textAlign = "right";
  context.fillStyle = theme.primary;
  context.fillText(
    w - heatmapWidth < 180 * dpr
      ? "CUMULATIVE"
      : controls.hiddenDepth
        ? "CUMULATIVE · TOTAL DEPTH"
        : "CUMULATIVE · VISIBLE DEPTH",
    w - 6 * dpr,
    bottom + (layout.volumeTop * h - bottom) * 0.85,
  );

  if (!mouse || mouse.y < layout.priceTop || mouse.y >= layout.priceBottom)
    return false;
  const band = JSON.parse(
    native.price_band_at(mouse.x, mouse.y),
  ) as PriceBand | null;
  if (!band) return false;
  const bandTop = top + (1 - (band.bin + 1) / layout.bins) * priceHeight;
  context.fillStyle = theme.primary;
  context.globalAlpha = 0.1;
  context.fillRect(0, bandTop, w, priceHeight / layout.bins);
  context.globalAlpha = 1;
  context.strokeStyle = theme.primary;
  context.setLineDash([3 * dpr, 4 * dpr]);
  context.beginPath();
  if (mouse.x < layout.heatmapWidth) {
    context.moveTo(mouse.x * w, top);
    context.lineTo(mouse.x * w, bottom);
  }
  context.moveTo(0, mouse.y * h);
  context.lineTo(w, mouse.y * h);
  context.stroke();
  context.setLineDash([]);
  const range =
    band.lower === band.upper
      ? band.lower.toFixed(decimals)
      : `${band.lower.toFixed(decimals)}–${band.upper.toFixed(decimals)}`;
  const hoverLines =
    mouse.x >= layout.heatmapWidth
      ? [
          `${band.side === "buy" ? "BID" : "ASK"} · ${range}`,
          `Bucket ${exactQuantity.format(quantities[band.bin * 2 + Number(band.side === "sell")])} · Cumulative ${exactQuantity.format(depth.cumulative[band.bin * 2 + Number(band.side === "sell")])}`,
        ]
      : [
          `${clock(native.clock_ms - (1 - mouse.x / layout.heatmapWidth) * controls.windowSeconds * 1000)} · ${range}`,
        ];
  const width = Math.min(
    w - 8 * dpr,
    Math.max(...hoverLines.map((line) => context.measureText(line).width)) +
      16 * dpr,
  );
  const lineHeight = font * 1.7;
  const height = lineHeight * hoverLines.length + 12 * dpr;
  const x = Math.max(
    4 * dpr,
    Math.min(mouse.x * w + 12 * dpr, w - width - 4 * dpr),
  );
  const y = Math.max(top + height + 4 * dpr, mouse.y * h - 10 * dpr);
  context.fillStyle = theme.panel;
  context.fillRect(x, y - height, width, height);
  context.strokeRect(x, y - height, width, height);
  context.fillStyle = theme.text;
  context.textAlign = "left";
  context.textBaseline = "middle";
  hoverLines.forEach((line, i) =>
    context.fillText(
      line,
      x + 8 * dpr,
      y - height + 6 * dpr + lineHeight * (i + 0.5),
      width - 16 * dpr,
    ),
  );
  return true;
}
