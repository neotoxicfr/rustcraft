//! Animation of a texture copied into a separate sheet (interface item sprites,
//! entity textures): same reading as the block atlas (`.png.mcmeta`: steps, durations in
//! ticks, cross-fade), one step per advance, and the current frame rewritten into the sheet
//! only when it changes (on every advance for a blended animation).

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
    sub: u32,
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
        (steps.len() > 1).then_some(SpriteAnim { frames, steps, interpolate: animation.interpolate, step: 0, sub: 0 })
    }

    /// One advance of a tick; true if the displayed frame has changed.
    pub fn tick(&mut self) -> bool {
        self.sub += 1;
        let old = self.steps[self.step].0;
        if self.sub >= self.steps[self.step].1 {
            self.step = (self.step + 1) % self.steps.len();
            self.sub = 0;
        }
        self.interpolate || self.steps[self.step].0 != old
    }

    /// Current frame at width `width` (one of the known sizes), blended towards the next one
    /// in proportion to the step for a blended animation; `None` if this width is missing.
    pub fn current(&self, width: u32) -> Option<Image> {
        let pick = |f: u32| self.frames[f as usize].iter().find(|i| i.width == width);
        let (frame, time) = self.steps[self.step];
        let cur = pick(frame)?;
        if !self.interpolate {
            return Some(cur.clone());
        }
        let next = pick(self.steps[(self.step + 1) % self.steps.len()].0).filter(|n| n.rgba.len() == cur.rgba.len())?;
        // Share of the next one truncated to the thousandth, like the game.
        let p = ((self.sub as f32 / time as f32) * 1000.0) as u32 as f32 / 1000.0;
        let rgba = cur
            .rgba
            .iter()
            .zip(&next.rgba)
            .map(|(&a, &b)| (f32::from(a) + (f32::from(b) - f32::from(a)) * p).round() as u8)
            .collect();
        Some(Image { width: cur.width, height: cur.height, rgba })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
