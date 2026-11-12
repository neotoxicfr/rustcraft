//! GPU micro-bench of the cloud draws (ignored by default; loaded machine possible):
//! `cargo test --release -p rc-app cloud_gpu_bench -- --ignored --nocapture`.
//! Range 128 chunks (radius 171) over the 256 × 256 patchy map of `cloud_bench`, 1600 × 900,
//! classic pass; GPU time of the clouds pass read by timestamps. The variants alternate over
//! several rounds (a slow drift of the GPU clock spreads over all of them); median and best.

use super::*;
use crate::render::upload::tests::gpu_with_timestamps;

const WIDTH: u32 = 1600;
const HEIGHT: u32 = 900;
const FRAMES: u32 = 200;
const ROUNDS: usize = 4;

fn target(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// GPU time (ms) of the clouds pass over `FRAMES` frames of one view: eye height `y`, direction `dir`.
fn pass_times(device: &wgpu::Device, queue: &wgpu::Queue, opts: SkyOpts, (y, dir): (f64, [f32; 3])) -> Vec<f64> {
    let rgba = crate::sky_mesh::tests::patchy_map_rgba();
    let assets =
        SkyAssets { clouds: Some(rc_assets::texture::Image { width: 256, height: 256, rgba }), ..Default::default() };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut sky = Sky::with_opts(device, queue, format, &assets, opts);
    let mut up = Uploader::new(device, true);
    let (color, depth) = (target(device, format), target(device, wgpu::TextureFormat::Depth32Float));
    let proj = crate::render::perspective(70.0, WIDTH as f32 / HEIGHT as f32, 0.05, 4000.0);
    let vp = proj * glam::camera::rh::view::look_to_mat4(Vec3::ZERO, Vec3::from(dir).normalize(), Vec3::Y);
    let clouds = CloudFrame {
        color: [1.0, 1.0, 1.0, 0.8],
        height: 192.33,
        game_time: 6000,
        pt: 0.0,
        range_chunks: 128,
        cloud_end: 2048.0,
        flat: false,
    };
    sky.prepare(device, queue, &mut up, vp, &frame(0.3, Some(clouds), false), false, [10.0, y, 20.0]);
    up.submit(queue, device.create_command_encoder(&Default::default()).finish());
    assert!(sky.has_clouds(), "clouds in view");
    let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: None,
        ty: wgpu::QueryType::Timestamp,
        count: 2 * FRAMES,
    });
    for f in 0..FRAMES {
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.2, g: 0.4, b: 0.9, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(crate::render::DEPTH_CLEAR),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: Some(wgpu::RenderPassTimestampWrites {
                    query_set: &queries,
                    beginning_of_pass_write_index: Some(2 * f),
                    end_of_pass_write_index: Some(2 * f + 1),
                }),
                ..Default::default()
            });
            sky.draw_clouds(&mut pass);
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
    let ticks: Vec<u64> = bytemuck::cast_slice(&read(device, queue, &resolved)).to_vec();
    let period_ns = f64::from(queue.get_timestamp_period());
    ticks.chunks(2).map(|t| t[1].saturating_sub(t[0]) as f64 * period_ns / 1e6).collect()
}

#[test]
#[ignore]
fn cloud_gpu_bench() {
    let Some((device, queue)) = gpu_with_timestamps() else {
        eprintln!("no hardware GPU with timestamp queries");
        return;
    };
    let old = SkyOpts { persist: true, spiral: true, unif: true, ..SkyOpts::OLD };
    let variants = [
        ("mesh, 6 vertices per face (old)", old),
        ("instanced faces, constant arrays", SkyOpts { indexed: false, packed_constants: false, ..NEW }),
        ("instanced faces, packed constants", SkyOpts { indexed: false, ..NEW }),
        ("indexed faces, constant arrays", SkyOpts { packed_constants: false, ..NEW }),
        ("indexed faces, packed constants", NEW),
    ];
    let views =
        [("below, toward the sky", 150.0, [0.3, 0.6, 1.0]), ("above, toward the ground", 260.0, [0.2, -0.6, 1.0])];
    for (view, y, dir) in views {
        let mut times = vec![Vec::new(); variants.len()];
        for _ in 0..ROUNDS {
            for (t, (_, opts)) in times.iter_mut().zip(variants) {
                t.extend(pass_times(&device, &queue, opts, (y, dir)));
            }
        }
        println!("{view} (range 128, {WIDTH} x {HEIGHT}, {} frames per variant):", FRAMES as usize * ROUNDS);
        for ((label, _), t) in variants.iter().zip(&mut times) {
            t.sort_by(f64::total_cmp);
            println!("    {label}: median {:.4} ms, best {:.4} ms", t[t.len() / 2], t[0]);
        }
    }
}
