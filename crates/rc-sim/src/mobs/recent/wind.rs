//! Wind charges (`server-mobs.md` § 4.F14), carried by the common projectile engine
//! (S8, kinds `WindCharge` and `BreezeWindCharge`): no acceleration or friction
//! (inertia 1), no fire or item, box lowered by 0.15, do not hit other
//! charges. On contact with an entity: 1 damage (`wind_charge`) then burst; with a block: burst at
//! 0.25 in front of the face hit; above the maximum height + 30: burst.
//! Burst ("trigger" interaction): radius 3 (breeze) or 1.2 (player,
//! knockback × 1.22), no damage, knockback by visible fraction, triggered blocks (levers, buttons,
//! doors, trapdoors, fence gates, candles). The breeze deflects other projectiles.

use rc_client::Event;
use rc_proto_v777::entity::Particle;
use rc_proto_v777::play::{SoundPacket, SoundRef};

use super::super::mob::Ent;
use super::super::mth::dist2;
use super::vibration::Src;
use crate::player::Stack;
use crate::projectiles::physics::{BlockHit, EntityHit, FACES, Target};
use crate::projectiles::{PKind, Proj};
use crate::server::Server;

/// Radius, knockback multiplier and sound of the burst by kind.
fn burst_params(k: PKind) -> (f32, f64, &'static str) {
    match k {
        PKind::WindCharge => (1.2, f64::from(1.22f32), "minecraft:entity.wind_charge.wind_burst"),
        _ => (3.0, 1.0, "minecraft:entity.breeze.wind_burst"),
    }
}

impl Server {
    /// Shot of a breeze wind charge: charge placed at
    /// the shot height, normalized direction + triangular inaccuracy (charge's random) ×
    /// power. The author is passed by UUID: the breeze is out of the list during its tick.
    pub(crate) fn shoot_wind_charge(&mut self, owner: u128, start: [f64; 3], dir: [f64; 3], pow: f32, unc: f32) {
        let id = self.proj_new_id();
        let item = Stack { id: PKind::BreezeWindCharge.default_item().into(), count: 1, components: None };
        let mut p = Proj::new(PKind::BreezeWindCharge, id, start, item);
        p.owner = Some(owner);
        p.accel = 0.0;
        p.shoot(dir, pow, unc);
        self.proj_add(p);
    }

    /// Charge at eye height, shot from the rotation (power 1.5,
    /// inaccuracy 1), then sound (world random) and stack consumed; the cooldown comes from the component.
    pub(crate) fn wind_charge_use(&mut self, hand: i32, s: &Stack) -> bool {
        let id = self.proj_new_id();
        let pos = [self.pos[0], self.player_eye_y(), self.pos[2]];
        let one = Stack { count: 1, ..s.clone() };
        let mut p = Proj::new(PKind::WindCharge, id, pos, one.clone());
        p.owner = Some(self.uuid);
        p.accel = 0.0;
        self.shoot_from_player(&mut p, self.pitch, self.yaw, 0.0, 1.5, 1.0);
        self.proj_spawn(p, &one);
        let pitch = 0.4 / (self.life.rng.next_float() * 0.4 + 0.8);
        self.play_sound("minecraft:entity.wind_charge.throw", 6, self.pos, 0.5, pitch);
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

    /// Above the maximum height + 30, burst and removal.
    pub(crate) fn wind_above_world(&mut self, p: &mut Proj) -> bool {
        let max_y = self.dim.height.min_y + self.dim.height.sections as i32 * 16 - 1;
        if (p.pos[1].floor() as i32) <= max_y + 30 {
            return false;
        }
        let c = p.pos;
        self.wind_explode(p, c);
        p.removed = true;
        true
    }

    /// The target becomes the author's last victim (cleared if `living` is false: part of the
    /// dragon), 1 damage, then burst.
    pub(crate) fn wind_on_hit_entity(&mut self, p: &mut Proj, t: Ent, living: bool) {
        let owner = self.proj_owner(p);
        self.set_last_hurt_mob(owner, living.then_some(t));
        self.proj_damage(t, "minecraft:wind_charge", 1.0, owner, (-p.vel[0], -p.vel[2]));
        let c = p.pos;
        self.wind_explode(p, c);
    }

    /// Burst at 0.25 in front of the face hit.
    pub(crate) fn wind_on_hit_block(&mut self, p: &mut Proj, b: BlockHit) {
        let f = FACES[b.face];
        let c =
            [b.loc[0] + f64::from(f[0]) * 0.25, b.loc[1] + f64::from(f[1]) * 0.25, b.loc[2] + f64::from(f[2]) * 0.25];
        self.wind_explode(p, c);
    }

    pub(crate) fn wind_explode(&mut self, p: &Proj, center: [f64; 3]) {
        let (radius, kb, sound) = burst_params(p.kind);
        let owner = self.proj_owner(p);
        self.prog.expl_cause_next = Some(p.kind.name()); // W8c: cause of the impact
        self.wind_burst(owner, center, radius, kb, sound);
        self.prog.expl_cause_next = None;
    }

    /// A mob of the `deflects_projectiles` tag (the breeze) deflects
    /// every projectile except wind charges (sound, U-turn, once per mob); true if
    /// the hit is replaced by the deflection.
    pub(crate) fn proj_deflected_by_entity(&mut self, p: &mut Proj, h: EntityHit) -> bool {
        let Target::Living(Ent::Mob(id)) = h.ent else { return false };
        let Some(kind) = self.mob_ref(id).map(|o| o.kind) else { return false };
        if p.kind.wind() || !self.entity_in(kind, &["#minecraft:deflects_projectiles".to_owned()]) {
            return false;
        }
        if p.last_deflected_by != Some(Ent::Mob(id)) {
            let seed = self.mobs.rng.next_long();
            if let Some(sid) = self.sound_id("minecraft:entity.breeze.deflect") {
                let category = self.mob_ref(id).map_or(5, |o| o.sound_category());
                let _ = self.send(Event::Sound(SoundPacket {
                    sound: SoundRef::Id(sid),
                    category,
                    pos: None,
                    entity: Some(id),
                    volume: 1.0,
                    pitch: 1.0,
                    seed,
                }));
            }
            self.proj_deflect_reverse(p, [1.0; 3]);
            p.last_deflected_by = Some(Ent::Mob(id));
        }
        true
    }

    /// Burst (trigger): `explode` event, rays over a
    /// 16³ cube (only `blocks_wind_charge_explosions` blocks stop them), entity knockback
    /// without damage, triggered blocks (shuffled order), explosion packet (burst, sound).
    pub(crate) fn wind_burst(&mut self, owner: Option<Ent>, center: [f64; 3], radius: f32, kb_mult: f64, sound: &str) {
        self.wind_burst_from(Src::Projectile(owner), owner, center, radius, kb_mult, sound);
    }

    /// Burst from any source (`src`: the author of the `explode` event; `owner`: the author
    /// to whom deflected projectiles go back). A source mob is not in the list
    /// (taken out during its tick): it does not receive its own knockback, like the game.
    pub(crate) fn wind_burst_from(
        &mut self,
        src: Src,
        owner: Option<Ent>,
        center: [f64; 3],
        radius: f32,
        kb_mult: f64,
        sound: &str,
    ) {
        self.game_event_src("minecraft:explode", center, Some(src), None);
        // 1. Blocks hit (set; only the number of draws matters for the random).
        let h = self.dim.height;
        let mut toblow: Vec<[i32; 3]> = Vec::new();
        let mut seen = hashbrown::HashSet::new();
        for xx in 0..16 {
            for yy in 0..16 {
                for zz in 0..16 {
                    if !(xx == 0 || xx == 15 || yy == 0 || yy == 15 || zz == 0 || zz == 15) {
                        continue;
                    }
                    let mut xd = f64::from(xx as f32 / 15.0 * 2.0 - 1.0);
                    let mut yd = f64::from(yy as f32 / 15.0 * 2.0 - 1.0);
                    let mut zd = f64::from(zz as f32 / 15.0 * 2.0 - 1.0);
                    let d = (xd * xd + yd * yd + zd * zd).sqrt();
                    xd /= d;
                    yd /= d;
                    zd /= d;
                    let mut power = radius * (0.7 + self.mobs.rng.next_float() * 0.6);
                    let (mut xp, mut yp, mut zp) = (center[0], center[1], center[2]);
                    while power > 0.0 {
                        let p = [xp.floor() as i32, yp.floor() as i32, zp.floor() as i32];
                        if p[1] < h.min_y || p[1] >= h.min_y + h.sections as i32 * 16 {
                            break;
                        }
                        if self.btag_id(p, crate::tag_bits::TagId::BlocksWindChargeExplosions) {
                            power -= (3_600_000.0 + 0.3) * 0.3;
                        }
                        if power > 0.0 && seen.insert(p) {
                            toblow.push(p);
                        }
                        xp += xd * f64::from(0.3f32);
                        yp += yd * f64::from(0.3f32);
                        zp += zd * f64::from(0.3f32);
                        power -= 0.225_000_01;
                    }
                }
            }
        }
        // 2. Entities: knockback only (kind multiplier; the player flying in creative
        //    receives none).
        let double = f64::from(radius * 2.0);
        let mut player_kb: Option<[f64; 3]> = None;
        let mut targets: Vec<Ent> = self.mobs.list.iter().flatten().map(|o| Ent::Mob(o.id)).collect();
        targets.push(Ent::Player);
        for t in targets {
            let (Some(pos), Some(eye), Some(bb)) = (self.ent_pos(t), self.ent_eye(t), self.ent_bbox(t)) else {
                continue;
            };
            if t == Ent::Player && (self.game_mode == 3 || !self.ent_alive(Ent::Player)) {
                continue;
            }
            if let Ent::Mob(id) = t
                && self.mob_ref(id).is_some_and(super::ignores_explosion)
            {
                continue;
            }
            let dist = dist2(pos, center).sqrt() / double;
            if dist > 1.0 {
                continue;
            }
            let mut dir = [pos[0] - center[0], eye - center[1], pos[2] - center[2]];
            let l = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
            dir = if l < 1.0e-4 { [0.0; 3] } else { dir.map(|v| v / l) };
            let mult = kb_mult;
            let exposure = f64::from(self.seen_percent(center, &bb));
            let res = match t {
                Ent::Mob(id) => self.mob_ref(id).map_or(0.0, |o| o.attrs.explosion_kb_resistance.value()),
                Ent::Player => self.player_attribute("minecraft:explosion_knockback_resistance"),
            };
            let kp = (1.0 - dist) * exposure * mult * (1.0 - res);
            let kb = dir.map(|v| v * kp);
            match t {
                Ent::Player => {
                    // W8c: impact point and cause (`fall_after_explosion`).
                    self.prog.expl_impact = Some((self.pos, self.prog.expl_cause_next.map(str::to_owned)));
                    if !(self.game_mode == 1 && self.flying) {
                        player_kb = Some(kb);
                    }
                }
                Ent::Mob(id) => {
                    if let Some(o) = self.mobs.get_mut(id) {
                        o.vel = [o.vel[0] + kb[0], o.vel[1] + kb[1], o.vel[2] + kb[2]];
                        o.needs_sync = true;
                    }
                }
            }
        }
        // Fireballs and other charges: pushed, the launcher becomes their owner.
        let uuid = self.blast_owner_uuid(owner, None);
        self.explosion_redirect(center, radius, uuid, kb_mult);
        // 3. Triggered blocks (shuffled order on the world random).
        self.mobs.rng.shuffle(&mut toblow);
        for p in toblow.iter().copied() {
            self.wind_trigger_block(p);
        }
        // 4. Packet to the player.
        if dist2(self.pos, center) < 4096.0 {
            let pt = |s: &Server, n: &str| Particle {
                kind: s
                    .data
                    .registry("minecraft:particle_type")
                    .and_then(|r| r.entries.iter().position(|e| e == n))
                    .unwrap_or(0) as i32,
                color: 0,
                state: 0,
                item: -1,
                scale: 1.0,
                color2: 0,
                vib: None,
            };
            // Radius of at least 2 with interaction (trigger): big burst, otherwise small.
            let emitter = if radius >= 2.0 { "minecraft:gust_emitter_large" } else { "minecraft:gust_emitter_small" };
            let ex = rc_client::extra::Explosion {
                center,
                radius,
                blocks: toblow.len() as i32,
                knockback: player_kb,
                particle: pt(self, emitter),
                sound: SoundRef::Id(
                    self.data
                        .registry("minecraft:sound_event")
                        .and_then(|r| r.entries.iter().position(|e| e == sound))
                        .unwrap_or(0) as i32,
                ),
                block_particles: Vec::new(),
                play_sound: true,
            };
            let _ = self.send(Event::Explode(Box::new(ex)));
        }
        if let Some(kb) = player_kb {
            let v = self.mobs.player_vel;
            self.mobs.player_vel = [v[0] + kb[0], v[1] + kb[1], v[2] + kb[2]];
        }
    }

    /// The reaction to an explosion in "trigger" mode: levers, buttons, doors and trapdoors openable by
    /// the wind charge, fence gates, lit candles.
    fn wind_trigger_block(&mut self, p: [i32; 3]) {
        let s = self.st(p);
        let name = self.state_name(s).trim_start_matches("minecraft:").to_owned();
        let prop = |k: &str| self.data.state_properties(s).find(|(n, _)| *n == k).map(|(_, v)| v.to_owned());
        let powered = prop("powered").as_deref() == Some("true");
        let waterlogged = prop("waterlogged").as_deref() == Some("true");
        if name == "lever" {
            // Sound and event in the common toggle.
            self.lever_pull(p, s);
        } else if name.ends_with("_button") {
            if !powered {
                self.button_press(p, s, None);
            }
        } else if name.ends_with("_door") && name != "iron_door" {
            if prop("half").as_deref() == Some("lower") && !powered {
                let open = prop("open").as_deref() == Some("true");
                if let Some(n) = self.with_prop(s, "open", if open { "false" } else { "true" }) {
                    self.set_block(p, n, 10);
                    let top = [p[0], p[1] + 1, p[2]];
                    let ts = self.st(top);
                    if let Some(tn) = self.with_prop(ts, "open", if open { "false" } else { "true" }) {
                        self.set_block(top, tn, 10);
                    }
                }
                self.openable_sound(p, s, !open, None);
            }
        } else if name.ends_with("_trapdoor") && name != "iron_trapdoor" {
            if !powered {
                let open = prop("open").as_deref() == Some("true");
                if let Some(n) = self.with_prop(s, "open", if open { "false" } else { "true" }) {
                    self.set_block(p, n, 2);
                }
                if waterlogged {
                    self.schedule_fluid_tick(p, 1, 5);
                }
                self.openable_sound(p, s, !open, None);
            }
        } else if name.ends_with("_fence_gate") {
            if !powered {
                let open = prop("open").as_deref() == Some("true");
                if let Some(n) = self.with_prop(s, "open", if open { "false" } else { "true" }) {
                    self.set_update(p, n);
                }
                self.openable_sound(p, s, !open, None);
            }
        } else if name == "bell" {
            // The bell ringing attempt without direction: the block's.
            self.bell_attempt_ring(p, None, None);
        } else if name == "beehive" || name == "bee_nest" {
            crate::mobs::wild::hive::anger_nearby(self, p);
        } else if (name.ends_with("candle") || name.ends_with("candle_cake")) && prop("lit").as_deref() == Some("true")
        {
            if let Some(n) = self.with_prop(s, "lit", "false") {
                self.set_update(p, n);
            }
            self.block_sound(p, "minecraft:block.candle.extinguish", 1.0, 1.0);
            self.game_event_block("minecraft:block_change", p, None, None);
        }
    }
}
