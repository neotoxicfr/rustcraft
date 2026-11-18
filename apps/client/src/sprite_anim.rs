//! Animation of a texture copied into a separate sheet (interface item sprites,
//! entity textures): same reading as the block atlas (`.png.mcmeta`: steps, durations in
//! ticks, cross-fade), one step per advance, and the current frame rewritten into the sheet
//! only when it changes (on every advance for a blended animation).
//!
//! Two ways to read the current frame: [`SpriteAnim::current`] gives an owned image (a new
//! allocation per call), [`SpriteAnim::current_pixels`] borrows the strip or blends into a buffer
//! the caller keeps (no allocation once the buffer has grown).

use std::sync::atomic::{AtomicU32, Ordering};

use rc_assets::texture::{Animation, Image};

/// Advances of the texture animation since launch (common clock of the game's atlases).
static TICKS: AtomicU32 = AtomicU32::new(0);

/// One advance of the common clock (called together with the block animation).
pub fn advance_clock() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Frame of an animated interface sprite with a single duration (`frametime`), strip of `frames` frames,
/// read on the common clock: index in the strip.
pub fn gui_frame(frames: u32, frametime: u32) -> u32 {
    (TICKS.load(Ordering::Relaxed) / frametime.max(1)) % frames.max(1)
}

pub struct SpriteAnim {
    /// Frames of the strip, each with its available sizes (the first = the largest).
    frames: Vec<Vec<Image>>,
    /// (frame, duration in ticks) in playback order.
    steps: Vec<(u32, u32)>,
    interpolate: bool,
    step: usize,
    /// Ticks already played in the current step.
    ticks_into_step: u32,
}

/// Pixels of the frame to copy into a sheet, borrowed from the strip or from the caller's blend
/// buffer; their width is the one that was asked for.
pub struct FramePixels<'a> {
    pub height: u32,
    pub rgba: &'a [u8],
}

impl SpriteAnim {
    /// `None` if the animation does not have at least two valid steps.
    pub fn new(frames: Vec<Vec<Image>>, animation: &Animation) -> Option<Self> {
        let steps: Vec<(u32, u32)> = animation
            .frames
            .iter()
            .copied()
            .filter(|&(f, t)| t > 0 && frames.get(f as usize).is_some_and(|l| !l.is_empty()))
            .collect();
        (steps.len() > 1).then_some(SpriteAnim {
            frames,
            steps,
            interpolate: animation.interpolate,
            step: 0,
            ticks_into_step: 0,
        })
    }

    /// True if the animation fades each frame into the next one (rewritten on every tick).
    #[cfg(test)]
    pub fn is_blended(&self) -> bool {
        self.interpolate
    }

    /// One advance of a tick; true if the displayed frame has changed.
    pub fn tick(&mut self) -> bool {
        self.ticks_into_step += 1;
        let old = self.steps[self.step].0;
        if self.ticks_into_step >= self.steps[self.step].1 {
            self.step = (self.step + 1) % self.steps.len();
            self.ticks_into_step = 0;
        }
        self.interpolate || self.steps[self.step].0 != old
    }

    fn frame_at_width(&self, frame: u32, width: u32) -> Option<&Image> {
        self.frames[frame as usize].iter().find(|image| image.width == width)
    }

    /// Frame of the current step at width `width` and, for a blended animation, the frame it fades
    /// into; `None` if this width is missing or the two frames differ in size.
    fn shown_frames(&self, width: u32) -> Option<(&Image, Option<&Image>)> {
        let current = self.frame_at_width(self.steps[self.step].0, width)?;
        if !self.interpolate {
            return Some((current, None));
        }
        let next_step = (self.step + 1) % self.steps.len();
        let next = self.frame_at_width(self.steps[next_step].0, width)?;
        (next.rgba.len() == current.rgba.len()).then_some((current, Some(next)))
    }

    /// Share of the next frame in the blend: progress in the step, truncated to the thousandth
    /// like the game.
    fn share_of_next(&self) -> f32 {
        let duration = self.steps[self.step].1;
        ((self.ticks_into_step as f32 / duration as f32) * 1000.0) as u32 as f32 / 1000.0
    }

    /// True if the current frame exists at `width` × `height` (what [`SpriteAnim::current`] and
    /// [`SpriteAnim::current_pixels`] would give), without blending anything.
    pub fn has_frame(&self, width: u32, height: u32) -> bool {
        self.shown_frames(width).is_some_and(|(current, _)| current.height == height)
    }

    /// Current frame at width `width` (one of the known sizes), blended towards the next one
    /// in proportion to the step for a blended animation; `None` if this width is missing.
    pub fn current(&self, width: u32) -> Option<Image> {
        let (current, next) = self.shown_frames(width)?;
        let Some(next) = next else { return Some(current.clone()) };
        let mut rgba = Vec::new();
        blend_into(&current.rgba, &next.rgba, self.share_of_next(), ChannelRounding::from_switches(), &mut rgba);
        Some(Image { width: current.width, height: current.height, rgba })
    }

    /// Same pixels as [`SpriteAnim::current`] without an allocation per call: a plain frame is
    /// borrowed from the strip, a blended one is written into `blend_buffer` (kept by the caller).
    pub fn current_pixels<'a>(&'a self, width: u32, blend_buffer: &'a mut Vec<u8>) -> Option<FramePixels<'a>> {
        let (current, next) = self.shown_frames(width)?;
        let rgba = match next {
            None => current.rgba.as_slice(),
            Some(next) => {
                let rounding = ChannelRounding::from_switches();
                blend_into(&current.rgba, &next.rgba, self.share_of_next(), rounding, blend_buffer);
                blend_buffer.as_slice()
            }
        };
        Some(FramePixels { height: current.height, rgba })
    }
}

/// How a blended channel becomes a byte; the three give the library's rounding (halves away from
/// zero) on every blend of two bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChannelRounding {
    /// The library rounding: a call per channel in the portable build (no SSE4.1), the oldest
    /// path (`RUSTCRAFT_OPT_ROUND_BY_TRUNCATION=0`).
    Library,
    /// Truncation behind a range check that falls back on the library: right for any float, but
    /// the check and the call behind it keep the loop scalar (`RUSTCRAFT_OPT_ROUND_BY_TRUNCATION`).
    CheckedTruncation,
    /// Truncation alone, a blend of two bytes never leaving [0, 255]: no check, but the
    /// conversion to an integer still keeps the loop scalar
    /// (`RUSTCRAFT_OPT_UNCHECKED_SPRITE_TRUNCATION=1`, off by default: slower than the shift).
    UncheckedTruncation,
    /// Shift by 2^23, where floats are whole numbers, then the ties fixed: additions, one
    /// comparison and the float's own bits, which the compiler turns into vector instructions
    /// (`RUSTCRAFT_OPT_VECTORIZED_SPRITE_BLEND`, the default).
    IntegerSpacingShift,
}

impl ChannelRounding {
    fn from_switches() -> Self {
        if rc_data::opt_switch!("RUSTCRAFT_OPT_VECTORIZED_SPRITE_BLEND") {
            ChannelRounding::IntegerSpacingShift
        } else if rc_data::opt_switch!("RUSTCRAFT_OPT_UNCHECKED_SPRITE_TRUNCATION", default_off) {
            ChannelRounding::UncheckedTruncation
        } else if rc_data::opt_switch!("RUSTCRAFT_OPT_ROUND_BY_TRUNCATION") {
            ChannelRounding::CheckedTruncation
        } else {
            ChannelRounding::Library
        }
    }
}

/// Replaces the contents of `blended` with `current` faded towards `next` (same length) by
/// `share_of_next`, channel by channel.
fn blend_into(current: &[u8], next: &[u8], share_of_next: f32, rounding: ChannelRounding, blended: &mut Vec<u8>) {
    let channels = current.iter().zip(next).map(|(&a, &b)| blend_channel(a, b, share_of_next));
    blended.clear();
    match rounding {
        ChannelRounding::Library => blended.extend(channels.map(|v| round_channel(v, false))),
        ChannelRounding::CheckedTruncation => blended.extend(channels.map(|v| round_channel(v, true))),
        ChannelRounding::UncheckedTruncation => blended.extend(channels.map(round_by_unchecked_truncation)),
        ChannelRounding::IntegerSpacingShift => blended.extend(channels.map(round_by_integer_spacing_shift)),
    }
}

/// Channel `a` faded towards `b`, before rounding: always within [0, 255] (between `a` and `b`).
fn blend_channel(a: u8, b: u8, share_of_next: f32) -> f32 {
    f32::from(a) + (f32::from(b) - f32::from(a)) * share_of_next
}

/// `v.round() as u8` of a blended channel. `by_truncation`: for a value of [0, 255.5[ the
/// truncation is one instruction and the fraction it leaves is exact, where the library
/// rounding is a call per channel (`RUSTCRAFT_OPT_ROUND_BY_TRUNCATION=0`: the library, the old path).
fn round_channel(v: f32, by_truncation: bool) -> u8 {
    if by_truncation && (0.0..255.5).contains(&v) {
        let whole = v as u32;
        (whole + u32::from(v - whole as f32 >= 0.5)) as u8
    } else {
        v.round() as u8
    }
}

/// `v.round() as u8` for `v` in [0, 255], the range of [`blend_channel`]: the truncation leaves
/// the exact fraction, and a fraction of one half or more goes up. No range check (the caller's
/// range is proven by the tests).
fn round_by_unchecked_truncation(v: f32) -> u8 {
    let whole = v as i32;
    (whole + i32::from(v - whole as f32 >= 0.5)) as u8
}

/// `v.round() as u8` for `v` in [0, 255], without a conversion to an integer (the saturating
/// conversion is what keeps the truncation loops scalar). From 2^23 to 2^24 the floats are the
/// whole numbers: adding 2^23 rounds `v` to the nearest one, a tie going to the even neighbour,
/// and leaves that whole number in the low bits of the sum. The library sends a tie upwards: a
/// tie that went down (`v` exactly one half above the result) gets the missing one.
fn round_by_integer_spacing_shift(v: f32) -> u8 {
    const WHOLE_FLOATS_START: f32 = 8_388_608.0;
    let shifted = v + WHOLE_FLOATS_START;
    let nearest_ties_to_even = shifted - WHOLE_FLOATS_START;
    let tie_went_down = v - nearest_ties_to_even == 0.5;
    (shifted.to_bits() as u8) + u8::from(tie_went_down)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every float (2^32): the channel rounded through the truncation is the library's.
    /// Sabotage: a sum with one half then a truncation (wrong at 0.49999997) is caught.
    #[test]
    fn channel_rounding_matches_library_on_every_float() {
        use rayon::prelude::*;
        let first_mismatch = |f: &(dyn Fn(f32) -> u8 + Sync)| {
            (0..=u32::MAX).into_par_iter().map(f32::from_bits).find_any(|&v| f(v) != v.round() as u8)
        };
        assert_eq!(first_mismatch(&|v| round_channel(v, true)), None);
        let half_sum = |v: f32| if (0.0..255.5).contains(&v) { (v + 0.5) as u8 } else { v.round() as u8 };
        assert!(first_mismatch(&half_sum).is_some(), "sabotage not detected");
    }

    /// Every blend the game can ask for (256 × 256 bytes, every thousandth from 0 to 1): the value
    /// stays in [0, 255], the range the two roundings without a check are written for, and both
    /// give the library's byte.
    #[test]
    fn every_blend_of_two_bytes_stays_in_range_and_rounds_like_the_library() {
        use rayon::prelude::*;
        let first_wrong = (0..=u8::MAX).into_par_iter().find_map_any(|a| {
            for b in 0..=u8::MAX {
                for thousandth in 0..=1000u32 {
                    let v = blend_channel(a, b, thousandth as f32 / 1000.0);
                    let library = v.round() as u8;
                    let same =
                        round_by_unchecked_truncation(v) == library && round_by_integer_spacing_shift(v) == library;
                    if !(0.0..=255.0).contains(&v) || !same {
                        return Some((a, b, thousandth, v));
                    }
                }
            }
            None
        });
        assert_eq!(first_wrong, None, "(current, next, thousandth, blended value)");
    }

    /// Every float of [0, 255] (about 1.1 billion): the two roundings without a range check are
    /// the library's. Sabotages: a sum with one half then a truncation (wrong at 0.49999997), and
    /// the shift without its fix of the ties (wrong at 0.5), are caught.
    #[test]
    fn unchecked_roundings_match_library_on_every_float_of_their_range() {
        use rayon::prelude::*;
        let first_mismatch = |f: &(dyn Fn(f32) -> u8 + Sync)| {
            (0..=255.0f32.to_bits()).into_par_iter().map(f32::from_bits).find_any(|&v| f(v) != v.round() as u8)
        };
        assert_eq!(first_mismatch(&round_by_unchecked_truncation), None);
        assert_eq!(first_mismatch(&round_by_integer_spacing_shift), None);
        assert!(first_mismatch(&|v| (v + 0.5) as i32 as u8).is_some(), "half sum sabotage not detected");
        let ties_to_even = |v: f32| (v + 8_388_608.0).to_bits() as u8;
        assert!(first_mismatch(&ties_to_even).is_some(), "ties sabotage not detected");
    }

    /// A blend in whole numbers (thousandths) is NOT the game's float blend: it gives another
    /// byte for a few hundred of the 65.6 million cases, which is why it is not used.
    #[test]
    fn whole_number_blend_differs_from_the_float_blend() {
        use rayon::prelude::*;
        let differing: u32 = (0..=u8::MAX)
            .into_par_iter()
            .map(|a| {
                let cases = (0..=u8::MAX).flat_map(|b| (0..=1000i32).map(move |thousandth| (b, thousandth)));
                cases
                    .filter(|&(b, thousandth)| {
                        let whole = (i32::from(a) * 1000 + (i32::from(b) - i32::from(a)) * thousandth + 500) / 1000;
                        whole as u8 != blend_channel(a, b, thousandth as f32 / 1000.0).round() as u8
                    })
                    .count() as u32
            })
            .sum();
        assert_eq!(differing, 416);
    }

    const ALL_ROUNDINGS: [ChannelRounding; 4] = [
        ChannelRounding::Library,
        ChannelRounding::CheckedTruncation,
        ChannelRounding::UncheckedTruncation,
        ChannelRounding::IntegerSpacingShift,
    ];

    /// Deterministic frame of `side` × `side` pixels whose bytes depend on `seed`.
    fn noise_frame(side: u32, seed: u32) -> Vec<Image> {
        let byte = |i: u32| (i.wrapping_mul(2_654_435_761).wrapping_add(seed.wrapping_mul(40_503)) >> 13) as u8;
        vec![Image { width: side, height: side, rgba: (0..side * side * 4).map(byte).collect() }]
    }

    /// A blended and a plain animation over 200 ticks: the borrowed pixels are the owned image's,
    /// for each rounding, and `has_frame` says when there is one. Sabotage: a blend whose share is
    /// not truncated to the thousandth gives other bytes.
    #[test]
    fn borrowed_pixels_equal_the_owned_image_on_every_tick() {
        let frames = || vec![noise_frame(4, 1), noise_frame(4, 2), noise_frame(4, 3)];
        let steps = vec![(0, 7), (2, 3), (1, 5)];
        let mut buffer = Vec::new();
        let mut untruncated_share_differs = false;
        for interpolate in [true, false] {
            let animation = Animation { frames: steps.clone(), interpolate };
            let mut playback = SpriteAnim::new(frames(), &animation).expect("animated");
            for tick in 0..200 {
                let owned = playback.current(4).expect("frame at the known width");
                let borrowed = playback.current_pixels(4, &mut buffer).expect("frame at the known width");
                assert_eq!((borrowed.height, borrowed.rgba), (owned.height, owned.rgba.as_slice()), "tick {tick}");
                assert!(playback.has_frame(4, 4) && !playback.has_frame(4, 8) && !playback.has_frame(8, 8));
                if let (current, Some(next)) = playback.shown_frames(4).expect("frames") {
                    let (current, next) = (current.rgba.clone(), next.rgba.clone());
                    let mut by_rounding = Vec::new();
                    for rounding in ALL_ROUNDINGS {
                        blend_into(&current, &next, playback.share_of_next(), rounding, &mut by_rounding);
                        assert_eq!(by_rounding, owned.rgba, "tick {tick}, {rounding:?}");
                    }
                    let exact_share = playback.ticks_into_step as f32 / playback.steps[playback.step].1 as f32;
                    blend_into(&current, &next, exact_share, ChannelRounding::IntegerSpacingShift, &mut by_rounding);
                    untruncated_share_differs |= by_rounding != owned.rgba;
                }
                playback.tick();
            }
            assert!(playback.current_pixels(8, &mut buffer).is_none());
        }
        assert!(untruncated_share_differs, "sabotage not detected");
    }

    /// Micro-bench (ignored by default, `--ignored --nocapture`): blend of one 16 × 16 frame
    /// (1,024 channels) per rounding, into a new vector or into a kept buffer; 1 core (the blend
    /// runs on the main thread, a few frames per tick: nothing to spread over cores).
    #[test]
    #[ignore]
    fn bench_blend() {
        let (current, next) = (noise_frame(16, 1).remove(0).rgba, noise_frame(16, 2).remove(0).rgba);
        const BLENDS: u32 = 200_000;
        let time = |name: &str, rounding: ChannelRounding, keep_buffer: bool| {
            let mut best = f64::MAX;
            let mut checksum = 0u64;
            for _ in 0..5 {
                let mut buffer = Vec::new();
                let start = std::time::Instant::now();
                for i in 0..BLENDS {
                    if !keep_buffer {
                        buffer = Vec::new();
                    }
                    let share = (i % 1000) as f32 / 1000.0;
                    blend_into(std::hint::black_box(&current), &next, share, rounding, &mut buffer);
                    checksum += u64::from(std::hint::black_box(&buffer)[i as usize % buffer.len()]);
                }
                best = best.min(start.elapsed().as_secs_f64() * 1e9 / f64::from(BLENDS));
            }
            eprintln!(
                "{name}: {best:.0} ns per 16x16 frame ({:.2} ns per channel), checksum {checksum}",
                best / 1024.0
            );
        };
        // Twice, in opposite orders: the machine's load drifts during the bench.
        for rounding in ALL_ROUNDINGS.into_iter().chain(ALL_ROUNDINGS.into_iter().rev()) {
            time(&format!("{rounding:?}, new vector"), rounding, false);
            time(&format!("{rounding:?}, kept buffer"), rounding, true);
        }
    }

    fn px(v: u8) -> Vec<Image> {
        vec![Image { width: 1, height: 1, rgba: vec![v, v, v, 255] }]
    }

    #[test]
    fn steps_follow_durations_and_order() {
        let a = Animation { frames: vec![(1, 2), (0, 1)], interpolate: false };
        let mut s = SpriteAnim::new(vec![px(10), px(20)], &a).expect("animated");
        assert_eq!(s.current(1).expect("image").rgba[0], 20);
        assert!(!s.tick());
        assert!(s.tick());
        assert_eq!(s.current(1).expect("image").rgba[0], 10);
        assert!(s.tick());
        assert_eq!(s.current(1).expect("image").rgba[0], 20);
        assert!(s.current(2).is_none());
    }

    #[test]
    fn interpolated_fade_redraws_every_tick() {
        let a = Animation { frames: vec![(0, 4), (1, 4)], interpolate: true };
        let mut s = SpriteAnim::new(vec![px(0), px(200)], &a).expect("animated");
        assert!(s.tick());
        assert_eq!(s.current(1).expect("image").rgba[0], 50);
        assert_eq!(s.current(1).expect("image").rgba[3], 255);
    }

    #[test]
    fn single_step_is_not_animated() {
        let a = Animation { frames: vec![(0, 1), (5, 1)], interpolate: false };
        assert!(SpriteAnim::new(vec![px(0)], &a).is_none());
    }
}
