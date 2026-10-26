//! Weapons and throwing items of the player (`use`, item release, use tick): bow (charge, power,
//! critical, infinity, flame), crossbow (charge, sounds, multishot, piercing, rockets), trident
//! (throw, impulse), snowballs, eggs, pearls, thrown potions, experience bottles,
//! rockets (elytra), item cooldowns (`use_cooldown`).

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::{PKind, Pickup, Proj};
use crate::items::{component, remove_component, set_component};
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

/// Degrees → radians factor in single float: `π / 180`.
const RAD: f32 = (std::f64::consts::PI / 180.0) as f32;

/// Use duration of bows, crossbows and tridents.
pub const USE_DURATION: i32 = 72000;

/// Bow power according to the draw time.
pub fn bow_power(time_held: i32) -> f32 {
    let mut p = time_held as f32 / 20.0;
    p = (p * p + p * 2.0) / 3.0;
    p.min(1.0)
}

/// Explosions of a rocket (`fireworks` component).
pub fn firework_explosions(s: &Stack) -> usize {
    component(s, "minecraft:fireworks").map_or(0, |f| crate::nbtio::list(f.get("explosions")).len())
}

pub fn firework_has_explosion(s: &Stack) -> bool {
    firework_explosions(s) > 0
}

fn flight_duration(s: &Stack) -> i32 {
    component(s, "minecraft:fireworks").and_then(|f| f.get("flight_duration")).and_then(Nbt::as_i64).unwrap_or(1) as i32
}

/// Aiming vector (tabulated sines).
pub fn view_vector(pitch: f32, yaw: f32) -> [f32; 3] {
    let f = pitch * RAD;
    let g = -yaw * RAD;
    let (h, i) = (rc_physics::cos_t(g), rc_physics::sin_t(g));
    let (j, k) = (rc_physics::cos_t(f), rc_physics::sin_t(f));
    [i * j, -k, h * j]
}

/// Rotation of a vector around a unit axis (JOML quaternion, floats).
pub(crate) fn rotate_axis(v: [f32; 3], angle: f32, axis: [f32; 3]) -> [f32; 3] {
    let half = f64::from(angle) * 0.5;
    let s = half.sin() as f32;
    let (qx, qy, qz, qw) = (axis[0] * s, axis[1] * s, axis[2] * s, half.cos() as f32);
    // Rotation of a vector by a quaternion: expanded formula of JOML.
    let (xx, yy, zz, ww) = (qx * qx, qy * qy, qz * qz, qw * qw);
    let (xy, xz, yz) = (qx * qy, qx * qz, qy * qz);
    let (xw, yw, zw) = (qx * qw, qy * qw, qz * qw);
    let k = 1.0 / (xx + yy + zz + ww);
    let [x, y, z] = v;
    [
        ((xx - yy - zz + ww) * k) * x + (2.0 * (xy - zw) * k) * y + (2.0 * (xz + yw) * k) * z,
        (2.0 * (xy + zw) * k) * x + ((yy - xx - zz + ww) * k) * y + (2.0 * (yz - xw) * k) * z,
        (2.0 * (xz - yw) * k) * x + (2.0 * (yz + xw) * k) * y + ((zz - xx - yy + ww) * k) * z,
    ]
}

impl Proj {
    /// Normalised direction, triangular inaccuracy, power; rotation.
    pub fn shoot(&mut self, d: [f64; 3], pow: f32, unc: f32) {
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let mut v = d.map(|c| if l < 1.0e-5 { 0.0 } else { c / l });
        for c in &mut v {
            *c += self.rng.triangle(0.0, 0.017_227_5 * f64::from(unc));
        }
        let v = v.map(|c| c * f64::from(pow));
        self.vel = v;
        self.needs_sync = true;
        let sd = (v[0] * v[0] + v[2] * v[2]).sqrt();
        self.yaw = (crate::mobs::mth::atan2(v[0], v[2]) * super::DEG) as f32;
        self.pitch = (crate::mobs::mth::atan2(v[1], sd) * super::DEG) as f32;
        self.life = 0;
    }
}

impl Server {
    /// Known movement of the player added to the shot (vertical zero on the ground).
    fn known_movement(&self) -> [f64; 3] {
        let m = self.proj_player.known_move;
        [m[0], if self.on_ground { 0.0 } else { m[1] }, m[2]]
    }

    /// The shot from a rotation from the player.
    pub(crate) fn shoot_from_player(&self, p: &mut Proj, pitch: f32, yaw: f32, y_off: f32, pow: f32, unc: f32) {
        let xd = -rc_physics::sin_t(yaw * RAD) * rc_physics::cos_t(pitch * RAD);
        let yd = -rc_physics::sin_t((pitch + y_off) * RAD);
        let zd = rc_physics::cos_t(yaw * RAD) * rc_physics::cos_t(pitch * RAD);
        p.shoot([f64::from(xd), f64::from(yd), f64::from(zd)], pow, unc);
        let m = self.known_movement();
        p.vel = [p.vel[0] + m[0], p.vel[1] + m[1], p.vel[2] + m[2]];
    }

    pub(crate) fn hand_slot(&self, hand: i32) -> usize {
        if hand == 0 { 36 + self.held as usize } else { 45 }
    }

    pub(crate) fn infinite_materials(&self) -> bool {
        self.game_mode == 1
    }

    /// `use_item` of throwing items; true if the item is handled here.
    pub(crate) fn proj_use_item(&mut self, hand: i32) -> bool {
        if self.game_mode == 3 || self.life.vit.dead {
            return false;
        }
        let slot = self.hand_slot(hand);
        let Some(s) = self.slots[slot].clone() else { return false };
        let id = s.id.trim_start_matches("minecraft:").to_owned();
        // Blank map (S8, `maps.rs`).
        if id == "map" {
            if !self.on_cooldown(&s) && self.empty_map_use(slot) {
                self.s8_used(&s.id); // S9
            }
            return true;
        }
        // G5-F10: Eye of Ender thrown towards the located structure.
        if id == "ender_eye" {
            return self.ender_eye_use(hand);
        }
        let handled = matches!(
            id.as_str(),
            "bow"
                | "crossbow"
                | "trident"
                | "snowball"
                | "egg"
                | "blue_egg"
                | "brown_egg"
                | "ender_pearl"
                | "splash_potion"
                | "lingering_potion"
                | "experience_bottle"
                | "firework_rocket"
                | "fishing_rod"
                | "wind_charge"
        );
        if !handled {
            return false;
        }
        if self.on_cooldown(&s) {
            return true;
        }
        let used = match id.as_str() {
            "bow" => self.bow_use(hand, &s),
            "crossbow" => self.crossbow_use(hand, &s),
            "trident" => self.trident_use(hand, &s),
            "fishing_rod" => self.fishing_rod_use(hand),
            "firework_rocket" => self.firework_use(hand),
            "wind_charge" => self.wind_charge_use(hand, &s), // F14
            _ => self.proj_throw(hand, &s),
        };
        if used {
            self.apply_use_cooldown(&s);
            // S9: "used" on throw (bow, crossbow and trident: on shot).
            if !matches!(id.as_str(), "bow" | "crossbow" | "trident") {
                self.s8_used(&s.id);
            }
        }
        self.broadcast_menus();
        true
    }

    /// Click of a rocket on a block; true if handled.
    pub(crate) fn proj_use_on(&mut self, hand: i32, pos: [i32; 3], face: usize, at: [f32; 3]) -> bool {
        let slot = self.hand_slot(hand);
        let Some(s) = self.slots[slot].clone() else { return false };
        // Adventure: use on the block goes through `can_place_on` (refusal handled further on).
        if self.game_mode == 2 && !self.adventure_can_place_on(Some(&s), self.get(pos)) {
            return false;
        }
        if crate::maps::is_map_item(&s.id) {
            let ok = self.map_use_on_banner(slot, pos);
            if ok {
                self.award_stat("minecraft:used", &s.id, 1);
            }
            return ok;
        }
        if s.id != "minecraft:firework_rocket" || self.proj_player.fall_flying {
            return false;
        }
        let d = super::physics::FACES[face.min(5)];
        let c = [
            f64::from(pos[0]) + f64::from(at[0]),
            f64::from(pos[1]) + f64::from(at[1]),
            f64::from(pos[2]) + f64::from(at[2]),
        ];
        let at = [c[0] + f64::from(d[0]) * 0.15, c[1] + f64::from(d[1]) * 0.15, c[2] + f64::from(d[2]) * 0.15];
        let mut one = s.clone();
        one.count = 1;
        let p = self.new_firework(at, one, Some(self.uuid), false);
        self.proj_spawn(p, &s);
        // Creative: the stack count is given back after use on a block.
        if self.game_mode != 1
            && let Some(st) = &mut self.slots[slot]
        {
            st.count -= 1;
            if st.count <= 0 {
                self.slots[slot] = None;
            }
        }
        self.award_stat("minecraft:used", &s.id, 1);
        self.broadcast_menus();
        true
    }

    // ---------- Cooldowns ----------

    fn cooldown_group(&self, s: &Stack) -> Option<(String, i32)> {
        let c = component(s, "minecraft:use_cooldown")
            .map(|n| {
                let secs = match n.get("seconds") {
                    Some(Nbt::Float(f)) => f64::from(*f),
                    Some(Nbt::Double(d)) => *d,
                    _ => 0.0,
                };
                (secs, n.get("cooldown_group").and_then(Nbt::as_str).map(str::to_owned))
            })
            .or_else(|| {
                let v = self.life.items.default_component(s, "minecraft:use_cooldown")?;
                Some((v["seconds"].as_f64().unwrap_or(0.0), v["cooldown_group"].as_str().map(str::to_owned)))
            })?;
        let ticks = (c.0 * 20.0) as i32;
        (ticks > 0).then(|| (c.1.unwrap_or_else(|| s.id.clone()), ticks))
    }

    pub(crate) fn on_cooldown(&self, s: &Stack) -> bool {
        let group = self.cooldown_group(s).map_or_else(|| s.id.clone(), |g| g.0);
        let now = self.proj_player.ticks;
        self.proj_player.cooldowns.iter().any(|(g, _, end)| *g == group && *end > now)
    }

    fn apply_use_cooldown(&mut self, s: &Stack) {
        if let Some((g, t)) = self.cooldown_group(s) {
            self.add_cooldown(&g, t);
        }
    }

    pub(crate) fn add_cooldown(&mut self, group: &str, ticks: i32) {
        let now = self.proj_player.ticks;
        self.proj_player.cooldowns.retain(|c| c.0 != group);
        self.proj_player.cooldowns.push((group.to_owned(), now, now + ticks));
        // `cooldown` packet to the player (veil on the client's slots).
        let _ = self.send(rc_client::Event::Cooldown { group: group.to_owned(), ticks });
    }

    // ---------- Bow ----------

    /// Projectile available for a weapon: inventory slot.
    fn find_ammo(&self, weapon: &Stack) -> Option<(Option<usize>, Stack)> {
        let crossbow = weapon.id == "minecraft:crossbow";
        let is_arrow = |s: &Stack| self.life.items.has_tag("minecraft:arrows", &s.id);
        let held_ok = |s: &Stack| is_arrow(s) || (crossbow && s.id == "minecraft:firework_rocket");
        for slot in [45, 36 + self.held as usize] {
            if let Some(s) = &self.slots[slot]
                && held_ok(s)
            {
                return Some((Some(slot), s.clone()));
            }
        }
        // Inventory: hotbar (0-8) then main storage (9-35), then off hand.
        for slot in (36..45).chain(9..36).chain([45]) {
            if let Some(s) = &self.slots[slot]
                && is_arrow(s)
            {
                return Some((Some(slot), s.clone()));
            }
        }
        self.infinite_materials().then(|| (None, Stack { id: "minecraft:arrow".into(), count: 1, components: None }))
    }

    fn bow_use(&mut self, hand: i32, w: &Stack) -> bool {
        if !self.infinite_materials() && self.find_ammo(w).is_none() {
            return false;
        }
        self.start_using(hand, USE_DURATION);
        true
    }

    /// `draw`: projectiles taken (multishot: free copies beyond the first).
    fn draw(&mut self, weapon: &Stack, ammo_slot: Option<usize>, ammo: &Stack) -> Vec<Stack> {
        let n = self.ench_value(
            weapon,
            "minecraft:projectile_count",
            1.0,
            &super::ench::Ctx { tool: Some(weapon), ..Default::default() },
        ) as i32;
        let mut out = Vec::new();
        for i in 0..n.max(1) {
            let force_inf = i > 0;
            let use_n = if !force_inf && !self.infinite_materials() {
                let ctx = super::ench::Ctx { tool: Some(ammo), ..Default::default() };
                self.ench_value(weapon, "minecraft:ammo_use", 1.0, &ctx) as i32
            } else {
                0
            };
            let have = ammo_slot.and_then(|s| self.slots[s].as_ref()).map_or(ammo.count, |s| s.count);
            if use_n > have {
                continue;
            }
            if use_n == 0 {
                let mut c = ammo.clone();
                c.count = 1;
                set_component(&mut c, "minecraft:intangible_projectile", Nbt::Compound(Vec::new()));
                out.push(c);
                continue;
            }
            let mut used = ammo.clone();
            used.count = use_n;
            if let Some(s) = ammo_slot
                && let Some(st) = &mut self.slots[s]
            {
                st.count -= use_n;
                if st.count <= 0 {
                    self.slots[s] = None;
                }
            }
            out.push(used);
        }
        out
    }

    /// Item release (packet "release use"); true if handled.
    pub(crate) fn proj_release(&mut self) -> bool {
        let Some((hand, left)) = self.life.vit.using else { return false };
        let slot = self.hand_slot(hand);
        let Some(w) = self.slots[slot].clone() else { return false };
        let held = USE_DURATION - left;
        match w.id.as_str() {
            "minecraft:bow" => self.bow_release(hand, &w, held),
            "minecraft:trident" => self.trident_release(hand, &w, held),
            "minecraft:crossbow" => {}
            _ => return false,
        }
        self.use_stack_emptied(slot); // thrown trident only: the end does not vibrate
        self.stop_using();
        self.broadcast_menus();
        true
    }

    fn bow_release(&mut self, hand: i32, w: &Stack, held: i32) {
        let Some((slot, ammo)) = self.find_ammo(w) else { return };
        let pow = bow_power(held);
        if pow < 0.1 {
            return;
        }
        let drawn = self.draw(w, slot, &ammo);
        if !drawn.is_empty() {
            self.weapon_shoot(hand, w, drawn, pow * 3.0, 1.0, pow == 1.0);
        }
        let pitch = 1.0 / (self.life.rng.next_float() * 0.4 + 1.2) + pow * 0.5;
        self.play_sound("minecraft:entity.arrow.shoot", 7, self.pos, 1.0, pitch);
        self.s8_used(&w.id); // S9
    }

    /// Arrow created by the player (the creation of an item's arrow + the base arrow constructor):
    /// author the player, pickup allowed.
    fn create_arrow(&mut self, ammo: &Stack, weapon: Option<&Stack>) -> Proj {
        let pos = [self.pos[0], self.player_eye_y() - f64::from(0.1f32), self.pos[2]];
        let mut p = self.new_arrow(pos, ammo, weapon);
        p.owner = Some(self.uuid);
        if p.pickup == Pickup::Disallowed {
            p.pickup = Pickup::Allowed;
        }
        p
    }

    /// New arrow at `pos` (picked-up stack, weapon, piercing, colour), without an author: common to the
    /// player and the mobs (`mobs/arrows.rs`, `mobs/illagers/crossbow.rs`).
    pub(crate) fn new_arrow(&mut self, pos: [f64; 3], ammo: &Stack, weapon: Option<&Stack>) -> Proj {
        let kind = if ammo.id == "minecraft:spectral_arrow" { PKind::SpectralArrow } else { PKind::Arrow };
        let id = self.proj_new_id();
        let mut pick = ammo.clone();
        pick.count = 1;
        let mut p = Proj::new(kind, id, pos, pick.clone());
        if component(&pick, "minecraft:intangible_projectile").is_some() {
            remove_component(&mut p.item, "minecraft:intangible_projectile");
            p.pickup = Pickup::CreativeOnly;
        }
        if let Some(w) = weapon {
            p.weapon = Some(w.clone());
            let ctx = super::ench::Ctx { tool: Some(&p.item), ..Default::default() };
            let pierce = self.ench_value(w, "minecraft:projectile_piercing", 0.0, &ctx) as i32;
            if pierce > 0 {
                p.pierce = pierce.clamp(0, 127) as u8;
            }
        }
        if kind == PKind::Arrow {
            let color = if component(&p.item, "minecraft:potion_contents").is_some() {
                super::hits::potion_color(&p.item)
            } else {
                -1
            };
            p.set_meta(11, Meta::Int(color));
        }
        p
    }

    /// Fan of the multishot, wear per projectile.
    fn weapon_shoot(&mut self, hand: i32, w: &Stack, list: Vec<Stack>, power: f32, unc: f32, crit: bool) {
        let max_angle = self.ench_value(
            w,
            "minecraft:projectile_spread",
            0.0,
            &super::ench::Ctx { tool: Some(w), ..Default::default() },
        );
        let n = list.len();
        let step = if n == 1 { 0.0 } else { 2.0 * max_angle / (n - 1) as f32 };
        let offset = ((n - 1) % 2) as f32 * step / 2.0;
        let mut dir = 1.0f32;
        let crossbow = w.id == "minecraft:crossbow";
        let slot = self.hand_slot(hand);
        for (i, ammo) in list.into_iter().enumerate() {
            let angle = offset + dir * i.div_ceil(2) as f32 * step;
            dir = -dir;
            let mut p = if crossbow && ammo.id == "minecraft:firework_rocket" {
                let pos = [self.pos[0], self.player_eye_y() - f64::from(0.15f32), self.pos[2]];
                self.new_firework(pos, ammo.clone(), Some(self.uuid), true)
            } else {
                let mut a = self.create_arrow(&ammo, Some(w));
                if crit {
                    a.crit = true;
                }
                if crossbow {
                    a.hit_sound = "minecraft:item.crossbow.hit";
                }
                a
            };
            if crossbow {
                let view = view_vector(self.pitch, self.yaw);
                let up = view_vector(self.pitch - 90.0, self.yaw);
                let shot = rotate_axis(view, angle * RAD, up);
                p.shoot(shot.map(f64::from), power, unc);
                let pitch = if i == 0 {
                    1.0
                } else {
                    let high = (i & 1) == 1;
                    1.0 / (self.life.rng.next_float() * 0.5 + 1.8) + if high { 0.63 } else { 0.43 }
                };
                self.play_sound("minecraft:item.crossbow.shoot", 7, self.pos, 1.0, pitch);
            } else {
                self.shoot_from_player(&mut p, self.pitch, self.yaw + angle, 0.0, power, unc);
            }
            self.proj_spawn(p, &ammo);
            let wear = if crossbow && ammo.id == "minecraft:firework_rocket" { 3 } else { 1 };
            self.hurt_item(slot, wear);
            if self.slots[slot].is_none() {
                break;
            }
        }
    }

    /// Addition, then spawn effects (flame) of the stack and of
    /// the weapon.
    pub(crate) fn proj_spawn(&mut self, mut p: Proj, pickup: &Stack) {
        let mut fire = self.ench_spawn_fire(pickup);
        if let Some(w) = p.weapon.clone()
            && w.id != pickup.id
        {
            fire = fire.max(self.ench_spawn_fire(&w));
        }
        if fire > 0 && p.fire < fire {
            p.fire = fire;
            p.sync_fire_flag();
        }
        self.proj_add(p);
    }

    // ---------- Crossbow ----------

    pub(crate) fn charged(s: &Stack) -> Vec<Stack> {
        component(s, "minecraft:charged_projectiles")
            .map(|n| crate::nbtio::list(Some(n)).iter().filter_map(Stack::from_nbt).collect())
            .unwrap_or_default()
    }

    fn crossbow_use(&mut self, hand: i32, w: &Stack) -> bool {
        let charged = Self::charged(w);
        if !charged.is_empty() {
            let power = if charged.iter().any(|s| s.id == "minecraft:firework_rocket") { 1.6 } else { 3.15 };
            let slot = self.hand_slot(hand);
            if let Some(st) = &mut self.slots[slot] {
                remove_component(st, "minecraft:charged_projectiles");
            }
            let weapon = self.slots[slot].clone().unwrap_or_else(|| w.clone());
            self.weapon_shoot(hand, &weapon, charged, power, 1.0, true);
            self.s8_shot_crossbow(&weapon); // S9
            self.s8_used(&weapon.id);
            return true;
        }
        if self.find_ammo(w).is_some() {
            self.proj_player.xbow_sounds = (false, false);
            self.start_using(hand, USE_DURATION);
            return true;
        }
        false
    }

    /// Charge duration (1.25 s modified by quick charge).
    fn charge_duration(&self, w: &Stack) -> i32 {
        let d = self.ench_value(w, "minecraft:crossbow_charge_time", 1.25, &super::ench::Ctx::default());
        (d * 20.0).floor() as i32
    }

    /// Use tick of the items in use (crossbow: sounds and loading).
    pub(crate) fn proj_use_tick(&mut self) {
        let Some((hand, left)) = self.life.vit.using else { return };
        let slot = self.hand_slot(hand);
        let Some(w) = self.slots[slot].clone() else { return };
        if w.id != "minecraft:crossbow" {
            return;
        }
        let dur = self.charge_duration(&w).max(1);
        let pct = (USE_DURATION - left) as f32 / dur as f32;
        let sounds = self.ench_highest(&w, "minecraft:crossbow_charging_sounds");
        let snd = |k: &str, d: &str| -> Option<String> {
            match &sounds {
                Some(v) => v[k].as_str().map(str::to_owned),
                None => Some(d.to_owned()),
            }
        };
        if pct < 0.2 {
            self.proj_player.xbow_sounds = (false, false);
        }
        if pct >= 0.2 && !self.proj_player.xbow_sounds.0 {
            self.proj_player.xbow_sounds.0 = true;
            if let Some(s) = snd("start", "minecraft:item.crossbow.loading_start") {
                self.play_sound(&s, 7, self.pos, 0.5, 1.0);
            }
        }
        if pct >= 0.5 && !self.proj_player.xbow_sounds.1 {
            self.proj_player.xbow_sounds.1 = true;
            if let Some(s) = snd("mid", "minecraft:item.crossbow.loading_middle") {
                self.play_sound(&s, 7, self.pos, 0.5, 1.0);
            }
        }
        if pct >= 1.0
            && Self::charged(&w).is_empty()
            && let Some((aslot, ammo)) = self.find_ammo(&w)
        {
            let drawn = self.draw(&w, aslot, &ammo);
            if !drawn.is_empty() {
                let list = Nbt::List(drawn.iter().map(|s| s.to_nbt(None)).collect());
                if let Some(st) = &mut self.slots[slot] {
                    set_component(st, "minecraft:charged_projectiles", list);
                }
                if let Some(s) = snd("end", "minecraft:item.crossbow.loading_end") {
                    let pitch = 1.0 / (self.life.rng.next_float() * 0.5 + 1.0) + 0.2;
                    self.play_sound(&s, 7, self.pos, 1.0, pitch);
                }
                self.broadcast_menus();
            }
        }
    }

    // ---------- Trident ----------

    fn next_damage_breaks(&self, s: &Stack) -> bool {
        self.life.items.damageable(s) && self.life.items.damage(s) + 1 >= self.life.items.max_damage(s)
    }

    fn riptide(&self, s: &Stack) -> f32 {
        self.ench_value(s, "minecraft:trident_spin_attack_strength", 0.0, &super::ench::Ctx::default())
    }

    /// Player in water or under rain.
    fn player_wet(&self) -> bool {
        // G6-F12: same test as the conduit (box touching water, rain at the top of the box).
        self.player_in_water_or_rain()
    }

    fn trident_use(&mut self, hand: i32, s: &Stack) -> bool {
        if self.next_damage_breaks(s) || (self.riptide(s) > 0.0 && !self.player_wet()) {
            return false;
        }
        self.start_using(hand, USE_DURATION);
        true
    }

    fn trident_release(&mut self, hand: i32, s: &Stack, held: i32) {
        if held < 10 {
            return;
        }
        let r = self.riptide(s);
        let riding = self.player_riding_any();
        if r > 0.0 && !(self.player_wet() && !riding) {
            return;
        }
        if self.next_damage_breaks(s) {
            return;
        }
        let sound = self
            .ench_highest(s, "minecraft:trident_sound")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "minecraft:item.trident.throw".to_owned());
        let slot = self.hand_slot(hand);
        // Damage without breaking.
        if self.game_mode != 1
            && let Some(st) = self.slots[slot].clone()
        {
            self.hurt_item(slot, 1);
            if self.slots[slot].is_none() {
                self.slots[slot] = Some(st);
            }
        }
        if r == 0.0 {
            let Some(mut thrown) = self.slots[slot].clone() else { return };
            thrown.count = 1;
            if !self.infinite_materials()
                && let Some(st) = &mut self.slots[slot]
            {
                st.count -= 1;
                if st.count <= 0 {
                    self.slots[slot] = None;
                }
            }
            let id = self.proj_new_id();
            let pos = [self.pos[0], self.player_eye_y() - f64::from(0.1f32), self.pos[2]];
            let mut p = Proj::new(PKind::Trident, id, pos, thrown.clone());
            p.owner = Some(self.uuid);
            p.pickup = if self.infinite_materials() { Pickup::CreativeOnly } else { Pickup::Allowed };
            let ctx = super::ench::Ctx { this: Some("minecraft:trident"), ..Default::default() };
            p.loyalty =
                self.ench_value(&thrown, "minecraft:trident_return_acceleration", 0.0, &ctx).clamp(0.0, 127.0) as u8;
            let foil = crate::enchanting::has_any(&self.life.items, &thrown)
                || component(&thrown, "minecraft:enchantment_glint_override")
                    .and_then(Nbt::as_i64)
                    .is_some_and(|v| v != 0);
            p.set_meta(11, Meta::Byte(p.loyalty as i8));
            p.set_meta(12, Meta::Bool(foil));
            self.shoot_from_player(&mut p, self.pitch, self.yaw, 0.0, 2.5, 1.0);
            let at = p.pos;
            self.proj_spawn(p, &thrown);
            self.play_sound(&sound, 7, at, 1.0, 1.0);
            self.s8_used(&s.id); // S9
            return;
        }
        self.s8_used(&s.id); // S9
        // Impulse: the client pushes the player; the server arms the spin attack.
        self.proj_player.spin = 20;
        self.proj_player.spin_damage = 8.0;
        self.proj_player.spin_item = self.slots[slot].clone();
        self.send_living_flags();
        self.play_sound(&sound, 7, self.pos, 1.0, 1.0);
    }

    /// Living flags of the player (data 8): uses an item (1), off hand (2),
    /// spin attack (4).
    fn living_flags(&self) -> i8 {
        let mut f = 0;
        if let Some((hand, _)) = self.life.vit.using {
            f |= 1 | if hand == 1 { 2 } else { 0 };
        }
        if self.proj_player.spin > 0 {
            f |= 4;
        }
        f
    }

    /// Sends the living flags of the player.
    pub(crate) fn send_living_flags(&mut self) {
        let f = self.living_flags();
        self.proj_player.flags_sent = f;
        let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Data {
            id: self.player_id,
            values: vec![(8, Meta::Byte(f))],
        }));
    }

    /// End of tick: sends the flags again if they changed. A use stopped by the server
    /// (shield disarmed, meal finished…) thus also stops the one the client predicts.
    pub(crate) fn sync_living_flags(&mut self) {
        if self.living_flags() != self.proj_player.flags_sent {
            self.send_living_flags();
        }
    }

    /// Spin attack: the first mob touched takes the
    /// impulse damage; the attack stops at the first hit or after 20 ticks.
    pub(crate) fn spin_attack_tick(&mut self) {
        if self.proj_player.spin <= 0 {
            return;
        }
        let b = self.player_aabb();
        let hit = self.mobs.list.iter().flatten().find(|m| m.alive() && m.touches(&b)).map(|m| m.id);
        if let Some(id) = hit {
            let dmg = self.proj_player.spin_damage;
            if let Some(i) = self.mob_index(id)
                && let Some(mut m) = self.mobs.list[i].take()
            {
                self.mob_hurt(&mut m, "minecraft:player_attack", dmg, Some(Ent::Player), Some(self.pos));
                self.mobs.list[i] = Some(m);
            }
            self.proj_player.spin = 0;
            self.mobs.player_vel = self.mobs.player_vel.map(|v| v * -0.2);
        } else {
            self.proj_player.spin -= 1;
        }
        if self.proj_player.spin == 0 {
            self.proj_player.spin_item = None;
            self.send_living_flags();
        }
    }

    // ---------- Thrown items ----------

    fn proj_throw(&mut self, hand: i32, s: &Stack) -> bool {
        let (kind, sound, cat, y_off, pow) = match s.id.trim_start_matches("minecraft:") {
            "snowball" => (PKind::Snowball, "minecraft:entity.snowball.throw", 6, 0.0, 1.5),
            "egg" | "blue_egg" | "brown_egg" => (PKind::Egg, "minecraft:entity.egg.throw", 7, 0.0, 1.5),
            "ender_pearl" => (PKind::EnderPearl, "minecraft:entity.ender_pearl.throw", 6, 0.0, 1.5),
            "splash_potion" => (PKind::SplashPotion, "minecraft:entity.splash_potion.throw", 7, -20.0, 0.5),
            "lingering_potion" => (PKind::LingeringPotion, "minecraft:entity.lingering_potion.throw", 6, -20.0, 0.5),
            "experience_bottle" => (PKind::XpBottle, "minecraft:entity.experience_bottle.throw", 6, -20.0, 0.7),
            _ => return false,
        };
        let pitch = 0.4 / (self.life.rng.next_float() * 0.4 + 0.8);
        self.play_sound(sound, cat, self.pos, 0.5, pitch);
        let id = self.proj_new_id();
        let pos = [self.pos[0], self.player_eye_y() - f64::from(0.1f32), self.pos[2]];
        let mut one = s.clone();
        one.count = 1;
        let mut p = Proj::new(kind, id, pos, one.clone());
        p.owner = Some(self.uuid);
        let net = self.net(Some(&one));
        p.set_meta(8, Meta::Item(net));
        self.shoot_from_player(&mut p, self.pitch, self.yaw, y_off, pow, 1.0);
        self.proj_spawn(p, &one);
        let slot = self.hand_slot(hand);
        if !self.infinite_materials()
            && let Some(st) = &mut self.slots[slot]
        {
            st.count -= 1;
            if st.count <= 0 {
                self.slots[slot] = None;
            }
        }
        true
    }

    // ---------- Rockets ----------

    /// New rocket.
    pub(crate) fn new_firework(&mut self, pos: [f64; 3], item: Stack, owner: Option<u128>, at_angle: bool) -> Proj {
        let id = self.proj_new_id();
        let mut p = Proj::new(PKind::Firework, id, pos, item.clone());
        p.owner = owner;
        p.vel = [p.rng.triangle(0.0, 0.002297), 0.05, p.rng.triangle(0.0, 0.002297)];
        let flights = 1 + flight_duration(&item);
        p.fw_lifetime = 10 * flights + p.rng.next_int_bound(6) + p.rng.next_int_bound(7);
        p.at_angle = at_angle;
        let net = self.net(Some(&item));
        p.set_meta(8, Meta::Item(net));
        p.set_meta(10, Meta::Bool(at_angle));
        p
    }

    /// Rocket used while gliding: attached to the player.
    fn firework_use(&mut self, hand: i32) -> bool {
        if !self.proj_player.fall_flying {
            return false;
        }
        let slot = self.hand_slot(hand);
        let Some(s) = self.slots[slot].clone() else { return false };
        let mut one = s.clone();
        one.count = 1;
        let mut p = self.new_firework(self.pos, one, Some(self.uuid), false);
        p.attached = Some(Ent::Player);
        p.set_meta(9, Meta::OptionalInt(Some(self.player_id)));
        self.proj_spawn(p, &s);
        if !self.infinite_materials()
            && let Some(st) = &mut self.slots[slot]
        {
            st.count -= 1;
            if st.count <= 0 {
                self.slots[slot] = None;
            }
        }
        true
    }

    // ---------- Elytra ----------

    /// `START_FALL_FLYING`: glide if the player can glide (glider worn, in the air).
    pub(crate) fn start_fall_flying(&mut self) {
        let can = !self.on_ground
            && !self.flying
            && !self.proj_player.fall_flying
            && !self.player_in_liquid()
            && self.glider_slot().is_some();
        if can {
            self.proj_player.fall_flying = true;
            self.proj_player.fall_fly_ticks = 0;
        } else {
            // Refusal: the official server switches the flag off then on again so that the client
            // predicting the glide receives the current value.
            self.life.fx.resend_flags = true;
        }
    }

    /// Player in water or lava (box reduced by 0.001, surface above the bottom of the
    /// box), like the client's liquid test.
    fn player_in_liquid(&self) -> bool {
        let h = if self.sneaking { 1.5 } else { 1.8 };
        let (min, max) = (
            [self.pos[0] - 0.3 + 0.001, self.pos[1] + 0.001, self.pos[2] - 0.3 + 0.001],
            [self.pos[0] + 0.3 - 0.001, self.pos[1] + h - 0.001, self.pos[2] + 0.3 - 0.001],
        );
        let lo = min.map(|v| v.floor() as i32);
        let hi = max.map(|v| v.ceil() as i32);
        for x in lo[0]..hi[0] {
            for y in lo[1]..hi[1] {
                for z in lo[2]..hi[2] {
                    let Some(s) = self.state_at([x, y, z]) else { continue };
                    let f = self.phys.fluids.fluid(s);
                    if f != rc_physics::Fluid::Water && f != rc_physics::Fluid::Lava {
                        continue;
                    }
                    let above = self.st([x, y + 1, z]);
                    if f64::from(y) + f64::from(self.phys.fluids.height_at(s, above)) > min[1] {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Slot carrying a usable glider (not about to break).
    fn glider_slot(&self) -> Option<usize> {
        (5..9).find(|&i| {
            self.slots[i].as_ref().is_some_and(|s| {
                (component(s, "minecraft:glider").is_some()
                    || self.life.items.default_component(s, "minecraft:glider").is_some())
                    && !self.next_damage_breaks(s)
            })
        })
    }

    /// Gliding: glider wear every 20 ticks, end on the ground.
    pub(crate) fn fall_flying_tick(&mut self) {
        if !self.proj_player.fall_flying {
            return;
        }
        let Some(slot) = self.glider_slot() else {
            self.proj_player.fall_flying = false;
            return;
        };
        if self.on_ground || self.flying {
            self.proj_player.fall_flying = false;
            return;
        }
        self.proj_player.fall_fly_ticks += 1;
        let t = self.proj_player.fall_fly_ticks + 1;
        if t % 10 == 0 {
            if (t / 10) % 2 == 0 {
                self.hurt_item(slot, 1);
                self.broadcast_menus();
            }
            let pos = self.pos;
            self.game_event_at("minecraft:elytra_glide", pos, Some(Ent::Player));
        }
    }
}
