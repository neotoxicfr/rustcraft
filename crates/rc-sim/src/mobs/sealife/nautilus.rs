//! Nautilus and zombie nautilus (26.x): common base (smooth swimming, nautilus air, taming
//! with pufferfish, saddle and armor, ridden dash, bubbles, nautilus breath to the rider),
//! brain (sensors, anger, charges against pufferfish, tempting, love, swimming wander);
//! zombie nautilus variants (temperate, warm) drawn by biome.

use rc_proto::nbt::Nbt;

use super::super::mob::{Ent, Mob};
use super::super::mth::{DEG, block_of, cos, dist2, sin};
use super::super::species::Kind;
use crate::nbtio;
use crate::player::Stack;
use crate::server::Server;

/// Rank of the zombie nautilus variant serializer.
pub const VARIANT_SERIALIZER: u8 = 32;

const SADDLE: usize = 7;
const BODY: usize = 6;

/// Followed target: entity (eyes or feet) or block center.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Track {
    Ent(Ent, bool),
    Pos([i32; 3]),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WalkTarget {
    pub target: Track,
    pub speed: f32,
    pub close: i32,
}

/// Behavior with a duration: running, end.
#[derive(Debug, Clone, Copy, Default)]
struct B {
    running: bool,
    end: i64,
}

/// Brain of a nautilus: sensor delays, memories, running behaviors.
#[derive(Debug, Clone, Default)]
pub struct Brain {
    /// Sensors: entities, hurt, tempting (adult and players drawn but unused).
    sensors: [i64; 3],
    // Memories.
    /// Anger: targeted UUID, ticks left.
    pub angry: Option<(u128, i64)>,
    pub attack: Option<Ent>,
    pub attack_cd: Option<i32>,
    pub charge_cd: Option<i32>,
    pub tempt_cd: Option<i32>,
    pub panicking: bool,
    hurt_by: Option<String>,
    hurt_by_entity: Option<Ent>,
    tempting: bool,
    pub breed: Option<i32>,
    look: Option<Track>,
    walk: Option<WalkTarget>,
    path: bool,
    cant_reach: Option<i64>,
    visible: Option<Vec<(Ent, Option<bool>)>>,
    // Behaviors.
    panic: B,
    look_sink: B,
    move_sink: B,
    move_cooldown: i32,
    move_has_path: bool,
    move_last: Option<[i32; 3]>,
    move_speed: f32,
    cd_tempt: bool,
    cd_charge: bool,
    cd_attack: bool,
    love: B,
    love_at: i64,
    tempt: bool,
    charge: B,
    charge_vel: [f64; 3],
    charge_start: [f64; 3],
    /// Activity: fight (otherwise idle).
    pub fight: bool,
}

/// Constructor: delays of the five sensors (entity random), knockback resistance 0.3.
pub fn on_new(m: &mut Mob) {
    let mut b = Brain::default();
    let mut d = [0i64; 5];
    for x in &mut d {
        *x = i64::from(m.rng.next_int_bound(20));
    }
    // Sensor order: entities, adult, players, hurt, tempting.
    b.sensors = [d[0], d[3], d[4]];
    m.sea.brain = Some(Box::new(b));
    m.attrs.knockback_resistance.base = f64::from(0.3f32);
}

/// Cooldown of non-player attacks (2,400 to 3,600 ticks, world random).
pub fn init_memories(s: &mut Server, m: &mut Mob) {
    let v = 2400 + s.mobs.rng.next_int_bound(1201);
    if let Some(b) = m.sea.brain.as_mut() {
        b.attack_cd = Some(v);
    }
}

/// Zombie nautilus variant drawn by biome (coral in warm water).
pub fn select_variant(s: &mut Server, m: &mut Mob) {
    if let Some(v) = s.select_variant("zombie_nautilus", m.block_pos()) {
        m.variant = v;
        m.sea.variant_name = s.regs.entries("minecraft:zombie_nautilus_variant").get(v as usize).cloned();
    }
}

/// Construction: variant read back by its name (registry), otherwise the default variant
/// (temperate).
pub fn init(s: &Server, m: &mut Mob) {
    if m.kind != Kind::ZombieNautilus {
        return;
    }
    let reg = s.regs.entries("minecraft:zombie_nautilus_variant");
    let name = m.sea.variant_name.clone().unwrap_or_else(|| "minecraft:temperate".into());
    if let Some(i) = reg.iter().position(|x| *x == name) {
        m.variant = i as i32;
        m.sea.variant_name = Some(name);
    }
}

// ---------- NBT ----------

pub fn load(m: &mut Mob, e: &Nbt) {
    if m.kind == Kind::ZombieNautilus {
        m.sea.variant_name = e.get("variant").and_then(Nbt::as_str).map(str::to_owned);
    }
    m.owner = nbtio::uuid(e.get("Owner"));
    m.tame = m.owner.is_some();
    m.sitting = nbtio::int(e, "Sitting").is_some_and(|v| v != 0);
    if let Some(Nbt::Compound(d)) = e.get("drop_chances") {
        for (k, v) in d {
            if let (Some(i), Some(c)) = (super::super::mob::slot_index(k), super::super::mob::f64_of(v)) {
                m.drop_chance[i] = c as f32;
            }
        }
    }
    // Return area (negative or absent radius: none).
    let r = nbtio::int(e, "home_radius").map_or(-1, |v| v as i32);
    m.sea.home = (r >= 0).then(|| match e.get("home_pos") {
        Some(Nbt::IntArray(v)) if v.len() == 3 => ([v[0], v[1], v[2]], r),
        _ => ([0, 0, 0], r),
    });
    let Some(b) = m.sea.brain.as_mut() else { return };
    let mem = e.get("Brain").and_then(|b| b.get("memories"));
    let Some(mem) = mem else { return };
    let int_of = |k: &str| mem.get(k).and_then(|v| v.get("value")).and_then(Nbt::as_i64).map(|v| v as i32);
    b.attack_cd = int_of("minecraft:attack_target_cooldown");
    b.charge_cd = int_of("minecraft:charge_cooldown_ticks");
    b.tempt_cd = int_of("minecraft:temptation_cooldown_ticks");
    b.panicking = int_of("minecraft:is_panicking").is_some_and(|v| v != 0);
    if let Some(a) = mem.get("minecraft:angry_at") {
        let ttl = a.get("ttl").and_then(Nbt::as_i64).unwrap_or(i64::MAX);
        if let Some(u) = nbtio::uuid(a.get("value")) {
            b.angry = Some((u, ttl));
        }
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    let set = |e: &mut Nbt, k: &str, v: Nbt| nbtio::set(e, k, v);
    if let Some(n) = &m.sea.variant_name {
        set(e, "variant", Nbt::String(n.clone()));
    }
    match m.owner {
        Some(o) => set(e, "Owner", nbtio::uuid_nbt(o)),
        None => nbtio::remove(e, "Owner"),
    }
    set(e, "Sitting", Nbt::Byte(i8::from(m.sitting)));
    match m.sea.home {
        Some((p, r)) => {
            set(e, "home_pos", Nbt::IntArray(vec![p[0], p[1], p[2]]));
            set(e, "home_radius", Nbt::Int(r));
        }
        None => {
            nbtio::remove(e, "home_pos");
            nbtio::remove(e, "home_radius");
        }
    }
    let chances: Vec<(String, Nbt)> = m
        .drop_chance
        .iter()
        .enumerate()
        .filter(|(_, c)| (**c - 0.085).abs() > f32::EPSILON)
        .map(|(i, c)| (super::super::mob::SLOT_NAMES[i].to_owned(), Nbt::Float(*c)))
        .collect();
    if chances.is_empty() {
        nbtio::remove(e, "drop_chances");
    } else {
        set(e, "drop_chances", Nbt::Compound(chances));
    }
    let Some(b) = m.sea.brain.as_ref() else { return };
    let mut mems: Vec<(String, Nbt)> = Vec::new();
    let int = |v: i32| Nbt::Compound(vec![("value".into(), Nbt::Int(v))]);
    if let Some((u, ttl)) = b.angry {
        mems.push((
            "minecraft:angry_at".into(),
            Nbt::Compound(vec![("value".into(), nbtio::uuid_nbt(u)), ("ttl".into(), Nbt::Long(ttl))]),
        ));
    }
    if let Some(v) = b.attack_cd {
        mems.push(("minecraft:attack_target_cooldown".into(), int(v)));
    }
    if let Some(v) = b.charge_cd {
        mems.push(("minecraft:charge_cooldown_ticks".into(), int(v)));
    }
    if b.panicking {
        mems.push(("minecraft:is_panicking".into(), Nbt::Compound(vec![("value".into(), Nbt::Byte(1))])));
    }
    if let Some(v) = b.tempt_cd {
        mems.push(("minecraft:temptation_cooldown_ticks".into(), int(v)));
    }
    set(e, "Brain", Nbt::Compound(vec![("memories".into(), Nbt::Compound(mems))]));
}


// ---------- Tick ----------

/// End of the nautilus tick: nautilus breath to the rider, end of dash, cooldown,
/// bubbles (entity random draws, particles with no server-side effect).
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    if m.mount.passengers.first() == Some(&Ent::Player) {
        let has = s.life.fx.list.iter().any(|x| x.id == "minecraft:breath_of_the_nautilus");
        if !has || s.time % 40 == 0 {
            let mut e = crate::effects::Inst::new("minecraft:breath_of_the_nautilus", 60, 0);
            e.ambient = true;
            s.add_effect(e);
        }
    }
    if m.mount.dash && m.mount.dash_cooldown < 35 {
        set_dash(m, false);
    }
    if m.mount.dash_cooldown > 0 {
        m.mount.dash_cooldown -= 1;
        if m.mount.dash_cooldown == 0 {
            let snd = dash_ready_sound(m);
            s.make_sound(m, snd);
        }
    }
    if m.in_water {
        let v = m.vel;
        let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let prob = (speed * 2.0).clamp(f64::from(0.15f32), 1.0);
        if f64::from(m.rng.next_float()) < prob {
            let _ = m.rng.next_double();
            let _ = (m.rng.next_float(), m.rng.next_float(), m.rng.next_float());
        }
    }
}

/// Dash: any change arms the cooldown at 40 if it is empty.
pub fn set_dash(m: &mut Mob, v: bool) {
    if m.mount.dash != v && m.tick_count > 0 && m.mount.dash_cooldown == 0 {
        m.mount.dash_cooldown = 40;
    }
    m.mount.dash = v;
}

fn dash_ready_sound(m: &Mob) -> &'static str {
    let under = super::under_water(m);
    match (m.kind, under) {
        (Kind::ZombieNautilus, true) => "minecraft:entity.zombie_nautilus.dash_ready",
        (Kind::ZombieNautilus, false) => "minecraft:entity.zombie_nautilus.dash_ready_land",
        (_, true) => "minecraft:entity.nautilus.dash_ready",
        (_, false) => "minecraft:entity.nautilus.dash_ready_land",
    }
}

pub fn dash_sound(m: &Mob) -> &'static str {
    let under = super::under_water(m);
    match (m.kind, under) {
        (Kind::ZombieNautilus, true) => "minecraft:entity.zombie_nautilus.dash",
        (Kind::ZombieNautilus, false) => "minecraft:entity.zombie_nautilus.dash_land",
        (_, true) => "minecraft:entity.nautilus.dash",
        (_, false) => "minecraft:entity.nautilus.dash_land",
    }
}

fn eat_sound(m: &Mob) -> &'static str {
    match m.kind {
        Kind::ZombieNautilus => "minecraft:entity.zombie_nautilus.eat",
        _ if m.is_baby() => "minecraft:entity.baby_nautilus.eat",
        _ => "minecraft:entity.nautilus.eat",
    }
}

/// Return area of a free tamed nautilus: 32 without saddle (adult),
/// 16 otherwise; recentered when it moves away by more than radius + 8.
fn check_restriction(m: &mut Mob) {
    if !m.tame || !m.mount.passengers.is_empty() {
        return;
    }
    let r = if !m.is_baby() && m.equipment[SADDLE].is_none() { 32 } else { 16 };
    let p = m.block_pos();
    let ok = m.sea.home.is_some_and(|(h, hr)| {
        let d = (0..3).map(|k| f64::from(h[k] - p[k]).powi(2)).sum::<f64>();
        d < f64::from(r + 8).powi(2) && hr == r
    });
    if !ok {
        m.sea.home = Some((p, r));
    }
}

fn track_pos(s: &Server, t: Track) -> Option<[f64; 3]> {
    match t {
        Track::Ent(e, eye) => {
            let p = s.ent_pos(e)?;
            Some(if eye { [p[0], s.ent_eye(e)?, p[2]] } else { p })
        }
        Track::Pos(b) => Some([f64::from(b[0]) + 0.5, f64::from(b[1]) + 0.5, f64::from(b[2]) + 0.5]),
    }
}

fn track_block(s: &Server, t: Track) -> Option<[i32; 3]> {
    match t {
        Track::Ent(e, _) => s.ent_pos(e).map(block_of),
        Track::Pos(b) => Some(b),
    }
}

fn manhattan(a: [i32; 3], b: [i32; 3]) -> i32 {
    (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()
}

/// Tag `nautilus_hostiles` in water: the pufferfish.
fn hostile(s: &Server, e: Ent) -> bool {
    match e {
        Ent::Mob(id) => s.mob_ref(id).is_some_and(|o| o.kind == Kind::Pufferfish && o.in_water),
        Ent::Player => false,
    }
}

impl Server {
    /// Entity of a UUID (player or simulated mob).
    fn ent_of_uuid(&self, u: u128) -> Option<Ent> {
        if u == self.uuid {
            return Some(Ent::Player);
        }
        self.mobs.get_uuid(u).map(|o| Ent::Mob(o.id))
    }

    fn ent_uuid_of(&self, e: Ent) -> Option<u128> {
        match e {
            Ent::Player => Some(self.uuid),
            Ent::Mob(id) => self.mob_ref(id).map(|o| o.uuid),
        }
    }

    fn ent_in_water(&self, e: Ent) -> bool {
        match e {
            Ent::Player => self.player_in_water(),
            Ent::Mob(id) => self.mob_ref(id).is_some_and(|o| o.in_water),
        }
    }

    /// Combat conditions, shared sensor
    /// range.
    fn attackable_no_sight(&self, m: &mut Mob, e: Ent) -> bool {
        self.targeting_ok(m, e, true, self.mobs.pig.sensor_range, false, true)
    }

    /// Anger against the aggressor (400 ticks) if attackable.
    pub(crate) fn nautilus_anger(&mut self, m: &mut Mob, by: Ent) {
        if !self.attackable_no_sight(m, by) {
            return;
        }
        let Some(u) = self.ent_uuid_of(by) else { return };
        if let Some(b) = m.sea.brain.as_mut() {
            b.cant_reach = None;
            b.angry = Some((u, 400));
        }
    }

    fn naut_visible_contains(&self, m: &mut Mob, b: &mut Brain, e: Ent) -> bool {
        let Some(v) = b.visible.as_mut() else { return false };
        let Some(slot) = v.iter_mut().find(|(x, _)| *x == e) else { return false };
        if let Some(r) = slot.1 {
            return r;
        }
        let r = self.targeting_ok(m, e, false, self.mobs.pig.sensor_range, true, true);
        if let Some(v) = b.visible.as_mut()
            && let Some(slot) = v.iter_mut().find(|(x, _)| *x == e)
        {
            slot.1 = Some(r);
        }
        r
    }

    fn naut_visible_closest(&self, m: &mut Mob, b: &mut Brain, f: &dyn Fn(&Server, Ent) -> bool) -> Option<Ent> {
        let list: Vec<Ent> = b.visible.as_ref()?.iter().map(|x| x.0).collect();
        list.into_iter().find(|&e| f(self, e) && self.naut_visible_contains(m, b, e))
    }

    /// Brain, activity, return area.
    pub(crate) fn nautilus_brain_tick(&mut self, m: &mut Mob) {
        let Some(mut b) = m.sea.brain.take() else { return };
        let t = self.time;
        self.naut_forget(&mut b);
        self.naut_sensors(m, &mut b);
        self.naut_start(m, &mut b, t);
        self.naut_tick(m, &mut b, t);
        // Activity: fight if target, neither tempting, nor love, nor charge cooldown.
        b.fight = b.attack.is_some() && !b.tempting && b.breed.is_none() && b.charge_cd.is_none();
        m.sea.brain = Some(b);
        check_restriction(m);
    }

    fn naut_forget(&mut self, b: &mut Brain) {
        if let Some((u, ttl)) = b.angry {
            if ttl <= 0 {
                b.angry = None;
            } else {
                b.angry = Some((u, ttl - 1));
            }
        }
    }

    fn naut_sensors(&mut self, m: &mut Mob, b: &mut Brain) {
        // Each sensor that ticks rewrites the shared range of the targeting conditions.
        if b.sensors.iter().any(|&t| t <= 1) {
            self.mobs.pig.sensor_range = m.follow_range();
        }
        // Living entities within 16 blocks, sorted by distance.
        b.sensors[0] -= 1;
        if b.sensors[0] <= 0 {
            b.sensors[0] = 20;
            let bb = m.bbox();
            let area = rc_physics::Aabb { min: bb.min.map(|v| v - 16.0), max: bb.max.map(|v| v + 16.0) };
            let mut list: Vec<(f64, Ent)> = self
                .mobs
                .list
                .iter()
                .flatten()
                .filter(|o| o.id != m.id && o.alive() && o.touches(&area))
                .map(|o| (dist2(m.pos, o.pos), Ent::Mob(o.id)))
                .collect();
            if self.ent_alive(Ent::Player) && self.player_aabb().intersects(&area) {
                list.push((dist2(m.pos, self.pos), Ent::Player));
            }
            list.sort_by(|a, c| a.0.partial_cmp(&c.0).unwrap_or(std::cmp::Ordering::Equal));
            b.visible = Some(list.into_iter().map(|x| (x.1, None)).collect());
        }
        // Recent hurt (40 ticks).
        b.sensors[1] -= 1;
        if b.sensors[1] <= 0 {
            b.sensors[1] = 20;
            match &m.last_hit {
                Some(h) if self.time - h.tick as i64 <= 40 => {
                    b.hurt_by = Some(h.ty.clone());
                    if let Some(e) = h.by {
                        b.hurt_by_entity = Some(e);
                    }
                }
                _ => b.hurt_by = None,
            }
            if b.hurt_by_entity.is_some_and(|e| !self.ent_alive(e)) {
                b.hurt_by_entity = None;
            }
        }
        // Tempting player (nautilus food in hand, tempt range, not a passenger).
        b.sensors[2] -= 1;
        if b.sensors[2] <= 0 {
            b.sensors[2] = 20;
            let range = m.attrs.tempt_range.value();
            let held = [&self.slots[36 + self.held as usize], &self.slots[45]];
            let holds = held.iter().any(|s| s.as_ref().is_some_and(|s| self.item_tag(&s.id, "minecraft:nautilus_food")));
            b.tempting = self.game_mode != 3
                && holds
                && !m.mount.passengers.contains(&Ent::Player)
                && self.targeting_ok(m, Ent::Player, false, range, false, true);
        }
    }

    /// Duration drawn at start (world random).
    fn naut_duration(&mut self, min: i32, max: i32) -> i64 {
        i64::from(min + self.mobs.rng.next_int_bound(max + 1 - min))
    }

    /// Start by priority: core 0..5, idle 1..4 (or fight 0).
    fn naut_start(&mut self, m: &mut Mob, b: &mut Brain, t: i64) {
        let zombie = m.kind == Kind::ZombieNautilus;
        // 0: panic (nautilus); charge (fight).
        if !zombie && !b.panic.running {
            let hurt = b.hurt_by.as_ref().is_some_and(|ty| self.dmg_tag(ty, "minecraft:panic_causes"));
            if hurt || b.panicking {
                b.panic.running = true;
                b.panic.end = t + self.naut_duration(100, 120);
                b.panicking = true;
                b.walk = None;
                m.nav.stop();
            }
        }
        if b.fight && !b.charge.running && b.charge_cd.is_none() && b.attack.is_some() {
            b.charge.running = true;
            b.charge.end = t + self.naut_duration(60, 60);
            self.charge_start(m, b);
        }
        // 1: look; love (nautilus).
        if !b.look_sink.running && b.look.is_some() {
            b.look_sink.running = true;
            b.look_sink.end = t + self.naut_duration(45, 90);
        }
        if !b.fight && !zombie && !b.love.running && b.visible.is_some() && b.breed.is_none() && !b.panicking && m.in_love > 0 {
            let me = m.clone();
            let f = move |s: &Server, e: Ent| match e {
                Ent::Mob(id) => s.mob_ref(id).is_some_and(|o| {
                    o.kind == Kind::Nautilus && can_mate(&me, o) && !o.sea.brain.as_ref().is_some_and(|x| x.panicking)
                }),
                Ent::Player => false,
            };
            if let Some(Ent::Mob(pid)) = self.naut_visible_closest(m, b, &f) {
                b.love.running = true;
                b.love.end = t + self.naut_duration(110, 110);
                b.breed = Some(pid);
                self.naut_lock_gaze(m, b, pid);
                b.love_at = t + i64::from(60 + m.rng.next_int_bound(50));
            }
        }
        // 2: go to the walk target; tempting (idle; priority 1 for the zombie).
        if !b.move_sink.running && b.walk.is_some() && !b.path && self.naut_move_start_ok(m, b) {
            b.move_sink.running = true;
            b.move_sink.end = t + self.naut_duration(150, 250);
            b.path = true;
            let sp = f64::from(b.move_speed);
            let p = m.nav.path.clone();
            self.move_to_path(m, p, sp);
        }
        // Fixed-duration behaviors (60) still draw their duration at start.
        if !b.fight && !b.tempt && b.tempt_cd.is_none() && b.tempting && b.breed.is_none() && !b.panicking {
            b.tempt = true;
            let _ = self.naut_duration(60, 60);
        }
        // 3: tempt countdown; attack (idle).
        if !b.cd_tempt && b.tempt_cd.is_some() {
            b.cd_tempt = true;
            let _ = self.naut_duration(60, 60);
        }
        if !b.fight && b.attack.is_none()
            && let Some(target) = self.naut_find_target(m, b)
            && self.targeting_ok(m, target, true, 0.0, false, false)
        {
            b.attack = Some(target);
            b.cant_reach = None;
        }
        // 4: charge countdown; swimming wander or toward the gaze (idle, without target).
        if !b.cd_charge && b.charge_cd.is_some() {
            b.cd_charge = true;
            let _ = self.naut_duration(60, 60);
        }
        if !b.fight && b.walk.is_none() {
            if m.in_water {
                let p = self.naut_swim_pos(m);
                b.walk = p.map(|p| WalkTarget { target: Track::Pos(block_of(p)), speed: 1.0, close: 0 });
            }
            if b.walk.is_none()
                && let Some(l) = b.look
            {
                b.walk = Some(WalkTarget { target: l, speed: 1.0, close: 3 });
            }
        }
        // 5: attack cooldown countdown.
        if !b.cd_attack && b.attack_cd.is_some() {
            b.cd_attack = true;
            let _ = self.naut_duration(60, 60);
        }
    }

    /// Attack target: the aggressor in water, otherwise, outside
    /// cooldown, one chance in two to target the closest seen pufferfish.
    fn naut_find_target(&mut self, m: &mut Mob, b: &mut Brain) -> Option<Ent> {
        if b.breed.is_some() || !m.in_water || m.is_baby() || m.tame {
            return None;
        }
        if let Some((u, _)) = b.angry
            && let Some(e) = self.ent_of_uuid(u)
            && self.ent_in_water(e)
            && self.attackable_no_sight(m, e)
        {
            return Some(e);
        }
        if b.attack_cd.is_some() {
            return None;
        }
        b.attack_cd = Some(2400 + self.mobs.rng.next_int_bound(1201));
        if self.mobs.rng.next_float() < 0.5 {
            return None;
        }
        self.naut_visible_closest(m, b, &hostile)
    }

    /// Distance tiers, fallback to the last valid point.
    fn naut_swim_pos(&mut self, m: &mut Mob) -> Option<[f64; 3]> {
        const TIERS: [(i32, i32); 6] = [(1, 1), (3, 3), (5, 5), (6, 5), (7, 7), (10, 7)];
        let mut fallback: Option<[f64; 3]> = None;
        let mut target: Option<[f64; 3]> = None;
        for (h, v) in TIERS {
            target = match fallback {
                None => self.random_swimmable_pos(m, h, v),
                Some(f) => {
                    let d = [f[0] - m.pos[0], f[1] - m.pos[1], f[2] - m.pos[2]];
                    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    let n = if l < 1.0e-4 { [0.0; 3] } else { d.map(|x| x / l) };
                    Some([
                        m.pos[0] + n[0] * f64::from(h),
                        m.pos[1] + n[1] * f64::from(v),
                        m.pos[2] + n[2] * f64::from(h),
                    ])
                }
            };
            match target {
                Some(p) if self.mob_fluid(block_of(p)) != super::super::env::Fluid::None => fallback = Some(p),
                _ => return fallback,
            }
        }
        target
    }

    fn charge_start(&mut self, m: &mut Mob, b: &mut Brain) {
        b.charge_start = m.pos;
        let Some(tp) = b.attack.and_then(|e| self.ent_pos(e)) else { return };
        let d = [tp[0] - m.pos[0], tp[1] - m.pos[1], tp[2] - m.pos[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let n = if l < 1.0e-4 { [0.0; 3] } else { d.map(|x| x / l) };
        let sp = if m.kind == Kind::ZombieNautilus { 0.5 } else { 0.6 };
        b.charge_vel = n.map(|x| x * f64::from(sp as f32));
        if self.charge_can_continue(m, b) {
            let snd = if m.kind == Kind::ZombieNautilus {
                "minecraft:entity.zombie_nautilus.dash"
            } else {
                "minecraft:entity.nautilus.dash"
            };
            self.mob_sound(m, snd, 1.0, 1.0);
        }
    }

    fn charge_can_continue(&mut self, m: &mut Mob, b: &Brain) -> bool {
        let Some(e) = b.attack else { return false };
        if m.tame {
            return false;
        }
        if dist2(m.pos, b.charge_start) >= 144.0 {
            return false;
        }
        let Some(tp) = self.ent_pos(e) else { return false };
        if dist2(tp, m.pos) >= 121.0 {
            return false;
        }
        self.line_of_sight(m, e) && b.charge_cd.is_none()
    }

    fn charge_stop(&mut self, b: &mut Brain) {
        b.charge.running = false;
        b.charge_cd = Some(80);
        b.attack = None;
    }

    fn naut_tick(&mut self, m: &mut Mob, b: &mut Brain, t: i64) {
        if b.panic.running {
            if t > b.panic.end {
                b.panic.running = false;
                b.panicking = false;
            } else if m.nav.is_done() {
                // On fire: the nearest water; otherwise a land point (not found in water).
                let water = if m.on_fire() { self.look_for_water(m, 5) } else { None };
                let p = water.or_else(|| self.land_random_pos(m, 5, 4).map(block_of));
                if let Some(p) = p {
                    b.walk = Some(WalkTarget { target: Track::Pos(p), speed: 1.6, close: 0 });
                }
            }
        }
        if b.charge.running {
            if t > b.charge.end || !self.charge_can_continue(m, b) {
                self.charge_stop(b);
            } else if let Some(e) = b.attack {
                if let (Some(tp), Some(eye)) = (self.ent_pos(e), self.ent_eye(e)) {
                    super::super::goals::mob_look_at_direct(m, tp, eye, 360.0, 360.0);
                }
                m.vel = b.charge_vel;
                self.charge_hit(m, b);
            }
        }
        if b.look_sink.running {
            let visible = match b.look {
                Some(Track::Ent(e, _)) => self.ent_alive(e) && self.naut_visible_contains(m, b, e),
                Some(Track::Pos(_)) => true,
                None => false,
            };
            if t > b.look_sink.end || !visible {
                b.look_sink.running = false;
                b.look = None;
            } else if let Some(p) = b.look.and_then(|l| track_pos(self, l)) {
                self.look_at(m, p, 10.0, 40.0);
            }
        }
        if b.love.running {
            let ok = b.breed.is_some_and(|pid| {
                self.mob_ref(pid).is_some_and(|p| {
                    p.alive() && can_mate(m, p) && !p.sea.brain.as_ref().is_some_and(|x| x.panicking)
                })
            }) && b.breed.is_some_and(|pid| self.naut_visible_contains(m, b, Ent::Mob(pid)))
                && t <= b.love_at
                && !b.panicking;
            if t > b.love.end || !ok {
                b.love.running = false;
                b.breed = None;
                b.walk = None;
                b.look = None;
                b.love_at = 0;
            } else if let Some(pid) = b.breed {
                self.naut_lock_gaze(m, b, pid);
                if let Some(pp) = self.mob_ref(pid).map(|p| p.pos)
                    && dist2(m.pos, pp) < 9.0
                    && t >= b.love_at
                {
                    self.nautilus_breed(m, pid);
                    b.breed = None;
                    if let Some(i) = self.mob_index(pid)
                        && let Some(o) = self.mobs.list[i].as_mut()
                        && let Some(ob) = o.sea.brain.as_mut()
                    {
                        ob.breed = None;
                    }
                }
            }
        }
        if b.move_sink.running {
            let reached = b.walk.and_then(|w| track_block(self, w.target).map(|tb| (tb, w.close)));
            let ok = b.move_has_path
                && b.move_last.is_some()
                && !m.nav.is_done()
                && matches!(reached, Some((tb, c)) if manhattan(tb, m.block_pos()) > c);
            if t > b.move_sink.end || !ok {
                b.move_sink.running = false;
                if let Some((tb, c)) = reached
                    && manhattan(tb, m.block_pos()) > c
                    && m.nav.stuck
                {
                    b.move_cooldown = self.mobs.rng.next_int_bound(40);
                }
                m.nav.stop();
                b.walk = None;
                b.path = false;
                b.move_has_path = false;
            } else if let Some(w) = b.walk
                && let (Some(tb), Some(last)) = (track_block(self, w.target), b.move_last)
            {
                let d = [tb[0] - last[0], tb[1] - last[1], tb[2] - last[2]];
                if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > 4 && self.naut_compute_path(m, b, w) {
                    b.move_last = Some(tb);
                    let sp = f64::from(b.move_speed);
                    let p = m.nav.path.clone();
                    self.move_to_path(m, p, sp);
                }
            }
        }
        if b.tempt {
            if !(b.tempting && b.breed.is_none() && !b.panicking) {
                b.tempt = false;
                b.tempt_cd = Some(100);
                b.walk = None;
                b.look = None;
            } else {
                b.look = Some(Track::Ent(Ent::Player, true));
                let close: f64 = if m.is_baby() { 2.5 } else { 3.5 };
                let speed = if m.kind == Kind::ZombieNautilus { 0.9 } else { 1.3 };
                if dist2(m.pos, self.pos) < close * close {
                    b.walk = None;
                } else {
                    b.walk = Some(WalkTarget { target: Track::Ent(Ent::Player, false), speed, close: 2 });
                }
            }
        }
        // Countdowns: the memory drops to 0, then is erased on the next tick.
        for (on, cd) in [(&mut b.cd_tempt, &mut b.tempt_cd), (&mut b.cd_charge, &mut b.charge_cd), (&mut b.cd_attack, &mut b.attack_cd)] {
            if *on {
                match *cd {
                    Some(n) if n > 0 => *cd = Some(n - 1),
                    _ => {
                        *on = false;
                        *cd = None;
                    }
                }
            }
        }
    }

    /// Charge: the first attackable living entity in the box takes the hit and knockback.
    fn charge_hit(&mut self, m: &mut Mob, b: &mut Brain) {
        let bb = m.bbox();
        let mut hit: Option<Ent> = None;
        let ids: Vec<i32> =
            self.mobs.list.iter().flatten().filter(|o| o.touches(&bb) && o.alive()).map(|o| o.id).collect();
        for id in ids {
            if self.targeting_ok(m, Ent::Mob(id), true, 0.0, false, false) {
                hit = Some(Ent::Mob(id));
                break;
            }
        }
        if hit.is_none() && self.player_aabb().intersects(&bb) && self.targeting_ok(m, Ent::Player, true, 0.0, false, false) {
            hit = Some(Ent::Player);
        }
        let Some(e) = hit else { return };
        if m.mount.passengers.contains(&e) {
            return;
        }
        let dmg = m.attrs.attack.value() as f32;
        match e {
            Ent::Player => {
                self.player_hurt_by_mob(m, "minecraft:mob_attack", dmg, Some((m.pos[0] - self.pos[0], m.pos[2] - self.pos[2])));
            }
            Ent::Mob(id) => {
                if let Some(i) = self.mob_index(id)
                    && let Some(mut o) = self.mobs.list[i].take()
                {
                    self.mob_hurt_by(&mut o, m, "minecraft:mob_attack", dmg);
                    self.mobs.list[i] = Some(o);
                }
            }
        }
        // Extra knockback: (velocity × dash) clamped, × 2.
        let sp = if m.kind == Kind::ZombieNautilus { 0.5f32 } else { 0.6 };
        let factor = (sp * m.movement_speed() as f32).clamp(0.2, 2.0) * 2.0;
        let (sn, cs) = (sin(m.yaw * DEG), cos(m.yaw * DEG));
        match e {
            Ent::Player => self.player_knockback(f64::from(factor), f64::from(sn), f64::from(-cs)),
            Ent::Mob(id) => {
                if let Some(i) = self.mob_index(id)
                    && let Some(mut o) = self.mobs.list[i].take()
                {
                    // Victim's own knockback (full sulfur cube: attacker's aim).
                    self.ghost_push(m);
                    let by = Some(Ent::Mob(m.id));
                    self.mob_knockback(&mut o, f64::from(factor), f64::from(sn), f64::from(-cs), by, dmg, false);
                    self.ghost_pop();
                    self.mobs.list[i] = Some(o);
                }
            }
        }
        m.vel[0] *= 0.6;
        m.vel[2] *= 0.6;
        self.charge_stop(b);
    }

    fn naut_lock_gaze(&mut self, m: &mut Mob, b: &mut Brain, pid: i32) {
        b.look = Some(Track::Ent(Ent::Mob(pid), true));
        b.walk = Some(WalkTarget { target: Track::Ent(Ent::Mob(pid), true), speed: 0.4, close: 2 });
        let me = m.id;
        if let Some(i) = self.mob_index(pid)
            && let Some(o) = self.mobs.list[i].as_mut()
            && let Some(ob) = o.sea.brain.as_mut()
        {
            ob.breed = Some(me);
            ob.look = Some(Track::Ent(Ent::Mob(me), true));
            ob.walk = Some(WalkTarget { target: Track::Ent(Ent::Mob(me), true), speed: 0.4, close: 2 });
        }
    }

    fn naut_move_start_ok(&mut self, m: &mut Mob, b: &mut Brain) -> bool {
        if b.move_cooldown > 0 {
            b.move_cooldown -= 1;
            return false;
        }
        let Some(w) = b.walk else { return false };
        let Some(tb) = track_block(self, w.target) else { return false };
        let reached = manhattan(tb, m.block_pos()) <= w.close;
        if !reached && self.naut_compute_path(m, b, w) {
            b.move_last = Some(tb);
            return true;
        }
        b.walk = None;
        if reached {
            b.cant_reach = None;
        }
        false
    }

    fn naut_compute_path(&mut self, m: &mut Mob, b: &mut Brain, w: WalkTarget) -> bool {
        let Some(tb) = track_block(self, w.target) else { return false };
        let path = self.create_path(m, tb, 0);
        b.move_speed = w.speed;
        if manhattan(tb, m.block_pos()) <= w.close || path.as_ref().is_some_and(|p| p.reached) {
            b.cant_reach = None;
        } else if b.cant_reach.is_none() {
            b.cant_reach = Some(self.time);
        }
        b.move_has_path = path.is_some();
        if let Some(p) = path {
            m.nav.path = Some(p);
            return true;
        }
        false
    }

    /// Baby nautilus born from two nautiluses, tamed if the parent is.
    pub(crate) fn nautilus_breed(&mut self, m: &mut Mob, partner: i32) {
        let Some(pi) = self.mob_index(partner) else { return };
        let mut baby = self.new_mob(Kind::Nautilus, m.pos, 0.0);
        baby.age = -24000;
        if m.tame {
            baby.owner = m.owner;
            baby.tame = true;
        }
        self.s9_bred(m, partner, Some(&baby));
        m.age = 6000;
        m.reset_love();
        if let Some(p) = self.mobs.list[pi].as_mut() {
            p.age = 6000;
            p.reset_love();
        }
        self.mob_event(m, 18);
        self.s9_bred(m, partner, Some(&baby)); // S9: statistic and trigger, like the common routine
        if self.rule("minecraft:mob_drops", true) {
            let xp = m.rng.next_int_bound(7) + 1;
            self.spawn_orbs(m.pos, xp);
        }
        self.add_mob(baby);
    }

    // ---------- Interactions ----------

    /// Right click (`interact` then the player's interaction): always persistent; taming,
    /// healing, saddle and armor, inventory, mounting, love.
    pub(crate) fn nautilus_interact(&mut self, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
        m.persistent = true;
        let food = |s: &Server, m: &Mob, it: &str| {
            if !m.tame && !m.is_baby() {
                s.item_tag(it, "minecraft:nautilus_taming_items")
            } else {
                s.item_tag(it, "minecraft:nautilus_food")
            }
        };
        if m.is_baby() {
            return Some(self.nautilus_animal_interact(m, slot, item));
        }
        if m.tame && self.sneaking {
            self.open_nautilus_menu(m);
            return Some(true);
        }
        if !item.is_empty() {
            if !m.tame && food(self, m, item) {
                self.nautilus_use_item(slot, item);
                if m.rng.next_int_bound(3) == 0 {
                    m.tame = true;
                    m.owner = Some(self.uuid);
                    m.nav.stop();
                    self.mob_event(m, 7);
                } else {
                    self.mob_event(m, 6);
                }
                let snd = eat_sound(m);
                self.make_sound(m, snd);
                return Some(true);
            }
            if food(self, m, item) && m.health < m.max_health() {
                let nutrition = self.food_nutrition(item);
                self.nautilus_use_item(slot, item);
                let heal = nutrition.map_or(1.0, |n| 2.0 * n as f32);
                m.health = (m.health + heal).min(m.max_health());
                let snd = eat_sound(m);
                self.make_sound(m, snd);
                return Some(true);
            }
            if let Some(h) = self.slots[slot].clone()
                && self.nautilus_equip(m, slot, &h)
            {
                return Some(true);
            }
        }
        if m.tame && !self.sneaking && !food(self, m, item) {
            self.player_start_riding(m);
            if m.mount.passengers.is_empty() {
                m.sea.home = None;
            }
            return Some(true);
        }
        Some(self.nautilus_animal_interact(m, slot, item))
    }

    /// Love (free adult) or growth (baby) on food.
    fn nautilus_animal_interact(&mut self, m: &mut Mob, slot: usize, item: &str) -> bool {
        if m.kind == Kind::ZombieNautilus || item.is_empty() {
            return false;
        }
        let food = if !m.tame && !m.is_baby() { "minecraft:nautilus_taming_items" } else { "minecraft:nautilus_food" };
        if !self.item_tag(item, food) {
            return false;
        }
        if m.age == 0 && m.in_love <= 0 {
            self.nautilus_use_item(slot, item);
            m.in_love = 600;
            m.love_cause = true;
            self.mob_event(m, 18);
            let snd = eat_sound(m);
            self.make_sound(m, snd);
            return true;
        }
        if m.is_baby() && !m.mount.age_locked {
            self.nautilus_use_item(slot, item);
            let secs = super::super::mounts::feeding_secs(-m.age);
            super::super::mounts::age_up(m, secs, true);
            let snd = eat_sound(m);
            self.make_sound(m, snd);
            return true;
        }
        false
    }

    /// Fish buckets give back a water bucket.
    fn nautilus_use_item(&mut self, slot: usize, item: &str) {
        if self.item_tag(item, "minecraft:nautilus_bucket_food") {
            let water = Stack { id: "minecraft:water_bucket".into(), count: 1, components: None };
            if self.game_mode == 1 {
                let has = self.slots.iter().flatten().any(|s| s.id == water.id);
                if !has && let Some(i) = (36..45).chain(9..36).find(|&i| self.slots[i].is_none()) {
                    self.slots[i] = Some(water);
                }
                return;
            }
            self.consume_held(slot);
            if self.slots[slot].is_none() {
                self.slots[slot] = Some(water);
            } else if let Some(i) = (36..45).chain(9..36).find(|&i| self.slots[i].is_none()) {
                self.slots[i] = Some(water);
            } else {
                self.drop_from_player(water, false);
            }
            return;
        }
        self.consume_held(slot);
    }

    /// Nutritional value of an item (component `food`).
    fn food_nutrition(&self, item: &str) -> Option<i32> {
        let s = Stack { id: item.to_owned(), count: 1, components: None };
        let v = self.life.items.default_component(&s, "minecraft:food")?;
        v["nutrition"].as_i64().map(|n| n as i32)
    }

    /// Saddle or armor placed on right click: tamed, adult.
    fn nautilus_equip(&mut self, m: &mut Mob, slot: usize, h: &Stack) -> bool {
        let Some(e) = self.equippable(h) else { return false };
        if !e.on_interact || !(e.slot == SADDLE || e.slot == BODY) || m.equipment[e.slot].is_some() {
            return false;
        }
        if !m.alive() || m.is_baby() || !m.tame {
            return false;
        }
        if e.allowed.as_ref().is_some_and(|l| !self.entity_in(m.kind, l)) {
            return false;
        }
        let one = Stack { count: 1, ..h.clone() };
        if self.game_mode != 1 {
            self.consume_held(slot);
        }
        m.drop_chance[e.slot] = 2.0;
        self.mount_set_slot(m, e.slot, Some(one));
        true
    }

    /// Mount inventory of the nautilus (saddle and armor only).
    fn open_nautilus_menu(&mut self, m: &mut Mob) {
        if !m.mount.passengers.is_empty() && !m.mount.passengers.contains(&Ent::Player) {
            return;
        }
        self.open_mount_menu(m);
    }
}

impl Server {
    /// Zombie nautilus ridden by a drowned (called by the drowned's family):
    /// created in its place and at its yaw, finalized (variant, cooldown), then mounted without forcing.
    pub(crate) fn zombie_nautilus_jockey(
        &mut self,
        rider: &mut Mob,
        reason: super::super::spawn::Reason,
        persistent: bool,
    ) -> Option<i32> {
        let mut z = self.new_mob(Kind::ZombieNautilus, rider.pos, rider.yaw);
        z.pitch = 0.0;
        z.head_yaw = z.yaw;
        z.body_yaw = z.yaw;
        if persistent {
            z.persistent = true;
        }
        let mut g = super::super::spawn::Group::default();
        self.finalize_spawn(&mut z, reason, &mut g);
        self.mob_start_riding(rider, &mut z, false);
        let id = z.id;
        self.add_mob(z);
        Some(id)
    }
}

/// Two adult nautiluses in love, distinct.
fn can_mate(m: &Mob, o: &Mob) -> bool {
    o.id != m.id && o.kind == Kind::Nautilus && m.in_love > 0 && o.in_love > 0 && o.age == 0 && m.age == 0
}

/// Anger set by a hit received from a living entity.
pub fn set_anger_target(s: &mut Server, m: &mut Mob, by: Ent) {
    s.nautilus_anger(m, by);
}
