export const SPEEDS = [1, 5, 10, 100, 1000];
export const speedLabel = (speed) => (speed === 1 ? "Real-time" : `${speed}×`);

export class ReplayClock {
  elapsed = 0;
  followSource(session) {
    // A completed simulation can display an older, frozen clock.
    this.elapsed = Math.max(0, session.source_clock_ms - session.start_ms);
  }
  target(session, dt, speed, playing) {
    // Reconstruct to the selected start before advancing the playback clock.
    if (session.warming) return 0;
    // RAF timestamps can precede the last cooperative task's wall clock.
    if (playing && !session.complete) this.elapsed += Math.max(0, dt) * speed;
    return this.elapsed;
  }
}

// Catch up to the timestamp horizon within a short cooperative work slice.
// Even at 1000×, the native adapter stops before records beyond that horizon.
export async function replaySlice(
  session,
  target,
  read,
  stopped,
  now = () => performance.now(),
) {
  // Startup reconstructs to the configured start, without drawing or pacing.
  // A finite zero offset stops before the first record beyond that start.
  const warming = session.warming;
  return advanceSlice(
    session,
    warming ? 0 : target,
    read,
    stopped,
    now,
    warming
      ? (target, budget) => session.advance_without_render(target, budget)
      : (target, budget) => session.advance(target, budget),
  );
}

async function advanceSlice(session, target, read, stopped, now, advance) {
  const begin = now();
  const warming = session.warming;
  do {
    if (stopped() || session.complete) return;
    if (session.needs_input) await read();
    if (stopped()) return;
    const consumed = session.bytes_consumed;
    advance(target, 20000);
    if (warming && !session.warming) return;
    if (
      !session.needs_input &&
      !session.warming &&
      (session.source_clock_ms >= session.start_ms + target ||
        session.bytes_consumed === consumed)
    )
      return;
  } while (now() - begin < 8 && !session.complete);
}
