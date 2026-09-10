//! A presentation-only uniform shared by both chart pipelines.
use crate::presentation::THEME_BYTES;

pub struct Palette {
    pub layout: wgpu::BindGroupLayout,
    pub group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
    colors: [f32; 52],
    dirty: bool,
}

impl Palette {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Chart palette"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(THEME_BYTES),
                },
                count: None,
            }],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Chart palette colors"),
            size: THEME_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Chart palette"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        // Standalone WASM callers default to Nordic Violet. The web app supplies
        // its resolved CSS tokens at startup and on theme changes.
        let defaults = [
            0x090a10u32,
            0x0f111a,
            0x202435,
            0x22c55e,
            0xf43f5e,
            0x10291f,
            0x2e1523,
            0x86efac,
            0xfda4af,
            0x281622,
            0x261f15,
            0x818cf8,
            0xc7d2fe,
        ];
        let mut colors = [1.0; 52];
        for (color, rgb) in colors.chunks_exact_mut(4).zip(defaults) {
            color[0] = ((rgb >> 16) & 255) as f32 / 255.0;
            color[1] = ((rgb >> 8) & 255) as f32 / 255.0;
            color[2] = (rgb & 255) as f32 / 255.0;
        }
        Self {
            layout,
            group,
            buffer,
            colors,
            dirty: true,
        }
    }

    /// Validate at the UI boundary, outside message publication and aggregation.
    pub fn set(&mut self, colors: &[f32]) -> Result<(), &'static str> {
        if colors.len() != self.colors.len()
            || colors
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err("Chart theme requires thirteen normalized RGBA colors");
        }
        if self.colors != colors {
            self.colors.copy_from_slice(colors);
            self.dirty = true;
        }
        Ok(())
    }

    pub fn upload(&mut self, queue: &wgpu::Queue) {
        if self.dirty {
            let mut bytes = [0; THEME_BYTES as usize];
            for (bytes, value) in bytes.chunks_exact_mut(4).zip(self.colors) {
                bytes.copy_from_slice(&value.to_le_bytes());
            }
            queue.write_buffer(&self.buffer, 0, &bytes);
            self.dirty = false;
        }
    }
}
