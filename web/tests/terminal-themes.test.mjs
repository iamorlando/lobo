import test from "node:test";
import assert from "node:assert/strict";
import { parseTheme, themeTokens, themeUrl } from "../lib/terminal-themes.mjs";
const theme = {
  background: "#101010",
  foreground: "#e0e0e0",
  selection: "#404040",
  ansi: [
    "#101010",
    "#c02030",
    "#20a060",
    "#f0c040",
    "#4080b0",
    "#b06090",
    "#40b0b0",
    "#e0e0e0",
  ],
  brights: [
    "#808080",
    "#ff4050",
    "#40c080",
    "#ffe060",
    "#60a0d0",
    "#d080b0",
    "#60d0d0",
    "#ffffff",
  ],
};
test("upstream TOML parsing accepts multiline arrays without executing configuration", () => {
  const source = `background = "${theme.background}"\nforeground = '${theme.foreground}'\nselection_bg = "${theme.selection}"\nansi = [\n${theme.ansi.map((c) => `"${c}"`).join(",\n")}\n]\nbrights = ${JSON.stringify(theme.brights)}`;
  assert.deepEqual(parseTheme(source), theme);
  assert.throws(() =>
    parseTheme(source.replace(theme.background, "red; url(evil)")),
  );
  assert.throws(() => parseTheme(source.replace(theme.ansi[1], "#123")));
});
test("both light and dark palettes map chart, queue, loading and background colors", () => {
  const dark = themeTokens(theme);
  const light = themeTokens({
    ...theme,
    background: "#ffffff",
    foreground: "#101010",
  });
  assert.equal(dark.mode, "dark");
  assert.equal(light.mode, "light");
  for (const palette of [dark, light]) {
    assert.equal(palette.tokens["--chart-bid"], theme.ansi[2]);
    assert.equal(palette.tokens["--chart-ask-high"], theme.brights[1]);
    assert.equal(palette.tokens["--loader-accent"], theme.brights[3]);
    assert.notEqual(
      palette.tokens["--canvas"],
      palette.tokens["--canvas-bottom"],
    );
    for (const color of Object.values(palette.tokens))
      assert.match(color, /^#[0-9a-f]{6}$/);
  }
  assert.equal(
    new URL(themeUrl("Espresso Libre")).host,
    "raw.githubusercontent.com",
  );
});
