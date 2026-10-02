use crate::{
    args::RendererKind,
    engine::Snapshot,
    theme::{Theme, rgb},
};
use anyhow::{Result, anyhow, bail};
use bytemuck::{Pod, Zeroable};
use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
use std::time::Duration;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Params {
    size: [f32; 4],
    scale: [f32; 4],
    bg: [f32; 4],
    grid: [f32; 4],
    bid: [f32; 4],
    ask: [f32; 4],
    sim: [f32; 4],
    quotes: [f32; 4],
}
pub struct Scene {
    pub params: Params,
    pub records: Vec<[f32; 4]>,
}
impl Scene {
    pub fn depth_scale(&self) -> f32 {
        self.params.scale[1]
    }
    pub fn new(
        snapshot: &Snapshot,
        theme: &Theme,
        size: (u16, u16),
        candles: bool,
        min: f64,
        span: f64,
        history_seconds: f64,
    ) -> Self {
        let (w, h) = size;
        let norm = |p: f64| ((p - min) / span) as f32;
        let mut params = Params {
            size: [
                f32::from(w),
                f32::from(h) * 2.0,
                if candles { 0.0 } else { 1.0 },
                0.0,
            ],
            scale: [1.0; 4],
            bg: rgb(theme.bg),
            grid: rgb(theme.grid),
            bid: rgb(theme.bid),
            ask: rgb(theme.ask),
            sim: rgb(theme.simulation),
            quotes: [
                snapshot.bid.map_or(-1.0, norm),
                snapshot.ask.map_or(2.0, norm),
                0.0,
                0.0,
            ],
        };
        let mut records = Vec::new();
        if candles {
            let bars: Vec<_> = snapshot
                .candles
                .iter()
                .rev()
                .take((usize::from(w) / 3).clamp(1, 80))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            params.size[3] = bars.len() as f32;
            params.scale[0] = bars.iter().map(|b| b.volume as f32).fold(1.0, f32::max);
            for bar in bars {
                records.push([
                    norm(bar.open),
                    norm(bar.high),
                    norm(bar.low),
                    norm(bar.close),
                ]);
                records.push([bar.volume as f32, f32::from(bar.forming), 0.0, 0.0]);
            }
        } else {
            let start = snapshot
                .clock_ns
                .saturating_sub((history_seconds * 1e9) as u64);
            let frames: Vec<_> = snapshot
                .depth
                .iter()
                .filter(|f| f.clock_ns >= start)
                .collect();
            let n = (usize::from(w) * 76 / 100).max(1);
            // History columns plus a separate current depth column.
            records.resize(n + 1, [0.0; 4]);
            params.size[3] = n as f32;
            let mut heatmax = 1.0f32;
            let mut quantity_max = 1.0f32;
            for slot in 0..=n {
                let timestamp = start.saturating_add(
                    ((u128::from(snapshot.clock_ns - start) * slot.min(n - 1) as u128)
                        / n.saturating_sub(1).max(1) as u128) as u64,
                );
                let index = frames.partition_point(|f| f.clock_ns <= timestamp);
                let levels = if slot == n {
                    snapshot.levels.as_slice()
                } else if index > 0 {
                    frames[index - 1].levels.as_slice()
                } else {
                    &[]
                };
                let offset = records.len();
                let price_height = f32::from(h) * 2.0 * 0.84;
                let mut bins = vec![[0.0f32; 4]; (price_height.ceil() as usize).max(1)];
                let mut visible = [0.0f32; 2];
                let mut totals = [0.0f32; 2];
                for level in levels {
                    totals[usize::from(level.side)] += level.quantity as f32;
                    let price = norm(level.price);
                    if !(0.0..=1.0).contains(&price) {
                        continue;
                    }
                    let bin = ((1.0 - price) * price_height) as usize;
                    let len = bins.len();
                    bins[bin.min(len - 1)][usize::from(level.side)] += level.quantity as f32;
                    visible[usize::from(level.side)] += level.quantity as f32;
                }
                // A terminal can only show one quantity per pixel row. Bin once,
                // then the GPU rasterizer does constant work per pixel instead
                // of scanning every price level for every output pixel.
                let mut cumulative = 0.0;
                for bin in &mut bins {
                    cumulative += bin[0];
                    bin[2] = cumulative;
                }
                cumulative = 0.0;
                for bin in bins.iter_mut().rev() {
                    cumulative += bin[1];
                    bin[3] = cumulative;
                }
                heatmax = heatmax.max(bins.iter().map(|b| b[0].max(b[1])).fold(0.0, f32::max));
                records.extend(bins);
                records[slot] = [
                    offset as f32,
                    (records.len() - offset) as f32,
                    totals[0],
                    totals[1],
                ];
                quantity_max = quantity_max.max(totals[0].max(totals[1]));
                if slot == n {
                    params.scale[1] = visible[0].max(visible[1]).max(0.00001);
                }
            }
            params.scale[0] = heatmax;
            params.scale[2] = quantity_max;
        }
        if records.is_empty() {
            records.push([0.0; 4]);
        }
        Self { params, records }
    }
}
pub struct Renderer {
    gpu: Option<Gpu>,
    pub name: String,
    strict: bool,
}
impl Renderer {
    pub fn new(kind: RendererKind) -> Result<Self> {
        if kind == RendererKind::Cpu {
            return Ok(Self {
                gpu: None,
                name: "CPU".into(),
                strict: false,
            });
        }
        match Gpu::new() {
            Ok(gpu) => {
                let name = gpu.name.clone();
                Ok(Self {
                    gpu: Some(gpu),
                    name,
                    strict: kind == RendererKind::Gpu,
                })
            }
            Err(e) if kind == RendererKind::Gpu => Err(e),
            Err(_) => Ok(Self {
                gpu: None,
                name: "CPU · GPU unavailable".into(),
                strict: false,
            }),
        }
    }
    pub fn draw(&mut self, scene: &Scene) -> Result<Pixels> {
        let values = if let Some(gpu) = &mut self.gpu {
            match gpu.draw(scene) {
                Ok(v) => v,
                Err(e) if self.strict => return Err(e),
                Err(_) => {
                    self.gpu = None;
                    self.name = "CPU · GPU disconnected".into();
                    cpu(scene)
                }
            }
        } else {
            cpu(scene)
        };
        Ok(Pixels {
            width: scene.params.size[0] as u16,
            height: scene.params.size[1] as u16,
            values,
        })
    }
}
pub struct Pixels {
    pub width: u16,
    pub height: u16,
    pub values: Vec<u32>,
}
impl Widget for &Pixels {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for y in 0..area.height.min(self.height / 2) {
            for x in 0..area.width.min(self.width) {
                let top =
                    self.values[usize::from(y * 2) * usize::from(self.width) + usize::from(x)];
                let bottom =
                    self.values[usize::from(y * 2 + 1) * usize::from(self.width) + usize::from(x)];
                buf[(area.x + x, area.y + y)]
                    .set_symbol("▀")
                    .set_fg(crate::theme::packed(top))
                    .set_bg(crate::theme::packed(bottom));
            }
        }
    }
}
pub fn cpu(scene: &Scene) -> Vec<u32> {
    let p = &scene.params;
    let w = p.size[0] as usize;
    let h = p.size[1] as usize;
    let n = p.size[3] as usize;
    let mut result = vec![0; w * h];
    let blend = |a: [f32; 4], b: [f32; 4], v: f32| {
        let v = v.clamp(0.0, 1.0);
        [
            a[0] + (b[0] - a[0]) * v,
            a[1] + (b[1] - a[1]) * v,
            a[2] + (b[2] - a[2]) * v,
            1.0,
        ]
    };
    for yy in 0..h {
        for xx in 0..w {
            let x = xx as f32 + 0.5;
            let y = yy as f32 + 0.5;
            let mut color = if yy % 10 == 0 || xx % 24 == 0 {
                p.grid
            } else {
                p.bg
            };
            if n > 0 && p.size[2] == 0.0 {
                let slot = ((xx * 2 + 1) * n / (w * 2)).min(n - 1);
                let v = scene.records[slot * 2];
                let meta = scene.records[slot * 2 + 1];
                let ph = h as f32 * 0.79;
                let price = 1.0 - y / ph;
                // Integer horizontal edges give identical coverage on CPU/GPU,
                // even when a pixel lies exactly on the 17% body boundary.
                let local = (xx * 2 + 1) * n - slot * w * 2;
                let body = local * 100 > w * 2 * 17 && local * 100 < w * 2 * 83;
                let wick = local.abs_diff(w) * 1000 < (w * 90).max(n * 1100);
                let mut ink = if v[3] >= v[0] { p.bid } else { p.ask };
                if meta[1] > 0.0 {
                    ink = blend(p.bg, ink, 0.52);
                }
                let edge = 0.6 / ph;
                if y < ph && wick && price >= v[2] - edge && price <= v[1] + edge {
                    color = ink;
                }
                if y < ph
                    && body
                    && price >= v[0].min(v[3]) - edge
                    && price <= v[0].max(v[3]) + edge
                {
                    color = ink;
                }
                if y > ph + 2.0
                    && body
                    && y >= h as f32 - (h as f32 - ph - 3.0) * meta[0] / p.scale[0].max(0.00001)
                {
                    color = blend(p.bg, ink, 0.65);
                }
            } else if n > 0 {
                let hw = w as f32 * 0.76;
                let slot = if x < hw {
                    ((x / hw * n as f32) as usize).min(n - 1)
                } else {
                    n
                };
                let col = scene.records[slot];
                let ph = h as f32 * 0.84;
                let price = 1.0 - y / ph;
                let half = 0.6 / ph;
                let row = yy.min(col[1] as usize - 1);
                let quantities = scene.records[col[0] as usize + row];
                let [bid, ask, cb, ca] = quantities;
                if x < hw {
                    if bid + ask > 0.0 {
                        color = blend(
                            color,
                            if bid >= ask { p.bid } else { p.ask },
                            0.15 + 0.85 * (bid.max(ask) / p.scale[0].max(0.00001)).sqrt(),
                        );
                    }
                    if (price - p.quotes[0]).abs() < half || (price - p.quotes[1]).abs() < half {
                        color = if (price - p.quotes[0]).abs() < half {
                            p.bid
                        } else {
                            p.ask
                        };
                    }
                } else {
                    let q = (w as f32 - 1.0 - x) / (w as f32 - hw) * p.scale[1].max(0.00001);
                    if cb >= q && price <= p.quotes[0] {
                        color = blend(p.bg, p.bid, 0.8);
                    }
                    if ca >= q && price >= p.quotes[1] {
                        color = blend(p.bg, p.ask, 0.8);
                    }
                    if (x - hw).abs() < 0.6 {
                        color = p.grid;
                    }
                }
            }
            if n > 0 && p.size[2] == 1.0 && y >= h as f32 * 0.84 {
                color = p.bg;
                let slot = ((x / w as f32 * n as f32) as usize).min(n - 1);
                let col = scene.records[slot];
                let band = h as f32 * 0.08;
                if y < h as f32 * 0.92 {
                    if y >= h as f32 * 0.92 - band * col[2] / p.scale[2] {
                        color = p.bid;
                    }
                } else if y >= h as f32 - band * col[3] / p.scale[2] {
                    color = p.ask;
                }
            }
            result[yy * w + xx] = ((color[0].clamp(0.0, 1.0) * 255.0).round() as u32) << 16
                | ((color[1].clamp(0.0, 1.0) * 255.0).round() as u32) << 8
                | (color[2].clamp(0.0, 1.0) * 255.0).round() as u32;
        }
    }
    result
}
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    name: String,
    buffers: Option<GpuBuffers>,
}
struct GpuBuffers {
    uniform: wgpu::Buffer,
    records: wgpu::Buffer,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    bind: wgpu::BindGroup,
    record_bytes: u64,
    pixel_bytes: u64,
}
impl Gpu {
    fn new() -> Result<Self> {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                    ..Default::default()
                })
                .await?;
            let info = adapter.get_info();
            if info.device_type == wgpu::DeviceType::Cpu {
                bail!("software GPU adapter; use --renderer cpu");
            }
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await?;
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Lobo terminal chart kernel"),
                source: wgpu::ShaderSource::Wgsl(include_str!("chart.wgsl").into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Lobo chart raster"),
                layout: None,
                module: &module,
                entry_point: Some("chart"),
                compilation_options: Default::default(),
                cache: None,
            });
            Ok(Self {
                device,
                queue,
                pipeline,
                name: format!("{:?} · {}", info.backend, info.name),
                buffers: None,
            })
        })
    }
    fn draw(&mut self, scene: &Scene) -> Result<Vec<u32>> {
        let bytes =
            u64::from(scene.params.size[0] as u32) * u64::from(scene.params.size[1] as u32) * 4;
        let record_bytes = (scene.records.len() * std::mem::size_of::<[f32; 4]>()) as u64;
        if self
            .buffers
            .as_ref()
            .is_none_or(|b| b.record_bytes < record_bytes || b.pixel_bytes < bytes)
        {
            let previous = self.buffers.as_ref();
            let record_bytes = record_bytes
                .max(previous.map_or(0, |b| b.record_bytes))
                .next_power_of_two();
            let pixel_bytes = bytes
                .max(previous.map_or(0, |b| b.pixel_bytes))
                .next_power_of_two();
            let buffer = |label, size, usage| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage,
                    mapped_at_creation: false,
                })
            };
            let uniform = buffer(
                "Lobo chart parameters",
                std::mem::size_of::<Params>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            );
            let records = buffer(
                "Lobo chart records",
                record_bytes,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            let output = buffer(
                "Lobo chart pixels",
                pixel_bytes,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            let readback = buffer(
                "Lobo terminal readback",
                pixel_bytes,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            );
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Lobo persistent chart buffers"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: records.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: output.as_entire_binding(),
                    },
                ],
            });
            self.buffers = Some(GpuBuffers {
                uniform,
                records,
                output,
                readback,
                bind,
                record_bytes,
                pixel_bytes,
            });
        }
        let buffers = self.buffers.as_ref().unwrap();
        self.queue
            .write_buffer(&buffers.uniform, 0, bytemuck::bytes_of(&scene.params));
        self.queue
            .write_buffer(&buffers.records, 0, bytemuck::cast_slice(&scene.records));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &buffers.bind, &[]);
            pass.dispatch_workgroups(
                (scene.params.size[0] as u32).div_ceil(8),
                (scene.params.size[1] as u32).div_ceil(8),
                1,
            );
        }
        encoder.copy_buffer_to_buffer(&buffers.output, 0, &buffers.readback, 0, bytes);
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffers
            .readback
            .slice(..bytes)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(2)),
        })?;
        rx.recv_timeout(Duration::from_secs(2))
            .map_err(|e| anyhow!(e))??;
        let values =
            bytemuck::cast_slice::<u8, u32>(&buffers.readback.slice(..bytes).get_mapped_range()?)
                .to_vec();
        buffers.readback.unmap();
        Ok(values)
    }
}
