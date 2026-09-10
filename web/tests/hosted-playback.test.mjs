import test from "node:test";
import assert from "node:assert/strict";
import {
  hostedPlayback,
  seekTimestamp,
  sourceMode,
} from "../lib/hosted-playback.mjs";

test("hosted socket sources respect the declared mode", () => {
  const choice = { kind: "live", adapterId: "custom-0" };
  assert.equal(sourceMode(choice, { mode: "replay" }), "replay");
  assert.equal(sourceMode(choice, { mode: "live" }), "live");
  assert.equal(sourceMode({ kind: "file" }), "replay");
});

test("seeks use the recording day and source clock", () => {
  assert.equal(seekTimestamp("09:30:01", 34200e9), 34201e9);
  const day = Date.UTC(2024, 0, 2) * 1e6;
  assert.equal(seekTimestamp("09:30:01", day + 34200e9), day + 34201e9);
  for (const value of ["", "25:00", "09:65", "09:30:61"])
    assert.throws(() => seekTimestamp(value, 0), /valid source time/);
});

test("public playback endpoint receives controls and propagates failures", async (t) => {
  const calls = [];
  t.mock.method(globalThis, "fetch", async (url, options) => {
    calls.push({ url, ...options });
    return {
      ok: calls.length < 3,
      json: async () =>
        calls.length < 3 ? { paused: true } : { error: "Invalid speed" },
    };
  });
  assert.deepEqual(await hostedPlayback("/api/playback"), { paused: true });
  await hostedPlayback("/api/playback", { action: "pause" });
  assert.equal(calls[0].method, "GET");
  assert.equal(calls[1].method, "POST");
  assert.deepEqual(JSON.parse(calls[1].body), { action: "pause" });
  await assert.rejects(
    hostedPlayback("/api/playback", { action: "speed", speed: 0 }),
    /Invalid speed/,
  );
});
