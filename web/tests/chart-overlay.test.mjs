import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import test from "node:test";
import ts from "typescript";

function loadModule(name, dependencies = {}) {
  const exports = {};
  vm.runInNewContext(
    ts.transpileModule(
      readFileSync(new URL(`../lib/${name}.ts`, import.meta.url), "utf8"),
      {
        compilerOptions: {
          module: ts.ModuleKind.CommonJS,
          target: ts.ScriptTarget.ES2020,
        },
      },
    ).outputText,
    { exports, Float64Array, require: (name) => dependencies[name] },
  );
  return exports;
}

const { drawChartOverlay } = loadModule("chart-overlay", {
  "./depth-profile": loadModule("depth-profile"),
});
const layout = {
  bins: 64,
  heatmapWidth: 0.74,
  priceTop: 0.025,
  priceBottom: 0.875,
  volumeTop: 0.925,
  volumeBottom: 0.985,
  depthFull: 0.08,
  depthZero: 0.92,
};
const width = 1200,
  height = 800;
const bucketY = (bin) =>
  layout.priceTop * height +
  (1 - (bin + 0.5) / layout.bins) *
    (layout.priceBottom * height - layout.priceTop * height);

function draw({ levels, bid = 99, ask = 101, hover = null }) {
  const quantities = new Float64Array(layout.bins * 2);
  for (const [bin, bidQuantity, askQuantity] of levels) {
    quantities[bin * 2] = bidQuantity;
    quantities[bin * 2 + 1] = askQuantity;
  }
  const labels = [];
  const context = {
    canvas: { width, height },
    clearRect() {},
    beginPath() {},
    moveTo() {},
    lineTo() {},
    stroke() {},
    fillRect() {},
    strokeRect() {},
    setLineDash() {},
    measureText: (text) => ({ width: text.length * 6 }),
    fillText: (text, x, y) => labels.push({ text, x, y }),
  };
  drawChartOverlay(
    context,
    {
      bid,
      ask,
      center: 100,
      span: 64,
      price_decimals: 2,
      quantity_decimals: 0,
      clock_ms: 34_800_000,
      depth_quantities: () => quantities,
      price_band_at: () => JSON.stringify(hover),
    },
    layout,
    { volumeView: false, aggregation: "volume", windowSeconds: 60 },
    hover ? { x: 0.9, y: bucketY(hover.bin) / height } : null,
    1,
    {
      font: "monospace",
      muted: "gray",
      line: "gray",
      panel: "black",
      bid: "green",
      ask: "red",
      primary: "yellow",
      text: "white",
    },
  );
  return {
    labels,
    at: (bin) =>
      labels.find(({ x, y }) => x === width - 6 && y === bucketY(bin))?.text,
  };
}

test("depth labels follow cumulative widths through small and empty buckets", () => {
  const { labels, at } = draw({
    levels: [
      [28, 240, 0],
      [20, 3000, 0],
      [12, 9, 0],
      [36, 0, 1269],
      [44, 0, 3059],
      [60, 0, 300],
    ],
    hover: { bin: 12, side: "buy", lower: 80, upper: 81 },
  });
  assert.deepEqual([28, 20, 12, 4, 36, 44, 52, 60].map(at), [
    "240",
    "3.24K",
    "3.249K",
    "3.249K",
    "1.269K",
    "4.328K",
    "4.328K",
    "4.628K",
  ]);
  assert.ok(labels.some(({ text }) => text === "CUMULATIVE QTY"));
  assert.ok(labels.some(({ text }) => text === "Bucket 9 · Cumulative 3,249"));
});

test("mixed buckets label the displayed side instead of combining both sides", () => {
  const levels = [
    [28, 240, 1000],
    [36, 3000, 9],
  ];
  const twoSided = draw({ levels });
  assert.equal(twoSided.at(28), "3.24K");
  assert.equal(twoSided.at(36), "1.009K");

  // The midpoint can split a bucket: match the side at the label's grid line.
  const splitBucket = draw({ levels, bid: 104, ask: 106 });
  assert.equal(splitBucket.at(36), "3K");
});

test("one-sided books keep the same side as the GPU across the full price range", () => {
  const bids = draw({ bid: 108, ask: 0, levels: [[36, 240, 0]] });
  assert.equal(bids.at(36), "240");
  assert.equal(bids.at(28), "240");

  const asks = draw({ bid: 0, ask: 92, levels: [[28, 0, 240]] });
  assert.equal(asks.at(28), "240");
  assert.equal(asks.at(36), "240");

  assert.equal(draw({ bid: 0, ask: 0, levels: [] }).at(36), "0");
});
