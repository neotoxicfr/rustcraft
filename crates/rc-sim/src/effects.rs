//! Status effects of the player (S7, `mechanics-s7.md` § 1): instances (duration, amplifier,
//! hidden longer-duration effect), adding and merging, tick (regeneration, poison, wither, hunger,
//! absorption, instant), attribute modifiers, `update_mob_effect` /
//! `remove_mob_effect` packets, particles and entity flags, `active_effects` save.

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;

use crate::nbtio::{int, list};
use crate::server::Server;

/// Category: beneficial, neutral, harmful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cat {
    Good,
    Neutral,
    Bad,
}

/// Attribute modifier of an effect: attribute, identifier, amount per level, operation
/// (0 add, 1 multiplied base, 2 multiplied total).
pub type Attr = (&'static str, &'static str, f64, i32);

pub struct Def {
    pub id: &'static str,
    pub cat: Cat,
    pub color: u32,
    pub attrs: &'static [Attr],
    pub instant: bool,
    /// Sound played on addition.
    pub sound: Option<&'static str>,
    /// Own particle (otherwise tinted `entity_effect`).
    pub particle: Option<&'static str>,
}

const fn d(id: &'static str, cat: Cat, color: u32) -> Def {
    Def { id, cat, color, attrs: &[], instant: false, sound: None, particle: None }
}

use Cat::{Bad, Good, Neutral};

pub static DEFS: &[Def] = &[
    Def {
        attrs: &[("minecraft:movement_speed", "minecraft:effect.speed", 0.2f32 as f64, 2)],
        ..d("minecraft:speed", Good, 3402751)
    },
    Def {
        attrs: &[("minecraft:movement_speed", "minecraft:effect.slowness", -0.15f32 as f64, 2)],
        ..d("minecraft:slowness", Bad, 9154528)
    },
    Def {
        attrs: &[("minecraft:attack_speed", "minecraft:effect.haste", 0.1f32 as f64, 2)],
        ..d("minecraft:haste", Good, 14270531)
    },
    Def {
        attrs: &[("minecraft:attack_speed", "minecraft:effect.mining_fatigue", -0.1f32 as f64, 2)],
        ..d("minecraft:mining_fatigue", Bad, 4866583)
    },
    Def {
        attrs: &[("minecraft:attack_damage", "minecraft:effect.strength", 3.0, 0)],
        ..d("minecraft:strength", Good, 16762624)
    },
    Def { instant: true, ..d("minecraft:instant_health", Good, 16262179) },
    Def { instant: true, ..d("minecraft:instant_damage", Bad, 11101546) },
    Def {
        attrs: &[("minecraft:safe_fall_distance", "minecraft:effect.jump_boost", 1.0, 0)],
        ..d("minecraft:jump_boost", Good, 16646020)
    },
    d("minecraft:nausea", Bad, 5578058),
    d("minecraft:regeneration", Good, 13458603),
    d("minecraft:resistance", Good, 9520880),
    d("minecraft:fire_resistance", Good, 16750848),
    d("minecraft:water_breathing", Good, 10017472),
    Def {
        attrs: &[("minecraft:waypoint_transmit_range", "minecraft:effect.waypoint_transmit_range_hide", -1.0, 2)],
        ..d("minecraft:invisibility", Good, 16185078)
    },
    d("minecraft:blindness", Bad, 2039587),
    d("minecraft:night_vision", Good, 12779366),
    d("minecraft:hunger", Bad, 5797459),
    Def {
        attrs: &[("minecraft:attack_damage", "minecraft:effect.weakness", -4.0, 0)],
        ..d("minecraft:weakness", Bad, 4738376)
    },
    d("minecraft:poison", Bad, 8889187),
    d("minecraft:wither", Bad, 7561558),
    Def {
        attrs: &[("minecraft:max_health", "minecraft:effect.health_boost", 4.0, 0)],
        ..d("minecraft:health_boost", Good, 16284963)
    },
    Def {
        attrs: &[("minecraft:max_absorption", "minecraft:effect.absorption", 4.0, 0)],
        ..d("minecraft:absorption", Good, 2445989)
    },
    Def { instant: true, ..d("minecraft:saturation", Good, 16262179) },
    d("minecraft:glowing", Neutral, 9740385),
    d("minecraft:levitation", Bad, 13565951),
    Def { attrs: &[("minecraft:luck", "minecraft:effect.luck", 1.0, 0)], ..d("minecraft:luck", Good, 5882118) },
    Def { attrs: &[("minecraft:luck", "minecraft:effect.unluck", -1.0, 0)], ..d("minecraft:unluck", Bad, 12624973) },
    d("minecraft:slow_falling", Good, 15978425),
    d("minecraft:conduit_power", Good, 1950417),
    d("minecraft:dolphins_grace", Good, 8954814),
    Def { sound: Some("minecraft:event.mob_effect.bad_omen"), ..d("minecraft:bad_omen", Neutral, 745784) },
    d("minecraft:hero_of_the_village", Good, 4521796),
    d("minecraft:darkness", Bad, 2696993),
    Def {
        sound: Some("minecraft:event.mob_effect.trial_omen"),
        particle: Some("minecraft:trial_omen"),
        ..d("minecraft:trial_omen", Neutral, 1484454)
    },
    Def {
        sound: Some("minecraft:event.mob_effect.raid_omen"),
        particle: Some("minecraft:raid_omen"),
        ..d("minecraft:raid_omen", Neutral, 14565464)
    },
    // 1.21 effects: own particle (no tint), hooks in `mobs/effects.rs`.
    Def { particle: Some("minecraft:small_gust"), ..d("minecraft:wind_charged", Bad, 12438015) },
    Def { particle: Some("minecraft:item_cobweb"), ..d("minecraft:weaving", Bad, 7891290) },
    Def { particle: Some("minecraft:item_slime"), ..d("minecraft:oozing", Bad, 10092451) },
    Def { particle: Some("minecraft:infested"), ..d("minecraft:infested", Bad, 9214860) },
    d("minecraft:breath_of_the_nautilus", Good, 65518),
];

pub fn def(id: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.id == id)
}

/// Base values of the player's attributes: those specific to the player, otherwise the default of
/// the attribute (`attrs::ATTRS`).
pub fn player_base(attr: &str) -> f64 {
    player_base_or(attr, crate::attrs::def(attr).map_or(0.0, |d| d.default))
}

/// Player-specific base, otherwise `default`.
pub fn player_base_or(attr: &str, default: f64) -> f64 {
    match attr {
        "minecraft:movement_speed" => f64::from(0.1f32),
        "minecraft:attack_damage" => 1.0,
        "minecraft:waypoint_transmit_range" | "minecraft:waypoint_receive_range" => 6.0e7,
        _ => default,
    }
}

/// Effect instance.
#[derive(Debug, Clone, PartialEq)]
pub struct Inst {
    pub id: String,
    pub amp: i32,
    /// Remaining ticks, −1 infinite.
    pub dur: i32,
    pub ambient: bool,
    pub visible: bool,
    pub icon: bool,
    pub hidden: Option<Box<Inst>>,
}

impl Inst {
    pub fn new(id: &str, dur: i32, amp: i32) -> Inst {
        Inst { id: id.to_owned(), amp: amp.clamp(0, 255), dur, ambient: false, visible: true, icon: true, hidden: None }
    }

    pub fn infinite(&self) -> bool {
        self.dur == -1
    }

    fn shorter_than(&self, o: &Inst) -> bool {
        !self.infinite() && (self.dur < o.dur || o.infinite())
    }

    fn set_details(&mut self, o: &Inst) {
        self.dur = o.dur;
        self.amp = o.amp;
        self.ambient = o.ambient;
        self.visible = o.visible;
        self.icon = o.icon;
    }

    /// Merge of a new instance of the same effect (`update`); true if changed.
    pub fn update(&mut self, o: &Inst) -> bool {
        let mut changed = false;
        if o.amp > self.amp {
            if o.shorter_than(self) {
                let prev = self.hidden.take();
                let mut h = self.clone();
                h.hidden = prev;
                self.hidden = Some(Box::new(h));
            }
            self.amp = o.amp;
            self.dur = o.dur;
            changed = true;
        } else if self.shorter_than(o) {
            if o.amp == self.amp {
                self.dur = o.dur;
                changed = true;
            } else {
                match &mut self.hidden {
                    None => {
                        let mut h = o.clone();
                        h.hidden = None;
                        self.hidden = Some(Box::new(h));
                    }
                    Some(h) => {
                        h.update(o);
                    }
                }
            }
        }
        if (!o.ambient && self.ambient) || changed {
            self.ambient = o.ambient;
            changed = true;
        }
        if o.visible != self.visible {
            self.visible = o.visible;
            changed = true;
        }
        if o.icon != self.icon {
            self.icon = o.icon;
            changed = true;
        }
        changed
    }

    pub(crate) fn remaining(&self) -> bool {
        self.infinite() || self.dur > 0
    }

    pub(crate) fn tick_down(&mut self) {
        if let Some(h) = &mut self.hidden {
            h.tick_down();
        }
        if !self.infinite() && self.dur != 0 {
            self.dur -= 1;
        }
    }

    pub(crate) fn downgrade(&mut self) -> bool {
        if self.dur == 0
            && let Some(h) = self.hidden.take()
        {
            self.set_details(&h);
            self.hidden = h.hidden;
            return true;
        }
        false
    }

    /// Scaled duration (lingering potion, arrow): at least 1 tick.
    pub fn scaled(&self, scale: f32) -> Inst {
        let mut c = self.clone();
        if !c.infinite() && c.dur != 0 {
            c.dur = ((c.dur as f32 * scale).floor() as i32).max(1);
        }
        c
    }

    fn details_nbt(&self) -> Vec<(String, Nbt)> {
        let mut f = vec![
            ("amplifier".into(), Nbt::Byte(self.amp as u8 as i8)),
            ("duration".into(), Nbt::Int(self.dur)),
            ("ambient".into(), Nbt::Byte(i8::from(self.ambient))),
            ("show_particles".into(), Nbt::Byte(i8::from(self.visible))),
            ("show_icon".into(), Nbt::Byte(i8::from(self.icon))),
        ];
        if let Some(h) = &self.hidden {
            f.push(("hidden_effect".into(), Nbt::Compound(h.details_nbt())));
        }
        f
    }

    pub fn to_nbt(&self) -> Nbt {
        let mut f = vec![("id".into(), Nbt::String(self.id.clone()))];
        f.extend(self.details_nbt());
        Nbt::Compound(f)
    }

    fn details_from(id: &str, n: &Nbt, depth: u32) -> Inst {
        let flag = |k: &str, d: bool| int(n, k).map_or(d, |v| v != 0);
        let visible = flag("show_particles", true);
        Inst {
            id: id.to_owned(),
            amp: int(n, "amplifier").map_or(0, |v| (v as i32) & 0xFF),
            dur: int(n, "duration").unwrap_or(0) as i32,
            ambient: flag("ambient", false),
            visible,
            icon: flag("show_icon", visible),
            hidden: n
                .get("hidden_effect")
                .filter(|_| depth < 16)
                .map(|h| Box::new(Inst::details_from(id, h, depth + 1))),
        }
    }

    pub fn from_nbt(n: &Nbt) -> Option<Inst> {
        let id = n.get("id")?.as_str()?;
        def(id)?;
        Some(Inst::details_from(id, n, 0))
    }

    /// Instance described in JSON (`consumable`, `potion_contents` components of the reports).
    pub fn from_json(v: &serde_json::Value) -> Option<Inst> {
        let id = v["id"].as_str()?;
        def(id)?;
        let visible = v["show_particles"].as_bool().unwrap_or(true);
        Some(Inst {
            id: id.to_owned(),
            amp: v["amplifier"].as_i64().unwrap_or(0).clamp(0, 255) as i32,
            dur: v["duration"].as_i64().unwrap_or(0) as i32,
            ambient: v["ambient"].as_bool().unwrap_or(false),
            visible,
            icon: v["show_icon"].as_bool().unwrap_or(visible),
            hidden: None,
        })
    }
}

/// Effects of the player.
#[derive(Debug, Clone, Default)]
pub struct Effects {
    pub list: Vec<Inst>,
    pub absorption: f32,
    /// Particles and invisibility to send back ("effects modified" indicator).
    pub dirty: bool,
    sent_absorption: Option<f32>,
    sent_flags: Option<i8>,
    /// Forced resend of the flags at the next tick (even unchanged: gliding refusal).
    pub resend_flags: bool,
    sent_particles: Option<(Vec<(i32, u32)>, bool)>,
}

impl Effects {
    pub fn read(p: &Nbt) -> Effects {
        let list = list(p.get("active_effects")).iter().filter_map(Inst::from_nbt).collect();
        let absorption = crate::nbtio::float(p, "AbsorptionAmount").unwrap_or(0.0);
        Effects { list, absorption, dirty: true, ..Default::default() }
    }

    pub fn write(&self, root: &mut Nbt) {
        if self.list.is_empty() {
            crate::nbtio::remove(root, "active_effects");
        } else {
            crate::nbtio::set(root, "active_effects", Nbt::List(self.list.iter().map(Inst::to_nbt).collect()));
        }
        crate::nbtio::set(root, "AbsorptionAmount", Nbt::Float(self.absorption));
    }

    pub fn get(&self, id: &str) -> Option<&Inst> {
        self.list.iter().find(|e| e.id == id)
    }

    pub fn has(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    /// Amplifier of the effect, `None` absent.
    pub fn amp(&self, id: &str) -> Option<i32> {
        self.get(id).map(|e| e.amp)
    }

    /// Modifiers of the effects on an attribute: (additions, base multiplier, total multiplier).
    pub fn modifiers(&self, attr: &str) -> Vec<(String, f64, i32)> {
        let mut out = Vec::new();
        for e in &self.list {
            let Some(d) = def(&e.id) else { continue };
            for (a, id, amount, op) in d.attrs {
                if *a == attr {
                    out.push(((*id).to_owned(), amount * f64::from(e.amp + 1), *op));
                }
            }
        }
        out
    }

    pub fn forget_sent(&mut self) {
        self.sent_absorption = None;
        self.sent_flags = None;
        self.sent_particles = None;
        self.dirty = true;
    }
}

/// Value of an attribute: base, then additions, multiplications of the base, of the total.
pub fn attribute_value(base: f64, mods: &[(String, f64, i32)]) -> f64 {
    let mut b = base;
    for (_, a, op) in mods {
        if *op == 0 {
            b += a;
        }
    }
    let mut v = b;
    for (_, a, op) in mods {
        if *op == 1 {
            v += b * a;
        }
    }
    for (_, a, op) in mods {
        if *op == 2 {
            v *= 1.0 + a;
        }
    }
    v
}

impl Server {
    pub(crate) fn fx(&self) -> &Effects {
        &self.life.fx
    }

    /// Maximum health (`max_health` attribute: base and modifiers of the player, effects).
    pub(crate) fn max_health(&self) -> f32 {
        let base = self.base_attr("minecraft:max_health");
        attribute_value(base, &self.player_mods("minecraft:max_health")).clamp(1.0, 1024.0) as f32
    }

    /// Registered modifiers of a player attribute (set by `/attribute`).
    pub(crate) fn stored_mods(&self, name: &str) -> Vec<(String, f64, i32)> {
        list(self.pdata.root.get("attributes"))
            .iter()
            .find(|a| a.get("id").and_then(Nbt::as_str) == Some(name))
            .map(|a| {
                list(a.get("modifiers"))
                    .iter()
                    .filter_map(|m| {
                        let id = m.get("id")?.as_str()?.to_owned();
                        let amount = m.get("amount").and_then(crate::mobs::mob::f64_of).unwrap_or(0.0);
                        let op = match m.get("operation").and_then(Nbt::as_str) {
                            Some("add_multiplied_base") => 1,
                            Some("add_multiplied_total") => 2,
                            _ => 0,
                        };
                        Some((id, amount, op))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Modifiers of a player attribute: registered, creative mode, worn equipment
    /// (W8a, `attrs.rs`), then effects.
    pub(crate) fn player_mods(&self, name: &str) -> Vec<(String, f64, i32)> {
        let mut m = self.stored_mods(name);
        m.extend(self.creative_mods(name));
        m.extend(self.equip_mods(name));
        m.extend(self.life.fx.modifiers(name));
        m
    }

    fn max_absorption(&self) -> f32 {
        attribute_value(self.base_attr("minecraft:max_absorption"), &self.player_mods("minecraft:max_absorption"))
            .clamp(0.0, 2048.0) as f32
    }

    /// Fall distance without damage (base 3, `/attribute`, improved jump).
    pub(crate) fn safe_fall_distance(&self) -> f64 {
        let base = self.base_attr("minecraft:safe_fall_distance");
        attribute_value(base, &self.player_mods("minecraft:safe_fall_distance")).clamp(-1024.0, 1024.0)
    }

    /// Luck (chest loot).
    pub(crate) fn luck(&self) -> f32 {
        let base = self.base_attr("minecraft:luck");
        attribute_value(base, &self.player_mods("minecraft:luck")).clamp(-1024.0, 1024.0) as f32
    }

    /// Base of an attribute: player file, otherwise default value.
    pub(crate) fn base_attr(&self, name: &str) -> f64 {
        for a in list(self.pdata.root.get("attributes")) {
            if a.get("id").and_then(Nbt::as_str) == Some(name) {
                return match a.get("base") {
                    Some(Nbt::Double(d)) => *d,
                    Some(o) => o.as_i64().unwrap_or(0) as f64,
                    None => player_base(name),
                };
            }
        }
        player_base(name)
    }

    fn effect_index(&self, id: &str) -> Option<i32> {
        self.data.registry("minecraft:mob_effect")?.entries.iter().position(|e| e == id).map(|i| i as i32)
    }

    fn send_effect(&self, e: &Inst, blend: bool) {
        let Some(effect) = self.effect_index(&e.id) else { return };
        let flags = i8::from(e.ambient) | (i8::from(e.visible) << 1) | (i8::from(e.icon) << 2) | (i8::from(blend) << 3);
        let _ = self.send(Event::Entity(EntityEvent::Effect {
            id: self.player_id,
            effect,
            amplifier: e.amp,
            duration: e.dur,
            flags,
        }));
    }

    /// All effects (entering the game, dimension change, respawn).
    pub(crate) fn send_all_effects(&mut self) {
        for e in self.life.fx.list.clone() {
            self.send_effect(&e, false);
        }
        self.life.fx.forget_sent();
    }

    /// Adding an effect without a source; true if the state changed.
    pub(crate) fn add_effect(&mut self, e: Inst) -> bool {
        self.add_effect_from(e, None)
    }

    /// Adding an effect by a source (mob); an addition or update that changes
    /// the state triggers `effects_changed` with this source.
    pub(crate) fn add_effect_from(&mut self, e: Inst, source: Option<crate::progress::pred::Who<'_>>) -> bool {
        let Some(def) = def(&e.id) else { return false };
        let changed;
        match self.life.fx.list.iter().position(|x| x.id == e.id) {
            None => {
                self.life.fx.list.push(e.clone());
                self.life.fx.dirty = true;
                self.send_effect(&e, true);
                self.attributes_changed();
                if let Some(s) = def.sound {
                    self.play_sound(s, 7, self.pos, 1.0, 1.0);
                }
                changed = true;
            }
            Some(i) => {
                let mut cur = self.life.fx.list[i].clone();
                changed = cur.update(&e);
                self.life.fx.list[i] = cur.clone();
                if changed {
                    self.life.fx.dirty = true;
                    self.attributes_changed();
                    self.send_effect(&cur, false);
                }
            }
        }
        // Absorption fills its reserve.
        if e.id == "minecraft:absorption" {
            self.life.fx.absorption = self.life.fx.absorption.max(4.0 * (1 + e.amp) as f32);
        }
        if changed {
            self.trig_effects_changed(source);
        }
        changed
    }

    pub(crate) fn remove_effect(&mut self, id: &str) -> bool {
        let Some(i) = self.life.fx.list.iter().position(|x| x.id == id) else { return false };
        let e = self.life.fx.list.remove(i);
        self.effects_removed(&[e]);
        true
    }

    /// Milk: all effects removed.
    pub(crate) fn clear_effects(&mut self) -> bool {
        if self.life.fx.list.is_empty() {
            return false;
        }
        let all = std::mem::take(&mut self.life.fx.list);
        self.effects_removed(&all);
        true
    }

    fn effects_removed(&mut self, list: &[Inst]) {
        self.life.fx.dirty = true;
        for e in list {
            if let Some(effect) = self.effect_index(&e.id) {
                let _ = self.send(Event::Entity(EntityEvent::RemoveEffect { id: self.player_id, effect }));
            }
        }
        self.attributes_changed();
        self.trig_effects_changed(None);
    }

    /// Attributes recomputed: health and absorption capped, attributes sent back.
    fn attributes_changed(&mut self) {
        let max = self.max_health();
        if self.life.vit.health > max {
            self.life.vit.health = max;
        }
        let ma = self.max_absorption();
        if self.life.fx.absorption > ma {
            self.life.fx.absorption = ma;
        }
        self.send_attributes();
    }

    /// Healing (`heal`): only if alive, capped at maximum health.
    pub(crate) fn heal(&mut self, amount: f32) {
        let h = self.life.vit.health;
        if h > 0.0 {
            self.life.vit.health = (h + amount).clamp(0.0, self.max_health());
        }
    }

    /// Effect of a tick; false if the effect must stop.
    fn apply_effect_tick(&mut self, id: &str, amp: i32) -> bool {
        match id {
            "minecraft:regeneration" => {
                if self.life.vit.health < self.max_health() {
                    self.heal(1.0);
                }
            }
            "minecraft:poison" => {
                if self.life.vit.health > 1.0 {
                    self.hurt("minecraft:magic", 1.0);
                }
            }
            "minecraft:wither" => {
                self.hurt("minecraft:wither", 1.0);
            }
            "minecraft:hunger" => {
                let invul = self.abilities().invulnerable;
                self.life.vit.exhaust(0.005 * (amp + 1) as f32, invul);
            }
            "minecraft:absorption" => return self.life.fx.absorption > 0.0,
            // F07: omens (raid at the village).
            "minecraft:bad_omen" => return self.bad_omen_tick(amp),
            "minecraft:raid_omen" => return self.raid_omen_tick(),
            "minecraft:saturation" => {
                let n = amp + 1;
                let v = &mut self.life.vit;
                v.food = (v.food + n).clamp(0, 20);
                v.saturation = (v.saturation + n as f32 * 2.0).clamp(0.0, v.food as f32);
            }
            "minecraft:instant_health" => self.heal((4i32 << amp.min(30)).max(0) as f32),
            "minecraft:instant_damage" => {
                self.hurt("minecraft:magic", (6i32 << amp.min(30)) as f32);
            }
            _ => {}
        }
        true
    }

    pub(crate) fn should_apply(id: &str, tick_count: i32, amp: i32) -> bool {
        let every = |base: i32| {
            let i = base >> amp.min(31);
            i <= 0 || tick_count % i == 0
        };
        match id {
            "minecraft:regeneration" => every(50),
            "minecraft:poison" => every(25),
            "minecraft:wither" => every(40),
            "minecraft:hunger" | "minecraft:absorption" | "minecraft:bad_omen" => true,
            // F07: the raid omen acts on the last tick.
            "minecraft:raid_omen" => tick_count == 1,
            "minecraft:instant_health" | "minecraft:instant_damage" | "minecraft:saturation" => tick_count >= 1,
            _ => false,
        }
    }

    /// Instant effect applied directly (drunk potion, cloud) with a scale.
    pub(crate) fn apply_instant(&mut self, id: &str, amp: i32, scale: f64) {
        match id {
            "minecraft:instant_health" => {
                let n = (scale * f64::from(4i32 << amp.min(30)) + 0.5) as i32;
                self.heal(n as f32);
            }
            "minecraft:instant_damage" => {
                let n = (scale * f64::from(6i32 << amp.min(30)) + 0.5) as i32;
                self.hurt("minecraft:magic", n as f32);
            }
            _ => {
                self.apply_effect_tick(id, amp);
            }
        }
    }

    /// Effects tick (end of the player's base tick).
    pub(crate) fn effects_tick(&mut self) {
        if self.life.vit.dead {
            return;
        }
        let ids: Vec<String> = self.life.fx.list.iter().map(|e| e.id.clone()).collect();
        for id in ids {
            let Some(mut e) = self.life.fx.get(&id).cloned() else { continue };
            let keep = if !e.remaining() {
                false
            } else {
                let tc = if e.infinite() { self.life.vit.age as i32 } else { e.dur };
                if Self::should_apply(&e.id, tc, e.amp) && !self.apply_effect_tick(&e.id, e.amp) {
                    false
                } else {
                    // The effect may have changed during its tick (absorption removed…): we take up
                    // the current state again.
                    if let Some(c) = self.life.fx.get(&id) {
                        e = c.clone();
                    }
                    e.tick_down();
                    let down = e.downgrade();
                    if let Some(c) = self.life.fx.list.iter_mut().find(|x| x.id == id) {
                        *c = e.clone();
                    }
                    if down {
                        self.life.fx.dirty = true;
                        self.attributes_changed();
                        self.send_effect(&e, false);
                        self.trig_effects_changed(None);
                    }
                    e.remaining()
                }
            };
            if !keep {
                if let Some(i) = self.life.fx.list.iter().position(|x| x.id == id) {
                    let r = self.life.fx.list.remove(i);
                    self.effects_removed(&[r]);
                }
            } else if e.dur % 600 == 0 {
                self.life.fx.dirty = true;
                self.send_effect(&e, false);
                self.trig_effects_changed(None);
            }
        }
        // Slow falling, levitation: fall distance reset to zero.
        if self.life.fx.has("minecraft:slow_falling") || self.life.fx.has("minecraft:levitation") {
            self.life.vit.fall = 0.0;
        }
        self.sync_effect_data();
    }

    /// Player entity data: effect particles (10, 11) and absorption (17).
    pub(crate) fn sync_effect_data(&mut self) {
        use rc_proto_v777::entity::{Meta, Particle};
        let mut values = Vec::new();
        if self.life.fx.dirty {
            self.life.fx.dirty = false;
            let kind = |n: &str| {
                self.data
                    .registry("minecraft:particle_type")
                    .and_then(|r| r.entries.iter().position(|e| e == n))
                    .map_or(0, |i| i as i32)
            };
            let mut colors = Vec::new();
            let mut parts = Vec::new();
            for e in self.life.fx.list.iter().filter(|e| e.visible) {
                let Some(d) = def(&e.id) else { continue };
                let (k, color) = match d.particle {
                    Some(p) => (kind(p), 0),
                    None => {
                        let alpha: u32 = if e.ambient { 38 } else { 255 };
                        (kind("minecraft:entity_effect"), (alpha << 24) | (d.color & 0xFF_FFFF))
                    }
                };
                colors.push((k, color));
                parts.push(Particle { kind: k, color, state: 0, item: -1, scale: 1.0, color2: 0, vib: None });
            }
            let ambient = self.life.fx.list.iter().all(|e| !e.visible || e.ambient);
            let key = (colors, ambient);
            if self.life.fx.sent_particles.as_ref() != Some(&key) {
                self.life.fx.sent_particles = Some(key);
                values.push((10, Meta::Particles(parts)));
                values.push((11, Meta::Bool(ambient)));
            }
        }
        let a = self.life.fx.absorption;
        if self.life.fx.sent_absorption != Some(a) {
            self.life.fx.sent_absorption = Some(a);
            values.push((17, Meta::Float(a)));
        }
        if !values.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Data { id: self.player_id, values }));
        }
    }

    /// Particles of a list of effects (data 10 and 11 of a living entity): visible effects
    /// only, ambient if all the visible ones are "ambient". Used for mobs.
    pub(crate) fn effect_particles(&self, list: &[Inst]) -> (Vec<rc_proto_v777::entity::Particle>, bool) {
        use rc_proto_v777::entity::Particle;
        let kind = |n: &str| {
            self.data
                .registry("minecraft:particle_type")
                .and_then(|r| r.entries.iter().position(|e| e == n))
                .map_or(0, |i| i as i32)
        };
        let mut parts = Vec::new();
        for e in list.iter().filter(|e| e.visible) {
            let Some(d) = def(&e.id) else { continue };
            let (k, color) = match d.particle {
                Some(p) => (kind(p), 0),
                None => {
                    let alpha: u32 = if e.ambient { 38 } else { 255 };
                    (kind("minecraft:entity_effect"), (alpha << 24) | (d.color & 0xFF_FFFF))
                }
            };
            parts.push(Particle { kind: k, color, state: 0, item: -1, scale: 1.0, color2: 0, vib: None });
        }
        (parts, list.iter().all(|e| !e.visible || e.ambient))
    }

    /// Shared flags of the player entity (fire, sneaking, invisible, glowing).
    pub(crate) fn send_shared_flags(&mut self, burning: bool) {
        let mut flags = i8::from(burning) | if self.sneaking { 2 } else { 0 };
        if self.life.fx.has("minecraft:invisibility") {
            flags |= 0x20;
        }
        if self.life.fx.has("minecraft:glowing") {
            flags |= 0x40;
        }
        // Gliding (flag 7): the client relies on it to stop gliding on landing, and
        // a refused start sends back the current state (`resend_flags`).
        if self.proj_player.fall_flying {
            flags |= i8::MIN;
        }
        let forced = std::mem::take(&mut self.life.fx.resend_flags);
        if !forced && self.life.fx.sent_flags == Some(flags) {
            return;
        }
        // First send with nothing lit: the client already starts from zero.
        if !forced && self.life.fx.sent_flags.is_none() && flags & !2 == 0 {
            self.life.fx.sent_flags = Some(flags);
            return;
        }
        self.life.fx.sent_flags = Some(flags);
        let _ = self.send(Event::Entity(EntityEvent::Data {
            id: self.player_id,
            values: vec![(0, rc_proto_v777::entity::Meta::Byte(flags))],
        }));
    }

    /// Resistance then absorption reserve (the actual application of damage).
    pub(crate) fn resistance_reduce(&self, ty_bypasses_resistance: bool, d: f32) -> f32 {
        match self.life.fx.amp("minecraft:resistance") {
            Some(a) if !ty_bypasses_resistance => {
                let absorb = 25 - (a + 1) * 5;
                (d * absorb as f32 / 25.0).max(0.0)
            }
            _ => d,
        }
    }

    pub(crate) fn absorb(&mut self, d: f32) -> f32 {
        let a = self.life.fx.absorption;
        let rest = (d - a).max(0.0);
        self.life.fx.absorption = a - (d - rest);
        rest
    }

    /// Water breathing by effect (respiration, conduit, nautilus shell breath).
    pub(crate) fn water_breathing(&self) -> bool {
        let f = &self.life.fx;
        f.has("minecraft:water_breathing")
            || f.has("minecraft:conduit_power")
            || f.has("minecraft:breath_of_the_nautilus")
    }

    /// Turtle helmet worn (slot designated by its `equippable` component), eyes out of
    /// the water: water breathing of 10 s without particles, with icon, re-applied at each tick
    /// (the client thus receives the update at each tick, as with the game).
    pub(crate) fn turtle_helmet_tick(&mut self) {
        // Slot of the component read without copy: component of the stack, otherwise default of the item
        // (unless the component is removed).
        let equips_to = |s: &crate::player::Stack, slot: &str| -> bool {
            if let Some(n) = crate::items::component(s, "minecraft:equippable") {
                return n.get("slot").and_then(Nbt::as_str) == Some(slot);
            }
            crate::items::component(s, "!minecraft:equippable").is_none()
                && self.life.items.defaults(&s.id)["minecraft:equippable"]["slot"].as_str() == Some(slot)
        };
        let worn = crate::attrs::SLOTS.iter().enumerate().any(|(k, slot)| {
            self.equip_stack(k).is_some_and(|s| s.id == "minecraft:turtle_helmet" && equips_to(s, slot))
        });
        if worn && !self.eye_in_water() {
            let e = Inst { visible: false, ..Inst::new("minecraft:water_breathing", 200, 0) };
            self.add_effect(e);
        }
    }

    /// Underwater without drowning: air rises except with the nautilus shell breath alone.
    pub(crate) fn effects_refill_air(&self) -> bool {
        let f = &self.life.fx;
        !f.has("minecraft:breath_of_the_nautilus")
            || f.has("minecraft:water_breathing")
            || f.has("minecraft:conduit_power")
    }

    /// Player eyes in water, game rule: in the feet column, a water cell
    /// (source, flowing, bubble column, waterlogged block) whose bottom is below the eye and whose
    /// height as seen by the camera (1 for a source under a full face, otherwise fluid height)
    /// reaches the eye; eye height according to the pose.
    pub(crate) fn eye_in_water(&self) -> bool {
        let eye = self.player_eye_y();
        let (x, z) = (self.pos[0].floor() as i32, self.pos[2].floor() as i32);
        let box_min = self.pos[1] + 0.001;
        // The eye touches at most two cells: its own, and the one below if it is exactly on
        // a boundary (height 1 reached).
        let top = eye.floor() as i32;
        [top - 1, top].into_iter().any(|y| {
            let f = self.fluid_at([x, y, z]);
            if f.kind != 1 || !self.is_loaded([x, y, z]) {
                return false;
            }
            let bottom = f64::from(y);
            if bottom + self.fluid_height_at([x, y, z]) < box_min || eye < bottom {
                return false;
            }
            let above = [x, y + 1, z];
            let cam = if f.source && self.v().sturdy(self.get(above), crate::blocks::Dir::Down) {
                1.0
            } else {
                self.fluid_height_at([x, y, z])
            };
            eye <= bottom + cam
        })
    }

    /// Mining speed factor of effects and enchantments applied to
    /// the tool speed.
    pub(crate) fn dig_speed(&self, tool_speed: f32, eye_in_water: bool) -> f32 {
        // W8a: efficiency, break speed, aquatic affinity read from the attributes
        // (tracked equipment, `/attribute`), like the destroy speed.
        let mut s = tool_speed;
        if s > 1.0 {
            s += self.player_attribute("minecraft:mining_efficiency") as f32;
        }
        let haste = self.life.fx.amp("minecraft:haste");
        let conduit = self.life.fx.amp("minecraft:conduit_power");
        if haste.is_some() || conduit.is_some() {
            let a = haste.unwrap_or(0).max(conduit.unwrap_or(0));
            s *= 1.0 + (a + 1) as f32 * 0.2;
        }
        if let Some(a) = self.life.fx.amp("minecraft:mining_fatigue") {
            s *= 0.3f64.powi(a + 1) as f32;
        }
        s *= self.player_attribute("minecraft:block_break_speed") as f32;
        if eye_in_water {
            s *= self.player_attribute("minecraft:submerged_mining_speed") as f32;
        }
        s
    }
}
