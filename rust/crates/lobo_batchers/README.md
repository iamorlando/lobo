# Batch destinations

Features are off by default. Enable `feather` for incremental Feather V2 IO or
`wgpu` for GPU uploads and rendering. `gpu` remains an alias for `wgpu`.
The optional wgpu dependency lives in this crate: native builds enable native
backends; browser WASM builds enable WebGPU.

Pass each completed batch from `Batch::push` to `Sink::write` immediately.
Write the final partial batch from `Batch::flush` before `Sink::finish`.
The application owns the channel receiver and scheduling.

## Browser canvas

A WASM binding can await `GpuCanvasSink::new(canvas, make_renderer)` once, then
reuse that sink for incoming batches. `make_renderer` receives the WebGPU device,
queue, and canvas format and returns an implementation of `GpuRenderer`.
That renderer defines how the Arrow columns become pixels.

The call pattern inside the binding is:

```rust,ignore
let mut sink = GpuCanvasSink::new(canvas, |device, queue, format| {
    MyRenderer::new(device, queue, format)
}).await?;

// For every batch as the batcher produces it:
sink.write(&batch)?;
```

`write` uploads the Arrow buffers, invokes the renderer with those GPU buffers
and the canvas texture view, submits the commands, and presents the same canvas.
It does not await GPU completion or copy pixels back to CPU memory. An async
binding can call it directly. There is still an initial CPU-to-GPU upload of the
Arrow buffers. The sink does not export a JS binding or choose a visualization.

The canvas must be unused or already have a `webgpu` context. A canvas already
rasterized through `2d`/`webgl` cannot change its context type; use a separate
WebGPU output canvas in that case. The renderer can import a source raster using
the queue's external-image-to-texture API without reading pixels into WASM.

Canvas presentation textures are temporary. To preserve incremental drawing,
the renderer should maintain a persistent GPU texture and composite it into
each canvas frame. Canvas backing dimensions are checked on each write and the
surface is reconfigured on resize. Zero-sized or unavailable surfaces return an
error before uploading the batch; retain the batch to retry, or recreate the
sink if the surface/device is lost.

For a caller that already owns a device and texture view, use
`GpuSink::write_to_view(&batch, &view, &mut renderer)`.

## Validation

```sh
cargo test -p lobo_batchers --all-features
cargo test -p lobo_batchers --features wgpu --test gpu_sink -- --ignored
cargo check -p lobo_batchers --features wgpu --target wasm32-unknown-unknown
```

The ignored native tests require a GPU. Their CPU readbacks are test-only checks
of uploaded bytes and rendered pixels; production sinks have no readback path.

## Observer metrics

`PriceLevelArrowBatcher::with_metrics(capacity, PriceLevelMetrics::empty())`
zeros hidden totals in its output while preserving the Arrow schema. Use
`PriceLevelMetrics::HIDDEN_QUANTITY` to include them; `new(capacity)` retains
the existing behavior of including all metrics. These `bitflags!` options are
evaluated by the batcher after native publication, independently for each sink.
Native price-level messages always contain their book's hidden total.
