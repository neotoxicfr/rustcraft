//! Building golems from blocks (carved pumpkin or jack o'lantern placed): patterns searched
//! in the 24 orientations like the game (lying or upside-down golems included), pattern blocks
//! erased (event 2001), golem placed, neighbors notified; the copper block becomes a chest.

use super::super::species::Kind;
use crate::server::Server;

/// Cell of a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    /// Space: always true, state never read.
    Any,
    Air,
    Pumpkin,
    Snow,
    Iron,
    Copper,
    /// F16: wither skeleton skull (floor or wall), block of the `wither_summon_base_blocks` tag.
    WitherSkull,
    SoulBase,
}

/// Single-slice pattern (depth 1): rows from top to bottom, columns from left to right.
struct Pattern {
    rows: &'static [&'static [Cell]],
}

impl Pattern {
    fn height(&self) -> i32 {
        self.rows.len() as i32
    }

    fn width(&self) -> i32 {
        self.rows[0].len() as i32
    }
}

use Cell::{Air, Any, Copper, Iron, Pumpkin, Snow};

const SNOW_FULL: Pattern = Pattern { rows: &[&[Pumpkin], &[Snow], &[Snow]] };
const IRON_FULL: Pattern = Pattern { rows: &[&[Air, Pumpkin, Air], &[Iron, Iron, Iron], &[Air, Iron, Air]] };
const COPPER_FULL: Pattern = Pattern { rows: &[&[Pumpkin], &[Copper]] };
const SNOW_BASE: Pattern = Pattern { rows: &[&[Any], &[Snow], &[Snow]] };
const IRON_BASE: Pattern = Pattern { rows: &[&[Air, Any, Air], &[Iron, Iron, Iron], &[Air, Iron, Air]] };
const COPPER_BASE: Pattern = Pattern { rows: &[&[Any], &[Copper]] };
// F16: wither ("^^^", "###", "~#~").
const WITHER_FULL: Pattern = Pattern {
    rows: &[
        &[Cell::WitherSkull, Cell::WitherSkull, Cell::WitherSkull],
        &[Cell::SoulBase; 3],
        &[Air, Cell::SoulBase, Air],
    ],
};
const WITHER_BASE: Pattern = Pattern { rows: &[&[Any, Any, Any], &[Cell::SoulBase; 3], &[Air, Cell::SoulBase, Air]] };

/// Directions in the game's order: down, up, north, south, west, east.
const DIRS: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

/// Match found: origin and frame (forward, up), states read (the game's cache).
pub(crate) struct Match {
    origin: [i32; 3],
    forward: [i32; 3],
    up: [i32; 3],
    width: i32,
    height: i32,
    cache: Vec<([i32; 3], u32)>,
}

fn cross(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// (right, down, forward) to a world position.
fn place(origin: [i32; 3], forward: [i32; 3], up: [i32; 3], right: i32, down: i32, fwd: i32) -> [i32; 3] {
    let r = cross(forward, up);
    std::array::from_fn(|i| origin[i] + up[i] * -down + r[i] * right + forward[i] * fwd)
}

impl Match {
    pub(crate) fn block(&self, x: i32, y: i32) -> [i32; 3] {
        place(self.origin, self.forward, self.up, x, y, 0)
    }

    /// F16: "forward" frame on the X axis.
    pub(crate) fn forward_on_x(&self) -> bool {
        self.forward[0] != 0
    }

    fn state(&self, s: &Server, x: i32, y: i32) -> u32 {
        let p = self.block(x, y);
        self.cache.iter().find(|(q, _)| *q == p).map_or_else(|| s.st(p), |c| c.1)
    }
}

impl Server {
    fn cell_ok(&self, c: Cell, state: u32) -> bool {
        let n = self.state_name(state);
        match c {
            Any => true,
            Air => self.v().is_air(state),
            Pumpkin => n == "minecraft:carved_pumpkin" || n == "minecraft:jack_o_lantern",
            Snow => n == "minecraft:snow_block",
            Iron => n == "minecraft:iron_block",
            Copper => COPPER_BLOCKS.iter().any(|(b, _)| *b == n),
            Cell::WitherSkull => n == "minecraft:wither_skeleton_skull" || n == "minecraft:wither_skeleton_wall_skull",
            Cell::SoulBase => self.tag_id(crate::tag_bits::TagId::WitherSummonBaseBlocks, self.v().block(state)),
        }
    }

    /// F16: wither pattern, full (skulls) or base (dispenser).
    pub(crate) fn wither_pattern(&self, origin: [i32; 3], full: bool) -> Option<Match> {
        self.find_pattern(if full { &WITHER_FULL } else { &WITHER_BASE }, origin)
    }

    /// Air (flag 2) and event 2001 with the state read by the pattern.
    pub(crate) fn clear_pattern_blocks(&mut self, mt: &Match) {
        let air = self.default_state("minecraft:air");
        for x in 0..mt.width {
            for y in 0..mt.height {
                let p = mt.block(x, y);
                let st = mt.state(self, x, y);
                self.set_block(p, air, 2);
                self.level_event(2001, p, st as i32);
            }
        }
    }

    /// Neighbors of each cell notified (source block: air).
    pub(crate) fn update_pattern_blocks(&mut self, mt: &Match) {
        let air = self.default_state("minecraft:air");
        let air_block = self.v().block(air);
        for x in 0..mt.width {
            for y in 0..mt.height {
                let p = mt.block(x, y);
                self.update_neighbors_at(p, air_block);
            }
        }
    }

    /// Positions `origin + [0, d−1]³` (x first), then the 24 frames.
    fn find_pattern(&self, pat: &Pattern, origin: [i32; 3]) -> Option<Match> {
        let d = pat.width().max(pat.height());
        let mut cache: Vec<([i32; 3], u32)> = Vec::new();
        for z in 0..d {
            for y in 0..d {
                for x in 0..d {
                    let test = [origin[0] + x, origin[1] + y, origin[2] + z];
                    for fwd in DIRS {
                        for up in DIRS {
                            if up == fwd || up == fwd.map(|v| -v) {
                                continue;
                            }
                            if self.matches(pat, test, fwd, up, &mut cache) {
                                return Some(Match {
                                    origin: test,
                                    forward: fwd,
                                    up,
                                    width: pat.width(),
                                    height: pat.height(),
                                    cache,
                                });
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Game order: columns (x), then rows (y); a state read is cached.
    fn matches(
        &self,
        pat: &Pattern,
        o: [i32; 3],
        fwd: [i32; 3],
        up: [i32; 3],
        cache: &mut Vec<([i32; 3], u32)>,
    ) -> bool {
        for x in 0..pat.width() {
            for y in 0..pat.height() {
                let c = pat.rows[y as usize][x as usize];
                if c == Any {
                    continue;
                }
                let p = place(o, fwd, up, x, y, 0);
                let st = match cache.iter().find(|(q, _)| *q == p) {
                    Some(&(_, s)) => s,
                    None => {
                        let s = self.st(p);
                        cache.push((p, s));
                        s
                    }
                };
                if !self.cell_ok(c, st) {
                    return false;
                }
            }
        }
        true
    }

    /// Can a golem spawn from a pumpkin placed here (dispenser, the golem spawn test)?
    pub(crate) fn can_spawn_golem(&self, top: [i32; 3]) -> bool {
        self.find_pattern(&SNOW_BASE, top).is_some()
            || self.find_pattern(&IRON_BASE, top).is_some()
            || self.find_pattern(&COPPER_BASE, top).is_some()
    }

    /// Carved pumpkin or jack o'lantern placed (block different from the old one).
    pub(crate) fn golem_on_place(&mut self, state: u32, top: [i32; 3], old: u32) {
        let n = self.state_name(state);
        if n != "minecraft:carved_pumpkin" && n != "minecraft:jack_o_lantern" {
            return;
        }
        if self.v().block(old) == self.v().block(state) {
            return;
        }
        if let Some(mt) = self.find_pattern(&SNOW_FULL, top) {
            let at = mt.block(0, 2);
            self.spawn_built_golem(&mt, Kind::SnowGolem, at);
            return;
        }
        if let Some(mt) = self.find_pattern(&IRON_FULL, top) {
            let at = mt.block(1, 2);
            self.spawn_built_golem(&mt, Kind::IronGolem, at);
            return;
        }
        if let Some(mt) = self.find_pattern(&COPPER_FULL, top) {
            let at = mt.block(0, 0);
            let copper = mt.state(self, 0, 1);
            let pumpkin = mt.state(self, 0, 0);
            let id = self.spawn_built_golem(&mt, Kind::CopperGolem, at);
            self.copper_block_to_chest(&mt, copper, pumpkin);
            let weather = copper_age(self.state_name(copper));
            if let Some(i) = self.mob_index(id) {
                let mut g = self.mobs.list[i].take().expect("mob");
                g.golem.weather = weather;
                self.mob_sound(&g, "minecraft:entity.copper_golem.spawn", 1.0, 1.0);
                self.mobs.list[i] = Some(g);
            }
        }
    }

    /// Pattern erased, golem placed at the given cell (+0.05), neighbors notified.
    fn spawn_built_golem(&mut self, mt: &Match, kind: Kind, at: [i32; 3]) -> i32 {
        self.clear_pattern_blocks(mt);
        let pos = [f64::from(at[0]) + 0.5, f64::from(at[1]) + 0.05, f64::from(at[2]) + 0.5];
        let mut g = self.new_mob(kind, pos, 0.0);
        if kind == Kind::IronGolem {
            g.golem.player_created = true;
        }
        let id = g.id;
        // Players (not spectators) in the golem's box inflated by 5: `summoned_entity`.
        let b = g.bbox();
        let area = rc_physics::Aabb { min: b.min.map(|v| v - 5.0), max: b.max.map(|v| v + 5.0) };
        if self.game_mode != 3 && self.player_aabb().intersects(&area) {
            self.trig_summoned(crate::progress::pred::Who::Mob(&g));
        }
        self.add_mob(g);
        self.update_pattern_blocks(mt);
        id
    }

    /// Copper chest with the same oxidation (wax included),
    /// oriented like the pumpkin, linked to a compatible neighboring chest.
    fn copper_block_to_chest(&mut self, mt: &Match, copper: u32, pumpkin: u32) {
        let p = mt.block(0, 1);
        let name = self.state_name(copper).to_owned();
        let chest = COPPER_BLOCKS.iter().find(|(b, _)| *b == name).map_or("minecraft:copper_chest", |c| c.1);
        let v = self.v();
        let facing = v.get(pumpkin, "facing").unwrap_or("north").to_owned();
        let def = self.default_state(chest);
        let mut s = v.set(def, "facing", &facing);
        let f = v.facing(s);
        // Neighbor on the right (clockwise) then on the left, single copper chest
        // with the same orientation.
        let partner = |d: crate::blocks::Dir| -> Option<crate::blocks::Dir> {
            let q = crate::blocks::rel(p, d);
            let qs = self.st(q);
            (is_copper_chest(self.state_name(qs)) && v.get(qs, "type") == Some("single")).then(|| v.facing(qs))
        };
        if partner(f.clockwise()) == Some(f) {
            s = v.set(s, "type", "left");
        } else if partner(f.counter_clockwise()) == Some(f) {
            s = v.set(s, "type", "right");
        }
        let s = self.least_oxidized_chest(s, p);
        self.set_block(p, s, 2);
    }

    /// A double chest takes the lowest oxidation
    /// of the two halves; mixed wax → both without wax.
    pub(crate) fn least_oxidized_chest(&self, s: u32, p: [i32; 3]) -> u32 {
        let v = self.v();
        let t = v.get(s, "type").unwrap_or("single");
        if t == "single" {
            return s;
        }
        let f = v.facing(s);
        let d = if t == "left" { f.clockwise() } else { f.counter_clockwise() };
        let other = self.st(crate::blocks::rel(p, d));
        let (a, b) = (self.state_name(s).to_owned(), self.state_name(other).to_owned());
        if !is_copper_chest(&b) {
            return s;
        }
        let waxed = |n: &str| n.contains("waxed_");
        let (mut na, mut nb) = (a.clone(), b.clone());
        if waxed(&a) != waxed(&b) {
            na = a.replace("waxed_", "");
            nb = b.replace("waxed_", "");
        }
        let pick = if chest_age(&a) <= chest_age(&b) { na } else { nb };
        if pick == a {
            return s;
        }
        // Same family of properties (orientation, type, water).
        let mut out = self.default_state(&pick);
        for prop in ["facing", "type", "waterlogged"] {
            if let Some(val) = v.get(s, prop) {
                out = v.set(out, prop, val);
            }
        }
        out
    }
}

/// Blocks of the `#minecraft:copper` tag and corresponding chest.
const COPPER_BLOCKS: [(&str, &str); 8] = [
    ("minecraft:copper_block", "minecraft:copper_chest"),
    ("minecraft:exposed_copper", "minecraft:exposed_copper_chest"),
    ("minecraft:weathered_copper", "minecraft:weathered_copper_chest"),
    ("minecraft:oxidized_copper", "minecraft:oxidized_copper_chest"),
    ("minecraft:waxed_copper_block", "minecraft:waxed_copper_chest"),
    ("minecraft:waxed_exposed_copper", "minecraft:waxed_exposed_copper_chest"),
    ("minecraft:waxed_weathered_copper", "minecraft:waxed_weathered_copper_chest"),
    ("minecraft:waxed_oxidized_copper", "minecraft:waxed_oxidized_copper_chest"),
];

/// Oxidation of a copper block (wax ignored): 0 intact … 3 oxidized.
pub(crate) fn copper_age(name: &str) -> i32 {
    let n = name.trim_start_matches("minecraft:").trim_start_matches("waxed_");
    if n.starts_with("exposed") {
        1
    } else if n.starts_with("weathered") {
        2
    } else if n.starts_with("oxidized") {
        3
    } else {
        0
    }
}

fn chest_age(name: &str) -> i32 {
    copper_age(name)
}

pub(crate) fn is_copper_chest(name: &str) -> bool {
    name.ends_with("copper_chest")
}
