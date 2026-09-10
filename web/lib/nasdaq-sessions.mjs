export const NASDAQ_DIRECTORY = "https://emi.nasdaq.com/ITCH/Nasdaq%20ITCH/";
export const INPUT_CHUNK_BYTES = 1048576;
const MAX_DIRECTORY_BYTES = 1048576;

// Nasdaq's IIS directory contains sidecars and older protocols as well as ITCH
// 5.0 sessions. Keep names from the listing; never synthesize session URLs/dates.
export function parseSessions(html) {
  const sessions = new Map();
  const base = new URL(NASDAQ_DIRECTORY);
  for (const match of html.matchAll(
    /(\d+)\s*<a\s+href="([^"]+)"[^>]*>[^<]*<\/a>/gi,
  )) {
    const url = new URL(match[2], base);
    const name = decodeURIComponent(url.pathname.slice(base.pathname.length));
    if (
      url.origin !== base.origin ||
      !url.pathname.startsWith(base.pathname) ||
      url.search ||
      url.hash
    )
      continue;
    if (
      !/^(?:\d{8}\.NASDAQ_ITCH50|S\d{6}-v50\.txt|itch50_[\w-]+)\.gz$/i.test(
        name,
      )
    )
      continue;
    const size = Number(match[1]);
    if (Number.isSafeInteger(size) && size > 0)
      sessions.set(name, { name, size });
  }
  return [...sessions.values()].sort((a, b) => a.name.localeCompare(b.name));
}

async function boundedBytes(response, limit) {
  if (!response.body) throw Error("Empty Nasdaq response");
  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      size += value.length;
      if (size > limit)
        throw Error("Nasdaq response exceeds the requested size");
      chunks.push(value);
    }
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error;
  } finally {
    reader.releaseLock();
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return bytes;
}

export function createNasdaqHandler(fetcher = fetch) {
  let cached,
    expires = 0,
    pending;
  const catalog = async () => {
    if (cached && Date.now() < expires) return cached;
    pending ??= (async () => {
      const response = await fetcher(NASDAQ_DIRECTORY, {
        signal: AbortSignal.timeout(15000),
        redirect: "error",
        cache: "no-store",
      });
      if (!response.ok) {
        await response.body?.cancel();
        throw Error("Nasdaq session directory is unavailable");
      }
      const list = parseSessions(
        new TextDecoder().decode(
          await boundedBytes(response, MAX_DIRECTORY_BYTES),
        ),
      );
      if (!list.length)
        throw Error("No ITCH 5.0 gzip sessions are listed by Nasdaq");
      cached = list;
      expires = Date.now() + 300000;
      return list;
    })().finally(() => {
      pending = undefined;
    });
    return pending;
  };
  return async (request) => {
    const query = new URL(request.url).searchParams;
    const name = query.get("file");
    const offset = Number(query.get("offset") ?? 0);
    const length = Number(query.get("length") ?? INPUT_CHUNK_BYTES);
    if (
      name !== null &&
      (!/^[\w.-]+\.gz$/i.test(name) ||
        !Number.isSafeInteger(offset) ||
        offset < 0 ||
        !Number.isSafeInteger(length) ||
        length < 1 ||
        length > INPUT_CHUNK_BYTES)
    )
      return new Response("Invalid Nasdaq session or byte range", {
        status: 400,
      });
    try {
      const list = await catalog();
      if (name === null)
        return Response.json(list, {
          headers: { "Cache-Control": "public, max-age=300" },
        });
      const session = list.find((s) => s.name === name);
      if (!session)
        return new Response("Session is not in the Nasdaq ITCH 5.0 directory", {
          status: 404,
        });
      if (offset >= session.size)
        return new Response("Byte range is past the session end", {
          status: 416,
        });
      const end = Math.min(offset + length, session.size) - 1;
      const abort = new AbortController();
      const response = await fetcher(
        new URL(encodeURIComponent(name), NASDAQ_DIRECTORY),
        {
          headers: {
            Range: `bytes=${offset}-${end}`,
            "Accept-Encoding": "identity",
          },
          signal: AbortSignal.any([
            request.signal,
            abort.signal,
            AbortSignal.timeout(30000),
          ]),
          redirect: "error",
          cache: "no-store",
        },
      );
      // A server ignoring Range must never turn this request into a full download.
      if (
        response.status !== 206 ||
        response.headers.get("Content-Range") !==
          `bytes ${offset}-${end}/${session.size}`
      ) {
        abort.abort();
        await response.body?.cancel().catch(() => {});
        return new Response(
          "Nasdaq did not return the requested session byte range",
          { status: 502 },
        );
      }
      const bytes = await boundedBytes(response, end - offset + 1);
      if (bytes.length !== end - offset + 1)
        throw Error("Nasdaq session range was truncated");
      return new Response(bytes, {
        headers: {
          "Content-Type": "application/octet-stream",
          "Cache-Control": "no-store",
          "Content-Length": String(bytes.length),
        },
      });
    } catch (error) {
      return new Response(
        error instanceof Error
          ? error.message
          : "Cannot read the Nasdaq sample session",
        { status: 502 },
      );
    }
  };
}
