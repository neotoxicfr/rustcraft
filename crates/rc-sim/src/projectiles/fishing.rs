//! Fishing: bobber cast, flight, floating, waiting
//! (lure), fish approach (particles), bite, draw (`gameplay/fishing` loot with
//! luck of the sea, experience), hooking and pulling of entities and dropped items.

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::{AddEntity, Meta};

use super::physics::{BlockHit, Hit, Target};
use super::{DEG, PKind, Proj, lerp_rotation};
use crate::mobs::jrand::JRand;
use crate::mobs::mob::Ent;
use crate::mobs::mth::atan2;
use crate::server::Server;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
    Flying,
    Hooked,
    Bobbing,
}

/// Player's bobber. The movement reuses a projectile (box 0.25, random, tracking).
#[derive(Debug, Clone)]
pub struct Hook {
    pub p: Proj,
    pub state: HookState,
    pub hooked: Option<Target>,
    pub biting: bool,
    pub out_of_water: i32,
    pub life: i32,
    pub nibble: i32,
    pub until_lured: i32,
    pub until_hooked: i32,
    pub fish_angle: f32,
    pub open_water: bool,
    pub luck: i32,
    pub lure: i32,
    /// Synchronised random (seed: low bits of the UUID ^ game time).
    sync: JRand,
}

impl Server {
    /// Recasts or reels in the bobber.
    pub(crate) fn fishing_rod_use(&mut self, hand: i32) -> bool {
        let slot = if hand == 0 { 36 + self.held as usize } else { 45 };
        let Some(rod) = self.slots[slot].clone() else { return false };
        let pitch = 0.4 / (self.life.rng.next_float() * 0.4 + 0.8);
        if self.proj.hook.is_some() {
            let dmg = self.hook_retrieve(&rod);
            if dmg > 0 {
                self.hurt_item(slot, dmg);
            }
            self.play_sound("minecraft:entity.fishing_bobber.retrieve", 6, self.pos, 1.0, pitch);
            // Stack read after the wear: a broken rod no longer vibrates.
            let left = self.slots[slot].clone();
            self.use_vibration(left.as_ref(), "minecraft:item_interact_finish");
        } else {
            self.play_sound("minecraft:entity.fishing_bobber.throw", 6, self.pos, 0.5, pitch);
            let ctx = super::ench::Ctx { tool: Some(&rod), ..Default::default() };
            let lure = (self.ench_value(&rod, "minecraft:fishing_time_reduction", 0.0, &ctx) * 20.0) as i32;
            let luck = self.ench_value(&rod, "minecraft:fishing_luck_bonus", 0.0, &ctx) as i32;
            self.hook_cast(luck, lure);
            self.use_vibration(Some(&rod), "minecraft:item_interact_start");
        }
        true
    }

    /// Creation of the fishing bobber (player, world, luck, lure).
    fn hook_cast(&mut self, luck: i32, lure: i32) {
        let id = self.proj_new_id();
        let (pitch, yaw) = (self.pitch, self.yaw);
        let yc = rc_physics::cos_t(-yaw * (std::f64::consts::PI / 180.0) as f32 - std::f32::consts::PI);
        let ys = rc_physics::sin_t(-yaw * (std::f64::consts::PI / 180.0) as f32 - std::f32::consts::PI);
        let xc = -rc_physics::cos_t(-pitch * (std::f64::consts::PI / 180.0) as f32);
        let xs = rc_physics::sin_t(-pitch * (std::f64::consts::PI / 180.0) as f32);
        let pos = [self.pos[0] - f64::from(ys) * 0.3, self.player_eye_y(), self.pos[2] - f64::from(yc) * 0.3];
        let item = crate::player::Stack { id: "minecraft:fishing_rod".into(), count: 1, components: None };
        let mut p = Proj::new(PKind::Snowball, id, pos, item);
        p.owner = Some(self.uuid);
        p.yaw = yaw;
        p.pitch = pitch;
        let m = [f64::from(-ys), f64::from((-(xs / xc)).clamp(-5.0, 5.0)), f64::from(-yc)];
        let l = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt();
        let k: [f64; 3] = std::array::from_fn(|_| 0.6 / l + p.rng.triangle(0.5, 0.010_336_5));
        p.vel = [m[0] * k[0], m[1] * k[1], m[2] * k[2]];
        p.yaw = (atan2(p.vel[0], p.vel[2]) * DEG) as f32;
        p.pitch = (atan2(p.vel[1], (p.vel[0] * p.vel[0] + p.vel[2] * p.vel[2]).sqrt()) * DEG) as f32;
        let mut h = Hook {
            p,
            state: HookState::Flying,
            hooked: None,
            biting: false,
            out_of_water: 0,
            life: 0,
            nibble: 0,
            until_lured: 0,
            until_hooked: 0,
            fish_angle: 0.0,
            open_water: true,
            luck: luck.max(0),
            lure: lure.max(0),
            sync: JRand::new(0),
        };
        self.hook_pair(&mut h);
        self.proj.hook = Some(h);
    }

    fn hook_pair(&self, h: &mut Hook) {
        let p = &mut h.p;
        let add = AddEntity {
            id: p.id,
            uuid: p.uuid,
            kind: self.entity_kind_id("minecraft:fishing_bobber"),
            pos: p.pos,
            velocity: crate::mobs::track::lp_quantize(p.vel),
            pitch: p.pitch,
            yaw: p.yaw,
            head_yaw: 0.0,
            data: self.player_id,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        p.track.seen = true;
    }

    fn hook_meta(&self, h: &Hook) {
        let hooked = match h.hooked {
            Some(Target::Living(e)) => self.ent_net_id(e) + 1,
            Some(Target::Item(id) | Target::Crystal(id) | Target::Proj(id) | Target::Cart(id)) => id + 1,
            // F15: dragon part (identifiers reserved after its own).
            Some(Target::Part(d, i)) => d + 1 + i32::from(i) + 1,
            None => 0,
        };
        let values = vec![(8, Meta::Int(hooked)), (9, Meta::Bool(h.biting))];
        let _ = self.send(Event::Entity(EntityEvent::Data { id: h.p.id, values }));
    }

    fn hook_remove(&mut self) {
        if let Some(h) = self.proj.hook.take() {
            let _ = self.send(Event::Entity(EntityEvent::Remove(vec![h.p.id])));
        }
    }

    /// Bobber tick.
    pub(crate) fn fishing_tick(&mut self) {
        let Some(mut h) = self.proj.hook.take() else { return };
        // Network tracking before the tick (official server order).
        self.hook_track(&mut h);
        h.p.tick_count += 1;
        h.p.old_pos = h.p.pos;
        let seed = (h.p.uuid as u64 as i64) ^ self.time;
        h.sync = JRand::new(seed);
        // A projectile's tick → base tick.
        self.proj_mark_shot(&mut h.p);
        self.check_left_owner(&mut h.p);
        self.proj_base_tick(&mut h.p, true);
        if h.p.removed || !self.hook_keep(&h) {
            self.proj.hook = Some(h);
            self.hook_remove();
            return;
        }
        if h.p.on_ground {
            h.life += 1;
            if h.life >= 1200 {
                self.proj.hook = Some(h);
                self.hook_remove();
                return;
            }
        } else {
            h.life = 0;
        }
        let bp = h.p.block_pos();
        let liquid = self.water_height(bp);
        let in_water = liquid > 0.0;
        let before = (h.hooked, h.biting);
        let mut skip_move = false;
        match h.state {
            HookState::Flying => {
                if h.hooked.is_some() {
                    h.p.vel = [0.0; 3];
                    h.state = HookState::Hooked;
                    skip_move = true;
                } else if in_water {
                    h.p.vel = [h.p.vel[0] * 0.3, h.p.vel[1] * 0.2, h.p.vel[2] * 0.3];
                    h.state = HookState::Bobbing;
                    skip_move = true;
                } else {
                    let hit = self.hit_on_move(&h.p, true);
                    match hit {
                        Hit::Entity(e) => {
                            h.hooked = Some(e.ent);
                            self.proj_land(&h.p, hit);
                        }
                        // World border: the bobber bounces.
                        Hit::Block(b) if b.border => self.proj_deflect_reverse(&mut h.p, [0.2; 3]),
                        Hit::Block(b) if !self.is_air(b.pos) => {
                            self.hook_hit_block(&mut h, b);
                            self.proj_land(&h.p, hit);
                        }
                        _ => {}
                    }
                }
            }
            HookState::Hooked => {
                if let Some(t) = h.hooked {
                    match self.hooked_pos(t) {
                        Some(pos) => h.p.pos = pos,
                        None => {
                            h.hooked = None;
                            h.state = HookState::Flying;
                        }
                    }
                }
                skip_move = true;
            }
            HookState::Bobbing => {
                let v = h.p.vel;
                let mut force = h.p.pos[1] + v[1] - f64::from(bp[1]) - f64::from(liquid);
                if force.abs() < 0.01 {
                    // The game's sign: 0 for zero force (Rust's `signum` would return 1).
                    force += if force > 0.0 {
                        0.1
                    } else if force < 0.0 {
                        -0.1
                    } else {
                        0.0
                    };
                }
                h.p.vel = [v[0] * 0.9, v[1] - force * f64::from(h.p.rng.next_float()) * 0.2, v[2] * 0.9];
                if h.nibble <= 0 && h.until_hooked <= 0 {
                    h.open_water = true;
                } else {
                    h.open_water = h.open_water && h.out_of_water < 10 && self.open_water_at(bp);
                }
                if in_water {
                    h.out_of_water = (h.out_of_water - 1).max(0);
                    if h.biting {
                        h.p.vel[1] += -0.1 * f64::from(h.sync.next_float()) * f64::from(h.sync.next_float());
                    }
                    self.catching_fish(&mut h, bp);
                } else {
                    h.out_of_water = (h.out_of_water + 1).min(10);
                }
            }
        }
        if !skip_move {
            if self.water_height(bp) <= 0.0 && !h.p.on_ground && h.hooked.is_none() {
                h.p.vel[1] -= f64::from(0.03f32);
            }
            let d = h.p.vel;
            self.proj_move_collide(&mut h.p, d);
            let v = h.p.vel;
            let xr = (atan2(v[1], (v[0] * v[0] + v[2] * v[2]).sqrt()) * DEG) as f32;
            let yr = (atan2(v[0], v[2]) * DEG) as f32;
            h.p.pitch = lerp_rotation(h.p.pitch, xr);
            h.p.yaw = lerp_rotation(h.p.yaw, yr);
            if h.state == HookState::Flying && (h.p.on_ground || h.p.horizontal_collision) {
                h.p.vel = [0.0; 3];
            }
            h.p.vel = h.p.vel.map(|c| c * 0.92);
        }
        if before != (h.hooked, h.biting) {
            self.hook_meta(&h);
        }
        self.proj.hook = Some(h);
    }

    /// Network tracking of the bobber (interval 5).
    fn hook_track(&self, h: &mut Hook) {
        let p = &mut h.p;
        if p.track.tick % 5 == 0 || p.needs_sync {
            let s = rc_proto_v777::entity::PositionSync {
                id: p.id,
                path: rc_proto_v777::entity::PositionPath { linear: true, steps: vec![(p.pos, 0)] },
                yaw: p.yaw,
                pitch: p.pitch,
                on_ground: p.on_ground,
            };
            let _ = self.send(Event::Entity(EntityEvent::Sync(s)));
            let velocity = crate::mobs::track::lp_quantize(p.vel);
            let _ = self.send(Event::Entity(EntityEvent::Motion { id: p.id, velocity }));
            p.needs_sync = false;
        }
        p.track.tick += 1;
    }

    /// Rod in hand and within 32 blocks.
    fn hook_keep(&self, h: &Hook) -> bool {
        if self.life.vit.dead || self.game_mode == 3 {
            return false;
        }
        let rod = |s: usize| self.slots[s].as_ref().is_some_and(|x| x.id == "minecraft:fishing_rod");
        let d2 = (0..3).map(|i| (h.p.pos[i] - self.pos[i]).powi(2)).sum::<f64>();
        (rod(36 + self.held as usize) || rod(45)) && d2 <= 1024.0
    }

    /// Water height of a block, 0 outside water.
    pub(crate) fn water_height(&self, p: [i32; 3]) -> f32 {
        let s = self.st(p);
        if self.phys.fluids.fluid(s) != rc_physics::Fluid::Water {
            return 0.0;
        }
        self.phys.fluids.height_at(s, self.st([p[0], p[1] + 1, p[2]]))
    }

    fn hooked_pos(&self, t: Target) -> Option<[f64; 3]> {
        match t {
            Target::Living(e) => {
                if !self.ent_alive(e) || (e == Ent::Player && self.game_mode == 3) {
                    return None;
                }
                let b = self.ent_bbox(e)?;
                let p = self.ent_pos(e)?;
                Some([p[0], p[1] + (b.max[1] - b.min[1]) * 0.8, p[2]])
            }
            Target::Item(id) => {
                let d = self.life.drops.iter().find(|d| d.id == id && !d.is_removed())?;
                Some([d.pos[0], d.pos[1] + 0.25 * 0.8, d.pos[2]])
            }
            // F15: dragon part (alive as long as the dragon is).
            Target::Part(id, i) => {
                let m = self.mob_ref(id).filter(|m| m.alive())?;
                let b = crate::mobs::end::dragon::part_box(m.end.dragon.as_ref()?, usize::from(i));
                Some([(b.min[0] + b.max[0]) / 2.0, b.min[1] + (b.max[1] - b.min[1]) * 0.8, (b.min[2] + b.max[2]) / 2.0])
            }
            // Minecart: hooked at 0.8 × its height as long as it exists.
            Target::Cart(id) => {
                let c = self.cart_ref(id)?;
                Some([c.pos[0], c.pos[1] + crate::minecart::HEIGHT * 0.8, c.pos[2]])
            }
            // The hook does not yet target crystals or projectiles (`proj_targets`).
            Target::Crystal(_) | Target::Proj(_) => None,
        }
    }

    fn hook_hit_block(&mut self, h: &mut Hook, b: BlockHit) {
        let mut tmp = h.p.clone();
        self.block_projectile_hit(&mut tmp, b);
        let v = h.p.vel;
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        // The game takes the SQUARE of the distance to the point touched (flaw reproduced).
        let d = (b.loc[0] - h.p.pos[0]).powi(2) + (b.loc[1] - h.p.pos[1]).powi(2) + (b.loc[2] - h.p.pos[2]).powi(2);
        h.p.vel = if l < f64::from(1.0e-5f32) { [0.0; 3] } else { v.map(|c| c / l * d) };
    }

    /// 5 × 5 on four layers, water then air with no mixing.
    fn open_water_at(&self, p: [i32; 3]) -> bool {
        #[derive(PartialEq, Clone, Copy)]
        enum T {
            Above,
            Inside,
            Invalid,
        }
        let mut prev = T::Invalid;
        for y in -1..=2 {
            let mut layer: Option<T> = None;
            for x in -2..=2 {
                for z in -2..=2 {
                    let q = [p[0] + x, p[1] + y, p[2] + z];
                    let s = self.st(q);
                    let name = self.state_name(s);
                    let t = if self.is_air(q) || name == "minecraft:lily_pad" {
                        T::Above
                    } else {
                        let src = self.phys.fluids.fluid(s) == rc_physics::Fluid::Water && self.water_source(s) && {
                            let mut bx = Vec::new();
                            self.phys.collision(s, &mut bx);
                            bx.is_empty()
                        };
                        if src { T::Inside } else { T::Invalid }
                    };
                    layer = Some(match layer {
                        None => t,
                        Some(l) if l == t => l,
                        _ => T::Invalid,
                    });
                }
            }
            match layer.unwrap_or(T::Invalid) {
                T::Above if prev == T::Invalid => return false,
                T::Inside if prev == T::Above => return false,
                T::Invalid => return false,
                l => prev = l,
            }
        }
        true
    }

    /// Water source (full water, level-0 water block or waterlogged block).
    fn water_source(&self, s: u32) -> bool {
        let name = self.state_name(s);
        if name == "minecraft:water" {
            return self.data.state_properties(s).any(|(k, v)| k == "level" && v == "0");
        }
        self.data.state_properties(s).any(|(k, v)| k == "waterlogged" && v == "true")
            || matches!(
                name,
                "minecraft:kelp"
                    | "minecraft:kelp_plant"
                    | "minecraft:seagrass"
                    | "minecraft:tall_seagrass"
                    | "minecraft:bubble_column"
            )
    }

    /// Speed (rain, sky), bite, approach, waiting.
    fn catching_fish(&mut self, h: &mut Hook, bp: [i32; 3]) {
        let mut speed = 1;
        let above = [bp[0], bp[1] + 1, bp[2]];
        if h.p.rng.next_float() < 0.25 && self.is_raining_at(above) {
            speed += 1;
        }
        if h.p.rng.next_float() < 0.5 && !self.can_see_sky(above) {
            speed -= 1;
        }
        if h.nibble > 0 {
            h.nibble -= 1;
            if h.nibble <= 0 {
                h.until_lured = 0;
                h.until_hooked = 0;
                h.biting = false;
            }
        } else if h.until_hooked > 0 {
            h.until_hooked -= speed;
            if h.until_hooked > 0 {
                h.fish_angle += h.p.rng.triangle(0.0, 9.188) as f32;
                let a = h.fish_angle * (std::f64::consts::PI / 180.0) as f32;
                let (s, c) = (rc_physics::sin_t(a), rc_physics::cos_t(a));
                let fx = h.p.pos[0] + f64::from(s * h.until_hooked as f32 * 0.1);
                let fy = h.p.pos[1].floor() + 1.0;
                let fz = h.p.pos[2] + f64::from(c * h.until_hooked as f32 * 0.1);
                let q = [fx.floor() as i32, (fy - 1.0).floor() as i32, fz.floor() as i32];
                if self.state_name(self.st(q)) == "minecraft:water" {
                    if h.p.rng.next_float() < 0.15 {
                        self.particles_full(
                            "minecraft:bubble",
                            0,
                            [fx, fy - f64::from(0.1f32), fz],
                            [s, 0.1, c],
                            [0.0; 3],
                            1,
                            0,
                        );
                    }
                    let (px, pz) = (s * 0.04, c * 0.04);
                    self.particles_full("minecraft:fishing", 0, [fx, fy, fz], [pz, 0.01, -px], [1.0; 3], 0, 0);
                    self.particles_full("minecraft:fishing", 0, [fx, fy, fz], [-pz, 0.01, px], [1.0; 3], 0, 0);
                }
            } else {
                let pitch = 1.0 + (h.p.rng.next_float() - h.p.rng.next_float()) * 0.4;
                self.world_sound("minecraft:entity.fishing_bobber.splash", 6, h.p.pos, 0.25, pitch);
                let y = h.p.pos[1] + 0.5;
                let n = (1.0 + 0.25 * 20.0) as i32;
                self.particles("minecraft:bubble", [h.p.pos[0], y, h.p.pos[2]], [0.25, 0.0, 0.25], 0.2, n);
                self.particles("minecraft:fishing", [h.p.pos[0], y, h.p.pos[2]], [0.25, 0.0, 0.25], 0.2, n);
                h.nibble = h.p.rng.int_between(20, 40);
                h.biting = true;
            }
        } else if h.until_lured > 0 {
            h.until_lured -= speed;
            let mut tease = 0.15f32;
            if h.until_lured < 20 {
                tease += (20 - h.until_lured) as f32 * 0.05;
            } else if h.until_lured < 40 {
                tease += (40 - h.until_lured) as f32 * 0.02;
            } else if h.until_lured < 60 {
                tease += (60 - h.until_lured) as f32 * 0.01;
            }
            if h.p.rng.next_float() < tease {
                let a = next_float_between(&mut h.p.rng, 0.0, 360.0) * (std::f64::consts::PI / 180.0) as f32;
                let d = next_float_between(&mut h.p.rng, 25.0, 60.0);
                let fx = h.p.pos[0] + f64::from(rc_physics::sin_t(a) * d) * 0.1;
                let fy = h.p.pos[1].floor() + 1.0;
                let fz = h.p.pos[2] + f64::from(rc_physics::cos_t(a) * d) * 0.1;
                let q = [fx.floor() as i32, (fy - 1.0).floor() as i32, fz.floor() as i32];
                if self.state_name(self.st(q)) == "minecraft:water" {
                    let n = 2 + h.p.rng.next_int_bound(2);
                    self.particles("minecraft:splash", [fx, fy, fz], [0.1, 0.0, 0.1], 0.0, n);
                }
            }
            if h.until_lured <= 0 {
                h.fish_angle = next_float_between(&mut h.p.rng, 0.0, 360.0);
                h.until_hooked = h.p.rng.int_between(20, 80);
            }
        } else {
            h.until_lured = h.p.rng.int_between(100, 600);
            h.until_lured -= h.lure;
        }
    }

    /// `retrieve`: pulling of the hooked entity, or loot if the fish bites; wear returned.
    pub(crate) fn hook_retrieve(&mut self, rod: &crate::player::Stack) -> i32 {
        let Some(mut h) = self.proj.hook.take() else { return 0 };
        if !self.hook_keep(&h) {
            self.proj.hook = Some(h);
            self.hook_remove();
            return 0;
        }
        let mut dmg = 0;
        if let Some(t) = h.hooked {
            let d =
                [(self.pos[0] - h.p.pos[0]) * 0.1, (self.pos[1] - h.p.pos[1]) * 0.1, (self.pos[2] - h.p.pos[2]) * 0.1];
            match t {
                Target::Living(e) => self.push_ent(e, d),
                // F15: pushing a part does not move the dragon.
                Target::Part(..) | Target::Crystal(_) | Target::Proj(_) => {}
                Target::Cart(id) => {
                    if let Some(c) = self.cart_mut(id) {
                        c.vel = [c.vel[0] + d[0], c.vel[1] + d[1], c.vel[2] + d[2]];
                    }
                }
                Target::Item(id) => {
                    if let Some(it) = self.life.drops.iter_mut().find(|x| x.id == id) {
                        it.vel = [it.vel[0] + d[0], it.vel[1] + d[1], it.vel[2] + d[2]];
                    }
                }
            }
            let _ = self.send(Event::Entity(EntityEvent::Event { id: h.p.id, event: 31 }));
            let hooked = match t {
                Target::Living(e) => Some(e),
                Target::Item(_) | Target::Part(..) | Target::Crystal(_) | Target::Proj(_) | Target::Cart(_) => None,
            };
            self.s8_rod_hooked(rod, hooked, &[]); // S9
            dmg = if matches!(t, Target::Item(_)) { 3 } else { 5 };
        } else if h.nibble > 0 {
            let items = self.fishing_loot(&h, rod);
            self.s8_rod_hooked(rod, None, &items); // S9: fish caught, trigger
            for s in items {
                let (dx, dy, dz) = (self.pos[0] - h.p.pos[0], self.pos[1] - h.p.pos[1], self.pos[2] - h.p.pos[2]);
                let v = [dx * 0.1, dy * 0.1 + (dx * dx + dy * dy + dz * dz).sqrt().sqrt() * 0.08, dz * 0.1];
                self.spawn_item(h.p.pos, v, s, 10);
                let xp = h.p.rng.next_int_bound(6) + 1;
                self.add_orb([self.pos[0], self.pos[1] + 0.5, self.pos[2] + 0.5], xp);
            }
            dmg = 1;
        }
        if h.p.on_ground {
            dmg = 2;
        }
        self.proj.hook = Some(h);
        self.hook_remove();
        dmg
    }

    /// Fishing loot (table `gameplay/fishing`, luck = luck of the sea + player luck).
    pub(crate) fn fishing_loot(&mut self, h: &Hook, rod: &crate::player::Stack) -> Vec<crate::player::Stack> {
        let luck = h.luck as f32 + self.luck();
        let bp = h.p.block_pos();
        let biome = self.biome_at(bp).and_then(|i| self.regs.entries("minecraft:worldgen/biome").get(i).cloned());
        let table = "minecraft:gameplay/fishing";
        let tags = &self.block_tags;
        let data = &self.data;
        let tag_fn = |tag: &str, name: &str| {
            let bid = data.blocks.iter().position(|b| b.name == name).map(|i| i as u32);
            bid.is_some_and(|b| tags.get(tag).is_some_and(|v| v.binary_search(&b).is_ok()))
        };
        let life = &mut *self.life;
        let loot = crate::loot::Loot { pack: &life.pack, items: &life.items, block_tag: &tag_fn };
        let ctx = crate::loot::Ctx {
            tool: Some(Some(rod)),
            luck,
            open_water: Some(h.open_water),
            biome,
            ..Default::default()
        };
        // Sequence declared by the table (`random_sequence`), otherwise the level's random.
        match life.pack.get("loot_table", table).and_then(|t| t["random_sequence"].as_str().map(str::to_owned)) {
            Some(q) => loot.drops(table, &ctx, life.seqs.get(&q)),
            None => loot.drops(table, &ctx, &mut life.rng),
        }
    }
}

/// A float draw between two bounds.
fn next_float_between(r: &mut JRand, min: f32, max: f32) -> f32 {
    if min >= max { min } else { r.next_float() * (max - min) + min }
}
