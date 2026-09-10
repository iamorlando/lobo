import type { AdapterInfo } from "./wasm";
import type { SourceChoice } from "./source";
export interface PlaybackStatus {
  paused: boolean;
  speed: number;
  clock_ns: number;
  start_ns: number | null;
  complete: boolean;
  seeking: boolean;
  error: string | null;
}
export type PlaybackCommand =
  | { action: "play" | "pause" | "restart" }
  | { action: "speed"; speed: number }
  | { action: "seek"; timestamp_ns: number };
export function sourceMode(
  choice: SourceChoice,
  adapter?: AdapterInfo,
): AdapterInfo["mode"];
export function hostedPlayback(
  endpoint: string,
  command?: PlaybackCommand,
  signal?: AbortSignal,
): Promise<PlaybackStatus>;
export function seekTimestamp(time: string, clockNs: number): number;
