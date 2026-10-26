//! Dragon fight (F15, the dragon fight): saved dimension state
//! (`data/minecraft/ender_dragon_fight.dat`), boss bar of nearby players, arena kept
//! loaded, first pass (inactive exit portal, dragon created), pillar crystals
//! counted, victory (active portal, gateway, egg), respawn with the four crystals placed
//! around the portal (beams, pillars rebuilt, new dragon).

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto_v777::play::{BossEvent, BossOp};

use super::super::jrand::JRand;
use super::super::mob::Ent;
use super::super::species::Kind;
use crate::blocks::flag;
use crate::nbtio::{self, int};
use crate::server::Server;

/// Respawn stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Start,
    PreparingToSummonPillars,
    SummoningPillars,
    SummoningDragon,
    End,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Start => "start",
            Stage::PreparingToSummonPillars => "preparing_to_summon_pillars",
            Stage::SummoningPillars => "summoning_pillars",
            Stage::SummoningDragon => "summoning_dragon",
            Stage::End => "end",
        }
    }

    fn from_name(n: &str) -> Option<Stage> {
        Some(match n {
            "start" => Stage::Start,
            "preparing_to_summon_pillars" => Stage::PreparingToSummonPillars,
            "summoning_pillars" => Stage::SummoningPillars,
            "summoning_dragon" => Stage::SummoningDragon,
            "end" => Stage::End,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Fight {
    pub needs_state_scanning: bool,
    pub dragon_killed: bool,
    pub previously_killed: bool,
    pub stage: Option<Stage>,
    pub respawn_time: i32,
    pub dragon_uuid: Option<u128>,
    pub exit_portal: Option<[i32; 3]>,
    pub gateways: Vec<i32>,
    pub respawn_crystals: Vec<u128>,
    pub origin: [i32; 3],
    pub alive_crystals: i32,
    ticks_since_dragon_seen: i32,
    ticks_since_crystals: i32,
    ticks_since_players: i32,
    pub dirty: bool,
    // Boss bar.
    pub bar: u128,
    pub bar_visible: bool,
    pub bar_player: bool,
    pub bar_progress: f32,
    pub bar_name: Option<Nbt>,
}

impl Fight {
    /// State read (game codec), otherwise default.
    fn read(root: Option<&Nbt>) -> Fight {
        let d = root.and_then(|r| r.get("data"));
        let flag_or = |k: &str, def: bool| d.and_then(|d| int(d, k)).map_or(def, |v| v != 0);
        let ints = |n: Option<&Nbt>| -> Vec<i32> {
            match n {
                Some(Nbt::IntArray(a)) => a.clone(),
                Some(Nbt::List(l)) => l.iter().filter_map(Nbt::as_i64).map(|v| v as i32).collect(),
                _ => Vec::new(),
            }
        };
        let pos = |n: Option<&Nbt>| -> Option<[i32; 3]> {
            let v = ints(n);
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        };
        Fight {
            needs_state_scanning: flag_or("needs_state_scanning", true),
            dragon_killed: flag_or("dragon_killed", false),
            previously_killed: flag_or("previously_killed", false),
            stage: d.and_then(|d| d.get("respawn_stage")).and_then(Nbt::as_str).and_then(Stage::from_name),
            respawn_time: d.and_then(|d| int(d, "respawn_time")).unwrap_or(0) as i32,
            dragon_uuid: d.and_then(|d| nbtio::uuid(d.get("dragon_uuid"))),
            exit_portal: pos(d.and_then(|d| d.get("exit_portal_location"))),
            gateways: ints(d.and_then(|d| d.get("gateways"))),
            respawn_crystals: nbtio::list(d.and_then(|d| d.get("respawn_crystals")))
                .iter()
                .filter_map(|n| nbtio::uuid(Some(n)))
                .collect(),
            origin: [0, 0, 0],
            alive_crystals: 0,
            ticks_since_dragon_seen: 0,
            ticks_since_crystals: 0,
            ticks_since_players: 21,
            dirty: false,
            bar: 0,
            bar_visible: true,
            bar_player: false,
            bar_progress: 1.0,
            bar_name: None,
        }
    }

    fn to_nbt(&self) -> Nbt {
        let mut d: Vec<(String, Nbt)> = vec![
            ("needs_state_scanning".into(), Nbt::Byte(i8::from(self.needs_state_scanning))),
            ("dragon_killed".into(), Nbt::Byte(i8::from(self.dragon_killed))),
            ("previously_killed".into(), Nbt::Byte(i8::from(self.previously_killed))),
        ];
        if let Some(s) = self.stage {
            d.push(("respawn_stage".into(), Nbt::String(s.name().into())));
        }
        d.push(("respawn_time".into(), Nbt::Int(self.respawn_time)));
        if let Some(u) = self.dragon_uuid {
            d.push(("dragon_uuid".into(), nbtio::uuid_nbt(u)));
        }
        if let Some(p) = self.exit_portal {
            d.push(("exit_portal_location".into(), Nbt::IntArray(p.to_vec())));
        }
        d.push(("gateways".into(), Nbt::IntArray(self.gateways.clone())));
        if !self.respawn_crystals.is_empty() {
            d.push((
                "respawn_crystals".into(),
                Nbt::List(self.respawn_crystals.iter().map(|u| nbtio::uuid_nbt(*u)).collect()),
            ));
        }
        Nbt::Compound(vec![
            ("data".into(), Nbt::Compound(d)),
            ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
        ])
    }
}

/// Pattern of the exit portal around its center `c` (ring level): 3-block pillar above, ring and
/// bedrock base. Layers from top to bottom (c.y + 3 .. c.y − 1).
const RING: [&str; 7] = ["  ###  ", " #   # ", "#     #", "#  #  #", "#     #", " #   # ", "  ###  "];
const BASE: [&str; 7] = ["       ", "  ###  ", " ##### ", " ##### ", " ##### ", "  ###  ", "       "];

/// End portal block entity: the portal, and the gateway, which is a variant of it.
fn portal_be(b: &Nbt) -> bool {
    matches!(b.get("id").and_then(Nbt::as_str), Some("minecraft:end_portal" | "minecraft:end_gateway"))
}

fn layer(i: usize) -> [&'static str; 7] {
    const PILLAR: [&str; 7] = ["       ", "       ", "       ", "   #   ", "       ", "       ", "       "];
    match i {
        0..=2 => PILLAR,
        3 => RING,
        _ => BASE,
    }
}

impl Server {
    /// The active dimension's fight (created at the first tick of a dimension with a fight).
    fn fight_init(&mut self) {
        if self.mobs.end.fight_checked {
            return;
        }
        self.mobs.end.fight_checked = true;
        let has =
            self.dim_type_field(&self.dim.name, "has_ender_dragon_fight").and_then(|v| v.as_bool()).unwrap_or(false);
        if !has {
            return;
        }
        let path = self.dim.dir.join("data").join("minecraft").join("ender_dragon_fight.dat");
        let root = nbtio::read_dat(&path).ok();
        let mut f = Fight::read(root.as_ref());
        // `init`: bar (UUID drawn from the world random), gateways shuffled by the seed.
        let r = &mut self.mobs.rng;
        let hi = (r.next_long() & -61441) | 16384;
        let lo = (r.next_long() & 4_611_686_018_427_387_903) | i64::MIN;
        f.bar = (hi as u64 as u128) << 64 | lo as u64 as u128;
        if f.gateways.is_empty() {
            let mut g: Vec<i32> = (0..20).collect();
            JRand::new(self.world_seed()).shuffle(&mut g);
            f.gateways = g;
            f.dirty = true;
        }
        self.mobs.end.fight = Some(Box::new(f));
    }

    fn fight(&mut self) -> Option<&mut Fight> {
        self.mobs.end.fight.as_deref_mut()
    }

    /// Fight tick (before the entities).
    pub(crate) fn fight_tick(&mut self) {
        self.fight_init();
        let Some(f) = self.mobs.end.fight.as_ref() else { return };
        let visible = !f.dragon_killed;
        self.fbar_set_visible(visible);
        let f = self.fight().expect("combat");
        f.ticks_since_players += 1;
        if f.ticks_since_players >= 20 {
            f.ticks_since_players = 0;
            self.fbar_update_players();
        }
        let f = self.mobs.end.fight.as_ref().expect("combat");
        if !f.bar_player {
            return;
        }
        // Arena ticket: chunks within 8 of the origin kept loaded.
        let o = f.origin;
        let (cx, cz) = (o[0] >> 4, o[2] >> 4);
        let arena: Vec<(i32, i32)> = (-8..=8).flat_map(|x| (-8..=8).map(move |z| (cx + x, cz + z))).collect();
        self.ensure_loaded(&arena);
        if !self.arena_loaded() {
            return;
        }
        if self.mobs.end.fight.as_ref().is_some_and(|f| f.needs_state_scanning) {
            self.fight_scan_state();
            if let Some(f) = self.fight() {
                f.needs_state_scanning = false;
                f.dirty = true;
            }
        }
        if let Some(stage) = self.mobs.end.fight.as_ref().and_then(|f| f.stage) {
            let refs = self.mobs.end.fight.as_ref().map(|f| f.respawn_crystals.clone()).unwrap_or_default();
            let crystals: Vec<i32> = refs
                .iter()
                .filter_map(|u| self.mobs.end.crystals.iter().find(|c| c.uuid == *u && !c.removed).map(|c| c.id))
                .collect();
            if crystals.is_empty() {
                self.fight_abort_respawn();
                return;
            }
            let time = {
                let f = self.fight().expect("combat");
                let t = f.respawn_time;
                f.respawn_time += 1;
                f.dirty = true;
                t
            };
            self.respawn_stage_tick(stage, &crystals, time);
        }
        let Some(f) = self.mobs.end.fight.as_ref() else { return };
        if !f.dragon_killed {
            let create = {
                let f = self.fight().expect("combat");
                if f.dragon_uuid.is_none() {
                    true
                } else {
                    f.ticks_since_dragon_seen += 1;
                    f.ticks_since_dragon_seen >= 1200
                }
            };
            if create {
                self.fight_find_or_create_dragon();
                if let Some(f) = self.fight() {
                    f.ticks_since_dragon_seen = 0;
                }
            }
            let scan = {
                let f = self.fight().expect("combat");
                f.ticks_since_crystals += 1;
                f.ticks_since_crystals >= 100
            };
            if scan {
                self.fight_update_crystal_count();
            }
        }
    }

    /// The game loop tests only row z = 8 (from −8 to 8 in x), then
    /// the entities of the 3 × 3 chunks around the origin.
    fn arena_loaded(&self) -> bool {
        let Some(f) = self.mobs.end.fight.as_ref() else { return false };
        let (cx, cz) = (f.origin[0] >> 4, f.origin[2] >> 4);
        let z = 8 + cz;
        for x in -8 + cx..=8 + cx {
            if !self.chunks.get(&(x, z)).is_some_and(Option::is_some) {
                return false;
            }
        }
        (-1..=1).all(|dx| (-1..=1).all(|dz| self.mobs.adopted.contains(&(cx + dx, cz + dz))))
    }

    /// Player in the fight zone (alive, within 192 blocks of the origin + 128 in height).
    fn fight_player_in_range(&self) -> bool {
        let Some(f) = self.mobs.end.fight.as_ref() else { return false };
        let c = [f64::from(f.origin[0]), f64::from(128 + f.origin[1]), f64::from(f.origin[2])];
        self.ent_alive(Ent::Player) && super::super::mth::dist2(self.pos, c) <= 192.0 * 192.0
    }

    // ---------- Boss bar ----------

    fn fbar_add_packet(&self) -> Option<BossEvent> {
        let f = self.mobs.end.fight.as_ref()?;
        let name = f.bar_name.clone().unwrap_or_else(|| {
            Nbt::Compound(vec![("translate".into(), Nbt::String("entity.minecraft.ender_dragon".into()))])
        });
        Some(BossEvent {
            id: f.bar,
            op: BossOp::Add { name: Some(name), progress: f.bar_progress, color: 0, style: 0, flags: 6 },
        })
    }

    fn fbar_set_visible(&mut self, v: bool) {
        let Some(f) = self.fight() else { return };
        if f.bar_visible == v {
            return;
        }
        f.bar_visible = v;
        let (player, bar) = (f.bar_player, f.bar);
        if player {
            if v {
                if let Some(p) = self.fbar_add_packet() {
                    let _ = self.send(Event::Boss(p));
                }
            } else {
                let _ = self.send(Event::Boss(BossEvent { id: bar, op: BossOp::Remove }));
            }
        }
    }

    fn fbar_update_players(&mut self) {
        let valid = self.fight_player_in_range();
        let Some(f) = self.fight() else { return };
        if valid && !f.bar_player {
            f.bar_player = true;
            if f.bar_visible
                && let Some(p) = self.fbar_add_packet()
            {
                let _ = self.send(Event::Boss(p));
            }
        } else if !valid && f.bar_player {
            f.bar_player = false;
            let (vis, bar) = (f.bar_visible, f.bar);
            if vis {
                let _ = self.send(Event::Boss(BossEvent { id: bar, op: BossOp::Remove }));
            }
        }
    }

    fn fbar_set_progress(&mut self, p: f32) {
        let Some(f) = self.fight() else { return };
        if f.bar_progress == p {
            return;
        }
        f.bar_progress = p;
        let (show, bar) = (f.bar_player && f.bar_visible, f.bar);
        if show {
            let _ = self.send(Event::Boss(BossEvent { id: bar, op: BossOp::Progress(p) }));
        }
    }

    /// Parked active dimension (player gone elsewhere): its boss bar is removed; silent
    /// exchanges (saving parked dimensions) do not touch it.
    pub(crate) fn end_on_park(&mut self) {
        if self.muted {
            return;
        }
        let Some(f) = self.fight() else { return };
        if !f.bar_player {
            return;
        }
        f.bar_player = false;
        let (vis, bar) = (f.bar_visible, f.bar);
        if vis {
            let _ = self.send(Event::Boss(BossEvent { id: bar, op: BossOp::Remove }));
        }
    }

    /// Bar progress, dragon seen, own name.
    pub(crate) fn fight_update_dragon(&mut self, m: &super::super::mob::Mob) {
        let Some(f) = self.mobs.end.fight.as_ref() else { return };
        if f.dragon_uuid != Some(m.uuid) {
            return;
        }
        self.fbar_set_progress(m.health / m.max_health());
        let name = m.custom_name.clone();
        let Some(f) = self.fight() else { return };
        f.ticks_since_dragon_seen = 0;
        if let Some(n) = name
            && f.bar_name.as_ref() != Some(&n)
        {
            f.bar_name = Some(n.clone());
            let (show, bar) = (f.bar_player && f.bar_visible, f.bar);
            if show {
                let _ = self.send(Event::Boss(BossEvent { id: bar, op: BossOp::Name(Some(n)) }));
            }
        }
    }

    // ---------- State ----------

    /// An End portal block entity (gateways included) in chunks −8..8
    /// (absolute coordinates).
    pub(crate) fn has_active_exit_portal(&self) -> bool {
        for x in -8..=8 {
            for z in -8..=8 {
                let Some(Some(c)) = self.chunks.get(&(x, z)) else { continue };
                if c.block_entities.iter().any(portal_be) {
                    return true;
                }
            }
        }
        false
    }

    /// Is the exit portal pattern present around `c` (center at the ring
    /// level)?
    fn exit_pattern_at(&self, c: [i32; 3]) -> bool {
        let bedrock = self.default_state("minecraft:bedrock");
        for (li, rows) in (0..5).map(|i| (i, layer(i))) {
            let y = c[1] + 3 - li as i32;
            for (ri, row) in rows.iter().enumerate() {
                for (ci, ch) in row.bytes().enumerate() {
                    if ch == b'#' && self.get([c[0] + ci as i32 - 3, y, c[2] + ri as i32 - 3]) != bedrock {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Center found (and noted if it was not).
    fn find_exit_portal(&mut self) -> Option<[i32; 3]> {
        let f = self.mobs.end.fight.as_ref()?;
        let o = f.origin;
        let (cx, cz) = (o[0] >> 4, o[2] >> 4);
        let mut found = None;
        'chunks: for x in -8 + cx..=8 + cx {
            for z in -8 + cz..=8 + cz {
                let Some(Some(c)) = self.chunks.get(&(x, z)) else { continue };
                let portals: Vec<[i32; 3]> = c
                    .block_entities
                    .iter()
                    .filter(|b| portal_be(b))
                    .filter_map(|b| Some([int(b, "x")? as i32, int(b, "y")? as i32, int(b, "z")? as i32]))
                    .collect();
                for p in portals {
                    for dx in -3..=3 {
                        for dz in -3..=3 {
                            let cand = [p[0] + dx, p[1], p[2] + dz];
                            if self.exit_pattern_at(cand) {
                                found = Some(cand);
                                break 'chunks;
                            }
                        }
                    }
                }
            }
        }
        if found.is_none() {
            let top = self.hm_motion_blocking(o[0], o[2]);
            let min = self.dim.height.min_y;
            for y in (min..=top).rev() {
                if self.exit_pattern_at([o[0], y, o[2]]) {
                    found = Some([o[0], y, o[2]]);
                    break;
                }
            }
        }
        if let Some(c) = found
            && let Some(f) = self.fight()
            && f.exit_portal.is_none()
        {
            f.exit_portal = Some(c);
            f.dirty = true;
        }
        found
    }

    /// First pass (old or new world).
    fn fight_scan_state(&mut self) {
        let active = self.has_active_exit_portal();
        if active {
            if let Some(f) = self.fight() {
                f.previously_killed = true;
            }
        } else {
            if let Some(f) = self.fight() {
                f.previously_killed = false;
            }
            if self.find_exit_portal().is_none() {
                self.spawn_exit_portal(false);
            }
        }
        let dragon =
            self.mobs.of_kind(Kind::EnderDragon).find(|o| o.alive()).map(|o| (o.id, o.uuid));
        match dragon {
            None => {
                if let Some(f) = self.fight() {
                    f.dragon_killed = true;
                }
            }
            Some((id, uuid)) => {
                if let Some(f) = self.fight() {
                    f.dragon_uuid = Some(uuid);
                    f.dragon_killed = false;
                }
                if !active {
                    if let Some(i) = self.mob_index(id)
                        && let Some(o) = self.mobs.list[i].as_mut()
                    {
                        o.removed = true;
                    }
                    if let Some(f) = self.fight() {
                        f.dragon_uuid = None;
                    }
                }
            }
        }
        if let Some(f) = self.fight() {
            if !f.previously_killed && f.dragon_killed {
                f.dragon_killed = false;
            }
            f.dirty = true;
        }
    }

    fn fight_find_or_create_dragon(&mut self) {
        let found = self.mobs.of_kind(Kind::EnderDragon).find(|o| o.alive()).map(|o| o.uuid);
        match found {
            None => {
                self.fight_create_dragon();
            }
            Some(u) => {
                if let Some(f) = self.fight() {
                    f.dragon_uuid = Some(u);
                    f.dirty = true;
                }
            }
        }
    }

    /// At (origin, 128), yaw drawn from the world random, circling flight.
    fn fight_create_dragon(&mut self) -> Option<i32> {
        let o = self.mobs.end.fight.as_ref()?.origin;
        let pos = [f64::from(o[0]), f64::from(128 + o[1]), f64::from(o[2])];
        self.ensure_loaded(&[(o[0] >> 4, o[2] >> 4)]);
        let mut m = self.new_mob(Kind::EnderDragon, pos, 0.0);
        if let Some(d) = m.end.dragon.as_mut() {
            d.fight = true;
            d.origin = o;
            d.set_phase(super::phases::Phase::HoldingPattern);
        }
        let yaw = self.mobs.rng.next_float() * 360.0;
        m.yaw = super::super::mth::wrap(yaw);
        m.head_yaw = m.yaw;
        m.body_yaw = m.yaw;
        let (id, uuid) = (m.id, m.uuid);
        self.add_mob(m);
        if let Some(f) = self.fight() {
            f.dragon_uuid = Some(uuid);
            f.dirty = true;
        }
        Some(id)
    }

    /// Crystals in the column of each pillar of the world.
    pub(crate) fn fight_update_crystal_count(&mut self) {
        let spikes = rc_worldgen::feature_end::world_spikes(self.world_seed());
        let mut n = 0;
        for s in spikes {
            let r = f64::from(s.radius);
            let b = rc_physics::Aabb {
                min: [f64::from(s.x) - r, -2032.0, f64::from(s.z) - r],
                max: [f64::from(s.x) + r, 2031.0, f64::from(s.z) + r],
            };
            n += self.mobs.end.crystals.iter().filter(|c| !c.removed && c.bbox().intersects(&b)).count() as i32;
        }
        if let Some(f) = self.fight() {
            f.alive_crystals = n;
            f.ticks_since_crystals = 0;
        }
    }

    /// Active portal, gateway, egg at the first dragon.
    pub(crate) fn fight_dragon_killed(&mut self, uuid: u128) {
        let Some(f) = self.mobs.end.fight.as_ref() else { return };
        if f.dragon_uuid != Some(uuid) {
            return;
        }
        self.fbar_set_progress(0.0);
        self.fbar_set_visible(false);
        self.spawn_exit_portal(true);
        self.spawn_new_gateway();
        let (first, o) = {
            let f = self.mobs.end.fight.as_ref().expect("combat");
            (!f.previously_killed, f.origin)
        };
        if first {
            let y = self.hm_motion_blocking(o[0], o[2]);
            let egg = self.default_state("minecraft:dragon_egg");
            self.set_block([o[0], y, o[2]], egg, flag::ALL);
        }
        if let Some(f) = self.fight() {
            f.previously_killed = true;
            f.dragon_killed = true;
            f.dirty = true;
        }
    }

    /// Last gateway of the list, on a circle of radius 96 at y 75.
    fn spawn_new_gateway(&mut self) {
        let Some(g) = self.fight().and_then(|f| f.gateways.pop()) else { return };
        let a = 2.0 * (-std::f64::consts::PI + std::f64::consts::PI / 20.0 * f64::from(g));
        let x = (96.0 * a.cos()).floor() as i32;
        let z = (96.0 * a.sin()).floor() as i32;
        let o = [x, 75, z];
        self.level_event(3000, o, 0);
        self.ensure_loaded(&[(x >> 4, z >> 4)]);
        let gw = self.default_state("minecraft:end_gateway");
        let bedrock = self.default_state("minecraft:bedrock");
        let air = self.default_state("minecraft:air");
        for p in crate::portal_shape::between_closed([o[0] - 1, o[1] - 2, o[2] - 1], [o[0] + 1, o[1] + 2, o[2] + 1]) {
            let (sx, sy, sz) = (p[0] == o[0], p[1] == o[1], p[2] == o[2]);
            let end = (p[1] - o[1]).abs() == 2;
            let s = if sx && sy && sz {
                gw
            } else if sy {
                air
            } else if (end && sx && sz) || ((sx || sz) && !end) {
                bedrock
            } else {
                air
            };
            self.set_block(p, s, flag::ALL);
        }
        if let Some(f) = self.fight() {
            f.dirty = true;
        }
    }

    /// Location found once (under the top of the podium, above
    /// the bedrock), then podium placed (active: portal, replaced blocks dropping their loot).
    fn spawn_exit_portal(&mut self, active: bool) {
        let Some(f) = self.mobs.end.fight.as_ref() else { return };
        let o = f.origin;
        let loc = match f.exit_portal {
            Some(p) => p,
            None => {
                let mut p = [o[0], self.hm_no_leaves(o[0], o[2]) - 1, o[2]];
                let bedrock = self.default_state("minecraft:bedrock");
                while self.get(p) == bedrock && p[1] > 63 {
                    p[1] -= 1;
                }
                p[1] = p[1].max(self.dim.height.min_y + 1);
                if let Some(f) = self.fight() {
                    f.exit_portal = Some(p);
                    f.dirty = true;
                }
                p
            }
        };
        self.place_podium(loc, active);
    }

    /// Podium.
    fn place_podium(&mut self, o: [i32; 3], active: bool) {
        let bedrock = self.default_state("minecraft:bedrock");
        let end_stone = self.default_state("minecraft:end_stone");
        let air = self.default_state("minecraft:air");
        let portal = self.default_state("minecraft:end_portal");
        let set = |s: &mut Server, p: [i32; 3], st: u32, drop: bool| {
            if drop {
                if s.get(p) != st {
                    s.destroy_block(p, true, 512);
                    s.set_block(p, st, flag::ALL);
                }
            } else {
                s.set_block(p, st, flag::ALL);
            }
        };
        for p in crate::portal_shape::between_closed([o[0] - 4, o[1] - 1, o[2] - 4], [o[0] + 4, o[1] + 32, o[2] + 4]) {
            let d2 = f64::from((p[0] - o[0]).pow(2) + (p[1] - o[1]).pow(2) + (p[2] - o[2]).pow(2));
            let inside = d2 < 2.5 * 2.5;
            if !(inside || d2 < 3.5 * 3.5) {
                continue;
            }
            if p[1] < o[1] {
                if inside {
                    set(self, p, bedrock, false);
                } else {
                    set(self, p, end_stone, active);
                }
            } else if p[1] > o[1] {
                set(self, p, air, active);
            } else if !inside {
                set(self, p, bedrock, false);
            } else if active {
                set(self, p, portal, true);
            } else {
                set(self, p, air, false);
            }
        }
        for y in 0..4 {
            self.set_block([o[0], o[1] + y, o[2]], bedrock, flag::ALL);
        }
        let c = [o[0], o[1] + 2, o[2]];
        let torch = self.default_state("minecraft:wall_torch");
        for (d, step) in [("north", [0, 0, -1]), ("east", [1, 0, 0]), ("south", [0, 0, 1]), ("west", [-1, 0, 0])] {
            let st = self.with_prop(torch, "facing", d).unwrap_or(torch);
            self.set_block([c[0] + step[0], c[1], c[2] + step[2]], st, flag::ALL);
        }
    }

    /// Crystal destroyed: respawn interrupted if it is one of its
    /// crystals, otherwise recount and dragon notified (player author, otherwise the closest).
    pub(crate) fn fight_crystal_destroyed(&mut self, crystal: i32, by: Option<Ent>) {
        let Some(f) = self.mobs.end.fight.as_ref() else { return };
        let uuid = self.mobs.end.crystals.iter().find(|c| c.id == crystal).map(|c| c.uuid);
        if f.stage.is_some() && uuid.is_some_and(|u| f.respawn_crystals.contains(&u)) {
            self.fight_abort_respawn();
            return;
        }
        self.fight_update_crystal_count();
        let Some(du) = self.mobs.end.fight.as_ref().and_then(|f| f.dragon_uuid) else { return };
        let Some(did) = self.mobs.find_uuid_where(du, |o| o.kind == Kind::EnderDragon).map(|o| o.id) else {
            return;
        };
        let player = if by == Some(Ent::Player) {
            Some(Ent::Player)
        } else {
            (self.ent_alive(Ent::Player)
                && self.game_mode != 1
                && self.game_mode != 3
                && self.level.difficulty_id() != 0)
                .then_some(Ent::Player)
        };
        self.dragon_crystal_destroyed(did, crystal, player);
    }

    fn fight_abort_respawn(&mut self) {
        if let Some(f) = self.fight() {
            f.stage = None;
            f.respawn_time = 0;
            f.dirty = true;
        }
        self.fight_reset_spike_crystals();
        self.spawn_exit_portal(true);
    }

    /// Pillar crystals: neither invulnerable nor beam.
    fn fight_reset_spike_crystals(&mut self) {
        let spikes = rc_worldgen::feature_end::world_spikes(self.world_seed());
        for s in spikes {
            let r = f64::from(s.radius);
            let b = rc_physics::Aabb {
                min: [f64::from(s.x) - r, -2032.0, f64::from(s.z) - r],
                max: [f64::from(s.x) + r, 2031.0, f64::from(s.z) + r],
            };
            for c in self.mobs.end.crystals.iter_mut().filter(|c| !c.removed && c.bbox().intersects(&b)) {
                c.invulnerable = false;
                c.set_beam(None);
            }
        }
    }

    /// The respawn attempt (crystal placed): dragon killed, no respawn in progress, a crystal on
    /// each of the four faces of the portal (3 blocks from the center, one block above).
    pub(crate) fn fight_try_respawn(&mut self) {
        let Some(f) = self.mobs.end.fight.as_ref() else { return };
        if !f.dragon_killed || f.stage.is_some() {
            return;
        }
        let loc = match f.exit_portal {
            Some(p) => p,
            None => {
                if self.find_exit_portal().is_none() {
                    self.spawn_exit_portal(true);
                }
                match self.mobs.end.fight.as_ref().and_then(|f| f.exit_portal) {
                    Some(p) => p,
                    None => return,
                }
            }
        };
        let center = [loc[0], loc[1] + 1, loc[2]];
        let mut found: Vec<u128> = Vec::new();
        for step in [[0, 0, -3], [3, 0, 0], [0, 0, 3], [-3, 0, 0]] {
            let p = [center[0] + step[0], center[1], center[2] + step[2]];
            let b = rc_physics::Aabb {
                min: [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])],
                max: [f64::from(p[0] + 1), f64::from(p[1] + 1), f64::from(p[2] + 1)],
            };
            let here: Vec<u128> = self
                .mobs
                .end
                .crystals
                .iter()
                .filter(|c| !c.removed && c.bbox().intersects(&b))
                .map(|c| c.uuid)
                .collect();
            if here.is_empty() {
                return;
            }
            found.extend(here);
        }
        self.fight_respawn_dragon(found);
    }

    fn fight_respawn_dragon(&mut self, crystals: Vec<u128>) {
        let Some(f) = self.mobs.end.fight.as_ref() else { return };
        if !f.dragon_killed || f.stage.is_some() {
            return;
        }
        let end_stone = self.default_state("minecraft:end_stone");
        let bedrock = self.default_state("minecraft:bedrock");
        let portal = self.default_state("minecraft:end_portal");
        while let Some(c) = self.find_exit_portal() {
            for li in 0..5 {
                let y = c[1] + 3 - li;
                for dz in -3..=3 {
                    for dx in -3..=3 {
                        let p = [c[0] + dx, y, c[2] + dz];
                        let s = self.get(p);
                        if s == bedrock || s == portal {
                            self.set_block(p, end_stone, flag::ALL);
                        }
                    }
                }
            }
        }
        if let Some(f) = self.fight() {
            f.stage = Some(Stage::Start);
            f.respawn_time = 0;
        }
        self.spawn_exit_portal(false);
        if let Some(f) = self.fight() {
            f.respawn_crystals = crystals;
            f.dirty = true;
        }
    }

    /// Time reset to 0; the end creates the new dragon.
    fn fight_set_stage(&mut self, stage: Stage) {
        if let Some(f) = self.fight() {
            f.respawn_time = 0;
            if stage == Stage::End {
                f.stage = None;
                f.dragon_killed = false;
            } else {
                f.stage = Some(stage);
            }
            f.dirty = true;
        }
        if stage == Stage::End
            && let Some(id) = self.fight_create_dragon()
            && self.mobs.end.fight.as_ref().is_some_and(|f| f.bar_player)
        {
            // Fight players (boss bar): `summoned_entity`.
            self.trig_summoned(crate::progress::pred::Who::E(crate::commands::ents::E::Mob(id)));
        }
    }

    /// Tick of a respawn stage.
    fn respawn_stage_tick(&mut self, stage: Stage, crystals: &[i32], time: i32) {
        let beam_all = |s: &mut Server, b: Option<[i32; 3]>| {
            for id in crystals {
                if let Some(c) = s.mobs.end.crystals.iter_mut().find(|c| c.id == *id) {
                    c.set_beam(b);
                }
            }
        };
        let top = [0, 128, 0];
        match stage {
            Stage::Start => {
                beam_all(self, Some(top));
                self.fight_set_stage(Stage::PreparingToSummonPillars);
            }
            Stage::PreparingToSummonPillars => {
                if time < 100 {
                    if time == 0 || time == 50 || time == 51 || time == 52 || time >= 95 {
                        self.level_event(3001, top, 0);
                    }
                } else {
                    self.fight_set_stage(Stage::SummoningPillars);
                }
            }
            Stage::SummoningPillars => {
                let start = time % 40 == 0;
                let end = time % 40 == 39;
                if start || end {
                    let spikes = rc_worldgen::feature_end::world_spikes(self.world_seed());
                    let index = (time / 40) as usize;
                    if index < spikes.len() {
                        let s = spikes[index];
                        if start {
                            beam_all(self, Some([s.x, s.height + 1, s.z]));
                        } else {
                            for p in crate::portal_shape::between_closed(
                                [s.x - 10, s.height - 10, s.z - 10],
                                [s.x + 10, s.height + 10, s.z + 10],
                            ) {
                                self.remove_block(p, false);
                            }
                            let c = [f64::from(s.x as f32 + 0.5), f64::from(s.height), f64::from(s.z as f32 + 0.5)];
                            self.game_event_src("minecraft:explode", c, None, None);
                            self.explode_with(
                                None,
                                None,
                                None,
                                c,
                                5.0,
                                false,
                                "minecraft:explosion",
                                super::super::explode::Blast::Block,
                            );
                            self.place_spike_live(s, true, Some(top));
                        }
                    } else if start {
                        self.fight_set_stage(Stage::SummoningDragon);
                    }
                }
            }
            Stage::SummoningDragon => {
                if time >= 100 {
                    self.fight_set_stage(Stage::End);
                    self.fight_reset_spike_crystals();
                    for id in crystals {
                        let Some(i) = self.mobs.end.crystals.iter().position(|c| c.id == *id) else { continue };
                        self.mobs.end.crystals[i].set_beam(None);
                        let pos = self.mobs.end.crystals[i].pos;
                        // The crystal is the source: game event emitted by it, and its
                        // own breath does not hit it (the other crystals, yes).
                        self.game_event_src("minecraft:explode", pos, Some(crate::mobs::recent::vibration::Src::Other), None);
                        self.explode_with(
                            None,
                            None,
                            Some(*id),
                            pos,
                            6.0,
                            false,
                            "minecraft:explosion",
                            super::super::explode::Blast::None,
                        );
                        self.mobs.end.crystals[i].removed = true;
                    }
                } else if time >= 80 {
                    self.level_event(3001, top, 0);
                } else if time == 0 {
                    beam_all(self, Some(top));
                } else if time < 5 {
                    self.level_event(3001, top, 0);
                }
            }
            Stage::End => {}
        }
    }

    /// Pillar rebuilt (in the world): obsidian, air
    /// above 65, bar cage if guarded, crystal (invulnerable, beam), bedrock and
    /// fire.
    fn place_spike_live(&mut self, s: rc_worldgen::feature_end::Spike, invulnerable: bool, beam: Option<[i32; 3]>) {
        let obsidian = self.default_state("minecraft:obsidian");
        let air = self.default_state("minecraft:air");
        let r = s.radius;
        let min = self.dim.height.min_y;
        for p in crate::portal_shape::between_closed([s.x - r, min, s.z - r], [s.x + r, s.height + 10, s.z + r]) {
            let (dx, dz) = (f64::from(p[0] - s.x), f64::from(p[2] - s.z));
            if dx * dx + dz * dz <= f64::from(r * r + 1) && p[1] < s.height {
                self.set_block(p, obsidian, flag::ALL);
            } else if p[1] > 65 {
                self.set_block(p, air, flag::ALL);
            }
        }
        if s.guarded {
            let bars = self.default_state("minecraft:iron_bars");
            for dx in -2..=2i32 {
                for dz in -2..=2i32 {
                    for dy in 0..=3 {
                        let (xs, zs, top) = (dx.abs() == 2, dz.abs() == 2, dy == 3);
                        if !(xs || zs || top) {
                            continue;
                        }
                        let xe = dx == -2 || dx == 2 || top;
                        let ze = dz == -2 || dz == 2 || top;
                        let b = |v: bool| if v { "true" } else { "false" };
                        let mut st = bars;
                        for (k, v) in [
                            ("north", xe && dz != -2),
                            ("south", xe && dz != 2),
                            ("west", ze && dx != -2),
                            ("east", ze && dx != 2),
                        ] {
                            st = self.with_prop(st, k, b(v)).unwrap_or(st);
                        }
                        self.set_block([s.x + dx, s.height + dy, s.z + dz], st, flag::ALL);
                    }
                }
            }
        }
        let yaw = self.mobs.rng.next_float() * 360.0;
        let pos = [f64::from(s.x) + 0.5, f64::from(s.height + 1), f64::from(s.z) + 0.5];
        let id = self.spawn_crystal(pos, yaw, true);
        if let Some(c) = self.mobs.end.crystals.iter_mut().find(|c| c.id == id) {
            c.invulnerable = invulnerable;
            c.beam = beam;
        }
        let cp = super::super::mth::block_of(pos);
        self.set_block([cp[0], cp[1] - 1, cp[2]], self.default_state("minecraft:bedrock"), flag::ALL);
        let fire = self.fire_state_for(cp);
        self.set_block(cp, fire, flag::ALL);
    }

    /// Writing of the fight state (if it changed).
    pub(crate) fn fight_save_jobs(&mut self, jobs: &mut Vec<crate::save::Job>) {
        let dir = self.dim.dir.clone();
        let Some(f) = self.fight() else { return };
        if !f.dirty {
            return;
        }
        f.dirty = false;
        let path = dir.join("data").join("minecraft").join("ender_dragon_fight.dat");
        jobs.push(crate::save::Job::Dat { path, nbt: nbtio::encode_root(&f.to_nbt()), backup: false });
    }

    /// The dragon of a fight ticks in the arena (fight ticket: 7 chunks from the origin)
    /// even far from the player's simulated zone.
    pub(crate) fn fight_forces_ticking(&self, m: &super::super::mob::Mob) -> bool {
        if m.kind != Kind::EnderDragon {
            return false;
        }
        let Some(f) = self.mobs.end.fight.as_ref() else { return false };
        if !f.bar_player {
            return false;
        }
        let c = ((m.pos[0].floor() as i32) >> 4, (m.pos[2].floor() as i32) >> 4);
        let o = (f.origin[0] >> 4, f.origin[2] >> 4);
        (c.0 - o.0).abs() <= 7 && (c.1 - o.1).abs() <= 7 && self.chunks.get(&c).is_some_and(Option::is_some)
    }
}

#[cfg(test)]
mod summon_tests {
    use super::{Fight, Stage};

    /// G1-F02: dragon respawned, player in the fight bar: `summoned_entity`
    /// ("The End... Again..."); test world absent: test skipped.
    #[test]
    fn respawned_dragon_summons() {
        let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else { return };
        let dir = crate::tests::fresh_copy(&world, "g1f02-dragon");
        let mut p = crate::blocks::probe::Probe::open(&dir, std::sync::Arc::new(data));
        p.s.loaded = true;
        p.s.game_mode = 0;
        let mut f = Fight::read(None);
        f.bar_player = true;
        p.s.mobs.end.fight = Some(Box::new(f));
        p.s.mobs.end.fight_checked = true;
        p.s.fight_set_stage(Stage::End);
        assert!(p.s.mobs.list.iter().flatten().any(|m| m.kind == super::Kind::EnderDragon), "dragon not created");
        let i = p.s.prog.tree.get("minecraft:end/respawn_dragon").expect("progress");
        assert!(p.s.prog.done(i), "summoned_entity missing");
        drop(p);
        let _ = std::fs::remove_dir_all(dir);
    }
}
