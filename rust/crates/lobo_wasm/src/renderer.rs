use crate::camera::Camera;
use crate::presentation::{self, BINS, BOOK_BYTES, COLUMNS, HISTORY_BYTES};
use arrow_schema::ArrowError;
use lobo_batchers::arrow::sink::{GpuBatch, GpuRenderer};

const PAGE_BOOKS: u32 = 128;
const MAX_BOOKS: usize = 16_384;

struct Page {
    history: wgpu::Buffer,
    params: wgpu::Buffer,
    compute: wgpu::BindGroup,
    display: wgpu::BindGroup,
}
pub struct Renderer {
    pub loading: crate::loading::LoadingRenderer,
    pub bars: crate::bars::BarRenderer,
    pub palette: crate::palette::Palette,
    slots: wgpu::Buffer,
    books: wgpu::Buffer,
    cameras: wgpu::Buffer,
    slot_capacity: u32,
    book_capacity: u32,
    pages: Vec<Page>,
    compute_layout: wgpu::BindGroupLayout,
    display_layout: wgpu::BindGroupLayout,
    patch_layout: wgpu::BindGroupLayout,
    apply: wgpu::ComputePipeline,
    clear: wgpu::ComputePipeline,
    aggregate: wgpu::ComputePipeline,
    capture: wgpu::ComputePipeline,
    display: wgpu::RenderPipeline,
    pub views: Vec<Camera>,
    pub selected: Option<u32>,
    pub price_slots: u32,
    pub width: u32,
    pub height: u32,
    pub span: f32,
    pub window_seconds: f32,
    pub elapsed: f64,
    pub warming: bool,
    pub history_gap: bool,
    dirty: bool,
    last_capture: Option<(u32, bool)>,
    released_slots: Vec<u32>,
}
fn storage(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}
fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}
impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        _: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Result<Self, ArrowError> {
        let binding = |binding, read_only, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: if binding == 0 {
                    wgpu::BufferBindingType::Uniform
                } else {
                    wgpu::BufferBindingType::Storage { read_only }
                },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("All books compute"),
            entries: &(0..5)
                .map(|i| binding(i, i == 3, wgpu::ShaderStages::COMPUTE))
                .collect::<Vec<_>>(),
        });
        let display_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Selected book display"),
            entries: &(0..3)
                .map(|i| binding(i, true, wgpu::ShaderStages::FRAGMENT))
                .collect::<Vec<_>>(),
        });
        let patch_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Routed Arrow patches"),
            entries: &(0..4)
                .map(|i| wgpu::BindGroupLayoutEntry {
                    binding: i,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                })
                .collect::<Vec<_>>(),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&compute_layout), Some(&patch_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("All books compute"),
            source: wgpu::ShaderSource::Wgsl(include_str!("compute.wgsl").into()),
        });
        let pipeline = |name| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some(name),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let display_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Canvas charts"),
            source: wgpu::ShaderSource::Wgsl(presentation::display_shader(format.is_srgb()).into()),
        });
        let palette = crate::palette::Palette::new(device);
        let render_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&display_layout), Some(&palette.layout)],
            immediate_size: 0,
        });
        let display = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Selected book canvas"),
            layout: Some(&render_layout),
            vertex: wgpu::VertexState {
                module: &display_shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &display_shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        Ok(Self {
            loading: crate::loading::LoadingRenderer::new(device, format, &palette.layout),
            bars: crate::bars::BarRenderer::new(device, format, &palette.layout),
            palette,
            slots: storage(device, "Shared price table", 1024 * 16),
            books: storage(device, "Shared book bins", PAGE_BOOKS as u64 * BOOK_BYTES),
            cameras: storage(device, "Book cameras", PAGE_BOOKS as u64 * 16),
            slot_capacity: 1024,
            book_capacity: PAGE_BOOKS,
            pages: Vec::new(),
            apply: pipeline("apply"),
            clear: pipeline("clear_bins"),
            aggregate: pipeline("aggregate"),
            capture: pipeline("capture"),
            compute_layout,
            display_layout,
            patch_layout,
            display,
            views: Vec::new(),
            selected: None,
            price_slots: 0,
            width: 1,
            height: 1,
            span: 0.005,
            window_seconds: 300.0,
            elapsed: 0.0,
            warming: true,
            history_gap: false,
            dirty: true,
            last_capture: None,
            released_slots: Vec::new(),
        })
    }
    pub fn center(&self) -> f32 {
        if self.bars.enabled
            && let Some((center, _)) = self.bars.price_range(self.selected)
        {
            return center;
        }
        self.selected
            .and_then(|i| self.views.get(i as usize))
            .map_or(0.0, |view| view.center)
    }
    pub fn price_span(&self) -> f32 {
        if self.bars.enabled
            && let Some((_, span)) = self.bars.price_range(self.selected)
        {
            return span;
        }
        self.selected
            .and_then(|i| self.views.get(i as usize))
            .map_or(1.0, |view| view.span)
    }
    pub fn invalidate(&mut self, index: u32) {
        if let Some(view) = self.views.get_mut(index as usize) {
            view.invalidate();
            self.dirty = true;
        }
    }
    pub fn release_slots(&mut self, slots: impl Iterator<Item = u32>) {
        self.released_slots.extend(slots);
        self.dirty = true;
    }
    pub fn recenter(&mut self) {
        if let Some(view) = self.selected.and_then(|i| self.views.get_mut(i as usize)) {
            view.recenter();
        }
    }
    /// Pan the selected camera and pause automatic tracking until recentered.
    /// Historical columns retain their price coordinates on the GPU.
    pub fn pan_price(&mut self, fraction: f32) {
        if let Some(view) = self.selected.and_then(|i| self.views.get_mut(i as usize)) {
            view.pan(fraction);
            self.dirty = true;
        }
    }
    pub fn configure(&mut self, window_seconds: f32, span: f32) {
        if self.span != span || self.window_seconds != window_seconds {
            self.dirty = true;
            for view in &mut self.views {
                if self.window_seconds != window_seconds {
                    view.epoch += 1;
                }
                if self.span != span {
                    view.reset_range(span);
                }
            }
        }
        self.span = span;
        self.window_seconds = window_seconds;
    }
    pub fn freeze_at(&mut self, index: u32, time: Option<f64>) {
        let view = &mut self.views[index as usize];
        if view.frozen_at != time {
            view.frozen_at = time;
            self.dirty = true;
        }
    }
    pub fn camera(
        &mut self,
        index: u32,
        bid: Option<f64>,
        ask: Option<f64>,
    ) -> Result<(), ArrowError> {
        if index as usize >= MAX_BOOKS {
            return Err(ArrowError::ComputeError(
                "Replay exceeds 16,384 active GPU books".into(),
            ));
        }
        self.dirty |= self.views.len() <= index as usize;
        self.views
            .resize_with(self.views.len().max(index as usize + 1), Camera::default);
        self.dirty |= self.views[index as usize].update(bid, ask, self.span, self.warming);
        Ok(())
    }
    fn groups(
        &self,
        device: &wgpu::Device,
        params: &wgpu::Buffer,
        history: &wgpu::Buffer,
    ) -> (wgpu::BindGroup, wgpu::BindGroup) {
        let compute = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.compute_layout,
            entries: &[
                entry(0, params),
                entry(1, &self.slots),
                entry(2, &self.books),
                entry(3, &self.cameras),
                entry(4, history),
            ],
        });
        let display = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.display_layout,
            entries: &[entry(0, params), entry(1, &self.books), entry(2, history)],
        });
        (compute, display)
    }
    fn reserve(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        let mut changed = false;
        if self.price_slots > self.slot_capacity {
            let capacity = self.price_slots.next_power_of_two();
            let buffer = storage(device, "Shared price table", capacity as u64 * 16);
            encoder.copy_buffer_to_buffer(
                &self.slots,
                0,
                &buffer,
                0,
                self.slot_capacity as u64 * 16,
            );
            self.slots = buffer;
            self.slot_capacity = capacity;
            changed = true;
        }
        if self.views.len() > self.book_capacity as usize {
            let capacity = (self.views.len() as u32).next_power_of_two();
            let buffer = storage(device, "Shared book bins", capacity as u64 * BOOK_BYTES);
            encoder.copy_buffer_to_buffer(
                &self.books,
                0,
                &buffer,
                0,
                self.book_capacity as u64 * BOOK_BYTES,
            );
            self.books = buffer;
            self.cameras = storage(device, "Book cameras", capacity as u64 * 16);
            self.book_capacity = capacity;
            changed = true;
        }
        if changed {
            for i in 0..self.pages.len() {
                let (compute, display) =
                    self.groups(device, &self.pages[i].params, &self.pages[i].history);
                self.pages[i].compute = compute;
                self.pages[i].display = display;
            }
        }
        while self.pages.len() < self.views.len().max(1).div_ceil(PAGE_BOOKS as usize) {
            let history = storage(
                device,
                "Book history page",
                PAGE_BOOKS as u64 * HISTORY_BYTES,
            );
            let params = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("History page parameters"),
                size: 64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let (compute, display) = self.groups(device, &params, &history);
            self.pages.push(Page {
                history,
                params,
                compute,
                display,
            });
        }
    }
}
impl GpuRenderer for Renderer {
    fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        batch: &GpuBatch,
        target: &wgpu::TextureView,
    ) -> Result<(), ArrowError> {
        self.palette.upload(queue);
        if self.loading.active {
            self.loading.render(
                device,
                queue,
                encoder,
                target,
                &self.palette.group,
                self.width,
                self.height,
            );
            return Ok(());
        }
        self.reserve(device, encoder);
        // Clear retired scenario slots on-device before any new Arrow patches
        // reuse them. Ordering is explicit in this command buffer; no readback.
        for slot in self.released_slots.drain(..) {
            encoder.clear_buffer(&self.slots, u64::from(slot) * 16, Some(16));
        }
        self.bars.upload(device, encoder, self.views.len() as u32);
        let absolute = (self.elapsed.max(0.0) / self.window_seconds as f64 * COLUMNS as f64) as u32;
        // Selecting a different page at a paused clock only presents its cached
        // history. Compute runs when events, sampling time, or cameras change.
        let compute = self.dirty
            || self.history_gap
            || batch.num_rows > 0
            || self.last_capture != Some((absolute, self.warming));
        if self.dirty {
            let camera_bytes = self
                .views
                .iter()
                .flat_map(|view| {
                    [
                        view.minimum().to_bits(),
                        view.span.to_bits(),
                        view.epoch.max(1),
                        view.frozen_at.map_or(0, |time| {
                            (time / self.window_seconds as f64 * COLUMNS as f64) as u32 + 1
                        }),
                    ]
                })
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>();
            if !camera_bytes.is_empty() {
                queue.write_buffer(&self.cameras, 0, &camera_bytes);
            }
        }
        // A new simulation can have no OHLC quote event on one side yet. Split
        // its depth with current native quotes, just like native hit-testing.
        let quote_midpoint = self
            .selected
            .and_then(|index| self.views.get(index as usize))
            .map_or(0.0, |view| view.quote_midpoint);
        let price_split =
            ((quote_midpoint - self.center() as f64) / self.price_span() as f64 + 0.5) as f32;
        for (index, page) in self.pages.iter().enumerate() {
            if !compute && index != self.selected.map_or(0, |i| i / PAGE_BOOKS) as usize {
                continue;
            }
            let values = [
                self.width,
                self.height,
                batch.num_rows as u32,
                self.selected.unwrap_or(u32::MAX),
                absolute,
                u32::from(self.history_gap),
                0,
                0,
                self.views.len() as u32,
                self.price_slots,
                index as u32 * PAGE_BOOKS,
                0,
                u32::from(!self.warming),
                price_split.to_bits(),
                (self.center() - self.price_span() / 2.0).to_bits(),
                self.price_span().to_bits(),
            ];
            let bytes = values
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>();
            queue.write_buffer(&page.params, 0, &bytes);
        }
        let patches = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("All books Arrow updates"),
            layout: &self.patch_layout,
            entries: &[
                entry(0, &batch.columns[2].buffers[0].buffer),
                entry(1, &batch.columns[3].buffers[0].buffer),
                entry(2, &batch.columns[6].buffers[0].buffer),
                entry(3, &batch.columns[7].buffers[0].buffer),
            ],
        });
        if compute && !self.views.is_empty() {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_bind_group(0, &self.pages[0].compute, &[]);
            pass.set_bind_group(1, &patches, &[]);
            if batch.num_rows > 0 {
                pass.set_pipeline(&self.apply);
                pass.dispatch_workgroups((batch.num_rows as u32).div_ceil(256), 1, 1);
            }
            pass.set_pipeline(&self.clear);
            pass.dispatch_workgroups((self.views.len() as u32 * BINS).div_ceil(256), 1, 1);
            pass.set_pipeline(&self.aggregate);
            pass.dispatch_workgroups(self.price_slots.div_ceil(256), 1, 1);
            pass.set_pipeline(&self.capture);
            for (index, page) in self.pages.iter().enumerate() {
                pass.set_bind_group(0, &page.compute, &[]);
                pass.dispatch_workgroups(
                    (self.views.len() as u32 - index as u32 * PAGE_BOOKS).min(PAGE_BOOKS),
                    1,
                    1,
                );
            }
        }
        self.last_capture = Some((absolute, self.warming));
        self.dirty = false;
        self.history_gap = false;
        if self.bars.enabled {
            self.bars.render(
                queue,
                encoder,
                target,
                &self.palette.group,
                self.selected,
                self.width,
                self.height,
                self.center(),
                self.price_span(),
            );
            return Ok(());
        }
        let page = self.selected.map_or(0, |i| i / PAGE_BOOKS) as usize;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Canvas presentation"),
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
        pass.set_pipeline(&self.display);
        pass.set_bind_group(0, &self.pages[page].display, &[]);
        pass.set_bind_group(1, &self.palette.group, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}
