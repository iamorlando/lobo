use arrow_array::RecordBatch;
use arrow_schema::ArrowError;
use web_sys::HtmlCanvasElement;

use super::{GpuRenderer, GpuSink};
use crate::traits::Sink;

/// Render batches directly into the supplied HTML canvas using WebGPU.
///
/// Initialization is async and can be awaited by a `wasm-bindgen` binding. Each
/// `write` submits the upload and rendering immediately and presents the canvas;
/// it does not await GPU completion or read pixels back to WASM/JavaScript.
///
/// The canvas must be unused or already use a `webgpu` context. A canvas with
/// an existing `2d`, `webgl`, or `bitmaprenderer` context cannot switch modes.
/// Raster content on such a canvas needs a separate WebGPU output canvas.
/// This sink owns configuration/presentation of the supplied canvas.
///
/// The renderer determines the visualization. For incremental raster updates,
/// keep a persistent texture in the renderer and composite it into each frame:
/// the browser may replace the presentation texture after every frame.
pub struct GpuCanvasSink<R: GpuRenderer> {
    canvas: HtmlCanvasElement,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    uploads: GpuSink,
    renderer: R,
}

impl<R: GpuRenderer> GpuCanvasSink<R> {
    /// Initialize browser WebGPU, then build the renderer for this device and
    /// canvas format. Browser device creation uses the default WebGPU limits.
    /// No native GPU backends, blocking executor, or `Send` future are required.
    pub async fn new(
        canvas: HtmlCanvasElement,
        make_renderer: impl FnOnce(
            &wgpu::Device,
            &wgpu::Queue,
            wgpu::TextureFormat,
        ) -> Result<R, ArrowError>,
    ) -> Result<Self, ArrowError> {
        if canvas
            .get_context("webgpu")
            .map_err(|error| ArrowError::ComputeError(format!("canvas context: {error:?}")))?
            .is_none()
        {
            return Err(ArrowError::InvalidArgumentError(
                "canvas requires WebGPU support and must not have another context type".into(),
            ));
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|error| ArrowError::ComputeError(error.to_string()))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|error| ArrowError::ComputeError(error.to_string()))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("lobo canvas device"),
                ..Default::default()
            })
            .await
            .map_err(|error| ArrowError::ComputeError(error.to_string()))?;
        // Read dimensions after the awaits: JavaScript may have resized the canvas.
        validate_size(&device, canvas.width(), canvas.height())?;
        let config = surface
            .get_default_config(&adapter, canvas.width(), canvas.height())
            .ok_or_else(|| ArrowError::ComputeError("unsupported WebGPU canvas surface".into()))?;
        let renderer = make_renderer(&device, &queue, config.format)?;
        surface.configure(&device, &config);
        Ok(Self {
            canvas,
            surface,
            config,
            uploads: GpuSink::new(device, queue),
            renderer,
        })
    }

    /// The actual backend of the device configured for this canvas.
    pub fn backend(&self) -> wgpu::Backend {
        self.uploads.device().adapter_info().backend
    }

    /// Access renderer-owned GPU state or visualization settings.
    pub fn renderer_mut(&mut self) -> &mut R {
        &mut self.renderer
    }

    fn configure_size(&mut self) -> Result<(), ArrowError> {
        let width = self.canvas.width();
        let height = self.canvas.height();
        validate_size(self.uploads.device(), width, height)?;
        if width != self.config.width || height != self.config.height {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(self.uploads.device(), &self.config);
        }
        Ok(())
    }
}

impl<R: GpuRenderer> Sink<RecordBatch> for GpuCanvasSink<R> {
    type SinkResult = wgpu::SubmissionIndex;
    type SinkError = ArrowError;

    fn write(&mut self, batch: &RecordBatch) -> Result<Self::SinkResult, ArrowError> {
        self.configure_size()?;
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            // Acquire before uploading or updating renderer state. On an error
            // the caller still owns the batch and can retry or rebuild the sink.
            status => {
                return Err(ArrowError::ComputeError(format!(
                    "canvas frame: {status:?}"
                )));
            }
        };
        let view = frame.texture.create_view(&Default::default());
        // Do not yield/await between acquiring a browser frame and presenting it.
        let submission = self
            .uploads
            .write_to_view(batch, &view, &mut self.renderer)?;
        self.uploads.queue().present(frame);
        Ok(submission)
    }

    fn flush(&mut self) -> Result<(), ArrowError> {
        self.uploads.flush()
    }

    fn finish(&mut self) -> Result<(), ArrowError> {
        self.uploads.finish()
    }
}

fn validate_size(device: &wgpu::Device, width: u32, height: u32) -> Result<(), ArrowError> {
    let max = device.limits().max_texture_dimension_2d;
    if width == 0 || height == 0 || width > max || height > max {
        return Err(ArrowError::InvalidArgumentError(format!(
            "canvas dimensions must be between 1 and {max} pixels, got {width}x{height}"
        )));
    }
    Ok(())
}
