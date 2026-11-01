//! F3 entry "chunk_generation_stats" (singleplayer): generator lines at the player's feet, computed
//! by the integrated server at the debug screen's request, only when the feet position changes
//! (like the game: a dimension change without moving keeps the old lines).

use std::sync::Mutex;

use crate::server::Server;

#[derive(Default)]
struct State {
    /// Position requested by the screen (last frame where the entry was shown).
    want: Option<[i32; 3]>,
    /// Position of the computed lines.
    done: Option<[i32; 3]>,
    lines: Vec<String>,
}

/// Exchange between the client's F3 screen and the integrated server.
#[derive(Default)]
pub struct GenDebug(Mutex<State>);

impl GenDebug {
    /// Client: requests the lines at the `feet` block and returns the last computed ones.
    pub fn request(&self, feet: [i32; 3]) -> Vec<String> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.want = Some(feet);
        s.lines.clone()
    }

    /// Server: position to recompute, if there is a new one.
    fn pending(&self) -> Option<[i32; 3]> {
        let s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.want.filter(|w| s.done != Some(*w))
    }

    fn store(&self, at: [i32; 3], lines: Vec<String>) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.done = Some(at);
        s.lines = lines;
    }
}

impl Server {
    /// Lines requested by the F3 screen, recomputed outside the lock.
    pub(crate) fn gen_debug_tick(&mut self) {
        let Some(at) = self.gen_debug.pending() else { return };
        let lines = self.gen_debug_lines(at);
        self.gen_debug.store(at, lines);
    }

    /// Multi-noise source: biome builder categories; other sources: nothing.
    pub(crate) fn gen_debug_lines(&mut self, at: [i32; 3]) -> Vec<String> {
        use rc_worldgen::biome::BiomeSource;
        let Some(g) = self.locate_gen() else { return Vec::new() };
        let Some(t) = g.terrain_generator() else { return Vec::new() };
        if !matches!(t.source, BiomeSource::MultiNoise(_)) {
            return Vec::new();
        }
        let mut ctx = rc_worldgen::sampler::Ctx::cached();
        let c = BiomeSource::climate_at(&t.g, &mut ctx, at[0] >> 2, at[1] >> 2, at[2] >> 2);
        vec![rc_worldgen::biome::builder_line(c)]
    }
}

#[cfg(test)]
mod tests {
    use super::GenDebug;

    /// Recompute only for a new position; nothing until the screen has requested something.
    #[test]
    fn recompute_only_on_new_position() {
        let g = GenDebug::default();
        assert_eq!(g.pending(), None);
        assert!(g.request([1, 2, 3]).is_empty());
        assert_eq!(g.pending(), Some([1, 2, 3]));
        g.store([1, 2, 3], vec!["a".into()]);
        assert_eq!(g.pending(), None);
        assert_eq!(g.request([1, 2, 3]), vec!["a".to_owned()]);
        assert_eq!(g.pending(), None);
        // New position: the old lines stay displayed until computed.
        assert_eq!(g.request([1, 2, 4]), vec!["a".to_owned()]);
        assert_eq!(g.pending(), Some([1, 2, 4]));
    }
}
