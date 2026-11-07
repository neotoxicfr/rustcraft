//! Resource reload on a kept device (`RUSTCRAFT_OPT_KEEP_GPU_ON_RELOAD`): the memory of the old
//! renderer is given back before the new one allocates, so a reload never holds both sets of block
//! textures. Played with the block texture arrays the renderer builds (the largest allocation it
//! makes from the pack). Sabotage checked: without [`wait_for_released_gpu_memory`] the old arrays
//! are still allocated when the new ones are built. Without a hardware GPU or an allocator report the
//! test skips itself; under `RUSTCRAFT_OPT_REQUIRE=1` it is a failure.

use super::pipelines::block_texture_array;
use super::wait_for_released_gpu_memory;
use rc_assets::texture::Image;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    crate::render::upload::tests::gpu_serial();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok();
    let hardware = adapter.filter(|a| a.get_info().device_type != wgpu::DeviceType::Cpu);
    let adapter = rc_testdata::need(hardware, "hardware GPU adapter")?;
    // Same memory setting as the game's device (`gpu_backend::open_device`).
    let desc = wgpu::DeviceDescriptor { memory_hints: wgpu::MemoryHints::MemoryUsage, ..Default::default() };
    Some(pollster::block_on(adapter.request_device(&desc)).expect("device"))
}

/// Bytes the GPU allocator holds for resources (blocks kept for reuse not counted).
fn allocated(device: &wgpu::Device) -> Option<u64> {
    device.generate_allocator_report().map(|r| r.total_allocated_bytes)
}

/// `count` block textures of `side` pixels with their mip chain, like a baked pack.
fn pack_textures(side: u32, count: usize) -> Vec<Vec<Image>> {
    let chain = |seed: usize| {
        (0..=side.ilog2())
            .map(|level| {
                let s = side >> level;
                let rgba = (0..s * s * 4).map(|i| (i as usize * 31 + seed * 7) as u8).collect();
                Image { width: s, height: s, rgba }
            })
            .collect()
    };
    (0..count).map(chain).collect()
}

/// The block arrays of one renderer, written and submitted like at its creation.
fn build_arrays(device: &wgpu::Device, queue: &wgpu::Queue, pack: &[Vec<Image>]) -> wgpu::TextureView {
    let layers: Vec<&Vec<Image>> = pack.iter().collect();
    let (view, _) = block_texture_array(device, queue, "block textures", 64, &layers);
    queue.submit([]);
    view
}

#[test]
fn reload_on_a_kept_device_gives_the_old_memory_back_first() {
    let Some((device, queue)) = device() else { return };
    let Some(baseline) = rc_testdata::need(allocated(&device), "GPU allocator report") else { return };
    let pack = pack_textures(64, 96);
    // Size of one set once its writes are done.
    let sizing = build_arrays(&device, &queue, &pack);
    wait_for_released_gpu_memory(&device);
    let one_set = allocated(&device).expect("report").saturating_sub(baseline);
    assert!(one_set >= 64 * 64 * 4 * 96, "block arrays not allocated: {one_set} bytes");
    drop(sizing);
    wait_for_released_gpu_memory(&device);
    // Reload: the old renderer's arrays are still used by its last submission when it is dropped;
    // its memory is given back, then the new renderer is built.
    let old = build_arrays(&device, &queue, &pack);
    drop(old);
    wait_for_released_gpu_memory(&device);
    let between = allocated(&device).expect("report").saturating_sub(baseline);
    let second = build_arrays(&device, &queue, &pack);
    wait_for_released_gpu_memory(&device);
    let rebuilt = allocated(&device).expect("report").saturating_sub(baseline);
    eprintln!("block arrays: {one_set} bytes, after release {between}, rebuilt {rebuilt}");
    assert_eq!(between, 0, "old arrays still allocated when the new renderer starts");
    assert_eq!(rebuilt, one_set, "the rebuilt arrays do not take the same memory");
    drop(second);
}
