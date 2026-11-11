//! Proofs of the menu blur on a real GPU, at radii 0, 5 and 10, on two frame formats and two
//! frame sizes, one blur object going through every size and radius in a row:
//! - the fragment forms (each lead alone, the defaults) leave the bytes of the form before, which
//!   themselves stay within one step of a CPU model of the six box averages (a GPU does not have
//!   to round an 8-bit write to the nearest);
//! - the compute forms leave the bytes of the model.
//!
//! Sabotages, each seen: a model one texel wider, the compute average truncated instead of
//! rounded, a compute window one texel short, compute edges not repeated, the configurations never
//! written again after the first frame.
//! Without a GPU the tests skip themselves; under `RUSTCRAFT_OPT_REQUIRE=1` it is a failure.

use std::time::{Duration, Instant};

use super::*;

/// A frame the passes copy from and draw into, like the window's.
pub(crate) struct TestFrame {
    pub(crate) texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    pub(crate) size: (u32, u32),
}

impl TestFrame {
    pub(crate) fn new(device: &wgpu::Device, size: (u32, u32), format: wgpu::TextureFormat) -> TestFrame {
        let usage =
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST;
        let texture = work_texture(device, "test frame", size, format, usage);
        let view = texture.create_view(&Default::default());
        TestFrame { texture, view, size }
    }

    /// Fills the frame with `bytes` (4 per texel, in the order of the format).
    pub(crate) fn fill(&self, queue: &wgpu::Queue, bytes: &[u8]) {
        let layout =
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.size.0 * 4), rows_per_image: None };
        queue.write_texture(self.texture.as_image_copy(), bytes, layout, self.texture.size());
    }

    /// Bytes of the frame, rows without padding.
    pub(crate) fn read(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
        let row = u64::from(self.size.0) * 4;
        let padded = row.next_multiple_of(u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT));
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: padded * u64::from(self.size.1),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout =
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded as u32), rows_per_image: None };
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buffer, layout },
            self.texture.size(),
        );
        queue.submit([enc.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
        let mapped = buffer.slice(..).get_mapped_range().expect("read");
        mapped.chunks(padded as usize).flat_map(|padded_row| &padded_row[..row as usize]).copied().collect()
    }
}

/// Deterministic image in four vertical bands: noise, gradients, small two-colour blocks and
/// saturated noise, in every channel (alpha included).
pub(crate) fn test_image((width, height): (u32, u32), seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let block_colours = [(random() as u32).to_le_bytes(), [250, 3, 128, 255]];
    let mut image = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let noise = (random() as u32).to_le_bytes();
            let texel: [u8; 4] = match x * 4 / width {
                0 => noise,
                1 => [(x * 255 / width) as u8, (y * 255 / height) as u8, ((x + y) % 256) as u8, (255 - y % 256) as u8],
                2 => block_colours[((x / 3 + y / 5) % 2) as usize],
                _ => noise.map(|n| if n & 1 == 0 { 0 } else { 255 }),
            };
            image.extend_from_slice(&texel);
        }
    }
    image
}

/// How many bytes differ, and the largest step between two of them.
pub(crate) fn difference(got: &[u8], want: &[u8]) -> (usize, u8) {
    assert_eq!(got.len(), want.len(), "images of the same size");
    let steps = got.iter().zip(want).map(|(g, w)| g.abs_diff(*w));
    steps.fold((0, 0), |(count, largest), step| (count + usize::from(step > 0), largest.max(step)))
}

/// One box pass as the blur stores it: each byte becomes the average of the `2 radius + 1` bytes
/// of its channel along the pass direction (edge texels repeated), rounded to the nearest.
fn box_pass(image: &[u8], (width, height): (u32, u32), radius: i64, horizontal: bool) -> Vec<u8> {
    let (width, height) = (i64::from(width), i64::from(height));
    let texel = |x: i64, y: i64, channel: usize| {
        let (x, y) = (x.clamp(0, width - 1), y.clamp(0, height - 1));
        u32::from(image[((y * width + x) * 4) as usize + channel])
    };
    let count = (2 * radius + 1) as u32;
    let mut out = Vec::with_capacity(image.len());
    for y in 0..height {
        for x in 0..width {
            for channel in 0..4 {
                let sum: u32 = (-radius..=radius)
                    .map(|k| if horizontal { texel(x + k, y, channel) } else { texel(x, y + k, channel) })
                    .sum();
                // Nearest integer: the count is odd, there is no tie.
                out.push(((2 * sum + count) / (2 * count)) as u8);
            }
        }
    }
    out
}

/// Image the six passes leave: horizontal then vertical, three times.
pub(crate) fn blurred_model(image: &[u8], size: (u32, u32), radius: i32) -> Vec<u8> {
    let mut current = image.to_vec();
    for pass in 0..6 {
        current = box_pass(&current, size, i64::from(radius), pass % 2 == 0);
    }
    current
}

/// `image` blurred in `frame` by `blur`, read back.
fn blurred_on_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    up: &mut Uploader,
    blur: &mut Blur,
    frame: &TestFrame,
    image: &[u8],
    radius: i32,
) -> Vec<u8> {
    frame.fill(queue, image);
    let mut enc = device.create_command_encoder(&Default::default());
    blur.run(device, queue, up, &mut enc, &frame.texture, &frame.view, radius, None);
    up.submit(queue, enc.finish());
    frame.read(device, queue)
}

const FORMATS: [wgpu::TextureFormat; 2] = [wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Rgba8Unorm];
/// A small frame and one with odd sides, wider than several compute runs.
const SIZES: [(u32, u32); 2] = [(320, 240), (853, 479)];
const RADII: [i32; 3] = [0, 5, 10];

/// Fragment forms: each lead alone, then the switches' defaults.
const FRAGMENT_FORMS: [(&str, Options); 4] = [
    ("before", Options::OLD),
    ("configurations on change", Options { skip_unchanged_config: true, ..Options::OLD }),
    ("idle targets freed", Options { release_idle_targets: true, ..Options::OLD }),
    ("defaults", Options::NEW),
];
/// Compute forms: the lead alone, then with the defaults.
const COMPUTE_FORMS: [(&str, Options); 2] = [
    ("compute passes", Options { compute_passes: true, ..Options::OLD }),
    ("defaults and compute passes", Options { compute_passes: true, ..Options::NEW }),
];

/// One blurred frame of the script: frame size, radius and source image.
struct ScriptFrame {
    size: (u32, u32),
    radius: i32,
    image: Vec<u8>,
}

/// Every size at every radius, in a row.
fn script() -> Vec<ScriptFrame> {
    let sizes = SIZES.into_iter().enumerate();
    let frames = sizes.flat_map(|(n, size)| {
        let image = test_image(size, 0x51ed_270b + n as u64);
        RADII.map(|radius| ScriptFrame { size, radius, image: image.clone() })
    });
    frames.collect()
}

/// Images of the script blurred by one blur object.
fn play_script(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    blur: &mut Blur,
    format: wgpu::TextureFormat,
) -> Vec<Vec<u8>> {
    let mut up = Uploader::new(device, true);
    let blurred = |frame: ScriptFrame| {
        let target = TestFrame::new(device, frame.size, format);
        blurred_on_gpu(device, queue, &mut up, blur, &target, &frame.image, frame.radius)
    };
    script().into_iter().map(blurred).collect()
}

/// The images the model computes for the script.
fn model_of_script() -> Vec<Vec<u8>> {
    script().iter().map(|frame| blurred_model(&frame.image, frame.size, frame.radius)).collect()
}

/// Bytes that differ between two plays of the script, and the largest step between two of them.
fn script_difference(got: &[Vec<u8>], want: &[Vec<u8>]) -> (usize, u8) {
    let per_frame = got.iter().zip(want).map(|(g, w)| difference(g, w));
    per_frame.fold((0, 0), |(count, largest), (c, l)| (count + c, largest.max(l)))
}

#[test]
fn fragment_forms_draw_the_images_of_before_on_gpu() {
    let Some((device, queue)) = crate::render::upload::tests::gpu() else { return };
    let model = model_of_script();
    let total_bytes: usize = model.iter().map(Vec::len).sum();
    for format in FORMATS {
        let play = |options| play_script(&device, &queue, &mut Blur::with_options(&device, format, options), format);
        let want = play(Options::OLD);
        // The form before is the blur: its images are the box averages, to one step (the GPU's
        // own rounding of each 8-bit write), and radius 0 changes nothing.
        let (differing, largest_step) = script_difference(&want, &model);
        eprintln!(
            "{format:?}, six fragment passes against the box averages: {:.1} % of the bytes one step away",
            differing as f64 * 100.0 / total_bytes as f64
        );
        assert!(largest_step <= 1, "{format:?}: fragment passes {largest_step} steps away from the box averages");
        for (frame, image) in script().iter().zip(&want).filter(|(frame, _)| frame.radius == 0) {
            assert_eq!(difference(image, &frame.image), (0, 0), "{format:?} {:?}: radius 0", frame.size);
        }
        for (name, options) in &FRAGMENT_FORMS[1..] {
            let (differing, largest_step) = script_difference(&play(*options), &want);
            assert_eq!(differing, 0, "form '{name}' on {format:?}: largest step {largest_step}");
        }
    }
}

#[test]
fn compute_forms_store_the_six_box_averages_on_gpu() {
    let Some((device, queue)) = crate::render::upload::tests::gpu() else { return };
    let model = model_of_script();
    for format in FORMATS {
        for (name, options) in COMPUTE_FORMS {
            let got = play_script(&device, &queue, &mut Blur::with_options(&device, format, options), format);
            let (differing, largest_step) = script_difference(&got, &model);
            assert_eq!(differing, 0, "form '{name}' on {format:?}: largest step {largest_step} from the model");
        }
    }
    // The model sees a radius error: one texel wider is another image.
    let (size, format) = (SIZES[0], FORMATS[0]);
    let image = test_image(size, 7);
    let (wider, _) = difference(&blurred_model(&image, size, 5), &blurred_model(&image, size, 6));
    assert!(wider > 0, "model blind to the radius");
    let sabotages = [
        ("compute average truncated instead of rounded", "+ rounding)", ")"),
        ("compute window one texel short", "stored(along - r, across", "stored(along - r + 1, across"),
        ("compute edges not repeated", "vec2<i32>(0), last)", "vec2<i32>(0), last - 1)"),
    ];
    let source = include_str!("blur_compute.wgsl");
    for (name, from, to) in sabotages {
        assert!(source.contains(from), "sabotage '{name}': `{from}` not found in the shader");
        let mut blur = Blur::with_compute_source(&device, format, COMPUTE_FORMS[0].1, &source.replace(from, to));
        let (differing, _) = script_difference(&play_script(&device, &queue, &mut blur, format), &model);
        eprintln!("sabotage '{name}': {differing} bytes differ");
        assert!(differing > 0, "sabotage '{name}' not seen");
    }
}

/// Writes of the configurations per frame, for the radii 5, 5, 5, 8, 8 then a smaller frame.
fn config_writes_per_frame(device: &wgpu::Device, queue: &wgpu::Queue, options: Options) -> Vec<u64> {
    let format = FORMATS[0];
    let mut blur = Blur::with_options(device, format, options);
    let mut up = Uploader::new(device, true);
    let frames = [(SIZES[0], 5), (SIZES[0], 5), (SIZES[0], 5), (SIZES[0], 8), (SIZES[0], 8), ((200, 120), 8)];
    let mut writes = Vec::new();
    for (size, radius) in frames {
        let frame = TestFrame::new(device, size, format);
        up.take_stats();
        let mut enc = device.create_command_encoder(&Default::default());
        blur.run(device, queue, &mut up, &mut enc, &frame.texture, &frame.view, radius, None);
        up.submit(queue, enc.finish());
        writes.push(up.take_stats().writes);
    }
    writes
}

#[test]
fn configurations_cross_only_when_they_change() {
    let Some((device, queue)) = crate::render::upload::tests::gpu() else { return };
    let before = config_writes_per_frame(&device, &queue, Options::OLD);
    let after = config_writes_per_frame(&device, &queue, Options::NEW);
    eprintln!("configuration writes per frame: before {before:?}, after {after:?}");
    assert_eq!(before, [2; 6], "form before: both configurations on every frame");
    assert_eq!(after, [2, 0, 0, 2, 0, 2], "new form: both configurations per change of radius or size");

    // A configuration kept when the radius changes shows in the image.
    let (size, format) = (SIZES[0], FORMATS[0]);
    let image = test_image(size, 11);
    let frame = TestFrame::new(&device, size, format);
    let second_radius_image = |options, sabotage| {
        let mut blur = Blur::with_options(&device, format, options);
        blur.sabotage = sabotage;
        let mut up = Uploader::new(&device, true);
        blurred_on_gpu(&device, &queue, &mut up, &mut blur, &frame, &image, 5);
        blurred_on_gpu(&device, &queue, &mut up, &mut blur, &frame, &image, 8)
    };
    let want = second_radius_image(Options::OLD, None);
    let got = second_radius_image(Options::NEW, None);
    assert_eq!(difference(&got, &want), (0, 0), "radius changed between two frames");
    let sabotaged = second_radius_image(Options::NEW, Some(Sabotage::ConfigWrittenOnce));
    let (differing, _) = difference(&sabotaged, &want);
    eprintln!("sabotage 'configurations written once': {differing} bytes differ");
    assert!(differing > 0, "sabotage 'configurations written once' not seen");
}

#[test]
fn idle_time_runs_from_the_last_use() {
    let used = Instant::now();
    assert!(!idle_long_enough(None, used), "never used: nothing to free");
    assert!(!idle_long_enough(Some(used), used + IDLE_RELEASE_AFTER - Duration::from_millis(1)));
    assert!(idle_long_enough(Some(used), used + IDLE_RELEASE_AFTER));
    assert!(!idle_long_enough(Some(used + Duration::from_secs(60)), used), "use after `now`: not idle");
}

#[test]
fn idle_targets_are_freed_then_made_again() {
    let Some((device, queue)) = crate::render::upload::tests::gpu() else { return };
    let (size, format) = (SIZES[1], FORMATS[0]);
    let image = test_image(size, 23);
    let frame = TestFrame::new(&device, size, format);
    let blurred_before = {
        let mut blur = Blur::with_options(&device, format, Options::OLD);
        blurred_on_gpu(&device, &queue, &mut Uploader::new(&device, true), &mut blur, &frame, &image, 5)
    };
    let model = blurred_model(&image, size, 5);
    for (name, options) in FRAGMENT_FORMS.into_iter().chain(COMPUTE_FORMS) {
        let want = if options.compute_passes { &model } else { &blurred_before };
        let mut blur = Blur::with_options(&device, format, options);
        let mut up = Uploader::new(&device, true);
        let first = blurred_on_gpu(&device, &queue, &mut up, &mut blur, &frame, &image, 5);
        let textures = if options.compute_passes { 3 } else { 2 };
        assert_eq!(blur.work_target_bytes(), textures * texture_bytes(size), "form '{name}': targets in use");
        let used = blur.targets_last_used.expect("blurred frame recorded");
        blur.release_idle_targets(used + IDLE_RELEASE_AFTER - Duration::from_millis(1));
        assert!(blur.work_target_bytes() > 0, "form '{name}': targets freed too early");
        blur.release_idle_targets(used + IDLE_RELEASE_AFTER);
        let freed = blur.work_target_bytes() == 0;
        assert_eq!(freed, options.release_idle_targets, "form '{name}': targets freed once idle");
        let again = blurred_on_gpu(&device, &queue, &mut up, &mut blur, &frame, &image, 5);
        assert_eq!(difference(&first, want), (0, 0), "form '{name}': first blur");
        assert_eq!(difference(&again, want), (0, 0), "form '{name}': blur after the targets were made again");
    }
}

#[test]
fn compute_workgroups_cover_every_texel() {
    for (along, lines) in [(1, 1), (64, 16), (65, 17), (256, 16), (257, 900), (1600, 900), (3840, 2160)] {
        let [runs, line_groups] = workgroups(along, lines);
        let (covered_along, covered_lines) = (runs * WORKGROUP_RUNS * RUN_TEXELS, line_groups * WORKGROUP_LINES);
        assert!(covered_along >= along && covered_lines >= lines, "{along} × {lines} not covered");
        let one_group_less = ((runs - 1) * WORKGROUP_RUNS * RUN_TEXELS, (line_groups - 1) * WORKGROUP_LINES);
        assert!(one_group_less.0 < along && one_group_less.1 < lines, "{along} × {lines}: a workgroup too many");
    }
    let shader = include_str!("blur_compute.wgsl");
    assert!(shader.contains(&format!("const RUN_TEXELS: i32 = {RUN_TEXELS};")), "run length of the shader");
    assert!(
        shader.contains(&format!("@workgroup_size({WORKGROUP_RUNS}, {WORKGROUP_LINES})")),
        "workgroup size of the shader"
    );
}

/// Duration (ms) of `frames` blurs of `frame` drawn back to back, from the submission to the end
/// of the GPU.
fn time_blurs(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    up: &mut Uploader,
    blur: &mut Blur,
    frame: &TestFrame,
    radius: i32,
    frames: usize,
) -> f64 {
    let start = Instant::now();
    for _ in 0..frames {
        let mut enc = device.create_command_encoder(&Default::default());
        blur.run(device, queue, up, &mut enc, &frame.texture, &frame.view, radius, None);
        up.submit(queue, enc.finish());
    }
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
    start.elapsed().as_secs_f64() * 1e3
}

/// GPU and CPU micro-bench of the forms: one blur per frame at the menu's default radius (5) and
/// at its largest (10), 1600 × 900 and 3840 × 2160, 40 frames per round, best of 7 alternating
/// rounds, then the CPU time of encoding a blurred frame and the bytes of the work targets:
/// `cargo test --release -p rc-app --bin rustcraft blur_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn blur_cost() {
    let Some((device, queue)) = crate::render::upload::tests::gpu() else { return };
    let adapter = device.adapter_info();
    eprintln!("GPU: {} ({:?})", adapter.name, adapter.backend);
    let format = FORMATS[0];
    let forms = [FRAGMENT_FORMS[0], FRAGMENT_FORMS[3], COMPUTE_FORMS[0], COMPUTE_FORMS[1]];
    for size in [(1600, 900), (3840, 2160)] {
        let frame = TestFrame::new(&device, size, format);
        frame.fill(&queue, &test_image(size, 3));
        for radius in [5, 10] {
            let mut runs: Vec<(Blur, Uploader)> = forms
                .iter()
                .map(|(_, o)| (Blur::with_options(&device, format, *o), Uploader::new(&device, true)))
                .collect();
            for (blur, up) in &mut runs {
                time_blurs(&device, &queue, up, blur, &frame, radius, 5);
            }
            let mut best = [f64::MAX; 4];
            for _ in 0..7 {
                for (i, (blur, up)) in runs.iter_mut().enumerate() {
                    best[i] = best[i].min(time_blurs(&device, &queue, up, blur, &frame, radius, 40) / 40.0);
                }
            }
            for (((name, _), ms), (blur, _)) in forms.iter().zip(best).zip(&runs) {
                eprintln!(
                    "{}x{} radius {radius}, {name}: {ms:.3} ms per blur ({:+.1}%), work targets {:.1} MB (loaded machine)",
                    size.0,
                    size.1,
                    (ms / best[0] - 1.0) * 100.0,
                    blur.work_target_bytes() as f64 / 1e6
                );
            }
        }
    }
    // CPU time of encoding one blurred frame (configurations, copy and six passes), nothing
    // submitted in between: 2,000 frames per round, best of 7 alternating rounds.
    let frame = TestFrame::new(&device, (1600, 900), format);
    let mut runs: Vec<(Blur, Uploader)> =
        forms.iter().map(|(_, o)| (Blur::with_options(&device, format, *o), Uploader::new(&device, true))).collect();
    let mut best = [f64::MAX; 4];
    for _ in 0..7 {
        for (i, (blur, up)) in runs.iter_mut().enumerate() {
            let mut encode = Duration::ZERO;
            for _ in 0..2000 {
                let mut enc = device.create_command_encoder(&Default::default());
                let start = Instant::now();
                blur.run(&device, &queue, up, &mut enc, &frame.texture, &frame.view, 5, None);
                encode += start.elapsed();
                up.submit(&queue, enc.finish());
            }
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
            best[i] = best[i].min(encode.as_secs_f64() * 1e6 / 2000.0);
        }
    }
    for ((name, _), micros) in forms.iter().zip(best) {
        eprintln!("encoding, {name}: {micros:.2} µs per blurred frame ({:+.1}%)", (micros / best[0] - 1.0) * 100.0);
    }
    // What a menu opened after a few seconds without one pays once: encoding the blurred frame
    // that makes the freed targets again (50 times, the GPU idle before each).
    for size in [(1600, 900), (3840, 2160)] {
        let frame = TestFrame::new(&device, size, format);
        let mut blur = Blur::with_options(&device, format, Options::NEW);
        let mut up = Uploader::new(&device, true);
        let mut micros: Vec<f64> = Vec::new();
        for _ in 0..50 {
            let mut enc = device.create_command_encoder(&Default::default());
            let start = Instant::now();
            blur.run(&device, &queue, &mut up, &mut enc, &frame.texture, &frame.view, 5, None);
            micros.push(start.elapsed().as_secs_f64() * 1e6);
            up.submit(&queue, enc.finish());
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
            let used = blur.targets_last_used.expect("blurred frame recorded");
            blur.release_idle_targets(used + IDLE_RELEASE_AFTER);
            assert_eq!(blur.work_target_bytes(), 0, "targets freed between two measures");
        }
        micros.sort_by(f64::total_cmp);
        eprintln!(
            "{}x{}: first blurred frame after the targets were freed: median {:.0} µs, slowest {:.0} µs",
            size.0, size.1, micros[25], micros[49]
        );
    }
}
