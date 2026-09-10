#![cfg(all(feature = "wgpu", not(target_arch = "wasm32")))]

mod common;

use std::{sync::Arc, time::Duration};

use arrow_array::{Array, RecordBatch, StringArray, StructArray, UInt64Array};
use arrow_data::ArrayData;
use arrow_schema::{ArrowError, DataType, Field};
use lobo_batchers::{
    arrow::{
        batch::PriceLevelArrowBatcher,
        sink::{GpuArray, GpuBatch, GpuBuffer, GpuRenderer, GpuSink},
    },
    traits::{Batch, Sink},
};

fn read_buffer(device: &wgpu::Device, queue: &wgpu::Queue, gpu: &GpuBuffer) -> Vec<u8> {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sink test readback"),
        size: gpu.buffer.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&gpu.buffer, 0, &readback, 0, gpu.buffer.size());
    let submission = queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(10)),
        })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    let view = readback.slice(..).get_mapped_range().unwrap();
    assert!(view[gpu.byte_len..].iter().all(|byte| *byte == 0));
    view[..gpu.byte_len].to_vec()
}

struct SequenceRenderer(wgpu::RenderPipeline);

impl SequenceRenderer {
    fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sink rendering test"),
            source: wgpu::ShaderSource::Wgsl(r#"
                @group(0) @binding(0) var<storage, read> sequences: array<vec2<u32>>;
                @vertex fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
                    let positions = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
                    return vec4(positions[index], 0.0, 1.0);
                }
                @fragment fn fragment() -> @location(0) vec4<f32> {
                    return vec4(f32(sequences[0].x) / 255.0, 0.0, 0.0, 1.0);
                }
            "#.into()),
        });
        Self(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("sink rendering test"),
                layout: None,
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
                    targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
                }),
                multiview_mask: None,
                cache: None,
            }),
        )
    }
}

impl GpuRenderer for SequenceRenderer {
    fn render(
        &mut self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        batch: &GpuBatch,
        target: &wgpu::TextureView,
    ) -> Result<(), ArrowError> {
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.0.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: batch.columns[0].buffers[0].buffer.as_entire_binding(),
            }],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
        pass.set_pipeline(&self.0);
        pass.set_bind_group(0, &bindings, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}

#[test]
#[ignore = "requires an available GPU adapter; run with --ignored"]
fn renders_uploaded_batches_into_the_same_gpu_target() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let mut sink = GpuSink::new(device.clone(), queue.clone());
    let mut renderer = SequenceRenderer::new(&device);
    let extent = wgpu::Extent3d {
        width: 1,
        height: 1,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("caller-owned render target"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut batcher = PriceLevelArrowBatcher::new(1);
    for sequence in [1, 17] {
        let batch = batcher.push(&common::event(sequence)).unwrap().unwrap();
        let submission = sink.write_to_view(&batch, &view, &mut renderer).unwrap();
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(10)),
            })
            .unwrap();

        // Test-only readback checks the pixels actually came from Arrow GPU
        // buffers. The production sink and renderer never map or read pixels.
        let pixels = GpuBuffer {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("test-only pixel verification"),
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            byte_len: 4,
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &pixels.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
                },
            },
            extent,
        );
        queue.submit([encoder.finish()]);
        assert_eq!(
            read_buffer(&device, &queue, &pixels),
            [sequence as u8, 0, 0, 255]
        );
    }
    sink.finish().unwrap();
    assert!(
        sink.write_to_view(&common::batch(), &view, &mut renderer)
            .is_err()
    );
}

fn assert_array(device: &wgpu::Device, queue: &wgpu::Queue, gpu: &GpuArray, cpu: &ArrayData) {
    assert_eq!(&gpu.data_type, cpu.data_type());
    assert_eq!(gpu.len, cpu.len());
    assert_eq!(gpu.offset, cpu.offset());
    assert_eq!(gpu.buffers.len(), cpu.buffers().len());
    for (gpu, cpu) in gpu.buffers.iter().zip(cpu.buffers()) {
        assert_eq!(read_buffer(device, queue, gpu), cpu.as_slice());
    }
    assert_eq!(gpu.nulls.is_some(), cpu.nulls().is_some());
    if let (Some(gpu), Some(cpu)) = (&gpu.nulls, cpu.nulls()) {
        assert_eq!(gpu.bit_offset, cpu.offset());
        assert_eq!(
            read_buffer(device, queue, &gpu.buffer),
            cpu.buffer().as_slice()
        );
    }
    assert_eq!(gpu.children.len(), cpu.child_data().len());
    for (gpu, cpu) in gpu.children.iter().zip(cpu.child_data()) {
        assert_array(device, queue, gpu, cpu);
    }
}

#[test]
#[ignore = "requires an available GPU adapter; run with --ignored"]
fn uploads_arrow_bytes_before_finish_and_keeps_batches_independent() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("GPU adapter required");
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let mut sink = GpuSink::new(device.clone(), queue.clone());
    let batch = common::batch();
    let uploaded = sink.write(&batch).unwrap();
    assert_eq!(uploaded.schema, batch.schema());
    assert_eq!(uploaded.num_rows, batch.num_rows());

    // Completion must be possible without a second write, flush, or finish.
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(uploaded.submission.clone()),
            timeout: Some(Duration::from_secs(10)),
        })
        .unwrap();

    let strings = StringArray::from(vec![Some("prefix"), None, Some("abc"), Some("")]);
    let numbers = UInt64Array::from(vec![Some(1), None, Some(u64::MAX), Some(0)]);
    let nested = StructArray::from(vec![(
        Arc::new(Field::new("nested_numbers", DataType::UInt64, true)),
        Arc::new(numbers.clone()) as Arc<dyn Array>,
    )]);
    let sliced = RecordBatch::try_from_iter(vec![
        ("strings", Arc::new(strings.slice(1, 3)) as Arc<dyn Array>),
        ("numbers", Arc::new(numbers.slice(1, 3)) as Arc<dyn Array>),
        ("nested", Arc::new(nested.slice(1, 3)) as Arc<dyn Array>),
    ])
    .unwrap();
    let next = sink.write(&sliced).unwrap();
    let empty = RecordBatch::new_empty(batch.schema());
    let empty_uploaded = sink.write(&empty).unwrap();

    // Later uploads must not overwrite any earlier batch's buffers.
    for (gpu, cpu) in [
        (&uploaded, &batch),
        (&next, &sliced),
        (&empty_uploaded, &empty),
    ] {
        for (gpu, cpu) in gpu.columns.iter().zip(cpu.columns()) {
            assert_array(&device, &queue, gpu, &cpu.to_data());
        }
    }
    sink.finish().unwrap();
    assert!(sink.write(&batch).is_err());
}
