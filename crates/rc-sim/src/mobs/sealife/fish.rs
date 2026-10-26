//! Fish: common base (swim control, movement in water, jump out of water, bucket),
//! schools (followed leader, school size), pufferfish (puffing, stings, poison), salmon
//! (sizes), tropical fish (patterns and colors packed into one variant).

use rc_proto::nbt::Nbt;

use super::super::goals::Who;
use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob, MoveOp};
use super::super::mth::{dist2, rotlerp_360, yaw_to};
use super::super::species::Kind;
use super::goals::AGoal;
use crate::player::Stack;
use crate::server::Server;

// ---------- Salmon ----------

pub const SALMON_MEDIUM: i32 = 1;

/// Box scale by size (clamped like the game's id).
pub fn salmon_scale(v: i32) -> f32 {
    match v.clamp(0, 2) {
        0 => 0.5,
        2 => 1.5,
        _ => 1.0,
    }
}

pub fn salmon_name(v: i32) -> &'static str {
    match v.clamp(0, 2) {
        0 => "small",
        2 => "large",
        _ => "medium",
    }
}

/// Size drawn: small 30, medium 50, large 15 (total 95).
pub fn random_salmon_size(r: &mut JRand) -> i32 {
    let n = r.next_int_bound(95);
    if n < 30 {
        0
    } else if n < 80 {
        1
    } else {
        2
    }
}

// ---------- Pufferfish ----------

/// Scale by puffing (small 0.5, medium 0.7, full 1).
pub fn puff_scale(p: i32) -> f32 {
    match p {
        0 => 0.5,
        1 => 0.7,
        _ => 1.0,
    }
}

// ---------- Tropical fish ----------

/// Packed pattern ids in the order of the game's enumeration (base | index << 8).
pub const PATTERNS: [i32; 12] = [0, 256, 512, 768, 1024, 1280, 1, 257, 513, 769, 1025, 1281];

/// Pattern, base color, pattern color → packed variant.
pub const fn pack(pattern: i32, base: i32, color: i32) -> i32 {
    (pattern & 0xFFFF) | ((base & 0xFF) << 16) | ((color & 0xFF) << 24)
}

/// The 22 common variants (game order).
pub const COMMON: [i32; 22] = [
    pack(257, 1, 7),   // striped, orange / gray
    pack(1, 7, 7),     // flopper gray / gray
    pack(1, 7, 11),    // flopper gray / blue
    pack(1281, 0, 7),  // clayfish white / gray
    pack(256, 11, 7),  // sunstreak blue / gray
    pack(0, 1, 0),     // kob orange / white
    pack(1280, 6, 3),  // spotty pink / light blue
    pack(769, 10, 4),  // blockfish purple / yellow
    pack(1281, 0, 14), // clayfish white / red
    pack(1280, 0, 4),  // spotty white / yellow
    pack(513, 0, 7),   // glitter white / gray
    pack(1281, 0, 1),  // clayfish white / orange
    pack(768, 9, 6),   // dasher cyan / pink
    pack(1024, 5, 3),  // brinely light green / light blue
    pack(1025, 14, 0), // betty red / white
    pack(512, 7, 14),  // snooper gray / red
    pack(769, 14, 0),  // blockfish red / white
    pack(1, 0, 4),     // flopper white / yellow
    pack(0, 14, 0),    // kob red / white
    pack(256, 7, 0),   // sunstreak gray / white
    pack(768, 9, 4),   // dasher cyan / yellow
    pack(1, 4, 4),     // flopper yellow / yellow
];

/// Variant read back: unknown pattern → kob, color out of bounds → white (game ids).
pub fn normalize(v: i32) -> i32 {
    let pat = v & 0xFFFF;
    let pat = if PATTERNS.contains(&pat) { pat } else { 0 };
    let col = |c: i32| if (0..16).contains(&c) { c } else { 0 };
    pack(pat, col((v >> 16) & 0xFF), col((v >> 24) & 0xFF))
}

pub const PATTERN_NAMES: [&str; 12] = [
    "kob",
    "sunstreak",
    "snooper",
    "dasher",
    "brinely",
    "spotty",
    "flopper",
    "stripey",
    "glitter",
    "blockfish",
    "betty",
    "clayfish",
];

pub const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// Finalization of the tropical fish (world random): variant of the group, common (90%) or
/// drawn at random (pattern, base, pattern) outside a school.
pub fn tropical_finalize(s: &mut Server, m: &mut Mob, group: &mut super::super::spawn::Group) {
    let r = &mut s.mobs.rng;
    let v = if let Some(v) = group.tropical_variant {
        v
    } else if r.next_float() < 0.9 {
        let v = COMMON[r.next_int_bound(COMMON.len() as i32) as usize];
        group.tropical_variant = Some(v);
        v
    } else {
        m.sea.not_school = true;
        let p = PATTERNS[r.next_int_bound(12) as usize];
        let b = r.next_int_bound(16);
        let c = r.next_int_bound(16);
        pack(p, b, c)
    };
    m.variant = v;
}

// ---------- Goals ----------

/// Goals of a fish: panic, flee from the player, random swim; schools; puffing.
pub fn register(m: &mut Mob) {
    use super::super::surface::{avoid, panic};
    m.goals.add(0, panic(1.25, "minecraft:panic_causes"));
    m.goals.add(2, avoid(Who::Player, 8.0, 1.6, 1.4));
    m.goals.add(4, AGoal::swim(1.0, 40));
    if super::is_schooling(m.kind) {
        // Start delay drawn at goal construction (entity random).
        let next = next_start(m);
        m.goals.add(5, AGoal::FollowLeader { recalc: 0, next_start: next }.wrap());
    }
    if m.kind == Kind::Pufferfish {
        m.goals.add(1, AGoal::Puff.wrap());
    }
}

/// 200 + (draw out of 200 reduced modulo 20), delay halved.
pub fn next_start(m: &mut Mob) -> i32 {
    super::super::mth::reduced(200 + m.rng.next_int_bound(200) % 20)
}

// ---------- Control and movement ----------

/// Fish swim control: gentle rise with eyes in water, smoothed speed, climb toward
/// the target, yaw turned by 90° at most.
pub fn move_control(m: &mut Mob) {
    if m.eyes_in_water {
        m.vel[1] += 0.005;
    }
    if m.move_ctl.op == MoveOp::MoveTo && !m.nav.is_done() {
        let target = (m.move_ctl.speed_mod * m.movement_speed()) as f32;
        let sp = m.speed + 0.125 * (target - m.speed);
        m.speed = sp;
        m.zza = sp;
        let w = m.move_ctl.wanted;
        let (xd, yd, zd) = (w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]);
        if yd != 0.0 {
            let dd = (xd * xd + yd * yd + zd * zd).sqrt();
            m.vel[1] += f64::from(sp) * (yd / dd) * 0.1;
        }
        if xd != 0.0 || zd != 0.0 {
            m.yaw = rotlerp_360(m.yaw, yaw_to(xd, zd), 90.0);
            m.body_yaw = m.yaw;
        }
    } else {
        m.speed = 0.0;
        m.zza = 0.0;
    }
}

/// Jump out of water (before the common one): on the ground, push and flop sound.
pub fn flop(s: &mut Server, m: &mut Mob) {
    if m.in_water || !m.on_ground || !m.vertical_collision {
        return;
    }
    let dx = f64::from((m.rng.next_float() * 2.0 - 1.0) * 0.05);
    let dz = f64::from((m.rng.next_float() * 2.0 - 1.0) * 0.05);
    m.vel[0] += dx;
    m.vel[1] += f64::from(0.4f32);
    m.vel[2] += dz;
    m.on_ground = false;
    m.needs_sync = true;
    let snd = match m.kind {
        Kind::Cod => "minecraft:entity.cod.flop",
        Kind::Salmon => "minecraft:entity.salmon.flop",
        Kind::Pufferfish => "minecraft:entity.puffer_fish.flop",
        _ => "minecraft:entity.tropical_fish.flop",
    };
    s.make_sound(m, snd);
}

// ---------- Pufferfish ----------

/// Puffing and deflating (before the common tick).
pub fn puff_tick(s: &mut Server, m: &mut Mob) {
    if !m.alive() || m.no_ai {
        return;
    }
    let a = &mut m.sea;
    if a.inflate > 0 {
        let mut snd = false;
        if a.puff == 0 {
            a.puff = 1;
            snd = true;
        } else if a.inflate > 40 && a.puff == 1 {
            a.puff = 2;
            snd = true;
        }
        a.inflate += 1;
        if snd {
            s.make_sound(m, "minecraft:entity.puffer_fish.blow_up");
        }
    } else if a.puff != 0 {
        let mut snd = false;
        if a.deflate > 60 && a.puff == 2 {
            a.puff = 1;
            snd = true;
        } else if a.deflate > 100 && a.puff == 1 {
            a.puff = 0;
            snd = true;
        }
        a.deflate += 1;
        if snd {
            s.make_sound(m, "minecraft:entity.puffer_fish.blow_out");
        }
    }
}

/// Tag `not_scary_for_pufferfish`: aquatic fauna, nautiluses, turtle, guardians, tadpole, sulfur cube
/// (the axolotl, though, scares).
pub(crate) fn not_scary(k: Kind) -> bool {
    super::handles(k)
        || matches!(k, Kind::Turtle | Kind::SulfurCube | Kind::Guardian | Kind::ElderGuardian | Kind::Tadpole)
}

/// Scary target: not a creative player, no species of the tag `not_scary_for_pufferfish`.
pub fn scary(s: &Server, e: Ent) -> bool {
    match e {
        Ent::Player => s.game_mode != 1 && s.game_mode != 3,
        Ent::Mob(id) => s.mob_ref(id).is_some_and(|o| o.alive() && !not_scary(o.kind)),
    }
}

/// The puffing goal: a scary entity within 2 blocks.
pub fn puff_can_use(s: &Server, m: &Mob) -> bool {
    let b = m.bbox();
    let area = rc_physics::Aabb { min: b.min.map(|v| v - 2.0), max: b.max.map(|v| v + 2.0) };
    let player = s.player_aabb().intersects(&area) && scary(s, Ent::Player) && s.ent_alive(Ent::Player);
    player || s.mobs.list.iter().flatten().any(|o| o.touches(&area) && scary(s, Ent::Mob(o.id)))
}

/// Stings of the mobs touched (after the common one), puffed only.
pub fn puff_touch(s: &mut Server, m: &mut Mob) {
    if !m.alive() || m.sea.puff <= 0 {
        return;
    }
    let b = m.bbox();
    let area = rc_physics::Aabb { min: b.min.map(|v| v - 0.3), max: b.max.map(|v| v + 0.3) };
    let ids: Vec<i32> = s
        .mobs
        .list
        .iter()
        .flatten()
        .filter(|o| o.touches(&area) && o.alive() && !not_scary(o.kind))
        .map(|o| o.id)
        .collect();
    let dmg = (1 + m.sea.puff) as f32;
    for id in ids {
        let Some(i) = s.mob_index(id) else { continue };
        let Some(mut o) = s.mobs.list[i].take() else { continue };
        if o.alive() && s.mob_hurt_by(&mut o, m, "minecraft:mob_attack", dmg) {
            s.give_mob_effect(&mut o, "minecraft:poison", 60 * m.sea.puff, 0);
            s.mob_sound(m, "minecraft:entity.puffer_fish.sting", 1.0, 1.0);
        }
        s.mobs.list[i] = Some(o);
    }
}

impl Server {
    /// The player touches puffed pufferfish: damage, poison, event.
    pub(crate) fn pufferfish_player_touch(&mut self) {
        if self.game_mode == 3 || self.life.vit.dead || self.life.vit.health <= 0.0 {
            return;
        }
        let pb = self.player_aabb();
        let area = rc_physics::Aabb {
            min: [pb.min[0] - 1.0, pb.min[1] - 0.5, pb.min[2] - 1.0],
            max: [pb.max[0] + 1.0, pb.max[1] + 0.5, pb.max[2] + 1.0],
        };
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| o.kind == Kind::Pufferfish && !o.removed && o.touches(&area))
            .map(|o| o.id)
            .collect();
        for id in ids {
            let Some(i) = self.mob_index(id) else { continue };
            let Some(m) = self.mobs.list[i].take() else { continue };
            let puff = m.sea.puff;
            if puff > 0 {
                let kb = Some((m.pos[0] - self.pos[0], m.pos[2] - self.pos[2]));
                if self.player_hurt_by_mob(&m, "minecraft:mob_attack", (1 + puff) as f32, kb) {
                    let _ = self.send(rc_client::Event::GameEvent { kind: 9, param: 0.0 });
                    self.add_effect_from(
                        crate::effects::Inst::new("minecraft:poison", 60 * puff, 0),
                        Some(crate::progress::pred::Who::Mob(&m)),
                    );
                }
            }
            self.mobs.list[i] = Some(m);
        }
    }
}

// ---------- Schools ----------

/// Is the followed leader alive?
pub fn is_follower(s: &Server, m: &Mob) -> bool {
    m.sea.leader.is_some_and(|l| s.mob_ref(l).is_some_and(Mob::alive))
}

pub fn max_school(k: Kind) -> i32 {
    if k == Kind::Salmon { 5 } else { 8 }
}

/// Finalization of a school: the first of the group becomes leader, the following ones follow it.
pub fn school_finalize(s: &mut Server, m: &mut Mob, group: &mut super::super::spawn::Group) {
    match group.school_leader {
        None => group.school_leader = Some(m.id),
        Some(l) => start_following(s, m, l),
    }
}

pub fn start_following(s: &mut Server, m: &mut Mob, leader: i32) {
    m.sea.leader = Some(leader);
    if let Some(i) = s.mob_index(leader)
        && let Some(l) = s.mobs.list[i].as_mut()
    {
        l.sea.school += 1;
    }
}

pub fn stop_following(s: &mut Server, m: &mut Mob) {
    if let Some(l) = m.sea.leader.take()
        && let Some(i) = s.mob_index(l)
        && let Some(o) = s.mobs.list[i].as_mut()
    {
        o.sea.school -= 1;
    }
}

/// End of a school fish's tick: 1 chance in 200 (world random) to recount the
/// neighbors; alone, the school falls back to 1 (followers keep their leader, as in the game).
pub fn school_tick(s: &mut Server, m: &mut Mob) {
    if m.sea.school > 1 && s.mobs.rng.next_int_bound(200) == 1 {
        let b = m.bbox();
        let area = rc_physics::Aabb { min: b.min.map(|v| v - 8.0), max: b.max.map(|v| v + 8.0) };
        // The fish itself counts (it is out of the list during its tick).
        let n = 1 + s.mobs.of_kind(m.kind).filter(|o| o.touches(&area)).count();
        if n <= 1 {
            m.sea.school = 1;
        }
    }
}

impl Server {
    /// The start condition of following the school leader.
    pub(crate) fn follow_leader_can_use(&mut self, m: &mut Mob, next: &mut i32) -> bool {
        if m.sea.school > 1 {
            return false;
        }
        if is_follower(self, m) {
            return true;
        }
        if *next > 0 {
            *next -= 1;
            return false;
        }
        *next = next_start(m);
        // Neighbors of the same species (the fish included, in its place in the entity list):
        // leaders with room, or free fish.
        let b = m.bbox();
        let area = rc_physics::Aabb { min: b.min.map(|v| v - 8.0), max: b.max.map(|v| v + 8.0) };
        let max = max_school(m.kind);
        let me_at = self.mobs.list.iter().position(Option::is_none);
        let mut list: Vec<(Option<i32>, bool, bool)> = Vec::new(); // (other, follower, can be followed)
        for (i, o) in self.mobs.list.iter().enumerate() {
            let entry = match o {
                None if Some(i) == me_at => (None, is_follower(self, m), false),
                Some(o) if o.kind == m.kind && o.touches(&area) => {
                    let s = o.sea.school;
                    (Some(o.id), is_follower(self, o), s > 1 && s < max)
                }
                _ => continue,
            };
            if entry.2 || !entry.1 {
                list.push(entry);
            }
        }
        // Leader: the first that can be followed, otherwise the fish itself.
        let leader = list.iter().find(|e| e.2).and_then(|e| e.0);
        let lschool = leader.and_then(|l| self.mob_ref(l)).map_or(m.sea.school, |o| o.sea.school);
        // The limit applies before discarding the leader itself.
        let room = (max - lschool).max(0) as usize;
        let free: Vec<Option<i32>> = list.iter().filter(|e| !e.1).map(|e| e.0).take(room).collect();
        for f in free {
            if f == leader {
                continue;
            }
            match (f, leader) {
                (None, Some(l)) => start_following(self, m, l),
                (Some(id), _) => {
                    let Some(i) = self.mob_index(id) else { continue };
                    let Some(mut o) = self.mobs.list[i].take() else { continue };
                    match leader {
                        Some(l) => start_following(self, &mut o, l),
                        None => {
                            o.sea.leader = Some(m.id);
                            m.sea.school += 1;
                        }
                    }
                    self.mobs.list[i] = Some(o);
                }
                (None, None) => {}
            }
        }
        is_follower(self, m)
    }

    /// Navigation toward the leader (speed 1).
    pub(crate) fn path_to_leader(&mut self, m: &mut Mob) {
        if !is_follower(self, m) {
            return;
        }
        let Some(p) = m.sea.leader.and_then(|l| self.ent_pos(Ent::Mob(l))) else { return };
        self.move_to_entity(m, p, 1.0);
    }

    pub(crate) fn in_range_of_leader(&self, m: &Mob) -> bool {
        m.sea.leader.and_then(|l| self.ent_pos(Ent::Mob(l))).is_some_and(|p| dist2(m.pos, p) <= 121.0)
    }
}

// ---------- Bucket ----------

/// Fish bucket item.
pub fn bucket_item(k: Kind) -> &'static str {
    match k {
        Kind::Cod => "minecraft:cod_bucket",
        Kind::Salmon => "minecraft:salmon_bucket",
        Kind::Pufferfish => "minecraft:pufferfish_bucket",
        _ => "minecraft:tropical_fish_bucket",
    }
}

/// Filled bucket: name, entity data, salmon size, tropical fish patterns.
pub fn bucket_of(m: &Mob) -> Stack {
    let mut b = Stack { id: bucket_item(m.kind).into(), count: 1, components: None };
    if let Some(n) = &m.custom_name {
        crate::items::set_component(&mut b, "minecraft:custom_name", n.clone());
    }
    let data = super::super::interact::bucket_default_data(m);
    crate::items::set_component(&mut b, "minecraft:bucket_entity_data", data);
    match m.kind {
        Kind::Salmon => {
            crate::items::set_component(&mut b, "minecraft:salmon/size", Nbt::String(salmon_name(m.variant).into()));
        }
        Kind::TropicalFish => {
            let v = normalize(m.variant);
            let pat = PATTERNS.iter().position(|p| *p == (v & 0xFFFF)).unwrap_or(0);
            let set = |b: &mut Stack, k: &str, s: &str| crate::items::set_component(b, k, Nbt::String(s.into()));
            set(&mut b, "minecraft:tropical_fish/pattern", PATTERN_NAMES[pat]);
            set(&mut b, "minecraft:tropical_fish/base_color", DYES[((v >> 16) & 0xF) as usize]);
            set(&mut b, "minecraft:tropical_fish/pattern_color", DYES[((v >> 24) & 0xF) as usize]);
        }
        _ => {}
    }
    b
}

/// Species contained in a fish bucket.
pub fn bucket_kind(item: &str) -> Option<Kind> {
    Some(match item {
        "minecraft:cod_bucket" => Kind::Cod,
        "minecraft:salmon_bucket" => Kind::Salmon,
        "minecraft:pufferfish_bucket" => Kind::Pufferfish,
        "minecraft:tropical_fish_bucket" => Kind::TropicalFish,
        // F10: tadpole and axolotl.
        "minecraft:tadpole_bucket" => Kind::Tadpole,
        "minecraft:axolotl_bucket" => Kind::Axolotl,
        _ => return None,
    })
}

impl Server {
    /// Fish bucket emptied against a block (water bucket then fish): water placed in the targeted
    /// cell if it contains liquid (waterloggable block), otherwise on the face; the fish spawns
    /// in that very cell even when the water had to go on the next face (like the game). False:
    /// nothing was emptied (the normal click handling continues).
    pub(crate) fn fish_bucket_use(&mut self, slot: usize, clicked: [i32; 3], face: usize) -> bool {
        let Some(bucket) = self.slots[slot].clone() else { return false };
        let Some(kind) = bucket_kind(&bucket.id) else { return false };
        // Adventure: the use test of an item at a refused place without a placement component.
        if self.game_mode == 2 || self.game_mode == 3 {
            return false;
        }
        let dirs = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
        let d = dirs[face.min(5)];
        let side = [clicked[0] + d[0], clicked[1] + d[1], clicked[2] + d[2]];
        let container = self.v().sim(self.get(clicked)) & rc_data::sim_bits::LIQUID_CONTAINER != 0;
        let place = if container { clicked } else { side };
        let sound = super::super::brainy::bucket_empty_sound(kind).unwrap_or("minecraft:item.bucket.empty_fish");
        if !self.empty_water(place, Some(side), sound) {
            return false;
        }
        self.spawn_from_bucket(kind, &bucket, place);
        // Creative: the bucket stays; otherwise it becomes an empty bucket again.
        if self.game_mode != 1 {
            self.slots[slot] = Some(Stack { id: "minecraft:bucket".into(), count: 1, components: None });
        }
        true
    }

    /// Emptying the contents of a water bucket (carried by the player); `retry`: fallback cell on the
    /// touched face when the targeted cell cannot receive the water.
    pub(crate) fn empty_water(&mut self, p: [i32; 3], retry: Option<[i32; 3]>, sound: &str) -> bool {
        let s = self.get(p);
        let v = self.v();
        let name = self.bname(p).to_owned();
        let portal = name == "minecraft:end_portal" || name == "minecraft:end_gateway";
        let may_replace = !portal && (v.replaceable(s) || !v.legacy_solid(s));
        let container = v.sim(s) & rc_data::sim_bits::LIQUID_CONTAINER != 0;
        let loggable = v.has(s, "waterlogged");
        let already = v.get_bool(s, "waterlogged");
        let can_hold = container && loggable && self.accepts_water(s, self.game_mode == 1);
        let place_liquid = may_replace || can_hold;
        let inside = v.is_air(s) || (place_liquid && (!self.sneaking || retry.is_none()));
        if !inside {
            return retry.is_some_and(|q| self.empty_water(q, None, sound));
        }
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        if self.dim.name == "minecraft:the_nether" {
            // The water evaporates: hissing, smoke; the fish will spawn anyway.
            let r = &mut self.mobs.rng;
            let pitch = 2.6 + (r.next_float() - r.next_float()) * 0.8;
            self.world_sound("minecraft:block.fire.extinguish", 4, at, 0.5, pitch);
            let corner = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
            self.particles("minecraft:large_smoke", corner, [1.0, 1.0, 1.0], 0.0, 8);
            return true;
        }
        if container {
            // Waterloggable block: the water enters it (other containers refuse without failing).
            if can_hold && !already {
                self.place_water_in(p, s); // G6-F01: campfire, dried ghast
            }
        } else {
            let liquid = name == "minecraft:water" || name == "minecraft:lava";
            if may_replace && !liquid {
                self.destroy_block(p, true, 512);
            }
            let water = crate::blocks::level::Fluid { kind: 1, source: true, amount: 8, falling: false };
            let ns = self.legacy_block(water);
            let was_source = self.fluid_of(s).source;
            if !self.set_block(p, ns, crate::blocks::flag::ALL | 8) && !was_source {
                return false;
            }
        }
        // Own sound of the fish bucket (neutral source), at the cell.
        self.world_sound(sound, 6, at, 1.0, 1.0);
        true
    }

    /// Fish taken out of the bucket: creation at the center of the cell
    /// (vertical offset), "bucket" finalization, name and components of the item, bucket
    /// data, "taken out of a bucket" flag, then ambient sound.
    pub(crate) fn spawn_from_bucket(&mut self, kind: Kind, bucket: &Stack, p: [i32; 3]) {
        let y_off = self.spawn_y_offset(p, false, kind);
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]) + y_off, f64::from(p[2]) + 0.5];
        let yaw = super::super::mth::wrap(self.mobs.rng.next_float() * 360.0);
        let mut m = self.new_mob(kind, pos, yaw);
        m.head_yaw = m.yaw;
        m.body_yaw = m.yaw;
        let mut g = super::super::spawn::Group::default();
        self.finalize_spawn(&mut m, super::super::spawn::Reason::Bucket, &mut g);
        let comp = |k: &str| crate::items::component(bucket, k).cloned();
        if let Some(n) = comp("minecraft:custom_name") {
            m.custom_name = Some(n);
        }
        let name_of = |k: &str| comp(k).and_then(|n| n.as_str().map(str::to_owned));
        match kind {
            Kind::Salmon => {
                if let Some(sz) = name_of("minecraft:salmon/size") {
                    m.variant = match sz.as_str() {
                        "small" => 0,
                        "large" => 2,
                        _ => SALMON_MEDIUM,
                    };
                }
            }
            Kind::TropicalFish => {
                let mut v = m.variant;
                if let Some(i) = name_of("minecraft:tropical_fish/pattern")
                    .and_then(|n| PATTERN_NAMES.iter().position(|p| *p == n.trim_start_matches("minecraft:")))
                {
                    v = (v & !0xFFFF) | PATTERNS[i];
                }
                let dye = |k: &str| name_of(k).and_then(|n| DYES.iter().position(|d| *d == n)).map(|i| i as i32);
                if let Some(c) = dye("minecraft:tropical_fish/base_color") {
                    v = (v & !(0xFF << 16)) | (c << 16);
                }
                if let Some(c) = dye("minecraft:tropical_fish/pattern_color") {
                    v = (v & 0x00FF_FFFF) | (c << 24);
                }
                m.variant = v;
            }
            _ => {}
        }
        if let Some(data) = comp("minecraft:bucket_entity_data") {
            super::super::interact::load_bucket_default_data(&mut m, &data);
        }
        // F10: own bucket data (tadpole, axolotl).
        if super::super::brainy::handles(kind) {
            super::super::brainy::load_from_bucket(&mut m, bucket);
        } else {
            m.sea.from_bucket = true;
        }
        let ambient = if super::super::brainy::handles(kind) {
            super::super::brainy::ambient_sound(&m)
        } else {
            super::ambient_sound(&m)
        };
        if let Some(a) = ambient {
            self.make_sound(&mut m, &a);
        }
        self.add_mob(m);
    }
}

/// Pickup with a water bucket; `None`: other item.
pub fn bucket_pickup(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if item != "minecraft:water_bucket" || !m.alive() {
        return None;
    }
    s.mob_sound(m, "minecraft:item.bucket.fill_fish", 1.0, 1.0);
    let filled = bucket_of(m);
    // The exchange for the filled item without creative limit: the water bucket stays in creative.
    if s.game_mode != 1 {
        s.consume_held(slot);
    }
    if s.slots[slot].is_none() {
        s.slots[slot] = Some(filled);
    } else if let Some(i) = (36..45).chain(9..36).find(|&i| s.slots[i].is_none()) {
        s.slots[i] = Some(filled);
    } else {
        s.drop_from_player(filled, false);
    }
    m.removed = true;
    Some(true)
}
