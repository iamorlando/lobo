use arrow_array::RecordBatch;
use arrow_data::ArrayData;
use arrow_schema::{ArrowError, DataType, SchemaRef};

use crate::traits::Sink;

/// An Arrow buffer copied to GPU storage, padded to wgpu's copy alignment.
#[derive(Debug)]
pub struct GpuBuffer {
    pub buffer: wgpu::Buffer,
    /// Original byte length, excluding GPU allocation padding.
    pub byte_len: usize,
}

/// Arrow validity bits, including the bit offset of a sliced array.
#[derive(Debug)]
pub struct GpuNullBuffer {
    pub buffer: GpuBuffer,
    pub bit_offset: usize,
}

/// The Arrow physical layout of one array, backed by GPU buffers.
///
/// Buffer and child order match `ArrayData`. `offset` is the Arrow element
/// offset; consumers must honor it (and string offsets) when addressing data.
/// Bytes are copied without conversion: integers keep their Arrow native byte
/// order, while the price-level batcher's prices remain 16-byte big-endian
/// values. Shaders must interpret these bytes explicitly, including 64-bit
/// integers on devices without native 64-bit shader arithmetic.
#[derive(Debug)]
pub struct GpuArray {
    pub data_type: DataType,
    pub len: usize,
    pub offset: usize,
    pub buffers: Vec<GpuBuffer>,
    pub nulls: Option<GpuNullBuffer>,
    pub children: Vec<GpuArray>,
}

/// An uploaded batch owned by the caller, with no retained CPU array buffers.
#[derive(Debug)]
#[must_use = "retain the GPU batch for downstream processing"]
pub struct GpuBatch {
    pub schema: SchemaRef,
    pub num_rows: usize,
    pub columns: Vec<GpuArray>,
    /// Uploads have been submitted, but are not necessarily complete.
    pub submission: wgpu::SubmissionIndex,
}

/// Encode GPU work that renders an uploaded batch into a caller-owned texture.
///
/// The renderer owns its pipelines and any persistent GPU visualization state.
/// It must initialize the whole output frame; canvas presentation textures do
/// not preserve previous frames. Incremental drawings can be accumulated in a
/// persistent GPU texture and composited into `target`, without CPU readback.
/// The target, pipelines, and buffers must all belong to the supplied device.
/// No `Send` bound is required, so browser WASM renderers can hold JS handles.
pub trait GpuRenderer {
    fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        batch: &GpuBatch,
        target: &wgpu::TextureView,
    ) -> Result<(), ArrowError>;
}

/// Upload each batch immediately through a caller-owned wgpu device and queue.
///
/// The device and queue must belong together. The caller configures the device's
/// error handler for asynchronous device loss, validation, and allocation errors.
/// Each write allocates independent storage buffers and submits their transfers
/// before returning. No GPU wait, computation, or run-wide batch collection is
/// performed. Subsequent work on the same queue is ordered after these uploads;
/// callers manage in-flight batches and poll the device when completion is needed.
pub struct GpuSink {
    device: wgpu::Device,
    queue: wgpu::Queue,
    finished: bool,
}

impl GpuSink {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            device,
            queue,
            finished: false,
        }
    }

    /// Upload and render into an existing GPU texture view, submitting before
    /// returning. No pixels or batch data are copied back to the CPU.
    ///
    /// This can be called directly from an async WASM binding. Submission does
    /// not wait for GPU completion; awaiting every batch would serialize the
    /// producer and GPU. The target's usages must support the renderer's work.
    pub fn write_to_view(
        &mut self,
        batch: &RecordBatch,
        target: &wgpu::TextureView,
        renderer: &mut impl GpuRenderer,
    ) -> Result<wgpu::SubmissionIndex, ArrowError> {
        let uploaded = self.write(batch)?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lobo batch rendering"),
            });
        renderer.render(&self.device, &self.queue, &mut encoder, &uploaded, target)?;
        Ok(self.queue.submit([encoder.finish()]))
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn device(&self) -> &wgpu::Device {
        &self.device
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    fn buffer_size(&self, byte_len: usize) -> Result<u64, ArrowError> {
        let size = (byte_len as u64)
            .max(1)
            .checked_next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
            .ok_or_else(|| ArrowError::InvalidArgumentError("GPU buffer size overflow".into()))?;
        if size > self.device.limits().max_buffer_size {
            return Err(ArrowError::InvalidArgumentError(format!(
                "Arrow buffer requires {size} GPU bytes, exceeding the device buffer limit"
            )));
        }
        Ok(size)
    }

    fn validate_array(&self, data: &ArrayData) -> Result<(), ArrowError> {
        for buffer in data.buffers() {
            self.buffer_size(buffer.len())?;
        }
        if let Some(nulls) = data.nulls() {
            self.buffer_size(nulls.buffer().len())?;
        }
        for child in data.child_data() {
            self.validate_array(child)?;
        }
        Ok(())
    }

    fn upload_buffer(&self, bytes: &[u8]) -> Result<GpuBuffer, ArrowError> {
        let size = self.buffer_size(bytes.len())?;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lobo Arrow buffer"),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        if !bytes.is_empty() {
            let mut staging = self
                .queue
                .write_buffer_with(&buffer, 0, wgpu::BufferSize::new(size).unwrap())
                .ok_or_else(|| ArrowError::ComputeError("GPU staging allocation failed".into()))?;
            staging.slice(..bytes.len()).copy_from_slice(bytes);
            staging.slice(bytes.len()..).fill(0);
        }
        Ok(GpuBuffer {
            buffer,
            byte_len: bytes.len(),
        })
    }

    fn upload_array(&self, data: &ArrayData) -> Result<GpuArray, ArrowError> {
        let buffers = data
            .buffers()
            .iter()
            .map(|buffer| self.upload_buffer(buffer.as_slice()))
            .collect::<Result<_, _>>()?;
        let nulls = data
            .nulls()
            .map(|nulls| {
                Ok::<_, ArrowError>(GpuNullBuffer {
                    buffer: self.upload_buffer(nulls.buffer().as_slice())?,
                    bit_offset: nulls.offset(),
                })
            })
            .transpose()?;
        let children = data
            .child_data()
            .iter()
            .map(|child| self.upload_array(child))
            .collect::<Result<_, _>>()?;
        Ok(GpuArray {
            data_type: data.data_type().clone(),
            len: data.len(),
            offset: data.offset(),
            buffers,
            nulls,
            children,
        })
    }
}

impl Sink<RecordBatch> for GpuSink {
    type SinkResult = GpuBatch;
    type SinkError = ArrowError;

    fn write(&mut self, batch: &RecordBatch) -> Result<GpuBatch, ArrowError> {
        if self.finished {
            return Err(ArrowError::ComputeError("GPU sink is finished".into()));
        }
        let arrays: Vec<_> = batch
            .columns()
            .iter()
            .map(|column| column.to_data())
            .collect();
        // Reject oversized buffers before scheduling any transfers.
        for array in &arrays {
            self.validate_array(array)?;
        }
        let columns = arrays
            .iter()
            .map(|array| self.upload_array(array))
            .collect::<Result<_, _>>();
        // Submit even on an allocation failure to release any staging buffers
        // already queued for this batch. Submission itself does not wait.
        let submission = self.queue.submit([]);
        Ok(GpuBatch {
            schema: batch.schema(),
            num_rows: batch.num_rows(),
            columns: columns?,
            submission,
        })
    }

    fn flush(&mut self) -> Result<(), ArrowError> {
        self.queue.submit([]);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ArrowError> {
        self.flush()?;
        self.finished = true;
        Ok(())
    }
}
