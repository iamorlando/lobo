import test from "node:test";
import assert from "node:assert/strict";
import {
  createNasdaqHandler,
  parseSessions,
  NASDAQ_DIRECTORY,
  INPUT_CHUNK_BYTES,
} from "../lib/nasdaq-sessions.mjs";
const name = "01302019.NASDAQ_ITCH50.gz";
const entry = (name, size = 2000000) =>
  `1/31/2019 1:18 AM ${size} <A HREF="/ITCH/Nasdaq%20ITCH/${name}">${name}</A><br>`;
const request = (query) =>
  new Request(
    `http://localhost/api/nasdaq-sessions${query ? `?${query}` : ""}`,
  );

test("Nasdaq directory discovers all ITCH 5.0 naming conventions and excludes sidecars and other formats", () => {
  const html =
    [
      name,
      "S061226-v50.txt.gz",
      "itch50_05_15.gz",
      "tvagg.gz",
      `${name}.md5sum`,
      "S061226-v50.txt.done",
      "S010303-v2.zip",
    ]
      .map((n) => entry(n))
      .join("") +
    `12 <a href="https://example.com/ITCH/Nasdaq%20ITCH/${name}">external</a>`;
  assert.deepEqual(
    parseSessions(html),
    [name, "itch50_05_15.gz", "S061226-v50.txt.gz"].map((name) => ({
      name,
      size: 2000000,
    })),
  );
});

test("catalog is cached; session reads request and return only exact bounded gzip bytes", async () => {
  let listings = 0;
  const offsets = [];
  const handler = createNasdaqHandler(async (url, options) => {
    if (String(url) === NASDAQ_DIRECTORY) {
      listings++;
      return new Response(entry(name));
    }
    assert.equal(String(url), `${NASDAQ_DIRECTORY}${name}`);
    assert.equal(options.redirect, "error");
    assert.equal(options.headers["Accept-Encoding"], "identity");
    offsets.push(options.headers.Range);
    return new Response(new Uint8Array([31, 139, 8, 0]), {
      status: 206,
      headers: { "Content-Range": "bytes 5-8/2000000" },
    });
  });
  assert.equal((await handler(request())).status, 200);
  assert.equal((await handler(request())).status, 200);
  const response = await handler(request(`file=${name}&offset=5&length=4`));
  assert.deepEqual(
    new Uint8Array(await response.arrayBuffer()),
    new Uint8Array([31, 139, 8, 0]),
  );
  assert.equal(listings, 1);
  assert.deepEqual(offsets, ["bytes=5-8"]);
  for (const query of [
    `file=../${name}`,
    `file=https://example.com/a.gz`,
    `file=${name}&offset=-1`,
    `file=${name}&length=${INPUT_CHUNK_BYTES + 1}`,
    `file=${name}&length=0`,
  ])
    assert.equal((await handler(request(query))).status, 400);
  assert.equal((await handler(request("file=unlisted.gz"))).status, 404);
  assert.equal(
    (await handler(request(`file=${name}&offset=2000000`))).status,
    416,
  );
  assert.equal(offsets.length, 1);
});

test("range-ignoring, mismatched and truncated responses fail without reading a whole session", async () => {
  for (const response of [
    new Response("too much"),
    new Response("abcd", {
      status: 206,
      headers: { "Content-Range": "bytes 0-3/999" },
    }),
    new Response("abc", {
      status: 206,
      headers: { "Content-Range": "bytes 0-3/2000000" },
    }),
    new Response("abcde", {
      status: 206,
      headers: { "Content-Range": "bytes 0-3/2000000" },
    }),
  ]) {
    const handler = createNasdaqHandler(async (url) =>
      String(url) === NASDAQ_DIRECTORY ? new Response(entry(name)) : response,
    );
    assert.equal((await handler(request(`file=${name}&length=4`))).status, 502);
  }
});

test("directory failures are reported and a failed lookup can be retried", async () => {
  let calls = 0;
  const handler = createNasdaqHandler(async () =>
    ++calls === 1
      ? new Response("offline", { status: 503 })
      : new Response(entry(name)),
  );
  assert.equal((await handler(request())).status, 502);
  assert.equal((await handler(request())).status, 200);
  const huge = createNasdaqHandler(
    async () => new Response("x".repeat(1048577)),
  );
  assert.equal((await huge(request())).status, 502);
});

test("aborting a client range request cancels the upstream fetch", async () => {
  const abort = new AbortController();
  let upstream;
  const handler = createNasdaqHandler(async (url, options) => {
    if (String(url) === NASDAQ_DIRECTORY) return new Response(entry(name));
    upstream = options.signal;
    return new Promise((_, reject) =>
      options.signal.addEventListener(
        "abort",
        () => reject(options.signal.reason),
        { once: true },
      ),
    );
  });
  const pending = handler(
    new Request(`http://localhost/api/nasdaq-sessions?file=${name}`, {
      signal: abort.signal,
    }),
  );
  while (!upstream) await new Promise((resolve) => setImmediate(resolve));
  abort.abort();
  assert.equal((await pending).status, 502);
  assert.equal(upstream.aborted, true);
});
