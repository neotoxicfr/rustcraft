//! Local effects of entity events (`entity_event`, `damage_event`): particles, sounds and
//! pushes that the official client produces itself on receipt, and that the server never
//! sends. The animation states that follow from them (attack, grazing, fuse…) stay in
//! `entities.rs`; the totem's full-screen animation in `totem_anim.rs`.
//!
//! Sound rule: a sound "played by the entity" on the client side is audible only to the local
//! player (the server excludes them from its broadcast); "local" sounds are played for any
//! entity. Only those are reproduced here.

use hashbrown::HashMap;

use rc_audio::{Category, SoundHandle};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;
use rc_proto_v777::item::ItemStack;

use crate::particles::{Kind as PKind, Options as POptions};

/// Following sound of an entity (the game's "self-updating" instance).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Follow {
    /// Guardian attack: volume and pitch depending on the beam charge, without attenuation.
    Guardian,
    /// Sniffer digging: while it searches or digs.
    Sniffer,
}

/// State of the entity event effects.
#[derive(Default)]
pub struct EventFx {
    /// Player injury sound by damage type (network index of the `damage_type` registry).
    damage_sounds: Vec<&'static str>,
    /// Sounds that follow an entity.
    follows: Vec<(SoundHandle, i32, Follow)>,
    /// Last sound feedback of a kinetic hit (spear) per entity: game time.
    kinetic: HashMap<i32, i64>,
    /// Full-screen animation of the activated item (totem).
    pub totem: crate::totem_anim::ItemActivation,
}

impl EventFx {
    /// Ticks (fraction `pt` included) since the entity's last kinetic hit feedback, at
    /// game time `now`; 0 if there never was one (first-person spear pose).
    pub fn kinetic_since(&self, id: i32, now: i64, pt: f32) -> f32 {
        self.kinetic.get(&id).map_or(0.0, |&t| (now - t) as f32 + pt)
    }
}

/// Box and orientation of an entity at the time of the event.
struct Body {
    pos: [f64; 3],
    pos_o: [f64; 3],
    width: f64,
    height: f64,
    eye: f64,
    yaw: f32,
    pitch: f32,
    silent: bool,
    kind: String,
    category: Category,
}

// Player injury sound according to the damage type's effect (table shared with the integrated server).
use rc_client::player_hurt_sound as hurt_sound;

/// Sound category of an entity (the game's sound source): players, monsters, neutral.
fn category(kind: &str, e: Option<&crate::entities::Entity>) -> Category {
    match kind {
        "player" => Category::Player,
        // Killer bunny (variant 99, data 18).
        "rabbit" if e.and_then(|e| e.meta_int(18)) == Some(99) => Category::Hostile,
        "blaze" | "bogged" | "breeze" | "cave_spider" | "creaking" | "creeper" | "drowned" | "elder_guardian"
        | "enderman" | "endermite" | "ender_dragon" | "evoker" | "ghast" | "giant" | "guardian" | "hoglin" | "husk"
        | "illusioner" | "magma_cube" | "parched" | "phantom" | "piglin" | "piglin_brute" | "pillager" | "ravager"
        | "shulker" | "silverfish" | "skeleton" | "slime" | "spider" | "stray" | "vex" | "vindicator" | "warden"
        | "witch" | "wither" | "wither_skeleton" | "zoglin" | "zombie" | "zombie_villager" | "zombified_piglin" => {
            Category::Hostile
        }
        _ => Category::Neutral,
    }
}

/// Rotation of a vector around X then Y (angles in game degrees, game signs).
fn rot_xy(v: [f64; 3], pitch: f32, yaw: f32) -> [f64; 3] {
    let (s, c) = (f64::from(-pitch.to_radians()).sin(), f64::from(-pitch.to_radians()).cos());
    let v = [v[0], v[1] * c + v[2] * s, v[2] * c - v[1] * s];
    let (s, c) = (f64::from(-yaw.to_radians()).sin(), f64::from(-yaw.to_radians()).cos());
    [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s]
}

/// Reading of a network sound event reference: registry id + 1, or 0 then name.
fn holder_sound(bytes: &[u8], registry: &[String]) -> Option<String> {
    let mut r = rc_proto::Reader::new(bytes);
    let n = rc_proto::varint::read_varint(&mut r).ok()?;
    if n > 0 {
        return registry.get(n as usize - 1).cloned();
    }
    rc_proto::read_str(&mut r, 32767).ok().map(str::to_owned)
}

impl crate::App {
    fn ev_r(&mut self) -> f64 {
        f64::from(self.world_random.f())
    }

    /// Uniform integer in [0, n).
    fn ev_bounded(&mut self, n: i32) -> i32 {
        (self.ev_r() * f64::from(n)) as i32
    }

    fn ev_g(&mut self) -> f64 {
        self.world_random.gaussian()
    }

    fn ev_body(&self, id: i32) -> Option<Body> {
        let table = &self.entities.dims;
        if id == self.player_id {
            let (w, h) = self.player.dimensions();
            let me = self.entities.me.as_ref();
            return Some(Body {
                pos: self.player.pos,
                pos_o: me.map_or(self.player.pos, |m| m.pos_o),
                width: w,
                height: h,
                eye: self.player.eye_height(),
                yaw: self.yaw,
                pitch: self.pitch,
                silent: me.is_some_and(|m| m.meta_bool(4)),
                kind: "player".into(),
                category: Category::Player,
            });
        }
        let e = self.entities.map.get(&id)?;
        // Current dimensions (pose, baby, size, scale attribute).
        let s = e.size(table);
        Some(Body {
            pos: e.pos,
            pos_o: e.pos_o,
            width: f64::from(s.w),
            height: f64::from(s.h),
            eye: f64::from(s.eye),
            yaw: e.yaw,
            pitch: e.pitch,
            silent: e.meta_bool(4),
            kind: e.kind.clone(),
            category: category(&e.kind, Some(e)),
        })
    }

    fn ev_spawn(&mut self, kind: PKind, o: &POptions, pos: [f64; 3], vel: [f64; 3]) {
        self.particles.spawn(kind, o, pos, vel);
    }

    fn ev_sound(&mut self, name: &str, cat: Category, pos: [f64; 3], volume: f32, pitch: f32) {
        if rc_audio::sound_log() {
            eprintln!("[entity event] sound {name} ({cat:?}, volume {volume:.2}, pitch {pitch:.2})");
        }
        if let Some(a) = self.audio.get() {
            a.play(name, cat, Some(pos), volume, pitch, None);
        }
    }

    fn ev_item_id(&self, name: &str) -> Option<i32> {
        let r = self.data.registry("minecraft:item")?;
        r.entries.iter().position(|e| e == name).map(|i| i as i32)
    }

    fn ev_item_name(&self, item: i32) -> &str {
        usize::try_from(item)
            .ok()
            .and_then(|i| self.data.registry("minecraft:item")?.entries.get(i))
            .map_or("", String::as_str)
    }

    /// The stack carries the component (patch, otherwise default of the non-removed item).
    fn ev_has(&self, stack: &ItemStack, component: &str) -> bool {
        let Some(c) = self.component_ids.of(component) else { return false };
        if stack.component(c).is_some() {
            return true;
        }
        let info = usize::try_from(stack.item).ok().and_then(|i| self.data.items.get(i));
        info.is_some_and(|i| i.components.contains(&c)) && !stack.removed.contains(&c)
    }

    /// Equipment of an entity by network slot (0 main hand, 1 off hand, 2 feet,
    /// 3 legs, 4 chest, 5 head, 6 body, 7 saddle); local player: their inventory.
    fn ev_equipment(&self, id: i32, slot: usize) -> Option<ItemStack> {
        if id == self.player_id {
            return match slot {
                0 => self.inventory.hotbar(self.hud.selected).cloned(),
                1 => self.inventory.offhand().cloned(),
                2..=5 => self.inventory.menu.get(10 - slot).cloned().flatten(),
                _ => None,
            };
        }
        self.entities.map.get(&id)?.equipment.get(slot)?.clone()
    }

    /// `damage_type` registry received: injury sound of each type, from its contents, or
    /// the game's data for a known pack.
    pub(crate) fn ev_damage_registry(&mut self, entries: &[(String, Option<Nbt>)]) {
        self.ev.damage_sounds = entries
            .iter()
            .map(|(name, nbt)| {
                let effects = match nbt {
                    Some(n) => n.get("effects").and_then(Nbt::as_str).unwrap_or("hurt").to_owned(),
                    None => {
                        let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
                        self.assets
                            .read(&format!("data/{ns}/damage_type/{path}.json"))
                            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                            .and_then(|j| j.get("effects").and_then(|v| v.as_str()).map(str::to_owned))
                            .unwrap_or_else(|| "hurt".into())
                    }
                };
                hurt_sound(&effects)
            })
            .collect();
    }

    /// Local effects of an entity event, before its application to the entity state.
    pub(crate) fn entity_event_fx(&mut self, ev: &rc_client::EntityEvent) {
        match *ev {
            rc_client::EntityEvent::Event { id, event } => self.on_entity_event(id, event),
            rc_client::EntityEvent::Damage { id, source } => self.on_damage_event(id, source),
            _ => {}
        }
    }

    /// Damage received: only the local player hears it (the server excludes them from its own sound).
    fn on_damage_event(&mut self, id: i32, source: i32) {
        if id != self.player_id {
            return;
        }
        let name = usize::try_from(source)
            .ok()
            .and_then(|i| self.ev.damage_sounds.get(i))
            .copied()
            .unwrap_or("minecraft:entity.player.hurt");
        let pitch = ((self.ev_r() - self.ev_r()) * 0.2 + 1.0) as f32;
        let pos = self.player.pos;
        self.ev_sound(name, Category::Player, pos, 1.0, pitch);
    }

    fn on_entity_event(&mut self, id: i32, event: i8) {
        let Some(b) = self.ev_body(id) else { return };
        let living =
            id == self.player_id || self.entities.map.get(&id).is_some_and(|e| e.class != crate::entities::Kind::Other);
        let mob = self.entities.map.get(&id).is_some_and(|e| e.class == crate::entities::Kind::Mob);
        let k = b.kind.as_str();
        match (k, event) {
            // Totem: following emitter of 30 ticks, sound, full-screen animation of the local player.
            (_, 35) => {
                self.fx_emitter(id, PKind::TotemOfUndying, 30);
                self.ev_sound("minecraft:item.totem.use", b.category, b.pos, 1.0, 1.0);
                if id == self.player_id {
                    let found = [self.ev_equipment(id, 0), self.ev_equipment(id, 1)]
                        .into_iter()
                        .flatten()
                        .find(|s| self.ev_has(s, "minecraft:death_protection"));
                    let stack = found.or_else(|| {
                        let item = self.ev_item_id("minecraft:totem_of_undying")?;
                        Some(ItemStack { item, count: 1, added: Vec::new(), removed: Vec::new() })
                    });
                    if let Some(s) = stack {
                        let (rx, ry) = (self.world_random.f(), self.world_random.f());
                        self.ev.totem.activate(s, rx, ry);
                    }
                }
            }
            // Death of the local player: death sound (that of the others comes from the server).
            ("player", 3) if id == self.player_id => {
                let pitch = ((self.ev_r() - self.ev_r()) * 0.2 + 1.0) as f32;
                self.ev_sound("minecraft:entity.player.death", Category::Player, b.pos, 1.0, pitch);
            }
            // Snowball, egg: 8 shards of the carried item (data 8, otherwise default item).
            ("snowball" | "egg", 3) => {
                let default = if k == "snowball" { "minecraft:snowball" } else { "minecraft:egg" };
                let item = match self.entities.map.get(&id).and_then(|e| e.meta.get(&8)) {
                    Some(Meta::Item(Some(s))) if s.count > 0 => Some(s.item),
                    Some(Meta::Item(_)) => None,
                    _ => self.ev_item_id(default),
                };
                for _ in 0..8 {
                    let (kind, o) = match item {
                        Some(item) => (PKind::Item, POptions { item, ..Default::default() }),
                        None if k == "snowball" => (PKind::ItemSnowball, POptions::default()),
                        None => break,
                    };
                    let v = if k == "egg" {
                        [(self.ev_r() - 0.5) * 0.08, (self.ev_r() - 0.5) * 0.08, (self.ev_r() - 0.5) * 0.08]
                    } else {
                        [0.0; 3]
                    };
                    self.ev_spawn(kind, &o, b.pos, v);
                }
            }
            // Effect arrow: 20 effect particles of its colour.
            ("arrow", 0) => {
                let Some(c) = self.entities.map.get(&id).and_then(|e| e.meta_int(11)).filter(|&c| c != -1) else {
                    return;
                };
                let o = POptions { argb: 0xff00_0000 | (c as u32 & 0x00ff_ffff), ..Default::default() };
                for _ in 0..20 {
                    let p = self.ev_random_in(&b, 0.5, 0.0);
                    self.ev_spawn(PKind::EntityEffect, &o, p, [0.0; 3]);
                }
            }
            // Evoker fangs: snapping (local sound).
            ("evoker_fangs", 4) if !b.silent => {
                let pitch = (self.ev_r() * 0.2 + 0.85) as f32;
                self.ev_sound("minecraft:entity.evoker_fangs.attack", Category::Neutral, b.pos, 1.0, pitch);
            }
            // Exploding firework: explosion engine (`particles.rs`).
            ("firework_rocket", 17) => {
                let (explosions, vel) = self.ev_fireworks(id);
                let sound = !b.silent;
                self.particles.create_fireworks(b.pos, vel, explosions, sound);
            }
            // Armor stand hit: local sound (the oscillation is in `entities.rs`).
            ("armor_stand", 32) => {
                self.ev_sound("minecraft:entity.armor_stand.hit", Category::Neutral, b.pos, 0.3, 1.0);
            }
            // Armadillo peeking.
            ("armadillo", 64) => {
                self.ev_sound("minecraft:entity.armadillo.peek", Category::Neutral, b.pos, 1.0, 1.0);
            }
            // Cured zombie villager: local sound at the eyes.
            ("zombie_villager", 16) if !b.silent => {
                let volume = (1.0 + self.ev_r()) as f32;
                let pitch = (self.ev_r() * 0.7 + 0.3) as f32;
                let p = [b.pos[0], b.pos[1] + b.eye, b.pos[2]];
                self.ev_sound("minecraft:entity.zombie_villager.cure", Category::Hostile, p, volume, pitch);
            }
            // Guardian charging its beam: sound that follows its charge.
            ("guardian" | "elder_guardian", 21) => self.ev_follow(id, Follow::Guardian),
            // Sniffer digging.
            ("sniffer", 63) => self.ev_follow(id, Follow::Sniffer),
            // Hearts: animals (7), allay (3); villager: hearts, anger, happiness, sweat.
            (_, 18) if k == "allay" => self.ev_around(&b, PKind::Heart, 3, 0.02, 0.5),
            (_, 18) if mob && self.ev_is_animal(k) => self.ev_around(&b, PKind::Heart, 7, 0.02, 0.5),
            ("villager", 12) => self.ev_around(&b, PKind::Heart, 5, 0.02, 1.0),
            ("villager", 13) => self.ev_around(&b, PKind::AngryVillager, 5, 0.02, 1.0),
            ("villager", 14) => self.ev_around(&b, PKind::HappyVillager, 5, 0.02, 1.0),
            ("villager", 42) => self.ev_around(&b, PKind::Splash, 5, 0.02, 1.0),
            ("dolphin", 38) => self.ev_around(&b, PKind::HappyVillager, 7, 0.01, 0.2),
            // Taming: hearts (7) or smoke (6); ocelot: trust 41 / 40.
            (_, 6 | 7) if self.ev_is_tamable(k) => {
                let p = if event == 7 { PKind::Heart } else { PKind::Smoke };
                self.ev_around(&b, p, 7, 0.02, 0.5);
            }
            ("ocelot", 40 | 41) => {
                let p = if event == 41 { PKind::Heart } else { PKind::Smoke };
                self.ev_around(&b, p, 7, 0.02, 0.5);
            }
            // Witch: the game draws the number at random (10 to 44) on EACH pass of the
            // loop, exit condition included: about 17 particles on average (quirk
            // of the game 26.3, reproduced).
            ("witch", 15) => {
                let mut i = 0;
                while i < self.ev_bounded(35) + 10 {
                    i += 1;
                    let p = [
                        b.pos[0] + self.ev_g() * 0.13,
                        b.pos[1] + b.height + 0.5 + self.ev_g() * 0.13,
                        b.pos[2] + self.ev_g() * 0.13,
                    ];
                    self.ev_spawn(PKind::Witch, &POptions::default(), p, [0.0; 3]);
                }
            }
            // Fox eating: 8 crumbs of the item in its mouth.
            ("fox", 45) => {
                if let Some(s) = self.ev_equipment(id, 0) {
                    let o = POptions { item: s.item, ..Default::default() };
                    let look = rot_xy([0.0, 0.0, 1.0], b.pitch, b.yaw);
                    for _ in 0..8 {
                        let d = rot_xy([(self.ev_r() - 0.5) * 0.1, self.ev_r() * 0.1 + 0.1, 0.0], b.pitch, b.yaw);
                        let p = [b.pos[0] + look[0] / 2.0, b.pos[1], b.pos[2] + look[2] / 2.0];
                        self.ev_spawn(PKind::Item, &o, p, [d[0], d[1] + 0.05, d[2]]);
                    }
                }
            }
            // Ravager roaring: 40 puffs at the centre, push of the nearby local player.
            ("ravager", 69) => self.ev_roar(&b),
            // Boat on a bubble column (locally piloted): sucked in or thrown.
            (_, 71 | 72) if k.ends_with("boat") || k.ends_with("raft") => {
                let down = event == 71;
                self.piloted_velocity(id, |v| {
                    if down {
                        v[1] -= 0.7;
                    } else {
                        v[1] = 0.6;
                    }
                });
            }
            // Fishing bobber pulling its catch: the hooked local player is pulled.
            ("fishing_bobber", 31) => {
                let Some(e) = self.entities.map.get(&id) else { return };
                let hooked = e.meta_int(8).filter(|&v| v > 0).map(|v| v - 1);
                let owner = e.data;
                if hooked == Some(self.player_id) && self.player_id != owner {
                    let o = if let Some(o) = self.entities.map.get(&owner) { o.pos } else { return };
                    let d = [(o[0] - b.pos[0]) * 0.1, (o[1] - b.pos[1]) * 0.1, (o[2] - b.pos[2]) * 0.1];
                    self.player.push(d);
                }
            }
            // Honey block: slide (5) and jump (10), shards of the block at the feet.
            (_, 53) => self.ev_honey(&b, 5),
            (_, 54) if living => self.ev_honey(&b, 10),
            // Teleportation (chorus, endermite…): 128 portal particles along the path.
            (_, 46) if living => {
                for i in 0..128 {
                    let d = f64::from(i) / 127.0;
                    let v = [(self.ev_r() - 0.5) * 0.2, (self.ev_r() - 0.5) * 0.2, (self.ev_r() - 0.5) * 0.2];
                    let lerp = |a: f64, c: f64| a + d * (c - a);
                    let p = [
                        lerp(b.pos_o[0], b.pos[0]) + (self.ev_r() - 0.5) * b.width * 2.0,
                        lerp(b.pos_o[1], b.pos[1]) + self.ev_r() * b.height,
                        lerp(b.pos_o[2], b.pos[2]) + (self.ev_r() - 0.5) * b.width * 2.0,
                    ];
                    // Speeds in single precision in the game.
                    let v = v.map(|c| f64::from(c as f32));
                    self.ev_spawn(PKind::Portal, &POptions::default(), p, v);
                }
            }
            // Broken equipment: item break sound and 5 shards in front of the eyes.
            (_, 47..=52 | 65 | 68) if living => {
                let slot = match event {
                    47 => 0,
                    48 => 1,
                    49 => 5,
                    50 => 4,
                    51 => 3,
                    52 => 2,
                    65 => 6,
                    _ => 7,
                };
                if let Some(s) = self.ev_equipment(id, slot) {
                    self.ev_break_item(&b, &s);
                }
            }
            // End of a meal of the local player: last bite.
            (_, 9) if id == self.player_id => self.finish_use_event(),
            // Kinetic hit (spear): local hit sound, at most once per 10 ticks.
            (_, 2) if living => self.ev_kinetic(id, &b),
            // Mob spawn (spawner, egg…) and death: smoke cloud.
            (_, 20) if mob => self.ev_poof(&b),
            (_, 60) if living => self.ev_poof(&b),
            // Drowning: 8 bubbles that follow the velocity.
            (_, 67) if living => {
                let v = self.entities.map.get(&id).map_or(self.player.vel, |e| e.velocity);
                for _ in 0..8 {
                    let o = [self.ev_tri(), self.ev_tri(), self.ev_tri()];
                    let p = [b.pos[0] + o[0], b.pos[1] + o[1], b.pos[2] + o[2]];
                    self.ev_spawn(PKind::Bubble, &POptions::default(), p, v);
                }
            }
            _ => {}
        }
    }

    /// Triangular draw of mode 0 and spread 1 (`triangle(0, 1)`).
    fn ev_tri(&mut self) -> f64 {
        self.ev_r() - self.ev_r()
    }

    /// Random point in the box: x, z within ± `spread` × width, y within the height + `dy`.
    fn ev_random_in(&mut self, b: &Body, spread: f64, dy: f64) -> [f64; 3] {
        [
            b.pos[0] + b.width * (2.0 * self.ev_r() - 1.0) * spread,
            b.pos[1] + b.height * self.ev_r() + dy,
            b.pos[2] + b.width * (2.0 * self.ev_r() - 1.0) * spread,
        ]
    }

    /// `n` particles around the entity, gaussian velocity × `speed`, height + `dy`.
    fn ev_around(&mut self, b: &Body, kind: PKind, n: u32, speed: f64, dy: f64) {
        for _ in 0..n {
            let v = [self.ev_g() * speed, self.ev_g() * speed, self.ev_g() * speed];
            let p = self.ev_random_in(b, 1.0, dy);
            self.ev_spawn(kind, &POptions::default(), p, v);
        }
    }

    /// Death / spawn cloud: 20 puffs set back by 10 × their velocity.
    fn ev_poof(&mut self, b: &Body) {
        for _ in 0..20 {
            let v = [self.ev_g() * 0.02, self.ev_g() * 0.02, self.ev_g() * 0.02];
            let p = self.ev_random_in(b, 1.0, 0.0);
            let p = [p[0] - v[0] * 10.0, p[1] - v[1] * 10.0, p[2] - v[2] * 10.0];
            self.ev_spawn(PKind::Poof, &POptions::default(), p, v);
        }
    }

    fn ev_honey(&mut self, b: &Body, n: u32) {
        let state = self.data.blocks.iter().find(|x| x.name == "minecraft:honey_block").map_or(0, |x| x.default_state);
        let o = POptions { state, ..Default::default() };
        for _ in 0..n {
            self.ev_spawn(PKind::Block, &o, b.pos, [0.0; 3]);
        }
    }

    /// Animals (love hearts, event 18): the game's animal subclasses.
    fn ev_is_animal(&self, k: &str) -> bool {
        self.ev_is_tamable(k)
            || matches!(
                k,
                "armadillo"
                    | "axolotl"
                    | "bee"
                    | "chicken"
                    | "cow"
                    | "mooshroom"
                    | "fox"
                    | "frog"
                    | "goat"
                    | "happy_ghast"
                    | "hoglin"
                    | "ocelot"
                    | "panda"
                    | "pig"
                    | "polar_bear"
                    | "rabbit"
                    | "sheep"
                    | "sniffer"
                    | "strider"
                    | "turtle"
            )
    }

    /// Tameable animals (wolf, cat, parrot, nautilus) and equines: events 6/7.
    fn ev_is_tamable(&self, k: &str) -> bool {
        matches!(
            k,
            "wolf"
                | "cat"
                | "parrot"
                | "nautilus"
                | "zombie_nautilus"
                | "horse"
                | "donkey"
                | "mule"
                | "skeleton_horse"
                | "zombie_horse"
                | "llama"
                | "trader_llama"
                | "camel"
                | "camel_husk"
        )
    }

    /// Broken item: `break_sound` sound (component, otherwise default of the item), 5 shards.
    fn ev_break_item(&mut self, b: &Body, s: &ItemStack) {
        let sound = match self.component_ids.of("minecraft:break_sound") {
            Some(c) if s.removed.contains(&c) => None,
            Some(c) => match s.component(c) {
                Some(bytes) => {
                    self.data.registry("minecraft:sound_event").and_then(|r| holder_sound(bytes, &r.entries))
                }
                None => Some(
                    match self.ev_item_name(s.item) {
                        "minecraft:shield" => "minecraft:item.shield.break",
                        "minecraft:wolf_armor" => "minecraft:item.wolf_armor.break",
                        _ => "minecraft:entity.item.break",
                    }
                    .to_owned(),
                ),
            },
            None => Some("minecraft:entity.item.break".to_owned()),
        };
        if let Some(name) = sound
            && !b.silent
        {
            let pitch = (0.8 + self.ev_r() * 0.4) as f32;
            self.ev_sound(&name, b.category, b.pos, 0.8, pitch);
        }
        let o = POptions { item: s.item, ..Default::default() };
        for _ in 0..5 {
            let d = rot_xy([(self.ev_r() - 0.5) * 0.1, self.ev_r() * 0.1 + 0.1, 0.0], b.pitch, b.yaw);
            let y = -self.ev_r() * 0.6 - 0.3;
            let p = rot_xy([(self.ev_r() - 0.5) * 0.3, y, 0.6], b.pitch, b.yaw);
            let p = [p[0] + b.pos[0], p[1] + b.pos[1] + b.eye, p[2] + b.pos[2]];
            self.ev_spawn(PKind::Item, &o, p, [d[0], d[1] + 0.05, d[2]]);
        }
    }

    /// Spear hit: item in use carrying `kinetic_weapon`, local hit sound.
    fn ev_kinetic(&mut self, id: i32, b: &Body) {
        let now = self.env.env.clocks.game_time;
        if self.ev.kinetic.get(&id).is_some_and(|&t| now - t <= 10) {
            return;
        }
        self.ev.kinetic.insert(id, now);
        let flags = if id == self.player_id {
            self.entities.me.as_ref().map_or(0, |m| m.meta_byte(8))
        } else {
            self.entities.map.get(&id).map_or(0, |e| e.meta_byte(8))
        };
        if flags & 1 == 0 {
            return;
        }
        let Some(s) = self.ev_equipment(id, usize::from(flags & 2 != 0)) else { return };
        if !self.ev_has(&s, "minecraft:kinetic_weapon") {
            return;
        }
        let name = if self.ev_item_name(s.item) == "minecraft:wooden_spear" {
            "minecraft:item.spear_wood.hit"
        } else {
            "minecraft:item.spear.hit"
        };
        self.ev_sound(name, b.category, b.pos, 1.0, 1.0);
    }

    /// Roar: 40 puffs at the centre of the box; the living local player inside the box
    /// widened by 4 is pushed back by 4/d² horizontally and 0.2 upwards.
    fn ev_roar(&mut self, b: &Body) {
        let c = [b.pos[0], b.pos[1] + b.height / 2.0, b.pos[2]];
        for _ in 0..40 {
            let v = [self.ev_g() * 0.2, self.ev_g() * 0.2, self.ev_g() * 0.2];
            self.ev_spawn(PKind::Poof, &POptions::default(), c, v);
        }
        if self.hud.dead() {
            return;
        }
        let (w, h) = self.player.dimensions();
        let p = self.player.pos;
        let hw = b.width / 2.0 + 4.0;
        let hit = p[0] - w / 2.0 < b.pos[0] + hw
            && p[0] + w / 2.0 > b.pos[0] - hw
            && p[1] < b.pos[1] + b.height + 4.0
            && p[1] + h > b.pos[1] - 4.0
            && p[2] - w / 2.0 < b.pos[2] + hw
            && p[2] + w / 2.0 > b.pos[2] - hw;
        if hit {
            let (dx, dz) = (p[0] - b.pos[0], p[2] - b.pos[2]);
            let dd = (dx * dx + dz * dz).max(0.001);
            self.player.push([dx / dd * 4.0, 0.2, dz / dd * 4.0]);
        }
    }

    /// Firework explosions (`fireworks` component of the carried item, data 8) and speed.
    fn ev_fireworks(&self, id: i32) -> (Vec<crate::particles::FireworkExplosion>, [f64; 3]) {
        let Some(e) = self.entities.map.get(&id) else { return (Vec::new(), [0.0; 3]) };
        let vel = e.velocity;
        let bytes = match (e.meta.get(&8), self.component_ids.of("minecraft:fireworks")) {
            (Some(Meta::Item(Some(s))), Some(c)) => s.component(c),
            _ => None,
        };
        (bytes.and_then(read_fireworks).unwrap_or_default(), vel)
    }

    /// Sound that follows an entity (replaces any sound of the same kind on it).
    fn ev_follow(&mut self, id: i32, kind: Follow) {
        let Some(e) = self.entities.map.get(&id) else { return };
        if e.meta_bool(4) {
            return;
        }
        let pos = e.pos;
        let Some(a) = self.audio.get() else { return };
        let mut req = match kind {
            Follow::Guardian => rc_audio::SoundRequest::new("minecraft:entity.guardian.attack", Category::Hostile)
                .at(pos)
                // Starting volume and pitch 1 (the game only adjusts them on the next tick); a
                // zero volume prevented any start.
                .looping(0),
            Follow::Sniffer => {
                rc_audio::SoundRequest::new("minecraft:entity.sniffer.digging", Category::Neutral).at(pos)
            }
        };
        if kind == Follow::Guardian {
            // Without attenuation: audible everywhere, always placed at the guardian's position.
            req.linear = false;
        }
        if let Some(h) = a.play_request(req) {
            self.ev.follows.push((h, id, kind));
        }
    }

    /// Client tick of the event effects: following sounds, smoke of primed TNT minecarts,
    /// totem animation.
    pub(crate) fn entity_events_tick(&mut self) {
        self.ev.totem.tick();
        let mut smoke = Vec::new();
        for e in self.entities.map.values().filter(|e| e.fuse > 0 && e.kind == "tnt_minecart") {
            smoke.push([e.pos[0], e.pos[1] + 0.5, e.pos[2]]);
        }
        for p in smoke {
            self.ev_spawn(PKind::Smoke, &POptions::default(), p, [0.0; 3]);
        }
        if self.ev.kinetic.len() > 64 {
            let ents = &self.entities;
            let me = self.player_id;
            self.ev.kinetic.retain(|id, _| *id == me || ents.map.contains_key(id));
        }
        if self.ev.follows.is_empty() {
            return;
        }
        let Some(a) = self.audio.get() else {
            self.ev.follows.clear();
            return;
        };
        let ents = &self.entities;
        self.ev.follows.retain(|&(h, id, kind)| {
            if !a.is_playing(h) {
                return false;
            }
            let keep = ents.map.get(&id).filter(|e| !e.meta_bool(4)).and_then(|e| match kind {
                Follow::Guardian => {
                    let dur = if e.kind == "elder_guardian" { 60.0 } else { 80.0 };
                    let s = e.extra[5] / dur;
                    Some((e.pos, s * s, 0.7 + 0.5 * s))
                }
                // Searching (4) or digging (5).
                Follow::Sniffer => matches!(e.meta_int(18), Some(4 | 5)).then_some((e.pos, 1.0, 1.0)),
            });
            match keep {
                Some((pos, volume, pitch)) => {
                    a.set_position(h, pos);
                    a.set_sound_params(h, volume, pitch);
                    true
                }
                None => {
                    a.stop_handle(h);
                    false
                }
            }
        });
    }
}

/// `fireworks` component: flight duration, then explosions (shape, colours, fades, trail,
/// twinkle).
fn read_fireworks(bytes: &[u8]) -> Option<Vec<crate::particles::FireworkExplosion>> {
    use rc_proto::varint::read_varint;
    let mut r = rc_proto::Reader::new(bytes);
    read_varint(&mut r).ok()?;
    let n = read_varint(&mut r).ok()?;
    let colors = |r: &mut rc_proto::Reader<'_>| -> Option<Vec<u32>> {
        let n = read_varint(r).ok()?;
        (0..n.clamp(0, 256)).map(|_| r.take(4).ok().map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))).collect()
    };
    (0..n.clamp(0, 256))
        .map(|_| {
            let shape = read_varint(&mut r).ok()? as u8;
            let colors_v = colors(&mut r)?;
            let fades = colors(&mut r)?;
            let flags = r.take(2).ok()?;
            Some(crate::particles::FireworkExplosion {
                shape,
                colors: colors_v,
                fades,
                trail: flags[0] != 0,
                twinkle: flags[1] != 0,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fireworks_component() {
        // Duration 1; one explosion: star (2), two colours, one fade, trail, no
        // twinkle.
        let mut b = vec![1u8, 1, 2, 2];
        b.extend(0x00ff_0000u32.to_be_bytes());
        b.extend(0x0000_ff00u32.to_be_bytes());
        b.push(1);
        b.extend(0x0000_00ffu32.to_be_bytes());
        b.extend([1, 0]);
        let e = read_fireworks(&b).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].shape, 2);
        assert_eq!(e[0].colors, vec![0xff_0000, 0xff00]);
        assert_eq!(e[0].fades, vec![0xff]);
        assert!(e[0].trail && !e[0].twinkle);
        // Without explosion: empty list (puffs only).
        assert!(read_fireworks(&[1, 0]).unwrap().is_empty());
    }

    #[test]
    fn hurt_sounds_by_effect() {
        assert_eq!(hurt_sound("hurt"), "minecraft:entity.player.hurt");
        assert_eq!(hurt_sound("thorns"), "minecraft:entity.player.hurt");
        assert_eq!(hurt_sound("drowning"), "minecraft:entity.player.hurt_drown");
        assert_eq!(hurt_sound("burning"), "minecraft:entity.player.hurt_on_fire");
        assert_eq!(hurt_sound("poking"), "minecraft:entity.player.hurt_sweet_berry_bush");
        assert_eq!(hurt_sound("freezing"), "minecraft:entity.player.hurt_freeze");
    }

    #[test]
    fn rotation_matches_game_axes() {
        // Yaw 90° (west): the front (z+) becomes x−.
        let v = rot_xy([0.0, 0.0, 1.0], 0.0, 90.0);
        assert!((v[0] + 1.0).abs() < 1e-6 && v[2].abs() < 1e-6, "{v:?}");
        // Pitch −90° (looking up): the front becomes y+.
        let v = rot_xy([0.0, 0.0, 1.0], -90.0, 0.0);
        assert!((v[1] - 1.0).abs() < 1e-6, "{v:?}");
    }
}
