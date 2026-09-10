export interface AdapterInfo {
  id: string;
  name: string;
  mode: "replay" | "live";
  endpoint: string | null;
  defaultSymbol: string;
  timezone: string;
  supportsTrades: boolean;
  level: "l1" | "l2" | "l3";
}
export interface SimulationStatus {
  simulated: true;
  stoppedReason: string | null;
  orderId: string;
  symbol: string;
  side: "buy" | "sell";
  price: number | null;
  kind: "market" | "limit";
  alternateTimeline: boolean;
  averagePrice: number | null;
  startedMs: number;
  requested: number;
  filled: number;
  remaining: number;
  complete: boolean;
  ignored: number;
  executionCount: number;
  executions: {
    type: "simulated_execution";
    simulated: true;
    timeMs: number;
    price: number;
    quantity: number;
    maker: boolean;
    sequence: number;
  }[];
}
export interface QueueStatus {
  symbol: string;
  side: "buy" | "sell";
  lower: number;
  upper: number;
  timestampMs: number;
  ordersAhead: number | null;
  quantityAhead: number | null;
  totalQuantity: number;
  orders: {
    id: string;
    price: number;
    quantity: number;
    timestampNs: string | null;
    mine: boolean;
  }[];
}
export interface ChartLayout {
  depthFull: number;
  depthZero: number;
  bins: number;
  heatmapWidth: number;
  priceTop: number;
  priceBottom: number;
  volumeTop: number;
  volumeBottom: number;
}
export interface PriceBand {
  bin: number;
  side: "buy" | "sell";
  lower: number;
  upper: number;
}
export interface Session {
  readonly gpu_backend: string;
  depth_quantities(): Float64Array;
  scoped_tickers(): string[];
  set_loading_tickers(tickers: string[]): void;
  render_loading(seconds: number, pixelRatio: number): void;
  bootstrap_requests(): string;
  bootstrap(id: string, bytes: Uint8Array): void;
  connections(): string;
  socket_receive(id: number, bytes: Uint8Array): void;
  socket_connected(id: number): void;
  socket_disconnected(id: number): void;
  socket_keepalive(id: number): void;
  socket_commands(id: number): string[];
  simulation_note(): string | undefined;
  price_band_at(x: number, y: number): string;
  simulate_market(side: string, quantity: number): void;
  select_queue_at(x: number, y: number): boolean;
  follow_simulated_order(): void;
  clear_queue(): void;
  queue_status(): string;
  readonly quantity_decimals: number;
  simulate(side: string, price: number, quantity: number): void;
  simulation_status(): string;
  return_to_main(): void;
  append(bytes: Uint8Array, eof: boolean): void;
  set_observer_metrics(bits: number): void;
  advance(elapsedMs: number, budget: number): void;
  advance_without_render(elapsedMs: number, budget: number): void;
  render(windowSeconds: number, priceSpan: number): void;
  recenter(): void;
  show_volume_bars(enabled: boolean): void;
  set_chart_theme(colors: Float32Array): void;
  set_volume_bar_size(size: number): void;
  set_bar_aggregation(kind: string, size: number): void;
  pan_price(fraction: number): void;
  bar_timestamps(): Float64Array;
  readonly forming_progress: number;
  readonly volume_bar_count: number;
  readonly forming_volume: number;
  connected(): void;
  disconnected(): void;
  commands(): string[];
  keepalive(): void;
  readonly checksum_checks: number;
  readonly checksum_failures: number;
  readonly price_decimals: number;
  readonly span: number;
  tickers(): string[];
  select_ticker(ticker: string): void;
  readonly ticker_count: number;
  readonly active_books: number;
  free(): void;
  readonly center: number;
  readonly buffered_bytes: number;
  readonly needs_input: boolean;
  readonly complete: boolean;
  readonly warming: boolean;
  readonly clock_ms: number;
  readonly source_clock_ms: number;
  readonly start_ms: number;
  readonly bytes_consumed: number;
  readonly messages: number;
  readonly updates: number;
  readonly bid: number;
  readonly ask: number;
}
interface WasmModule {
  default(): Promise<unknown>;
  available_adapters(): string;
  chart_layout(): string;
  FeedSession: {
    create_observed_scoped(
      canvas: HTMLCanvasElement,
      descriptorJson: string,
      ticker: string,
      scopeJson: string,
    ): Promise<Session>;
    create_observed(
      canvas: HTMLCanvasElement,
      descriptorJson: string,
      ticker: string,
    ): Promise<Session>;
    create_scoped(
      canvas: HTMLCanvasElement,
      adapterId: string,
      ticker: string,
      startMs: number,
      scopeJson: string,
    ): Promise<Session>;
    create(
      canvas: HTMLCanvasElement,
      adapterId: string,
      ticker: string,
      startMs: number,
    ): Promise<Session>;
  };
}
let promise: Promise<WasmModule> | undefined;
export function loadWasm(): Promise<WasmModule> {
  promise ??= (async () => {
    const url = "/wasm/lobo_wasm.js";
    const module = (await import(
      /* webpackIgnore: true */ /* turbopackIgnore: true */ url
    )) as WasmModule;
    await module.default();
    return module;
  })().catch((error) => {
    promise = undefined;
    throw error;
  });
  return promise;
}
