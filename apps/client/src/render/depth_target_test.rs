//! Proofs of the depth target paths (`RUSTCRAFT_OPT_DEPTH_DISCARD`, `RUSTCRAFT_OPT_DEPTH_NO_SAMPLING`).
//!
//! On a real GPU, a world pass writes a triangle's depth, then the next pass of the frame either
//! reads that depth (improved transparency) or clears it (every other frame): drawn with the depth
//! kept at the end of the world pass as decided by [`DepthPaths`] and as before, the images are equal
//! to the byte, with and without improved transparency, and with a depth target made with or
//! without the texture usage. Sabotage: the depth discarded although improved transparency reads it
//! changes the image (wgpu clears a discarded target before the next read). The depth target made
//! without the texture usage cannot be bound by improved transparency: the usage decided for a
//! frame with it on is the one that allows it.

use super::{DepthPaths, depth_view};
use crate::render::upload::tests::{gpu, gpu_with_timestamps, read};

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const SHADER: &str = r"
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
};

// Instance 0: the world's triangle (lower left half, depth 0.5, red). Instance 1: a full-screen
// triangle behind it (depth 0.3, green), drawn by the pass that follows.
@vertex
fn vs(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> Out {
    var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0));
    var depth = 0.5;
    var color = vec4<f32>(1.0, 0.0, 0.0, 1.0);
    if i == 1u {
        corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
        depth = 0.3;
        color = vec4<f32>(0.0, 1.0, 0.0, 1.0);
    }
    var o: Out;
    o.pos = vec4<f32>(corners[v], depth, 1.0);
    o.color = color;
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    return i.color;
}
";

fn pipeline(device: &wgpu::Device) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(super::super::pipelines::terrain_depth_state(true)),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// Image of a frame: world pass with the depth kept by `store`, then the next pass reads the depth
/// (`reader`, improved transparency) or clears it; depth target with the texture usage or not.
fn frame(device: &wgpu::Device, queue: &wgpu::Queue, store: wgpu::StoreOp, reader: bool, sampled: bool) -> Vec<u8> {
    let pipeline = pipeline(device);
    let depth = depth_view(device, SIZE, SIZE, sampled);
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    let run_pass = |enc: &mut wgpu::CommandEncoder,
                    color_load: wgpu::LoadOp<wgpu::Color>,
                    depth_ops: wgpu::Operations<f32>,
                    instance: u32| {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: color_load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(depth_ops),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.draw(0..3, instance..instance + 1);
    };
    let clear = wgpu::LoadOp::Clear(crate::render::DEPTH_CLEAR);
    run_pass(&mut enc, wgpu::LoadOp::Clear(wgpu::Color::BLACK), wgpu::Operations { load: clear, store }, 0);
    let next = if reader { wgpu::LoadOp::Load } else { clear };
    run_pass(&mut enc, wgpu::LoadOp::Load, wgpu::Operations { load: next, store: wgpu::StoreOp::Discard }, 1);
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &out,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: None },
        },
        wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    read(device, queue, &out)
}

/// Pixels of the world's triangle left on top (red).
fn red_pixels(image: &[u8]) -> usize {
    image.as_chunks::<4>().0.iter().filter(|p| p[0] == 255).count()
}

#[test]
fn depth_paths_draw_the_same_frames_on_gpu() {
    let Some((device, queue)) = gpu() else { return };
    let new = DepthPaths { discard_unread: true, sampled_only_for_transparency: true };
    for improved_transparency in [false, true] {
        let old = frame(
            &device,
            &queue,
            DepthPaths::OLD.world_pass_store(improved_transparency),
            improved_transparency,
            true,
        );
        let sampled = new.depth_sampled(improved_transparency);
        let got = frame(&device, &queue, new.world_pass_store(improved_transparency), improved_transparency, sampled);
        assert!(got == old, "improved transparency {improved_transparency}: the frame changed");
        // The reader sees the world's depth: its triangle stays in front; cleared, it is drawn over.
        let red = red_pixels(&old);
        if improved_transparency {
            assert!(red > (SIZE * SIZE / 3) as usize, "the world's depth was not read: {red} pixels");
        } else {
            assert_eq!(red, 0, "the depth was not cleared");
        }
    }
    // Sabotage: depth discarded although improved transparency reads it next.
    let kept = frame(&device, &queue, wgpu::StoreOp::Store, true, true);
    let discarded = frame(&device, &queue, wgpu::StoreOp::Discard, true, true);
    assert!(kept != discarded, "a discarded depth read by the next pass is not seen");
}

/// Improved transparency binds the depth target as a texture: refused for a target made without
/// the texture usage, accepted for the usage decided for a frame with improved transparency.
#[test]
fn improved_transparency_gets_a_depth_it_can_read() {
    let Some((device, _queue)) = gpu() else { return };
    if !device.features().contains(wgpu::Features::FLOAT32_BLENDABLE) {
        return;
    }
    let new = DepthPaths { discard_unread: true, sampled_only_for_transparency: true };
    let binds = |sampled: bool| {
        let depth = depth_view(&device, SIZE, SIZE, sampled);
        let mut oit = crate::oit::Oit::new(&device, FORMAT);
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        oit.ensure(&device, (SIZE, SIZE), &depth);
        pollster::block_on(scope.pop()).is_none()
    };
    assert!(binds(new.depth_sampled(true)), "the depth made for improved transparency cannot be bound");
    assert!(!new.depth_sampled(false), "the depth keeps the texture usage without improved transparency");
    assert!(!binds(false), "a depth without the texture usage was bound: the check sees nothing");
    assert!(DepthPaths::OLD.depth_sampled(false), "old path: always made with the texture usage");
}

/// GPU time of a frame without improved transparency, 1600 × 900: world pass (full-screen depth
/// written, kept or discarded), then a pass that clears the depth, with the depth target made with
/// or without the texture usage (`cargo test --release -p rc-app depth_store_cost -- --ignored
/// --nocapture`). Timestamps from the start of the first pass to the end of the second; the forms
/// alternate over rounds; median and best.
#[test]
#[ignore]
fn depth_store_cost() {
    const FRAMES: u32 = 500;
    const ROUNDS: usize = 4;
    let Some((device, queue)) = gpu_with_timestamps() else {
        eprintln!("no hardware GPU with timestamp queries");
        return;
    };
    let size = (1600, 900);
    let pipeline = pipeline(&device);
    let color = device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default());
    let forms = [
        ("depth kept, sampled (old)", wgpu::StoreOp::Store, true),
        ("depth discarded, sampled", wgpu::StoreOp::Discard, true),
        ("depth kept, not sampled", wgpu::StoreOp::Store, false),
        ("depth discarded, not sampled (new)", wgpu::StoreOp::Discard, false),
    ];
    let mut times = vec![Vec::new(); forms.len()];
    for _ in 0..ROUNDS {
        for (k, &(_, store, sampled)) in forms.iter().enumerate() {
            let depth = depth_view(&device, size.0, size.1, sampled);
            let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
                label: None,
                ty: wgpu::QueryType::Timestamp,
                count: 2 * FRAMES,
            });
            for f in 0..FRAMES {
                let mut enc = device.create_command_encoder(&Default::default());
                for (instance, store, stamp) in
                    [(1, store, (Some(2 * f), None)), (0, wgpu::StoreOp::Discard, (None, Some(2 * f + 1)))]
                {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &color,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &depth,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(crate::render::DEPTH_CLEAR),
                                store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: Some(wgpu::RenderPassTimestampWrites {
                            query_set: &queries,
                            beginning_of_pass_write_index: stamp.0,
                            end_of_pass_write_index: stamp.1,
                        }),
                        ..Default::default()
                    });
                    pass.set_pipeline(&pipeline);
                    pass.draw(0..3, instance..instance + 1);
                }
                queue.submit([enc.finish()]);
            }
            let resolved = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 16 * u64::from(FRAMES),
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let mut enc = device.create_command_encoder(&Default::default());
            enc.resolve_query_set(&queries, 0..2 * FRAMES, &resolved, 0);
            queue.submit([enc.finish()]);
            let ticks: Vec<u64> = bytemuck::cast_slice(&read(&device, &queue, &resolved)).to_vec();
            let period_ns = f64::from(queue.get_timestamp_period());
            times[k].extend(ticks.chunks(2).map(|t| t[1].saturating_sub(t[0]) as f64 * period_ns / 1e3));
        }
    }
    println!("depth target, {} x {}, {} frames per form:", size.0, size.1, FRAMES as usize * ROUNDS);
    for ((label, ..), t) in forms.iter().zip(&mut times) {
        t.sort_by(f64::total_cmp);
        println!("    {label}: median {:.2} µs, best {:.2} µs", t[t.len() / 2], t[0]);
    }
}
