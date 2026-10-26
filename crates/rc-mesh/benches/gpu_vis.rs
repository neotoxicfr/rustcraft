//! CPU against GPU for the face communication of the sections (occlusion culling), on the
//! real sections of the reference capture. The GPU must return exactly the same thing.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use rayon::prelude::*;
use rc_mesh::visibility::{Vis, section_vis_bits, section_vis_fill, solid_bits};
use rc_proto::Reader;
use rc_proto_v777::chunk::{ChunkContext, read_chunk};
use wgpu::util::DeviceExt;

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Masks of the solid cells of all the non-empty sections of the capture.
fn sections() -> Option<Vec<[u64; 64]>> {
    let (cap, d) = (rc_testdata::reference_capture()?, rc_testdata::version_data()?);
    let solid: Vec<bool> = d.states.iter().map(|s| s.has(rc_data::flags::SOLID_RENDER)).collect();
    let ctx = ChunkContext::new(24, d.states.len() as u32, 67);
    let mut out = Vec::new();
    let mut values = vec![0u32; 4096];
    for p in rc_testdata::play_packets(&cap, &d, "minecraft:level_chunk_with_light") {
        let c = read_chunk(&mut Reader::new(&p), &ctx).ok()?;
        for s in c.sections.iter().filter(|s| !s.is_empty()) {
            s.blocks.unpack_into(&mut values);
            out.push(solid_bits(&values, &solid));
        }
    }
    Some(out)
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    input: wgpu::Buffer,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    bind: wgpu::BindGroup,
    n: u32,
}

impl Gpu {
    fn new(bits: &[[u64; 64]]) -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .ok()?;
        eprintln!("GPU: {}", adapter.get_info().name);
        let (device, queue) = pollster::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() }),
        )
        .ok()?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu_vis.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let words: Vec<u32> = bits.iter().flat_map(|b| b.iter().flat_map(|w| [*w as u32, (*w >> 32) as u32])).collect();
        let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let size = bits.len() as u64 * 8;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: input.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: output.as_entire_binding() },
            ],
        });
        Some(Gpu { device, queue, pipeline, input, output, readback, bind, n: bits.len() as u32 })
    }

    /// Upload of the masks (if `upload`), computation, readback: communications per section.
    fn run(&self, bits: Option<&[[u64; 64]]>) -> Vec<Vis> {
        if let Some(bits) = bits {
            let words: Vec<u32> =
                bits.iter().flat_map(|b| b.iter().flat_map(|w| [*w as u32, (*w >> 32) as u32])).collect();
            self.queue.write_buffer(&self.input, 0, bytemuck::cast_slice(&words));
        }
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.dispatch_workgroups(self.n, 1, 1);
        }
        enc.copy_buffer_to_buffer(&self.output, 0, &self.readback, 0, u64::from(self.n) * 8);
        self.queue.submit([enc.finish()]);
        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        let v: Vec<Vis> = {
            let data = slice.get_mapped_range().expect("readback");
            bytemuck::cast_slice::<u8, u32>(&data).chunks(2).map(|p| u64::from(p[0]) | u64::from(p[1]) << 32).collect()
        };
        self.readback.unmap();
        v
    }
}

fn bench(c: &mut Criterion) {
    let Some(bits) = sections() else {
        eprintln!("capture or cache absent");
        return;
    };
    eprintln!("{} non-empty sections", bits.len());
    let cpu: Vec<Vis> = bits.iter().map(section_vis_fill).collect();
    let mut g = c.benchmark_group("section_vis");
    g.sample_size(20);
    g.bench_function("cpu_fill_1t", |b| {
        b.iter(|| bits.iter().map(|s| black_box(section_vis_fill(s))).fold(0, |a, v| a ^ v))
    });
    let dil: Vec<Vis> = bits.iter().map(section_vis_bits).collect();
    assert_eq!(dil, cpu, "dilation by masks identical to the fill");
    eprintln!("dilation identical to the fill on all the sections");
    g.bench_function("cpu_dilate_1t", |b| {
        b.iter(|| bits.iter().map(|s| black_box(section_vis_bits(s))).fold(0, |a, v| a ^ v))
    });
    g.bench_function("cpu_parallel", |b| {
        b.iter(|| bits.par_iter().map(|s| black_box(section_vis_bits(s))).reduce(|| 0, |a, v| a ^ v))
    });
    if let Some(gpu) = Gpu::new(&bits) {
        assert_eq!(gpu.run(None), cpu, "the GPU must return the same communications");
        eprintln!("GPU identical to the CPU on all the sections");
        g.bench_function("gpu_with_upload", |b| b.iter(|| gpu.run(Some(&bits)).len()));
        g.bench_function("gpu_resident", |b| b.iter(|| gpu.run(None).len()));
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
