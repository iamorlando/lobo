export interface NasdaqSession {
  name: string;
  size: number;
}
export const NASDAQ_DIRECTORY: string;
export const INPUT_CHUNK_BYTES: number;
export function parseSessions(html: string): NasdaqSession[];
export function createNasdaqHandler(
  fetcher?: typeof fetch,
): (request: Request) => Promise<Response>;
