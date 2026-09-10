import test from "node:test";
import assert from "node:assert/strict";
import { gzipSync } from "node:zlib";
import { replayInput, REMOTE_PREFETCH_RANGES } from "../lib/replay-input.mjs";

const open = (
  bytes,
  compressed = true,
  signal = new AbortController().signal,
) =>
  replayInput({
    size: bytes.length,
    compressed,
    signal,
    readAt: async (offset, length) => bytes.subarray(offset, offset + length),
  });
const drain = async (input) => {
  const chunks = [];
  let result;
  do {
    result = await input.read();
    assert.ok(result.bytes.length <= 1048576);
    chunks.push(result.bytes);
  } while (!result.eof);
  return Buffer.concat(chunks);
};

test("gzip and raw sources yield identical bounded input with accurate EOF and byte counters", async () => {
  const raw = Buffer.alloc(3 * 1048576 + 321);
  for (let i = 0; i < raw.length; i++) raw[i] = (i * 103 + (i >> 8)) % 256;
  for (const compressed of [false, true]) {
    const bytes = compressed ? gzipSync(raw) : raw;
    const input = open(bytes, compressed);
    assert.deepEqual(await drain(input), raw);
    assert.equal(input.received, bytes.length);
    assert.equal((await input.read()).eof, true);
  }
});

test("range fetching stops with consumer backpressure and resuming keeps gzip state", async () => {
  const raw = Buffer.alloc(12 * 1048576);
  let seed = 7;
  for (let i = 0; i < raw.length; i++) {
    seed ^= seed << 13;
    seed ^= seed >>> 17;
    seed ^= seed << 5;
    raw[i] = seed & 255;
  }
  const gzip = gzipSync(raw);
  for (const prefetchRanges of [1, REMOTE_PREFETCH_RANGES]) {
    let reads = 0;
    const input = replayInput({
      size: gzip.length,
      compressed: true,
      prefetchRanges,
      signal: new AbortController().signal,
      readAt: async (o, n) => {
        reads++;
        return gzip.subarray(o, o + n);
      },
    });
    const first = await input.read();
    assert.equal(first.eof, false);
    assert.deepEqual(
      Buffer.from(first.bytes),
      raw.subarray(0, first.bytes.length),
    );
    const paused = reads;
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.equal(reads, paused);
    // This incompressible fixture needs part of its second range to decode 1 MiB.
    assert.ok(reads <= prefetchRanges + 1);
    assert.ok(reads < gzip.length / 1048576);
    assert.deepEqual(await drain(input), raw.subarray(first.bytes.length));
  }
});

test("corrupt gzip trailers and truncated raw ranges report errors", async () => {
  const gzip = gzipSync(Buffer.alloc(100000, 7));
  gzip[gzip.length - 8] ^= 255;
  await assert.rejects(drain(open(gzip)));
  await assert.rejects(drain(open(gzip.subarray(0, gzip.length - 8))));
  const input = replayInput({
    size: 4,
    compressed: false,
    signal: new AbortController().signal,
    readAt: async () => new Uint8Array(3),
  });
  await assert.rejects(input.read(), /advertised size/);
});

test("abort stops decoding and further reads", async () => {
  const abort = new AbortController();
  const input = open(
    gzipSync(Buffer.alloc(3 * 1048576, 7)),
    true,
    abort.signal,
  );
  await input.read();
  abort.abort();
  await assert.rejects(input.read(), { name: "AbortError" });
});

test("gzip input failures reject pending output reads without hanging", async () => {
  const input = replayInput({
    size: 1048576,
    compressed: true,
    signal: new AbortController().signal,
    readAt: async () => {
      throw Error("Remote source disconnected");
    },
  });
  await assert.rejects(input.read(), /Remote source disconnected/);
});

test("empty gzip output closes cleanly and truncated headers reject", async () => {
  const empty = open(gzipSync(Buffer.alloc(0)));
  assert.deepEqual(await drain(empty), Buffer.alloc(0));
  assert.equal((await empty.read()).eof, true);
  await assert.rejects(drain(open(new Uint8Array([31, 139, 8]))));
});

const turn = () => new Promise((resolve) => setImmediate(resolve));
const chunkBytes = 1048576;

test("remote read-ahead overlaps requests, preserves order and stays bounded while paused", async () => {
  const raw = Buffer.alloc(12 * chunkBytes + 321);
  for (let i = 0; i < 13; i++)
    raw.fill(i, i * chunkBytes, Math.min((i + 1) * chunkBytes, raw.length));
  const requests = [];
  let releaseImmediately = false;
  const input = replayInput({
    size: raw.length,
    compressed: false,
    signal: new AbortController().signal,
    prefetchRanges: REMOTE_PREFETCH_RANGES,
    readAt: (offset, length) => {
      const bytes = raw.subarray(offset, offset + length);
      return new Promise((resolve) => {
        requests.push({ offset, length, release: () => resolve(bytes) });
        if (releaseImmediately) resolve(bytes);
      });
    },
  });
  let firstDone = false;
  const first = input.read().then((result) => {
    firstDone = true;
    return result;
  });
  await turn();
  assert.equal(requests.length, REMOTE_PREFETCH_RANGES);
  for (const request of requests.slice(1).reverse()) request.release();
  await turn();
  assert.equal(firstDone, false, "later ranges must not overtake the first");
  assert.equal(input.received, 7 * chunkBytes);
  requests[0].release();
  assert.deepEqual(
    Buffer.from((await first).bytes),
    raw.subarray(0, chunkBytes),
  );
  await turn();
  assert.equal(requests.length, REMOTE_PREFETCH_RANGES);
  assert.equal(input.received, REMOTE_PREFETCH_RANGES * chunkBytes);
  releaseImmediately = true;
  assert.deepEqual(await drain(input), raw.subarray(chunkBytes));
  assert.deepEqual(
    requests.map(({ offset }) => offset),
    Array.from({ length: 13 }, (_, i) => i * chunkBytes),
  );
  assert.equal(requests.at(-1).length, 321);
  assert.equal(input.received, raw.length);
});

test("out-of-order remote gzip ranges decode identically to local input", async () => {
  const raw = Buffer.alloc(3 * chunkBytes + 321);
  let seed = 7;
  for (let i = 0; i < raw.length; i++) {
    seed ^= seed << 13;
    seed ^= seed >>> 17;
    seed ^= seed << 5;
    raw[i] = seed & 255;
  }
  const gzip = gzipSync(raw);
  const releases = [];
  const input = replayInput({
    size: gzip.length,
    compressed: true,
    signal: new AbortController().signal,
    prefetchRanges: REMOTE_PREFETCH_RANGES,
    readAt: (offset, length) =>
      new Promise((resolve) => {
        releases.push(() => resolve(gzip.subarray(offset, offset + length)));
      }),
  });
  const remote = drain(input);
  await turn();
  assert.equal(releases.length, 4);
  for (const release of releases.reverse()) release();
  assert.deepEqual(await remote, await drain(open(gzip)));
  assert.equal(input.received, gzip.length);
});

test("speculative failures are handled immediately and reported at the affected range", async () => {
  for (const truncated of [false, true]) {
    const input = replayInput({
      size: 3 * chunkBytes,
      compressed: false,
      signal: new AbortController().signal,
      prefetchRanges: REMOTE_PREFETCH_RANGES,
      readAt: async (offset, length) => {
        if (offset === chunkBytes) {
          if (truncated) return new Uint8Array(length - 1);
          throw Error("Remote range failed");
        }
        return new Uint8Array(length);
      },
    });
    assert.equal((await input.read()).bytes.length, chunkBytes);
    await turn();
    await assert.rejects(
      input.read(),
      truncated ? /advertised size/ : /Remote range failed/,
    );
  }
});

test("abort cancels every outstanding remote request without refilling the queue", async () => {
  const abort = new AbortController();
  let requests = 0,
    cancelled = 0;
  const input = replayInput({
    size: 20 * chunkBytes,
    compressed: true,
    signal: abort.signal,
    prefetchRanges: REMOTE_PREFETCH_RANGES,
    readAt: () => {
      requests++;
      return new Promise((_, reject) => {
        abort.signal.addEventListener(
          "abort",
          () => {
            cancelled++;
            reject(abort.signal.reason);
          },
          { once: true },
        );
      });
    },
  });
  const first = input.read();
  const rejected = assert.rejects(first, { name: "AbortError" });
  await turn();
  assert.equal(requests, REMOTE_PREFETCH_RANGES);
  abort.abort();
  await rejected;
  await turn();
  assert.equal(cancelled, requests);
  assert.equal(requests, REMOTE_PREFETCH_RANGES);
  assert.equal(input.received, 0);
  await assert.rejects(input.read(), { name: "AbortError" });
});
