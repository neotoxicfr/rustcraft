//! The sections the game would hold compiled, for the visibility of entities and block entities.
//!
//! The game draws an entity whose box is in the view frustum as soon as the section of its block
//! position is compiled and has appeared to 0.3 of its fade, without asking its occlusion walk; it
//! draws block entities only from compiled sections at 0.3 of their fade. It compiles a section the
//! first time its walk reaches it in view, and takes an empty section (air alone) of a loaded column
//! as compiled the first time its walk reaches it, column compiled or not. So an entity behind a
//! hill is drawn (its glowing outline and its name show through the hill) if its section was seen
//! once, and one in a cave never seen is not.
//!
//! The game holds its sections in a square of columns around the camera's section, as many on each
//! side as the render distance: a section outside it is never compiled (nothing there is drawn,
//! whatever columns the server still sends), a section the square leaves when the camera changes
//! section is reset (its fade forgotten), and a new render distance resets them all. Unloading a
//! column resets nothing: a column sent again within the square shows its entities at once. The game
//! moves the square at the start of its drawing, after the entity test of that frame; ours too
//! ([`CompiledSections::reposition`] from the terrain render).
//!
//! Our meshing builds a column whole: a section counts as compiled from the first walk that reaches
//! it while its column is meshed (or from the meshing of a column the last walk reached), an empty
//! section of a column not meshed yet from the first walk that reaches it; the view frustum the game
//! also asks of a section it compiles is left out (the box of an entity in a section out of view is
//! out of view too). The instant of that first time starts the section's fade.

use std::time::{Duration, Instant};

use hashbrown::HashMap;
use rc_mesh::visibility::VIS_ALL;

use super::walk_inputs::Connectivity;

/// Share of its fade from which the game shows what a compiled section holds.
const SHOWN_FROM_FADE: f64 = 0.3;

/// The square of columns the game holds sections for: `radius` columns on each side of the
/// camera's column `center` (module header).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ColumnSquare {
    pub center: (i32, i32),
    pub radius: i32,
}

impl ColumnSquare {
    fn contains(&self, key: (i32, i32)) -> bool {
        let reach = u32::try_from(self.radius).unwrap_or(0);
        key.0.abs_diff(self.center.0) <= reach && key.1.abs_diff(self.center.1) <= reach
    }
}

/// The compiled sections of the square around the camera (module header).
#[derive(Clone)]
pub(super) struct CompiledSections {
    epoch: Instant,
    /// Per column, per section from the bottom of the world: when it became compiled, in
    /// milliseconds after `epoch` plus one (4 bytes rather than an `Instant`'s 16); 0 while it is not.
    columns: HashMap<(i32, i32), Box<[u32]>>,
    /// `None` before the first frame placed it: no section is compiled yet.
    square: Option<ColumnSquare>,
}

impl CompiledSections {
    pub fn new(epoch: Instant) -> Self {
        CompiledSections { epoch, columns: HashMap::new(), square: None }
    }

    /// The square the sections are held in, as the last frame placed it.
    pub fn square(&self) -> Option<ColumnSquare> {
        self.square
    }

    /// The camera's square becomes `square`: the columns it leaves are reset, and all of them when
    /// the render distance changed (module header).
    pub fn reposition(&mut self, square: ColumnSquare) {
        match self.square {
            Some(old) if old == square => return,
            Some(old) if old.radius == square.radius => self.columns.retain(|&key, _| square.contains(key)),
            _ => self.columns.clear(),
        }
        self.square = Some(square);
    }

    fn stamp(&self, now: Instant) -> u32 {
        let ms = now.saturating_duration_since(self.epoch).as_millis();
        u32::try_from(ms).unwrap_or(u32::MAX).saturating_add(1)
    }

    /// Sections `heights` of column `key` (`sections` high) become compiled at `now`, unless they
    /// already are or the column is outside the camera's square.
    pub fn mark(&mut self, key: (i32, i32), sections: usize, heights: impl Iterator<Item = i32>, now: Instant) {
        let mut heights = heights.peekable();
        if heights.peek().is_none() || !self.square.is_some_and(|s| s.contains(key)) {
            return;
        }
        let stamp = self.stamp(now);
        let slots = self.columns.entry(key).or_insert_with(|| vec![0; sections].into_boxed_slice());
        for height in heights {
            if let Some(slot) = usize::try_from(height).ok().and_then(|y| slots.get_mut(y))
                && *slot == 0
            {
                *slot = stamp;
            }
        }
    }

    /// A walk reached `reached` (sections sorted by column, then height): the sections of the meshed
    /// columns (`meshed`) and the empty sections of the columns loaded but not meshed (`unmeshed`:
    /// their provisional connectivity, open on every side exactly when empty) become compiled.
    pub fn mark_reached(
        &mut self,
        reached: &[(i32, i32, i32)],
        sections: usize,
        meshed: impl Fn((i32, i32)) -> bool,
        unmeshed: &HashMap<(i32, i32), Connectivity>,
        now: Instant,
    ) {
        for run in reached.chunk_by(|a, b| (a.0, a.2) == (b.0, b.2)) {
            let key = (run[0].0, run[0].2);
            let heights = run.iter().map(|s| s.1);
            if meshed(key) {
                self.mark(key, sections, heights, now);
            } else if let Some(vis) = unmeshed.get(&key) {
                let empty = |y: &i32| usize::try_from(*y).ok().and_then(|y| vis.get(y)) == Some(&VIS_ALL);
                self.mark(key, sections, heights.filter(empty), now);
            }
        }
    }

    /// No column is loaded any more (another world or dimension: the game builds its square anew).
    pub fn clear(&mut self) {
        self.columns.clear();
    }

    /// Section `sy` (from the bottom of the world) of column `key` is compiled and, at `now`, has
    /// appeared to 0.3 of a fade of `fade_secs` (0: no fade). Only columns of the camera's square
    /// are held ([`Self::reposition`], [`Self::mark`]), so one outside it is never shown.
    pub fn shown(&self, key: (i32, i32), sy: i32, now: Instant, fade_secs: f64) -> bool {
        let slot = usize::try_from(sy).ok().and_then(|y| self.columns.get(&key)?.get(y).copied());
        let Some(stamp) = slot.filter(|&s| s != 0) else { return false };
        let compiled_at = self.epoch + Duration::from_millis(u64::from(stamp - 1));
        fade_secs <= 0.0 || now.saturating_duration_since(compiled_at).as_secs_f64() >= fade_secs * SHOWN_FROM_FADE
    }
}

#[cfg(test)]
#[path = "compiled_sections_test.rs"]
mod tests;
