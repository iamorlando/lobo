/// <reference types="@webgpu/types" />

/** Observe the device configured by the chart, after session creation succeeds. */
export function observeGpuStatus(
  canvas: HTMLCanvasElement,
  backend: string,
  onChange: (backend: string | null) => void,
): () => void {
  const context = canvas.getContext("webgpu");
  const device = context?.getConfiguration?.()?.device;
  const info = device?.adapterInfo;
  let active = true;
  // The session exports its device's wgpu::Backend variant. Hardware metadata
  // must not replace that value or turn Metal/Vulkan into a generic WebGPU label.
  const label = backend === "BrowserWebGpu" ? "WebGPU" : backend;
  onChange(
    context && !info?.isFallbackAdapter && backend !== "Noop"
      ? label || null
      : null,
  );
  void device?.lost.then(() => {
    if (active) onChange(null);
  });
  return () => {
    active = false;
  };
}
