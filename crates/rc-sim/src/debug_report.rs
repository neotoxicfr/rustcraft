//! F3 values that only the integrated server knows (singleplayer): heightmaps of the server chunk
//! at the feet ("SH"), number of force-loaded chunks ("FC"), sprint of the tick rate, local
//! difficulty and the last natural spawn census ("SC"). The debug screen posts its dimension and
//! feet block every frame one of these entries is shown; the server answers at its next loop turn
//! with the values of the last tick, and computes nothing while nobody asks.

use std::sync::Mutex;

use crate::server::Server;

/// Server values for the debug screen (`Copy`: read every frame without allocation).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Report {
    /// Highest block taken in the WORLD_SURFACE, OCEAN_FLOOR, MOTION_BLOCKING and
    /// MOTION_BLOCKING_NO_LEAVES maps of the feet column; `None`: chunk not loaded on the server.
    pub heightmaps: Option<[i32; 4]>,
    pub forced: usize,
    pub sprinting: bool,
    /// Effective difficulty and special multiplier; `None`: chunk not loaded or feet outside the
    /// build height.
    pub local_difficulty: Option<(f32, f32)>,
    /// Last natural spawn census of the dimension: spawn chunk count, mobs per category in the
    /// game's order (monster, creature, ambient, axolotls, underground water creature, water
    /// creature, water ambient; the game's "misc" is never counted).
    pub spawn: Option<(i32, [i32; 7])>,
}

#[derive(Default)]
struct State {
    /// Last request: dimension and feet block, `asked` until the server picks it up.
    dim: String,
    feet: [i32; 3],
    asked: bool,
    /// Dimension of `report`.
    done: String,
    report: Option<Report>,
}

/// Exchange between the client's F3 screen and the integrated server.
#[derive(Default)]
pub struct DebugReport(Mutex<State>);

impl DebugReport {
    /// Client: posts the dimension and feet block shown and returns the last report of that
    /// dimension (`None` until the server has answered).
    pub fn request(&self, dim: &str, feet: [i32; 3]) -> Option<Report> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if s.dim != dim {
            s.dim.clear();
            s.dim.push_str(dim);
        }
        s.feet = feet;
        s.asked = true;
        s.report.filter(|_| s.done == dim)
    }

    /// Server: feet block of a pending request for its dimension `dim` (a request for another
    /// dimension, during a change, gets no report).
    fn pending(&self, dim: &str) -> Option<[i32; 3]> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !std::mem::take(&mut s.asked) {
            return None;
        }
        (s.dim == dim).then_some(s.feet)
    }

    fn store(&self, dim: &str, r: Report) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if s.done != dim {
            s.done.clear();
            s.done.push_str(dim);
        }
        s.report = Some(r);
    }
}

/// Network identifiers of the maps shown by "SH", in the game's order.
const SERVER_MAPS: [i32; 4] = [1, 3, 4, 5];

impl Server {
    /// Answer to the F3 screen, once per loop turn while it asks.
    pub(crate) fn debug_report_tick(&mut self) {
        let Some(feet) = self.debug_report.pending(&self.dim.name) else { return };
        let r = self.debug_report_values(feet);
        self.debug_report.store(&self.dim.name, r);
    }

    pub(crate) fn debug_report_values(&self, feet: [i32; 3]) -> Report {
        let h = self.dim.height;
        let chunk = self.chunks.get(&(feet[0] >> 4, feet[2] >> 4)).and_then(|c| c.as_deref());
        let heightmaps = chunk.map(|c| {
            let (bits, longs) =
                (rc_world::heightmap::bits(h.sections * 16), rc_world::heightmap::longs(h.sections * 16));
            let i = ((feet[2] & 15) * 16 + (feet[0] & 15)) as usize;
            SERVER_MAPS.map(|id| {
                let map = c.heightmaps.iter().find(|(k, l)| *k == id && l.len() == longs);
                let stored = map.map_or(0, |(_, l)| rc_world::heightmap::get(l, i, bits));
                // First free height − 1: the highest block taken (an empty column: below the world).
                stored + h.min_y - 1
            })
        });
        let inside = feet[1] >= h.min_y && feet[1] < h.min_y + h.sections as i32 * 16;
        let census = self.mobs.census.as_ref().filter(|c| c.dim == self.dim.name);
        Report {
            heightmaps,
            forced: self.cmd.forced.len(),
            sprinting: self.cmd.ticking.sprinting(),
            local_difficulty: (chunk.is_some() && inside).then(|| self.local_difficulty(feet)),
            spawn: census.map(|c| (c.chunks, c.counts)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DebugReport, Report};

    /// Nothing computed before a request; one answer per request; a report of another
    /// dimension is not returned.
    #[test]
    fn answers_only_requests_of_its_dimension() {
        let d = DebugReport::default();
        assert_eq!(d.pending("minecraft:overworld"), None);
        assert_eq!(d.request("minecraft:overworld", [1, 64, 2]), None);
        assert_eq!(d.pending("minecraft:overworld"), Some([1, 64, 2]));
        assert_eq!(d.pending("minecraft:overworld"), None, "picked up once");
        let r = Report { forced: 3, ..Report::default() };
        d.store("minecraft:overworld", r);
        assert_eq!(d.request("minecraft:overworld", [1, 64, 2]), Some(r));
        // Dimension change: the overworld report is not shown, the server skips the request.
        assert_eq!(d.request("minecraft:the_nether", [0, 70, 0]), None);
        assert_eq!(d.pending("minecraft:overworld"), None);
    }
}
