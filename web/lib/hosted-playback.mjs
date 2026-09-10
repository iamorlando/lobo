/** Transport choice does not determine whether a feed is live or recorded. */
export function sourceMode(choice, adapter) {
  return choice.kind === "live" ? (adapter?.mode ?? "live") : "replay";
}

export async function hostedPlayback(endpoint, command, signal) {
  const response = await fetch(endpoint, {
    method: command ? "POST" : "GET",
    cache: "no-store",
    headers: { "Content-Type": "application/json" },
    body: command ? JSON.stringify(command) : undefined,
    signal,
  });
  const result = await response.json();
  if (!response.ok)
    throw new Error(
      result.error ?? `Playback request failed (${response.status})`,
    );
  return result;
}

/** START AT uses the same UTC/source-day clock as the terminal display. */
export function seekTimestamp(time, clockNs) {
  if (!/^\d{2}:\d{2}(:\d{2})?$/.test(time))
    throw new Error("Enter a valid source time");
  const [hours, minutes, seconds = 0] = time.split(":").map(Number);
  if (hours > 23 || minutes > 59 || seconds > 59)
    throw new Error("Enter a valid source time");
  const dayNs = 86400e9;
  return (
    Math.floor(clockNs / dayNs) * dayNs +
    (hours * 3600 + minutes * 60 + seconds) * 1e9
  );
}
