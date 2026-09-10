/** Canvas labels and FIFO segments share the terminal's CSS design tokens. */
export function chartTheme(element: Element) {
  const style = getComputedStyle(element);
  const token = (name: string) => style.getPropertyValue(name).trim();
  // Thirteen RGBA vectors match the WGSL Theme uniform. Resolved CSS tokens are
  // the single app palette; this tiny upload never touches chart history.
  const gpu = new Float32Array(
    [
      "--canvas",
      "--chart-panel",
      "--chart-grid",
      "--chart-bid",
      "--chart-ask",
      "--chart-bid-low",
      "--chart-ask-low",
      "--chart-bid-high",
      "--chart-ask-high",
      "--canvas-top",
      "--canvas-bottom",
      "--primary-solid",
      "--loader-accent",
    ].flatMap((name) => {
      // Production CSS minifies six-digit tokens such as #ffffff to #fff.
      const raw = token(name);
      const hex = /^#[0-9a-f]{3}$/i.test(raw)
        ? "#" + [...raw.slice(1)].map((digit) => digit + digit).join("")
        : raw;
      if (!/^#[0-9a-f]{6}$/i.test(hex))
        throw new Error(`Invalid chart color: ${name}`);
      return [1, 3, 5]
        .map((offset) => parseInt(hex.slice(offset, offset + 2), 16) / 255)
        .concat(1);
    }),
  );
  return {
    gpu,
    font: token("--font-terminal"),
    canvas: token("--canvas"),
    panel: token("--panel"),
    text: token("--text"),
    muted: token("--muted"),
    line: token("--line"),
    primary: token("--primary"),
    bid: token("--green"),
    ask: token("--red"),
    simulation: token("--simulation"),
    queueInk: token("--queue-ink"),
    simulationInk: token("--simulation-ink"),
    bidLow: token("--queue-bid-low"),
    askLow: token("--queue-ask-low"),
    bidHigh: token("--queue-bid-high"),
    askHigh: token("--queue-ask-high"),
  };
}
