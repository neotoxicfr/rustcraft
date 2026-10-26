//! Copper golem (§ 4.5): brain (sensors, core and idle behaviors), sorting of the items of
//! copper chests into chests, stepwise oxidation up to the statue, wax and axe, lightning,
//! shearing of the antenna poppy, sounds by oxidation.
//!
//! Brain order: memories forgotten, sensors (nearby living, hurt),
//! start of the stopped behaviors by priority (core then idle at equal priority), tick of the
//! running behaviors in the same order.

use rc_proto::nbt::Nbt;

use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::super::mth::{block_of, dist2};
use super::super::species::Kind;
use crate::player::Stack;
use crate::server::Server;

/// Sounds: head rotation, hurt, death, steps (by oxidation).
pub fn sounds(weather: i32) -> [&'static str; 4] {
    match weather {
        2 => [
            "minecraft:entity.copper_golem_weathered.spin",
            "minecraft:entity.copper_golem_weathered.hurt",
            "minecraft:entity.copper_golem_weathered.death",
            "minecraft:entity.copper_golem_weathered.step",
        ],
        3 => [
            "minecraft:entity.copper_golem_oxidized.spin",
            "minecraft:entity.copper_golem_oxidized.hurt",
            "minecraft:entity.copper_golem_oxidized.death",
            "minecraft:entity.copper_golem_oxidized.step",
        ],
        _ => [
            "minecraft:entity.copper_golem.spin",
            "minecraft:entity.copper_golem.hurt",
            "minecraft:entity.copper_golem.death",
            "minecraft:entity.copper_golem.step",
        ],
    }
}

/// Walk target (`WalkTarget`): cell, speed, sufficient Manhattan distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Walk {
    pub pos: [i32; 3],
    pub speed: f32,
    pub close: i32,
}

/// Gaze target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Look {
    Block([i32; 3]),
    Entity(Ent),
}

/// Behavior with a duration: running, end.
#[derive(Debug, Clone, Copy, Default)]
pub struct Run {
    pub running: bool,
    pub end: i64,
}

/// Transport state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TState {
    #[default]
    Travelling,
    Queuing,
    Interacting,
}

#[derive(Debug, Clone, Default)]
pub struct Transport {
    pub run: Run,
    /// Target: position, block name as chosen (block entity unchanged).
    pub target: Option<([i32; 3], String)>,
    pub state: TState,
    /// Interaction: 0 pickup, 1 failed pickup, 2 drop-off, 3 failed drop-off.
    pub interaction: Option<u8>,
    pub ticks: i32,
}

#[derive(Debug, Clone)]
pub struct Brain {
    // Sensors (delay before the next sweep).
    pub sense_living: i64,
    pub sense_hurt: i64,
    pub visible: Vec<Ent>,
    pub hurt_by: Option<String>,
    pub hurt_by_entity: Option<Ent>,
    // Memories.
    pub walk: Option<Walk>,
    pub look: Option<Look>,
    pub path: bool,
    pub cant_reach_since: Option<i64>,
    pub panicking: bool,
    pub transport_cd: Option<i32>,
    pub visited: Vec<[i32; 3]>,
    pub visited_ttl: i64,
    pub unreachable: Vec<[i32; 3]>,
    pub unreachable_ttl: i64,
    // Behaviors.
    pub panic: Run,
    pub look_sink: Run,
    pub move_sink: Run,
    pub move_cooldown: i32,
    pub move_path: bool,
    pub move_last: Option<[i32; 3]>,
    pub move_speed: f32,
    pub cd_transport: bool,
    pub transport: Transport,
    pub look_ticker: i32,
    pub run_one: bool,
    /// Shuffled order of "one of": 0 wander, 1 nothing.
    pub run_order: [u8; 2],
    pub do_nothing: Run,
    /// Own random of the shuffled list (clock seed).
    pub shuffle: JRand,
    /// Path kept by the walk toward the target.
    pub pending: Option<super::super::nav::Path>,
}

impl Default for Brain {
    fn default() -> Brain {
        Brain {
            sense_living: 0,
            sense_hurt: 0,
            visible: Vec::new(),
            hurt_by: None,
            hurt_by_entity: None,
            walk: None,
            look: None,
            path: false,
            cant_reach_since: None,
            panicking: false,
            transport_cd: None,
            visited: Vec::new(),
            visited_ttl: 0,
            unreachable: Vec::new(),
            unreachable_ttl: 0,
            panic: Run::default(),
            look_sink: Run::default(),
            move_sink: Run::default(),
            move_cooldown: 0,
            move_path: false,
            move_last: None,
            move_speed: 0.0,
            cd_transport: false,
            transport: Transport::default(),
            look_ticker: 0,
            run_one: false,
            run_order: [0, 1],
            do_nothing: Run::default(),
            shuffle: JRand::from_time(0x00C0_FFEE),
            pending: None,
        }
    }
}

/// New brain: random delays of the two sensors (entity random).
fn new_brain(m: &mut Mob) -> Brain {
    let sense_living = i64::from(m.rng.next_int_bound(20));
    let sense_hurt = i64::from(m.rng.next_int_bound(20));
    Brain { sense_living, sense_hurt, shuffle: JRand::from_time(i64::from(m.id)), ..Brain::default() }
}

/// Construction: brain, persistence, "idle" state, transport cooldown 60 to 99 ticks.
pub fn on_new(m: &mut Mob) {
    m.golem.brain = new_brain(m);
    m.persistent = true;
    m.golem.state = 0;
    m.golem.next_weather = -1;
    m.golem.brain.transport_cd = Some(60 + m.rng.next_int_bound(40));
}

/// Reload: the brain is rebuilt from the NBT (new sensor delays, the constructor's cooldown
/// is lost).
pub fn load_brain(m: &mut Mob, e: &Nbt) {
    m.golem.brain = new_brain(m);
    let mem = e.get("Brain").and_then(|b| b.get("memories"));
    let Some(mem) = mem else { return };
    if let Some(v) = mem.get("minecraft:is_panicking").and_then(|v| v.get("value")) {
        m.golem.brain.panicking = v.as_i64().is_some_and(|b| b != 0);
    }
    let read = |k: &str| -> (Vec<[i32; 3]>, i64) {
        let Some(slot) = mem.get(k) else { return (Vec::new(), 0) };
        let ttl = slot.get("ttl").and_then(Nbt::as_i64).unwrap_or(i64::MAX);
        let list = match slot.get("value") {
            Some(Nbt::List(l)) => l
                .iter()
                .filter_map(|g| match g.get("pos") {
                    Some(Nbt::IntArray(a)) if a.len() == 3 => Some([a[0], a[1], a[2]]),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        (list, ttl)
    };
    (m.golem.brain.visited, m.golem.brain.visited_ttl) = read("minecraft:visited_block_positions");
    (m.golem.brain.unreachable, m.golem.brain.unreachable_ttl) =
        read("minecraft:unreachable_transport_block_positions");
    if let Some(Nbt::Compound(d)) = e.get("drop_chances") {
        for (k, v) in d {
            if let (Some(i), Some(c)) = (super::super::mob::slot_index(k), super::super::mob::f64_of(v)) {
                m.drop_chance[i] = c as f32;
            }
        }
    }
}

/// Serializable memories of the brain, non-default drop chances.
pub fn save_brain(m: &Mob, e: &mut Nbt) {
    let b = &m.golem.brain;
    let mut mems: Vec<(String, Nbt)> = Vec::new();
    if b.panicking {
        mems.push(("minecraft:is_panicking".into(), Nbt::Compound(vec![("value".into(), Nbt::Byte(1))])));
    }
    let list = |v: &[[i32; 3]], ttl: i64| {
        let l: Vec<Nbt> = v
            .iter()
            .map(|p| {
                Nbt::Compound(vec![
                    ("dimension".into(), Nbt::String("minecraft:overworld".into())),
                    ("pos".into(), Nbt::IntArray(p.to_vec())),
                ])
            })
            .collect();
        Nbt::Compound(vec![("value".into(), Nbt::List(l)), ("ttl".into(), Nbt::Long(ttl))])
    };
    if !b.visited.is_empty() {
        mems.push(("minecraft:visited_block_positions".into(), list(&b.visited, b.visited_ttl)));
    }
    if !b.unreachable.is_empty() {
        mems.push(("minecraft:unreachable_transport_block_positions".into(), list(&b.unreachable, b.unreachable_ttl)));
    }
    crate::nbtio::set(e, "Brain", Nbt::Compound(vec![("memories".into(), Nbt::Compound(mems))]));
    let chances: Vec<(String, Nbt)> = m
        .drop_chance
        .iter()
        .enumerate()
        .filter(|(_, c)| (**c - 0.085).abs() > f32::EPSILON)
        .map(|(i, c)| (super::super::mob::SLOT_NAMES[i].to_owned(), Nbt::Float(*c)))
        .collect();
    if chances.is_empty() {
        crate::nbtio::remove(e, "drop_chances");
    } else {
        crate::nbtio::set(e, "drop_chances", Nbt::Compound(chances));
    }
}

const STATUE_POSES: [&str; 4] = ["standing", "sitting", "running", "star"];

/// Oxidation (world random) then possible statue once oxidized.
pub fn weathering_tick(s: &mut Server, m: &mut Mob) {
    let g = &mut m.golem;
    if g.next_weather == -2 {
        return;
    }
    if g.next_weather == -1 {
        g.next_weather = s.time + i64::from(s.mobs.rng.int_between(504_000, 552_000));
        return;
    }
    let oxidized = g.weather == 3;
    if s.time >= g.next_weather && !oxidized {
        g.weather += 1;
        g.next_weather =
            if g.weather == 3 { 0 } else { g.next_weather + i64::from(s.mobs.rng.int_between(504_000, 552_000)) };
    }
    if oxidized && s.is_air(m.block_pos()) && s.mobs.rng.next_float() <= 0.0058 {
        turn_to_statue(s, m);
    }
}

fn turn_to_statue(s: &mut Server, m: &mut Mob) {
    let p = m.block_pos();
    let pose = STATUE_POSES[m.rng.next_int_bound(4) as usize];
    let facing = ["south", "west", "north", "east"][super::super::mth::quarter(m.yaw)];
    let v = s.v();
    let mut st = s.default_state("minecraft:oxidized_copper_golem_statue");
    st = v.set(st, "copper_golem_pose", pose);
    st = v.set(st, "facing", facing);
    s.set_block(p, st, 3);
    if let Some(name) = m.custom_name.clone()
        && let Some(be) = s.block_entity_mut(p, "minecraft:copper_golem_statue")
    {
        let comps = Nbt::Compound(vec![("minecraft:custom_name".into(), name)]);
        crate::nbtio::set(be, "components", comps);
    }
    s.drop_preserved_equipment(m);
    m.removed = true;
    s.mob_sound(m, "minecraft:entity.copper_golem_become_statue", 1.0, 1.0);
}

impl Server {
    /// Intact statue hit with an axe: new golem at the center of the block,
    /// turned like the statue, statue name kept, spawn sound. The golem is added
    /// to the world after the axe wears (game order): it is returned to the caller.
    pub(crate) fn statue_golem(&mut self, p: [i32; 3], state: u32) -> Mob {
        let yaw = match self.v().facing(state) {
            crate::blocks::Dir::South => 0.0,
            crate::blocks::Dir::West => 90.0,
            crate::blocks::Dir::North => 180.0,
            _ => 270.0,
        };
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        let mut g = self.new_mob(Kind::CopperGolem, pos, yaw);
        g.head_yaw = yaw;
        g.body_yaw = yaw;
        g.custom_name = self
            .block_entity(p)
            .and_then(|be| be.get("components"))
            .and_then(|c| c.get("minecraft:custom_name"))
            .cloned();
        self.mob_sound(&g, "minecraft:entity.copper_golem.spawn", 1.0, 1.0);
        g
    }
}

/// Right click: give back the carried item (empty hand), antenna shearing, wax, scraping.
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if item.is_empty()
        && let Some(held) = m.equipment[0].take()
    {
        s.throw_item_towards(m, held, s.pos);
        return Some(true);
    }
    if item == "minecraft:shears" && m.equipment[7].as_ref().is_some_and(|i| i.id == "minecraft:poppy") {
        s.entity_sound(m.id, "minecraft:entity.copper_golem.shear", 7, 1.0, 1.0);
        if let Some(a) = m.equipment[7].take() {
            s.drop_at_height(m, a, 1.5);
        }
        s.sheared_by_player(m);
        s.hurt_item(slot, 1);
        return Some(true);
    }
    let axe = s.item_tag(item, "minecraft:axes");
    let b = m.block_pos();
    let center = [f64::from(b[0]) + 0.5, f64::from(b[1]) + 0.5, f64::from(b[2]) + 0.5];
    if item == "minecraft:honeycomb" && m.golem.next_weather != -2 {
        s.level_event(3003, b, 0);
        s.world_sound("minecraft:item.honeycomb.wax_on", 4, center, 1.0, 1.0);
        m.golem.next_weather = -2;
        s.consume_held(slot);
        return Some(true);
    }
    if axe && m.golem.next_weather == -2 {
        s.mob_sound_unmuted(m, "minecraft:item.axe.scrape", 1.0, 1.0);
        s.level_event(3004, b, 0);
        m.golem.next_weather = -1;
        s.hurt_item(slot, 1);
        return Some(true);
    }
    if axe && m.golem.weather != 0 {
        s.mob_sound_unmuted(m, "minecraft:item.axe.scrape", 1.0, 1.0);
        s.level_event(3005, b, 0);
        m.golem.next_weather = -1;
        m.golem.weather -= 1;
        s.hurt_item(slot, 1);
        return Some(true);
    }
    None
}

/// Lightning (a single effect per lightning): one oxidation step removed.
pub fn thunder_hit(m: &mut Mob, bolt: u128) {
    if m.golem.last_bolt == Some(bolt) {
        return;
    }
    m.golem.last_bolt = Some(bolt);
    if m.golem.weather != 0 {
        m.golem.next_weather = -1;
        m.golem.weather -= 1;
    }
}

// ---------- Brain ----------

/// Brain tick.
pub fn brain_tick(s: &mut Server, m: &mut Mob) {
    forget_memories(m);
    sensors(s, m);
    start_behaviors(s, m);
    tick_behaviors(s, m);
}

fn forget_memories(m: &mut Mob) {
    let b = &mut m.golem.brain;
    if !b.visited.is_empty() {
        b.visited_ttl -= 1;
        if b.visited_ttl <= 0 {
            b.visited.clear();
        }
    }
    if !b.unreachable.is_empty() {
        b.unreachable_ttl -= 1;
        if b.unreachable_ttl <= 0 {
            b.unreachable.clear();
        }
    }
}

fn sensors(s: &mut Server, m: &mut Mob) {
    // Shared range of the targeting conditions, rewritten by each sensor that ticks.
    if m.golem.brain.sense_living <= 1 || m.golem.brain.sense_hurt <= 1 {
        s.mobs.pig.sensor_range = m.follow_range();
    }
    // Nearby living (tracking range), sorted by distance; visible = sight + conditions.
    m.golem.brain.sense_living -= 1;
    if m.golem.brain.sense_living <= 0 {
        m.golem.brain.sense_living = 20;
        let f = m.follow_range();
        let bb = m.bbox();
        let area = rc_physics::Aabb { min: bb.min.map(|v| v - f), max: bb.max.map(|v| v + f) };
        let mut list: Vec<(f64, Ent)> = s
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| o.id != m.id && o.alive() && o.touches(&area))
            .map(|o| (dist2(m.pos, o.pos), Ent::Mob(o.id)))
            .collect();
        if s.ent_alive(Ent::Player) && s.player_aabb().intersects(&area) {
            list.push((dist2(m.pos, s.pos), Ent::Player));
        }
        list.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut visible = Vec::new();
        for (_, e) in list {
            if s.targeting_ok(m, e, false, f, true, true) {
                visible.push(e);
            }
        }
        m.golem.brain.visible = visible;
    }
    // Recent hurt (40 ticks).
    m.golem.brain.sense_hurt -= 1;
    if m.golem.brain.sense_hurt <= 0 {
        m.golem.brain.sense_hurt = 20;
        let recent = m.last_hit.as_ref().filter(|h| s.time as u64 <= h.tick + 40).cloned();
        match recent {
            Some(h) => {
                m.golem.brain.hurt_by = Some(h.ty.clone());
                if let Some(e) = h.by {
                    m.golem.brain.hurt_by_entity = Some(e);
                }
            }
            None => m.golem.brain.hurt_by = None,
        }
        if let Some(e) = m.golem.brain.hurt_by_entity
            && !s.ent_alive(e)
        {
            m.golem.brain.hurt_by_entity = None;
        }
    }
}

/// Duration of a behavior: `min + next_int(max + 1 − min)` on the world random.
fn duration(s: &mut Server, min: i32, max: i32) -> i64 {
    i64::from(min + s.mobs.rng.next_int_bound(max + 1 - min))
}

fn start_behaviors(s: &mut Server, m: &mut Mob) {
    let now = s.time;
    // Priority 0: panic (core), transport (idle).
    if !m.golem.brain.panic.running {
        let cause = m.golem.brain.hurt_by.as_deref().is_some_and(|t| s.dmg_tag(t, "minecraft:panic_causes"));
        if cause || m.golem.brain.panicking {
            let d = duration(s, 100, 120);
            m.golem.brain.panic = Run { running: true, end: now + d };
            m.golem.brain.panicking = true;
            m.golem.brain.walk = None;
            m.nav.stop();
        }
    }
    if !m.golem.brain.transport.run.running && m.golem.brain.transport_cd.is_none() && !m.golem.brain.panicking {
        let d = duration(s, 60, 60);
        m.golem.brain.transport.run = Run { running: true, end: now + d };
        m.nav.below_surface = true;
    }
    // Priority 1: gaze (core), gaze at the player sometimes (idle).
    if !m.golem.brain.look_sink.running && m.golem.brain.look.is_some() {
        let d = duration(s, 45, 90);
        m.golem.brain.look_sink = Run { running: true, end: now + d };
    }
    if m.golem.brain.look.is_none() && look_sometimes(s, m) {
        // One-shot trigger: stopped immediately at the behavior tick.
    }
    // Priority 2: walk toward the target (core), "one of" (idle).
    if !m.golem.brain.move_sink.running && !m.golem.brain.path && m.golem.brain.walk.is_some() && move_can_start(s, m) {
        let d = duration(s, 150, 250);
        m.golem.brain.move_sink = Run { running: true, end: now + d };
        move_start(s, m);
    }
    if !m.golem.brain.run_one && m.golem.brain.walk.is_none() && m.golem.brain.transport_cd.is_some() {
        m.golem.brain.run_one = true;
        // Shuffle: weight 1, order by −random (the largest first), stable sort.
        let r0 = m.golem.brain.shuffle.next_float();
        let r1 = m.golem.brain.shuffle.next_float();
        m.golem.brain.run_order = if -f64::from(r1) < -f64::from(r0) { [1, 0] } else { [0, 1] };
        for k in m.golem.brain.run_order {
            if k == 0 {
                // Wander (2 × 2): target set or cleared, always started.
                if m.golem.brain.walk.is_none() {
                    let p = s.land_random_pos(m, 2, 2);
                    m.golem.brain.walk = p.map(|p| Walk { pos: block_of(p), speed: 1.0, close: 0 });
                    break;
                }
            } else {
                let d = duration(s, 30, 60);
                m.golem.brain.do_nothing = Run { running: true, end: now + d };
                break;
            }
        }
    }
    // Priority 5: transport cooldown countdown.
    if !m.golem.brain.cd_transport && m.golem.brain.transport_cd.is_some() {
        let _ = duration(s, 60, 60);
        m.golem.brain.cd_transport = true;
    }
}

fn tick_behaviors(s: &mut Server, m: &mut Mob) {
    let now = s.time;
    // Panic.
    if m.golem.brain.panic.running {
        if now > m.golem.brain.panic.end {
            m.golem.brain.panic.running = false;
            m.golem.brain.panicking = false;
        } else if m.nav.is_done()
            && let Some(p) = s.land_random_pos(m, 5, 4)
        {
            m.golem.brain.walk = Some(Walk { pos: block_of(p), speed: 1.5, close: 0 });
        }
    }
    // Transport (no end of duration).
    if m.golem.brain.transport.run.running {
        if m.golem.brain.transport_cd.is_none() && !m.golem.brain.panicking {
            s.transport_tick(m);
        } else {
            s.transport_stop(m);
        }
    }
    // Gaze.
    if m.golem.brain.look_sink.running {
        let visible = match m.golem.brain.look {
            Some(Look::Block(_)) => true,
            Some(Look::Entity(e)) => s.ent_alive(e) && m.golem.brain.visible.contains(&e),
            None => false,
        };
        if now > m.golem.brain.look_sink.end || !visible {
            m.golem.brain.look_sink.running = false;
            m.golem.brain.look = None;
        } else if let Some(at) = look_pos(s, m.golem.brain.look) {
            s.look_at(m, at, 10.0, 40.0);
        }
    }
    // Walk toward the target.
    if m.golem.brain.move_sink.running {
        let ok = m.golem.brain.move_path
            && m.golem.brain.move_last.is_some()
            && !m.nav.is_done()
            && m.golem.brain.walk.is_some_and(|w| !reached(m, w));
        if now > m.golem.brain.move_sink.end || !ok {
            move_stop(s, m);
        } else {
            move_tick(s, m);
        }
    }
    // "One of": wandering stops immediately, "nothing" at its end.
    if m.golem.brain.run_one {
        if m.golem.brain.do_nothing.running && now > m.golem.brain.do_nothing.end {
            m.golem.brain.do_nothing.running = false;
        }
        if !m.golem.brain.do_nothing.running {
            m.golem.brain.run_one = false;
        }
    }
    // Transport cooldown.
    if m.golem.brain.cd_transport {
        match m.golem.brain.transport_cd {
            Some(t) if t > 0 => m.golem.brain.transport_cd = Some(t - 1),
            _ => {
                m.golem.brain.cd_transport = false;
                m.golem.brain.transport_cd = None;
            }
        }
    }
}

fn look_pos(s: &Server, l: Option<Look>) -> Option<[f64; 3]> {
    match l? {
        Look::Block(p) => Some([f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]),
        Look::Entity(e) => {
            let p = s.ent_pos(e)?;
            Some([p[0], s.ent_eye(e)?, p[2]])
        }
    }
}

/// Gaze at the player at 6 blocks, interval 40 to 80 ticks (world random).
fn look_sometimes(s: &mut Server, m: &mut Mob) -> bool {
    let near = m.golem.brain.visible.iter().copied().find(|e| *e == Ent::Player && dist2(s.pos, m.pos) <= 36.0);
    let Some(t) = near else { return false };
    let b = &mut m.golem.brain;
    if b.look_ticker == 0 {
        b.look_ticker = s.mobs.rng.int_between(40, 80) - 1;
        return false;
    }
    b.look_ticker -= 1;
    if b.look_ticker != 0 {
        return false;
    }
    b.look = Some(Look::Entity(t));
    true
}

fn reached(m: &Mob, w: Walk) -> bool {
    let b = m.block_pos();
    (w.pos[0] - b[0]).abs() + (w.pos[1] - b[1]).abs() + (w.pos[2] - b[2]).abs() <= w.close
}

/// The start conditions of the movement toward the target.
fn move_can_start(s: &mut Server, m: &mut Mob) -> bool {
    if m.golem.brain.move_cooldown > 0 {
        m.golem.brain.move_cooldown -= 1;
        return false;
    }
    let Some(w) = m.golem.brain.walk else { return false };
    let done = reached(m, w);
    if !done && try_compute_path(s, m, w) {
        m.golem.brain.move_last = Some(w.pos);
        return true;
    }
    m.golem.brain.walk = None;
    if done {
        m.golem.brain.cant_reach_since = None;
    }
    false
}

/// Path kept by the behavior (the navigation is not touched).
fn try_compute_path(s: &mut Server, m: &mut Mob, w: Walk) -> bool {
    let path = s.create_path(m, w.pos, 0);
    m.golem.brain.move_speed = w.speed;
    m.golem.brain.move_path = path.is_some();
    m.golem.brain.pending = path.clone();
    if reached(m, w) {
        m.golem.brain.cant_reach_since = None;
    } else {
        let can = path.as_ref().is_some_and(|p| p.reached);
        if can {
            m.golem.brain.cant_reach_since = None;
        } else if m.golem.brain.cant_reach_since.is_none() {
            m.golem.brain.cant_reach_since = Some(s.time);
        }
        if path.is_some() {
            return true;
        }
        let to = [f64::from(w.pos[0]) + 0.5, f64::from(w.pos[1]), f64::from(w.pos[2]) + 0.5];
        if let Some(step) = s.pos_towards(m, 10, 7, to, std::f32::consts::FRAC_PI_2) {
            let p = s.create_path(m, block_of(step), 0);
            m.golem.brain.move_path = p.is_some();
            m.golem.brain.pending = p;
            return m.golem.brain.move_path;
        }
    }
    false
}

fn move_start(s: &mut Server, m: &mut Mob) {
    m.golem.brain.path = true;
    let path = m.golem.brain.pending.clone();
    let speed = f64::from(m.golem.brain.move_speed);
    s.move_to_path(m, path, speed);
}

fn move_tick(s: &mut Server, m: &mut Mob) {
    // The behavior's path follows the navigation's.
    m.golem.brain.move_path = m.nav.path.is_some();
    m.golem.brain.pending = m.nav.path.clone();
    let Some(w) = m.golem.brain.walk else { return };
    let Some(last) = m.golem.brain.move_last else { return };
    let d = [w.pos[0] - last[0], w.pos[1] - last[1], w.pos[2] - last[2]];
    if m.nav.path.is_some() && d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > 4 && try_compute_path(s, m, w) {
        m.golem.brain.move_last = Some(w.pos);
        move_start(s, m);
    }
}

fn move_stop(s: &mut Server, m: &mut Mob) {
    m.golem.brain.move_sink.running = false;
    if let Some(w) = m.golem.brain.walk
        && !reached(m, w)
        && m.nav.stuck
    {
        m.golem.brain.move_cooldown = s.mobs.rng.next_int_bound(40);
    }
    m.nav.stop();
    m.golem.brain.walk = None;
    m.golem.brain.path = false;
    m.golem.brain.move_path = false;
    m.golem.brain.pending = None;
}

// ---------- Item transport ----------

const SOUNDS_REACHED: [Option<&str>; 4] = [
    // Names from the game (pickup: "no_item_*" sounds, sic).
    Some("minecraft:entity.copper_golem.no_item_get"),
    Some("minecraft:entity.copper_golem.no_item_no_get"),
    Some("minecraft:entity.copper_golem.item_drop"),
    Some("minecraft:entity.copper_golem.item_no_drop"),
];

fn is_source(name: &str) -> bool {
    super::build::is_copper_chest(name)
}

fn is_destination(name: &str) -> bool {
    name == "minecraft:chest" || name == "minecraft:trapped_chest"
}

impl Server {
    fn picking(m: &Mob) -> bool {
        m.equipment[0].is_none()
    }

    fn wanted_block(m: &Mob, name: &str) -> bool {
        if Self::picking(m) { is_source(name) } else { is_destination(name) }
    }

    /// Linked half of a double chest.
    fn chest_other(&self, p: [i32; 3]) -> Option<[i32; 3]> {
        let v = self.v();
        let s = self.st(p);
        let t = v.get(s, "type")?;
        if t == "single" {
            return None;
        }
        let f = v.facing(s);
        let d = if t == "left" { f.clockwise() } else { f.counter_clockwise() };
        Some(crate::blocks::rel(p, d))
    }

    /// Chest container (double if linked): halves in the game's order (right first);
    /// `None` if blocked or without a block entity.
    fn chest_halves(&self, p: [i32; 3]) -> Option<Vec<[i32; 3]>> {
        self.block_entity(p)?;
        if self.chest_blocked_at(p) {
            return None;
        }
        let Some(o) = self.chest_other(p) else { return Some(vec![p]) };
        let name = self.state_name(self.st(p)).to_owned();
        let oname = self.state_name(self.st(o)).to_owned();
        let compatible = if is_source(&name) { is_source(&oname) } else { oname == name };
        if !compatible || self.block_entity(o).is_none() {
            return Some(vec![p]);
        }
        if self.chest_blocked_at(o) {
            return None;
        }
        let right_first = self.v().get(self.st(p), "type") == Some("right");
        Some(if right_first { vec![p, o] } else { vec![o, p] })
    }

    fn halves_items(&self, halves: &[[i32; 3]]) -> Vec<Option<Stack>> {
        let mut out = Vec::new();
        for h in halves {
            out.extend(self.block_entity(*h).map_or_else(|| vec![None; 27], |b| crate::containers::read_items(b, 27)));
        }
        out
    }

    fn write_halves(&mut self, halves: &[[i32; 3]], items: &[Option<Stack>]) {
        for (i, h) in halves.iter().enumerate() {
            // Copper chests and chests: "chest" block entity (trapped: its own).
            let be_id = if self.state_name(self.st(*h)) == "minecraft:trapped_chest" {
                "minecraft:trapped_chest"
            } else {
                "minecraft:chest"
            };
            if let Some(be) = self.block_entity_mut(*h, be_id) {
                crate::containers::write_items(be, &items[i * 27..(i + 1) * 27]);
            }
        }
    }

    /// Does another golem (or the player) have this chest open?
    fn chest_in_use(&self, m: &Mob, p: [i32; 3]) -> bool {
        let mut set = vec![p];
        if let Some(o) = self.chest_other(p) {
            set.push(o);
        }
        let golem = self
            .mobs
            .list
            .iter()
            .flatten()
            .any(|o| o.id != m.id && o.golem.opened_chest.is_some_and(|c| set.contains(&c)));
        let player = self.life.menu.as_ref().is_some_and(|mn| mn.block.is_some_and(|b| set.contains(&b)));
        golem || player
    }

    /// Bounds of the target's collision shape, inflated, against the mob's box
    /// centered at `from`.
    fn within_target(&self, m: &Mob, p: [i32; 3], dist: f64, from: [f64; 3]) -> bool {
        let mut boxes = Vec::new();
        self.phys.collision(self.st(p), &mut boxes);
        if boxes.is_empty() {
            return false;
        }
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for b in &boxes {
            for k in 0..3 {
                lo[k] = lo[k].min(b.min[k]);
                hi[k] = hi[k].max(b.max[k]);
            }
        }
        let infl = [dist, 0.5, dist];
        let t = rc_physics::Aabb {
            min: std::array::from_fn(|k| lo[k] - infl[k] + f64::from(p[k])),
            max: std::array::from_fn(|k| hi[k] + infl[k] + f64::from(p[k])),
        };
        let (w, h) = (f64::from(m.width()), f64::from(m.height()));
        let mb = rc_physics::Aabb {
            min: [from[0] - w / 2.0, from[1] - h / 2.0, from[2] - w / 2.0],
            max: [from[0] + w / 2.0, from[1] + h / 2.0, from[2] + w / 2.0],
        };
        t.intersects(&mb)
    }

    fn center_pos(m: &Mob) -> [f64; 3] {
        [m.pos[0], m.pos[1] + f64::from(m.height()) / 2.0, m.pos[2]]
    }

    fn interaction_range(m: &Mob) -> f64 {
        if m.nav.path.as_ref().is_some_and(|p| p.is_done()) { 1.0 } else { 0.5 }
    }

    pub(crate) fn transport_stop(&mut self, m: &mut Mob) {
        self.transport_on_travel(m);
        m.golem.brain.transport.run.running = false;
        m.nav.below_surface = false;
    }

    /// The movement start (golem: chest closed in the state, idle).
    fn transport_on_travel(&mut self, m: &mut Mob) {
        m.golem.opened_chest = None;
        m.golem.state = 0;
        let t = &mut m.golem.brain.transport;
        t.state = TState::Travelling;
        t.interaction = None;
        t.ticks = 0;
    }

    fn stop_targeting(m: &mut Mob) {
        m.golem.brain.transport.ticks = 0;
        m.golem.brain.transport.target = None;
        m.nav.stop();
        m.golem.brain.walk = None;
    }

    fn clear_after_found(m: &mut Mob) {
        Self::stop_targeting(m);
        m.golem.brain.visited.clear();
        m.golem.brain.unreachable.clear();
    }

    fn cooldown_no_target(m: &mut Mob) {
        Self::stop_targeting(m);
        m.golem.brain.transport_cd = Some(140);
        m.golem.brain.visited.clear();
        m.golem.brain.unreachable.clear();
    }

    pub(super) fn stop_in_place(m: &mut Mob) {
        m.nav.stop();
        m.xxa = 0.0;
        m.yya = 0.0;
        // Mob speed (and forward walk) to zero; the movement control's multiplier
        // stays as is: a "go to" armed this tick still advances by one tick.
        m.speed = 0.0;
        m.zza = 0.0;
        m.vel = [0.0, m.vel[1], 0.0];
    }

    pub(crate) fn transport_tick(&mut self, m: &mut Mob) {
        let updated = self.transport_update_invalid(m);
        let Some((p, _)) = m.golem.brain.transport.target.clone() else {
            self.transport_stop(m);
            return;
        };
        if updated {
            return;
        }
        if m.golem.brain.transport.state == TState::Queuing && !self.chest_in_use(m, p) {
            m.golem.brain.transport.state = TState::Travelling;
            Self::walk_towards(m, p);
        }
        if m.golem.brain.transport.state == TState::Travelling {
            let c = Self::center_pos(m);
            if self.within_target(m, p, 3.0, c) && self.chest_in_use(m, p) {
                Self::stop_in_place(m);
                m.golem.brain.transport.state = TState::Queuing;
            } else if self.within_target(m, p, Self::interaction_range(m), c) {
                self.start_reached(m, p);
            } else {
                Self::walk_towards(m, p);
            }
        }
        if m.golem.brain.transport.state == TState::Interacting {
            self.on_reached(m, p);
        }
    }

    fn walk_towards(m: &mut Mob, p: [i32; 3]) {
        m.golem.brain.look = Some(Look::Block(p));
        m.golem.brain.walk = Some(Walk { pos: p, speed: 1.0, close: 0 });
    }

    fn start_reached(&mut self, m: &mut Mob, p: [i32; 3]) {
        let halves = self.chest_halves(p).unwrap_or_else(|| vec![p]);
        let items = self.halves_items(&halves);
        let empty = items.iter().all(Option::is_none);
        let i = if Self::picking(m) {
            if !empty { 0 } else { 1 }
        } else if empty || Self::has_matching(m, &items) {
            2
        } else {
            3
        };
        m.golem.brain.transport.interaction = Some(i);
        m.golem.brain.transport.state = TState::Interacting;
    }

    fn has_matching(m: &Mob, items: &[Option<Stack>]) -> bool {
        let Some(h) = &m.equipment[0] else { return false };
        items.iter().flatten().any(|s| s.id == h.id)
    }

    fn on_reached(&mut self, m: &mut Mob, p: [i32; 3]) {
        let c = Self::center_pos(m);
        if !self.within_target(m, p, 2.0, c) {
            self.transport_on_travel(m);
            return;
        }
        m.golem.brain.transport.ticks += 1;
        let ticks = m.golem.brain.transport.ticks;
        // Gaze, stop, golem's own action.
        m.golem.brain.look = Some(Look::Block(p));
        Self::stop_in_place(m);
        if let Some(i) = m.golem.brain.transport.interaction {
            if ticks == 1 {
                self.container_openers(p, Ent::Mob(m.id), 1);
                m.golem.opened_chest = Some(p);
                m.golem.state = i32::from(i) + 1;
            }
            if ticks == 9
                && let Some(snd) = SOUNDS_REACHED[i as usize]
            {
                self.mob_sound(m, snd, 1.0, 1.0);
            }
            if ticks == 60 {
                if m.golem.opened_chest.is_some() {
                    self.container_openers(p, Ent::Mob(m.id), -1);
                }
                m.golem.opened_chest = None;
            }
        }
        if ticks >= 60 {
            let halves = self.chest_halves(p).unwrap_or_else(|| vec![p]);
            let mut items = self.halves_items(&halves);
            if Self::picking(m) {
                if items.iter().any(Option::is_some) {
                    // First non-empty stack: 16 at most.
                    if let Some(slot) = items.iter().position(Option::is_some)
                        && let Some(st) = &mut items[slot]
                    {
                        let n = st.count.min(16);
                        let mut taken = st.clone();
                        taken.count = n;
                        st.count -= n;
                        if st.count <= 0 {
                            items[slot] = None;
                        }
                        m.equipment[0] = Some(taken);
                        m.drop_chance[0] = 2.0;
                    }
                    self.write_halves(&halves, &items);
                    Self::clear_after_found(m);
                } else {
                    Self::stop_targeting(m);
                }
            } else if items.iter().all(Option::is_none) || Self::has_matching(m, &items) {
                let left = self.add_to_container(m, &mut items);
                self.write_halves(&halves, &items);
                m.equipment[0] = left;
                if m.equipment[0].is_none() {
                    Self::clear_after_found(m);
                } else {
                    Self::stop_targeting(m);
                }
            } else {
                Self::stop_targeting(m);
            }
            self.transport_on_travel(m);
        }
    }

    /// First empty slot, or merge (the removal subtracts the available room,
    /// not the quantity added — no visible effect, the stack drops to zero).
    fn add_to_container(&self, m: &Mob, items: &mut [Option<Stack>]) -> Option<Stack> {
        let mut hand = m.equipment[0].clone()?;
        for slot in items.iter_mut() {
            match slot {
                None => {
                    *slot = Some(hand);
                    return None;
                }
                Some(c) => {
                    let max = self.life.items.max_stack(c);
                    if crate::items::same(c, &hand) && c.count < max {
                        let room = max - c.count;
                        let add = room.min(hand.count);
                        c.count += add;
                        hand.count -= room;
                        if hand.count <= 0 {
                            return None;
                        }
                    }
                }
            }
        }
        Some(hand)
    }

    /// True if the target has just been changed or abandoned.
    fn transport_update_invalid(&mut self, m: &mut Mob) -> bool {
        if self.transport_target_valid(m) {
            return false;
        }
        Self::stop_targeting(m);
        match self.find_transport_target(m) {
            Some((p, name)) => {
                m.golem.brain.transport.target = Some((p, name));
                self.transport_on_travel(m);
                let b = &mut m.golem.brain;
                if !b.visited.contains(&p) {
                    b.visited.push(p);
                }
                if b.visited.len() > 10 {
                    Self::cooldown_no_target(m);
                } else {
                    b.visited_ttl = 6000;
                }
            }
            None => Self::cooldown_no_target(m),
        }
        true
    }

    fn transport_target_valid(&mut self, m: &mut Mob) -> bool {
        let Some((p, name)) = m.golem.brain.transport.target.clone() else { return false };
        let now = self.state_name(self.st(p)).to_owned();
        let ok = Self::wanted_block(m, &name) && now == name && self.block_entity(p).is_some();
        if !ok || self.chest_blocked_at(p) {
            return false;
        }
        if m.golem.brain.transport.state != TState::Travelling {
            return true;
        }
        if self.travel_path_valid(m, p) {
            return true;
        }
        // Unreachable: removed from the visited, added to the unreachable.
        let b = &mut m.golem.brain;
        b.visited.retain(|q| *q != p);
        if !b.unreachable.contains(&p) {
            b.unreachable.push(p);
        }
        if b.unreachable.len() > 50 {
            Self::cooldown_no_target(m);
        } else {
            b.visited_ttl = 6000;
            b.unreachable_ttl = 6000;
        }
        false
    }

    fn travel_path_valid(&mut self, m: &mut Mob, p: [i32; 3]) -> bool {
        let path = match &m.nav.path {
            Some(path) => Some(path.clone()),
            None => self.create_path(m, p, 0),
        };
        let end = path.as_ref().and_then(|pt| pt.end());
        let from = match end {
            Some(e) => [f64::from(e[0]) + 0.5, f64::from(e[1]), f64::from(e[2]) + 0.5],
            None => m.pos,
        };
        let from = [from[0], from[1] + f64::from(m.height()) / 2.0, from[2]];
        let can = self.within_target(m, p, Self::interaction_range(m), from);
        if path.is_none() && !can {
            return true;
        }
        can && self.sees_target_side(p, from)
    }

    /// Is a face of the target the first one hit from `from`?
    fn sees_target_side(&self, p: [i32; 3], from: [f64; 3]) -> bool {
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        let dirs = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
        dirs.iter().any(|d| {
            let to = [c[0] + 0.5 * f64::from(d[0]), c[1] + 0.5 * f64::from(d[1]), c[2] + 0.5 * f64::from(d[2])];
            let Some(t) = self.clip_t(from, to) else { return false };
            let dv = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
            let l = (dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2]).sqrt().max(1.0e-9);
            let hit = std::array::from_fn(|k| from[k] + dv[k] * t + dv[k] / l * 1.0e-4);
            block_of(hit) == p
        })
    }

    /// Closest wanted chest within ±32 / ±8, chunks within ±3.
    fn find_transport_target(&mut self, m: &mut Mob) -> Option<([i32; 3], String)> {
        let b = m.block_pos();
        let area = rc_physics::Aabb {
            min: [f64::from(b[0] - 32), f64::from(b[1] - 8), f64::from(b[2] - 32)],
            max: [f64::from(b[0] + 33), f64::from(b[1] + 9), f64::from(b[2] + 33)],
        };
        let (cx, cz) = (b[0] >> 4, b[2] >> 4);
        let mut best: Option<([i32; 3], String)> = None;
        let mut closest = f64::from(f32::MAX);
        for z in cz - 3..=cz + 3 {
            for x in cx - 3..=cx + 3 {
                let Some(Some(c)) = self.chunks.get(&(x, z)) else { continue };
                let list: Vec<[i32; 3]> = c
                    .block_entities
                    .iter()
                    .filter(|be| {
                        matches!(
                            be.get("id").and_then(Nbt::as_str),
                            Some("minecraft:chest" | "minecraft:trapped_chest")
                        )
                    })
                    .filter_map(|be| {
                        Some([
                            crate::nbtio::int(be, "x")? as i32,
                            crate::nbtio::int(be, "y")? as i32,
                            crate::nbtio::int(be, "z")? as i32,
                        ])
                    })
                    .collect();
                for p in list {
                    let d = (f64::from(p[0]) + 0.5 - m.pos[0]).powi(2)
                        + (f64::from(p[1]) + 0.5 - m.pos[1]).powi(2)
                        + (f64::from(p[2]) + 0.5 - m.pos[2]).powi(2);
                    if d >= closest {
                        continue;
                    }
                    let inside = (0..3).all(|k| f64::from(p[k]) >= area.min[k] && f64::from(p[k]) < area.max[k]);
                    if !inside || self.chest_halves(p).is_none() {
                        continue;
                    }
                    let name = self.state_name(self.st(p)).to_owned();
                    if !Self::wanted_block(m, &name) {
                        continue;
                    }
                    let mut connected = vec![p];
                    if let Some(o) = self.chest_other(p)
                        && self.block_entity(o).is_some()
                    {
                        connected.push(o);
                    }
                    let br = &m.golem.brain;
                    if connected.iter().any(|q| br.visited.contains(q) || br.unreachable.contains(q)) {
                        continue;
                    }
                    if self.block_entity(p).is_some_and(|be| be.get("lock").is_some()) {
                        continue;
                    }
                    best = Some((p, name));
                    closest = d;
                }
            }
        }
        best
    }

    /// Item thrown from the eyes − 0.3 toward the target (×0.3).
    pub(crate) fn throw_item_towards(&mut self, m: &Mob, item: Stack, to: [f64; 3]) {
        let pos = [m.pos[0], m.eye_y() - f64::from(0.3f32), m.pos[2]];
        let r = &mut self.mobs.rng;
        let _ = (r.next_double(), r.next_double());
        let d = [to[0] - m.pos[0], to[1] - m.pos[1], to[2] - m.pos[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let v = if l < 1.0e-5 { [0.0; 3] } else { d.map(|c| c / l * f64::from(0.3f32)) };
        self.spawn_item(pos, v, item, 10);
    }

    /// Items with a guaranteed drop dropped and removed.
    pub(crate) fn drop_preserved_equipment(&mut self, m: &mut Mob) {
        for i in 0..8 {
            if m.drop_chance[i] > 1.0
                && let Some(s) = m.equipment[i].take()
            {
                let pos = m.pos;
                let r = &mut self.mobs.rng;
                let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
                self.spawn_item(pos, v, s, 10);
            }
        }
    }
}
