import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";

const exports = {};
vm.runInNewContext(
  ts.transpileModule(
    readFileSync(new URL("../lib/gpu-status.ts", import.meta.url), "utf8"),
    {
      compilerOptions: {
        module: ts.ModuleKind.CommonJS,
        target: ts.ScriptTarget.ES2022,
      },
    },
  ).outputText,
  { exports },
);
const { observeGpuStatus } = exports;

function canvas(device) {
  return {
    getContext(kind) {
      assert.equal(kind, "webgpu");
      return {
        getConfiguration: () => ({
          device: { lost: new Promise(() => {}), ...device },
        }),
      };
    },
  };
}

test("the session's backend enum controls the label, regardless of hardware metadata", () => {
  for (const [backend, label] of [
    ["Metal", "Metal"],
    ["Vulkan", "Vulkan"],
    ["Dx12", "Dx12"],
    ["Gl", "Gl"],
    ["BrowserWebGpu", "WebGPU"],
  ]) {
    for (const context of [
      canvas({ adapterInfo: { description: "Apple" } }),
      canvas({
        adapterInfo: {
          vendor: "apple",
          description: "Apple M2",
          architecture: "metal-3",
        },
      }),
      canvas({ adapterInfo: { description: "Intel (Vulkan 1.3)" } }),
      canvas({ adapterInfo: { description: "" } }),
      canvas({}),
      { getContext: () => ({}) },
    ]) {
      const values = [];
      observeGpuStatus(context, backend, (value) => values.push(value));
      assert.deepEqual(values, [label]);
    }
  }
});

test("missing and software devices do not claim GPU acceleration", () => {
  for (const context of [
    { getContext: () => null },
    canvas({
      adapterInfo: { description: "Software", isFallbackAdapter: true },
    }),
  ]) {
    const values = [];
    observeGpuStatus(context, "BrowserWebGpu", (value) => values.push(value));
    assert.deepEqual(values, [null]);
  }
});

test("missing and Noop backends do not fall back to a WebGPU label", () => {
  for (const backend of [undefined, "", "Noop"]) {
    const values = [];
    observeGpuStatus(canvas({}), backend, (value) => values.push(value));
    assert.deepEqual(values, [null]);
  }
});

test("device loss clears status, but a disposed session cannot clear its replacement", async () => {
  for (const disposed of [false, true]) {
    const { promise, resolve } = Promise.withResolvers();
    const values = [];
    const stop = observeGpuStatus(canvas({ lost: promise }), "Metal", (value) =>
      values.push(value),
    );
    if (disposed) stop();
    resolve({ reason: "destroyed" });
    await promise;
    assert.deepEqual(values, disposed ? ["Metal"] : ["Metal", null]);
  }
});
