"use client";
import { useEffect, useRef, useState } from "react";
import {
  SimulationPanel,
  SimulationResults,
} from "@/components/simulation-panel";
import { LevelQueue } from "@/components/level-queue";
import { TickerPicker } from "@/components/ticker-picker";
import { BookScopeSelector, type BookScope } from "@/components/book-scope";
import categories from "@/lib/ticker-categories.json";
import { ThemeSelector } from "@/components/theme-provider";
import {
  loadWasm,
  type Session,
  type AdapterInfo,
  type SimulationStatus,
  type QueueStatus,
  type ChartLayout,
} from "@/lib/wasm";
import { openSource, type SourceChoice } from "@/lib/source";
import type { NasdaqSession } from "@/lib/nasdaq-sessions.mjs";
import {
  ReplayClock,
  replaySlice,
  SPEEDS,
  speedLabel,
  type ReplaySpeed,
} from "@/lib/playback.mjs";
import { aggregations, sizeLabel, type Aggregation } from "@/lib/aggregation";
import { bootstrapFeed, connectSocket } from "@/lib/live";
import { chartTheme } from "@/lib/chart-theme";
import { observeGpuStatus } from "@/lib/gpu-status";
import { drawChartOverlay } from "@/lib/chart-overlay";
import {
  subscribeHostedAdapter,
  type ServerConfiguration,
} from "@/lib/server-context";
import {
  hostedPlayback,
  seekTimestamp,
  sourceMode,
  type PlaybackCommand,
  type PlaybackStatus,
} from "@/lib/hosted-playback.mjs";

const count = new Intl.NumberFormat("en-US");
const price = (value: number, decimals = 2) =>
  value ? value.toFixed(decimals).replace(/(\.\d{2}.*?)0+$/, "$1") : "—";
const clock = (ms: number) =>
  new Date(Math.max(0, ms)).toISOString().slice(11, 19);
const bytes = (value: number) =>
  value > 1024 ** 3
    ? `${(value / 1024 ** 3).toFixed(2)} GB`
    : `${(value / 1024 ** 2).toFixed(1)} MB`;
const initial = {
  status: "Loading",
  name: "Nasdaq sample sessions",
  clock: 34200000,
  bid: 0,
  ask: 0,
  updates: 0,
  messages: 0,
  consumed: 0,
  size: 0,
  received: 0,
  compressed: false,
  center: 189.35,
  activeBooks: 0,
  priceDecimals: 2,
  span: 1.5,
  adapterName: "",
  timezone: "ET",
  checksums: 0,
  checksumFailures: 0,
  supportsTrades: true,
  level: "l3" as AdapterInfo["level"],
  mode: "replay" as AdapterInfo["mode"],
  quantityDecimals: 0,
  volumeBars: 0,
  formingVolume: 0,
  formingProgress: 0,
  connectionDetail: "",
};

function Icon({
  kind,
}: {
  kind: "play" | "pause" | "restart" | "file" | "focus";
}) {
  return (
    <svg
      viewBox="0 0 24 24"
      width="16"
      height="16"
      aria-hidden="true"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {kind === "play" ? (
        <path d="m9 5 11 7-11 7Z" fill="currentColor" stroke="none" />
      ) : kind === "pause" ? (
        <>
          <path d="M8 5v14M16 5v14" strokeWidth="4" />
        </>
      ) : kind === "restart" ? (
        <>
          <path d="M4 10a8 8 0 1 1 1 8M4 4v6h6" />
        </>
      ) : kind === "file" ? (
        <>
          <path d="M14 3H5v18h14V8ZM14 3v6h5M8 13h8M8 17h5" />
        </>
      ) : (
        <>
          <path d="M8 3H3v5m13-5h5v5M3 16v5h5m8 0h5v-5M8 12h8m-4-4v8" />
        </>
      )}
    </svg>
  );
}

export default function ReplayLab({
  server,
}: {
  server?: ServerConfiguration;
}) {
  const [choice, setChoice] = useState<SourceChoice>(
    server
      ? { kind: "live", adapterId: server.adapters?.[0]?.id ?? "server" }
      : {
          kind: "nasdaq",
          session: null,
        },
  );
  const [samples, setSamples] = useState<NasdaqSession[]>([]);
  const [samplesError, setSamplesError] = useState("");
  const [tickers, setTickers] = useState<string[]>([]);
  const [directory, setDirectory] = useState<string[]>([]);
  const [selecting, setSelecting] = useState(false);
  const hostedAdapter =
    choice.kind === "live"
      ? server?.adapters?.find((adapter) => adapter.id === choice.adapterId)
      : undefined;
  const [scope, setScope] = useState<BookScope>(server ? null : categories.tech);
  const [loadedTicker, setLoadedTicker] = useState(
    server
      ? (server.adapters?.[0]?.defaultSymbol ??
          server.books[0]?.symbol ??
          "BOOK")
      : "AAPL",
  );
  const [start, setStart] = useState("09:30:00");
  const [generation, setGeneration] = useState(0);
  const [playing, setPlaying] = useState(
    () => !server?.adapters?.[0]?.playback?.paused,
  );
  const [reconstruction, setReconstruction] = useState({
    clock: 0,
    percent: 0,
    target: start,
  });
  const [speed, setSpeed] = useState<ReplaySpeed>(
    server?.adapters?.[0]?.playback?.speed ?? 5,
  );
  const [windowSeconds, setWindow] = useState(300);
  const [span, setSpan] = useState(0.005);
  const [volumeView, setVolumeView] = useState(false);
  const [aggregation, setAggregation] = useState<Aggregation>("volume");
  const [barSizes, setBarSizes] = useState<Record<Aggregation, number>>({
    volume: aggregations.volume.initial,
    time: aggregations.time.initial,
    ticks: aggregations.ticks.initial,
    notional: aggregations.notional.initial,
  });
  const [liveVolumeSize, setLiveVolumeSize] = useState(1);
  const isLive = sourceMode(choice, hostedAdapter) === "live";
  const playbackEndpoint = hostedAdapter?.playbackEndpoint;
  const hostedState = useRef<PlaybackStatus | undefined>(
    hostedAdapter?.playback,
  );
  const [playbackBusy, setPlaybackBusy] = useState(false);
  const playbackRequest = useRef(false);
  const playbackVersion = useRef(0);
  const currentPlaybackEndpoint = useRef(playbackEndpoint);
  currentPlaybackEndpoint.current = playbackEndpoint;
  const barSize =
    aggregation === "volume" && isLive ? liveVolumeSize : barSizes[aggregation];
  const barOptions =
    aggregation === "volume" && isLive
      ? [1, 5, 10, 50, 100, 1000]
      : aggregations[aggregation].sizes;
  const [queue, setQueue] = useState<QueueStatus | null>(null);
  const [simulation, setSimulation] = useState<SimulationStatus | null>(null);
  const [adapters, setAdapters] = useState<AdapterInfo[]>([]);
  const [gpuBackend, setGpuBackend] = useState<string | null>(null);
  const [stats, setStats] = useState(() => ({
    ...initial,
    ...(server
      ? {
          name: server.name,
          mode: sourceMode(choice, hostedAdapter),
          timezone: hostedAdapter?.timezone ?? "UTC",
        }
      : {}),
  }));
  const [sessionError, setError] = useState("");
  const error = sessionError || (choice.kind === "nasdaq" ? samplesError : "");
  const host = useRef<HTMLDivElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const session = useRef<Session | null>(null);
  const connection = useRef<ReturnType<typeof connectSocket> | null>(null);
  const controls = useRef({
    playing,
    speed,
    windowSeconds,
    span,
    volumeView,
    barSize,
    aggregation,
  });
  controls.current = {
    playing,
    speed,
    windowSeconds,
    span,
    volumeView,
    barSize,
    aggregation,
  };
  const loadedValue = useRef(loadedTicker);
  loadedValue.current = loadedTicker;
  const startValue = useRef(start);
  startValue.current = start;

  const acceptPlayback = (state: PlaybackStatus) => {
    hostedState.current = state;
    controls.current.playing = !state.paused;
    setPlaying(!state.paused);
    setSpeed(state.speed);
    if (state.error) setError(state.error);
  };
  const controlPlayback = async (command: PlaybackCommand) => {
    if (!playbackEndpoint || playbackRequest.current) return;
    playbackRequest.current = true;
    playbackVersion.current++;
    setPlaybackBusy(true);
    try {
      const state = await hostedPlayback(playbackEndpoint, command);
      if (currentPlaybackEndpoint.current !== playbackEndpoint) return;
      acceptPlayback(state);
      setError("");
      if (command.action === "seek" || command.action === "restart") {
        setSimulation(null);
        setQueue(null);
        // Recreate chart history and obtain a coherent post-seek snapshot.
        setGeneration((n) => n + 1);
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      playbackRequest.current = false;
      setPlaybackBusy(false);
    }
  };
  const changePlaying = (next: boolean) => {
    if (playbackEndpoint)
      void controlPlayback({ action: next ? "play" : "pause" });
    else {
      controls.current.playing = next;
      setPlaying(next);
    }
  };
  useEffect(() => {
    if (!playbackEndpoint) {
      hostedState.current = undefined;
      return;
    }
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      try {
        if (!playbackRequest.current) {
          const version = playbackVersion.current;
          const state = await hostedPlayback(
            playbackEndpoint,
            undefined,
            abort.signal,
          );
          if (
            !abort.signal.aborted &&
            !playbackRequest.current &&
            version === playbackVersion.current
          )
            acceptPlayback(state);
        }
      } catch (cause) {
        if (!abort.signal.aborted)
          setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        if (!abort.signal.aborted) timer = setTimeout(refresh, 200);
      }
    };
    void refresh();
    return () => {
      abort.abort();
      clearTimeout(timer);
    };
  }, [playbackEndpoint]);

  useEffect(() => {
    if (server) return;
    const abort = new AbortController();
    setSamplesError("");
    void (async () => {
      try {
        const response = await fetch("/api/nasdaq-sessions", {
          signal: abort.signal,
        });
        if (!response.ok) throw Error(await response.text());
        const list = (await response.json()) as NasdaqSession[];
        if (!list.length)
          throw Error("No Nasdaq sample sessions are available");
        setSamples(list);
        setChoice((current) => {
          if (current.kind !== "nasdaq") return current;
          const selected =
            list.find((s) => s.name === current.session?.name) ?? list[0];
          return current.session?.name === selected.name &&
            current.session.size === selected.size
            ? current
            : { kind: "nasdaq", session: selected };
        });
      } catch (cause) {
        if (!abort.signal.aborted)
          setSamplesError(
            cause instanceof Error ? cause.message : String(cause),
          );
      }
    })();
    return () => abort.abort();
  }, [generation, server]);

  useEffect(() => {
    const container = host.current;
    if (!container) return;
    const abort = new AbortController();
    setSimulation(null);
    setQueue(null);
    setGpuBackend(null);
    setReconstruction({ clock: 0, percent: 0, target: startValue.current });
    const canvas = document.createElement("canvas");
    canvas.className = "gpu-canvas";
    canvas.setAttribute(
      "aria-label",
      `${loadedValue.current} depth heatmap, current depth, and volume history`,
    );
    canvas.setAttribute("role", "img");
    const overlay = document.createElement("canvas");
    overlay.className = "axis-canvas";
    overlay.setAttribute("aria-hidden", "true");
    container.replaceChildren(canvas, overlay);
    const context = overlay.getContext("2d")!;
    let stopGpuStatus: (() => void) | undefined;
    let theme = chartTheme(container);
    let disposed = false,
      native: Session | null = null,
      animation = 0,
      loadingAnimation = 0,
      previous = 0,
      lastRender = 0,
      lastUi = 0;
    const replayClock = new ReplayClock();
    const tasks = new MessageChannel();
    let firstReplayTime: number | undefined;
    const warmingReplay = () => choice.kind !== "live" && !!native?.warming;
    let mouse: { x: number; y: number } | null = null;
    const resize = () => {
      const dpr = Math.min(window.devicePixelRatio || 1, 2),
        rect = container.getBoundingClientRect();
      canvas.width = overlay.width = Math.max(1, Math.round(rect.width * dpr));
      canvas.height = overlay.height = Math.max(
        1,
        Math.round(rect.height * dpr),
      );
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(container);
    const onMove = (event: PointerEvent) => {
      if (warmingReplay()) return;
      const rect = container.getBoundingClientRect();
      mouse = {
        x: (event.clientX - rect.left) / rect.width,
        y: (event.clientY - rect.top) / rect.height,
      };
    };
    const onLeave = () => {
      mouse = null;
    };
    container.addEventListener("pointermove", onMove);
    container.addEventListener("pointerleave", onLeave);
    const onWheel = (event: WheelEvent) => {
      if (
        !native ||
        warmingReplay() ||
        controls.current.volumeView ||
        event.ctrlKey
      )
        return;
      event.preventDefault();
      const pixels =
        event.deltaY *
        (event.deltaMode === 1
          ? 16
          : event.deltaMode === 2
            ? container.clientHeight
            : 1);
      native.pan_price(-pixels / Math.max(1, container.clientHeight));
    };
    const onKey = (event: KeyboardEvent) => {
      if (!native || warmingReplay() || controls.current.volumeView) return;
      if (event.key === "ArrowUp" || event.key === "ArrowDown") {
        event.preventDefault();
        native.pan_price(event.key === "ArrowUp" ? 0.1 : -0.1);
      } else if (event.key === "Home") {
        event.preventDefault();
        native.recenter();
      }
    };
    const onClick = (event: MouseEvent) => {
      if (!native || warmingReplay() || controls.current.volumeView) return;
      const rect = container.getBoundingClientRect();
      if (
        native.select_queue_at(
          (event.clientX - rect.left) / rect.width,
          (event.clientY - rect.top) / rect.height,
        )
      ) {
        setQueue(JSON.parse(native.queue_status()) as QueueStatus | null);
      }
    };
    container.addEventListener("click", onClick);
    container.addEventListener("wheel", onWheel, { passive: false });
    container.addEventListener("keydown", onKey);
    let layout: ChartLayout | null = null;
    const drawAxes = () => {
      if (!native || !layout || native.warming) return;
      const hovering = drawChartOverlay(
        context,
        native,
        layout,
        { ...controls.current, hiddenDepth: !!((server?.metrics ?? 0) & 1) },
        mouse,
        Math.min(window.devicePixelRatio || 1, 2),
        theme,
      );
      container.style.cursor = hovering ? "crosshair" : "default";
    };
    const themeObserver = new MutationObserver(() => {
      theme = chartTheme(container);
      native?.set_chart_theme(theme.gpu);
      drawAxes();
    });
    themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme", "data-terminal-theme"],
    });
    setError("");
    setTickers([]);
    setDirectory([]);
    setStats({
      ...initial,
      status: "Loading",
      mode: sourceMode(choice, hostedAdapter),
      timezone:
        hostedAdapter?.timezone ?? (choice.kind === "live" ? "UTC" : "ET"),
      name:
        choice.kind === "file"
          ? choice.file.name
          : choice.kind === "nasdaq"
            ? (choice.session?.name ?? "Nasdaq sample sessions")
            : (server?.name ?? "Live feed"),
    });
    void (async () => {
      try {
        if (!("gpu" in navigator))
          throw new Error(
            "This browser does not expose WebGPU. Open the app in a WebGPU-capable browser on localhost or HTTPS.",
          );
        const module = await loadWasm();
        if (disposed) return;
        layout = JSON.parse(module.chart_layout()) as ChartLayout;
        container.parentElement?.style.setProperty(
          "--heatmap-width",
          `${layout.heatmapWidth * 100}%`,
        );
        container.parentElement?.style.setProperty(
          "--price-top",
          `${layout.priceTop * 100}%`,
        );
        const catalog = server?.adapters?.length
          ? server.adapters
          : (JSON.parse(module.available_adapters()) as AdapterInfo[]);
        setAdapters(catalog);
        if (choice.kind === "nasdaq" && !choice.session) return;
        const source = await openSource(choice, abort.signal, catalog);
        if (disposed) return;
        const live = choice.kind === "live";
        if (!live && source.size === 0)
          throw new Error("The selected file is empty.");
        const parts = startValue.current.split(":").map(Number);
        const startMs = live
          ? 0
          : (parts[0] * 3600 + parts[1] * 60 + (parts[2] || 0)) * 1000;
        native = server?.adapters?.some(
          (a) => a.id === source.adapter.id && a.observer,
        )
          ? await module.FeedSession.create_observed_scoped(
              canvas,
              JSON.stringify(source.adapter),
              loadedValue.current,
              JSON.stringify(scope),
            )
          : await module.FeedSession.create_scoped(
              canvas,
              source.adapter.id,
              loadedValue.current,
              startMs,
              JSON.stringify(scope),
            );
        if (disposed) {
          native.free();
          native = null;
          return;
        }
        stopGpuStatus = observeGpuStatus(
          canvas,
          native.gpu_backend,
          setGpuBackend,
        );
        native.set_observer_metrics(server?.metrics ?? 0);
        native.set_chart_theme(theme.gpu);
        native.set_bar_aggregation(
          controls.current.aggregation,
          controls.current.barSize,
        );
        native.show_volume_bars(controls.current.volumeView);
        session.current = native;
        const loadingStarted = performance.now();
        let lastLoading = -Infinity;
        const animateLoading = (now: number) => {
          if (disposed || !native) return;
          const visibility = native.warming ? "hidden" : "visible";
          if (overlay.style.visibility !== visibility)
            overlay.style.visibility = visibility;
          if (native.warming && !document.hidden && now - lastLoading >= 80) {
            try {
              native.render_loading(
                (now - loadingStarted) / 1000,
                Math.min(window.devicePixelRatio || 1, 2),
              );
              lastLoading = now;
            } catch (cause) {
              setError(cause instanceof Error ? cause.message : String(cause));
              return;
            }
          }
          loadingAnimation = requestAnimationFrame(animateLoading);
        };
        native.set_loading_tickers(scope ?? []);
        loadingAnimation = requestAnimationFrame(animateLoading);
        let eof = false,
          directoryCount = -1;
        let observedClock = 0;
        const read = async () => {
          if (eof) return;
          const chunk = await source.read();
          if (disposed) return;
          eof = chunk.eof;
          native!.append(chunk.bytes, eof);
        };
        if (live) {
          await bootstrapFeed(native, abort.signal);
          if (disposed) return;
          connection.current = connectSocket(native, abort.signal);
        } else await read();
        if (disposed) return;
        const frame = async (now: number) => {
          if (disposed || !native) return;
          try {
            if (live && source.adapter.mode === "replay") {
              if (native.source_clock_ms < observedClock) {
                // Python or another browser can rewind the shared replay too.
                // Recreate GPU history from the host's reconstructed snapshot.
                setGeneration((n) => n + 1);
                return;
              }
              observedClock = native.source_clock_ms;
            }
            const dt = previous ? Math.min(now - previous, 250) : 0;
            previous = now;
            const wasWarming = !live && native.warming;
            if (live) {
              if (native.start_ms && source.adapter.mode === "live")
                native.advance(
                  Math.max(0, Date.now() - native.start_ms),
                  20000,
                );
              else if (source.adapter.mode === "replay")
                native.advance(
                  Math.max(0, native.source_clock_ms - native.start_ms),
                  20000,
                );
              connection.current?.flush();
            } else if (
              !document.hidden &&
              (controls.current.playing || native.warming)
            ) {
              const speed = controls.current.speed;
              const target = replayClock.target(
                native,
                dt,
                speed,
                controls.current.playing,
              );
              await replaySlice(
                native,
                target,
                read,
                () =>
                  disposed ||
                  document.hidden ||
                  controls.current.speed !== speed ||
                  (!controls.current.playing && !native?.warming),
              );
              if (disposed || !native) return;
            }
            const preparing = !live && native.warming;
            if (wasWarming && !preparing) {
              replayClock.followSource(native);
              // Reconstruction time must not become paced playback time.
              previous = 0;
              mouse = null;
              lastRender = lastUi = -Infinity;
            }
            if (!native.warming && now - lastRender >= 33) {
              native.render(
                controls.current.windowSeconds,
                controls.current.span,
              );
              drawAxes();
              lastRender = now;
            }
            if (now - lastUi >= 200) {
              if (native.ticker_count !== directoryCount) {
                const available = native.tickers();
                const scoped = native.scoped_tickers();
                setDirectory(available);
                setTickers(scoped);
                if (
                  server &&
                  scoped.length &&
                  !scoped.includes(loadedValue.current)
                ) {
                  native.select_ticker(scoped[0]);
                  loadedValue.current = scoped[0];
                  setLoadedTicker(scoped[0]);
                }
                // Discovery and subscription are distinct: All opens only the
                // visited ticker; an explicit browser scope opens its members.
                if (hostedAdapter?.subscriptionsEndpoint && scoped.length) {
                  await subscribeHostedAdapter(
                    hostedAdapter.subscriptionsEndpoint,
                    loadedValue.current,
                    scope ?? [],
                    abort.signal,
                  );
                  if (disposed) return;
                }
                // Spread the bounded display across the real directory, instead
                // of filling it with only the first alphabetic prefix.
                const shown =
                  scoped.length <= 100
                    ? scoped
                    : Array.from(
                        { length: 100 },
                        (_, i) => scoped[Math.floor((i * scoped.length) / 100)],
                      );
                native.set_loading_tickers(shown);
                directoryCount = available.length;
              }
              if (preparing) {
                const sourceTime = native.source_clock_ms;
                if (sourceTime > 0) firstReplayTime ??= sourceTime;
                const origin = firstReplayTime ?? 0;
                const target = native.start_ms || startMs;
                setReconstruction({
                  clock: sourceTime,
                  target: clock(target),
                  percent:
                    target > origin
                      ? Math.min(
                          100,
                          Math.max(
                            0,
                            ((sourceTime - origin) / (target - origin)) * 100,
                          ),
                        )
                      : 0,
                });
                const progress = {
                  name: source.name,
                  size: source.size,
                  received: source.received,
                  compressed: source.compressed,
                  messages: native.messages,
                  consumed: native.bytes_consumed,
                };
                setStats((current) => ({
                  ...current,
                  ...progress,
                  status: "Reconstructing",
                }));
              } else {
                const playback = hostedState.current;
                const status =
                  live && source.adapter.mode === "live"
                    ? connection.current?.status() === "Connected"
                      ? native.warming
                        ? "Waiting for snapshot"
                        : "Live"
                      : connection.current?.status() || "Connecting"
                    : playback?.seeking
                      ? "Reconstructing"
                      : (playback?.complete ?? native.complete)
                        ? "Complete"
                        : (
                              playback
                                ? !playback.paused
                                : controls.current.playing
                            )
                          ? "Playing"
                          : "Paused";
                setSimulation(
                  JSON.parse(
                    native.simulation_status(),
                  ) as SimulationStatus | null,
                );
                setQueue(
                  JSON.parse(native.queue_status()) as QueueStatus | null,
                );
                const nextStats = {
                  status,
                  name: source.name,
                  size: source.size,
                  received: source.received,
                  compressed: source.compressed,
                  clock: native.clock_ms || startMs,
                  bid: native.bid,
                  ask: native.ask,
                  updates: native.updates,
                  supportsTrades: source.adapter.supportsTrades,
                  level: source.adapter.level,
                  mode: source.adapter.mode,
                  quantityDecimals: native.quantity_decimals,
                  volumeBars: native.volume_bar_count,
                  formingVolume: native.forming_volume,
                  formingProgress: native.forming_progress,
                  messages: native.messages,
                  consumed: native.bytes_consumed,
                  center: native.center,
                  activeBooks: native.active_books,
                  priceDecimals: native.price_decimals,
                  span: native.span,
                  adapterName: source.adapter.name,
                  timezone: source.adapter.timezone,
                  checksums: native.checksum_checks,
                  checksumFailures: native.checksum_failures,
                  connectionDetail: connection.current?.detail() || "",
                };
                setStats(nextStats);
              }
              lastUi = now;
            }
            if (!live && !document.hidden && !native.complete && native.warming)
              tasks.port2.postMessage(null);
            else
              animation = requestAnimationFrame((time) => {
                void frame(time);
              });
          } catch (cause) {
            if (!disposed) {
              setError(cause instanceof Error ? cause.message : String(cause));
              setPlaying(false);
            }
          }
        };
        tasks.port1.onmessage = () => {
          void frame(performance.now());
        };
        animation = requestAnimationFrame((time) => {
          void frame(time);
        });
      } catch (cause) {
        if (!disposed)
          setError(cause instanceof Error ? cause.message : String(cause));
      }
    })();
    return () => {
      disposed = true;
      stopGpuStatus?.();
      abort.abort();
      connection.current?.close();
      connection.current = null;
      cancelAnimationFrame(animation);
      cancelAnimationFrame(loadingAnimation);
      tasks.port1.close();
      tasks.port2.close();
      observer.disconnect();
      themeObserver.disconnect();
      container.removeEventListener("pointermove", onMove);
      container.removeEventListener("pointerleave", onLeave);
      container.removeEventListener("wheel", onWheel);
      container.removeEventListener("click", onClick);
      container.removeEventListener("keydown", onKey);
      // No WASM calls are pending across await except initial async construction.
      if (native) {
        native.free();
        native = null;
      }
      session.current = null;
    };
  }, [choice, generation, scope, server]);

  useEffect(() => {
    const hide = () => {
      if (document.hidden) {
        changePlaying(false);
      }
    };
    document.addEventListener("visibilitychange", hide);
    return () => document.removeEventListener("visibilitychange", hide);
  }, [playbackEndpoint]);
  const chooseSource = (next: SourceChoice) => {
    setScope(next.kind === "nasdaq" ? categories.tech : null);
    const adapter =
      next.kind === "live"
        ? adapters.find((a) => a.id === next.adapterId)
        : adapters.find((a) => a.mode === "replay");
    if (next.kind === "nasdaq") setLoadedTicker("AAPL");
    else if (adapter) setLoadedTicker(adapter.defaultSymbol);
    setChoice(next);
    if (adapter && !adapter.supportsTrades) {
      setVolumeView(false);
      session.current?.show_volume_bars(false);
    }
    setPlaying(adapter?.mode === "replay" && server ? false : true);
  };
  const restart = () => {
    if (playbackEndpoint) {
      void controlPlayback({ action: "restart" });
      return;
    }
    setGeneration((n) => n + 1);
    setPlaying(true);
  };
  const seek = async () => {
    if (!playbackEndpoint) {
      restart();
      return;
    }
    try {
      // An untouched epoch-based file has no known recording day yet. Discover
      // its origin through the public replay API before interpreting START AT.
      if (hostedState.current?.start_ns == null) {
        await controlPlayback({ action: "restart" });
        if (hostedState.current?.start_ns == null) return;
      }
      void controlPlayback({
        action: "seek",
        timestamp_ns: seekTimestamp(start, hostedState.current?.clock_ns ?? 0),
      });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };
  const sourceProgress = stats.compressed ? stats.received : stats.consumed;
  const progress = stats.size
    ? Math.min(100, (sourceProgress / stats.size) * 100)
    : 0;
  const loading = [
    "Loading",
    "Reconstructing",
    "Connecting",
    "Reconnecting",
    "Waiting for snapshot",
  ].includes(stats.status);
  const preparing = !isLive && (loading || playbackBusy);
  const sourceOptions = [
    ...(!server
      ? [
          {
            id: "replay",
            name: "Nasdaq ITCH",
            mode: "replay",
            selected: !isLive,
            select: () =>
              chooseSource({ kind: "nasdaq", session: samples[0] ?? null }),
          },
        ]
      : []),
    ...adapters
      .filter((adapter) =>
        server
          ? adapter.id === "server" ||
            server.adapters?.some((configured) => configured.id === adapter.id)
          : adapter.mode === "live" && adapter.id !== "server",
      )
      .map((adapter) => ({
        id: adapter.id,
        name: adapter.name,
        mode: adapter.mode,
        selected: choice.kind === "live" && choice.adapterId === adapter.id,
        select: () => chooseSource({ kind: "live", adapterId: adapter.id }),
      })),
  ];
  return (
    <main className="terminal">
      <h1 className="sr-only">lobo order book viewer</h1>
      <section className="source-bar" aria-label="Market data source">
        <a
          className="source-icon"
          href="/"
          aria-label="lobo home"
          title="lobo · Replay lab"
        >
          <Icon kind="file" />
        </a>
        <div className="source-description">
          <span className="field-label">DATA SOURCE</span>
          {server && <strong title={stats.name}>{stats.name}</strong>}
        </div>
        <div
          className="source-toggles toggle-group"
          role="group"
          aria-label="Data source"
        >
          {sourceOptions.map((source) => (
            <button
              key={source.id}
              aria-pressed={source.selected}
              onClick={() => {
                if (!source.selected) source.select();
              }}
            >
              <span>{source.name}</span>
              {source.selected ? (
                <span className="source-active">
                  {source.mode === "live" ? "Live" : "Replay"} · Active
                </span>
              ) : (
                <span
                  className={source.mode === "live" ? "green" : "source-mode"}
                >
                  {source.mode === "live" ? "Live" : "Replay"}
                </span>
              )}
            </button>
          ))}
        </div>
        {(!server || hostedAdapter?.subscriptionsEndpoint) && (
          <BookScopeSelector
            value={scope}
            tickers={directory}
            live={isLive}
            onApply={(next) => {
              if (JSON.stringify(scope) === JSON.stringify(next)) return;
              if (next && !next.includes(loadedTicker))
                setLoadedTicker(next[0]);
              setScope(next);
              setPlaying(true);
            }}
          />
        )}
        {!server && (
          <div className="source-actions">
            <label className="sample-sessions">
              <span className="field-label">Nasdaq sample sessions</span>
              <select
                aria-label="Nasdaq sample sessions"
                value={
                  choice.kind === "nasdaq" ? (choice.session?.name ?? "") : ""
                }
                disabled={!samples.length}
                onChange={(event) => {
                  const sample = samples.find(
                    (s) => s.name === event.target.value,
                  );
                  if (sample) chooseSource({ kind: "nasdaq", session: sample });
                }}
              >
                <option value="" disabled>
                  {samplesError
                    ? "Directory unavailable"
                    : samples.length
                      ? "Select a session"
                      : "Loading sessions…"}
                </option>
                {samples.map((sample) => (
                  <option key={sample.name} value={sample.name}>
                    {sample.name} · {bytes(sample.size)}
                  </option>
                ))}
              </select>
            </label>
            <button
              className={
                choice.kind === "repository" ? "subtle selected" : "subtle"
              }
              title={choice.kind === "repository" ? stats.name : undefined}
              onClick={() => {
                chooseSource({ kind: "repository" });
              }}
            >
              Repository file
            </button>
            <button
              className={
                choice.kind === "file" ? "outline selected" : "outline"
              }
              title={choice.kind === "file" ? choice.file.name : undefined}
              aria-label="Open replay file"
              onClick={() => fileInput.current?.click()}
            >
              <Icon kind="file" />
              {choice.kind === "file" ? choice.file.name : "Open replay file"}
            </button>
          </div>
        )}
        <ThemeSelector />
        {!server && (
          <input
            ref={fileInput}
            type="file"
            className="file-input"
            aria-label="Choose a replay file"
            onChange={(event) => {
              const file = event.target.files?.[0];
              if (file) {
                chooseSource({ kind: "file", file });
              }
              event.target.value = "";
            }}
          />
        )}
      </section>

      <div className="workstation">
        <aside className="control-rail" aria-label="Session controls">
          <section
            className="quote-panel terminal-panel"
            aria-label="Current book"
          >
            <div className="symbol-clock">
              <TickerPicker
                key={`${generation}-${choice.kind}`}
                value={loadedTicker}
                tickers={tickers}
                venue={stats.adapterName}
                disabled={
                  selecting || preparing || !!simulation?.alternateTimeline
                }
                onSelect={async (symbol) => {
                  const native = session.current;
                  if (!native) return;
                  setSelecting(true);
                  try {
                    if (hostedAdapter?.subscriptionsEndpoint) {
                      await subscribeHostedAdapter(
                        hostedAdapter.subscriptionsEndpoint,
                        symbol,
                      );
                      if (session.current !== native) return;
                    }
                    native.select_ticker(symbol);
                    connection.current?.flush();
                    setLoadedTicker(symbol);
                    host.current
                      ?.querySelector(".gpu-canvas")
                      ?.setAttribute(
                        "aria-label",
                        `${symbol} depth heatmap, current depth, and volume history`,
                      );
                    setStats((current) => ({
                      ...current,
                      bid: native.bid,
                      ask: native.ask,
                      updates: native.updates,
                      volumeBars: native.volume_bar_count,
                      formingVolume: native.forming_volume,
                      formingProgress: native.forming_progress,
                      center: native.center,
                      priceDecimals: native.price_decimals,
                      span: native.span,
                    }));
                    setError("");
                  } catch (cause) {
                    setError(
                      cause instanceof Error ? cause.message : String(cause),
                    );
                  } finally {
                    setSelecting(false);
                  }
                }}
              />

              <div className="clock-quote">
                <span className="field-label">EXCHANGE TIME</span>
                <strong>
                  {playbackEndpoint && hostedState.current?.start_ns == null
                    ? "—"
                    : clock(stats.clock)}
                  <small>{stats.timezone}</small>
                </strong>
                <span className="feed-capabilities">
                  {stats.level.toUpperCase()} · {stats.mode.toUpperCase()}
                </span>
              </div>
            </div>
            <div className="quote-grid">
              <div className="quote bid-quote">
                <span>BEST BID</span>
                <strong className="green">
                  {price(stats.bid, stats.priceDecimals)}
                </strong>
              </div>
              <div className="quote ask-quote">
                <span>BEST ASK</span>
                <strong className="red">
                  {price(stats.ask, stats.priceDecimals)}
                </strong>
              </div>
            </div>
            <div className="spread-quote">
              <span className="field-label">SPREAD</span>
              <strong>
                {stats.bid && stats.ask
                  ? Math.max(0, stats.ask - stats.bid).toFixed(
                      stats.priceDecimals,
                    )
                  : "—"}
              </strong>
            </div>
          </section>
          <section
            className="controller terminal-panel"
            aria-label={isLive ? "Live feed controller" : "Replay controller"}
          >
            <div className="panel-heading">
              <h2>{isLive ? "LIVE FEED" : "REPLAY CONTROLLER"}</h2>
              <span
                className={`status-dot ${stats.status === "Playing" || stats.status === "Live" ? "active" : ""}`}
              />
              <span role="status">
                {error ? "Needs attention" : stats.status}
              </span>
            </div>
            <div className="transport">
              {isLive ? (
                <div className="live-transport">
                  <span className="local-dot" />
                  Selected pairs stay subscribed
                  <button
                    className="subtle"
                    onClick={() => connection.current?.reconnect()}
                  >
                    Reconnect
                  </button>
                </div>
              ) : (
                <>
                  <div className="play-controls">
                    <button
                      className="play-button"
                      aria-label={playing ? "Pause replay" : "Play replay"}
                      disabled={
                        !!error || playbackBusy || stats.status === "Complete"
                      }
                      onClick={() => changePlaying(!playing)}
                    >
                      <Icon kind={playing ? "pause" : "play"} />
                    </button>
                    <button
                      className="icon-button"
                      aria-label="Restart replay"
                      title="Restart replay"
                      disabled={playbackBusy}
                      onClick={restart}
                    >
                      <Icon kind="restart" />
                    </button>
                  </div>
                  <div className="speed-control">
                    <span className="field-label">PLAYBACK SPEED</span>
                    <div
                      className="speeds toggle-group"
                      role="group"
                      aria-label="Playback speed"
                      inert={preparing}
                    >
                      {SPEEDS.map((value) => (
                        <button
                          key={value}
                          aria-pressed={speed === value}
                          className={speed === value ? "active" : ""}
                          onClick={() =>
                            playbackEndpoint
                              ? void controlPlayback({
                                  action: "speed",
                                  speed: value,
                                })
                              : setSpeed(value)
                          }
                        >
                          {speedLabel(value)}
                        </button>
                      ))}
                    </div>
                  </div>
                  <div className="start-control">
                    <label htmlFor="start-time">START AT</label>
                    <input
                      id="start-time"
                      type="time"
                      step="1"
                      value={start}
                      onChange={(e) => setStart(e.target.value)}
                    />
                    <button
                      className="subtle"
                      disabled={playbackBusy}
                      onClick={seek}
                    >
                      Apply
                    </button>
                  </div>
                </>
              )}
            </div>

            <dl className="telemetry">
              <div>
                <dt>GPU acceleration</dt>
                <dd
                  className={gpuBackend ? "green" : "red"}
                  title={gpuBackend ?? undefined}
                >
                  {gpuBackend ?? "Not available"}
                </dd>
              </div>
              <div>
                <dt>Feed messages</dt>
                <dd>{count.format(stats.messages)}</dd>
              </div>
              <div>
                <dt>{loadedTicker} updates</dt>
                <dd>{count.format(stats.updates)}</dd>
              </div>
              {isLive || playbackEndpoint ? (
                <>
                  <div>
                    <dt>Received</dt>
                    <dd>{bytes(stats.consumed)}</dd>
                  </div>
                  <div>
                    <dt>Checksums verified</dt>
                    <dd>
                      {count.format(stats.checksums - stats.checksumFailures)}
                    </dd>
                  </div>
                  <div>
                    <dt>Checksum resyncs</dt>
                    <dd>{count.format(stats.checksumFailures)}</dd>
                  </div>
                </>
              ) : (
                <>
                  <div>
                    <dt>
                      {stats.compressed ? "Source received" : "File processed"}
                    </dt>
                    <dd>{progress.toFixed(1)}%</dd>
                  </div>
                  <div className="file-size">
                    <dt>
                      {bytes(sourceProgress)} / {bytes(stats.size)}
                    </dt>
                    <dd>{speedLabel(speed)}</dd>
                  </div>
                  {stats.compressed && (
                    <div>
                      <dt>Decoded / processed</dt>
                      <dd>{bytes(stats.consumed)}</dd>
                    </div>
                  )}
                </>
              )}
            </dl>
            {!isLive && !playbackEndpoint && (
              <div
                className="progress-track"
                role="progressbar"
                aria-label={
                  stats.compressed
                    ? "Source transfer progress"
                    : "File replay progress"
                }
                aria-valuenow={Math.round(progress)}
                aria-valuemin={0}
                aria-valuemax={100}
              >
                <div style={{ width: `${progress}%` }} />
              </div>
            )}
          </section>
          <SimulationPanel
            disabled={preparing}
            key={`${generation}-${choice.kind}`}
            level={stats.level}
            mode={stats.mode}
            quantityDecimals={stats.quantityDecimals}
            note={session.current?.simulation_note()}
            ready={stats.bid > 0 || stats.ask > 0}
            bid={stats.bid}
            ask={stats.ask}
            decimals={stats.priceDecimals}
            status={simulation}
            ended={stats.status === "Complete"}
            onOpen={() => {
              if (stats.mode === "replay") changePlaying(false);
            }}
            onRun={(kind, side, price, quantity) => {
              const native = session.current;
              if (!native) throw new Error("The book is not ready");
              if (kind === "market") native.simulate_market(side, quantity);
              else native.simulate(side, price, quantity);
              setSimulation(
                JSON.parse(native.simulation_status()) as SimulationStatus,
              );
              setQueue(JSON.parse(native.queue_status()) as QueueStatus | null);
              if (kind === "limit") changePlaying(true);
            }}
            onReturn={() => {
              session.current?.return_to_main();
              setSimulation(null);
              setQueue(null);
            }}
          />

          <p className="session-note">
            {isLive
              ? "Public market data · no credentials. Fresh snapshots restore books after reconnection."
              : playbackEndpoint
                ? "Shared playback · recorded data. Seeking reconstructs from the beginning."
                : choice.kind === "nasdaq"
                  ? "Nasdaq gzip stream · no session file saved. Reconstructed from the start. Hidden tabs pause playback."
                  : "Reconstructed from the start. Local files stay on this device. Hidden tabs pause playback."}
          </p>
        </aside>
        <section className="book-panel" aria-label="Order book charts">
          <div className="chart-controls" inert={preparing}>
            <div className="chart-title">
              <span className="chart-title-line" />
              <strong>
                {loadedTicker}{" "}
                {volumeView
                  ? `${aggregations[aggregation].name} bars`
                  : "liquidity heatmap & vertical depth"}
              </strong>
              <span className="chart-subtitle">
                {count.format(stats.activeBooks)} BOOKS ADVANCING
              </span>
            </div>
            <div className="chart-options">
              <div className="legend">
                <span className="legend-bid" />
                Bid
                <span className="legend-ask" />
                Ask
              </div>
              {volumeView ? (
                <>
                  <label>
                    Aggregate by
                    <select
                      aria-label="Bar aggregation"
                      value={aggregation}
                      title="Changing aggregation starts fresh bars at the current replay position"
                      onChange={(e) => {
                        const next = e.target.value as Aggregation;
                        session.current?.set_bar_aggregation(
                          next,
                          next === "volume" && isLive
                            ? liveVolumeSize
                            : barSizes[next],
                        );
                        setAggregation(next);
                        setStats((v) => ({
                          ...v,
                          volumeBars: 0,
                          formingVolume: 0,
                          formingProgress: 0,
                        }));
                      }}
                    >
                      {Object.entries(aggregations).map(([key, value]) => (
                        <option key={key} value={key}>
                          {value.name}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label title={aggregations[aggregation].hint}>
                    {aggregations[aggregation].label}
                    <select
                      aria-label="Bar size"
                      value={barSize}
                      onChange={(e) => {
                        const size = Number(e.target.value);
                        session.current?.set_bar_aggregation(aggregation, size);
                        if (aggregation === "volume" && isLive)
                          setLiveVolumeSize(size);
                        else
                          setBarSizes((v) => ({ ...v, [aggregation]: size }));
                        setStats((v) => ({
                          ...v,
                          volumeBars: 0,
                          formingVolume: 0,
                          formingProgress: 0,
                        }));
                      }}
                    >
                      {barOptions.map((size) => (
                        <option key={size} value={size}>
                          {sizeLabel(aggregation, size)}
                        </option>
                      ))}
                    </select>
                  </label>
                </>
              ) : (
                <label>
                  Window
                  <select
                    aria-label="History window"
                    value={windowSeconds}
                    onChange={(e) => setWindow(Number(e.target.value))}
                  >
                    <option value="120">2 min</option>
                    <option value="300">5 min</option>
                    <option value="900">15 min</option>
                  </select>
                </label>
              )}
              {!volumeView && (
                <label>
                  Min. range
                  <select
                    aria-label="Price range"
                    title="Automatically widens to keep the best bid and ask visible. Scrolling pauses tracking; Recenter resumes it."
                    value={span}
                    onChange={(e) => setSpan(Number(e.target.value))}
                  >
                    <option value="0.001">0.1%</option>
                    <option value="0.005">0.5%</option>
                    <option value="0.02">2%</option>
                    <option value="0.1">10%</option>
                  </select>
                </label>
              )}
              {!volumeView && (
                <button
                  className="icon-button"
                  title="Center on current best price and resume automatic zoom"
                  aria-label="Recenter chart"
                  onClick={() => session.current?.recenter()}
                >
                  <Icon kind="focus" />
                </button>
              )}
            </div>
          </div>
          <div className="chart-view-bar" inert={preparing}>
            <div className="chart-tabs" role="tablist" aria-label="Chart view">
              <button
                role="tab"
                aria-selected={!volumeView}
                aria-controls="market-chart"
                onClick={() => {
                  setVolumeView(false);
                  session.current?.show_volume_bars(false);
                }}
              >
                Order book
              </button>
              <button
                role="tab"
                aria-selected={volumeView}
                aria-controls="market-chart"
                disabled={!stats.supportsTrades}
                title={
                  !stats.supportsTrades
                    ? "This feed does not supply execution messages"
                    : undefined
                }
                onClick={() => {
                  setVolumeView(true);
                  session.current?.show_volume_bars(true);
                }}
              >
                OHLC bars
              </button>
              {volumeView && (
                <span>
                  {count.format(stats.volumeBars)} completed · forming{" "}
                  {new Intl.NumberFormat("en-US", {
                    maximumFractionDigits:
                      aggregation === "time"
                        ? 1
                        : aggregation === "volume" && isLive
                          ? 6
                          : 0,
                  }).format(stats.formingProgress)}
                  {aggregation === "time" ? "s" : ""} /{" "}
                  {sizeLabel(aggregation, barSize)}{" "}
                  {aggregation !== "time" ? aggregations[aggregation].unit : ""}
                </span>
              )}
            </div>
          </div>
          <div
            className="chart-area"
            id="market-chart"
            role="tabpanel"
            aria-label={
              volumeView
                ? `${aggregations[aggregation].name} OHLC bars`
                : "Order book"
            }
          >
            {!volumeView && !loading && (
              <div className="depth-heading">
                <strong>DEPTH / PRICE AXIS</strong>
                <span>CUMULATIVE QTY</span>
              </div>
            )}
            {volumeView &&
              stats.volumeBars === 0 &&
              stats.formingVolume === 0 &&
              !loading &&
              !error && (
                <div className="loading-note">Waiting for executed volume…</div>
              )}
            <div
              className="plot-host"
              inert={preparing}
              ref={host}
              tabIndex={0}
              role="region"
              aria-label={
                volumeView
                  ? `${loadedTicker} ${aggregations[aggregation].name} OHLC chart`
                  : `${loadedTicker} depth — scroll or use arrow keys to pan; Home to recenter`
              }
            />
            {!volumeView &&
              !preparing &&
              !error &&
              stats.status !== "Reconstructing" &&
              stats.bid > 0 &&
              Math.abs(
                (stats.ask ? (stats.bid + stats.ask) / 2 : stats.bid) -
                  stats.center,
              ) >
                stats.span / 2 && (
                <button
                  className="outside-view"
                  onClick={() => session.current?.recenter()}
                >
                  Market outside this range · Recenter <Icon kind="focus" />
                </button>
              )}
            {error && (
              <div role="alert" className="error-panel">
                <strong>Replay couldn’t continue</strong>
                <p>{error}</p>
                <button className="outline" onClick={restart}>
                  Try again
                </button>
              </div>
            )}
            {!error &&
              loading &&
              (isLive ? (
                <div className="loading-note">
                  <span className="spinner" />
                  {`${stats.status}…${stats.connectionDetail ? ` ${stats.connectionDetail}` : ""}`}
                </div>
              ) : (
                <div className="loading-note reconstruction-note">
                  <span>Preparing books to {reconstruction.target}</span>
                  <div
                    className="progress-track"
                    role="progressbar"
                    aria-label={`Advance to ${reconstruction.target}`}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={Math.round(reconstruction.percent)}
                    aria-valuetext={
                      reconstruction.clock
                        ? `${clock(reconstruction.clock)} of ${reconstruction.target}`
                        : "Reading session"
                    }
                  >
                    <div style={{ width: `${reconstruction.percent}%` }} />
                  </div>
                  <span className="reconstruction-caption">
                    <span>
                      {reconstruction.clock
                        ? clock(reconstruction.clock)
                        : "Reading session…"}
                    </span>
                    <span>{count.format(stats.messages)} records</span>
                  </span>
                </div>
              ))}
          </div>

          {(simulation || (queue && stats.level === "l3")) && (
            <div className="inspection-dock" inert={preparing}>
              {simulation && (
                <SimulationResults
                  status={simulation}
                  decimals={stats.priceDecimals}
                />
              )}
              {queue && stats.level === "l3" && (
                <LevelQueue
                  status={queue}
                  decimals={stats.priceDecimals}
                  following={!!simulation?.alternateTimeline}
                  onFollow={() => {
                    session.current?.follow_simulated_order();
                    setQueue(
                      JSON.parse(
                        session.current?.queue_status() ?? "null",
                      ) as QueueStatus | null,
                    );
                  }}
                  onClose={() => {
                    session.current?.clear_queue();
                    setQueue(null);
                  }}
                />
              )}
            </div>
          )}
          <footer className="panel-footer">
            <span>
              <span className="local-dot" />
              {volumeView
                ? "Actual execution prices · OHLC"
                : "Shared price axis · aggregated depth"}
            </span>
            <span>
              {!volumeView && (
                <>
                  {stats.level === "l3" && "Click a level for FIFO · "}Scroll
                  price · ↑ ↓ · Home to recenter
                </>
              )}
            </span>
          </footer>
        </section>
      </div>
    </main>
  );
}
