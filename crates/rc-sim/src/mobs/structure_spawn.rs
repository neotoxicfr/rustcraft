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
        let grow = self.structure_adapts_terrain(id);
        self.structure_starts_find(p, |n, ch| {
            (n == id && full_box_of(ch, grow).is_some_and(|b| inside(&b, p))).then_some(())
        })
        .is_some()
    }

    /// The structure `id` adapts the terrain around it (its whole box then reaches 12 blocks
    /// beyond its pieces).
    pub(crate) fn structure_adapts_terrain(&self, id: &str) -> bool {
        let def = self.life.pack.get("worldgen/structure", id.strip_prefix("minecraft:").unwrap_or(id));
        def.as_ref().is_some_and(|d| d["terrain_adaptation"].as_str().is_some_and(|t| t != "none"))
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
            return fortress_list();
        }
        // Structure without replacement for this category: its pieces are not read.
        let found = self.structure_starts_find(p, |name, ch| {
            let ov = self.life.pack.structure_spawns(name, key)?;
            let hit = if ov.piece { in_piece(ch, p) } else { full_box_of(ch, ov.grow).is_some_and(|b| inside(&b, p)) };
            hit.then(|| ov.spawns.clone())
        });
        found.unwrap_or_else(|| self.life.pack.biome_spawns(self.biome_str(p), key))
    }

    /// `spawn_list_at` on the starts decoded for a natural spawn pass: each start's pieces are
    /// read from its NBT once per pass instead of on every attempt (a fortress has hundreds).
    pub(crate) fn spawn_list_decoded(
        &self,
        starts: &mut DecodedStarts,
        cat: Category,
        key: &str,
        p: [i32; 3],
    ) -> Arc<[Value]> {
        if cat == Category::Monster
            && self.bname([p[0], p[1] - 1, p[2]]) == "minecraft:nether_bricks"
            && self.in_structure_decoded(starts, p, "minecraft:fortress")
        {
            return fortress_list();
        }
        let found = self.decoded_starts_find(starts, p, |name, start| {
            let ov = self.life.pack.structure_spawns(name, key)?;
            let hit = if ov.piece { start.has_piece_at(p) } else { inside(&start.full_box(ov.grow), p) };
            hit.then(|| ov.spawns.clone())
        });
        found.unwrap_or_else(|| self.life.pack.biome_spawns(self.biome_str(p), key))
    }

    /// `in_structure` on the decoded starts.
    fn in_structure_decoded(&self, starts: &mut DecodedStarts, p: [i32; 3], id: &str) -> bool {
        self.decoded_starts_find(starts, p, |n, start| {
            (n == id && inside(&start.full_box(start.adapts_terrain), p)).then_some(())
        })
        .is_some()
    }

    /// `structure_starts_find` on the decoded starts: the references of the chunk of `p` are
    /// read as before, each start they name decoded on its first use in the pass.
    fn decoded_starts_find<R>(
        &self,
        starts: &mut DecodedStarts,
        p: [i32; 3],
        mut f: impl FnMut(&str, &StartBoxes) -> Option<R>,
    ) -> Option<R> {
        let Some(Some(c)) = self.chunks.get(&(p[0] >> 4, p[2] >> 4)) else { return None };
        let Some(Nbt::Compound(refs)) = c.root.get("structures").and_then(|s| s.get("References")) else {
            return None;
        };
        for (name, keys) in refs {
            let Nbt::LongArray(keys) = keys else { continue };
            for &k in keys {
                if let Some(start) = starts.get_or_decode(self, unpack_chunk(k), name)
                    && let Some(r) = f(name, start)
                {
                    return Some(r);
                }
            }
        }
        None
    }

    /// Boxes of the start `name` kept by chunk `chunk`; `None` when the start is not usable
    /// (chunk not loaded, start missing or `INVALID`, no readable piece box), like the checks
    /// of `structure_starts_find`.
    fn decode_start(&self, chunk: (i32, i32), name: &str) -> Option<StartBoxes> {
        let Some(Some(sc)) = self.chunks.get(&chunk) else { return None };
        let start = sc.root.get("structures")?.get("starts")?.get(name)?;
        if start.get("id").and_then(Nbt::as_str) == Some("INVALID") {
            return None;
        }
        let Some(Nbt::List(children)) = start.get("Children") else { return None };
        let pieces: Vec<Bb> = children.iter().filter_map(bb_of).collect();
        let full = full_box_of(children, false)?;
        let adapts_terrain = self.structure_adapts_terrain(name);
        Some(StartBoxes { pieces, full, adapts_terrain, by_column: std::cell::OnceCell::new() })
    }
}

/// The fixed fortress list, built once.
fn fortress_list() -> Arc<[Value]> {
    static FORTRESS: std::sync::OnceLock<Arc<[Value]>> = std::sync::OnceLock::new();
    FORTRESS.get_or_init(|| fortress_enemies().into()).clone()
}

/// Structure starts decoded during one natural spawn pass. Nothing a pass does changes the
/// structures of the chunks or loads one, so the boxes stay exact until the pass ends (then
/// dropped: no invalidation to keep up with `/place`, loads or unloads).
#[derive(Default)]
pub(crate) struct DecodedStarts {
    /// A pass meets a few starts: a list beats a map.
    known: Vec<KnownStart>,
}

/// A start met during the pass, by its chunk and structure name; `boxes` is `None` for an
/// unusable start.
struct KnownStart {
    chunk: (i32, i32),
    name: Box<str>,
    boxes: Option<StartBoxes>,
}

impl DecodedStarts {
    fn get_or_decode(&mut self, s: &Server, chunk: (i32, i32), name: &str) -> Option<&StartBoxes> {
        let i = match self.known.iter().position(|k| k.chunk == chunk && &*k.name == name) {
            Some(i) => i,
            None => {
                self.known.push(KnownStart { chunk, name: name.into(), boxes: s.decode_start(chunk, name) });
                self.known.len() - 1
            }
        };
        self.known[i].boxes.as_ref()
    }
}

/// Widest piece (in chunks per axis) listed under each column it covers; a wider (broken or
/// edited) piece is tested for every position instead, so a huge box costs no huge table.
const MAX_PIECE_COLUMNS: i32 = 32;

/// Boxes of a usable structure start.
pub(crate) struct StartBoxes {
    /// Readable piece boxes, in the start's order.
    pieces: Vec<Bb>,
    /// Box enclosing every piece (not grown).
    full: Bb,
    /// The structure adapts the terrain (its box grown by 12 for `in_structure`).
    adapts_terrain: bool,
    /// Pieces by chunk column, built on the first piece query of the pass.
    by_column: std::cell::OnceCell<PiecesByColumn>,
}

/// Indices of the pieces overlapping each chunk column, plus the too wide ones.
struct PiecesByColumn {
    columns: hashbrown::HashMap<(i32, i32), Vec<u32>>,
    wide: Vec<u32>,
}

impl StartBoxes {
    /// The enclosing box, grown by 12 when `grow`.
    fn full_box(&self, grow: bool) -> Bb {
        let mut b = self.full;
        if grow {
            for i in 0..3 {
                b[i] -= 12;
                b[i + 3] += 12;
            }
        }
        b
    }

    /// A piece contains `p` (`in_piece`): outside the enclosing box no piece can; inside, only
    /// the pieces overlapping the column of `p` are tested (5 to 20 instead of hundreds).
    fn has_piece_at(&self, p: [i32; 3]) -> bool {
        if !inside(&self.full, p) {
            return false;
        }
        let index = self.by_column.get_or_init(|| self.group_by_column());
        let hit = |i: &u32| inside(&self.pieces[*i as usize], p);
        index.columns.get(&(p[0] >> 4, p[2] >> 4)).is_some_and(|l| l.iter().any(hit)) || index.wide.iter().any(hit)
    }

    fn group_by_column(&self) -> PiecesByColumn {
        let mut index = PiecesByColumn { columns: hashbrown::HashMap::new(), wide: Vec::new() };
        for (i, b) in self.pieces.iter().enumerate() {
            let (x0, x1, z0, z1) = (b[0] >> 4, b[3] >> 4, b[2] >> 4, b[5] >> 4);
            if x1 - x0 >= MAX_PIECE_COLUMNS || z1 - z0 >= MAX_PIECE_COLUMNS {
                index.wide.push(i as u32);
                continue;
            }
            for cx in x0..=x1 {
                for cz in z0..=z1 {
                    index.columns.entry((cx, cz)).or_default().push(i as u32);
                }
            }
        }
        index
    }
}

/// A readable piece of the start contains `p`.
fn in_piece(children: &[Nbt], p: [i32; 3]) -> bool {
    children.iter().filter_map(bb_of).any(|b| inside(&b, p))
}

/// Box enclosing the readable pieces of a start, enlarged by 12
/// if the structure adapts the terrain, without copying them; `None` if there is
/// none.
pub(crate) fn full_box_of(children: &[Nbt], grow: bool) -> Option<Bb> {
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
