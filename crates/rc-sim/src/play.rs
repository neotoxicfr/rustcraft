//! The player in the world (`integrated-server.md` § 4, § 6): movements validated by
//! collision replay, breaking and placing with sequence acknowledgements, block changes
//! broadcast at the end of the tick, light recomputed by the shared engine (`rc-light`).

use hashbrown::HashMap;
use std::collections::BTreeMap;

use rc_client::Event;
use rc_physics::Aabb;
use rc_physics::collide;
use rc_proto::BlockPos;
use rc_world::{LightLayer, Section, block_index};

use crate::chunk::Chunk;
use crate::server::Server;

/// "Hat" bit of the skin parts.
pub(crate) const HAT: u8 = 0x40;

/// Face directions (game order): down, up, north, south, west, east.
pub const FACES: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

/// Server-side mining (§ 6.1), state kept from one action to the next like the game: the hit
/// cell and its start tick remain known after the end of the hit.
#[derive(Debug, Clone, Copy)]
pub struct Mining {
    /// Hit in progress (mining events every tick).
    pub active: bool,
    pub pos: [i32; 3],
    /// Player's own tick at the start of the hit.
    pub start: i64,
    /// Face hit (data of events 2019/2020), changed by action 1.
    pub face: u8,
    /// End received too early: block and start tick; the server breaks on its own.
    pub delayed: Option<([i32; 3], i64)>,
    /// Last crack stage broadcast to the other players (-1: none).
    pub last_stage: i32,
}

impl Default for Mining {
    fn default() -> Self {
        Mining { active: false, pos: [0; 3], start: 0, face: 0, delayed: None, last_stage: -1 }
    }
}

/// Server world as seen by physics and light.
pub(crate) struct SimWorld<'a> {
    pub chunks: &'a HashMap<(i32, i32), Option<Box<Chunk>>>,
    pub min_y: i32,
    pub height: i32,
    pub t: &'a rc_physics::Tables,
    /// Moving piston blocks and direction of the push in progress (ignored by them).
    pub movers: &'a HashMap<[i32; 3], crate::blocks::level::Mover>,
    pub noclip: Option<u8>,
}

impl SimWorld<'_> {
    /// Shape of a moving piston block at `p`; false if it is not one.
    fn piston_shape(&self, s: u32, p: [i32; 3], out: &mut Vec<Aabb>) -> bool {
        if !self.t.is_moving_piston(s) {
            return false;
        }
        if let Some(m) = self.movers.get(&p) {
            self.t.moving_collision(&m.physics(), self.noclip, out);
        }
        true
    }
}

pub(crate) fn block_at(chunks: &HashMap<(i32, i32), Option<Box<Chunk>>>, min_y: i32, p: [i32; 3]) -> Option<u32> {
    let c = chunks.get(&(p[0] >> 4, p[2] >> 4))?.as_ref()?;
    let ly = p[1] - min_y;
    let s = c.sections.get(usize::try_from(ly >> 4).ok()?)?;
    Some(s.blocks.get(block_index((p[0] & 15) as usize, (ly & 15) as usize, (p[2] & 15) as usize)))
}

impl rc_physics::World for SimWorld<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if !(self.min_y..self.min_y + self.height).contains(&y) {
            return Some(0);
        }
        block_at(self.chunks, self.min_y, [x, y, z])
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        match self.state(x, y, z) {
            Some(s) => (!self.movers.is_empty() && self.piston_shape(s, [x, y, z], out)) || self.t.collision(s, out),
            None => false,
        }
    }

    fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        if self.movers.is_empty() {
            return false;
        }
        match self.state(x, y, z) {
            Some(s) => self.piston_shape(s, [x, y, z], out),
            None => false,
        }
    }
}

/// Server chunks as seen by the light engine.
/// Switches of the server's relight (mission C2-M04, `docs/perf.md`), read once at
/// startup (`=0`: old path) and kept per server (the tests compare the two paths
/// in a single process).
/// - `sky_list` (`RUSTCRAFT_OPT_SKYBIT`, shared with `rc-light`): changes that touch
///   the sky (attenuation or shape) are picked up at write time and passed separately: a lamp
///   that turns on does not restart the sky resolution.
/// - `sections` (`RUSTCRAFT_OPT_LIGHTSECTIONS`): light update limited to the sections
///   the game signals (changed and edge neighbors, created layers), sky and block separate;
///   null layer declared in the empty mask (also in the chunk packet).
/// - `misc` (`RUSTCRAFT_OPT_SIMLIGHTMISC`): dimension type read once, implicit layers
///   searched once per (column, section), consecutive duplicate of a position discarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LightOpts {
    pub sky_list: bool,
    pub sections: bool,
    pub misc: bool,
}

impl LightOpts {
    #[cfg(test)]
    pub(crate) const OLD: LightOpts = LightOpts { sky_list: false, sections: false, misc: false };
    #[cfg(test)]
    pub(crate) const NEW: LightOpts = LightOpts { sky_list: true, sections: true, misc: true };

    pub(crate) fn current() -> LightOpts {
        static O: std::sync::OnceLock<LightOpts> = std::sync::OnceLock::new();
        *O.get_or_init(|| LightOpts {
            sky_list: rc_data::env_str!("RUSTCRAFT_OPT_SKYBIT").map(str::trim) != Some("0"),
            sections: rc_data::env_str!("RUSTCRAFT_OPT_LIGHTSECTIONS").map(str::trim) != Some("0"),
            misc: rc_data::env_str!("RUSTCRAFT_OPT_SIMLIGHTMISC").map(str::trim) != Some("0"),
        })
    }
}

#[cfg(test)]
thread_local! {
    /// Sabotages of the C2-M04 proofs (tests only): 1 edge neighbors omitted, 2 a changed
    /// section omitted, 3 empty mask never set, 4 created layers not signaled.
    pub(crate) static LIGHT_SABOTAGE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn light_sabotage(n: u8) -> bool {
    LIGHT_SABOTAGE.with(|s| s.get() == n)
}

#[cfg(not(test))]
#[inline(always)]
fn light_sabotage(_: u8) -> bool {
    false
}

/// Sabotage 3 (empty mask never set), read by `server::light_data`.
#[inline(always)]
pub(crate) fn light_sabotage_empty() -> bool {
    light_sabotage(3)
}

/// Filters of a per-column light update: (sky, block), one bit per layer position.
type LightFilters = BTreeMap<(i32, i32), (Vec<u64>, Vec<u64>)>;

/// Marks layer position `i` (0: under the world) of the column `(x, z)`.
fn mark_section(f: &mut LightFilters, n: usize, x: i32, z: i32, i: i32, sky: bool) {
    let Some(i) = usize::try_from(i).ok().filter(|&i| i < n) else { return };
    let e = f.entry((x, z)).or_insert_with(|| (vec![0; n.div_ceil(64)], vec![0; n.div_ceil(64)]));
    let m = if sky { &mut e.0 } else { &mut e.1 };
    m[i / 64] |= 1 << (i % 64);
}

/// Sections signaled like the game from the changed sections: the section of a written
/// cell and those of its neighbors at ±1 (only at the section's edge: edge bits; the
/// bits being accumulated per section, all combinations of set axes are taken).
fn mark_touched(f: &mut LightFilters, n: usize, t: &rc_light::Touched, sky: bool) {
    use rc_world::edge;
    let span = |b: u8, lo: u8| -> std::ops::RangeInclusive<i32> {
        if light_sabotage(1) {
            return 0..=0;
        }
        i32::from(b & lo == 0) - 1..=i32::from(b & (lo << 1) != 0)
    };
    for (&(x, s, z), &b) in t {
        if light_sabotage(2) && (x, s, z) == t.keys().min().copied().unwrap_or_default() {
            continue;
        }
        for dx in span(b, edge::X0) {
            for dy in span(b, edge::Y0) {
                for dz in span(b, edge::Z0) {
                    mark_section(f, n, x + dx, z + dz, s + 1 + dy, sky);
                }
            }
        }
    }
}

struct LightChunks<'a>(&'a mut HashMap<(i32, i32), Option<Box<Chunk>>>);

impl rc_light::LightGrid for LightChunks<'_> {
    fn column(&self, cx: i32, cz: i32) -> Option<(&[Section], &[LightLayer], &[LightLayer])> {
        let c = self.0.get(&(cx, cz))?.as_ref()?;
        Some((&c.sections, &c.sky, &c.block))
    }

    fn layers_mut(&mut self, cx: i32, cz: i32, sky: bool) -> Option<&mut [LightLayer]> {
        let c = self.0.get_mut(&(cx, cz))?.as_mut()?;
        Some(if sky { &mut c.sky } else { &mut c.block })
    }
}

/// Trace of the light recomputations (`RUSTCRAFT_LIGHTTRACE=<file>`, read once) for the
/// `rc-light` benches (format: `crates/rc-light/benches/common/mod.rs`). Before each
/// recomputation: the columns it may read, the first time they are used (blocks and
/// layers as the engine sees them), then the positions and their states. A single
/// dimension per trace (`RUSTCRAFT_LIGHTTRACE_DIM`, otherwise the first seen). Without the variable:
/// a boolean test, nothing else.
fn light_trace(
    chunks: &HashMap<(i32, i32), Option<Box<Chunk>>>,
    dim: &str,
    height: i32,
    has_sky: bool,
    direct_bits: u8,
    tick: u64,
    changes: &[[i32; 3]],
) {
    use std::io::Write;
    struct Trace {
        out: std::io::BufWriter<std::fs::File>,
        dim: Option<String>,
        dumped: hashbrown::HashSet<(i32, i32)>,
        started: bool,
    }
    static TRACE: std::sync::OnceLock<Option<std::sync::Mutex<Trace>>> = std::sync::OnceLock::new();
    let Some(m) = TRACE.get_or_init(|| {
        let path = rc_data::env_os!("RUSTCRAFT_LIGHTTRACE")?;
        let file = std::fs::File::create(path).ok()?;
        // `RUSTCRAFT_LIGHTTRACE_DIM`: recorded dimension (otherwise the first seen).
        let dim = rc_data::env_str!("RUSTCRAFT_LIGHTTRACE_DIM").map(str::to_owned);
        Some(std::sync::Mutex::new(Trace { out: std::io::BufWriter::new(file), dim, dumped: Default::default(), started: false }))
    }) else {
        return;
    };
    let Ok(mut tr) = m.lock() else { return };
    let tr = &mut *tr;
    match &tr.dim {
        Some(d) if d != dim => return,
        Some(_) if tr.started => {}
        _ => {
            tr.started = true;
            let mut h = Vec::new();
            h.extend_from_slice(b"RCLTRACE");
            h.extend_from_slice(&1u32.to_le_bytes());
            h.extend_from_slice(&(height as u32).to_le_bytes());
            h.push(u8::from(has_sky));
            h.push(direct_bits);
            h.extend_from_slice(&(dim.len() as u16).to_le_bytes());
            h.extend_from_slice(dim.as_bytes());
            let _ = tr.out.write_all(&h);
            tr.dim = Some(dim.to_string());
        }
    }
    let mut buf = Vec::new();
    let mut values = [0u32; rc_world::SECTION_BLOCKS];
    for k in trace_columns(changes) {
        let Some(Some(c)) = chunks.get(&k) else { continue };
        if !tr.dumped.insert(k) {
            continue;
        }
        buf.push(1u8);
        buf.extend_from_slice(&k.0.to_le_bytes());
        buf.extend_from_slice(&k.1.to_le_bytes());
        buf.extend_from_slice(&(c.sections.len() as u16).to_le_bytes());
        for s in &c.sections {
            s.blocks.unpack_into(&mut values);
            values.iter().for_each(|v| buf.extend_from_slice(&v.to_le_bytes()));
        }
        for l in c.sky.iter().chain(&c.block) {
            match l {
                Some(a) => {
                    buf.push(1);
                    buf.extend_from_slice(&a[..]);
                }
                None => buf.push(0),
            }
        }
    }
    buf.push(2u8);
    buf.extend_from_slice(&tick.to_le_bytes());
    buf.extend_from_slice(&(changes.len() as u32).to_le_bytes());
    for p in changes {
        let state = chunks.get(&(p[0] >> 4, p[2] >> 4)).and_then(|c| c.as_ref()).map_or(0, |c| {
            c.sections.get((p[1] >> 4) as usize).map_or(0, |s| {
                s.blocks.get(block_index((p[0] & 15) as usize, (p[1] & 15) as usize, (p[2] & 15) as usize))
            })
        });
        for v in [p[0], p[1], p[2]] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&state.to_le_bytes());
    }
    let _ = tr.out.write_all(&buf);
    let _ = tr.out.flush();
}

/// Columns a recomputation may read: the box of each group of changes (same
/// grouping as the engine) at ±16 cells, like the zone that resolves it.
fn trace_columns(changes: &[[i32; 3]]) -> Vec<(i32, i32)> {
    let mut want: Vec<(i32, i32)> = Vec::new();
    for (lo, hi, _) in rc_light::change_groups(changes) {
        for cx in (lo[0] - 16) >> 4..=(hi[0] + 16) >> 4 {
            for cz in (lo[2] - 16) >> 4..=(hi[2] + 16) >> 4 {
                if !want.contains(&(cx, cz)) {
                    want.push((cx, cz));
                }
            }
        }
    }
    want
}

/// Missing sky layer: the one the game reads in its place (lowest plane of the first
/// stored layer above, otherwise 15 everywhere).
fn implied_sky(layers: &[LightLayer], i: usize) -> [u8; rc_world::LIGHT_BYTES] {
    match layers[i + 1..].iter().flatten().next() {
        None => [0xff; rc_world::LIGHT_BYTES],
        Some(above) => {
            let mut out = [0u8; rc_world::LIGHT_BYTES];
            for y in 0..16 {
                out[y * 128..y * 128 + 128].copy_from_slice(&above[..128]);
            }
            out
        }
    }
}

/// Block-entity data copied onto the item picked with Ctrl (creative): own fields
/// (without id, position, components nor those carried by the collected components) in
/// `block_entity_data` with the type's id, then the collected components.
fn block_data_on_item(stack: &mut crate::player::Stack, be: &rc_proto::nbt::Nbt) {
    use rc_proto::nbt::Nbt;
    let collected = crate::loot::be::collect(be);
    // Fields taken over by a collected component, specific to the block-entity type.
    let carried = crate::loot::be::carried(be.get("id").and_then(Nbt::as_str).unwrap_or(""));
    let mut comps = match stack.components.take() {
        Some(Nbt::Compound(c)) => c,
        _ => Vec::new(),
    };
    if let Nbt::Compound(fields) = be {
        let data: Vec<(String, Nbt)> = fields
            .iter()
            .filter(|(k, _)| {
                !matches!(k.as_str(), "id" | "x" | "y" | "z" | "components") && !carried.contains(&k.as_str())
            })
            .cloned()
            .collect();
        comps.retain(|(k, _)| k != "minecraft:block_entity_data");
        if !data.is_empty() {
            let mut d = vec![("id".to_owned(), be.get("id").cloned().unwrap_or(Nbt::String(String::new())))];
            d.extend(data);
            comps.push(("minecraft:block_entity_data".into(), Nbt::Compound(d)));
        }
    }
    for (k, v) in collected {
        comps.retain(|(x, _)| *x != k);
        comps.push((k, v));
    }
    stack.components = (!comps.is_empty()).then_some(Nbt::Compound(comps));
}

pub(crate) fn player_box(pos: [f64; 3], sneaking: bool) -> Aabb {
    let h = if sneaking { 1.5 } else { 1.8 };
    Aabb { min: [pos[0] - 0.3, pos[1], pos[2] - 0.3], max: [pos[0] + 0.3, pos[1] + h, pos[2] + 0.3] }
}

fn overlaps(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| a.max[i] > b.min[i] && a.min[i] < b.max[i])
}

impl Server {
    pub(crate) fn world(&self) -> SimWorld<'_> {
        SimWorld {
            chunks: &self.chunks,
            min_y: self.dim.height.min_y,
            height: self.dim.height.sections as i32 * 16,
            t: &self.phys,
            movers: &self.bs.movers,
            noclip: self.bs.noclip,
        }
    }

    pub(crate) fn state_at(&self, p: [i32; 3]) -> Option<u32> {
        block_at(&self.chunks, self.dim.height.min_y, p)
    }

    /// Chunks around a block loaded (actions received before the first chunk send).
    fn ensure_around(&mut self, p: [i32; 3]) {
        let (cx, cz) = (p[0] >> 4, p[2] >> 4);
        let near: Vec<(i32, i32)> = (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| (cx + dx, cz + dz))).collect();
        self.ensure_loaded(&near);
    }

    fn in_height(&self, y: i32) -> bool {
        let h = self.dim.height;
        (h.min_y..h.min_y + h.sections as i32 * 16).contains(&y)
    }

    /// `move_player_*` (§ 4.2).
    pub(crate) fn on_move(&mut self, pos: Option<[f64; 3]>, rot: Option<(f32, f32)>, on_ground: bool) {
        if let Some((y, p)) = rot
            && y.is_finite()
            && p.is_finite()
        {
            self.yaw = y.rem_euclid(360.0);
            if self.yaw > 180.0 {
                self.yaw -= 360.0;
            }
            self.pitch = p.clamp(-90.0, 90.0);
        }
        let Some(target) = pos else {
            self.on_ground = on_ground;
            // Rotation only: counted by the guests' speed check.
            if self.teleport.is_none() && self.lan_move_too_fast(None) {
                self.teleport_to(self.pos);
            }
            return;
        };
        if target.iter().any(|v| !v.is_finite()) || !self.player_ready() {
            return;
        }
        // Pending teleport: only the rotation counts (§ 4.1).
        if self.teleport.is_some() {
            return;
        }
        let target = [target[0].clamp(-3.0e7, 3.0e7), target[1].clamp(-2.0e7, 2.0e7), target[2].clamp(-3.0e7, 3.0e7)];
        // Guest too fast: sent back to their position (the world owner is not checked).
        if self.lan_move_too_fast(Some(target)) {
            eprintln!("lan: {} too fast, reset", self.name);
            self.teleport_to(self.pos);
            return;
        }
        // Spectator: no collision, always accepted.
        if self.game_mode == 3 {
            self.accept_move(target, on_ground);
            return;
        }
        // Difference after collision (speed check: guests only, above).
        let start = self.last_good;
        let around = |p: [f64; 3]| {
            let (cx, cz) = ((p[0].floor() as i32) >> 4, (p[2].floor() as i32) >> 4);
            [
                (cx - 1, cz - 1),
                (cx - 1, cz),
                (cx - 1, cz + 1),
                (cx, cz - 1),
                (cx, cz),
                (cx, cz + 1),
                (cx + 1, cz - 1),
                (cx + 1, cz),
                (cx + 1, cz + 1),
            ]
        };
        let mut need: Vec<(i32, i32)> = around(start).to_vec();
        need.extend(around(target));
        self.ensure_loaded(&need);
        let d = [target[0] - start[0], target[1] - start[1], target[2] - start[2]];
        let old_box = player_box(start, self.sneaking);
        let w = self.world();
        let m = collide::resolve(&w, &old_box, d, self.on_ground, 0.6);
        let reached = [start[0] + m[0], start[1] + m[1], start[2] + m[2]];
        // Vertical difference always ignored (§ 4.2.8).
        let (ex, ez) = (target[0] - reached[0], target[2] - reached[2]);
        let fail = ex * ex + ez * ez > 0.0625 && self.game_mode != 1;
        let was_colliding = !collide::is_free(&w, &old_box.deflate(1.0e-5));
        let new_box = player_box(target, self.sneaking).deflate(1.0e-5);
        let mut boxes = Vec::new();
        collide::collect(&w, &new_box, &mut boxes);
        let old = old_box.deflate(1.0e-5);
        let new_contact = boxes.iter().any(|s| overlaps(s, &new_box) && !overlaps(s, &old));
        if rc_data::env_flag!("RUSTCRAFT_SOLOLOG") {
            let below = [target[0].floor() as i32, target[1].floor() as i32, target[2].floor() as i32];
            eprintln!(
                "singleplayer: movement {start:?} → {target:?}, fail {fail}, collision {was_colliding}, new contact {new_contact}, {} boxes, state {:?} loaded {:?}",
                boxes.len(),
                self.state_at(below),
                self.chunks.get(&(below[0] >> 4, below[2] >> 4)).map(Option::is_some)
            );
        }
        if (!fail || was_colliding) && !new_contact {
            self.accept_move(target, on_ground);
        } else {
            eprintln!(
                "singleplayer: movement refused ({:.3}, {:.3}, {:.3}) → ({:.3}, {:.3}, {:.3}), reframing",
                start[0], start[1], start[2], target[0], target[1], target[2]
            );
            self.teleport_to(start);
        }
    }

    /// Player raised by `dy`: fully relative teleport (position,
    /// rotation, momentum kept), the expected position is the absolute one.
    pub(crate) fn teleport_up(&mut self, dy: f64) {
        use rc_proto_v777::play::relative as r;
        self.teleport_id = if self.teleport_id == i32::MAX { 0 } else { self.teleport_id + 1 };
        self.pos[1] += dy;
        self.teleport = Some((self.teleport_id, self.pos, self.tick));
        let all = r::X | r::Y | r::Z | r::YAW | r::PITCH | r::DELTA_X | r::DELTA_Y | r::DELTA_Z | r::ROTATE_DELTA;
        let _ = self.send(Event::Teleport(rc_proto_v777::play::PlayerPosition {
            id: self.teleport_id,
            pos: [0.0, dy, 0.0],
            delta: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            relative: all,
        }));
    }

    fn accept_move(&mut self, target: [f64; 3], on_ground: bool) {
        self.pos = target;
        self.last_good = target;
        self.on_ground = on_ground;
        self.player_touch_blocks();
    }

    /// Interaction reach: distance from the eyes to the block's box < reach + 1.
    fn in_reach(&self, p: [i32; 3]) -> bool {
        // W8a: reach read from the attribute (creative mode, `/attribute`, equipment).
        self.within_block_range(p, 1.0)
    }

    pub(crate) fn send_block(&self, p: [i32; 3]) {
        if let Some(state) = self.state_at(p) {
            let _ = self.send(Event::BlockUpdate { pos: BlockPos { x: p[0], y: p[1], z: p[2] }, state });
        }
    }

    /// Stack held in the main hand.
    fn held_stack(&self) -> Option<&crate::player::Stack> {
        self.slots.get(36 + self.held as usize)?.as_ref()
    }

    fn item_index(&self, id: &str) -> Option<usize> {
        self.data.registry("minecraft:item")?.entries.iter().position(|e| e == id)
    }

    fn tag_has(&self, set: &rc_data::BlockSet, block: u32) -> bool {
        match set {
            rc_data::BlockSet::Tag(t) => self.block_tags.get(t).is_some_and(|v| v.binary_search(&block).is_ok()),
            rc_data::BlockSet::Blocks(b) => b.contains(&block),
        }
    }

    /// Mining gain per tick (`server-gameplay.md` § 3.2, like the client prediction); effects
    /// and attributes not handled at milestone 1.
    fn mine_gain(&self, state: u32) -> f32 {
        let hardness = self.data.hardness.get(state as usize).copied().unwrap_or(0.0);
        if hardness < 0.0 {
            return 0.0;
        }
        let block = self.data.state(state).map_or(u32::MAX, |s| u32::from(s.block));
        let tool = self
            .held_stack()
            .and_then(|s| self.item_index(&s.id))
            .and_then(|i| self.data.items.get(i))
            .and_then(|i| i.tool.as_ref());
        let (mut v, correct) = match tool {
            None => (1.0, false),
            Some(t) => (
                t.rules
                    .iter()
                    .find(|r| r.speed.is_some() && self.tag_has(&r.blocks, block))
                    .and_then(|r| r.speed)
                    .unwrap_or(t.default_speed),
                t.rules
                    .iter()
                    .find(|r| r.correct.is_some() && self.tag_has(&r.blocks, block))
                    .and_then(|r| r.correct)
                    .unwrap_or(false),
            ),
        };
        v = self.dig_speed(v, self.eye_in_water()); // S7: efficiency, haste, fatigue, water
        if !self.on_ground {
            v /= 5.0;
        }
        let needs = self.data.needs_tool.get(state as usize).copied().unwrap_or(false);
        v / hardness / if !needs || correct { 30.0 } else { 100.0 }
    }

    /// Crack of a block hit by the current player, sent to the OTHER nearby players
    /// (the hitter draws their own); -1 clears it.
    fn destroy_progress(&self, pos: [i32; 3], stage: i32) {
        let e = Event::BlockDestruction { id: self.player_id, pos, stage: stage as u8 };
        self.send_others_near(pos.map(f64::from), 32.0, e);
    }

    /// Breaking progress and crack stage broadcast when it changes.
    fn destroy_stage(&self, m: &mut Mining, state: u32, pos: [i32; 3], ticks: i64) -> f32 {
        let progress = self.mine_gain(state) * (ticks + 1) as f32;
        let stage = (progress * 10.0) as i32;
        if stage != m.last_stage {
            self.destroy_progress(pos, stage);
            m.last_stage = stage;
        }
        progress
    }

    fn is_air_at(&self, p: [i32; 3]) -> bool {
        self.state_at(p).is_none_or(|s| self.v().is_air(s))
    }

    /// Mining `player_action` (§ 6.1), 26.3 network numbering: 0 start, 1 face
    /// change, 2 abort, 3 finish.
    pub(crate) fn on_action(&mut self, action: i32, pos: [i32; 3], face: u8, seq: i32) {
        if !(0..=3).contains(&action) {
            return;
        }
        // Demo over: breaking is refused (reminder), the acknowledgement is sent anyway.
        let refused = self.demo_refuses();
        self.acknowledge(seq);
        if refused {
            return;
        }
        if !self.player_ready() {
            return;
        }
        self.ensure_around(pos);
        let mut m = self.mining.unwrap_or_default();
        if !self.in_reach(pos) {
            if m.active && action == 2 {
                self.abort_destroy(&mut m, pos);
                self.mining = Some(m);
            }
            return;
        }
        if !self.in_height(pos[1]) {
            self.send_block(pos);
            return;
        }
        match action {
            0 => {
                // Out of bounds, refusal and true state sent back.
                if !self.may_interact(pos) {
                    self.send_block(pos);
                    return;
                }
                if self.game_mode == 1 {
                    self.try_break(pos);
                    return;
                }
                // Spectator, adventure (without rights): refusal, true state sent back.
                if self.game_mode >= 2 {
                    self.send_block(pos);
                    return;
                }
                m.start = self.mine_ticks;
                let state = self.state_at(pos).unwrap_or(0);
                let air = self.v().is_air(state);
                let mut progress = 1.0;
                if !air {
                    // Hit of the block (egg, note, ore), then progress from the previous state.
                    self.block_attack(pos, state);
                    progress = self.mine_gain(state);
                }
                if !air && progress >= 1.0 {
                    self.mining = Some(m);
                    self.try_break(pos);
                    return;
                }
                if m.active {
                    // Other hit in progress (immediate break refused): true state sent back.
                    self.send_block(m.pos);
                }
                m.active = true;
                m.pos = pos;
                m.face = face;
                let stage = (progress * 10.0) as i32;
                self.destroy_progress(pos, stage);
                m.last_stage = stage;
            }
            1 => m.face = face,
            3 => {
                if pos == m.pos && !self.is_air_at(pos) {
                    let state = self.state_at(pos).unwrap_or(0);
                    let ticks = self.mine_ticks - m.start;
                    if self.mine_gain(state) * (ticks + 1) as f32 >= 0.7 {
                        m.active = false;
                        self.destroy_progress(pos, -1);
                        self.mining = Some(m);
                        self.try_break(pos);
                        return;
                    }
                    if m.delayed.is_none() {
                        m.active = false;
                        m.delayed = Some((pos, m.start));
                    }
                }
            }
            _ => self.abort_destroy(&mut m, pos),
        }
        self.mining = Some(m);
    }

    /// Abort of the hit: cracks cleared (that of another cell too).
    fn abort_destroy(&self, m: &mut Mining, pos: [i32; 3]) {
        m.active = false;
        if m.pos != pos {
            self.destroy_progress(m.pos, -1);
        }
        self.destroy_progress(pos, -1);
    }

    /// Mining tick (§ 6.1): the deferred break goes first; otherwise the hit in progress emits
    /// every tick event 2019 (pieces on the face), 2020 every 4 ticks (with the sound).
    pub(crate) fn tick_mining(&mut self) {
        self.mine_ticks += 1;
        let Some(mut m) = self.mining else { return };
        if let Some((pos, start)) = m.delayed {
            if self.is_air_at(pos) {
                m.delayed = None;
            } else {
                // Quirk of 26.3: the START tick is passed as the hit's duration; the
                // progress no longer grows, the break falls on the next tick or never.
                let state = self.state_at(pos).unwrap_or(0);
                if self.destroy_stage(&mut m, state, pos, start) >= 1.0 {
                    m.delayed = None;
                    self.mining = Some(m);
                    self.player_destroy_block(pos);
                    return;
                }
            }
        } else if m.active {
            if self.is_air_at(m.pos) {
                self.destroy_progress(m.pos, -1);
                m.last_stage = -1;
                m.active = false;
            } else {
                let spent = self.mine_ticks - m.start;
                let event = if spent % 4 == 0 { 2020 } else { 2019 };
                let state = self.state_at(m.pos).unwrap_or(0);
                let at = m.pos;
                self.destroy_stage(&mut m, state, at, spent);
                self.level_event(event, m.pos, i32::from(m.face));
            }
        }
        self.mining = Some(m);
    }

    /// Break with acknowledgement (§ 6.1): refused, the true state is sent back.
    pub(crate) fn try_break(&mut self, pos: [i32; 3]) {
        if !self.player_destroy_block(pos) {
            self.send_block(pos);
        }
    }

    /// Break by the player: capable item, operator blocks, loot, removal.
    pub(crate) fn player_destroy_block(&mut self, pos: [i32; 3]) -> bool {
        let Some(state) = self.state_at(pos) else { return false };
        // G5-F10: the debug stick never breaks; it picks the next property.
        if self.held_stack().is_some_and(|s| s.id == "minecraft:debug_stick") {
            self.debug_stick_use(36 + self.held as usize, pos, false);
            return false;
        }
        let creative_tool_ok = self
            .held_stack()
            .and_then(|s| self.item_index(&s.id))
            .and_then(|i| self.data.items.get(i))
            .and_then(|i| i.tool.as_ref())
            .is_none_or(|t| t.creative_destroy);
        let operator = self.data.block_of(state).is_some_and(|b| {
            matches!(
                b.name.as_str(),
                "minecraft:command_block"
                    | "minecraft:chain_command_block"
                    | "minecraft:repeating_command_block"
                    | "minecraft:structure_block"
                    | "minecraft:jigsaw"
                    | "minecraft:test_block"
                    | "minecraft:test_instance_block"
            )
        });
        if self.game_mode == 1 && !creative_tool_ok {
            return false;
        }
        if operator && !self.gamemaster_blocks() {
            self.send_block(pos);
            return false;
        }
        // Tool as before the wear.
        let tool = self.held_stack().cloned();
        let state = self.player_will_destroy(pos, state, tool.as_ref());
        // S2: loot, wear, experience and container contents (block entity still there), then
        // removal: linked halves, remaining fluid and notifications (`blocks::placement`).
        self.on_block_broken(pos, state);
        self.player_destroy(pos);
        if self.game_mode != 1 {
            self.player_destroyed(pos, state, tool.as_ref());
        }
        true
    }

    /// `use_item_on` (§ 6.2).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_use_item_on(
        &mut self,
        hand: i32,
        pos: [i32; 3],
        face: i32,
        at: [f32; 3],
        inside: bool,
        seq: i32,
    ) {
        self.acknowledge(seq);
        if !self.player_ready() || !(0..6).contains(&face) {
            return;
        }
        let face = face as usize;
        self.ensure_around(pos);
        // Point hit within 1.0000001 of the block's center on each axis.
        if !self.in_reach(pos) || at.iter().any(|v| (f64::from(*v) - 0.5).abs() >= 1.000_000_1) {
            return;
        }
        let d = FACES[face];
        let neighbor = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
        // Aimed cell out of height: message only.
        if pos[1] >= self.top_y() || pos[1] < self.dim.height.min_y {
            self.build_limit_message(pos[1] >= self.top_y());
            return;
        }
        // The allowed-interaction test (out of bounds) or pending teleport: nothing is used or
        // placed, "too high" message (like the game), true states sent back.
        if !self.may_interact(pos) || self.teleport.is_some() {
            self.build_limit_message(true);
            self.send_block(pos);
            self.send_block(neighbor);
            return;
        }
        // Demo over: nothing is used or placed (reminder), true states sent back.
        if self.demo_refuses() {
            self.send_block(pos);
            self.send_block(neighbor);
            return;
        }
        // S2: menu block or bed first (except when sneaking with an item in hand).
        let busy_hands = self.slots[36 + self.held as usize].is_some() || self.slots[45].is_some();
        let s9_before = (self.data.block_of(self.get(pos)).map(|b| b.name.clone()).unwrap_or_default(), self.life.menu.is_some()); // S9
        let used = hand == 0 && !(self.sneaking && busy_hands) && (self.cmdblock_use(pos) || self.use_block_or_bed(pos)); // S5
        if used {
            self.s9_block_used(pos, &s9_before.0, !s9_before.1 && self.life.menu.is_some()); // S9
            self.piglin_container_opened(&s9_before.0, !s9_before.1 && self.life.menu.is_some()); // F13
        }
        // Spectator: a clicked portal makes them pass through (without a menu to open).
        let used = used || (self.game_mode == 3 && self.spectator_portal(pos));
        let used = used || (self.game_mode != 3 && self.proj_use_on(hand, pos, face, at)); // S8: rocket placed
        if !used && self.teleport.is_none() && self.game_mode != 3 && hand == 0 {
            // W8c: buckets (including mob ones) act on use (`use_item`, server aim),
            // like the game: their use on a block goes through.
            self.hit_inside = inside;
            self.use_item_on_block(pos, crate::blocks::Dir::from_index(face), at);
            self.hit_inside = false;
        }
        // Always: true state of the clicked cell and its neighbor (fixes a prediction).
        self.send_block(pos);
        self.send_block(neighbor);
    }

    /// Height limit message (action bar, red): ceiling or floor.
    pub(crate) fn build_limit_message(&self, too_high: bool) {
        let (key, limit) =
            if too_high { ("build.tooHigh", self.top_y() - 1) } else { ("build.tooLow", self.dim.height.min_y) };
        let mut msg = rc_text::Component::translate(key, vec![rc_text::Arg::Text(limit.to_string())]);
        msg.style.color = Some(crate::commands::text::RED);
        let _ = self.send(Event::ActionBar(msg));
    }

    /// Middle click: the block's clone item in the hotbar, already present or in a free
    /// slot (the held slot first), otherwise in place of the held stack; `with_data` (Ctrl,
    /// creative only) copies the block entity's data into it.
    pub(crate) fn pick_block(&mut self, pos: [i32; 3], with_data: bool) {
        if !self.in_reach(pos) {
            return;
        }
        let Some(state) = self.state_at(pos) else { return };
        let with_data = self.game_mode == 1 && with_data;
        let Some(mut stack) = self.clone_item(pos, state, with_data) else { return };
        if with_data && let Some(be) = self.be_get(pos) {
            block_data_on_item(&mut stack, be);
        }
        // W8a: choice shared with the entities: in survival, the item already carried
        // is selected or brought back to the hotbar; in creative, it is added if missing.
        self.try_pick_item(stack);
    }

    /// Clone item of a block: the item that places it (wall variants, crops, stems,
    /// potted plants, full cauldrons…), with the state or components that some blocks
    /// put on it; `None` for a block without an item (air, fluids, portals, frosted ice…).
    pub(crate) fn clone_item(&self, pos: [i32; 3], state: u32, with_data: bool) -> Option<crate::player::Stack> {
        use rc_proto::nbt::Nbt;
        let block = self.data.block_of(state)?.name.as_str();
        let short = block.strip_prefix("minecraft:").unwrap_or(block);
        let prop = |k: &str| self.data.state_properties(state).find(|(n, _)| *n == k).map(|(_, v)| v.to_owned());
        let block_state = |k: &str, v: String| {
            Nbt::Compound(vec![("minecraft:block_state".into(), Nbt::Compound(vec![(k.into(), Nbt::String(v))]))])
        };
        let mut components = None;
        let name: String = match short {
            "end_gateway" | "end_portal" | "nether_portal" | "frosted_ice" | "moving_piston" => return None,
            "piston_head" => if prop("type").as_deref() == Some("sticky") { "sticky_piston" } else { "piston" }.into(),
            "melon_stem" | "attached_melon_stem" => "melon_seeds".into(),
            "pumpkin_stem" | "attached_pumpkin_stem" => "pumpkin_seeds".into(),
            "wheat" => "wheat_seeds".into(),
            "carrots" => "carrot".into(),
            "potatoes" => "potato".into(),
            "beetroots" => "beetroot_seeds".into(),
            "torchflower_crop" => "torchflower_seeds".into(),
            "pitcher_crop" => "pitcher_pod".into(),
            "cocoa" => "cocoa_beans".into(),
            "redstone_wire" => "redstone".into(),
            "tripwire" => "string".into(),
            "powder_snow" => "powder_snow_bucket".into(),
            "sweet_berry_bush" => "sweet_berries".into(),
            "cave_vines" | "cave_vines_plant" => "glow_berries".into(),
            "bamboo_sapling" => "bamboo".into(),
            "big_dripleaf_stem" => "big_dripleaf".into(),
            "tall_seagrass" => "seagrass".into(),
            "kelp_plant" | "weeping_vines_plant" | "twisting_vines_plant" => short.trim_end_matches("_plant").into(),
            "water_cauldron" | "lava_cauldron" | "powder_snow_cauldron" => "cauldron".into(),
            "potted_azalea_bush" => "azalea".into(),
            "potted_flowering_azalea_bush" => "flowering_azalea".into(),
            s if s.starts_with("potted_") => s["potted_".len()..].into(),
            s if s.ends_with("candle_cake") => "cake".into(),
            s => {
                if self.item_index(block).is_some() {
                    s.into()
                } else {
                    // Wall variant: the item of the variant placed on the ground.
                    let wall = s.replacen("wall_", "", 1);
                    if wall == s {
                        return None;
                    }
                    wall
                }
            }
        };
        let id = format!("minecraft:{name}");
        self.item_index(&id)?;
        let be = self.be_get(pos);
        match short {
            "light" => components = prop("level").map(|v| block_state("level", v)),
            "test_block" => components = prop("mode").map(|v| block_state("mode", v)),
            "beehive" | "bee_nest" if with_data => {
                components = prop("honey_level").map(|v| block_state("honey_level", v))
            }
            "decorated_pot" => {
                if let Some(sherds) = be.and_then(|b| b.get("sherds")) {
                    components = Some(Nbt::Compound(vec![("minecraft:pot_decorations".into(), sherds.clone())]));
                }
            }
            s if s.ends_with("banner") || s.contains("copper_golem_statue") => {
                if let Some(b) = be {
                    let mut c = crate::loot::be::collect(b);
                    if s.contains("copper_golem_statue")
                        && let Some(pose) = prop("copper_golem_pose")
                    {
                        c.retain(|(k, _)| k != "minecraft:block_state");
                        c.push((
                            "minecraft:block_state".into(),
                            Nbt::Compound(vec![("copper_golem_pose".into(), Nbt::String(pose))]),
                        ));
                    }
                    if !c.is_empty() {
                        components = Some(Nbt::Compound(c));
                    }
                }
            }
            _ => {}
        }
        let mut st = crate::player::Stack { id, count: 1, components };
        crate::items::normalize_patch(&mut st);
        Some(st)
    }

    /// `client_information`: skin parts (metadata 16) and main arm (15) sent
    /// when they change; hat changed: player list updated for everyone.
    pub(crate) fn set_model(&mut self, skin_parts: u8, main_hand: i32) {
        use rc_proto_v777::entity::Meta;
        let (old_skin, old_hand) = self.model;
        self.model = (skin_parts, main_hand);
        let mut values = Vec::new();
        if main_hand != old_hand {
            values.push((15, Meta::Variant(main_hand, 42)));
        }
        if skin_parts != old_skin {
            values.push((16, Meta::Byte(skin_parts as i8)));
        }
        if !values.is_empty() {
            let _ = self.send(Event::Entity(rc_client::EntityEvent::Data { id: self.player_id, values }));
        }
        if (skin_parts ^ old_skin) & HAT != 0 {
            use rc_proto_v777::players::{InfoEntry, InfoUpdate, action};
            let entries =
                vec![InfoEntry { uuid: self.uuid, show_hat: Some(skin_parts & HAT != 0), ..Default::default() }];
            let _ = self.send(Event::PlayerInfo(InfoUpdate { actions: action::UPDATE_HAT, entries }));
        }
    }

    /// `spectator_action`: as a spectator, the camera moves to the targeted entity if it is within
    /// the border, within entity reach + 3 and selectable.
    pub(crate) fn on_spectator_action(&mut self, target: Option<i32>) {
        use crate::commands::ents::E;
        if !self.player_ready() || self.game_mode != 3 {
            return;
        }
        let Some(e) = target.and_then(|id| self.e_of_net(id)) else { return };
        let feet = self.e_pos(e).map(|v| v.floor() as i32);
        if !Self::block_in_border(self.border_bounds(), feet) {
            return;
        }
        let range = self.player_attribute("minecraft:entity_interaction_range") + 3.0;
        let bb = self.e_bbox(e);
        let eye = [self.pos[0], self.pos[1] + self.ent_eye_height(E::Player), self.pos[2]];
        let d2: f64 = (0..3).map(|a| (eye[a] - eye[a].clamp(bb.min[a], bb.max[a])).powi(2)).sum();
        if d2 >= range * range {
            return;
        }
        let ty = self.ent_type(e);
        let short = ty.trim_start_matches("minecraft:");
        let pickable = match e {
            E::Player => false, // the player themselves, spectator
            E::Mob(_) => short != "ender_dragon",
            E::Item(_) | E::Orb(_) => false,
            // Armor stand: alive, selectable except as a marker.
            E::Other(_) if short == "armor_stand" => {
                self.ent_nbt(e).get("Marker").and_then(rc_proto::nbt::Nbt::as_i64).unwrap_or(0) == 0
            }
            // Attached to a block, crystal, interaction, falling block, TNT, shulker bullet,
            // vehicles, and projectiles of the `redirectable_projectile` tag (fireball, wind
            // charges); the other projectiles and arrows are not.
            E::Other(_) => {
                matches!(
                    short,
                    "item_frame"
                        | "glow_item_frame"
                        | "painting"
                        | "leash_knot"
                        | "end_crystal"
                        | "falling_block"
                        | "tnt"
                        | "shulker_bullet"
                        | "interaction"
                        | "fireball"
                        | "wind_charge"
                        | "breeze_wind_charge"
                        | "minecart"
                ) || short.ends_with("_boat")
                    || short.ends_with("_raft")
                    || short.ends_with("_minecart")
            }
        };
        if pickable {
            self.prog.cmds.camera = Some(e);
            let _ = self.send(Event::SetCamera(self.ent_id(e)));
        }
    }

    pub(crate) fn send_slot(&mut self, _slot: usize) {
        self.broadcast_menus(); // S2: differences sent with the tracked state number
    }

    /// Flags of the active dimension's type (read once, see [`LightOpts::misc`]).
    pub(crate) fn dim_kind(&self) -> crate::world::DimKind {
        if self.light_opts.misc {
            *self.dim.kind.get_or_init(|| crate::world::DimKind::of(&self.data, &self.dim.name))
        } else {
            crate::world::DimKind::of(&self.data, &self.dim.name)
        }
    }

    /// Light recomputation around the tick's changes (`sky`: those that touch the sky)
    /// then `light_update` of the sent columns.
    fn relight_flush(&mut self, relight: &[[i32; 3]], sky: &[[i32; 3]]) {
        let o = self.light_opts;
        let min_y = self.dim.height.min_y;
        let height = self.dim.height.sections as i32 * 16;
        let n = self.dim.height.sections + 2;
        // Missing layers around the changes: those the game would read in their place. Created:
        // (column, layer position, sky), signaled like the game (section and 26 neighbors).
        let mut created: Vec<(i32, i32, i32, bool)> = Vec::new();
        let mut fill = |chunks: &mut HashMap<(i32, i32), Option<Box<Chunk>>>, x: i32, z: i32, i: i32| {
            let Some(Some(c)) = chunks.get_mut(&(x, z)) else { return };
            let Some(iu) = usize::try_from(i).ok().filter(|&i| i < c.sky.len()) else { return };
            if c.sky[iu].is_none() {
                c.sky[iu] = Some(std::sync::Arc::new(implied_sky(&c.sky, iu)));
                created.push((x, z, i, true));
            }
            if c.block[iu].is_none() {
                c.block[iu] = Some(rc_world::zero_layer());
                created.push((x, z, i, false));
            }
        };
        const AROUND: [(i32, i32); 9] = [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 0), (0, 1), (1, -1), (1, 0), (1, 1)];
        if o.misc {
            // Once per distinct (column, position): the fill order changes nothing (an implicit
            // layer takes the bottom plane of the first stored layer above, which an interposed implicit
            // layer copies identically).
            let mut keys: Vec<(i32, i32, i32)> = Vec::with_capacity(relight.len() * 27);
            for p in relight {
                let si = ((p[1] - min_y) >> 4) + 1;
                for (dx, dz) in AROUND {
                    for i in [si - 1, si, si + 1] {
                        keys.push(((p[0] >> 4) + dx, (p[2] >> 4) + dz, i));
                    }
                }
            }
            keys.sort_unstable();
            keys.dedup();
            for (x, z, i) in keys {
                fill(&mut self.chunks, x, z, i);
            }
        } else {
            for p in relight {
                for (dx, dz) in AROUND {
                    let si = ((p[1] - min_y) >> 4) + 1;
                    for i in [si - 1, si, si + 1] {
                        fill(&mut self.chunks, (p[0] >> 4) + dx, (p[2] >> 4) + dz, i);
                    }
                }
            }
        }
        let local: Vec<[i32; 3]> = relight.iter().map(|p| [p[0], p[1] - min_y, p[2]]).collect();
        let has_sky = self.dim_kind().sky;
        light_trace(&self.chunks, &self.dim.name, height, has_sky, self.data.state_bits() as u8, self.tick, &local);
        if !o.sky_list && !o.sections {
            // Old path: every touched column sent back in full.
            let touched = rc_light::relight(&mut LightChunks(&mut self.chunks), height, &self.light, has_sky, &local);
            for (x, z) in touched {
                let Some(Some(c)) = self.chunks.get_mut(&(x, z)) else { continue };
                c.dirty = true;
                if self.sent.contains(&(x, z)) {
                    let light = Box::new(crate::server::light_data(&c.sky, &c.block, false));
                    let _ = self.send(Event::LightUpdate { x, z, light });
                }
            }
            return;
        }
        let sky_local: Option<Vec<[i32; 3]>> = o.sky_list.then(|| sky.iter().map(|p| [p[0], p[1] - min_y, p[2]]).collect());
        let (sky_t, block_t) = rc_light::relight_sections_split(
            &mut LightChunks(&mut self.chunks),
            height,
            &self.light,
            has_sky,
            &local,
            sky_local.as_deref(),
        );
        let mut cols: Vec<(i32, i32)> = sky_t.keys().chain(block_t.keys()).map(|&(x, _, z)| (x, z)).collect();
        cols.sort_unstable();
        cols.dedup();
        for &(x, z) in &cols {
            if let Some(Some(c)) = self.chunks.get_mut(&(x, z)) {
                c.dirty = true;
            }
        }
        if !o.sections {
            for (x, z) in cols {
                let Some(Some(c)) = self.chunks.get(&(x, z)) else { continue };
                if self.sent.contains(&(x, z)) {
                    let light = Box::new(crate::server::light_data(&c.sky, &c.block, false));
                    let _ = self.send(Event::LightUpdate { x, z, light });
                }
            }
            return;
        }
        let mut f = LightFilters::new();
        mark_touched(&mut f, n, &sky_t, true);
        mark_touched(&mut f, n, &block_t, false);
        if !light_sabotage(4) {
            for (x, z, i, sky) in created {
                for (dx, dz) in AROUND {
                    for di in -1..=1 {
                        mark_section(&mut f, n, x + dx, z + dz, i + di, sky);
                    }
                }
            }
        }
        for ((x, z), (sf, bf)) in f {
            let Some(Some(c)) = self.chunks.get(&(x, z)) else { continue };
            if self.sent.contains(&(x, z)) {
                let light = Box::new(crate::server::light_data_filtered(&c.sky, &c.block, &sf, &bf));
                let _ = self.send(Event::LightUpdate { x, z, light });
            }
        }
    }

    /// End of tick (§ 3.3): light recomputed then `light_update`, then per section a
    /// `block_update` (one block) or a `section_blocks_update` (several).
    pub(crate) fn flush_block_changes(&mut self) {
        if self.changed.is_empty() {
            return;
        }
        let changed = std::mem::take(&mut self.changed);
        let relight = std::mem::take(&mut self.relight);
        let relight_sky = std::mem::take(&mut self.relight_sky);
        if !relight.is_empty() {
            self.relight_flush(&relight, &relight_sky);
        }
        let min_section = self.dim.height.min_y >> 4;
        let mut by_section: BTreeMap<(i32, i32, i32), Vec<[i32; 3]>> = BTreeMap::new();
        for p in changed {
            let v = by_section.entry((p[0] >> 4, p[1] >> 4, p[2] >> 4)).or_default();
            if !v.contains(&p) {
                v.push(p);
            }
        }
        for ((sx, sy, sz), list) in by_section {
            if !self.sent.contains(&(sx, sz)) || sy < min_section {
                continue;
            }
            if list.len() == 1 {
                self.send_block(list[0]);
            } else {
                let changes = list
                    .iter()
                    .filter_map(|&p| {
                        let s = self.state_at(p)?;
                        Some((block_index((p[0] & 15) as usize, (p[1] & 15) as usize, (p[2] & 15) as usize) as u16, s))
                    })
                    .collect();
                let _ = self.send(Event::SectionUpdate { section: [sx, sy, sz], changes });
            }
            // G6-F01: block entities with displayed contents (campfire, shelf, pot): their
            // update follows the state, like the possible block-entity broadcast.
            for &p in &list {
                let Some(s) = self.state_at(p) else { continue };
                if let Some(nbt) = self.be_update_tag(p, s)
                    && let Some(kind) = self.be_type_index(p)
                {
                    let _ = self.send(Event::BlockEntityData { pos: p, kind, nbt: Some(nbt) });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// Diagonal flow of 126 cells (a single chained group) plus an isolated change
    /// far away: the trace writes the whole group's box at ±16 (corners included), and the
    /// isolated change's zone, nothing else.
    #[test]
    fn trace_columns_cover_each_group_box() {
        let mut changes: Vec<[i32; 3]> = (0..=21).map(|i| [i * 6, 64, i * 6]).collect();
        changes.push([1000, 64, -1000]);
        let got: std::collections::HashSet<(i32, i32)> = super::trace_columns(&changes).into_iter().collect();
        let mut want = std::collections::HashSet::new();
        for cx in -1..=8 {
            for cz in -1..=8 {
                want.insert((cx, cz));
            }
        }
        for cx in (1000 - 16) >> 4..=(1000 + 16) >> 4 {
            for cz in (-1000 - 16) >> 4..=(-1000 + 16) >> 4 {
                want.insert((cx, cz));
            }
        }
        assert_eq!(got, want);
    }
}
