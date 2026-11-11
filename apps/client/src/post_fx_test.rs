//! Proofs of the image effects. On the CPU: the passes of every form of the spider build the same
//! tree of images, whatever the work targets and the order they use. On a real GPU: a script of
//! frames (each effect, several in one frame, the menu blur among them, two frame sizes, a list
//! longer than the pass budget) played by the form before, each lead alone and the defaults,
//! gives the same bytes. The creeper in one pass is measured against its two passes over the
//! 16,777,216 colours: it may differ by one of the 16 levels on a small share of them (a GPU
//! does not have to round the 8-bit write between the two passes to the nearest).
//! Sabotages, each seen: configurations kept across a frame size, bindings kept by their source
//! alone, a work target written while its image is still read, the one-pass creeper without its
//! 8-bit rounding.
//! Without a GPU the GPU tests skip themselves; under `RUSTCRAFT_OPT_REQUIRE=1` it is a failure.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use super::*;
use crate::blur::tests::{TestFrame, difference, test_image};
use crate::blur::{Blur, IDLE_RELEASE_AFTER, texture_bytes};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
/// A small frame and one with odd sides (mosaic blocks cut by the edges).
const SIZES: [(u32, u32); 2] = [(320, 240), (853, 479)];
const BLUR_RADIUS: i32 = 5;

/// Forms that draw the bytes of before: each lead alone, then the switches' defaults.
const FORMS: [(&str, Options); 6] = [
    ("before", Options::OLD),
    ("configurations per frame size", Options { skip_unchanged_config: true, ..Options::OLD }),
    ("bindings kept", Options { bind_group_cache: true, ..Options::OLD }),
    ("idle targets freed", Options { release_idle_targets: true, ..Options::OLD }),
    ("targets reused", Options { target_aliasing: true, ..Options::OLD }),
    ("defaults", Options::NEW),
];
/// Forms with the creeper in one pass: the lead alone, then with the defaults.
const ONE_PASS_CREEPER_FORMS: [(&str, Options); 2] = [
    ("creeper in one pass", Options { creeper_pass_fusion: true, ..Options::OLD }),
    ("defaults and creeper in one pass", Options { creeper_pass_fusion: true, ..Options::NEW }),
];

fn base_config(size: (u32, u32)) -> PassConfig {
    PassConfig { size: [size.0 as f32, size.1 as f32], modulate: [1.0; 4], ..Default::default() }
}

/// What a chain writes into the frame, as the tree of the passes that produced it: two chains
/// with the same tree draw the same frame, whatever the work targets and the order they use.
fn frame_expression(passes: &[Pass]) -> String {
    let mut held: Vec<String> = (0..MAX_WORK_TARGETS).map(|target| format!("unwritten target {target}")).collect();
    held[FRAME_COPY] = "frame".to_owned();
    let mut frame = None;
    for pass in passes {
        let reads_its_output = [pass.source, pass.back].map(Output::Target).contains(&pass.output);
        assert!(!reads_its_output, "a pass reads the target it writes");
        // Only the eyes of the spider read their second input.
        let inputs = match pass.program {
            Program::Spider => format!("{}, {}", held[pass.source], held[pass.back]),
            _ => held[pass.source].clone(),
        };
        let image = format!("{:?}{:?}({inputs})", pass.program, bytemuck::bytes_of(&pass.config));
        match pass.output {
            Output::Target(target) => held[target] = image,
            Output::Frame => frame = Some(image),
        }
    }
    frame.expect("a chain ends in the frame")
}

#[test]
fn reused_targets_build_the_same_spider_image() {
    let base = base_config((853, 479));
    let (one_per_image, reused) = (spider_chain(base, false), spider_chain(base, true));
    let want = frame_expression(&one_per_image);
    assert!(!want.contains("unwritten"), "the chain of before reads a target nobody wrote");
    assert_eq!(frame_expression(&reused), want, "spider with reused targets");
    assert_eq!(Chain::new(one_per_image, 10, 0).work_targets, 5);
    assert_eq!(Chain::new(reused, 10, 0).work_targets, 3);

    // The small blur finished in the target of the first eye, still to be read: another image.
    let mut broken = spider_chain(base, true);
    broken[4].output = Output::Target(1);
    assert_ne!(frame_expression(&broken), want, "sabotage 'target written while still read' not seen");
}

#[test]
fn plan_gives_every_pass_its_slot() {
    let size = (640, 360);
    for (name, options) in FORMS.into_iter().chain(ONE_PASS_CREEPER_FORMS) {
        let plan = Plan::new(size, options);
        let block = plan.config_block();
        let mut slots = BTreeSet::new();
        for chain in &plan.chains {
            for (index, pass) in chain.passes.iter().enumerate() {
                let slot = chain.first_slot + index as u64;
                assert!(slots.insert(slot), "form '{name}': slot {slot} given twice");
                let at = (slot * SLOT_BYTES) as usize;
                let written = &block[at..at + PASS_CONFIG_BYTES as usize];
                assert_eq!(written, bytemuck::bytes_of(&pass.config), "form '{name}': slot {slot}");
            }
            assert!(chain.passes.len() as u64 <= chain.counted_passes, "form '{name}': passes over the budget");
        }
        assert!(slots.len() as u64 <= PASSES_PER_FRAME, "form '{name}': more slots than the buffer holds");
        assert_eq!(block.len() as u64, slots.len() as u64 * SLOT_BYTES, "form '{name}': block size");
        let [invert, creeper, spider] = &plan.chains;
        assert_eq!([invert.counted_passes, creeper.counted_passes, spider.counted_passes], [1, 2, 10]);
        let targets = [invert.work_targets, creeper.work_targets, spider.work_targets];
        let creeper_targets = if options.creeper_pass_fusion { 1 } else { 2 };
        let spider_targets = if options.target_aliasing { 3 } else { 5 };
        assert_eq!(targets, [1, creeper_targets, spider_targets], "form '{name}': work targets per effect");
    }
}

/// The effects, the blur they call and what they draw with, in one form.
struct Rig<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    up: Uploader,
    effects: PostFx,
    blur: Blur,
}

impl<'a> Rig<'a> {
    fn new(device: &'a wgpu::Device, queue: &'a wgpu::Queue, effects: PostFx) -> Rig<'a> {
        let blur = Blur::with_options(device, FORMAT, crate::blur::Options::OLD);
        Rig { device, queue, up: Uploader::new(device, true), effects, blur }
    }

    fn with_options(device: &'a wgpu::Device, queue: &'a wgpu::Queue, options: Options) -> Rig<'a> {
        Rig::new(device, queue, PostFx::with_options(device, FORMAT, options))
    }

    /// Encodes and submits `list` over `frame` as it is.
    fn draw(&mut self, frame: &TestFrame, list: &[Effect]) {
        let mut enc = self.device.create_command_encoder(&Default::default());
        let (device, queue) = (self.device, self.queue);
        let (texture, view) = (&frame.texture, &frame.view);
        self.effects.run(device, queue, &mut self.up, &mut enc, texture, view, list, &mut self.blur, BLUR_RADIUS, None);
        self.up.submit(queue, enc.finish());
    }

    /// `image` after `list`, read back.
    fn apply(&mut self, frame: &TestFrame, image: &[u8], list: &[Effect]) -> Vec<u8> {
        frame.fill(self.queue, image);
        self.draw(frame, list);
        frame.read(self.device, self.queue)
    }
}

use Effect::{Blur as MenuBlur, Creeper, Invert, Spider};

/// Frames of the script: index of the frame size, effects of the frame.
const SCRIPT: [(usize, &[Effect]); 13] = [
    (0, &[Invert]),
    (0, &[Creeper]),
    (0, &[Spider]),
    (0, &[Spider]),
    (0, &[Creeper, Spider, Invert]),
    (1, &[Spider]),
    (1, &[Creeper]),
    (1, &[Invert, MenuBlur]),
    (0, &[MenuBlur, Creeper, Creeper]),
    (0, &[Spider, Invert]),
    // 32 passes, the budget of a frame, then an effect that no longer fits (twice).
    (0, &[Spider, Spider, Spider, Creeper, Invert]),
    (0, &[Spider, Spider, Spider, Invert, Invert, Creeper]),
    (1, &[Invert]),
];

/// Images of the script, one per frame.
fn play_script(rig: &mut Rig<'_>) -> Vec<Vec<u8>> {
    let frames = SIZES.map(|size| TestFrame::new(rig.device, size, FORMAT));
    let step = |(n, (size_index, list)): (usize, &(usize, &[Effect]))| {
        let image = test_image(SIZES[*size_index], 0x9d2c_5680 + n as u64);
        rig.apply(&frames[*size_index], &image, list)
    };
    SCRIPT.iter().enumerate().map(step).collect()
}

/// Bytes that differ between two plays of the script.
fn differing_bytes(got: &[Vec<u8>], want: &[Vec<u8>]) -> usize {
    got.iter().zip(want).map(|(g, w)| difference(g, w).0).sum()
}

/// The images of the form before are those of the effects: opaque, changed, the inverted one
/// within one step of `0.8 − 0.6 c`, and the list over the budget stops where it should.
fn check_script_of_before(images: &[Vec<u8>]) {
    for (n, ((size_index, list), image)) in SCRIPT.iter().zip(images).enumerate() {
        let source = test_image(SIZES[*size_index], 0x9d2c_5680 + n as u64);
        let changed = difference(image, &source).0;
        assert!(changed > image.len() / 2, "frame {n} ({list:?}): {changed} bytes changed");
        let last_effect_opaque = list.last() != Some(&MenuBlur);
        assert!(!last_effect_opaque || image.iter().skip(3).step_by(4).all(|a| *a == 255), "frame {n}: alpha of 1");
    }
    let inverted = |c: u8| ((0.8 - 0.6 * f64::from(c) / 255.0) * 255.0).round() as u8;
    let source = test_image(SIZES[0], 0x9d2c_5680);
    let model: Vec<u8> = source.iter().enumerate().map(|(i, c)| if i % 4 == 3 { 255 } else { inverted(*c) }).collect();
    let (_, largest_step) = difference(&images[0], &model);
    assert!(largest_step <= 1, "inverted image {largest_step} steps away from 0.8 − 0.6 c");
}

#[test]
fn every_form_draws_the_images_of_before_on_gpu() {
    let Some((device, queue)) = crate::render::upload::tests::gpu() else { return };
    let want = play_script(&mut Rig::with_options(&device, &queue, Options::OLD));
    check_script_of_before(&want);
    // The last effect of frame 11 no longer fits in the budget: without it, the same image.
    let mut rig = Rig::with_options(&device, &queue, Options::OLD);
    let frame = TestFrame::new(&device, SIZES[0], FORMAT);
    let image = test_image(SIZES[0], 0x9d2c_5680 + 11);
    let cut = rig.apply(&frame, &image, &[Spider, Spider, Spider, Invert, Invert]);
    assert_eq!(difference(&cut, &want[11]), (0, 0), "effect beyond the pass budget applied");

    for (name, options) in &FORMS[1..] {
        let got = play_script(&mut Rig::with_options(&device, &queue, *options));
        for (n, (got, want)) in got.iter().zip(&want).enumerate() {
            let (differing, largest_step) = difference(got, want);
            assert_eq!(differing, 0, "form '{name}', frame {n} ({:?}): largest step {largest_step}", SCRIPT[n].1);
        }
    }

    let sabotaged = |options: Options, sabotage: Sabotage| {
        let mut rig = Rig::with_options(&device, &queue, options);
        rig.effects.sabotage = Some(sabotage);
        differing_bytes(&play_script(&mut rig), &want)
    };
    let sabotages = [
        ("configurations kept across a frame size", sabotaged(Options::NEW, Sabotage::ConfigsWrittenOnce)),
        ("bindings kept by their source alone", sabotaged(Options::NEW, Sabotage::BindingsKeptBySourceOnly)),
        ("target written while its image is still read", {
            let mut rig = Rig::with_options(&device, &queue, Options::NEW);
            let spider_image = test_image(SIZES[0], 0x9d2c_5680 + 2);
            rig.apply(&frame, &spider_image, &[Spider]);
            let plan = rig.effects.plan.as_mut().expect("plan of the first frame");
            plan.chains[2].passes[4].output = Output::Target(1);
            difference(&rig.apply(&frame, &spider_image, &[Spider]), &want[2]).0
        }),
    ];
    for (name, differing) in sabotages {
        eprintln!("sabotage '{name}': {differing} bytes differ");
        assert!(differing > 0, "sabotage '{name}' not seen");
    }
}

/// Largest step between two neighbouring levels of the creeper's mosaic, as drawn: one sixteenth
/// of the green, saturation 1.5 (19.2 of 255).
const ONE_MOSAIC_LEVEL: u8 = 20;
/// Share of the colours the one-pass creeper may draw one level away from the two passes: those
/// whose green lands close enough to a half between two 8-bit values for the GPU's own rounding
/// of the write between the two passes to go the other way (measured: 0.21 %).
const COLOURS_ALLOWED_ONE_LEVEL_AWAY: f64 = 0.005;

/// Frame of 4096 × 4096 whose 4 × 4 blocks hold, one colour each, the 2^20 colours of `round`
/// (the 16 rounds cover the 2^24 colours).
fn colour_blocks(round: u32) -> Vec<u8> {
    const SIDE: usize = 4096;
    let mut image = vec![255; SIDE * SIDE * 4];
    for (texel_index, texel) in image.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (texel_index % SIDE, texel_index / SIDE);
        let colour = (round << 20) | ((y / 4) << 10 | (x / 4)) as u32;
        texel[..3].copy_from_slice(&colour.to_le_bytes()[..3]);
    }
    image
}

/// Creeper drawn by `one_pass` against the two passes over the colours of `rounds`: share of the
/// colours drawn differently, largest step between the two images.
fn one_pass_creeper_distance(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    one_pass: PostFx,
    rounds: impl IntoIterator<Item = u32>,
) -> (f64, u8) {
    // Reused targets on both sides: two frame-sized textures instead of five for the two passes.
    let two_passes = Options { target_aliasing: true, ..Options::OLD };
    let mut before = Rig::with_options(device, queue, two_passes);
    let mut after = Rig::new(device, queue, one_pass);
    let frame = TestFrame::new(device, (4096, 4096), FORMAT);
    let (mut colours, mut colours_differing, mut largest_step) = (0, 0, 0);
    let mut green_levels_seen: BTreeSet<u8> = BTreeSet::new();
    for round in rounds {
        let image = colour_blocks(round);
        let want = before.apply(&frame, &image, &[Creeper]);
        let got = after.apply(&frame, &image, &[Creeper]);
        let (bytes_differing, step) = difference(&got, &want);
        // A colour fills the 16 texels of a block, and only their green byte moves.
        colours += image.len() / (4 * 16);
        colours_differing += bytes_differing / 16;
        largest_step = largest_step.max(step);
        // Bgra8: the green is the second byte of a texel.
        green_levels_seen.extend(want.iter().skip(1).step_by(4));
    }
    // The two passes did draw: the levels of the mosaic came out (the brightest ones saturate).
    assert!(green_levels_seen.len() >= 12, "green levels drawn by the two passes: {green_levels_seen:?}");
    (colours_differing as f64 / colours as f64, largest_step)
}

#[test]
fn creeper_in_one_pass_stays_within_one_level_for_every_colour_on_gpu() {
    let Some((device, queue)) = crate::render::upload::tests::gpu() else { return };
    let options = Options { target_aliasing: true, ..ONE_PASS_CREEPER_FORMS[0].1 };
    let one_pass = PostFx::with_options(&device, FORMAT, options);
    let (share, largest_step) = one_pass_creeper_distance(&device, &queue, one_pass, 0..16);
    eprintln!("creeper in one pass: {:.3} % of the colours differ, largest step {largest_step}", share * 100.0);
    assert!(largest_step <= ONE_MOSAIC_LEVEL, "one-pass creeper {largest_step} steps away: more than a level");
    assert!(share <= COLOURS_ALLOWED_ONE_LEVEL_AWAY, "one-pass creeper: {:.3} % of the colours differ", share * 100.0);

    // Without its own 8-bit rounding, the one pass leaves that allowance.
    let from = "sixteen_levels(as_stored_in_8_bits(green_vision(c)))";
    let source = include_str!("post_fx.wgsl");
    assert!(source.contains(from), "sabotage: `{from}` not found in the shader");
    let unrounded = source.replace(from, "sixteen_levels(clamp(green_vision(c), vec3<f32>(0.0), vec3<f32>(1.0)))");
    let sabotaged = PostFx::with_shader_source(&device, FORMAT, options, &unrounded);
    let (share, largest_step) = one_pass_creeper_distance(&device, &queue, sabotaged, [3, 12]);
    eprintln!(
        "sabotage 'no 8-bit rounding': {:.3} % of the colours differ, largest step {largest_step}",
        share * 100.0
    );
    assert!(share > COLOURS_ALLOWED_ONE_LEVEL_AWAY, "sabotage 'no 8-bit rounding' not seen");
}

/// The creeper in one pass only touches the frames with a creeper, and those by two levels at
/// most (a creeper of a creeper doubles a level).
#[test]
fn creeper_in_one_pass_leaves_the_other_effects_alone_on_gpu() {
    let Some((device, queue)) = crate::render::upload::tests::gpu() else { return };
    let want = play_script(&mut Rig::with_options(&device, &queue, Options::OLD));
    for (name, options) in ONE_PASS_CREEPER_FORMS {
        let got = play_script(&mut Rig::with_options(&device, &queue, options));
        for (n, (got, want)) in got.iter().zip(&want).enumerate() {
            let (_, largest_step) = difference(got, want);
            let allowed = if SCRIPT[n].1.contains(&Creeper) { 2 * ONE_MOSAIC_LEVEL } else { 0 };
            assert!(
                largest_step <= allowed,
                "form '{name}', frame {n} ({:?}): largest step {largest_step}",
                SCRIPT[n].1
            );
        }
    }
}

#[test]
fn spider_frames_write_and_bind_once() {
    let Some((device, queue)) = crate::render::upload::tests::gpu() else { return };
    let mut rig = Rig::with_options(&device, &queue, Options::NEW);
    let frames = SIZES.map(|size| TestFrame::new(&device, size, FORMAT));
    let mut writes = Vec::new();
    let mut kept_bindings = Vec::new();
    for size_index in [0, 0, 0, 1, 1] {
        rig.up.take_stats();
        rig.draw(&frames[size_index], &[Spider]);
        writes.push(rig.up.take_stats().writes);
        let targets = rig.effects.targets.as_ref().expect("targets of the frame");
        kept_bindings.push(targets.bind_groups.iter().flatten().flatten().count());
    }
    eprintln!("spider, per frame: configuration writes {writes:?}, bindings kept {kept_bindings:?}");
    assert_eq!(writes, [1, 0, 0, 1, 0], "one block of configurations per frame size");
    // Ten passes read five different pairs of targets.
    assert_eq!(kept_bindings, [5; 5], "bindings made once per pair of targets read");
}

#[test]
fn idle_targets_are_freed_then_made_again() {
    let Some((device, queue)) = crate::render::upload::tests::gpu() else { return };
    let size = SIZES[1];
    let image = test_image(size, 41);
    let frame = TestFrame::new(&device, size, FORMAT);
    let want = Rig::with_options(&device, &queue, Options::OLD).apply(&frame, &image, &[Spider]);
    for (name, options) in FORMS {
        let mut rig = Rig::with_options(&device, &queue, options);
        let first = rig.apply(&frame, &image, &[Spider]);
        let textures = if options.target_aliasing { 3 } else { 5 };
        assert_eq!(rig.effects.work_target_bytes(), textures * texture_bytes(size), "form '{name}': targets in use");
        let used = rig.effects.targets_last_used.expect("frame with an effect recorded");
        rig.effects.release_idle_targets(used + IDLE_RELEASE_AFTER - Duration::from_millis(1));
        assert!(rig.effects.work_target_bytes() > 0, "form '{name}': targets freed too early");
        rig.effects.release_idle_targets(used + IDLE_RELEASE_AFTER);
        let freed = rig.effects.work_target_bytes() == 0;
        assert_eq!(freed, options.release_idle_targets, "form '{name}': targets freed once idle");
        let again = rig.apply(&frame, &image, &[Spider]);
        assert_eq!(difference(&first, &want), (0, 0), "form '{name}': first frame");
        assert_eq!(difference(&again, &want), (0, 0), "form '{name}': frame after the targets were made again");
    }
}

/// CPU micro-bench and frame time of the forms, one effect per frame at 1600 × 900: time of 40
/// frames drawn back to back, from the submission to the end of the GPU (best of 7 alternating
/// rounds), CPU time of encoding a frame (2,000 frames per round, best of 7) and bytes of the
/// work targets:
/// `cargo test --release -p rc-app --bin rustcraft image_effect_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn image_effect_cost() {
    let Some((device, queue)) = crate::render::upload::tests::gpu() else { return };
    let size = (1600, 900);
    let frame = TestFrame::new(&device, size, FORMAT);
    frame.fill(&queue, &test_image(size, 3));
    let wait = || device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
    let forms: Vec<(&str, Options)> = FORMS.into_iter().chain(ONE_PASS_CREEPER_FORMS).collect();
    for effect in [Invert, Creeper, Spider] {
        let mut rigs: Vec<Rig<'_>> = forms.iter().map(|(_, o)| Rig::with_options(&device, &queue, *o)).collect();
        let mut gpu_ms = vec![f64::MAX; forms.len()];
        let mut encode_micros = vec![f64::MAX; forms.len()];
        for round in 0..8 {
            for (i, rig) in rigs.iter_mut().enumerate() {
                let start = Instant::now();
                for _ in 0..40 {
                    rig.draw(&frame, &[effect]);
                }
                wait();
                // First round: targets, bindings and pipelines warm up.
                if round > 0 {
                    gpu_ms[i] = gpu_ms[i].min(start.elapsed().as_secs_f64() * 1e3 / 40.0);
                }
                let mut encode = Duration::ZERO;
                for _ in 0..2000 {
                    let mut enc = device.create_command_encoder(&Default::default());
                    let start = Instant::now();
                    let (texture, view) = (&frame.texture, &frame.view);
                    rig.effects.run(
                        &device,
                        &queue,
                        &mut rig.up,
                        &mut enc,
                        texture,
                        view,
                        &[effect],
                        &mut rig.blur,
                        5,
                        None,
                    );
                    encode += start.elapsed();
                    rig.up.submit(&queue, enc.finish());
                }
                wait();
                encode_micros[i] = encode_micros[i].min(encode.as_secs_f64() * 1e6 / 2000.0);
            }
        }
        for (i, (name, _)) in forms.iter().enumerate() {
            eprintln!(
                "{effect:?}, {name}: {:.3} ms per frame to the end of the GPU ({:+.1}%), encoding {:.2} µs per \
                 frame ({:+.1}%), work targets {:.1} MB (loaded machine)",
                gpu_ms[i],
                (gpu_ms[i] / gpu_ms[0] - 1.0) * 100.0,
                encode_micros[i],
                (encode_micros[i] / encode_micros[0] - 1.0) * 100.0,
                rigs[i].effects.work_target_bytes() as f64 / 1e6
            );
        }
    }
    // What an effect coming back after a few seconds without one pays once: encoding the spider
    // frame that makes the freed targets and their bindings again (50 times, the GPU idle before
    // each).
    let mut rig = Rig::with_options(&device, &queue, Options::NEW);
    let mut micros: Vec<f64> = Vec::new();
    for _ in 0..50 {
        let mut enc = device.create_command_encoder(&Default::default());
        let start = Instant::now();
        let (texture, view) = (&frame.texture, &frame.view);
        rig.effects.run(&device, &queue, &mut rig.up, &mut enc, texture, view, &[Spider], &mut rig.blur, 5, None);
        micros.push(start.elapsed().as_secs_f64() * 1e6);
        rig.up.submit(&queue, enc.finish());
        wait();
        let used = rig.effects.targets_last_used.expect("frame with an effect recorded");
        rig.effects.release_idle_targets(used + IDLE_RELEASE_AFTER);
        assert_eq!(rig.effects.work_target_bytes(), 0, "targets freed between two measures");
    }
    micros.sort_by(f64::total_cmp);
    eprintln!(
        "first spider frame after the targets were freed: median {:.0} µs, slowest {:.0} µs",
        micros[25], micros[49]
    );
}
