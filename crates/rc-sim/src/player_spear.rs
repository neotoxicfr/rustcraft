//! The player's spear on the integrated server, after game 26.3: the stab of a left click
//! (`player_action` 8, `piercing_weapon` component), the charge while the use is held
//! (`kinetic_weapon` component), and the weapon's enchantment effects after a piercing attack
//! (Lunge, `ench_fx::player_post_piercing_attack`). Both find their targets along the weapon's
//! reach like the spears of mobs (`mobs::piglins::spear`).
//!
//! Targets: mobs, the other players of the local network, and the projectiles a hit sends back.
//! Known differences: the other non-living entities (minecarts, decorations, End crystals,
//! interaction entities) are not reached; a dismount only takes a rider off a mob; the swing shown
//! to the other players is sent even while a previous swing still plays; the speed of a projectile
//! target is its velocity.

use rc_client::{EntityEvent, Event};
use rc_data::KineticParams;

use crate::mobs::cubes::view_vector;
use crate::mobs::mob::{Ent, Mob};
use crate::mobs::mth::{DEG, cos, sin};
use crate::player::Stack;
use crate::server::Server;

#[cfg(test)]
#[path = "player_spear_tests.rs"]
mod tests;

/// Use duration of an item with a kinetic weapon: it lasts until released.
pub(crate) const KINETIC_USE_TICKS: i32 = 72_000;

/// Knockback every stab deals before the weapon's own.
const STAB_BASE_KNOCKBACK: f32 = 0.4;

/// Sound category of the player's sounds.
const PLAYERS: i32 = 7;

const MAIN_HAND: i32 = 0;

/// Mob kinds a stab cannot take off their vehicle.
const CANNOT_BE_DISMOUNTED: &str = "#minecraft:cannot_be_dismounted_by_item_usage";

/// Something the stab or the charge can hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpearTarget {
    Mob(i32),
    /// Another player of the local network (entity id).
    OtherPlayer(i32),
    /// A projectile a hit sends back along the player's gaze (entity id).
    Projectile(i32),
}

impl SpearTarget {
    /// Living targets count for the `spear_mobs` criterion.
    fn living(self) -> bool {
        !matches!(self, SpearTarget::Projectile(_))
    }
}

/// What one stab does to its target.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StabEffects {
    pub damage: f32,
    pub deals_damage: bool,
    pub deals_knockback: bool,
    pub dismounts: bool,
}

/// `piercing_weapon` component: what its stab does besides the damage, and its sounds.
struct StabSettings {
    deals_knockback: bool,
    dismounts: bool,
    sound: Option<String>,
    hit_sound: Option<String>,
}

/// Reach of the held weapon for the player: (minimum, maximum), and the margin added to the
/// boxes of the targets.
struct WeaponReach {
    reach: (f64, f64),
    margin: f64,
}

/// Sound of a component field: an identifier, or an object with `sound_id`.
fn sound_name(v: &serde_json::Value) -> Option<String> {
    v.as_str().or_else(|| v["sound_id"].as_str()).map(crate::commands::args::id)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl Server {
    // ---------- Components of the held item ----------

    /// Compiled prototype of the stack's item.
    fn item_info(&self, s: &Stack) -> Option<&rc_data::ItemInfo> {
        let id = self.life.items.id(&s.id)?;
        self.data.items.get(id as usize)
    }

    /// The stack's patch for component `name`: `Some(None)` when the patch removes it,
    /// `Some(Some(json))` when it carries its own, `None` when the item's default applies. One walk
    /// of the patch without building the `!name` key: the charge asks every tick.
    fn patched_component(s: &Stack, name: &str) -> Option<Option<serde_json::Value>> {
        let Some(rc_proto::nbt::Nbt::Compound(entries)) = &s.components else { return None };
        entries.iter().find_map(|(key, value)| {
            if key == name {
                Some(Some(crate::commands::cmds_s9b::nbt_to_json(value)))
            } else {
                (key.strip_prefix('!') == Some(name)).then_some(None)
            }
        })
    }

    /// The stack's `kinetic_weapon` component. The item's default is read from its compiled
    /// prototype, without a copy: the charge reads it every tick.
    pub(crate) fn kinetic_weapon(&self, s: &Stack) -> Option<KineticParams> {
        match Self::patched_component(s, "minecraft:kinetic_weapon") {
            Some(patch) => patch.map(|k| KineticParams::from_component(&k)),
            None => self.item_info(s)?.kinetic_params,
        }
    }

    /// The stack's `attack_range` component: min, max, creative min and max reach, margin, mob factor.
    fn attack_range(&self, s: &Stack) -> Option<[f32; 6]> {
        match Self::patched_component(s, "minecraft:attack_range") {
            Some(patch) => patch.map(|r| rc_data::attack_range_from_component(&r)),
            None => self.item_info(s)?.attack_range,
        }
    }

    /// The stack's `minimum_attack_charge` (0 without).
    fn minimum_attack_charge(&self, s: &Stack) -> f32 {
        match Self::patched_component(s, "minecraft:minimum_attack_charge") {
            Some(patch) => patch.and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            None => self.item_info(s).map_or(0.0, |i| i.min_attack_charge),
        }
    }

    /// Does the stack carry a `piercing_weapon` component (its left click stabs)?
    pub(crate) fn has_piercing_weapon(&self, s: &Stack) -> bool {
        match Self::patched_component(s, "minecraft:piercing_weapon") {
            Some(patch) => patch.is_some(),
            None => self.item_info(s).is_some_and(|i| i.piercing),
        }
    }

    fn piercing_weapon(&self, s: &Stack) -> Option<StabSettings> {
        let v = self.comp_json(s, "minecraft:piercing_weapon")?;
        Some(StabSettings {
            deals_knockback: v["deals_knockback"].as_bool().unwrap_or(true),
            dismounts: v["dismounts"].as_bool().unwrap_or(false),
            sound: sound_name(&v["sound"]),
            hit_sound: sound_name(&v["hit_sound"]),
        })
    }

    /// Attack animation of the stack (kind 0 none, 1 whack, 2 stab; duration in ticks); a whack
    /// of 6 ticks without one.
    fn attack_animation(&self, s: Option<&Stack>) -> [u8; 2] {
        let Some(s) = s else { return [1, 6] };
        match Self::patched_component(s, "minecraft:attack_animation") {
            Some(patch) => {
                let v = patch.unwrap_or_default();
                let kind = match v["type"].as_str() {
                    Some("none") => 0,
                    Some("stab") => 2,
                    _ => 1,
                };
                [kind, v["duration"].as_u64().unwrap_or(6).min(255) as u8]
            }
            None => self.item_info(s).map_or([1, 6], |i| i.attack_anim),
        }
    }

    // ---------- Attack charge ----------

    /// Ticks of a full attack charge with the held item (20 / attack speed).
    fn attack_strength_delay(&self) -> f32 {
        (1.0 / self.player_attribute("minecraft:attack_speed") * 20.0) as f32
    }

    /// Attack charge of the player, 0 to 1, half a tick ahead like the game's hits.
    fn attack_strength_scale(&self) -> f32 {
        ((self.mobs.attack_ticker as f32 + 0.5) / self.attack_strength_delay()).clamp(0.0, 1.0)
    }

    /// The item cannot attack yet: its minimum attack charge is above the charge the player will
    /// have `tolerance` ticks from now.
    pub(crate) fn cannot_attack_with_item(&self, s: Option<&Stack>, tolerance: i32) -> bool {
        let required = s.map_or(0.0, |s| self.minimum_attack_charge(s));
        required > 0.0 && ((self.mobs.attack_ticker + tolerance) as f32 / self.attack_strength_delay()) < required
    }

    /// The attack packet is ignored: the main hand stabs instead (piercing weapon), or its item
    /// cannot attack yet.
    pub(crate) fn melee_attack_refused(&self) -> bool {
        let main = self.slots[36 + self.held as usize].as_ref();
        main.is_some_and(|s| self.has_piercing_weapon(s)) || self.cannot_attack_with_item(main, 5)
    }

    /// Base damage and enchantment bonus of a stab with the weapon of `hand`: both reduced by the
    /// attack charge (the base by 0.2 + 0.8 × charge², the bonus by the charge) unless that hand is
    /// the one in use (the charge of a held spear hits at full strength).
    pub(crate) fn stab_damage(&self, hand: i32, base: f32, bonus: f32) -> (f32, f32) {
        if self.life.vit.using.is_some_and(|(h, _)| h == hand) {
            return (base, bonus);
        }
        let charge = self.attack_strength_scale();
        (base * (0.2 + charge * charge * 0.8), bonus * charge)
    }

    /// The weapon's own knockback after the stab's 0.4: the attack knockback attribute plus the
    /// main hand's Knockback level, halved.
    pub(crate) fn stab_weapon_knockback(&self) -> f32 {
        let enchantment =
            self.slots[36 + self.held as usize].as_ref().map_or(0, |w| self.life.items.enchantment(w, "minecraft:knockback"));
        (self.player_attribute("minecraft:attack_knockback") as f32 + enchantment as f32) / 2.0
    }

    /// Wear of a stab that touched a living target (the weapon's `weapon` component).
    pub(crate) fn stab_weapon_wear(&mut self, slot: usize, weapon: Option<&Stack>) {
        if let Some(w) = weapon {
            let per = self.weapon_wear(w);
            if per > 0 {
                self.hurt_item(slot, per);
            }
        }
    }

    // ---------- Targets ----------

    /// Reach of the held weapon: its `attack_range` (the creative bounds in creative mode),
    /// otherwise the player's entity interaction range without a margin.
    fn player_weapon_reach(&self, weapon: Option<&Stack>) -> WeaponReach {
        match weapon.and_then(|w| self.attack_range(w)) {
            Some([min, max, creative_min, creative_max, margin, _]) => {
                let (lo, hi) = if self.game_mode == 1 { (creative_min, creative_max) } else { (min, max) };
                WeaponReach { reach: (f64::from(lo), f64::from(hi)), margin: f64::from(margin) }
            }
            None => {
                let range = self.player_attribute("minecraft:entity_interaction_range") as f32;
                WeaponReach { reach: (0.0, f64::from(range)), margin: 0.0 }
            }
        }
    }

    /// The vehicle the player rides without steering it: it moves the player.
    fn unsteered_vehicle(&self) -> Option<&Mob> {
        let v = self.mob_ref(self.mobs.mounts.riding.vehicle?)?;
        (!Self::player_controls(v)).then_some(v)
    }

    /// Movement of the player known to the server for the reach (that of a vehicle it does not
    /// steer), per tick.
    fn player_known_movement(&self) -> [f64; 3] {
        self.unsteered_vehicle().map_or(self.proj_player.known_move, |v| v.vel)
    }

    /// Speed of the player for the charge, blocks per second.
    fn player_kinetic_motion(&self) -> [f64; 3] {
        let v = self.unsteered_vehicle().map_or(self.proj_player.known_move, |v| v.pig.known_speed);
        v.map(|c| c * 20.0)
    }

    /// A mob the spear can hit: alive, not invulnerable, not riding with the player.
    fn spear_can_hit_mob(&self, o: &Mob) -> bool {
        o.alive() && !o.is_invulnerable() && !self.player_same_root(o)
    }

    /// Can the current player hurt the other player of seat `who` (`pvp` rule, friendly fire)?
    fn may_harm_other_player(&mut self, who: Option<usize>) -> bool {
        let me = self.name.clone();
        !self.as_player(who, |sv| sv.refuses_player_damage(&me))
    }

    /// Targets along the reach of the held weapon, from the player's eye along its gaze: mobs,
    /// then the other players, then the projectiles a hit sends back.
    fn player_spear_targets(&mut self, weapon: Option<&Stack>) -> Vec<SpearTarget> {
        let use_search_area = crate::mobs::piglins::spear::use_spear_search_area();
        self.player_spear_targets_with(weapon, use_search_area)
    }

    /// [`Self::player_spear_targets`], the search box used or not (the bench measures both).
    pub(crate) fn player_spear_targets_with(&mut self, weapon: Option<&Stack>, use_search_area: bool) -> Vec<SpearTarget> {
        let r = self.player_weapon_reach(weapon);
        let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
        let look = view_vector(self.pitch, self.yaw);
        let Some(seg) = self.weapon_reach_segment(eye, look, r.reach, self.player_known_movement()) else {
            return Vec::new();
        };
        let area = seg.prefilter(r.margin, use_search_area);
        let hits = |s: &Self, bb: &rc_physics::Aabb| {
            area.is_none_or(|a| a.intersects(bb)) && s.reach_segment_hits(&seg, bb, r.margin)
        };
        // The species bound of `touches` skips the far mobs before their box is even built, and
        // before the vehicle walk of a mounted charge.
        let mut out: Vec<SpearTarget> = self
            .mobs
            .list
            .iter()
            .filter(|o| {
                area.is_none_or(|a| o.touches(&a))
                    && self.spear_can_hit_mob(o)
                    && self.reach_segment_hits(&seg, &o.bbox(), r.margin)
            })
            .map(|o| SpearTarget::Mob(o.id))
            .collect();
        for p in self.lan_peers() {
            if p.game_mode != 3 && hits(self, &p.bbox) && self.may_harm_other_player(p.who) {
                out.push(SpearTarget::OtherPlayer(p.id));
            }
        }
        let projectiles = self.proj.list.iter().filter(|p| !p.removed && crate::mobs::fireballs::redirectable_by_hit(p));
        out.extend(projectiles.filter(|p| hits(self, &p.bbox())).map(|p| SpearTarget::Projectile(p.id)));
        out
    }

    /// Speed of a target for the charge, blocks per second.
    fn spear_target_motion(&mut self, t: SpearTarget) -> [f64; 3] {
        let per_tick = match t {
            SpearTarget::Mob(id) => self.mob_ref(id).map_or([0.0; 3], |o| o.pig.known_speed),
            SpearTarget::OtherPlayer(id) => match self.lan_player(id) {
                Some((who, ..)) => self.as_player(who, |sv| sv.proj_player.known_move),
                None => [0.0; 3],
            },
            SpearTarget::Projectile(id) => self.proj.list.iter().find(|p| p.id == id).map_or([0.0; 3], |p| p.vel),
        };
        per_tick.map(|c| c * 20.0)
    }

    // ---------- Stab ----------

    /// Stab of a left click (`player_action` 8): nothing for a spectator, nor with an item that
    /// cannot attack yet or carries no piercing weapon. Otherwise every target along the weapon's
    /// reach is stabbed with the player's attack damage; then the attack charge restarts, the
    /// weapon's after-stab enchantment effects run (Lunge), the hit sound plays for everyone when
    /// something was hit and the weapon's sound for the others, who also see the arm swing.
    pub(crate) fn player_stab(&mut self) {
        if self.game_mode == 3 {
            return;
        }
        let weapon = self.slots[36 + self.held as usize].clone();
        if self.cannot_attack_with_item(weapon.as_ref(), 5) {
            return;
        }
        let Some(piercing) = weapon.as_ref().and_then(|w| self.piercing_weapon(w)) else { return };
        let fx = StabEffects {
            damage: self.player_attribute("minecraft:attack_damage") as f32,
            deals_damage: true,
            deals_knockback: piercing.deals_knockback,
            dismounts: piercing.dismounts,
        };
        let mut hit_something = false;
        for target in self.player_spear_targets(weapon.as_ref()) {
            hit_something |= self.player_stab_target(target, MAIN_HAND, fx);
        }
        self.mobs.attack_ticker = 0;
        self.player_post_piercing_attack();
        let pos = self.pos;
        if hit_something && let Some(s) = &piercing.hit_sound {
            self.world_sound(s, PLAYERS, pos, 1.0, 1.0);
        }
        if let Some(s) = &piercing.sound {
            self.play_sound_for_others(s, PLAYERS, pos, 1.0, 1.0);
        }
        self.swing_main_hand_for_observers(weapon.as_ref());
    }

    /// One stab of the weapon of `hand` on `target`; true if it touched it.
    fn player_stab_target(&mut self, target: SpearTarget, hand: i32, fx: StabEffects) -> bool {
        match target {
            SpearTarget::Mob(id) => {
                let Some(i) = self.mob_index(id) else { return false };
                let Some(mut m) = self.mobs.list.take(i) else { return false };
                let touched = self.player_stab_mob(&mut m, hand, fx);
                self.mobs.list.put(i, m);
                touched
            }
            SpearTarget::OtherPlayer(id) => self.player_stab_other_player(id, hand, fx),
            // A projectile is only sent back by a stab that knocks back; nothing else touches it.
            SpearTarget::Projectile(id) => {
                let Some(i) = self.proj.list.iter().position(|p| p.id == id && !p.removed) else { return false };
                fx.deals_knockback && self.deflect_by_player_hit(i)
            }
        }
    }

    /// Stab of the weapon of `hand` on mob `m` (held out of the list): damage of the stab type,
    /// knockbacks, dismount; then, if any of them happened, the enchantment spark, the last
    /// target, the after-hit effects of a hurt, the weapon's wear, the statistics, the damage
    /// particles and the exhaustion.
    fn player_stab_mob(&mut self, m: &mut Mob, hand: i32, fx: StabEffects) -> bool {
        let slot = self.hand_slot(hand);
        let weapon = self.slots[slot].clone();
        // The enchantment bonus is read on the main hand's weapon, whatever the hand.
        let main = self.slots[36 + self.held as usize].clone();
        let bonus = main.as_ref().map_or(0.0, |w| self.enchanted_bonus(w, m));
        let (base, magic) = self.stab_damage(hand, fx.damage, bonus);
        let ty = self.weapon_damage_type(weapon.as_ref(), false, "minecraft:player_attack");
        let total = if fx.deals_damage { base + magic } else { 0.0 };
        let health_before = m.health;
        let hurt = fx.deals_damage && self.mob_hurt(m, &ty, total, Some(Ent::Player), Some(self.pos));
        if fx.deals_knockback {
            self.stab_knockback_mob(m, total);
        }
        let dismounted = fx.dismounts && self.dismount_by_item(m);
        if !hurt && !fx.deals_knockback && !dismounted {
            return false;
        }
        if magic > 0.0 {
            let _ = self.send(Event::Entity(EntityEvent::Animate { id: m.id, action: 2 }));
        }
        self.set_last_hurt_mob(Some(Ent::Player), (!self.mobs.end.part_attack).then_some(Ent::Mob(m.id)));
        if hurt {
            let mut hit = crate::ench_fx::Hit {
                ty: &ty,
                victim: crate::ench_fx::Side::Mob(m),
                attacker: Some(crate::ench_fx::Side::Player),
                direct: true,
            };
            self.post_attack(&mut hit, weapon.as_ref());
        }
        self.stab_weapon_wear(slot, weapon.as_ref());
        let dealt = health_before - m.health;
        self.s9_player_hit(dealt, weapon.as_ref());
        if dealt > 2.0 {
            let at = [m.pos()[0], m.pos()[1] + f64::from(m.height()) * 0.5, m.pos()[2]];
            self.particles("minecraft:damage_indicator", at, [0.1, 0.0, 0.1], 0.2, (dealt * 0.5) as i32);
        }
        let invulnerable = self.abilities().invulnerable;
        self.life.vit.exhaust(0.1, invulnerable);
        true
    }

    /// The two knockbacks of a stab on a mob, along the player's yaw: 0.4, then the weapon's own;
    /// each one that pushes stops the player's sprint.
    fn stab_knockback_mob(&mut self, m: &mut Mob, damage: f32) {
        let (xd, zd) = (f64::from(sin(self.yaw * DEG)), f64::from(-cos(self.yaw * DEG)));
        for (power, from_effect) in [(STAB_BASE_KNOCKBACK, false), (self.stab_weapon_knockback(), true)] {
            if power > 0.0 {
                self.mob_knockback(m, f64::from(power), xd, zd, Some(Ent::Player), damage, from_effect);
                self.life.vit.sprinting = false;
            }
        }
    }

    /// A stab takes mob `m` off its mob vehicle unless its kind resists it; true if it did.
    fn dismount_by_item(&mut self, m: &mut Mob) -> bool {
        let Some(vid) = m.mount.vehicle else { return false };
        if self.entity_in(m.kind, &[CANNOT_BE_DISMOUNTED.to_owned()]) {
            return false;
        }
        let Some(i) = self.mob_index(vid) else { return false };
        let Some(mut v) = self.mobs.list.take(i) else { return false };
        self.mob_dismount_from(m, &mut v);
        self.mobs.list.put(i, v);
        true
    }

    /// The main-hand swing of a stab, shown to the players who see the current player (its own
    /// client already swung), with the weapon's attack animation.
    fn swing_main_hand_for_observers(&self, weapon: Option<&Stack>) {
        if self.lan.is_none() {
            return;
        }
        let [kind, duration] = self.attack_animation(weapon);
        let swing = EntityEvent::Swing { id: self.player_id, hand: MAIN_HAND, kind: i32::from(kind), duration: i32::from(duration) };
        self.send_to_observers_of_current_player(Event::Entity(swing));
    }

    // ---------- Charge ----------

    /// Start of the charge (`use_item` with a kinetic weapon): a use held until release (the
    /// memory of the entities hit emptied when no use was running), and the weapon's sound for
    /// the others.
    pub(crate) fn start_kinetic_use(&mut self, hand: i32, s: &Stack) {
        if self.life.vit.using.is_none() {
            self.life.vit.kinetic_hits.clear();
        }
        self.start_using(hand, KINETIC_USE_TICKS);
        if let Some(sound) = self.comp_json(s, "minecraft:kinetic_weapon").and_then(|k| sound_name(&k["sound"])) {
            self.play_sound_for_others(&sound, PLAYERS, self.pos, 1.0, 1.0);
        }
    }

    /// One tick of the charge, before the use counts down (`left` ticks left). Past the weapon's
    /// delay, each target along the reach not hit within the weapon's contact cooldown is
    /// remembered, then stabbed when the speeds meet one of the weapon's conditions (dismount,
    /// knockback, damage): damage = the player's base attack damage + the relative speed × the
    /// weapon's multiplier. A stab that touched shows the hit feedback (entity event 2) and counts
    /// for `spear_mobs`.
    pub(crate) fn player_kinetic_tick(&mut self, hand: i32, left: i32, kin: &KineticParams) {
        let used = KINETIC_USE_TICKS - left - kin.delay;
        if used < 0 {
            return;
        }
        let weapon = self.slots[self.hand_slot(hand)].clone();
        let look = view_vector(self.pitch, self.yaw);
        let own_speed = dot(look, self.player_kinetic_motion());
        let base = self.base_attr("minecraft:attack_damage") as f32;
        let mut affected = false;
        for target in self.player_spear_targets(weapon.as_ref()) {
            if !self.remember_charge_hit(target, kin.contact_cooldown) {
                continue;
            }
            let relative = (own_speed - dot(look, self.spear_target_motion(target))).max(0.0);
            // A player's conditions take the speeds as they are (a mob's are scaled by 0.2).
            let met = |c: Option<(i32, f32, f32)>| {
                c.is_some_and(|(ticks, speed, rel)| used <= ticks && own_speed >= f64::from(speed) && relative >= f64::from(rel))
            };
            let [dismounts, deals_knockback, deals_damage] = kin.conditions.map(met);
            if dismounts || deals_knockback || deals_damage {
                // The game floors to an int before adding to the single-precision base.
                let damage = base + (relative * f64::from(kin.damage_multiplier)).floor() as i32 as f32;
                let fx = StabEffects { damage, deals_damage, deals_knockback, dismounts };
                affected |= self.player_stab_target(target, hand, fx);
            }
        }
        if affected {
            let _ = self.send(Event::Entity(EntityEvent::Event { id: self.player_id, event: 2 }));
            self.trigger_spear_mobs();
        }
    }

    /// Remembers that the charge hits `target` now; false (nothing remembered) when it was hit
    /// less than `cooldown` ticks ago.
    fn remember_charge_hit(&mut self, target: SpearTarget, cooldown: i32) -> bool {
        let now = self.time;
        let hits = &mut self.life.vit.kinetic_hits;
        match hits.iter_mut().find(|(t, _)| *t == target) {
            Some((_, at)) if now - *at < i64::from(cooldown) => false,
            Some((_, at)) => {
                *at = now;
                true
            }
            None => {
                hits.push((target, now));
                true
            }
        }
    }

    /// `spear_mobs`: the number of living entities the charge in progress has hit.
    fn trigger_spear_mobs(&mut self) {
        if !self.trig_active("minecraft:spear_mobs") {
            return;
        }
        let count = self.life.vit.kinetic_hits.iter().filter(|(t, _)| t.living()).count() as i64;
        self.trigger("minecraft:spear_mobs", &|_, c| c["count"].as_i64().is_none_or(|n| count >= n));
    }
}
