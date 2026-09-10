import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import test from "node:test";
import ts from "typescript";

const exports = {};
vm.runInNewContext(
  ts.transpileModule(
    readFileSync(new URL("../lib/depth-profile.ts", import.meta.url), "utf8"),
    {
      compilerOptions: {
        module: ts.ModuleKind.CommonJS,
        target: ts.ScriptTarget.ES2020,
      },
    },
  ).outputText,
  { exports, Float64Array },
);
const profile = (values, decimals = 0) =>
  exports.depthProfile(new Float64Array(values), decimals);

test("depth accumulates bids toward lower prices and asks toward higher prices", () => {
  const result = profile([5, 0, 10, 0, 20, 1, 0, 30]);
  assert.deepEqual(Array.from(result.cumulative), [35, 0, 30, 0, 20, 1, 0, 31]);
  assert.equal(result.maximum, 35);
});

test("bucket quantity and cumulative depth can move in opposite directions", () => {
  const before = profile([100, 0, 150, 0, 0, 500]);
  const after = profile([150, 0, 60, 0, 0, 500]);
  assert.equal(before.cumulative[0], 250);
  assert.equal(after.cumulative[0], 210);
  // The ask side holds the scale steady, isolating the cumulative effect.
  assert.equal(before.maximum, after.maximum);
});

test("displayed-unit scale preserves fractional lots and zero-book quantity atoms", () => {
  assert.equal(profile([0, 0.1, 0.2, 0.3], 8).maximum, 0.4);
  assert.equal(profile([0, 0, 0, 0], 8).maximum, 1e-8);
  assert.equal(profile([0, 0, 0, 0]).maximum, 1);
});
