//! Spawn lists specific to structures (`server-mobs.md` § 4.F07): a structure
//! with a start covering the position replaces the biome's list for its category (outpost:
//! pillagers across the enlarged box; witch hut: witch and cat in the room);
//! the Nether fortress has its own fixed list when the block below is Nether bricks.
//! Structures are read from the `structures` compound kept by each loaded chunk
//! (references to the starting chunks, `BB` boxes of the pieces). A starting chunk that is not
//! loaded is ignored (the game would load it up to the "starts" step).

use std::sync::Arc;

use rc_proto::nbt::Nbt;
use serde_json::{Value, json};

use super::species::Category;
use crate::server::Server;

/// Integer box `[min x, y, z, max x, y, z]` (bounds included).
type Bb = [i32; 6];

fn inside(b: &Bb, p: [i32; 3]) -> bool {
    p[0] >= b[0] && p[0] <= b[3] && p[1] >= b[1] && p[1] <= b[4] && p[2] >= b[2] && p[2] <= b[5]
}

fn bb_of(n: &Nbt) -> Option<Bb> {
    match n.get("BB")? {
        Nbt::IntArray(v) if v.len() == 6 => Some([v[0], v[1], v[2], v[3], v[4], v[5]]),
        _ => None,
    }
}

/// The game's chunk key: x in the low 32 bits, z in the high 32 bits.
fn unpack_chunk(k: i64) -> (i32, i32) {
    (k as i32, (k >> 32) as i32)
}

/// Fixed list of the Nether fortress enemies (weight, type, min count, max).
fn fortress_enemies() -> Vec<Value> {
    [
        ("minecraft:blaze", 10, 2, 3),
        ("minecraft:zombified_piglin", 5, 4, 4),
        ("minecraft:wither_skeleton", 8, 5, 5),
        ("minecraft:skeleton", 2, 5, 5),
        ("minecraft:magma_cube", 3, 4, 4),
    ]
    .into_iter()
    .map(|(t, w, lo, hi)| {
        let count = if lo == hi { json!(lo) } else { json!({"min_inclusive": lo, "max_inclusive": hi}) };
        json!({"type": t, "weight": w, "count": count})
    })
    .collect()
}

impl Server {
    /// Valid starts of a structure referenced by the chunk of `p`, in the order of the
    /// references: `f(name, pieces)` up to the first `Some`. The pieces stay in the NBT of the
    /// starting chunk (read as needed): a spawn attempt no longer copies the boxes of
    /// hundreds of pieces (fortress) to often look at none of them.
    fn structure_starts_find<R>(&self, p: [i32; 3], mut f: impl FnMut(&str, &[Nbt]) -> Option<R>) -> Option<R> {
        let Some(Some(c)) = self.chunks.get(&(p[0] >> 4, p[2] >> 4)) else { return None };
        let Some(Nbt::Compound(refs)) = c.root.get("structures").and_then(|s| s.get("References")) else {
            return None;
        };
        for (name, keys) in refs {
            let Nbt::LongArray(keys) = keys else { continue };
            for &k in keys {
                let Some(Some(sc)) = self.chunks.get(&unpack_chunk(k)) else { continue };
                let Some(start) = sc.root.get("structures").and_then(|s| s.get("starts")).and_then(|s| s.get(name))
                else {
                    continue;
                };
                if start.get("id").and_then(Nbt::as_str) == Some("INVALID") {
                    continue;
                }
                let Some(Nbt::List(children)) = start.get("Children") else { continue };
                // Start with no readable box: ignored.
                if !children.iter().any(|n| bb_of(n).is_some()) {
                    continue;
                }
                if let Some(r) = f(name, children) {
                    return Some(r);
                }
            }
        }
        None
    }

    /// Start of a structure covering `p` (integer box, enlarged by 12 with terrain adaptation),
    /// like the structure at a spot.
    pub(crate) fn in_structure(&self, p: [i32; 3], id: &str) -> bool {
        let def = self.life.pack.get("worldgen/structure", id.strip_prefix("minecraft:").unwrap_or(id));
        let grow = def.as_ref().is_some_and(|d| d["terrain_adaptation"].as_str().is_some_and(|t| t != "none"));
        self.structure_starts_find(p, |n, ch| {
            (n == id && full_box_of(ch, grow).is_some_and(|b| inside(&b, p))).then_some(())
        })
        .is_some()
    }

    /// A piece of a structure `id` contains `p`.
    pub(crate) fn in_structure_piece(&self, p: [i32; 3], id: &str) -> bool {
        self.structure_starts_find(p, |n, ch| (n == id && in_piece(ch, p)).then_some(())).is_some()
    }
    /// Spawn list of a category at a position: fortress, structure that
    /// replaces the list, otherwise biome.
    ///
    /// Shared lists (kept by the pack): no more deep copy of the JSON at each attempt.
    pub(crate) fn spawn_list_at(&self, cat: Category, key: &str, p: [i32; 3]) -> Arc<[Value]> {
        if cat == Category::Monster
            && self.bname([p[0], p[1] - 1, p[2]]) == "minecraft:nether_bricks"
            && self.in_structure(p, "minecraft:fortress")
        {
            static FORTRESS: std::sync::OnceLock<Arc<[Value]>> = std::sync::OnceLock::new();
            return FORTRESS.get_or_init(|| fortress_enemies().into()).clone();
        }
        // Structure without replacement for this category: its pieces are not read.
        let found = self.structure_starts_find(p, |name, ch| {
            let ov = self.life.pack.structure_spawns(name, key)?;
            let hit = if ov.piece { in_piece(ch, p) } else { full_box_of(ch, ov.grow).is_some_and(|b| inside(&b, p)) };
            hit.then(|| ov.spawns.clone())
        });
        found.unwrap_or_else(|| self.life.pack.biome_spawns(self.biome_str(p), key))
    }
}

/// A readable piece of the start contains `p`.
fn in_piece(children: &[Nbt], p: [i32; 3]) -> bool {
    children.iter().filter_map(bb_of).any(|b| inside(&b, p))
}

/// Box enclosing the readable pieces of a start, enlarged by 12
/// if the structure adapts the terrain, without copying them; `None` if there is
/// none.
fn full_box_of(children: &[Nbt], grow: bool) -> Option<Bb> {
    let mut it = children.iter().filter_map(bb_of);
    let mut b = it.next()?;
    for p in it {
        for i in 0..3 {
            b[i] = b[i].min(p[i]);
            b[i + 3] = b[i + 3].max(p[i + 3]);
        }
    }
    if grow {
        for i in 0..3 {
            b[i] -= 12;
            b[i + 3] += 12;
        }
    }
    Some(b)
}

/// Test reference: same box from pieces already read.
#[cfg(test)]
fn full_box(pieces: &[Bb], grow: bool) -> Bb {
    let mut b = pieces[0];
    for p in &pieces[1..] {
        for i in 0..3 {
            b[i] = b[i].min(p[i]);
            b[i + 3] = b[i + 3].max(p[i + 3]);
        }
    }
    if grow {
        for i in 0..3 {
            b[i] -= 12;
            b[i + 3] += 12;
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_and_keys() {
        let pieces = [[0, 60, 0, 4, 70, 4], [10, 62, -3, 12, 64, 1]];
        let b = full_box(&pieces, false);
        assert_eq!(b, [0, 60, -3, 12, 70, 4]);
        let g = full_box(&pieces, true);
        assert!(inside(&g, [-12, 48, 16]) && !inside(&g, [-13, 60, 0]));
        let k = (i64::from(-5i32) & 0xFFFF_FFFF) | (i64::from(7i32) << 32);
        assert_eq!(unpack_chunk(k), (-5, 7));
        assert_eq!(fortress_enemies().len(), 5);
    }
}
