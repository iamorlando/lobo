// The catalog and theme files stay on the upstream project's GitHub CDN.
export const themeRepository =
  "https://github.com/mbadolato/iTerm2-Color-Schemes";
export const themeCatalogUrl =
  "https://api.github.com/repos/mbadolato/iTerm2-Color-Schemes/contents/wezterm";
export const themeUrl = (name) =>
  `https://raw.githubusercontent.com/mbadolato/iTerm2-Color-Schemes/master/wezterm/${encodeURIComponent(name)}.toml`;
export function parseTheme(source) {
  const color = (key) => {
    const match = source.match(
      new RegExp(`^${key}\\s*=\\s*["'](#[0-9a-f]{6})["']`, "mi"),
    );
    if (!match) throw Error(`Theme is missing ${key}`);
    return match[1].toLowerCase();
  };
  const array = (key) => {
    const value = source.match(
      new RegExp(`^${key}\\s*=\\s*\\[([^\\]]+)\\]`, "mi"),
    );
    const colors = value?.[1].match(/#[0-9a-f]{6}/gi);
    if (colors?.length !== 8) throw Error(`Theme needs eight ${key} colors`);
    return colors.map((c) => c.toLowerCase());
  };
  return {
    background: color("background"),
    foreground: color("foreground"),
    selection: color("selection_bg"),
    ansi: array("ansi"),
    brights: array("brights"),
  };
}
const rgb = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
export const mix = (a, b, weight) =>
  "#" +
  rgb(a)
    .map((v, i) =>
      Math.round(v + (rgb(b)[i] - v) * weight)
        .toString(16)
        .padStart(2, "0"),
    )
    .join("");
export const luminance = (hex) =>
  rgb(hex)
    .map((v) => v / 255)
    .map((v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4))
    .reduce((sum, v, i) => sum + v * [0.2126, 0.7152, 0.0722][i], 0);
const contrast = (a, b) =>
  (Math.max(luminance(a), luminance(b)) + 0.05) /
  (Math.min(luminance(a), luminance(b)) + 0.05);
/** Every semantic color derives from terminal colors, including gradient endpoints. */
export function themeTokens(theme) {
  const { background: bg, foreground: fg, ansi: a, brights: b } = theme;
  const light = luminance(bg) > 0.45;
  const ink = (background) =>
    contrast(bg, background) > contrast(fg, background) ? bg : fg;
  const readable = (color) => {
    for (let step = 0; step <= 10; step++) {
      const candidate = mix(color, fg, step / 10);
      if (contrast(candidate, bg) >= 4.5) return candidate;
    }
    return fg;
  };
  const primary = readable(a[3]),
    green = readable(a[2]),
    red = readable(a[1]),
    simulation = readable(a[5]);
  const low = (color) => mix(bg, color, light ? 0.6 : 0.2);
  const tokens = {
    bg,
    canvas: bg,
    "canvas-top": mix(bg, a[5], 0.12),
    "canvas-bottom": mix(bg, a[3], 0.16),
    panel: mix(bg, fg, 0.035),
    "panel-raised": mix(bg, fg, 0.065),
    "panel-active": mix(bg, fg, 0.1),
    line: mix(bg, fg, 0.16),
    "line-strong": mix(bg, fg, 0.28),
    text: fg,
    muted: readable(mix(bg, fg, 0.52)),
    primary,
    "primary-solid": a[3],
    "primary-active": mix(a[3], fg, 0.15),
    "primary-dim": mix(bg, a[3], 0.13),
    "on-primary": ink(a[3]),
    green,
    "green-action": a[2],
    "on-green": ink(a[2]),
    "green-dim": mix(bg, a[2], 0.12),
    red,
    "red-dim": mix(bg, a[1], 0.12),
    simulation,
    "simulation-ink": ink(simulation),
    "queue-ink": fg,
    "queue-bid-low": low(a[2]),
    "queue-bid-high": mix(a[2], bg, 0.35),
    "queue-ask-low": low(a[1]),
    "queue-ask-high": mix(a[1], bg, 0.35),
    "chart-panel": mix(bg, fg, 0.025),
    "chart-grid": mix(bg, fg, 0.085),
    "chart-bid": a[2],
    "chart-ask": a[1],
    "chart-bid-low": low(a[2]),
    "chart-ask-low": low(a[1]),
    "chart-bid-high": b[2],
    "chart-ask-high": b[1],
    "loader-accent": b[3],
    selection: theme.selection,
  };
  return {
    mode: light ? "light" : "dark",
    tokens: Object.fromEntries(
      Object.entries(tokens).map(([key, value]) => [`--${key}`, value]),
    ),
  };
}
