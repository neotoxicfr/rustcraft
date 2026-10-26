//! Impacts (impact against an entity, impact against a block): damage and knockback of arrows (crit,
//! piercing, flame, punch, effects), trident (loyalty, channeling), snowball, egg
//! (chicks), pearl (teleport, endermite), potions (splash, cloud), experience
//! bottle, rocket (explosion), fireball; reactions of the blocks hit.

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::Meta;

use super::physics::{BlockHit, EntityHit, FACES, Hit, Target};
use super::{PKind, Pickup, Proj};
use crate::blocks::{Dir, flag};
use crate::effects::Inst;
use crate::mobs::mob::Ent;
use crate::mobs::species::Kind;
use crate::player::Stack;
use crate::server::Server;

/// Color of a potion: own color, otherwise mix of the visible effects
/// weighted by their level, otherwise the base color.
pub fn potion_color(s: &Stack) -> i32 {
    potion_color_or(s, -13083194)
}

pub fn potion_color_or(s: &Stack, default: i32) -> i32 {
    if let Some(c) = crate::items::component(s, "minecraft:potion_contents")
        && let Some(v) = c.get("custom_color").and_then(rc_proto::nbt::Nbt::as_i64)
    {
        return v as i32;
    }
    mix_colors(&crate::potions::stack_effects(s, 1.0)).unwrap_or(default)
}

/// The optional color of a potion.
pub fn mix_colors(effects: &[Inst]) -> Option<i32> {
    let (mut r, mut g, mut b, mut w) = (0i32, 0i32, 0i32, 0i32);
    for e in effects.iter().filter(|e| e.visible) {
        let Some(d) = crate::effects::def(&e.id) else { continue };
        let c = d.color as i32;
        let a = e.amp + 1;
        r += a * ((c >> 16) & 255);
        g += a * ((c >> 8) & 255);
        b += a * (c & 255);
        w += a;
    }
    (w != 0).then(|| (255 << 24) | ((r / w) << 16) | ((g / w) << 8) | (b / w))
}

/// "Water" potion (tags `douses_fire`, `extinguishes_entities`, `hurts_water_sensitive_entities`).
fn is_water(s: &Stack) -> bool {
    crate::potions::potion_of(s).as_deref() == Some("minecraft:water")
}

/// Push of an arrow's punch: horizontal velocity normalized like the game
/// (zero vector below 1e-5 in single precision), scaled by `scale`, 0.1 upward;
/// `None` if nothing is pushed.
pub(crate) fn arrow_knockback_push(vel: [f64; 3], scale: f64) -> Option<[f64; 3]> {
    let (x, z) = (vel[0], vel[2]);
    let l = (x * x + z * z).sqrt();
    let n = if l < f64::from(1.0e-5f32) { [0.0, 0.0] } else { [x / l, z / l] };
    let m = [n[0] * scale, n[1] * scale];
    (m[0] * m[0] + m[1] * m[1] > 0.0).then_some([m[0], 0.1, m[1]])
}

impl Server {
    /// Damage of a projectile on a living entity; `kb` = knockback direction (the game's `xd`, `zd`).
    pub(crate) fn proj_damage(&mut self, t: Ent, ty: &str, amount: f32, by: Option<Ent>, kb: (f64, f64)) -> bool {
        // W8c: direct entity = the projectile (shield: side it comes from, no retaliation).
        if t == Ent::Player && self.prog.dmg_src.pos.is_none() {
            let pos = [self.pos[0] + kb.0, self.pos[1], self.pos[2] + kb.1];
            self.prog.dmg_src = crate::item_use::DmgSrc { pos: Some(pos), ..Default::default() };
        }
        match t {
            Ent::Player => match by {
                Some(Ent::Mob(id)) if self.mob_ref(id).is_some() => {
                    let m = self.mob_ref(id).map(|m| (*m).clone()).expect("mob");
                    self.player_hurt_by_mob(&m, ty, amount, Some(kb))
                }
                _ => self.player_hurt_kb(ty, amount, Some(kb)),
            },
            Ent::Mob(id) => {
                let Some(i) = self.mob_index(id) else { return false };
                let Some(mut m) = self.mobs.list[i].take() else { return false };
                let src = [m.pos[0] + kb.0, m.pos[1], m.pos[2] + kb.1];
                let before = m.health;
                let r = self.mob_hurt_dir(&mut m, ty, amount, by, Some(src), Some(kb));
                let dealt = before - m.health;
                self.mobs.list[i] = Some(m);
                if r && by == Some(Ent::Player) {
                    self.s8_player_hurt_by_projectile(t, ty, amount, dealt); // S9
                }
                r
            }
        }
    }

    /// Fire on a living entity (at least `ticks`); returns the old value.
    pub(crate) fn ignite_ent(&mut self, t: Ent, ticks: i32) -> i32 {
        match t {
            Ent::Player => {
                let old = self.life.vit.fire;
                if !self.fx().has("minecraft:fire_resistance") {
                    self.player_ignite(ticks);
                }
                old
            }
            Ent::Mob(id) => {
                let Some(i) = self.mob_index(id) else { return 0 };
                let Some(m) = self.mobs.list[i].as_mut() else { return 0 };
                let old = m.fire;
                if !m.kind.info().fire_immune && old < ticks {
                    m.fire = ticks;
                }
                old
            }
        }
    }

    fn set_fire_ent(&mut self, t: Ent, ticks: i32) {
        match t {
            Ent::Player => self.life.vit.fire = ticks,
            Ent::Mob(id) => {
                if let Some(i) = self.mob_index(id)
                    && let Some(m) = self.mobs.list[i].as_mut()
                {
                    m.fire = ticks;
                }
            }
        }
    }

    /// Push (`push`) of a living entity.
    pub(crate) fn push_ent(&mut self, t: Ent, d: [f64; 3]) {
        match t {
            Ent::Player => {
                let v = self.mobs.player_vel;
                let nv = [v[0] + d[0], v[1] + d[1], v[2] + d[2]];
                self.mobs.player_vel = nv;
                let velocity = crate::mobs::track::lp_quantize(nv);
                let _ = self.send(Event::Entity(EntityEvent::Motion { id: self.player_id, velocity }));
            }
            Ent::Mob(id) => {
                if let Some(i) = self.mob_index(id)
                    && let Some(m) = self.mobs.list[i].as_mut()
                {
                    m.vel = [m.vel[0] + d[0], m.vel[1] + d[1], m.vel[2] + d[2]];
                    m.needs_sync = true;
                }
            }
        }
    }

    fn knockback_resistance(&self, t: Ent) -> f64 {
        match t {
            Ent::Player => self.player_attribute("minecraft:knockback_resistance"),
            Ent::Mob(id) => self.mob_ref(id).map_or(0.0, |m| m.attrs.knockback_resistance.value()),
        }
    }

    // ---------- Arrows and trident ----------

    /// Impact of an arrow against an entity / impact of a trident against an entity; true if the projectile was
    /// deflected by the entity (never for simulated living ones).
    pub(crate) fn proj_hit_entity(&mut self, p: &mut Proj, h: EntityHit) -> bool {
        // F14: the breeze returns arrows and tridents.
        if self.proj_deflected_by_entity(p, h) {
            return true;
        }
        if h.ent.other() {
            self.proj_redirect(p, h.ent);
            self.arrow_hit_other(p, h.ent);
            return false;
        }
        let Some(t) = h.ent.living() else { return false };
        if p.kind == PKind::Trident {
            self.trident_hit_entity(p, t);
            return false;
        }
        let owner = self.proj_owner(p);
        let pow = (p.vel[0] * p.vel[0] + p.vel[1] * p.vel[1] + p.vel[2] * p.vel[2]).sqrt() as f32;
        let mut base = p.damage;
        // Power ("minecraft:damage" of the weapon, direct attacker of the `arrows` tag).
        if let Some(w) = &p.weapon {
            base = f64::from(self.ench_damage(w, p.kind, self.target_kind(t), base as f32));
        }
        let mut damage = (f64::from(pow) * base).clamp(0.0, 2.147_483_647e9).ceil() as i32;
        let tid = self.ent_net_id(t);
        if p.pierce > 0 {
            p.pierce_killed.get_or_insert_with(Vec::new);
            if p.pierced.len() > usize::from(p.pierce) {
                p.removed = true;
                return false;
            }
            p.pierced.push(tid);
        }
        if p.crit {
            let inc = i64::from(p.rng.next_int_bound(damage / 2 + 2));
            damage = (inc + i64::from(damage)).min(i64::from(i32::MAX)) as i32;
        }
        let fire_before = self.ent_fire(t);
        if p.fire > 0 {
            self.ignite_ent(t, 100);
        }
        let ty = "minecraft:arrow";
        // Living shooter (player included): the target becomes their last victim before the damage;
        // a dragon part is not a living being: the memory is cleared.
        self.set_last_hurt_mob(owner, (!matches!(h.ent, Target::Part(..))).then_some(t));
        let kb = (-p.vel[0], -p.vel[2]);
        // F15: the seated dragon sets the arrow on fire (1 s) which does not hurt it.
        if matches!(h.ent, Target::Part(..)) && crate::mobs::end::sitting_dragon(self, t) {
            p.fire = p.fire.max(20);
            p.sync_fire_flag();
        }
        if t == Ent::Player {
            // W8c: arrow at its position; piercing, never parried.
            self.prog.dmg_src =
                crate::item_use::DmgSrc { pos: Some(p.pos), piercing_arrow: p.pierce > 0, ..Default::default() };
        }
        if self.proj_damage(t, ty, damage as f32, owner, kb) {
            if t == Ent::Player && p.pierce == 0 {
                self.proj_player.arrow_count += 1;
                let c = self.proj_player.arrow_count;
                let _ = self
                    .send(Event::Entity(EntityEvent::Data { id: self.player_id, values: vec![(12, Meta::Int(c))] }));
            }
            // F15: a dragon part is not a living entity (neither knockback nor effects).
            if self.target_kind(t) != Some(Kind::EnderDragon) {
                self.arrow_knockback(p, t);
                self.arrow_post_hurt(p, t);
                // S9: `killed_by_arrow`. Arrow that pierced: the cumulative list of the killed, at
                // each hit landed (even without a death); otherwise the target alone, if it dies.
                let dead = !self.ent_alive(t);
                if dead && let Some(k) = &mut p.pierce_killed {
                    k.push(t);
                }
                if owner == Some(Ent::Player) {
                    match &p.pierce_killed {
                        Some(k) => self.s8_killed_by_arrow(k, p.weapon.as_ref()),
                        None if dead => self.s8_killed_by_arrow(&[t], p.weapon.as_ref()),
                        None => {}
                    }
                }
            }
            let pitch = 1.2 / (p.rng.next_float() * 0.2 + 0.9);
            if !p.silent {
                self.world_sound(p.hit_sound, 6, p.pos, 1.0, pitch);
            }
            if p.pierce == 0 {
                p.removed = true;
            }
        } else if self.proj_side_effects(t, false) {
            // The enderman dodges: the arrow continues, without bouncing, and the fire stays set.
            self.set_fire_ent(t, fire_before);
            self.proj_deflect_reverse(p, [0.2; 3]);
            if p.vel[0] * p.vel[0] + p.vel[1] * p.vel[1] + p.vel[2] * p.vel[2] < 1.0e-7 {
                if p.pickup == Pickup::Allowed {
                    self.proj_spawn_at(p.pos, p.item.clone(), 0.1);
                }
                p.removed = true;
            }
        }
        false
    }

    /// Last victim of a living author (`None`: non-living target, memory cleared), dated with
    /// its tick counter: goals "my owner's enemy" (wolf), fox that trusts.
    pub(crate) fn set_last_hurt_mob(&mut self, owner: Option<Ent>, t: Option<Ent>) {
        match owner {
            Some(Ent::Player) => self.mobs.player_hit = t.map(|e| (e, self.mobs.player_ticks)),
            Some(Ent::Mob(oid)) => {
                if let Some(o) = self.mobs.get_mut(oid) {
                    o.last_hurt_mob = t;
                }
            }
            None => {}
        }
    }

    /// Does the projectile undergo the aftermath of an impact (bounce, fire returned, sound)? The enderman
    /// only if it was hurt, any other target always.
    fn proj_side_effects(&self, t: Ent, hurt: bool) -> bool {
        hurt || self.target_kind(t) != Some(Kind::Enderman)
    }

    fn ent_fire(&self, t: Ent) -> i32 {
        match t {
            Ent::Player => self.life.vit.fire,
            Ent::Mob(id) => self.mob_ref(id).map_or(0, |m| m.fire),
        }
    }

    /// Species of a target (enchantment tags: impaling).
    pub(crate) fn target_kind(&self, t: Ent) -> Option<Kind> {
        match t {
            Ent::Player => None,
            Ent::Mob(id) => self.mob_ref(id).map(|m| m.kind),
        }
    }

    /// Punch: horizontal knockback in the flight direction.
    fn arrow_knockback(&mut self, p: &Proj, t: Ent) {
        let Some(w) = &p.weapon else { return };
        let k = f64::from(self.ench_knockback(w, p.kind, self.target_kind(t)));
        if k <= 0.0 {
            return;
        }
        let res = (1.0 - self.knockback_resistance(t)).max(0.0);
        if let Some(m) = arrow_knockback_push(p.vel, k * 0.6 * res) {
            self.push_ent(t, m);
        }
    }

    /// Effects after the injury: potion of the tipped arrow, glow of the spectral arrow.
    fn arrow_post_hurt(&mut self, p: &Proj, t: Ent) {
        let list: Vec<Inst> = match p.kind {
            PKind::Arrow => {
                let scale = self.duration_scale(&p.item);
                crate::potions::stack_effects(&p.item, scale)
            }
            PKind::SpectralArrow => vec![Inst::new("minecraft:glowing", p.spectral_duration, 0)],
            _ => Vec::new(),
        };
        for e in list {
            self.give_ent_effect(t, e, 1.0);
        }
    }

    /// Duration scale of a stack (`potion_duration_scale`, the item's default).
    pub(crate) fn duration_scale(&self, s: &Stack) -> f32 {
        crate::items::component(s, "minecraft:potion_duration_scale")
            .and_then(|v| match v {
                rc_proto::nbt::Nbt::Float(f) => Some(*f),
                rc_proto::nbt::Nbt::Double(d) => Some(*d as f32),
                _ => None,
            })
            .or_else(|| {
                self.life.items.default_component(s, "minecraft:potion_duration_scale")?.as_f64().map(|f| f as f32)
            })
            .unwrap_or(1.0)
    }

    /// Effect applied (instant one applied with `scale`), indirect source = owner.
    pub(crate) fn give_ent_effect(&mut self, t: Ent, e: Inst, scale: f64) {
        if crate::lootfn::instantaneous(&e.id) {
            self.instant_on(t, &e.id, e.amp, scale, None, None);
            return;
        }
        match t {
            Ent::Player => {
                self.add_effect(e);
            }
            Ent::Mob(id) => {
                if let Some(i) = self.mob_index(id)
                    && let Some(mut m) = self.mobs.list[i].take()
                {
                    self.add_mob_effect(&mut m, e);
                    self.mobs.list[i] = Some(m);
                }
            }
        }
    }

    /// Instant heal or damage; with a
    /// source (thrown potion, cloud), "indirect magic" damage. `origin` = position of the entity
    /// delivering the hit (potion, cloud): the victim's knockback, player or mob, follows
    /// "origin minus target"; without it, no direction (the game's random draw).
    pub(crate) fn instant_on(
        &mut self,
        t: Ent,
        id: &str,
        amp: i32,
        scale: f64,
        by: Option<Option<Ent>>,
        origin: Option<[f64; 3]>,
    ) {
        let harm = id == "minecraft:instant_damage";
        if !harm && id != "minecraft:instant_health" {
            if t == Ent::Player {
                self.apply_instant(id, amp, scale);
            }
            return;
        }
        let undead = self.target_kind(t).is_some_and(|k| k.undead());
        let ty = if by.is_some() { "minecraft:indirect_magic" } else { "minecraft:magic" };
        let causing = by.flatten();
        if harm == undead {
            let n = (scale * f64::from(4i32 << amp.clamp(0, 30)) + 0.5) as i32;
            match t {
                Ent::Player => self.heal(n as f32),
                Ent::Mob(mid) => {
                    if let Some(i) = self.mob_index(mid)
                        && let Some(m) = self.mobs.list[i].as_mut()
                        && m.health > 0.0
                    {
                        m.health = (m.health + n as f32).clamp(0.0, m.max_health());
                    }
                }
            }
        } else {
            let n = (scale * f64::from(6i32 << amp.clamp(0, 30)) + 0.5) as i32;
            match t {
                Ent::Player => {
                    // Responsible mob (witch, dragon): difficulty, named author; knockback
                    // from the origin (potion, cloud), random without it.
                    let kb = origin.map(|o| (o[0] - self.pos[0], o[2] - self.pos[2]));
                    let owner = match causing {
                        Some(Ent::Mob(o)) => self.mob_ref(o).map(|m| (*m).clone()),
                        _ => None,
                    };
                    match owner {
                        Some(om) => self.player_hurt_by_mob(&om, ty, n as f32, kb),
                        None => self.player_hurt_kb(ty, n as f32, kb),
                    };
                }
                Ent::Mob(mid) => {
                    if let Some(i) = self.mob_index(mid)
                        && let Some(mut m) = self.mobs.list[i].take()
                    {
                        // F07: the witch hit by her own potion takes nothing (the hit
                        // counts: animation, invulnerability).
                        let own = m.kind == Kind::Witch && causing == Some(t);
                        let dir = origin.map(|o| (o[0] - m.pos[0], o[2] - m.pos[2]));
                        let amt = if own { 0.0 } else { n as f32 };
                        self.mob_hurt_dir(&mut m, ty, amt, causing, origin, dir);
                        self.mobs.list[i] = Some(m);
                    }
                }
            }
        }
    }

    /// Reverse deflection.
    pub(crate) fn proj_deflect_reverse(&mut self, p: &mut Proj, power: [f64; 3]) {
        let rot = 170.0 + p.rng.next_float() * 20.0;
        p.vel = [p.vel[0] * -power[0] * 0.5, p.vel[1] * -power[1] * 0.5, p.vel[2] * -power[2] * 0.5];
        p.yaw += rot;
        p.needs_sync = true;
    }

    /// Item on the ground without random velocity, delay 10.
    pub(crate) fn proj_spawn_at(&mut self, pos: [f64; 3], s: Stack, y_off: f32) {
        let at = [pos[0], pos[1] + f64::from(y_off), pos[2]];
        let r = &mut self.life.rng;
        let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
        self.spawn_item(at, v, s, 10);
    }

    /// Impact of an arrow against a block.
    pub(crate) fn proj_hit_block(&mut self, p: &mut Proj, b: BlockHit) {
        let state = self.st(b.pos);
        p.last_state = Some(state);
        self.block_projectile_hit(p, b);
        if p.kind == PKind::Trident {
            self.channeling_block(p, b);
        }
        let s = |v: f64| {
            if v > 0.0 {
                1.0
            } else if v < 0.0 {
                -1.0
            } else {
                0.0
            }
        };
        let k = f64::from(0.05f32);
        p.pos = [p.pos[0] - s(p.vel[0]) * k, p.pos[1] - s(p.vel[1]) * k, p.pos[2] - s(p.vel[2]) * k];
        p.vel = [0.0; 3];
        let pitch = 1.2 / (p.rng.next_float() * 0.2 + 0.9);
        if !p.silent {
            self.world_sound(p.hit_sound, 6, p.pos, 1.0, pitch);
        }
        p.in_ground = true;
        p.shake = 7;
        p.crit = false;
        p.pierce = 0;
        p.hit_sound = "minecraft:entity.arrow.hit";
        p.pierced.clear();
        if let Some(k) = &mut p.pierce_killed {
            k.clear();
        }
        p.sync_arrow_flags();
    }

    /// Embedded arrow: nothing more than the countdown (effect lost after 600 ticks).
    pub(crate) fn arrow_ground_tick(&mut self, p: &mut Proj) {
        if p.kind == PKind::Arrow {
            self.tipped_arrow_tick(p);
        }
    }

    /// Arrow tick (server): tipped arrow embedded for 600 ticks → plain arrow.
    pub(crate) fn tipped_arrow_tick(&mut self, p: &mut Proj) {
        if p.in_ground
            && p.in_ground_time != 0
            && p.in_ground_time >= 600
            && crate::items::component(&p.item, "minecraft:potion_contents").is_some()
        {
            let _ = self.send(Event::Entity(EntityEvent::Event { id: p.id, event: 0 }));
            p.item = Stack { id: "minecraft:arrow".into(), count: 1, components: None };
            p.set_meta(11, Meta::Int(-1));
        }
    }

    /// Tick of a thrown trident before an arrow's tick: return by loyalty.
    pub(crate) fn trident_pre_tick(&mut self, p: &mut Proj) {
        if p.in_ground_time > 4 {
            p.dealt_damage = true;
        }
        let owner = self.proj_owner(p);
        if p.loyalty == 0 || !(p.dealt_damage || p.no_physics) {
            return;
        }
        let Some(o) = owner else { return };
        let acceptable = self.ent_alive(o) && !(o == Ent::Player && self.game_mode == 3);
        if !acceptable {
            if p.pickup == Pickup::Allowed {
                self.proj_spawn_at(p.pos, p.item.clone(), 0.1);
            }
            p.removed = true;
            return;
        }
        let eye = match o {
            // Player resolved in any dimension: from a parked world, their real coordinates
            // (not the fictitious position set during this tick).
            Ent::Player => {
                let at = if self.dims.home.is_some() { self.dims.home_pos } else { self.pos };
                [at[0], at[1] + self.player_eye_h(), at[2]]
            }
            Ent::Mob(id) => self.mob_ref(id).map_or(p.pos, |m| [m.pos[0], m.eye_y(), m.pos[2]]),
        };
        if let Ent::Mob(id) = o {
            let w = self.mob_ref(id).map_or(0.6, |m| f64::from(m.width()));
            let d = ((p.pos[0] - eye[0]).powi(2) + (p.pos[1] - eye[1]).powi(2) + (p.pos[2] - eye[2]).powi(2)).sqrt();
            if d < w + 1.0 {
                p.removed = true;
                return;
            }
        }
        p.no_physics = true;
        let v = [eye[0] - p.pos[0], eye[1] - p.pos[1], eye[2] - p.pos[2]];
        let loyalty = f64::from(p.loyalty);
        p.pos[1] += v[1] * 0.015 * loyalty;
        let accel = 0.05 * loyalty;
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let n = if l < 1.0e-4 { [0.0; 3] } else { v.map(|c| c / l) };
        p.vel = std::array::from_fn(|i| p.vel[i] * 0.95 + n[i] * accel);
        if p.return_ticks == 0 {
            self.world_sound("minecraft:item.trident.return", 6, p.pos, 10.0, 1.0);
        }
        p.return_ticks += 1;
        p.sync_arrow_flags();
    }

    /// 8 damage (+ impaling), channeling, bounce.
    fn trident_hit_entity(&mut self, p: &mut Proj, t: Ent) {
        let owner = self.proj_owner(p);
        let dmg = self.ench_damage(&p.item, p.kind, self.target_kind(t), 8.0);
        p.dealt_damage = true;
        let kb = (-p.vel[0], -p.vel[2]);
        let hurt = self.proj_damage(t, "minecraft:trident", dmg, owner, kb);
        if hurt {
            self.channeling_entity(p, t);
            self.arrow_knockback_trident(p, t);
        }
        // Dodged enderman: neither sound nor bounce, the trident keeps going.
        if self.proj_side_effects(t, hurt) {
            if !p.silent {
                self.world_sound("minecraft:item.trident.hit", 6, p.pos, 1.0, 1.0);
            }
            self.proj_deflect_reverse(p, [0.02, 0.2, 0.02]);
        }
    }

    fn arrow_knockback_trident(&mut self, p: &Proj, t: Ent) {
        // The trident carries no shooting weapon: no punch.
        let _ = (p, t);
    }

    /// Channeling on a target (thunderstorm, visible sky): lightning and thunder.
    fn channeling_entity(&mut self, p: &Proj, t: Ent) {
        if super::ench::level(&self.life.items, &p.item, "minecraft:channeling") <= 0 || !self.world_thundering() {
            return;
        }
        let Some(pos) = self.ent_pos(t) else { return };
        let bp = pos.map(|v| v.floor() as i32);
        if !self.can_see_sky(bp) {
            return;
        }
        // Thrown by the player: the "channeling" advancement follows the lightning's strikes.
        let cause = self.proj_owner(p) == Some(Ent::Player);
        self.proj_lightning(pos, cause);
    }

    /// Channeling on a lightning rod (thunderstorm, visible sky).
    fn channeling_block(&mut self, p: &Proj, b: BlockHit) {
        if super::ench::level(&self.life.items, &p.item, "minecraft:channeling") <= 0 || !self.world_thundering() {
            return;
        }
        let name = self.state_name(self.st(b.pos)).to_owned();
        if !name.ends_with("lightning_rod") || !self.can_see_sky(b.pos.map(|v| v)) {
            return;
        }
        // Impact point brought back into the block.
        let c = std::array::from_fn(|i| b.loc[i].clamp(f64::from(b.pos[i]), f64::from(b.pos[i] + 1)));
        let cause = self.proj_owner(p) == Some(Ent::Player);
        self.proj_lightning(c, cause);
    }

    /// Channeling lightning and thunder sound (volume 5).
    fn proj_lightning(&mut self, pos: [f64; 3], cause: bool) {
        self.spawn_lightning(pos, cause);
        self.world_sound("minecraft:item.trident.thunder", 6, pos, 5.0, 1.0);
    }

    // ---------- Other projectiles ----------

    /// The impact or deflection then the impact of the non-arrow projectiles.
    pub(crate) fn proj_hit(&mut self, p: &mut Proj, hit: Hit) {
        match hit {
            Hit::Miss(_) => {}
            Hit::Block(b) => {
                // Air block: nothing; world border: impact even in the air.
                if !b.border && self.is_air(b.pos) {
                    return;
                }
                if p.kind == PKind::Firework {
                    // Rocket: the block treats it as an entity that entered it, then it
                    // explodes (if it carries explosions), then the block's common reaction.
                    self.firework_inside_block(p, b.pos);
                    if super::weapons::firework_has_explosion(&p.item) {
                        self.firework_explode(p);
                    }
                    self.block_projectile_hit(p, b);
                } else {
                    self.block_projectile_hit(p, b);
                    self.proj_on_hit_block(p, b);
                }
                self.proj_land(p, hit);
                self.proj_on_hit(p, hit);
            }
            Hit::Entity(e) => {
                // F14: the breeze returns the projectiles.
                if self.proj_deflected_by_entity(p, e) {
                    return;
                }
                // F15: dragon part aimed at (the closest to the impact point).
                self.mobs.end.part = self.dragon_part_hit(e);
                self.proj_redirect(p, e.ent);
                self.proj_on_hit_entity(p, e);
                self.mobs.end.part = None;
                self.proj_land(p, hit);
                self.proj_on_hit(p, hit);
            }
        }
    }

    /// End of the common impact: `projectile_land` event at the point hit (entity) or at the
    /// block hit with its state of the moment.
    pub(crate) fn proj_land(&mut self, p: &Proj, hit: Hit) {
        use crate::mobs::recent::vibration::Src;
        let src = Some(Src::Projectile(self.proj_owner(p)));
        match hit {
            Hit::Miss(_) => {}
            Hit::Entity(e) => self.game_event_src("minecraft:projectile_land", e.loc, src, None),
            Hit::Block(b) => {
                let st = self.st(b.pos);
                let c = [f64::from(b.pos[0]) + 0.5, f64::from(b.pos[1]) + 0.5, f64::from(b.pos[2]) + 0.5];
                self.game_event_src("minecraft:projectile_land", c, src, Some(st));
            }
        }
    }

    fn proj_on_hit_entity(&mut self, p: &mut Proj, h: EntityHit) {
        let Some(t) = h.ent.living() else {
            self.proj_on_hit_other(p, h.ent);
            return;
        };
        let owner = self.proj_owner(p);
        let kb = (-p.vel[0], -p.vel[2]);
        match p.kind {
            PKind::Snowball => {
                let blaze = self.target_kind(t).is_some_and(|k| k.info().name == "minecraft:blaze");
                self.proj_damage(t, "minecraft:thrown", if blaze { 3.0 } else { 0.0 }, owner, kb);
            }
            PKind::Egg | PKind::EnderPearl => {
                self.proj_damage(t, "minecraft:thrown", 0.0, owner, kb);
            }
            PKind::Firework => {
                self.firework_explode(p);
            }
            PKind::SmallFireball => {
                // "Fireball" source with an author, otherwise "fireball without author".
                let ty = if owner.is_some() { "minecraft:fireball" } else { "minecraft:unattributed_fireball" };
                let before = self.ent_fire(t);
                self.ignite_ent(t, 100);
                if !self.proj_damage(t, ty, 5.0, owner, kb) {
                    self.set_fire_ent(t, before);
                }
            }
            PKind::WindCharge | PKind::BreezeWindCharge => {
                self.wind_on_hit_entity(p, t, !matches!(h.ent, Target::Part(..)))
            }
            PKind::LargeFireball => {
                // F12: 6; 1,000 to the ghast when the player returned it (direct source = the
                // big ball, signaled by `direct_large`).
                let ty = if owner.is_some() { "minecraft:fireball" } else { "minecraft:unattributed_fireball" };
                let reflected = matches!(t, Ent::Mob(id) if self.mob_ref(id).is_some_and(|o| crate::mobs::nether::reflected_fireball(o, true, owner)));
                self.mobs.fireballs.direct_large = true;
                self.proj_damage(t, ty, if reflected { 1000.0 } else { 6.0 }, owner, kb);
                self.mobs.fireballs.direct_large = false;
            }
            PKind::ShulkerBullet => self.shulker_bullet_hit(p, t, owner), // F15
            // F16: wither skull.
            PKind::WitherSkull => self.wither_skull_hit_entity(t, owner, kb),
            _ => {}
        }
    }

    // ---------- Non-living targets: crystal, shulker bullet, deflectable projectile ----------

    /// Server-side damage computation of a non-living target; true if it lands. The crystal explodes (except fire or
    /// dragon author), the shulker bullet destroys itself (sound, 15 particles), a
    /// deflectable projectile never takes damage.
    pub(crate) fn hurt_other(&mut self, t: Target, ty: &str, by: Option<Ent>, amount: f32) -> bool {
        match t {
            Target::Crystal(id) => self.crystal_hurt(id, ty, by, false),
            Target::Proj(id) => self.bullet_destroy(id),
            // G5-F06: minecart.
            Target::Cart(id) => match self.cart_take(id) {
                Some((k, mut c)) => {
                    let r = self.cart_hurt(&mut c, ty, amount, by);
                    self.cart_put(k, c);
                    r
                }
                None => false,
            },
            _ => false,
        }
    }

    /// A deflectable projectile that is hit goes off in the direction of the gaze
    /// of the author of the projectile that hits it (power 1; nothing without an author), who becomes
    /// its author; acceleration reset to 0.1 ("by attack" deflection). The shulker bullet
    /// returns it instead according to its own momentum: its known velocity normalized (zero on the first tick).
    fn proj_redirect(&mut self, p: &Proj, t: Target) {
        let Target::Proj(id) = t else { return };
        let look = if p.kind == PKind::ShulkerBullet {
            let v = p.bullet.as_deref().map_or([0.0; 3], |b| b.known_speed);
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            Some(if l < f64::from(1.0e-5f32) { [0.0; 3] } else { v.map(|c| c / l) })
        } else {
            match self.proj_owner(p) {
                Some(Ent::Player) => Some(crate::mobs::nether::ghast::view_vector(self.pitch, self.yaw)),
                Some(Ent::Mob(m)) => self.mob_ref(m).map(|o| crate::mobs::nether::ghast::view_vector(o.pitch, o.yaw)),
                None => None,
            }
        };
        let owner = p.owner;
        let Some(q) =
            self.proj.list.iter_mut().find(|q| q.id == id && !q.removed && super::physics::redirectable(q.kind))
        else {
            return;
        };
        if let Some(l) = look {
            q.vel = l;
            q.needs_sync = true;
        }
        q.owner = owner;
        q.accel = 0.1;
    }

    /// Impact of an arrow against an entity / impact of a trident against an entity, on a non-living target.
    fn arrow_hit_other(&mut self, p: &mut Proj, t: Target) {
        let owner = self.proj_owner(p);
        if p.kind == PKind::Trident {
            p.dealt_damage = true;
            // G5-F06: burning trident on a TNT minecart, explosion before the damage.
            self.burning_arrow_on_tnt_cart(p, t, owner);
            let burning = Some(p.fire > 0);
            self.with_direct(burning, |s| s.hurt_other(t, "minecraft:trident", owner, 8.0));
            if !p.silent {
                self.world_sound("minecraft:item.trident.hit", 6, p.pos, 1.0, 1.0);
            }
            self.proj_deflect_reverse(p, [0.02, 0.2, 0.02]);
            return;
        }
        let pow = (p.vel[0] * p.vel[0] + p.vel[1] * p.vel[1] + p.vel[2] * p.vel[2]).sqrt() as f32;
        let mut base = p.damage;
        if let Some(w) = &p.weapon {
            base = f64::from(self.ench_damage(w, p.kind, None, base as f32));
        }
        let damage = (f64::from(pow) * base).clamp(0.0, 2.147_483_647e9).ceil() as i32;
        let (Target::Crystal(tid) | Target::Proj(tid) | Target::Cart(tid)) = t else { return };
        if p.pierce > 0 {
            p.pierce_killed.get_or_insert_with(Vec::new);
            if p.pierced.len() > usize::from(p.pierce) {
                p.removed = true;
                return;
            }
            p.pierced.push(tid);
        }
        let mut damage = damage;
        if p.crit {
            // Crit draw (only the minecart takes the damage into account).
            let extra = p.rng.next_int_bound(damage / 2 + 2);
            damage = (i64::from(extra) + i64::from(damage)).min(i64::from(i32::MAX)) as i32;
        }
        // Non-living target: the shooter forgets their last victim.
        self.set_last_hurt_mob(owner, None);
        // G5-F06: burning arrow on a TNT minecart: immediate explosion (the hit lands,
        // the minecart being removed).
        let exploded = self.burning_arrow_on_tnt_cart(p, t, owner);
        let burning = Some(p.fire > 0);
        if exploded || self.with_direct(burning, |s| s.hurt_other(t, "minecraft:arrow", owner, damage as f32)) {
            let pitch = 1.2 / (p.rng.next_float() * 0.2 + 0.9);
            if !p.silent {
                self.world_sound(p.hit_sound, 6, p.pos, 1.0, pitch);
            }
            if p.pierce == 0 {
                p.removed = true;
            }
        } else {
            self.proj_deflect_reverse(p, [0.2; 3]);
            if p.vel[0] * p.vel[0] + p.vel[1] * p.vel[1] + p.vel[2] * p.vel[2] < 1.0e-7 {
                if p.pickup == Pickup::Allowed {
                    self.proj_spawn_at(p.pos, p.item.clone(), 0.1);
                }
                p.removed = true;
            }
        }
    }

    /// Burning arrow or trident hitting a TNT minecart: explosion at the projectile's velocity
    /// (author = its owner); true if the minecart blew up.
    fn burning_arrow_on_tnt_cart(&mut self, p: &Proj, t: Target, owner: Option<Ent>) -> bool {
        let Target::Cart(cid) = t else { return false };
        if p.fire <= 0 {
            return false;
        }
        let Some((k, mut c)) = self.cart_take(cid) else { return false };
        let mut exploded = false;
        if c.kind == crate::minecart::CKind::Tnt {
            let v2 = p.vel[0] * p.vel[0] + p.vel[1] * p.vel[1] + p.vel[2] * p.vel[2];
            self.tnt_cart_explode(&mut c, owner, v2);
            exploded = c.removed;
        }
        self.cart_put(k, c);
        exploded
    }

    /// Impact against an entity of the other projectiles on a non-living target (damage of their type).
    fn proj_on_hit_other(&mut self, p: &mut Proj, t: Target) {
        let owner = self.proj_owner(p);
        let ty = match p.kind {
            PKind::Snowball | PKind::Egg | PKind::EnderPearl => "minecraft:thrown",
            PKind::SmallFireball | PKind::LargeFireball if owner.is_some() => "minecraft:fireball",
            PKind::SmallFireball | PKind::LargeFireball => "minecraft:unattributed_fireball",
            PKind::WindCharge | PKind::BreezeWindCharge => "minecraft:wind_charge",
            PKind::ShulkerBullet => "minecraft:mob_projectile",
            PKind::WitherSkull if owner.is_some() => "minecraft:wither_skull",
            PKind::WitherSkull => "minecraft:magic",
            PKind::Firework => {
                self.firework_explode(p);
                return;
            }
            _ => return,
        };
        let amount = match p.kind {
            PKind::SmallFireball => 5.0,
            PKind::LargeFireball => 6.0,
            PKind::WindCharge | PKind::BreezeWindCharge => 1.0,
            PKind::ShulkerBullet => 4.0,
            PKind::WitherSkull if owner.is_some() => 8.0,
            PKind::WitherSkull => 5.0,
            _ => 0.0,
        };
        // Wind charge on a non-living target: the thrower forgets their last victim.
        if p.kind.wind() {
            self.set_last_hurt_mob(owner, None);
        }
        // Fireballs always burn in flight; the others only if they were lit.
        let burning = p.fire > 0 || matches!(p.kind, PKind::SmallFireball | PKind::LargeFireball);
        self.with_direct(Some(burning), |s| s.hurt_other(t, ty, owner, amount));
        if p.kind.wind() {
            let c = p.pos;
            self.wind_explode(p, c);
        }
    }

    fn proj_on_hit_block(&mut self, p: &mut Proj, b: BlockHit) {
        match p.kind {
            // F15: explosion (2 particles) and shulker bullet sound.
            PKind::ShulkerBullet => {
                self.particles("minecraft:explosion", p.pos, [0.2, 0.2, 0.2], 0.0, 2);
                self.world_sound("minecraft:entity.shulker_bullet.hit", 5, p.pos, 1.0, 1.0);
            }
            PKind::SplashPotion | PKind::LingeringPotion if is_water(&p.item) => {
                let f = FACES[b.face];
                let e = [b.pos[0] + f[0], b.pos[1] + f[1], b.pos[2] + f[2]];
                self.douse_fire(e, p);
                self.douse_fire(b.pos.map(|v| v), p);
                let back = [e[0] - f[0], e[1] - f[1], e[2] - f[2]];
                let _ = back;
                for d in [[0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]] {
                    self.douse_fire([e[0] + d[0], e[1] + d[1], e[2] + d[2]], p);
                }
            }
            PKind::SmallFireball => {
                let f = FACES[b.face];
                let q = [b.pos[0] + f[0], b.pos[1] + f[1], b.pos[2] + f[2]];
                let mob_owner = matches!(self.proj_owner(p), Some(Ent::Mob(_)));
                if (!mob_owner || self.rule("minecraft:mob_griefing", true)) && self.is_air(q) {
                    self.place_fire(q);
                }
            }
            PKind::WindCharge | PKind::BreezeWindCharge => self.wind_on_hit_block(p, b),
            _ => {}
        }
    }

    /// Fire placed (soul fire on the blocks of the tag, otherwise fire).
    fn place_fire(&mut self, q: [i32; 3]) {
        let below = self.state_name(self.st([q[0], q[1] - 1, q[2]])).to_owned();
        let soul = matches!(below.as_str(), "minecraft:soul_sand" | "minecraft:soul_soil");
        let name = if soul { "minecraft:soul_fire" } else { "minecraft:fire" };
        if let Some(s) = self.default_state_of(name) {
            self.set_block(q, s, flag::ALL);
        }
    }

    /// Default state of a named block.
    pub(crate) fn default_state_of(&self, name: &str) -> Option<u32> {
        self.data.blocks.iter().find(|b| b.name == name).map(|b| b.default_state)
    }

    /// Fire destroyed, candle put out, campfire put out.
    fn douse_fire(&mut self, q: [i32; 3], p: &Proj) {
        let s = self.st(q);
        let name = self.state_name(s).to_owned();
        if name == "minecraft:fire" || name == "minecraft:soul_fire" {
            let by = Some(crate::mobs::recent::vibration::Src::Projectile(self.proj_owner(p)));
            self.destroy_block_by(q, false, 512, by);
        } else if (name.ends_with("candle") || name.ends_with("candle_cake")) && self.v().get_bool(s, "lit") {
            let ns = self.v().set_bool(s, "lit", false);
            self.set_block(q, ns, flag::ALL);
            self.level_event(1009, q, 0);
        } else if name.ends_with("campfire") && self.v().get_bool(s, "lit") {
            self.level_event(1009, q, 0);
            let ns = self.v().set_bool(s, "lit", false);
            self.set_block(q, ns, flag::ALL);
        }
        let _ = p;
    }

    /// Kind-specific impact (after the impact against an entity / impact against a block).
    fn proj_on_hit(&mut self, p: &mut Proj, hit: Hit) {
        match p.kind {
            PKind::Snowball => {
                let _ = self.send(Event::Entity(EntityEvent::Event { id: p.id, event: 3 }));
                p.removed = true;
            }
            PKind::Egg => {
                if p.rng.next_int_bound(8) == 0 {
                    let n = if p.rng.next_int_bound(32) == 0 { 4 } else { 1 };
                    for _ in 0..n {
                        self.egg_chicken(p);
                    }
                }
                let _ = self.send(Event::Entity(EntityEvent::Event { id: p.id, event: 3 }));
                p.removed = true;
            }
            PKind::EnderPearl => self.pearl_land(p),
            PKind::SplashPotion | PKind::LingeringPotion => self.potion_land(p, hit),
            PKind::XpBottle => {
                let bp = p.block_pos();
                self.level_event(2002, bp, -13083194);
                if !p.silent {
                    self.level_event(1053, bp, 0);
                }
                let n = 3 + p.rng.next_int_bound(5) + p.rng.next_int_bound(5);
                let dir = match hit {
                    Hit::Block(b) => FACES[b.face].map(f64::from),
                    _ => p.vel.map(|v| -v),
                };
                self.orbs_with_direction(hit.loc(), dir, n);
                p.removed = true;
            }
            PKind::SmallFireball | PKind::WindCharge | PKind::BreezeWindCharge | PKind::ShulkerBullet => {
                p.removed = true
            }
            // F15: breath cloud, except on its thrower (a dragon part is not one).
            PKind::DragonFireball => {
                let hit_owner = match hit {
                    Hit::Entity(EntityHit { ent: Target::Living(t), .. }) => self.proj_owner(p) == Some(t),
                    _ => false,
                };
                self.dragon_fireball_impact(p, hit_owner);
            }
            PKind::LargeFireball => {
                // F12: explosion (fire per `mob_griefing`), "player" type if an author exists.
                let by = self.proj_owner(p);
                let ty = if by.is_some() { "minecraft:player_explosion" } else { "minecraft:explosion" };
                let griefing = self.rule("minecraft:mob_griefing", true);
                self.mobs.fireballs.direct_large = true;
                let src = Some(crate::mobs::recent::vibration::Src::Projectile(by));
                self.game_event_src("minecraft:explode", p.pos, src, None);
                self.prog.expl_cause_next = Some("minecraft:fireball"); // W8c
                self.carts.blast_direct = Some(true); // G5-F06: the ball burns
                self.deco.blast_wet = p.in_water; // direct source: the fireball
                self.explode_core(None, by, None, p.pos, p.power as f32, griefing, ty);
                self.prog.expl_cause_next = None;
                self.mobs.fireballs.direct_large = false;
                p.removed = true;
            }
            PKind::WitherSkull => self.wither_skull_explode(p), // F16
            _ => {}
        }
    }

    /// Chick from the egg: baby, variant of the egg.
    fn egg_chicken(&mut self, p: &Proj) {
        let mut m = self.new_mob(Kind::Chicken, p.pos, p.yaw);
        m.age = -24000;
        // Variant carried by the egg (component), otherwise the default variant (temperate).
        let name = crate::items::component(&p.item, "minecraft:chicken_variant")
            .and_then(rc_proto::nbt::Nbt::as_str)
            .map(str::to_owned)
            .or_else(|| {
                self.life.items.default_component(&p.item, "minecraft:chicken_variant")?.as_str().map(str::to_owned)
            })
            .unwrap_or_else(|| "minecraft:temperate".to_owned());
        if let Some(i) = self.regs.entries("minecraft:chicken_variant").iter().position(|n| *n == name) {
            m.variant = i as i32;
        }
        m.head_yaw = m.yaw;
        m.body_yaw = m.yaw;
        self.add_mob(m);
    }

    /// Experience orbs thrown in a direction.
    pub(crate) fn orbs_with_direction(&mut self, pos: [f64; 3], dir: [f64; 3], mut amount: i32) {
        while amount > 0 {
            let v = crate::drops::orb_value(amount);
            amount -= v;
            let r = self.life.rng.next_int_bound(40);
            if let Some(o) = self.life.orbs.iter_mut().find(|o| {
                !o.is_removed()
                    && o.value == v
                    && (o.id - r) % 40 == 0
                    && (0..3).all(|a| (o.pos[a] - pos[a]).abs() <= 0.5)
            }) {
                o.count += 1;
                o.age = 0;
                continue;
            }
            self.add_orb_dir(pos, dir, v);
        }
    }

    /// Teleport of the player (5 damage), endermite at 5%.
    fn pearl_land(&mut self, p: &mut Proj) {
        if p.owner != Some(self.uuid) || self.life.vit.dead || self.life.vit.sleeping.is_some() {
            p.removed = true;
            return;
        }
        let to = p.old_pos;
        // Pearl landed in a parked dimension (the player is elsewhere): endermite in that
        // of the pearl, at the player's position; player taken back to their dimension on return.
        let away = self.dims.home.clone();
        let owner_pos = if away.is_some() { self.dims.home_pos } else { self.pos };
        if p.rng.next_float() < 0.05 && self.rule("minecraft:spawn_monsters", true) && self.level.difficulty_id() != 0 {
            let mut m = self.new_mob(Kind::Endermite, owner_pos, self.yaw);
            m.pitch = self.pitch;
            m.head_yaw = m.yaw;
            m.body_yaw = m.yaw;
            self.add_mob(m);
        }
        // Pearl in return delay (passed through a portal): the player too.
        let cooling = self.entity_portal_cooling(p.id);
        if away.is_some() {
            // Sound played after the teleport, in the arrival dimension (here the sends are cut off).
            self.dims.pearl_jumps.push((self.dim.name.clone(), to, cooling));
            p.removed = true;
            return;
        }
        if cooling {
            self.dims.player.cooldown = crate::portal::PLAYER_COOLDOWN;
        }
        self.send_relative_teleport(to, 0.0, 0.0, crate::dims::rel::ROTATION);
        self.life.vit.fall = 0.0;
        self.mobs.player_vel = [0.0; 3];
        self.hurt("minecraft:ender_pearl", 5.0);
        self.play_sound("minecraft:entity.player.teleport", 7, to, 1.0, 1.0);
        p.removed = true;
    }

    /// Water (damage to the sensitive, extinguishing), effects, cloud.
    fn potion_land(&mut self, p: &mut Proj, hit: Hit) {
        let item = p.item.clone();
        let effects = crate::potions::stack_effects(&item, 1.0);
        let water = is_water(&item);
        let b = p.bbox();
        let area = rc_physics::Aabb {
            min: [b.min[0] - 4.0, b.min[1] - 2.0, b.min[2] - 4.0],
            max: [b.max[0] + 4.0, b.max[1] + 2.0, b.max[2] + 4.0],
        };
        if water {
            let ents: Vec<(Ent, [f64; 3])> = self.living_in(&area);
            for (e, pos) in ents {
                let d = (pos[0] - p.pos[0]).powi(2) + (pos[1] - p.pos[1]).powi(2) + (pos[2] - p.pos[2]).powi(2);
                if d >= 16.0 {
                    continue;
                }
                let sensitive = self.target_kind(e).is_some_and(|k| {
                    matches!(
                        k.info().name,
                        "minecraft:enderman" | "minecraft:blaze" | "minecraft:snow_golem" | "minecraft:strider"
                    )
                });
                if sensitive {
                    let by = self.proj_owner(p);
                    self.proj.direct_potion = Some(true);
                    self.proj_damage(e, "minecraft:indirect_magic", 1.0, by, (p.pos[0] - pos[0], p.pos[2] - pos[2]));
                    self.proj.direct_potion = None;
                }
                if self.ent_fire(e) > 0 && self.ent_alive(e) {
                    self.set_fire_ent(e, 0);
                }
            }
            // Axolotls hit (even outside the sphere of 4): 1,800 more air, cap 6,000.
            for m in self.mobs.list.iter_mut().flatten() {
                if m.kind == Kind::Axolotl && !m.removed && m.touches(&area) {
                    m.air = (m.air + 1800).min(crate::mobs::brainy::axolotl::MAX_AIR);
                }
            }
        }
        if !effects.is_empty() {
            if p.kind == PKind::SplashPotion {
                self.splash_effects(p, &item, hit);
            } else {
                self.lingering_cloud(p, &item, hit);
            }
        }
        let instant = crate::potions::potion_of(&item)
            .is_some_and(|n| crate::potions::potion_effects(&n).iter().any(|e| crate::lootfn::instantaneous(e.0)));
        let bp = p.block_pos();
        let color = potion_color(&item);
        if instant {
            self.level_event(2007, bp, color);
            if !p.silent {
                self.level_event(1054, bp, 0);
            }
        } else {
            self.level_event(2002, bp, color);
            if !p.silent {
                self.level_event(1053, bp, 0);
            }
        }
        p.removed = true;
    }

    /// Living entities whose box touches `area`.
    pub(crate) fn living_in(&self, area: &rc_physics::Aabb) -> Vec<(Ent, [f64; 3])> {
        let mut out = Vec::new();
        for m in self.mobs.list.iter().flatten() {
            if m.alive() && !m.removed && m.touches(area) {
                out.push((Ent::Mob(m.id), m.pos));
            }
        }
        if self.ent_alive(Ent::Player) && self.game_mode != 3 && self.player_aabb().intersects(area) {
            out.push((Ent::Player, self.pos));
        }
        out
    }

    /// Box of a living entity.
    fn living_box(&self, e: Ent) -> Option<rc_physics::Aabb> {
        self.ent_bbox(e)
    }

    /// Impact of a throwable potion.
    fn splash_effects(&mut self, p: &Proj, item: &Stack, hit: Hit) {
        let scale_d = self.duration_scale(item);
        let effects = crate::potions::stack_effects(item, 1.0);
        let loc = hit.loc();
        let pb = p.bbox();
        let off = [loc[0] - p.pos[0], loc[1] - p.pos[1], loc[2] - p.pos[2]];
        let potion_box = pb.offset(off[0], off[1], off[2]);
        let area = rc_physics::Aabb {
            min: [potion_box.min[0] - 4.0, potion_box.min[1] - 2.0, potion_box.min[2] - 4.0],
            max: [potion_box.max[0] + 4.0, potion_box.max[1] + 2.0, potion_box.max[2] + 4.0],
        };
        let m = super::physics::margin(p.tick_count);
        let by = self.proj_owner(p);
        for (e, _) in self.living_in(&area) {
            let Some(eb) = self.living_box(e) else { continue };
            let eb = rc_physics::Aabb { min: eb.min.map(|v| v - m), max: eb.max.map(|v| v + m) };
            let mut dist = 0.0;
            for k in 0..3 {
                let d = (potion_box.min[k] - eb.max[k]).max(eb.min[k] - potion_box.max[k]).max(0.0);
                dist += d * d;
            }
            if dist >= 16.0 {
                continue;
            }
            let scale = 1.0 - dist.sqrt() / 4.0;
            for eff in &effects {
                if crate::lootfn::instantaneous(&eff.id) {
                    // Direct source: the potion (the enderman is only hurt by water).
                    self.proj.direct_potion = Some(is_water(item));
                    self.instant_on(e, &eff.id, eff.amp, scale, Some(by), Some(p.pos));
                    self.proj.direct_potion = None;
                } else {
                    let mut n = eff.clone();
                    if !n.infinite() {
                        n.dur = (scale * f64::from(eff.dur) * f64::from(scale_d) + 0.5) as i32;
                    }
                    if n.infinite() || n.dur > 20 {
                        self.give_ent_effect(e, n, 1.0);
                    }
                }
            }
        }
    }

    /// Cloud of radius 3 (−0.5 per use), 600 ticks.
    fn lingering_cloud(&mut self, p: &Proj, item: &Stack, hit: Hit) {
        let pos = match hit {
            Hit::Entity(e) => self.target_pos(e.ent),
            _ => p.pos,
        };
        let mut c = super::cloud::Cloud::new(self.proj_new_id(), pos);
        c.owner = p.owner.filter(|_| self.proj_owner(p).is_some());
        c.radius = 3.0;
        c.radius_on_use = -0.5;
        c.duration = 600;
        c.wait_time = 10;
        c.radius_per_tick = -c.radius / c.duration as f32;
        c.potion = Some(item.clone());
        c.duration_scale = self.duration_scale(item);
        self.cloud_add(c);
    }

    // ---------- Rockets ----------

    /// Block hit by a rocket, treated as an entity that entered it (without deferred effects:
    /// no fire, no extinguishing, no freezing). Only the honey block acts on a rocket: slide. The
    /// other reactions target living things, items or vehicles; the cactus only
    /// marks the rocket as "hit" (velocity sync, already requested after the impact).
    fn firework_inside_block(&mut self, p: &mut Proj, pos: [i32; 3]) {
        if self.state_name(self.st(pos)) == "minecraft:honey_block"
            && let Some(v) = rc_physics::honey_slide(p.pos, p.vel, p.on_ground, p.kind.size().0 as f32, pos)
        {
            p.vel = v;
        }
    }

    /// Event 17, explosion damage, removal.
    pub(crate) fn firework_explode(&mut self, p: &mut Proj) {
        let _ = self.send(Event::Entity(EntityEvent::Event { id: p.id, event: 17 }));
        // `explode` event at the rocket, emitted by its shooter.
        let owner = self.proj_owner(p);
        self.game_event_at("minecraft:explode", p.pos, owner);
        let n = super::weapons::firework_explosions(&p.item);
        if n > 0 {
            let dmg = 5.0 + n as f32 * 2.0;
            let owner = self.proj_owner(p);
            if let Some(a) = p.attached {
                // "Rocket minus carrier" direction: the hand offset when gliding, zero otherwise.
                let ap = self.ent_pos(a).unwrap_or(p.pos);
                self.proj_damage(a, "minecraft:fireworks", dmg, owner, (p.pos[0] - ap[0], p.pos[2] - ap[2]));
            }
            let b = p.bbox();
            let area = rc_physics::Aabb { min: b.min.map(|v| v - 5.0), max: b.max.map(|v| v + 5.0) };
            for (e, pos) in self.living_in(&area) {
                if Some(e) == p.attached {
                    continue;
                }
                let d2 = (pos[0] - p.pos[0]).powi(2) + (pos[1] - p.pos[1]).powi(2) + (pos[2] - p.pos[2]).powi(2);
                if d2 > 25.0 {
                    continue;
                }
                let h = self.ent_bbox(e).map_or(1.8, |b| b.max[1] - b.min[1]);
                let see = (0..2).any(|k| {
                    let to = [pos[0], pos[1] + h * 0.5 * f64::from(k), pos[2]];
                    self.proj_clip(p.pos, to).is_none()
                });
                if see {
                    let dist = d2.sqrt();
                    let d = dmg * ((5.0 - dist) / 5.0).sqrt() as f32;
                    let kb = (pos[0] - p.pos[0], pos[2] - p.pos[2]);
                    self.proj_damage(e, "minecraft:fireworks", d, owner, (-kb.0, -kb.1));
                }
            }
        }
        p.removed = true;
    }

    // ---------- Blocks hit ----------

    /// Reaction of the block hit: target, TNT, bell, campfire, chorus flower, decorated pot,
    /// dripstone (trident), wooden button and plate (arrows).
    pub(crate) fn block_projectile_hit(&mut self, p: &mut Proj, b: BlockHit) {
        let s = self.st(b.pos);
        let name = self.state_name(s).to_owned();
        let short = name.trim_start_matches("minecraft:");
        let on_fire = p.fire > 0;
        let player_owned = p.owner == Some(self.uuid);
        let griefing_ok = player_owned || p.owner.is_none() || self.rule("minecraft:mob_griefing", true);
        match short {
            "target" => {
                let power = target_strength(b);
                let duration = if p.kind.arrow_like() { 20 } else { 8 };
                let blk = self.v().block(s);
                if !self.has_scheduled_tick(b.pos, blk) {
                    let ns = self.v().set_int(s, "power", power);
                    self.set_block(b.pos, ns, flag::ALL);
                    self.schedule_tick(b.pos, blk, duration);
                }
                if player_owned {
                    self.s8_target_hit(power, p); // S9
                }
            }
            "tnt" if on_fire && griefing_ok => {
                // Author = living owner of the projectile; a player in adventure does not prime it
                // (the projectile has no `can_break`).
                let owner = self.proj_owner(p);
                if self.tnt_prime_block_with(b.pos, owner, None) {
                    self.remove_block(b.pos, false);
                }
            }
            // Bell: hit from the right side only (as by hand); ringer = the shooting player.
            "bell" => {
                let rel_y = b.loc[1] - f64::from(b.pos[1]);
                self.bell_on_hit(b.pos, s, Dir::from_index(b.face), rel_y, player_owned);
            }
            // Amethyst (block, budding, buds, cluster): chime.
            "amethyst_block" | "budding_amethyst" | "small_amethyst_bud" | "medium_amethyst_bud"
            | "large_amethyst_bud" | "amethyst_cluster" => {
                let pitch = 0.5 + self.bs.rng.next_float() * 1.2;
                self.block_sound(b.pos, "minecraft:block.amethyst_block.chime", 1.0, pitch);
            }
            // Falling big dripleaf: tipped at once (any projectile).
            "big_dripleaf" => self.dripleaf_projectile_hit(s, b.pos),
            "chorus_flower" if griefing_ok && p.kind != PKind::Firework => {
                let by = Some(crate::mobs::recent::vibration::Src::Projectile(self.proj_owner(p)));
                self.destroy_block_by(b.pos, true, 512, by);
            }
            "decorated_pot" if griefing_ok => {
                // Cracked first (without a visible update), so that the loot is the sherds.
                let cracked = self.v().set_bool(s, "cracked", true);
                self.set_block(b.pos, cracked, flag::INVISIBLE | flag::SKIP_BE_SIDE_EFFECTS);
                let by = Some(crate::mobs::recent::vibration::Src::Projectile(self.proj_owner(p)));
                self.destroy_block_by(b.pos, true, 512, by);
            }
            "pointed_dripstone"
                if p.kind == PKind::Trident && griefing_ok && p.vel[0].hypot(p.vel[1]).hypot(p.vel[2]) > 0.6 =>
            {
                self.destroy_block(b.pos, true, 512);
            }
            _ if short.ends_with("campfire")
                && on_fire
                && !self.v().get_bool(s, "lit")
                && !self.v().get_bool(s, "waterlogged") =>
            {
                let ns = self.v().set_bool(s, "lit", true);
                self.set_block(b.pos, ns, flag::ALL | 8);
            }
            // Candles and candle cakes (the plain "candle_cake" included).
            _ if (short.ends_with("candle") || short.ends_with("candle_cake"))
                && on_fire
                && !self.v().get_bool(s, "lit")
                && !self.v().get_bool(s, "waterlogged") =>
            {
                let ns = self.v().set_bool(s, "lit", true);
                self.set_block(b.pos, ns, flag::ALL | 8);
            }
            _ => {}
        }
        if p.kind.arrow_like() {
            self.arrow_presses(b.pos, s, &name);
        }
    }

    /// Wooden buttons and plates pressed by arrows: left to the blocks (S3),
    /// which test the arrows every tick if they can see them.
    fn arrow_presses(&mut self, _p: [i32; 3], _s: u32, _name: &str) {}
}

/// Power of the target block according to the impact point.
pub fn target_strength(b: BlockHit) -> i32 {
    let frac = |v: f64| v - v.floor();
    let dx = (frac(b.loc[0]) - 0.5).abs();
    let dy = (frac(b.loc[1]) - 0.5).abs();
    let dz = (frac(b.loc[2]) - 0.5).abs();
    let distance = match b.face {
        0 | 1 => dx.max(dz),
        2 | 3 => dx.max(dy),
        _ => dy.max(dz),
    };
    (15.0 * ((0.5 - distance) / 0.5).clamp(0.0, 1.0)).ceil().max(1.0) as i32
}
