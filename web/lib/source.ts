import type { AdapterInfo } from "./wasm";
import type { NasdaqSession } from "./nasdaq-sessions.mjs";
import {
  replayInput,
  REMOTE_PREFETCH_RANGES,
  type ReplayInput,
} from "./replay-input.mjs";
export type SourceChoice =
  | { kind: "nasdaq"; session: NasdaqSession | null }
  | { kind: "repository" }
  | { kind: "file"; file: File }
  | { kind: "live"; adapterId: string };
export interface ReplaySource extends ReplayInput {
  adapter: AdapterInfo;
  name: string;
}
export async function openSource(
  choice: SourceChoice,
  signal: AbortSignal,
  adapters: AdapterInfo[],
): Promise<ReplaySource> {
  const adapter =
    choice.kind === "live"
      ? adapters.find((a) => a.id === choice.adapterId)
      : adapters.find((a) => a.mode === "replay");
  if (!adapter) throw new Error("Source adapter is unavailable");
  if (choice.kind === "live")
    return {
      adapter,
      name: `${adapter.name} · ${adapter.mode} feed`,
      size: 0,
      received: 0,
      compressed: false,
      read: async () => {
        throw new Error("Live sources use a socket transport");
      },
    };
  const source = (
    name: string,
    size: number,
    readAt: (offset: number, length: number) => Promise<Uint8Array>,
  ) =>
    Object.assign(
      replayInput({
        size,
        compressed: /\.gz$/i.test(name),
        readAt,
        signal,
        prefetchRanges: choice.kind === "file" ? 1 : REMOTE_PREFETCH_RANGES,
      }),
      { adapter, name },
    );
  if (choice.kind === "file") {
    const file = choice.file;
    return source(
      file.name,
      file.size,
      async (offset, length) =>
        new Uint8Array(await file.slice(offset, offset + length).arrayBuffer()),
    );
  }
  const nasdaq = choice.kind === "nasdaq" ? choice.session : null;
  if (choice.kind === "nasdaq" && !nasdaq)
    throw Error("Select a Nasdaq sample session");
  const url = nasdaq
    ? `/api/nasdaq-sessions?file=${encodeURIComponent(nasdaq.name)}`
    : "/api/replay";
  let info = nasdaq;
  if (!info) {
    const response = await fetch(`${url}?info`, { signal });
    if (!response.ok) throw new Error(await response.text());
    info = (await response.json()) as NasdaqSession;
  }
  return source(info.name, info.size, async (offset, length) => {
    const response = await fetch(
      `${url}${nasdaq ? "&" : "?"}offset=${offset}&length=${length}`,
      { signal, cache: "no-store" },
    );
    if (!response.ok) throw new Error(await response.text());
    return new Uint8Array(await response.arrayBuffer());
  });
}
