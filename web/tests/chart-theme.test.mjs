import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import test from "node:test";
import ts from "typescript";

// Exercise the browser boundary without a DOM or GPU, including production CSS
// shorthand. Use the actual TS module rather than duplicating its packing logic.
const source = ts.transpileModule(
  readFileSync(new URL("../lib/chart-theme.ts", import.meta.url), "utf8"),
  {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  },
).outputText;
function theme(tokens) {
  const exports = {};
  vm.runInNewContext(source, {
    exports,
    Float32Array,
    getComputedStyle: () => ({
      getPropertyValue: (key) => tokens[key] ?? "#123456",
    }),
  });
  return exports.chartTheme({});
}

test("production shorthand and full hex colors pack into the same GPU palette", () => {
  const expanded = theme({
    "--canvas": " #ffffff ",
    "--chart-grid": "#aabbcc",
  });
  const minified = theme({ "--canvas": "#fff", "--chart-grid": "#abc" });
  assert.deepEqual(minified.gpu, expanded.gpu);
  assert.equal(minified.gpu.byteLength, 208);
  assert.deepEqual(Array.from(minified.gpu.slice(0, 4)), [1, 1, 1, 1]);
  assert.equal(minified.gpu[8], Math.fround(170 / 255));
  assert.equal(minified.gpu[11], 1);
});

test("invalid palette tokens fail before reaching the WASM uniform", () => {
  assert.throws(
    () => theme({ "--chart-ask": "" }),
    /Invalid chart color: --chart-ask/,
  );
  assert.throws(
    () => theme({ "--canvas": "#ggg" }),
    /Invalid chart color: --canvas/,
  );
});
