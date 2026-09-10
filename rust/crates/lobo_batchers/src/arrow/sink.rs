//! Destinations for completed Arrow record batches.
//!
//! Enable `feather` for `FeatherSink` or `wgpu` for `GpuSink` (`gpu` is an alias).
//! Both features enable `arrow`. Pass every batch returned by `Batch::push` directly to
//! `Sink::write`, then write the partial batch from `Batch::flush` before
//! `Sink::finish`. The application owns the message receiver and consumer loop.
//!
//! Feather writes to any `std::io::Write` destination and requires finalization
//! for its file footer. GPU writes return owned buffers and a submission index;
//! the caller can immediately submit downstream work on the same wgpu queue.
//! On browser WASM, `GpuCanvasSink::new(canvas, make_renderer).await` initializes
//! a WebGPU canvas. Its `Sink::write` uploads, renders, and presents each batch
//! with no CPU readback. The renderer defines how price levels become pixels.

#[cfg(feature = "feather")]
mod feather;
#[cfg(feature = "feather")]
pub use feather::FeatherSink;

#[cfg(feature = "wgpu")]
mod gpu;
#[cfg(feature = "wgpu")]
pub use gpu::{GpuArray, GpuBatch, GpuBuffer, GpuNullBuffer, GpuRenderer, GpuSink};

#[cfg(all(feature = "wgpu", target_arch = "wasm32"))]
mod canvas;
#[cfg(all(feature = "wgpu", target_arch = "wasm32"))]
pub use canvas::GpuCanvasSink;
