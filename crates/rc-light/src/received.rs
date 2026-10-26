//! Singleplayer client (mission C2-M05): sections whose light has just arrived from the integrated server.
//!
//! The integrated server recomputes the light at the end of each tick and sends it (changed sections)
//! BEFORE the block changes of the same tick. The local relight the client runs for these
//! blocks therefore redoes work already received. It is only skipped for a change whose section
//! received a sheet (sky or block) since the last local relight: the section was then last
//! written by the server, which knows the same world. A change without received light (light
//! that arrived on the previous frame, server that changed nothing in the section) keeps the relight: a
//! section written by a local relight on a client-own state (piston replay, prediction)
//! thus never waits for a correction the server would not send.

use hashbrown::HashSet;
use rc_world::LightLayer;

/// Sheet positions (column x, sheet position, column z) received since the last
/// [`Received::clear`].
#[derive(Debug, Default)]
pub struct Received {
    seen: HashSet<(i32, i32, i32)>,
}

impl Received {
    /// Sheets carried by a light update of column `(x, z)` (sheet position =
    /// section index + 1).
    pub fn note(&mut self, x: i32, z: i32, sky: &[LightLayer], block: &[LightLayer]) {
        for layers in [sky, block] {
            for (i, l) in layers.iter().enumerate() {
                if l.is_some() {
                    self.seen.insert((x, i as i32, z));
                }
            }
        }
    }

    /// Section `sy` (section index from the bottom of the world) of column `(cx, cz)` received
    /// a sheet.
    pub fn covers(&self, cx: i32, sy: i32, cz: i32) -> bool {
        self.seen.contains(&(cx, sy + 1, cz))
    }

    /// After the local relight of the frame.
    pub fn clear(&mut self) {
        if !self.seen.is_empty() {
            self.seen.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_covers_clear() {
        let some = Some(rc_world::zero_layer());
        let sky = vec![None, some.clone(), None];
        let block = vec![None, None, some];
        let mut r = Received::default();
        r.note(2, -3, &sky, &block);
        assert!(r.covers(2, 0, -3), "sky sheet at position 1 = section 0");
        assert!(r.covers(2, 1, -3), "block sheet at position 2 = section 1");
        assert!(!r.covers(2, -1, -3));
        assert!(!r.covers(2, 0, 3));
        assert!(!r.covers(-3, 0, 2));
        r.clear();
        assert!(!r.covers(2, 0, -3));
    }
}
