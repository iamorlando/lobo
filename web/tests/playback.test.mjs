import test from "node:test";
import assert from "node:assert/strict";
import {
  ReplayClock,
  replaySlice,
  SPEEDS,
  speedLabel,
} from "../lib/playback.mjs";

const sessionAtStart = () => ({
  warming: false,
  complete: false,
  needs_input: false,
  clock_ms: 34200000,
  source_clock_ms: 34200000,
  start_ms: 34200000,
  bytes_consumed: 0,
});

test("1000× advances a finite timestamp horizon and speed changes preserve position", () => {
  const clock = new ReplayClock();
  const session = sessionAtStart();
  session.warming = true;
  assert.equal(clock.target(session, 10, 1000, true), 0);
  session.warming = false;
  assert.equal(clock.target(session, 16, 1000, true), 16000);
  // Pausing, a frozen simulation display, and changing speeds cannot rewind it.
  assert.equal(clock.target(session, 100, 1000, false), 16000);
  assert.equal(clock.target(session, 100, 5, true), 16500);
  assert.equal(clock.target(session, -3, 5, true), 16500);
  session.complete = true;
  assert.equal(clock.target(session, 100, 1000, true), 16500);
  assert.deepEqual(SPEEDS.map(speedLabel), [
    "Real-time",
    "5×",
    "10×",
    "100×",
    "1000×",
  ]);
});

test("a dense paced replay drains multiple budgets but stops at the source horizon", async () => {
  let calls = 0;
  const session = {
    ...sessionAtStart(),
    advance(target, budget) {
      assert.equal(budget, 20000);
      assert.equal(target, 15000);
      this.bytes_consumed += 100;
      this.source_clock_ms += 5000;
      calls++;
      // The displayed simulation clock intentionally stays frozen.
    },
  };
  await replaySlice(
    session,
    15000,
    async () => assert.fail("unexpected read"),
    () => false,
    () => 0,
  );
  assert.equal(calls, 3);
  assert.equal(session.clock_ms, session.start_ms);
  assert.equal(session.source_clock_ms, session.start_ms + 15000);
  assert.equal(session.complete, false);
});

test("work yields within a slice, stops at reconstruction boundary, and obeys pause after input", async () => {
  let calls = 0,
    time = 0;
  const session = {
    ...sessionAtStart(),
    advance() {
      calls++;
      this.bytes_consumed++;
    },
  };
  await replaySlice(
    session,
    1,
    async () => {},
    () => false,
    () => time++,
  );
  assert.equal(calls, 8);
  session.warming = true;
  session.advance_without_render = () => {
    calls++;
    session.warming = false;
  };
  await replaySlice(
    session,
    0,
    async () => {},
    () => false,
    () => 0,
  );
  assert.equal(calls, 9);
  session.needs_input = true;
  let stopped = false;
  await replaySlice(
    session,
    1,
    async () => {
      stopped = true;
    },
    () => stopped,
    () => 0,
  );
  assert.equal(calls, 9);
});

test("future records and fractional clock rounding do not spin until the slice deadline", async () => {
  let calls = 0;
  const session = {
    ...sessionAtStart(),
    source_clock_ms: 34200000.123456,
    advance() {
      calls++;
    },
  };
  // WASM truncates the requested f64 milliseconds to integer nanoseconds.
  await replaySlice(
    session,
    0.1234567,
    async () => assert.fail("unexpected read"),
    () => false,
    () => 0,
  );
  assert.equal(calls, 1);
});

test("startup automatically uses undrawn batches, yields, and stops exactly at the configured start", async () => {
  let undrawn = 0,
    drawn = 0,
    time = 0;
  const session = {
    ...sessionAtStart(),
    warming: true,
    source_clock_ms: 33600000,
    advance_without_render(target, budget) {
      assert.equal(
        target,
        0,
        "reconstruction must not advance beyond Start at",
      );
      assert.equal(budget, 20000);
      undrawn++;
      this.bytes_consumed += 500;
      this.source_clock_ms += 60000;
      if (this.source_clock_ms === this.start_ms) this.warming = false;
    },
    advance(target) {
      drawn++;
      this.bytes_consumed++;
      this.source_clock_ms = this.start_ms + target;
    },
  };
  // Even a large incoming playback target cannot skip the selected start.
  await replaySlice(
    session,
    99999999,
    async () => {},
    () => false,
    () => time++,
  );
  assert.equal(undrawn, 8);
  assert.equal(drawn, 0);
  assert.equal(session.warming, true);
  await replaySlice(
    session,
    99999999,
    async () => {},
    () => false,
    () => 0,
  );
  assert.equal(undrawn, 10);
  assert.equal(drawn, 0);
  assert.equal(session.source_clock_ms, session.start_ms);
  const clock = new ReplayClock();
  clock.followSource(session);
  const target = clock.target(session, 10, 5, true);
  await replaySlice(
    session,
    target,
    async () => {},
    () => false,
    () => 0,
  );
  assert.equal(drawn, 1);
  assert.equal(undrawn, 10);
  assert.equal(session.source_clock_ms, session.start_ms + 50);
});

test("reconstruction handles EOF before Start at and cancels safely after awaited input", async () => {
  let calls = 0;
  const session = {
    ...sessionAtStart(),
    warming: true,
    source_clock_ms: 30000000,
    advance_without_render() {
      calls++;
      this.complete = true;
      this.warming = false;
    },
  };
  await replaySlice(
    session,
    0,
    async () => {},
    () => false,
    () => 0,
  );
  assert.equal(calls, 1);
  await replaySlice(
    session,
    0,
    async () => assert.fail("EOF must not read"),
    () => false,
  );
  assert.equal(calls, 1);
  session.complete = false;
  session.warming = true;
  session.needs_input = true;
  let stopped = false;
  await replaySlice(
    session,
    0,
    async () => {
      stopped = true;
    },
    () => stopped,
  );
  assert.equal(calls, 1);
});
