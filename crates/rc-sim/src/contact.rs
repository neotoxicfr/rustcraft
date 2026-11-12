//! Contacts of entities with the blocks they cross (G5-F02e) and boots enchantments tied to
//! the block stepped on.
//!
//! Blocks crossed, after the movement of the tick: the entity's box (shrunk by 1e-5) is
//! swept from its position at the start of the tick to its final position, axis by axis (Y,
//! then the larger of X and Z) for what moves with collisions, in a single straight line for
//! projectiles that take their position directly (all but the firework); at most 16
//! iterations, then only the arrival box; one effect per cell. Effects:
//! - pressure plates and tripwire: every entity (mobs, items, orbs, projectiles other than the
//!   Ender eye, falling blocks, TNT) tests the plate at 0 or the relaxed tripwire, like the
//!   player, right after its own movement; the tripwire only if the swept box intersects its
//!   shape (1 to 2.5 px if attached, 0 to 8 px otherwise);
//! - sweet berry bush (living entities other than fox and bee): slowdown 0.8 × 0.75 × 0.8 and,
//!   from age 1, 1 `sweet_berry_bush` damage if the horizontal movement of the tick exceeds
//!   0.003 on an axis;
//! - cobweb: slowdown 0.25 × 0.05 × 0.25, or 0.5 × 0.25 × 0.5 under the Weaving effect;
//! - wither rose (living entities, not on Peaceful difficulty, entity not invulnerable):
//!   Wither 40 ticks;
//! - cauldrons: in the crossed-block pass (`inside_blocks`), whose deferred effects they share.
//!
//! Boots: Frost Walker (frosted ice disc on water, immunity to hot-floor damage) and Soul
//! Speed (speed and movement efficiency on soul sand and soul soil, wear, particles and
//! sound), from the game's enchantment files.

use rc_physics::Aabb;

use crate::mobs::mob::{Ent, Mob};
use crate::mobs::species::Kind;
use crate::server::Server;

type P = [i32; 3];

const NONE: u8 = 0;
pub(crate) const PLATE: u8 = 1;
pub(crate) const TRIPWIRE: u8 = 2;
const BERRY: u8 = 3;
const WEB: u8 = 4;
const WITHER_ROSE: u8 = 5;
/// Fire, lava and wooden button (pressed by arrows): projectile effects only (living entities
/// are handled elsewhere).
pub(crate) const FIRE: u8 = 9;
pub(crate) const LAVA: u8 = 10;
pub(crate) const BUTTON: u8 = 11;

/// Contact family of a block (by name).
fn kind_of(name: &str) -> u8 {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    match n {
        "tripwire" => TRIPWIRE,
        "sweet_berry_bush" => BERRY,
        "cobweb" => WEB,
        "wither_rose" => WITHER_ROSE,
        "fire" | "soul_fire" => FIRE,
        "lava" => LAVA,
        _ if n.ends_with("pressure_plate") => PLATE,
        _ => NONE,
    }
}

/// Id of the Soul Speed modifiers (feet slot).
const SOUL_ID: &str = "minecraft:enchantment.soul_speed/feet";

/// State of the block-tied boots enchantments (player or mob).
#[derive(Debug, Clone, Default)]
pub struct Boots {
    /// Block of the last evaluation (block change = new evaluation).
    last: Option<P>,
    /// On the ground at the previous tick (a landing restarts the evaluation, like the game).
    grounded: bool,
    /// Levels read on the worn boots at the last evaluation.
    soul: i32,
    frost: i32,
    /// Soul Speed modifiers applied.
    pub(crate) soul_active: bool,
    /// Spectator player at the previous tick.
    spectator: bool,
}

/// Contact state held by the server.
#[derive(Debug, Default)]
pub struct Contact {
    /// Contact family per block (built on first use).
    table: Vec<u8>,
    /// Cells of the walk in progress (reused from one call to the next).
    pub(crate) cells: Vec<Seen>,
    /// Cells already entered during this walk (one effect per cell).
    entered: Vec<P>,
    /// Mob being ticked, taken out of the list: id, box, ignores trigger blocks (bat).
    /// Counted by the plates it presses.
    pub(crate) ticking: Option<(i32, Aabb, bool)>,
    /// Projectile being ticked (copy outside its slot): id, box at its current position;
    /// its slot, still at the old position, is not counted by the plates.
    pub(crate) ticking_proj: Option<(i32, Aabb)>,
    /// Falling block or TNT being ticked, same principle as `ticking_proj`.
    pub(crate) ticking_sim: Option<(i32, Aabb)>,
    /// Ground item being ticked out of its slot (which holds a removed placeholder): position and
    /// removal, kept up to date during its tick. A pressed plate thus counts the items already
    /// moved at their new place, the following ones at the old one, this one where it is now.
    pub(crate) ticking_item: Option<([f64; 3], bool)>,
    /// During a long run of items ticked with their list taken out of the server
    /// (`items_taken`): position and removal of each, kept up to date as the run goes on (the
    /// items born during the run are in the server's list meanwhile, counted from there).
    pub(crate) live_items: Vec<([f64; 3], bool)>,
    pub(crate) items_taken: bool,
    /// Same for the experience orbs.
    pub(crate) ticking_orb: Option<([f64; 3], bool)>,
    pub(crate) live_orbs: Vec<([f64; 3], bool)>,
    pub(crate) orbs_taken: bool,
    /// G5-F05: orbs of the list by identifier modulo 40 (the only ones that can merge with each
    /// other), rebuilt at the first merge scan of an entity walk; reused buffers.
    pub(crate) orb_buckets: Vec<Vec<u32>>,
    /// Player's boots.
    pub(crate) player: Boots,
}

/// A cell seen by the walk of a movement and the segment (start, end) that saw it: the shape
/// test follows this segment. A cell may come back (next segment, arrival box after the cap):
/// an effect that cannot be replayed harmlessly guards against the cells already entered.
pub(crate) type Seen = (P, [f64; 3], [f64; 3]);

/// Iteration cap of the walk of a movement, like the game.
const MAX_STEPS: i32 = 16;

/// Per-axis path whose three axes sum to less than 7 blocks: a one-axis segment consumes at
/// most its length + 3 iterations, so the cap is out of reach.
const SHORT: f64 = 7.0;

/// Order of the segments of a per-axis path: Y, then the larger of X and Z (X on a tie).
fn axis_order(d: [f64; 3]) -> [usize; 3] {
    if d[0].abs() < d[2].abs() { [1, 2, 0] } else { [1, 0, 2] }
}

/// Cells touched by the box (half-width `half`, height `h`, shrunk by 1e-5) moved from `from`
/// to `to`, like the game's crossed-block walk: in a straight line (`straight`, projectile that
/// takes its position without collision) or in one segment per axis (movement with
/// collisions). At most 16 iterations for the whole path; cap reached: the arrival box as
/// well. Short per-axis path: each segment covers exactly its envelope, each cell once, tested
/// over the whole path (same result, without a walk).
pub(crate) fn path_cells(from: [f64; 3], to: [f64; 3], half: f64, h: f64, straight: bool, out: &mut Vec<Seen>) {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let budget = if straight {
        MAX_STEPS - walk(from, to, half, h, MAX_STEPS, out)
    } else if d[0].abs() + d[1].abs() + d[2].abs() < SHORT {
        envelope_cells(from, to, half, h, out);
        return;
    } else {
        axis_walks(from, to, half, h, out)
    };
    if budget <= 0 {
        walk(to, to, half, h, 1, out);
    }
}

/// One-axis segments walked one by one under the common cap; returns the remaining budget.
/// Null path: the arrival box alone, like the game (straight line of zero length).
fn axis_walks(from: [f64; 3], to: [f64; 3], half: f64, h: f64, out: &mut Vec<Seen>) -> i32 {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut budget = MAX_STEPS;
    if d == [0.0; 3] {
        return budget - walk(from, to, half, h, budget, out);
    }
    let mut p = from;
    for a in axis_order(d) {
        if d[a] != 0.0 {
            let mut q = p;
            q[a] = to[a];
            budget -= walk(p, q, half, h, budget, out);
            p = q;
        }
    }
    budget
}

/// Envelopes of the one-axis segments of a short path (a one-axis segment sweeps exactly its
/// envelope), each cell once, with the whole path as the test segment.
fn envelope_cells(from: [f64; 3], to: [f64; 3], half: f64, h: f64, out: &mut Vec<Seen>) {
    let e = f64::from(1.0e-5f32);
    let push_env = |a: [f64; 3], b: [f64; 3], out: &mut Vec<Seen>| {
        let lo = |i: usize, off: f64| (a[i].min(b[i]) + off + e).floor() as i32;
        let hi = |i: usize, off: f64| (a[i].max(b[i]) + off - e).floor() as i32;
        let (x0, x1) = (lo(0, -half), hi(0, half));
        let (y0, y1) = (lo(1, 0.0), hi(1, h));
        let (z0, z1) = (lo(2, -half), hi(2, half));
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    if !out.iter().any(|s| s.0 == [x, y, z]) {
                        out.push(([x, y, z], from, to));
                    }
                }
            }
        }
    };
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut p = from;
    let mut moved = false;
    for a in axis_order(d) {
        if d[a] != 0.0 {
            let mut q = p;
            q[a] = to[a];
            push_env(p, q, out);
            p = q;
            moved = true;
        }
    }
    if !moved {
        push_env(from, from, out);
    }
}

/// Bit-grid dedupe option in the walk (`RUSTCRAFT_OPT_WALKGRID=1`): no gain measured on a
/// loaded machine (G5-F02e, docs/perf.md), kept for the quiet-machine bench.
fn walk_grid() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_WALKGRID") == Some("1"))
}

/// One game walk from `a` to `b` around the arrival box shrunk by 1e-5: each cell once in
/// this walk, stopping at the first new cell of iteration `max` or more. Returns the
/// iterations consumed (last iteration seen + 1).
fn walk(a: [f64; 3], b: [f64; 3], half: f64, h: f64, max: i32, out: &mut Vec<Seen>) -> i32 {
    let e = f64::from(1.0e-5f32);
    let at = Aabb {
        min: [b[0] - half + e, b[1] + e, b[2] - half + e],
        max: [b[0] + half - e, b[1] + h - e, b[2] + half - e],
    };
    // Cells already seen in this walk: search in the list, or (`RUSTCRAFT_OPT_WALKGRID=1`) one
    // bit per cell of the path's envelope when it fits in 256 cells.
    let (grid, lo, n) = if walk_grid() {
        let lo = [(a[0].min(b[0]) - half).floor(), a[1].min(b[1]).floor(), (a[2].min(b[2]) - half).floor()];
        let hi = [(a[0].max(b[0]) + half).floor(), (a[1].max(b[1]) + h).floor(), (a[2].max(b[2]) + half).floor()];
        let n = [hi[0] - lo[0] + 1.0, hi[1] - lo[1] + 1.0, hi[2] - lo[2] + 1.0];
        (n[0] * n[1] * n[2] <= 256.0, lo.map(|v| v as i32), n.map(|v| v as i32))
    } else {
        (false, [0; 3], [0; 3])
    };
    let slot = |p: P| -> Option<usize> {
        let r = [p[0] - lo[0], p[1] - lo[1], p[2] - lo[2]];
        (grid && (0..3).all(|i| (0..n[i]).contains(&r[i]))).then(|| ((r[0] * n[1] + r[1]) * n[2] + r[2]) as usize)
    };
    let mut bits = [0u64; 4];
    let start = out.len();
    let mut last = 0;
    // Shared walk of the crossed-block pass (game order, iteration number of each cell).
    let mut walked = crate::inside_blocks::Seen::default();
    crate::inside_blocks::each_cell_stepped(a, b, &at, &mut walked, &mut |p, it| {
        let i = slot(p);
        let seen = match i {
            Some(i) => bits[i / 64] >> (i % 64) & 1 == 1,
            None => out[start..].iter().any(|s| s.0 == p),
        };
        if seen {
            return true;
        }
        if it >= max {
            return false;
        }
        if let Some(i) = i {
            bits[i / 64] |= 1 << (i % 64);
        }
        last = it;
        out.push((p, a, b));
        true
    });
    last + 1
}

/// The box (half-width `half`, height `h`, feet at `from`) moved axis by axis up to `to`
/// intersects the shape `c`, segment by segment in the order of `path_cells` (a segment
/// follows a single axis: its envelope is the swept area), margin of 1e-7 like the game.
pub(crate) fn swept_touches(c: &Aabb, from: [f64; 3], to: [f64; 3], half: f64, h: f64) -> bool {
    let e = 1.0e-7;
    let hits = |a: [f64; 3], b: [f64; 3]| {
        let lo = [a[0].min(b[0]) - half, a[1].min(b[1]), a[2].min(b[2]) - half];
        let hi = [a[0].max(b[0]) + half, a[1].max(b[1]) + h, a[2].max(b[2]) + half];
        (0..3).all(|i| hi[i] > c.min[i] + e && lo[i] < c.max[i] - e)
    };
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut p = from;
    let mut moved = false;
    for a in axis_order(d) {
        if d[a] != 0.0 {
            let mut q = p;
            q[a] = to[a];
            if hits(p, q) {
                return true;
            }
            p = q;
            moved = true;
        }
    }
    !moved && hits(from, from)
}

/// The box moved from `a` to `b` intersects the shape `c`: in a straight line (start box, not
/// shrunk, slid along the path, like the game) or by one-axis segments.
pub(crate) fn touches(c: &Aabb, a: [f64; 3], b: [f64; 3], half: f64, h: f64, straight: bool) -> bool {
    if !straight {
        return swept_touches(c, a, b, half, h);
    }
    let start = Aabb { min: [a[0] - half, a[1], a[2] - half], max: [a[0] + half, a[1] + h, a[2] + half] };
    let travel = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    crate::mobs::nether::ghast::collided_along(&start, travel, std::slice::from_ref(c))
}

/// "Skip the empty" option for projectiles (`RUSTCRAFT_OPT_CONTACTPAL=0` disables it): no
/// walk when the palettes of the crossed sections hold no block that acts on them.
pub(crate) fn contact_pal() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_CONTACTPAL") != Some("0"))
}

/// Same palette scan for mobs (`RUSTCRAFT_OPT_CONTACTPAL_MOBS=1`).
fn contact_pal_mobs() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_CONTACTPAL_MOBS") == Some("1"))
}

/// Families that act on a mob (cauldrons: in the crossed-block pass, `inside_blocks`).
const MOB_KINDS: u32 = 1 << PLATE | 1 << TRIPWIRE | 1 << BERRY | 1 << WEB | 1 << WITHER_ROSE;

/// Cells (bounds included) of the envelope of the box (half-width `half`, height `h`) moved
/// from `from` to `to`: any walk of this path stays inside.
pub(crate) fn envelope(from: [f64; 3], to: [f64; 3], half: f64, h: f64) -> (P, P) {
    let lo = [from[0].min(to[0]) - half, from[1].min(to[1]), from[2].min(to[2]) - half];
    let hi = [from[0].max(to[0]) + half, from[1].max(to[1]) + h, from[2].max(to[2]) + half];
    (lo.map(|v| v.floor() as i32), hi.map(|v| v.floor() as i32))
}

impl Server {
    /// Contact family per block, built on first use.
    fn contact_table(&mut self) -> &[u8] {
        if self.contact.table.is_empty() {
            let kinds = &self.bs.t.kind;
            self.contact.table = (self.data.blocks.iter().enumerate())
                .map(|(i, b)| match kinds.get(i) {
                    Some(crate::blocks::Kind::Button { arrows: true, .. }) => BUTTON,
                    _ => kind_of(&b.name),
                })
                .collect();
        }
        &self.contact.table
    }

    pub(crate) fn contact_kind(&mut self, s: u32) -> u8 {
        let b = self.state_block(s);
        self.contact_table().get(usize::from(b)).copied().unwrap_or(NONE)
    }

    /// Can one of the sections covering cells `lo..=hi` contain a block of a family of `mask`
    /// (bit `1 << family`)? Read from their palettes (single value or list of states), without
    /// reading their blocks; section without palette (direct ids): maybe; absent chunk or
    /// section: no (their cells read as void air).
    pub(crate) fn contact_maybe(&mut self, lo: P, hi: P, mask: u32) -> bool {
        self.contact_table();
        let (table, states) = (&self.contact.table, &self.data.states);
        let hit = |s: u32| {
            let b = states.get(s as usize).map_or(u16::MAX, |i| i.block);
            table.get(usize::from(b)).is_some_and(|&k| mask >> k & 1 == 1)
        };
        let min_y = self.dim.height.min_y;
        let (sy0, sy1) = ((lo[1] - min_y).max(0) >> 4, (hi[1] - min_y) >> 4);
        for cx in lo[0] >> 4..=hi[0] >> 4 {
            for cz in lo[2] >> 4..=hi[2] >> 4 {
                let Some(Some(c)) = self.chunks.get(&(cx, cz)) else { continue };
                for sy in sy0..=sy1 {
                    let Some(sec) = usize::try_from(sy).ok().and_then(|i| c.sections.get(i)) else { continue };
                    let maybe = match &sec.blocks {
                        rc_world::Container::Single(v) => hit(*v),
                        rc_world::Container::Packed { palette: Some(p), .. } => p.iter().any(|&v| hit(v)),
                        rc_world::Container::Packed { palette: None, .. } => true,
                    };
                    if maybe {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Walk over the cells of the moved box (cells kept in `self.contact.cells`, to be put back
    /// after use).
    pub(crate) fn contact_cells(
        &mut self,
        from: [f64; 3],
        to: [f64; 3],
        half: f64,
        h: f64,
        straight: bool,
    ) -> Vec<Seen> {
        let mut cells = std::mem::take(&mut self.contact.cells);
        cells.clear();
        path_cells(from, to, half, h, straight, &mut cells);
        cells
    }

    /// Plates and tripwire touched by an entity that is not alive and moves with collisions
    /// (item, orb, falling block, TNT), right after its movement. Its current position must be
    /// visible to the plate count (up-to-date list, or `ticking_sim`, `ticking_item` /
    /// `live_items`, `ticking_orb` / `live_orbs`). Plate and tripwire can be replayed without
    /// effect: no guard on the cells
    /// entered.
    pub(crate) fn contact_triggers(&mut self, from: [f64; 3], to: [f64; 3], half: f64, h: f64) {
        let cells = self.contact_cells(from, to, half, h, false);
        for &(p, a, b) in &cells {
            match self.contact_kind(self.get(p)) {
                PLATE => self.plate_entity_inside(p),
                TRIPWIRE => self.tripwire_entity_inside(p, false, a, b, half, h, false),
                _ => {}
            }
        }
        self.contact.cells = cells;
    }

    /// Lava at `p` touched by the box moved from `a` to `b`: fluid box (own height, whole block
    /// under more lava), like the game.
    pub(crate) fn lava_touched(&self, p: P, a: [f64; 3], b: [f64; 3], half: f64, h: f64, straight: bool) -> bool {
        let f = self.fluid_at(p);
        if f.kind != 2 {
            return false;
        }
        let top = if self.fluid_at([p[0], p[1] + 1, p[2]]).kind == 2 { 1.0 } else { f64::from(f.own_height()) };
        let [x, y, z] = p.map(f64::from);
        let c = Aabb { min: [x, y, z], max: [x + 1.0, y + top, z + 1.0] };
        touches(&c, a, b, half, h, straight)
    }

    /// Blocks crossed by a mob during its tick (after the movement).
    pub(crate) fn mob_contacts(&mut self, m: &mut Mob) {
        if m.removed || !m.alive() || crate::mobs::illagers::no_physics(m) {
            return;
        }
        let (half, h) = (f64::from(m.width()) / 2.0, f64::from(m.height()));
        let (from, to) = (m.old, m.pos());
        if contact_pal_mobs() {
            let (lo, hi) = envelope(from, to, half, h);
            if !self.contact_maybe(lo, hi, MOB_KINDS) {
                return;
            }
        }
        let cells = self.contact_cells(from, to, half, h, false);
        let mut entered = std::mem::take(&mut self.contact.entered);
        entered.clear();
        let ignores = m.kind == Kind::Bat;
        self.contact.ticking = Some((m.id, m.bbox(), ignores));
        for &(p, a, b) in &cells {
            if !m.alive() {
                break;
            }
            let s = self.get(p);
            let k = self.contact_kind(s);
            if k == NONE || entered.contains(&p) {
                continue;
            }
            match k {
                PLATE => self.plate_entity_inside(p),
                TRIPWIRE => self.tripwire_entity_inside(p, ignores, a, b, half, h, false),
                BERRY if !matches!(m.kind, Kind::Fox | Kind::Bee) => {
                    entered.push(p);
                    m.fall_distance = 0.0;
                    m.stuck = [f64::from(0.8f32), 0.75, f64::from(0.8f32)];
                    if self.view().get_int(s, "age") != 0 && berry_moved([from[0] - to[0], from[2] - to[2]]) {
                        self.mob_hurt(m, "minecraft:sweet_berry_bush", 1.0, None, None);
                    }
                }
                WEB => {
                    m.fall_distance = 0.0;
                    let weaving = m.x.effects.iter().any(|e| e.id == "minecraft:weaving");
                    m.stuck = if weaving { [0.5, 0.25, 0.5] } else { [0.25, f64::from(0.05f32), 0.25] };
                }
                WITHER_ROSE if self.level.difficulty_id() != 0 && !m.is_invulnerable() => {
                    entered.push(p);
                    self.give_mob_effect(m, "minecraft:wither", 40, 0);
                }
                _ => {}
            }
        }
        self.contact.ticking = None;
        self.contact.cells = cells;
        self.contact.entered = entered;
    }

    /// Blocks crossed by the player during the tick (its known movement; plates and tripwire
    /// stay with the received movement, `player_touch_blocks`).
    pub(crate) fn player_contacts(&mut self) {
        if self.game_mode == 3 || self.life.vit.dead {
            return;
        }
        let (w, h, _) = self.player_dims();
        let k = self.proj_player.known_move;
        let to = self.pos;
        let from = [to[0] - k[0], to[1] - k[1], to[2] - k[2]];
        let half = w / 2.0;
        let cells = self.contact_cells(from, to, half, h, false);
        let mut entered = std::mem::take(&mut self.contact.entered);
        entered.clear();
        for &(p, _, _) in &cells {
            if self.life.vit.dead {
                break;
            }
            let s = self.get(p);
            let kind = self.contact_kind(s);
            if kind == NONE || entered.contains(&p) {
                continue;
            }
            match kind {
                BERRY => {
                    entered.push(p);
                    self.life.vit.fall = 0.0;
                    if self.view().get_int(s, "age") != 0 && berry_moved([k[0], k[2]]) {
                        self.hurt("minecraft:sweet_berry_bush", 1.0);
                    }
                }
                WEB => self.life.vit.fall = 0.0,
                WITHER_ROSE if self.level.difficulty_id() != 0 && !self.abilities().invulnerable => {
                    entered.push(p);
                    self.give_player_effect("minecraft:wither", 40, 0);
                }
                _ => {}
            }
        }
        self.contact.cells = cells;
        self.contact.entered = entered;
    }

    /// Burning entity in a water or powder snow cauldron: one level less (the snow becomes
    /// water at the same level before lowering).
    pub(crate) fn cauldron_quench(&mut self, p: P, s: u32) {
        let s = if self.view().block(s) == self.bs.t.id.powder_snow_cauldron {
            let w = self.default_state("minecraft:water_cauldron");
            self.view().set_int(w, "level", self.view().get_int(s, "level"))
        } else {
            s
        };
        self.cauldron_lower(p, s);
    }

    // --- Boots: Frost Walker, Soul Speed. ---

    /// Frost Walker immunity: `burn_from_stepping` damage that does not bypass
    /// invulnerability.
    pub(crate) fn frost_walker_immune(&self, boots: Option<&crate::player::Stack>, ty: &str) -> bool {
        boots.is_some()
            && self.dmg_tag(ty, "minecraft:burn_from_stepping")
            && !self.dmg_tag(ty, "minecraft:bypasses_invulnerability")
            && boots.is_some_and(|b| self.life.items.enchantment(b, "minecraft:frost_walker") > 0)
    }

    /// Block under the feet that governs movement (0.500001 under the feet): soul sand or soul
    /// soil.
    fn soul_block_below(&self, pos: [f64; 3], support: Option<P>) -> bool {
        let p = self.on_pos(pos, support, 0.500_001);
        self.btag_id(p, crate::tag_bits::TagId::SoulSpeedBlocks)
    }

    /// Frosted ice disc of Frost Walker (radius 3 + level − 1, capped at 16) under the entity:
    /// source water under air, with no entity in the way.
    fn frost_disk(&mut self, pos: [f64; 3], level: i32, src: Ent) {
        let dist = (3 + level - 1).clamp(0, 16);
        let c = [pos[0].floor() as i32, pos[1].floor() as i32 - 1, pos[2].floor() as i32];
        let ice = self.default_state("minecraft:frosted_ice");
        let r2 = f64::from(dist * dist);
        for z in c[2] - dist..=c[2] + dist {
            for x in c[0] - dist..=c[0] + dist {
                let p = [x, c[1], z];
                let (dx, dz) = (f64::from(x) + 0.5 - pos[0], f64::from(z) + 0.5 - pos[2]);
                if dx * dx + dz * dz >= r2 {
                    continue;
                }
                let s = self.get(p);
                if self.state_name(s) != "minecraft:water" || self.view().get_int(s, "level") != 0 {
                    continue;
                }
                if !matches!(
                    self.bname([x, c[1] + 1, z]),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                ) {
                    continue;
                }
                if !self.frost_unobstructed(p) {
                    continue;
                }
                if self.set_update(p, ice) {
                    self.game_event_block("minecraft:block_place", p, Some(src), None);
                }
            }
        }
    }

    /// No entity that gets in the way of placement (living entities other than spectators,
    /// falling blocks, TNT, minecarts) intersects the cube `p`.
    fn frost_unobstructed(&self, p: P) -> bool {
        let [x, y, z] = p.map(f64::from);
        let b = Aabb { min: [x, y, z], max: [x + 1.0, y + 1.0, z + 1.0] };
        let hit = |o: &Aabb| (0..3).all(|i| o.max[i] > b.min[i] && o.min[i] < b.max[i]);
        if self.entities_in(&b, true, None, false) > 0 {
            return false;
        }
        if self.contact.ticking.as_ref().is_some_and(|t| hit(&t.1)) {
            return false;
        }
        !self.bs.entities.list.iter().any(|e| !e.removed && hit(&Self::entity_box(e)))
            && !self.carts.list.iter().flatten().any(|c| !c.removed && hit(&c.bbox()))
    }

    /// Player's boots, at the start of its tick: Soul Speed particles and sound, then
    /// block-tied effects when it changes (or on landing, on a change of boots, on leaving
    /// spectator mode).
    pub(crate) fn player_boots_tick(&mut self) {
        let spectator = self.game_mode == 3;
        let (soul, frost) = self.slots[8].as_ref().map_or((0, 0), |b| {
            let it = &self.life.items;
            (it.enchantment(b, "minecraft:soul_speed"), it.enchantment(b, "minecraft:frost_walker"))
        });
        let mut st = std::mem::take(&mut self.contact.player);
        // No relevant boots nor effect in progress: nothing to evaluate (common case, no query).
        if soul == 0 && frost == 0 && !st.soul_active && st.soul == 0 && st.frost == 0 {
            st.last = None;
            st.grounded = self.on_ground;
            st.spectator = spectator;
            self.contact.player = st;
            return;
        }
        let flying = self.flying || self.proj_player.fall_flying;
        let support = self.player_support();
        let on_soul = self.soul_block_below(self.pos, support);
        let ground = self.on_ground;
        // Tick effects (periodic, every 5 ticks).
        if soul > 0 && !spectator && !self.life.vit.dead {
            let k = self.proj_player.known_move;
            let moving = (k[0] * 20.0).powi(2) + (k[2] * 20.0).powi(2) >= MIN_SPEED * MIN_SPEED;
            let due = !flying && ground && moving && on_soul && self.life.vit.age.is_multiple_of(5);
            if due {
                let w = self.player_dims().0;
                let rx = self.life.rng.next_double();
                let rz = self.life.rng.next_double();
                let at = [self.pos[0] + (rx - 0.5) * w, self.pos[1] + 0.1, self.pos[2] + (rz - 0.5) * w];
                self.particles("minecraft:soul", at, [(k[0] * -0.2) as f32, 0.1, (k[2] * -0.2) as f32], 1.0, 0);
            }
            let chance = self.life.rng.next_float() < 0.35;
            if chance && due {
                let pitch = 0.6 + self.life.rng.next_float() * 0.4;
                self.play_sound("minecraft:particle.soul_escape", 7, self.pos, 0.6, pitch);
            }
        }
        // Block-tied evaluation.
        let pos = self.pos.map(|v| v.floor() as i32);
        let changed_boots = soul != st.soul || frost != st.frost;
        if changed_boots && st.soul_active {
            self.player_soul_mods(false, st.soul);
            st.soul_active = false;
        }
        if spectator {
            if st.soul_active {
                self.player_soul_mods(false, st.soul);
                st.soul_active = false;
            }
        } else if !self.life.vit.dead {
            let landed = ground && !st.grounded;
            if st.last != Some(pos) || landed || changed_boots || st.spectator {
                let vehicle = self.player_riding_any();
                if frost > 0 && ground && !vehicle {
                    self.frost_disk(self.pos, frost, Ent::Player);
                }
                if soul > 0 {
                    let want = soul_wanted(st.soul_active, vehicle, flying, on_soul, ground);
                    if want != st.soul_active {
                        self.player_soul_mods(want, soul);
                        st.soul_active = want;
                    }
                    let wear = self.life.rng.next_float() < 0.04 * soul as f32;
                    if wear && ground && on_soul {
                        self.hurt_item(8, 1);
                    }
                }
            }
        }
        st.last = Some(pos);
        st.grounded = ground;
        st.soul = soul;
        st.frost = frost;
        st.spectator = spectator;
        self.contact.player = st;
    }

    /// Soul Speed modifiers of the player applied or removed.
    fn player_soul_mods(&mut self, on: bool, level: i32) {
        let mods = &mut self.life.attrs.mods;
        mods.retain(|m| m.id != SOUL_ID);
        if on {
            for (attr, amount) in
                [("minecraft:movement_speed", soul_speed_bonus(level)), ("minecraft:movement_efficiency", 1.0)]
            {
                mods.push(crate::attrs::EquipMod { attr: attr.to_owned(), id: SOUL_ID.to_owned(), amount, op: 0 });
            }
        }
    }

    /// Tick effects of a mob's boots (start of the base tick).
    pub(crate) fn mob_boots_tick_effects(&mut self, m: &mut Mob) {
        let Some(b) = m.equipment[2].as_ref() else { return };
        let soul = self.life.items.enchantment(b, "minecraft:soul_speed");
        if soul <= 0 {
            return;
        }
        let v = m.vel;
        let moving = (v[0] * 20.0).powi(2) + (v[2] * 20.0).powi(2) >= MIN_SPEED * MIN_SPEED;
        let due = m.on_ground && moving && m.tick_count % 5 == 0 && self.soul_block_below(m.pos(), m.support);
        if due {
            let w = f64::from(m.width());
            let rx = m.rng.next_double();
            let rz = m.rng.next_double();
            let at = [m.pos()[0] + (rx - 0.5) * w, m.pos()[1] + 0.1, m.pos()[2] + (rz - 0.5) * w];
            self.particles("minecraft:soul", at, [(v[0] * -0.2) as f32, 0.1, (v[2] * -0.2) as f32], 1.0, 0);
        }
        let chance = self.life.rng.next_float() < 0.35;
        if chance && due && !m.x.silent {
            let pitch = 0.6 + m.rng.next_float() * 0.4;
            self.mob_sound(m, "minecraft:particle.soul_escape", 0.6, pitch);
        }
    }

    /// Block-tied effects of a mob's boots (after the air supply of the base tick).
    pub(crate) fn mob_boots_location(&mut self, m: &mut Mob) {
        let (soul, frost) = m.equipment[2].as_ref().map_or((0, 0), |b| {
            let it = &self.life.items;
            (it.enchantment(b, "minecraft:soul_speed"), it.enchantment(b, "minecraft:frost_walker"))
        });
        let st = m.boots.clone();
        if soul == 0 && frost == 0 && !st.soul_active && st.soul == 0 && st.frost == 0 {
            m.boots.last = None;
            m.boots.grounded = m.on_ground;
            return;
        }
        let pos = m.block_pos();
        let changed_boots = soul != st.soul || frost != st.frost;
        let mut active = st.soul_active;
        if changed_boots && active {
            m.attrs.speed.remove_mod(SOUL_ID);
            active = false;
        }
        let landed = m.on_ground && !st.grounded;
        if m.alive() && (st.last != Some(pos) || landed || changed_boots) {
            let vehicle = m.mount.vehicle.is_some();
            if frost > 0 && m.on_ground && !vehicle {
                self.frost_disk(m.pos(), frost, Ent::Mob(m.id));
            }
            if soul > 0 {
                // Mob gliding is not simulated: never "flying".
                let flying = false;
                let on_soul = self.soul_block_below(m.pos(), m.support);
                let want = soul_wanted(active, vehicle, flying, on_soul, m.on_ground);
                if want && !active {
                    m.attrs.speed.set_mod(SOUL_ID, soul_speed_bonus(soul), 0);
                } else if !want && active {
                    m.attrs.speed.remove_mod(SOUL_ID);
                }
                active = want;
                let wear = self.life.rng.next_float() < 0.04 * soul as f32;
                if wear && m.on_ground && on_soul {
                    self.mob_hurt_boots(m, 1);
                }
            }
        }
        m.boots.soul_active = active;
        m.boots.last = Some(pos);
        m.boots.grounded = m.on_ground;
        m.boots.soul = soul;
        m.boots.frost = frost;
    }

    /// Wear of a mob's boots (Unbreaking by the item's kind); broken: removed, event 52.
    fn mob_hurt_boots(&mut self, m: &mut Mob, n: i32) {
        self.mob_hurt_slot(m, 2, n);
    }
}

/// Minimum speed (blocks per second) of the Soul Speed tick effects.
const MIN_SPEED: f64 = 9.999_999_747_378_752e-6;

/// Soul Speed speed bonus: 0.0405 + 0.0105 per level beyond the first (computed in single
/// precision, like the game's per-level values).
fn soul_speed_bonus(level: i32) -> f64 {
    f64::from(0.0405f32 + 0.0105f32 * (level - 1) as f32)
}

/// Condition of the Soul Speed modifiers: never in a vehicle or flying; once active, they
/// stay as long as the entity is on a soul block or in the air; inactive, they are applied on
/// a soul block.
fn soul_wanted(active: bool, vehicle: bool, flying: bool, on_soul: bool, ground: bool) -> bool {
    !vehicle && !flying && (on_soul || (active && !ground))
}

/// Horizontal movement of the tick large enough for the berry bush to sting (0.003 on an
/// axis, single precision value).
fn berry_moved(d: [f64; 2]) -> bool {
    let t = f64::from(0.003f32);
    (d[0] * d[0] + d[1] * d[1]) > 0.0 && (d[0].abs() >= t || d[1].abs() >= t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Short per-axis path (sum of the axes under 7): the envelopes give exactly the cells
    /// of the segments walked one by one, without reaching the cap, and the same shape verdict
    /// for any box in each cell. Positions on the 1/16 grid every other time (entities at rest,
    /// on the ground), one axis still one time in three.
    #[test]
    fn envelopes_equal_walk_for_short_path() {
        let mut r = crate::mobs::jrand::JRand::new(crate::rng::test_seed());
        let (mut fast, mut exact) = (Vec::new(), Vec::new());
        let sizes = [(0.125, 0.25), (0.25, 0.5), (0.3, 1.8), (0.49, 0.98), (0.7, 1.95), (1.0, 2.9)];
        for i in 0..20_000 {
            let snap = |v: f64| if i % 2 == 0 { (v * 16.0).round() / 16.0 } else { v };
            let from: [f64; 3] = std::array::from_fn(|_| snap(r.next_double() * 8.0 - 4.0));
            let still = i % 3;
            let d: [f64; 3] = std::array::from_fn(|a| if a == still { 0.0 } else { snap(r.next_double() * 6.8 - 3.4) });
            let to = [from[0] + d[0], from[1] + d[1], from[2] + d[2]];
            let (half, h) = sizes[i % sizes.len()];
            fast.clear();
            exact.clear();
            envelope_cells(from, to, half, h, &mut fast);
            assert!(axis_walks(from, to, half, h, &mut exact) > 0, "cap reached: {from:?} -> {to:?}");
            let set = |v: &Vec<Seen>| {
                let mut c: Vec<P> = v.iter().map(|s| s.0).collect();
                c.sort_unstable();
                c.dedup();
                c
            };
            let cells = set(&fast);
            assert_eq!(cells, set(&exact), "{from:?} -> {to:?}, half-width {half}, height {h}");
            for &p in &cells {
                let [x, y, z] = p.map(f64::from);
                let (a, b) = (r.next_double(), r.next_double());
                let c =
                    Aabb { min: [x, y + a * 0.5, z + b * 0.5], max: [x + 1.0, y + a * 0.5 + 0.1, z + 0.5 + b * 0.5] };
                let quick = swept_touches(&c, from, to, half, h);
                let slow = exact.iter().filter(|s| s.0 == p).any(|s| swept_touches(&c, s.1, s.2, half, h));
                assert_eq!(quick, slow, "shape {c:?}, {from:?} -> {to:?}, half-width {half}, height {h}");
            }
        }
    }
}
