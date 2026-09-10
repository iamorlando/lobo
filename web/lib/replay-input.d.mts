export interface ReplayInput {
  size: number;
  compressed: boolean;
  readonly received: number;
  read(): Promise<{ bytes: Uint8Array; eof: boolean }>;
}
export const REMOTE_PREFETCH_RANGES: 8;
export function replayInput(options: {
  size: number;
  compressed: boolean;
  readAt(offset: number, length: number): Promise<Uint8Array>;
  signal: AbortSignal;
  prefetchRanges?: number;
}): ReplayInput;
