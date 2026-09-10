//! A bounded glyph pass. It never prepares market batches, scans books, or reads back.
use crate::presentation;
use wgpu::util::DeviceExt;

pub struct LoadingRenderer {
    pipeline: wgpu::RenderPipeline,
    params: wgpu::Buffer,
    group: wgpu::BindGroup,
    glyphs: wgpu::Buffer,
    count: u32,
    pub active: bool,
    pub time: f32,
    pub scale: f32,
    pub tickers: Vec<String>,
    previous: (u32, u32, u32),
    dirty: bool,
}
impl LoadingRenderer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        palette: &wgpu::BindGroupLayout,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Loading clock"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Loading viewport"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ticker loading shader"),
            source: wgpu::ShaderSource::Wgsl(
                presentation::themed_shader(include_str!("loading.wgsl"), format.is_srgb()).into(),
            ),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout), Some(palette)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Ticker loading pass"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vertex"), compilation_options: Default::default(), buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Instance, attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x2, 2 => Uint32x2] })] },
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fragment"), compilation_options: Default::default(), targets: &[Some(format.into())] }),
            multiview_mask: None, cache: None,
        });
        Self {
            pipeline,
            params,
            group,
            glyphs: device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 32,
                usage: wgpu::BufferUsages::VERTEX,
                mapped_at_creation: false,
            }),
            count: 0,
            active: false,
            time: 0.0,
            scale: 1.0,
            tickers: Vec::new(),
            previous: (0, 0, 0),
            dirty: true,
        }
    }
    pub fn set_tickers(&mut self, tickers: Vec<String>) {
        let mut seen = std::collections::BTreeSet::new();
        let tickers: Vec<_> = tickers
            .into_iter()
            .filter(|s| seen.insert(s.clone()))
            .take(100)
            .collect();
        if self.tickers != tickers {
            self.tickers = tickers;
            self.dirty = true;
        }
    }
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        palette: &wgpu::BindGroup,
        width: u32,
        height: u32,
    ) {
        let viewport = (width, height, self.scale.to_bits());
        let w = width as f32 / self.scale;
        let h = height as f32 / self.scale;
        if self.dirty || self.previous != viewport {
            let n = self.tickers.len().max(1);
            let columns = ((n as f32 * w / h / 4.0).sqrt().ceil() as usize).clamp(1, n);
            let rows = n.div_ceil(columns);
            let cw = w / columns as f32;
            let ch = h / rows as f32;
            let mut bytes = Vec::new();
            // Instance zero draws the background using this same tiny pipeline.
            bytes.extend_from_slice(&[0u8; 32]);
            for (i, ticker) in self.tickers.iter().enumerate() {
                let size = (ch * 0.48)
                    .min(cw * 0.85 / (ticker.len().max(1) as f32 * 0.65))
                    .min(56.0);
                let text_width = ticker.len() as f32 * size * 0.65;
                let x = (i % columns) as f32 * cw + (cw - text_width) * 0.5;
                let y = (i / columns) as f32 * ch + (ch - size) * 0.5;
                for (index, letter) in ticker.bytes().take(64).enumerate() {
                    for value in [
                        x,
                        y,
                        index as f32,
                        size,
                        text_width,
                        0.5 + (i % 5) as f32 * 0.12,
                    ] {
                        bytes.extend(value.to_le_bytes());
                    }
                    for mask in glyph(letter) {
                        bytes.extend(mask.to_le_bytes());
                    }
                }
            }
            self.count = (bytes.len() / 32) as u32;
            self.glyphs = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("At most 100 ticker glyphs"),
                contents: &bytes,
                usage: wgpu::BufferUsages::VERTEX,
            });
            self.previous = viewport;
            self.dirty = false;
        }
        let bytes: Vec<_> = [w, h, self.time, self.scale]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        queue.write_buffer(&self.params, 0, &bytes);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Loading tickers only"),
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
        pass.set_vertex_buffer(0, self.glyphs.slice(..));
        pass.draw(0..6, 0..self.count);
    }
}

/// Five columns by seven rows, uppercase ASCII market symbols. Unknown printable
/// punctuation gets a visible box, so it cannot silently disappear from a symbol.
fn glyph(letter: u8) -> [u32; 2] {
    let rows = match letter {
        b'A' => [14, 17, 17, 31, 17, 17, 17],
        b'B' => [30, 17, 17, 30, 17, 17, 30],
        b'C' => [14, 17, 16, 16, 16, 17, 14],
        b'D' => [30, 17, 17, 17, 17, 17, 30],
        b'E' => [31, 16, 16, 30, 16, 16, 31],
        b'F' => [31, 16, 16, 30, 16, 16, 16],
        b'G' => [14, 17, 16, 23, 17, 17, 15],
        b'H' => [17, 17, 17, 31, 17, 17, 17],
        b'I' => [14, 4, 4, 4, 4, 4, 14],
        b'J' => [7, 2, 2, 2, 2, 18, 12],
        b'K' => [17, 18, 20, 24, 20, 18, 17],
        b'L' => [16, 16, 16, 16, 16, 16, 31],
        b'M' => [17, 27, 21, 21, 17, 17, 17],
        b'N' => [17, 25, 21, 19, 17, 17, 17],
        b'O' => [14, 17, 17, 17, 17, 17, 14],
        b'P' => [30, 17, 17, 30, 16, 16, 16],
        b'Q' => [14, 17, 17, 17, 21, 18, 13],
        b'R' => [30, 17, 17, 30, 20, 18, 17],
        b'S' => [15, 16, 16, 14, 1, 1, 30],
        b'T' => [31, 4, 4, 4, 4, 4, 4],
        b'U' => [17, 17, 17, 17, 17, 17, 14],
        b'V' => [17, 17, 17, 17, 17, 10, 4],
        b'W' => [17, 17, 17, 21, 21, 21, 10],
        b'X' => [17, 17, 10, 4, 10, 17, 17],
        b'Y' => [17, 17, 10, 4, 4, 4, 4],
        b'Z' => [31, 1, 2, 4, 8, 16, 31],
        b'0' => [14, 17, 19, 21, 25, 17, 14],
        b'1' => [4, 12, 4, 4, 4, 4, 14],
        b'2' => [14, 17, 1, 2, 4, 8, 31],
        b'3' => [30, 1, 1, 14, 1, 1, 30],
        b'4' => [2, 6, 10, 18, 31, 2, 2],
        b'5' => [31, 16, 16, 30, 1, 1, 30],
        b'6' => [14, 16, 16, 30, 17, 17, 14],
        b'7' => [31, 1, 2, 4, 8, 8, 8],
        b'8' => [14, 17, 17, 14, 17, 17, 14],
        b'9' => [14, 17, 17, 15, 1, 1, 14],
        b'.' => [0, 0, 0, 0, 0, 6, 6],
        b'/' => [1, 1, 2, 4, 8, 16, 16],
        b'-' => [0, 0, 0, 31, 0, 0, 0],
        b'_' => [0, 0, 0, 0, 0, 0, 31],
        b':' => [0, 6, 6, 0, 6, 6, 0],
        b'^' => [4, 10, 17, 0, 0, 0, 0],
        b'+' => [0, 4, 4, 31, 4, 4, 0],
        b'$' => [4, 15, 20, 14, 5, 30, 4],
        b'!' => [4, 4, 4, 4, 4, 0, 4],
        b'"' => [10, 10, 10, 0, 0, 0, 0],
        b'#' => [10, 31, 10, 10, 31, 10, 0],
        b'%' => [25, 25, 2, 4, 8, 19, 19],
        b'&' => [12, 18, 20, 8, 21, 18, 13],
        b'\'' => [4, 4, 8, 0, 0, 0, 0],
        b'(' => [2, 4, 8, 8, 8, 4, 2],
        b')' => [8, 4, 2, 2, 2, 4, 8],
        b'*' => [0, 21, 14, 31, 14, 21, 0],
        b',' => [0, 0, 0, 0, 6, 4, 8],
        b';' => [0, 6, 6, 0, 6, 4, 8],
        b'<' => [2, 4, 8, 16, 8, 4, 2],
        b'=' => [0, 0, 31, 0, 31, 0, 0],
        b'>' => [8, 4, 2, 1, 2, 4, 8],
        b'?' => [14, 17, 1, 2, 4, 0, 4],
        b'@' => [14, 17, 23, 21, 23, 16, 14],
        b'[' => [14, 8, 8, 8, 8, 8, 14],
        b'\\' => [16, 16, 8, 4, 2, 1, 1],
        b']' => [14, 2, 2, 2, 2, 2, 14],
        b'`' => [8, 4, 2, 0, 0, 0, 0],
        b'{' => [3, 4, 4, 8, 4, 4, 3],
        b'|' => [4, 4, 4, 4, 4, 4, 4],
        b'}' => [24, 4, 4, 2, 4, 4, 24],
        b'~' => [0, 0, 9, 22, 0, 0, 0],
        b' ' => [0; 7],
        _ => [31, 17, 17, 17, 17, 17, 31],
    };
    let mut bits = 0u64;
    for (y, row) in rows.into_iter().enumerate() {
        for x in 0..5 {
            if row & (1 << (4 - x)) != 0 {
                bits |= 1 << (y * 5 + x);
            }
        }
    }
    [bits as u32, (bits >> 32) as u32]
}
