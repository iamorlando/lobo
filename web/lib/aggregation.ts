export const aggregations = {
  volume: {
    name: "Volume",
    label: "Quantity / bar",
    sizes: [1000, 5000, 10000, 50000],
    initial: 5000,
    unit: "qty",
    hint: "Executions are split at exact quantity boundaries.",
  },
  time: {
    name: "Time",
    label: "Interval",
    sizes: [1, 5, 15, 30, 60, 300],
    initial: 5,
    unit: "s",
    hint: "Exchange-time intervals; periods without executions have no candle.",
  },
  ticks: {
    name: "Ticks",
    label: "Executions / bar",
    sizes: [10, 50, 100, 500, 1000],
    initial: 100,
    unit: "ticks",
    hint: "Each execution message counts once, regardless of quantity.",
  },
  notional: {
    name: "Notional",
    label: "Quote value / bar",
    sizes: [25000, 100000, 500000, 1000000, 5000000],
    initial: 100000,
    unit: "quote",
    hint: "Execution quantity × execution price. The closing execution can exceed the target.",
  },
} as const;
export type Aggregation = keyof typeof aggregations;
export function sizeLabel(kind: Aggregation, size: number) {
  if (kind === "time") return size < 60 ? `${size} sec` : `${size / 60} min`;
  return new Intl.NumberFormat("en-US", {
    notation: size >= 1000000 ? "compact" : "standard",
  }).format(size);
}
