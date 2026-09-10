//! Completed and forming volume bars remain in a GPU ring for every book.
use wgpu::util::DeviceExt;
const ROW_BYTES: u64 = 32;
const ROWS: u64 = 256;
struct Patch {
    book: u32,
    index: u64,
    values: [f32; 5],
}
pub struct BarRenderer {
    data: wgpu::Buffer,
    params: wgpu::Buffer,
    capacity: u32,
    layout: wgpu::BindGroupLayout,
    group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    pending: Vec<Patch>,
    ranges: Vec<std::collections::VecDeque<(u64, f32, f32, f32, u64)>>,
    pub enabled: bool,
    pub completed: u64,
    pub forming: bool,
    pub bid: f32,
    pub ask: f32,
}
fn storage(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Volume bar history"),
        size: capacity as u64 * ROWS * ROW_BYTES,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}
fn group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    params: &wgpu::Buffer,
    data: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Volume bar display"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: data.as_entire_binding(),
            },
        ],
    })
}
impl BarRenderer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        palette: &wgpu::BindGroupLayout,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Volume bars"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("OHLC volume bars"),
            source: wgpu::ShaderSource::Wgsl(
                crate::presentation::themed_shader(include_str!("bars.wgsl"), format.is_srgb())
                    .into(),
            ),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout), Some(palette)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Volume bars canvas"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let data = storage(device, 128);
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Volume bar parameters"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = group(device, &layout, &params, &data);
        Self {
            data,
            params,
            capacity: 128,
            layout,
            group,
            pipeline,
            pending: Vec::new(),
            ranges: Vec::new(),
            enabled: false,
            completed: 0,
            forming: false,
            bid: 0.0,
            ask: 0.0,
        }
    }
    pub fn reset(&mut self) {
        self.pending.clear();
        self.ranges.clear();
        self.completed = 0;
        self.forming = false;
    }
    pub fn reset_book(&mut self, book: u32) {
        self.pending.retain(|patch| patch.book != book);
        if let Some(ranges) = self.ranges.get_mut(book as usize) {
            ranges.clear();
        }
    }
    pub fn push(&mut self, book: u32, index: u64, values: [f32; 5], start_ns: u64) {
        self.ranges
            .resize_with(self.ranges.len().max(book as usize + 1), Default::default);
        let ranges = &mut self.ranges[book as usize];
        if ranges.back().is_some_and(|last| last.0 > index) {
            ranges.clear();
        }
        if ranges.back().is_some_and(|last| last.0 == index) {
            ranges.pop_back();
        }
        ranges.push_back((index, values[2], values[1], values[4], start_ns));
        while ranges.front().is_some_and(|first| index - first.0 >= 80) {
            ranges.pop_front();
        }
        self.pending.push(Patch {
            book,
            index,
            values,
        });
    }
    /// Display bounds come from message metadata already on the CPU. Candle
    /// pixels and the GPU history are never read back.
    pub fn price_range(&self, selected: Option<u32>) -> Option<(f32, f32)> {
        let ranges = self.ranges.get(selected? as usize)?;
        let &(_, mut low, mut high, _, _) = ranges.front()?;
        for &(_, bottom, top, _, _) in ranges {
            low = low.min(bottom);
            high = high.max(top);
        }
        let center = (low + high) * 0.5;
        Some((center, (high - low).max(center.abs() * 0.0001) * 1.15))
    }
    pub fn timestamps(&self, selected: Option<u32>) -> Vec<f64> {
        selected
            .and_then(|i| self.ranges.get(i as usize))
            .map_or_else(Vec::new, |rows| {
                rows.iter().map(|row| row.4 as f64 / 1e6).collect()
            })
    }
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        books: u32,
    ) {
        if books > self.capacity {
            let capacity = books.next_power_of_two();
            let data = storage(device, capacity);
            encoder.copy_buffer_to_buffer(
                &self.data,
                0,
                &data,
                0,
                self.capacity as u64 * ROWS * ROW_BYTES,
            );
            self.data = data;
            self.capacity = capacity;
            self.group = group(device, &self.layout, &self.params, &self.data);
        }
        if self.pending.is_empty() {
            return;
        }
        let mut bytes = Vec::with_capacity(self.pending.len() * ROW_BYTES as usize);
        for patch in &self.pending {
            for value in patch.values {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&(patch.index as u32).to_le_bytes());
            bytes.extend_from_slice(&[0; 8]);
        }
        let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Volume bar uploads"),
            contents: &bytes,
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        for (row, patch) in self.pending.drain(..).enumerate() {
            let offset = (patch.book as u64 * ROWS + patch.index % ROWS) * ROW_BYTES;
            encoder.copy_buffer_to_buffer(
                &staging,
                row as u64 * ROW_BYTES,
                &self.data,
                offset,
                ROW_BYTES,
            );
        }
    }
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        palette: &wgpu::BindGroup,
        selected: Option<u32>,
        width: u32,
        height: u32,
        center: f32,
        span: f32,
    ) {
        let total = self.completed + u64::from(self.forming);
        let count = total.min(80);
        let values = [
            width,
            height,
            selected.unwrap_or(0),
            count as u32,
            (total - count) as u32,
            total as u32,
            u32::from(self.forming),
            0,
            (center - span / 2.0).to_bits(),
            span.to_bits(),
            self.bid.to_bits(),
            self.ask.to_bits(),
            self.ranges
                .get(selected.unwrap_or(0) as usize)
                .map_or(1.0, |rows| {
                    rows.iter().map(|r| r.3).fold(1e-20f32, f32::max)
                })
                .to_bits(),
            0,
            0,
            0,
        ];
        let bytes: Vec<_> = values.into_iter().flat_map(u32::to_le_bytes).collect();
        queue.write_buffer(&self.params, 0, &bytes);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Volume bars presentation"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_bind_group(1, palette, &[]);
        pass.draw(0..3, 0..1);
    }
}
