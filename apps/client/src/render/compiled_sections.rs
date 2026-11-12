//! The sections the game would hold compiled, for the visibility of entities and block entities.
//!
//! The game draws an entity whose box is in the view frustum as soon as the section of its block
//! position is compiled and has appeared to 0.3 of its fade, without asking its occlusion walk; it
//! draws block entities only from compiled sections at 0.3 of their fade. It compiles a section the
//! first time its walk reaches it in view, keeps it compiled while the column stays loaded, and
//! takes an empty section (air alone) of a loaded column as compiled the first time its walk reaches
//! it, column compiled or not. So an entity behind a hill is drawn (its glowing outline and its name
//! show through the hill) if its section was seen once, and one in a cave never seen is not.
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

/// The compiled sections of the loaded columns (module header).
#[derive(Clone)]
pub(super) struct CompiledSections {
    epoch: Instant,
    /// Per column, per section from the bottom of the world: when it became compiled, in
    /// milliseconds after `epoch` plus one (4 bytes rather than an `Instant`'s 16); 0 while it is not.
    columns: HashMap<(i32, i32), Box<[u32]>>,
}

impl CompiledSections {
    pub fn new(epoch: Instant) -> Self {
        CompiledSections { epoch, columns: HashMap::new() }
    }

    fn stamp(&self, now: Instant) -> u32 {
        let ms = now.saturating_duration_since(self.epoch).as_millis();
        u32::try_from(ms).unwrap_or(u32::MAX).saturating_add(1)
    }

    /// Sections `heights` of column `key` (`sections` high) become compiled at `now`, unless they
    /// already are.
    pub fn mark(&mut self, key: (i32, i32), sections: usize, heights: impl Iterator<Item = i32>, now: Instant) {
        let mut heights = heights.peekable();
        if heights.peek().is_none() {
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

    /// Column `key` is no longer loaded.
    pub fn forget_column(&mut self, key: (i32, i32)) {
        self.columns.remove(&key);
    }

    /// No column is loaded any more.
    pub fn clear(&mut self) {
        self.columns.clear();
    }

    /// Section `sy` (from the bottom of the world) of column `key` is compiled and, at `now`, has
    /// appeared to 0.3 of a fade of `fade_secs` (0: no fade).
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
