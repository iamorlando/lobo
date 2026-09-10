import type { Session } from "./wasm";
export type ReplaySpeed = number;
export const SPEEDS: ReplaySpeed[];
export function speedLabel(speed: ReplaySpeed): string;
export class ReplayClock {
  elapsed: number;
  followSource(session: Pick<Session, "source_clock_ms" | "start_ms">): void;
  target(
    session: Pick<Session, "warming" | "complete">,
    dt: number,
    speed: ReplaySpeed,
    playing: boolean,
  ): number;
}
export function replaySlice(
  session: Pick<
    Session,
    | "complete"
    | "needs_input"
    | "warming"
    | "advance"
    | "advance_without_render"
    | "source_clock_ms"
    | "start_ms"
    | "bytes_consumed"
  >,
  target: number,
  read: () => Promise<void>,
  stopped: () => boolean,
  now?: () => number,
): Promise<void>;
