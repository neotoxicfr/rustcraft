//! Health, hunger, experience, damage, death and respawn (`server-gameplay.md` § 1-2); eating;
//! dropping and swapping hands; game modes.

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::play::{SoundPacket, SoundRef};

use crate::nbtio::{float, int, list, set};
use crate::player::Stack;
use crate::server::Server;

/// Survival state of the player.
#[derive(Debug, Clone)]
pub struct Vitals {
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub exhaustion: f32,
    pub food_timer: i32,
    pub xp_level: i32,
    pub xp_progress: f32,
    pub xp_total: i32,
    pub xp_seed: i32,
    pub score: i32,
    pub xp_delay: i32,
    /// Player tick of the last level-up sound (not saved).
    pub last_level_up: i32,
    pub air: i32,
    /// Last published air supply (data 1).
    sent_air: Option<i32>,
    /// Last published score (data 18, death screen).
    sent_score: Option<i32>,
    /// Freeze ticks (powder snow), 140 = frozen; last value sent.
    pub ticks_frozen: i32,
    pub sent_frozen: Option<i32>,
    pub fire: i32,
    pub fall: f64,
    /// Impulse context (wind charge jump): impact point whose height bounds the next fall, and
    /// grace ticks before a landing can forget it.
    pub impulse_impact: Option<[f64; 3]>,
    pub impulse_grace: i32,
    /// G5-F04: a smash on a target on the ground: block particles on the next landing.
    pub extra_fall_particles: bool,
    /// Invulnerability window (§ 2.3 step 7) and last hit.
    pub cooldown: i32,
    pub last_hurt: f32,
    pub dead: bool,
    pub death_time: i32,
    pub ender: Vec<Option<Stack>>,
    /// Respawn point (`respawn`): dimension, block, yaw, pitch, forced.
    pub respawn: Option<(String, [i32; 3], f32, f32, bool)>,
    pub last_death: Option<(String, [i32; 3])>,
    sent_health: Option<(f32, i32, bool)>,
    pub(crate) sent_xp: Option<i32>,
    /// Item use in progress: hand, remaining ticks.
    pub using: Option<(i32, i32)>,
    /// Does the item of the use in progress vibrate (`use_effects.interact_vibrations`)? Read at the start
    /// of the use: the end takes it back even if the stack changed in the meantime. Reset to false when
    /// the stack in use is emptied (thrown, broken): an empty stack does not vibrate.
    pub use_vibrations: bool,
    /// Item of the use in progress (empty once the stack in use is emptied): the use stops at the
    /// tick where the hand no longer holds it.
    pub use_item: String,
    /// Last hit received: type, fall distance, tick.
    pub last_damage: Option<(String, f32, u64)>,
    pub sprinting: bool,
    pub sleeping: Option<[i32; 3]>,
    pub sleep_timer: i32,
    pub age: u64,
    on_fire_sent: bool,
    /// Last values of the read-only criteria (restart from zero on respawn).
    pub(crate) crit: crate::score_criteria::CriteriaWatch,
}

/// Fall location of a climbing block (`death.fell.accident.*` key).
fn climb_location(name: &str) -> &'static str {
    match name {
        "minecraft:ladder" => "ladder",
        n if n.ends_with("_trapdoor") => "ladder",
        "minecraft:vine" => "vines",
        "minecraft:weeping_vines" | "minecraft:weeping_vines_plant" => "weeping_vines",
        "minecraft:twisting_vines" | "minecraft:twisting_vines_plant" => "twisting_vines",
        "minecraft:scaffolding" => "scaffolding",
        _ => "other_climbable",
    }
}

/// Points to reach the next level (§ 1.3).
pub fn xp_needed(level: i32) -> i32 {
    if level >= 30 {
        112 + (level - 30) * 9
    } else if level >= 15 {
        37 + (level - 15) * 5
    } else {
        7 + level * 2
    }
}

impl Vitals {
    /// Hit by an explosion: a player's wind charge sets the impact point (`at`) and at least 40
    /// grace ticks; any other explosion cancels the grace (the point stays).
    pub(crate) fn impulse_from_explosion(&mut self, wind_charge: bool, at: [f64; 3]) {
        if wind_charge {
            self.impulse_grace = self.impulse_grace.max(40);
            self.impulse_impact = Some(at);
        } else {
            self.impulse_grace = 0;
        }
    }

    /// Impulse context forgotten.
    pub(crate) fn impulse_reset(&mut self) {
        self.impulse_grace = 0;
        self.impulse_impact = None;
    }

    /// Context forgotten only once the grace has elapsed.
    pub(crate) fn impulse_try_reset(&mut self) {
        if self.impulse_grace == 0 {
            self.impulse_reset();
        }
    }

    /// Health, hunger and experience resent on the next tick (dimension change, S6).
    pub(crate) fn force_resend(&mut self) {
        self.sent_health = None;
        self.sent_xp = None;
    }

    pub fn read(p: &Nbt) -> Vitals {
        let respawn = p.get("respawn").and_then(|r| {
            let pos = match r.get("pos") {
                Some(Nbt::IntArray(a)) if a.len() == 3 => [a[0], a[1], a[2]],
                _ => return None,
            };
            Some((
                r.get("dimension").and_then(Nbt::as_str).unwrap_or("minecraft:overworld").to_owned(),
                pos,
                float(r, "yaw").unwrap_or(0.0),
                float(r, "pitch").unwrap_or(0.0),
                int(r, "forced").unwrap_or(0) != 0,
            ))
        });
        let mut ender = vec![None; 27];
        for it in list(p.get("EnderItems")) {
            if let (Some(s), Some(st)) = (int(it, "Slot"), Stack::from_nbt(it))
                && (0..27).contains(&s)
            {
                ender[s as usize] = Some(st);
            }
        }
        let health = float(p, "Health").unwrap_or(20.0);
        Vitals {
            health,
            food: int(p, "foodLevel").unwrap_or(20) as i32,
            saturation: float(p, "foodSaturationLevel").unwrap_or(5.0),
            exhaustion: float(p, "foodExhaustionLevel").unwrap_or(0.0),
            food_timer: int(p, "foodTickTimer").unwrap_or(0) as i32,
            xp_level: int(p, "XpLevel").unwrap_or(0) as i32,
            xp_progress: float(p, "XpP").unwrap_or(0.0),
            xp_total: int(p, "XpTotal").unwrap_or(0) as i32,
            // S7: null seed in an existing file → drawn on load (like the game).
            xp_seed: match int(p, "XpSeed").unwrap_or(0) as i32 {
                0 if p.get("XpLevel").is_some() => crate::rng::Rng::from_time().next_int(),
                s => s,
            },
            score: int(p, "Score").unwrap_or(0) as i32,
            xp_delay: 0,
            last_level_up: 0,
            air: int(p, "Air").unwrap_or(300) as i32,
            sent_air: None,
            sent_score: None,
            ticks_frozen: int(p, "TicksFrozen").unwrap_or(0) as i32,
            sent_frozen: None,
            fire: int(p, "Fire").unwrap_or(-20) as i32,
            fall: p
                .get("fall_distance")
                .and_then(|v| if let Nbt::Double(d) = v { Some(*d) } else { None })
                .unwrap_or(0.0),
            impulse_impact: match list(p.get("current_explosion_impact_pos")) {
                [Nbt::Double(x), Nbt::Double(y), Nbt::Double(z)] => Some([*x, *y, *z]),
                _ => None,
            },
            impulse_grace: int(p, "current_impulse_context_reset_grace_time").unwrap_or(0) as i32,
            extra_fall_particles: int(p, "spawn_extra_particles_on_fall").is_some_and(|v| v != 0),
            cooldown: 0,
            last_hurt: 0.0,
            dead: health <= 0.0,
            death_time: int(p, "DeathTime").unwrap_or(0) as i32,
            ender,
            respawn,
            // Last place of death (dimension and block), reread as written.
            last_death: p.get("LastDeathLocation").and_then(|d| match d.get("pos") {
                Some(Nbt::IntArray(a)) if a.len() == 3 => {
                    Some((d.get("dimension").and_then(Nbt::as_str)?.to_owned(), [a[0], a[1], a[2]]))
                }
                _ => None,
            }),
            sent_health: None,
            sent_xp: None,
            using: None,
            use_vibrations: false,
            use_item: String::new(),
            last_damage: None,
            sprinting: false,
            sleeping: None,
            sleep_timer: 0,
            age: 0,
            on_fire_sent: false,
            crit: Default::default(),
        }
    }

    pub fn write(&self, root: &mut Nbt) {
        set(root, "Health", Nbt::Float(self.health));
        set(root, "foodLevel", Nbt::Int(self.food));
        set(root, "foodSaturationLevel", Nbt::Float(self.saturation));
        set(root, "foodExhaustionLevel", Nbt::Float(self.exhaustion));
        set(root, "foodTickTimer", Nbt::Int(self.food_timer));
        set(root, "XpLevel", Nbt::Int(self.xp_level));
        set(root, "XpP", Nbt::Float(self.xp_progress));
        set(root, "XpTotal", Nbt::Int(self.xp_total));
        set(root, "XpSeed", Nbt::Int(self.xp_seed));
        set(root, "Score", Nbt::Int(self.score));
        set(root, "Air", Nbt::Short(self.air as i16));
        if self.ticks_frozen > 0 {
            set(root, "TicksFrozen", Nbt::Int(self.ticks_frozen));
        } else {
            crate::nbtio::remove(root, "TicksFrozen");
        }
        set(root, "Fire", Nbt::Short(self.fire.clamp(-32768, 32767) as i16));
        set(root, "fall_distance", Nbt::Double(self.fall));
        set(root, "current_impulse_context_reset_grace_time", Nbt::Int(self.impulse_grace));
        // G5-F04: always written, like the game (false by default).
        set(root, "spawn_extra_particles_on_fall", Nbt::Byte(i8::from(self.extra_fall_particles)));
        match self.impulse_impact {
            Some(p) => set(root, "current_explosion_impact_pos", Nbt::List(p.map(Nbt::Double).to_vec())),
            None => crate::nbtio::remove(root, "current_explosion_impact_pos"),
        }
        set(root, "DeathTime", Nbt::Short(self.death_time as i16));
        let ender: Vec<Nbt> =
            self.ender.iter().enumerate().filter_map(|(i, s)| s.as_ref().map(|s| s.to_nbt(Some(i as i8)))).collect();
        set(root, "EnderItems", Nbt::List(ender));
        match &self.respawn {
            Some((d, p, yaw, pitch, forced)) => set(
                root,
                "respawn",
                Nbt::Compound(vec![
                    ("dimension".into(), Nbt::String(d.clone())),
                    ("pos".into(), Nbt::IntArray(p.to_vec())),
                    ("yaw".into(), Nbt::Float(*yaw)),
                    ("pitch".into(), Nbt::Float(*pitch)),
                    ("forced".into(), Nbt::Byte(i8::from(*forced))),
                ]),
            ),
            None => crate::nbtio::remove(root, "respawn"),
        }
        if let Some((d, p)) = &self.last_death {
            set(
                root,
                "LastDeathLocation",
                Nbt::Compound(vec![
                    ("dimension".into(), Nbt::String(d.clone())),
                    ("pos".into(), Nbt::IntArray(p.to_vec())),
                ]),
            );
        }
    }

    /// Exhaustion (ignored if invulnerable).
    pub fn exhaust(&mut self, n: f32, invulnerable: bool) {
        if !invulnerable {
            self.exhaustion = (self.exhaustion + n).min(40.0);
        }
    }

    /// Experience points gain (§ 1.3).
    pub fn give_points(&mut self, p: i32) {
        let _ = self.give_points_at(p, 0);
    }

    /// The gift of experience points at player tick `now`: levels gained or lost one by one (a
    /// negative level resets everything to 0, total included); volume of the level-up sound if it
    /// must play (W11).
    pub fn give_points_at(&mut self, p: i32, now: i32) -> Option<f32> {
        if p == 0 {
            return None;
        }
        self.score = self.score.wrapping_add(p);
        self.xp_progress += p as f32 / xp_needed(self.xp_level) as f32;
        self.xp_total = (i64::from(self.xp_total) + i64::from(p)).clamp(0, i64::from(i32::MAX)) as i32;
        let mut sound = None;
        while self.xp_progress < 0.0 {
            let rest = self.xp_progress * xp_needed(self.xp_level) as f32;
            let had = self.xp_level > 0;
            self.give_levels_at(-1, now);
            self.xp_progress = if had { 1.0 + rest / xp_needed(self.xp_level) as f32 } else { 0.0 };
        }
        while self.xp_progress >= 1.0 {
            self.xp_progress = (self.xp_progress - 1.0) * xp_needed(self.xp_level) as f32;
            sound = self.give_levels_at(1, now).or(sound);
            self.xp_progress /= xp_needed(self.xp_level) as f32;
        }
        sound
    }

    /// Saturating addition; below 0, level, progress and total at 0;
    /// sound (volume 0.75 × min(level/30, 1)) when rising onto a multiple of 5, at most every
    /// 100 player ticks.
    pub fn give_levels_at(&mut self, n: i32, now: i32) -> Option<f32> {
        if n == 0 {
            return None;
        }
        self.xp_level = self.xp_level.saturating_add(n);
        if self.xp_level < 0 {
            self.xp_level = 0;
            self.xp_progress = 0.0;
            self.xp_total = 0;
        }
        if n > 0 && self.xp_level % 5 == 0 && (self.last_level_up as f32) < now as f32 - 100.0 {
            self.last_level_up = now;
            let vol = if self.xp_level > 30 { 1.0 } else { self.xp_level as f32 / 30.0 };
            return Some(vol * 0.75);
        }
        None
    }
}

/// Properties of a damage type read in the pack (`damage_type/<id>.json`).
struct DamageInfo {
    message: String,
    exhaustion: f32,
    scaling: String,
    /// `death_message_type` : `default`, `fall_variants`, `intentional_game_design`.
    death_type: String,
    /// Player hurt sound (`effects` field of the type, `hurt` by default).
    hurt_sound: &'static str,
}

/// Author of a hit (responsible entity) seen at the time of the hit.
#[derive(Debug, Clone)]
struct Attacker {
    /// Displayed name (team, hover, insertion).
    display: rc_text::Component,
    /// Item in the main hand if it carries a custom name: its displayed name.
    named_item: Option<rc_text::Component>,
    /// Living entity (the hit makes it enter combat).
    living: bool,
}

/// Hit received (`CombatEntry`): type, damage, location and fall distance, author, direct
/// entity without author.
#[derive(Debug, Clone)]
struct CombatEntry {
    ty: String,
    damage: f32,
    fall_location: Option<&'static str>,
    fall_distance: f32,
    attacker: Option<Attacker>,
    direct: Option<rc_text::Component>,
}

/// Tracking of the player's hits for death messages (W8a).
#[derive(Debug, Clone, Default)]
pub struct Combat {
    entries: Vec<CombatEntry>,
    last_damage_time: u64,
    in_combat: bool,
    taking_damage: bool,
    /// Last climbing block (forgotten on the ground).
    pub climbable: Option<[i32; 3]>,
    /// Direct entity of a hit without author (projectile without shooter…), set by the caller
    /// just before `hurt`.
    pub direct: Option<rc_text::Component>,
}

impl Combat {
    /// End of a combat (300 ticks without a hit) or of a series of hits (100), or death: list
    /// emptied. `now`: age of the tracked entity in ticks.
    pub(crate) fn recheck(&mut self, now: u64, alive: bool) {
        let reset: u64 = if self.in_combat { 300 } else { 100 };
        if self.taking_damage && (!alive || now.saturating_sub(self.last_damage_time) > reset) {
            self.taking_damage = false;
            self.in_combat = false;
            self.entries.clear();
        }
    }

    /// Hit recorded (after a recheck by the caller); a living author starts a combat.
    fn push(&mut self, entry: CombatEntry, now: u64, alive: bool) {
        let enter = entry.attacker.as_ref().is_some_and(|a| a.living);
        self.entries.push(entry);
        self.last_damage_time = now;
        self.taking_damage = true;
        if !self.in_combat && alive && enter {
            self.in_combat = true;
        }
    }
}

impl Server {
    fn damage_info(&self, ty: &str) -> DamageInfo {
        let v = self.life.pack.get("damage_type", ty);
        let s = |k: &str| v.as_ref().and_then(|v| v[k].as_str().map(str::to_owned));
        DamageInfo {
            message: s("message_id").unwrap_or_else(|| ty.trim_start_matches("minecraft:").to_owned()),
            exhaustion: v.as_ref().and_then(|v| v["exhaustion"].as_f64()).unwrap_or(0.0) as f32,
            scaling: s("scaling").unwrap_or_default(),
            death_type: s("death_message_type").unwrap_or_default(),
            hurt_sound: rc_client::player_hurt_sound(v.as_ref().and_then(|v| v["effects"].as_str()).unwrap_or("hurt")),
        }
    }

    /// Setting a player's mob on fire: duration multiplied by the `burning_time` attribute
    /// (rounded up), never shortened.
    pub(crate) fn player_ignite(&mut self, ticks: i32) {
        let t = (f64::from(ticks) * self.player_attribute("minecraft:burning_time")).ceil() as i32;
        if self.life.vit.fire < t {
            self.set_player_fire_ticks(t);
        }
        self.life.vit.ticks_frozen = 0; // ignition always clears freezing
    }

    /// Writes the player's fire counter. An invulnerable player (creative, spectator) never holds
    /// more than one tick of fire: like the game, the counter is capped each time it is written
    /// for them. A player who was burning when it became invulnerable is out two ticks later, and
    /// does not burn again once back in survival.
    pub(crate) fn set_player_fire_ticks(&mut self, ticks: i32) {
        self.life.vit.fire = if self.abilities().invulnerable { ticks.min(1) } else { ticks };
    }

    /// Damage type in a tag (`tags/damage_type`).
    fn damage_tag(&self, ty: &str, tag: &str) -> bool {
        // Tags are stored under their full name (`minecraft:is_fall`): a short name (`is_fall`,
        // `:is_fall`) is read in `minecraft`, like the game.
        let Some(idx) = self.regs.entries("minecraft:damage_type").iter().position(|e| e == ty) else { return false };
        self.regs
            .tags
            .iter()
            .find(|(r, _)| r == "minecraft:damage_type")
            .and_then(|(_, t)| t.iter().find(|(n, _)| crate::mobs::combat::same_identifier(n, tag)))
            .is_some_and(|(_, ids)| ids.contains(&(idx as i32)))
    }

    pub(crate) fn play_sound(&self, name: &str, category: i32, pos: [f64; 3], volume: f32, pitch: f32) {
        if let Some(e) = self.sound_event(name, category, pos, volume, pitch) {
            let _ = self.send(e);
        }
    }

    /// Sound sent to the current player only (direct game packet, not a world broadcast).
    pub(crate) fn play_sound_personal(&self, name: &str, category: i32, pos: [f64; 3], volume: f32, pitch: f32) {
        if let Some(e) = self.sound_event(name, category, pos, volume, pitch) {
            self.send_personal(e);
        }
    }

    /// Sound for the players near `pos` other than the current one, whose own client plays it:
    /// the game leaves the acting player out and reaches those within 16 blocks (farther above
    /// volume 1). Closed world: nobody else, so nothing is built.
    pub(crate) fn play_sound_for_others(&self, name: &str, category: i32, pos: [f64; 3], volume: f32, pitch: f32) {
        if self.lan.is_none() {
            return;
        }
        if let Some(e) = self.sound_event(name, category, pos, volume, pitch) {
            self.send_others_near(pos, 16.0 * f64::from(volume.max(1.0)), e);
        }
    }

    /// [`Self::play_sound_for_others`] attached to entity `entity` standing at `pos` (the sound
    /// follows it on the clients).
    pub(crate) fn entity_sound_for_others(
        &self,
        entity: i32,
        name: &str,
        category: i32,
        pos: [f64; 3],
        volume: f32,
        pitch: f32,
    ) {
        if self.lan.is_none() {
            return;
        }
        if let Some(Event::Sound(mut s)) = self.sound_event(name, category, pos, volume, pitch) {
            (s.pos, s.entity) = (None, Some(entity));
            self.send_others_near(pos, 16.0 * f64::from(volume.max(1.0)), Event::Sound(s));
        }
    }

    /// [`Self::play_sound_for_others`] at the centre of block `p`, in the block category.
    pub(crate) fn block_sound_for_others(&self, p: [i32; 3], name: &str, volume: f32, pitch: f32) {
        let centre = p.map(|v| f64::from(v) + 0.5);
        self.play_sound_for_others(name, 4, centre, volume, pitch);
    }

    fn sound_event(&self, name: &str, category: i32, pos: [f64; 3], volume: f32, pitch: f32) -> Option<Event> {
        let id = self.data.registry("minecraft:sound_event").and_then(|r| r.entries.iter().position(|e| e == name))?;
        Some(Event::Sound(SoundPacket {
            sound: SoundRef::Id(id as i32),
            category,
            pos: Some(pos),
            entity: None,
            volume,
            pitch,
            seed: sound_seed(),
        }))
    }

    /// Whole armor of the player (`armor` criterion of the scoreboard).
    pub(crate) fn armor_value(&self) -> i32 {
        self.player_attribute("minecraft:armor").floor() as i32
    }

    /// Armor (integer part) and toughness: full values of the attributes
    /// (W8a: tracked equipment, `/attribute`, effects), capped.
    fn armor_values(&self) -> (f32, f32) {
        let armor = self.player_attribute("minecraft:armor");
        let tough = self.player_attribute("minecraft:armor_toughness");
        (armor.floor() as f32, tough as f32)
    }

    /// Enchantment protection of the worn equipment: the `damage_protection` effects of the pack
    /// (G5-F04, `ench_fx`), whose conditions exclude `bypasses_invulnerability` (`/kill`, void).
    fn protection(&mut self, ty: &str) -> f32 {
        let (mut world, mut own) = (self.mobs.rng, self.life.rng);
        let p = {
            let sl = |i: usize| self.slots[i].as_ref();
            let gear = [(0, sl(36 + self.held as usize)), (1, sl(45)), (2, sl(8)), (3, sl(7)), (4, sl(6)), (5, sl(5))];
            let ctx = self.player_hurt_ctx(ty);
            self.ench_protection(&gear, &ctx, &mut world, &mut own)
        };
        (self.mobs.rng, self.life.rng) = (world, own);
        p
    }

    /// G5-F04: damage context of a hit on the player: the mob hitting it (direct when the type has
    /// no projectile of its own).
    fn player_hurt_ctx(&self, ty: &str) -> crate::progress::pred::Ctx<'_> {
        use crate::progress::pred::Who;
        let attacker = self.prog.attacker.as_deref().map(Who::Mob);
        let direct = attacker.is_some() && crate::loot::direct_of(ty) == Some(None);
        self.damage_ctx(ty, Who::E(crate::commands::ents::E::Player), self.pos, attacker, direct)
    }

    /// G5-F04: weapon of a direct attacker of the player (mob in combat, other player).
    fn player_hurt_weapon(&self, ty: &str) -> Option<Stack> {
        if crate::loot::direct_of(ty) != Some(None) {
            return None;
        }
        self.prog.attacker_weapon.clone().or_else(|| self.prog.attacker.as_ref()?.equipment[0].clone())
    }

    /// Worn piece worn down by a hit of type `ty`: `equippable` component present with
    /// `damage_on_hurt` (true by default; false for the elytra), and no `damage_resistant` that
    /// covers the type (netherite against fire).
    pub(crate) fn armor_wears(&self, s: &Stack, ty: &str) -> bool {
        let Some(eq) = self.comp_json(s, "minecraft:equippable") else { return false };
        let h = &eq["damage_on_hurt"];
        if !h.as_bool().or_else(|| h.as_i64().map(|n| n != 0)).unwrap_or(true) {
            return false;
        }
        !self.item_resists(s, ty)
    }

    /// Damage refused outright: player dead or not ready, invulnerable, game rule.
    pub(crate) fn hurt_refused(&self, ty: &str) -> bool {
        if self.life.vit.dead || !self.player_ready() {
            return true;
        }
        if self.abilities().invulnerable && !self.damage_tag(ty, "bypasses_invulnerability") {
            return true;
        }
        // G5-F02e: Frost Walker, immunity to hot-floor damage.
        if self.frost_walker_immune(self.slots[8].as_ref(), ty) {
            return true;
        }
        (self.damage_tag(ty, "is_fall") && !self.rule("minecraft:fall_damage", true))
            || (self.damage_tag(ty, "is_fire") && !self.rule("minecraft:fire_damage", true))
            || (self.damage_tag(ty, "is_drowning") && !self.rule("minecraft:drowning_damage", true))
            || (self.damage_tag(ty, "is_freezing") && !self.rule("minecraft:freeze_damage", true))
    }

    /// Damage taken by the player (§ 2.2-2.3, without attacker); true if it landed.
    pub(crate) fn hurt(&mut self, ty: &str, amount: f32) -> bool {
        let src = std::mem::take(&mut self.prog.dmg_src); // W8c: source (shield)
        if self.hurt_refused(ty) {
            return false;
        }
        // The shoulder parrots get off before the damage computation.
        crate::mobs::surface::parrot::remove_from_shoulders(self);
        // S7: fire resistance.
        if self.damage_tag(ty, "is_fire") && self.fx().has("minecraft:fire_resistance") {
            return false;
        }
        let info = self.damage_info(ty);
        let mut d = amount;
        if info.scaling == "always" {
            d = match self.level.difficulty_id() {
                0 => 0.0,
                1 => (d / 2.0 + 1.0).min(d),
                3 => d * 1.5,
                _ => d,
            };
            if d == 0.0 {
                return false;
            }
        }
        // Damage received (after the difficulty), as the other player's `player_hurt_entity` reads it.
        let received = d.max(0.0);
        // W8c: blocking item (shield), before the invulnerability window.
        let in_use = self.using_blocks_attacks();
        let parried = self.apply_item_blocking(ty, d, &src);
        d -= parried;
        let blocked = parried > 0.0;
        // A hit that damages helmets (falling block, anvil, stalactite) wears a worn head piece
        // (⌊d/4⌋, at least 1) and loses a quarter, like any living entity.
        if self.slots[5].is_some() && self.damage_tag(ty, "damages_helmet") {
            if d > 0.0 && self.slots[5].as_ref().is_some_and(|s| self.armor_wears(s, ty)) {
                self.hurt_item(5, (d / 4.0).max(1.0) as i32);
            }
            d *= 0.75;
        }
        if !d.is_finite() {
            d = f32::MAX;
        }
        let dealt = d;
        // Invulnerability window.
        let full;
        let bypass_cd = self.damage_tag(ty, "bypasses_cooldown");
        let v = &mut self.life.vit;
        if v.cooldown > 10 && !bypass_cd {
            if d <= v.last_hurt {
                return false;
            }
            let excess = d - v.last_hurt;
            v.last_hurt = d;
            d = excess;
            full = false;
        } else {
            v.last_hurt = d;
            v.cooldown = 20;
            full = true;
        }
        // Last hit received: any hit past the invulnerability window and not entirely parried,
        // judged on the damage before armor and absorption (tamed wolf's retaliation).
        if !blocked || dealt > 0.0 {
            let fall = self.life.vit.fall as f32;
            self.life.vit.last_damage = Some((ty.to_owned(), fall, self.tick));
        }
        // Armor.
        if !self.damage_tag(ty, "bypasses_armor") {
            let (armor, tough) = self.armor_values();
            // Wear: nothing for a zero hit; feet first, equippable piece that wears on
            // hurt (not the elytra) and that does not resist the type (netherite and fire).
            if d > 0.0 {
                let n = (d / 4.0).max(1.0) as i32;
                for slot in (5..9).rev() {
                    if self.slots[slot].as_ref().is_some_and(|s| self.armor_wears(s, ty)) {
                        self.hurt_item(slot, n);
                    }
                }
            }
            let t = 2.0 + tough / 4.0;
            let a = (armor - d / t).clamp(armor * 0.2, 20.0);
            let mut frac = a / 25.0;
            // G5-F04: the weapon of a direct attacker modifies the armor fraction (Breach).
            if let Some(w) = self.player_hurt_weapon(ty) {
                let mut own = self.life.rng;
                frac = {
                    let ctx = self.player_hurt_ctx(ty);
                    self.armor_fraction(frac, Some(&w), &ctx, &mut own)
                };
                self.life.rng = own;
            }
            d *= 1.0 - frac;
        }
        // S7: Resistance effect, then enchantment protections.
        if !self.damage_tag(ty, "bypasses_effects") {
            let before = d;
            d = self.resistance_reduce(self.damage_tag(ty, "bypasses_resistance"), d);
            // `damage_resisted` statistic (tenths of a point).
            let resisted = before - d;
            if resisted > 0.0 && resisted < 3.402_823_5e37 {
                self.award_custom("damage_resisted", (resisted * 10.0).round() as i32);
            }
        }
        if !self.damage_tag(ty, "bypasses_effects") && d > 0.0 && !self.damage_tag(ty, "bypasses_enchantments") {
            let p = self.protection(ty);
            if p > 0.0 {
                d *= 1.0 - p.clamp(0.0, 20.0) / 25.0;
            }
        }
        // S7: absorption reserve consumed first.
        let pre_absorb = d; // S9
        d = self.absorb(d);
        let direct = self.life.combat.direct.take();
        if d > 0.0 {
            self.record_damage(ty, d, direct); // W8a: tracked hits (death messages)
            let invul = self.abilities().invulnerable;
            self.life.vit.exhaust(info.exhaustion, invul);
            self.life.vit.health = (self.life.vit.health - d).max(0.0);
            let pos = self.pos;
            self.game_event_at("minecraft:entity_damage", pos, Some(crate::mobs::mob::Ent::Player));
        }
        if full {
            // W8c: parried hit → shield sound instead of the damage event.
            match in_use.as_ref().filter(|_| blocked) {
                Some(ba) => self.blocked_sound(ba),
                None => {
                    let _ = self.send(Event::Entity(EntityEvent::Hurt { id: self.player_id, yaw: 0.0 }));
                }
            }
        }
        if self.life.vit.health <= 0.0 {
            // W8c: totem (protection against death) before death.
            if !self.death_protection(ty) {
                // G5-F04: second sound of a full hit of thorns.
                if full {
                    self.thorns_hit_sound(ty, 7, self.pos);
                }
                self.die(ty);
            }
        } else if full && !blocked {
            let pitch = 1.0 + (self.life.rng.next_float() - self.life.rng.next_float()) * 0.2;
            self.play_sound(info.hurt_sound, 7, self.pos, 1.0, pitch);
            self.thorns_hit_sound(ty, 7, self.pos);
        }
        // Effects on injury (infestation): any hit that landed.
        if !blocked || dealt > 0.0 {
            self.player_hurt_effects();
        }
        self.prog.hurt_parts = Some((received, dealt, blocked));
        self.s9_player_hurt(ty, (received, dealt), d, pre_absorb - d, parried); // S9, W8c: parried part
        self.send_health();
        !blocked || dealt > 0.0
    }

    /// Displayed name of a mob: custom name or type, team, "entity"
    /// hover, UUID insertion.
    pub(crate) fn mob_display(&self, m: &crate::mobs::mob::Mob) -> rc_text::Component {
        let ty = m.kind.info().name.to_owned();
        let base =
            m.custom_name.as_ref().and_then(rc_text::from_nbt).unwrap_or_else(|| {
                rc_text::Component::translate(format!("entity.{}", ty.replace(':', ".")), Vec::new())
            });
        let uuid = crate::nbtio::uuid_text(m.uuid);
        let mut c = self.cmd.scores.decorate(&uuid, base.clone());
        c.style.hover =
            Some(std::sync::Arc::new(rc_text::HoverEvent::Entity { id: ty, uuid: m.uuid, name: Some(base) }));
        c.style.insertion = Some(std::sync::Arc::from(uuid));
        c
    }

    /// Current fall location: last climbing block, otherwise water.
    fn fall_location(&self) -> Option<&'static str> {
        if let Some(p) = self.life.combat.climbable {
            let s = self.get(p);
            return Some(climb_location(self.data.block_of(s).map_or("", |b| b.name.as_str())));
        }
        (self.fluid_at(crate::mobs::mth::block_of(self.pos)).kind == 1).then_some("water")
    }

    /// End of a combat or of a series of hits: list emptied.
    pub(crate) fn combat_recheck(&mut self) {
        let age = self.life.vit.age;
        let alive = !self.life.vit.dead;
        self.life.combat.recheck(age, alive);
    }

    /// Hit received: author taken from the mob responsible for the hit in progress.
    fn record_damage(&mut self, ty: &str, damage: f32, direct: Option<rc_text::Component>) {
        self.combat_recheck();
        let attacker = self.prog.attacker.as_deref().map(|m| Attacker {
            display: self.mob_display(m),
            named_item: m.equipment[0]
                .as_ref()
                .filter(|s| crate::items::component(s, "minecraft:custom_name").is_some())
                .map(|s| self.stack_display(s)),
            living: true,
        });
        // Hit from another player (local network).
        let attacker = attacker.or_else(|| {
            let (display, named_item) = self.prog.attacker_player.clone()?;
            Some(Attacker { display, named_item, living: true })
        });
        let entry = CombatEntry {
            ty: ty.to_owned(),
            damage,
            fall_location: self.fall_location(),
            fall_distance: self.life.vit.fall as f32,
            attacker,
            direct,
        };
        let age = self.life.vit.age;
        let alive = !self.life.vit.dead;
        self.life.combat.push(entry, age, alive);
    }

    /// Most significant fall of a hit list.
    fn most_significant_fall<'c>(&self, entries: &'c [CombatEntry]) -> Option<&'c CombatEntry> {
        let mut result: Option<&CombatEntry> = None;
        let mut alternative: Option<&CombatEntry> = None;
        let mut alt_damage = 0.0f32;
        let mut best_fall = 0.0f32;
        for (i, e) in entries.iter().enumerate() {
            let fake = self.damage_tag(&e.ty, "always_most_significant_fall");
            let fall = if fake { f32::MAX } else { e.fall_distance };
            if (self.damage_tag(&e.ty, "is_fall") || fake) && fall > 0.0 && (result.is_none() || fall > best_fall) {
                result = Some(if i > 0 { &entries[i - 1] } else { e });
                best_fall = fall;
            }
            if e.fall_location.is_some() && (alternative.is_none() || e.damage > alt_damage) {
                alternative = Some(e);
                alt_damage = e.damage;
            }
        }
        if best_fall > 5.0 && result.is_some() {
            result
        } else if alt_damage > 5.0 {
            alternative
        } else {
            None
        }
    }

    /// Mob credited with the death without an author: last mob that
    /// hurt the player at most 100 ticks ago, still alive.
    fn kill_credit(&self) -> Option<rc_text::Component> {
        let (who, ts) = self.mobs.player_hurt_by?;
        if self.mobs.player_ticks - ts > 100 {
            return None;
        }
        match who {
            crate::mobs::mob::Ent::Mob(id) => {
                let m = self.mobs.mob_ref_any(id)?;
                (m.health > 0.0).then(|| self.mob_display(m))
            }
            _ => None,
        }
    }

    /// Message of a hit (`credit`: the victim's kill credit, for a hit without an author).
    fn localized_death(
        &self,
        e: &CombatEntry,
        victim: rc_text::Component,
        credit: Option<rc_text::Component>,
    ) -> rc_text::Component {
        use rc_text::{Arg, Component};
        let msg = format!("death.attack.{}", self.damage_info(&e.ty).message);
        match (&e.attacker, &e.direct) {
            (None, None) => match credit {
                Some(k) => {
                    Component::translate(format!("{msg}.player"), vec![Arg::Component(victim), Arg::Component(k)])
                }
                None => Component::translate(msg, vec![Arg::Component(victim)]),
            },
            (Some(a), _) => match &a.named_item {
                Some(item) => Component::translate(
                    format!("{msg}.item"),
                    vec![Arg::Component(victim), Arg::Component(a.display.clone()), Arg::Component(item.clone())],
                ),
                None => Component::translate(msg, vec![Arg::Component(victim), Arg::Component(a.display.clone())]),
            },
            (None, Some(d)) => Component::translate(msg, vec![Arg::Component(victim), Arg::Component(d.clone())]),
        }
    }

    /// Fall message: pure fall, pushed by, finished by.
    fn fall_message(
        &self,
        knock: &CombatEntry,
        killer: Option<&Attacker>,
        victim: rc_text::Component,
    ) -> rc_text::Component {
        use rc_text::{Arg, Component};
        if self.damage_tag(&knock.ty, "is_fall") || self.damage_tag(&knock.ty, "always_most_significant_fall") {
            let loc = knock.fall_location.unwrap_or("generic");
            return Component::translate(format!("death.fell.accident.{loc}"), vec![Arg::Component(victim)]);
        }
        let assisted = |a: &Attacker, with_item: &str, without: &str| match &a.named_item {
            Some(item) => Component::translate(
                with_item,
                vec![Arg::Component(victim.clone()), Arg::Component(a.display.clone()), Arg::Component(item.clone())],
            ),
            None => {
                Component::translate(without, vec![Arg::Component(victim.clone()), Arg::Component(a.display.clone())])
            }
        };
        match (&knock.attacker, killer) {
            (Some(a), k) if k.is_none_or(|k| k.display != a.display) => {
                assisted(a, "death.fell.assist.item", "death.fell.assist")
            }
            (_, Some(k)) => assisted(k, "death.fell.finish.item", "death.fell.finish"),
            _ => Component::translate("death.fell.killer", vec![Arg::Component(victim)]),
        }
    }

    /// Death message of the player.
    fn death_message(&self) -> rc_text::Component {
        let victim = self.player_display_name();
        self.death_message_of(&self.life.combat, victim, self.kill_credit())
    }

    /// Death message of a tracked entity: its hit list, its displayed name, its kill credit.
    pub(crate) fn death_message_of(
        &self,
        c: &Combat,
        victim: rc_text::Component,
        credit: Option<rc_text::Component>,
    ) -> rc_text::Component {
        use rc_text::{Arg, Component};
        let Some(kill) = c.entries.last() else {
            return Component::translate("death.attack.generic", vec![Arg::Component(victim)]);
        };
        let info = self.damage_info(&kill.ty);
        match (info.death_type.as_str(), self.most_significant_fall(&c.entries)) {
            ("fall_variants", Some(knock)) => self.fall_message(knock, kill.attacker.as_ref(), victim),
            ("intentional_game_design", _) => {
                let msg = format!("death.attack.{}", info.message);
                let mut link = crate::commands::text::brackets(Component::translate(format!("{msg}.link"), Vec::new()));
                link.style.click = Some(std::sync::Arc::new(rc_text::ClickEvent {
                    action: "open_url".into(),
                    value: "https://bugs.mojang.com/browse/MCPE-28723".into(),
                    nbt: None,
                }));
                link.style.hover = Some(std::sync::Arc::new(rc_text::HoverEvent::Text(Component::text("MCPE-28723"))));
                Component::translate(format!("{msg}.message"), vec![Arg::Component(victim), Arg::Component(link)])
            }
            _ => self.localized_death(kill, victim, credit),
        }
    }

    /// G5-F04: hit received by a tamed animal (its owner gets its death message): author seen
    /// at the time of the hit, fall location (current climbing block, otherwise water).
    pub(crate) fn mob_record_damage(
        &mut self,
        m: &mut crate::mobs::mob::Mob,
        ty: &str,
        damage: f32,
        by: Option<crate::mobs::mob::Ent>,
    ) {
        use crate::mobs::mob::Ent as E;
        let named = |s: &Server, st: Option<&Stack>| {
            st.filter(|s| crate::items::component(s, "minecraft:custom_name").is_some()).map(|st| s.stack_display(st))
        };
        let attacker = match by {
            Some(E::Player) => Some(Attacker {
                display: self.player_display_name(),
                named_item: named(self, self.slots[36 + self.held as usize].as_ref()),
                living: true,
            }),
            Some(E::Mob(id)) => self.mob_ref(id).map(|a| Attacker {
                display: self.mob_display(a),
                named_item: named(self, a.equipment[0].as_ref()),
                living: true,
            }),
            None => None,
        };
        // Direct entity without an author (arrow of a dispenser…): named by its type.
        let direct = match (by, crate::loot::direct_of(ty)) {
            (None, Some(Some(t))) => {
                Some(rc_text::Component::translate(format!("entity.{}", t.replace(':', ".")), Vec::new()))
            }
            _ => None,
        };
        let here = crate::mobs::mth::block_of(m.pos());
        let fall_location = if self.phys.climbable(self.st(here)) {
            Some(climb_location(self.bname(here)))
        } else {
            (self.fluid_at(here).kind == 1).then_some("water")
        };
        let entry =
            CombatEntry { ty: ty.to_owned(), damage, fall_location, fall_distance: m.fall_distance as f32, attacker, direct };
        let now = m.tick_count.max(0) as u64;
        let alive = m.health > 0.0 && !m.dead;
        let c = m.combat.get_or_insert_with(Default::default);
        c.recheck(now, alive);
        c.push(entry, now, alive);
    }

    /// G5-F04: kill credit of a mob: the player who hurt it recently, otherwise its last attacker.
    pub(crate) fn mob_kill_credit(&self, m: &crate::mobs::mob::Mob) -> Option<rc_text::Component> {
        use crate::mobs::mob::Ent as E;
        if m.last_hurt_by_player > 0 {
            return Some(self.player_display_name());
        }
        match m.last_hurt_by_mob?.0 {
            E::Player => Some(self.player_display_name()),
            E::Mob(id) => self.mob_ref(id).map(|a| self.mob_display(a)),
        }
    }

    /// Death of the player (§ 1.4).
    fn die(&mut self, ty: &str) {
        let pos = self.pos;
        self.game_event_at("minecraft:entity_die", pos, Some(crate::mobs::mob::Ent::Player));
        self.s9_player_died(ty); // S9: statistics, scores, `entity_killed_player`
        self.life.vit.dead = true;
        self.stop_using();
        let msg = self.death_message();
        let show = self.rule("minecraft:show_death_messages", true);
        let _ = self.send(Event::Died { entity: self.player_id, message: show.then(|| Box::new(msg.clone())) });
        if show {
            let _ = self.send(Event::Chat { component: Box::new(msg), tag: rc_client::ChatTag::System });
        }
        if self.life.menu.is_some() {
            self.close_menu(true);
        }
        if self.rule("minecraft:forgive_dead_players", true) {
            self.forgive_dead_player();
        }
        if self.game_mode != 3 && !self.rule("minecraft:keep_inventory", false) {
            // Vanishing curse: deleted; the rest scattered.
            for i in 0..crate::player::MENU_SLOTS {
                if i < 5 && i != 0 {
                    // 2×2 grid: returned by closing the inventory.
                }
                let Some(s) = self.slots[i].take() else { continue };
                if i == 0 {
                    continue;
                }
                if self.life.items.enchantment(&s, "minecraft:vanishing_curse") > 0 {
                    continue;
                }
                self.drop_from_player_with(s, true);
            }
            let xp = (self.life.vit.xp_level * 7).min(100);
            self.life.vit.xp_level = 0;
            self.life.vit.xp_progress = 0.0;
            self.life.vit.xp_total = 0;
            if xp > 0 {
                self.spawn_orbs(self.pos, xp);
            }
        }
        self.wither_rose_player(); // F16: killed by a wither
        let _ = self.send(Event::Entity(EntityEvent::Event { id: self.player_id, event: 3 }));
        self.life.vit.fire = 0;
        self.life.vit.last_death = Some((
            self.dim.name.clone(),
            [self.pos[0].floor() as i32, self.pos[1].floor() as i32, self.pos[2].floor() as i32],
        ));
        self.broadcast_menus();
    }

    pub(crate) fn send_health(&mut self) {
        let v = &self.life.vit;
        let now = (v.health, v.food, v.saturation == 0.0);
        if v.sent_health != Some(now) {
            let _ = self.send(Event::Health { health: v.health, food: v.food, saturation: v.saturation });
            self.life.vit.sent_health = Some(now);
        }
        let v = &self.life.vit;
        if v.sent_xp != Some(v.xp_total) {
            let _ = self.send(Event::Experience { progress: v.xp_progress, level: v.xp_level, total: v.xp_total });
            self.life.vit.sent_xp = Some(v.xp_total);
        }
    }

    /// Points of a picked-up orb: only the level-up sound here, the pickup sound is played by the
    /// client on the pickup packet (no server draw).
    pub(crate) fn give_xp(&mut self, p: i32) {
        if let Some(vol) = self.life.vit.give_points_at(p, self.mobs.player_ticks) {
            self.play_sound("minecraft:entity.player.levelup", 7, self.pos, vol, 1.0);
        }
    }

    /// Experience points given (command, reward): the game's level-up sound.
    pub(crate) fn give_experience_points(&mut self, p: i32) {
        if let Some(vol) = self.life.vit.give_points_at(p, self.mobs.player_ticks) {
            self.play_sound("minecraft:entity.player.levelup", 7, self.pos, vol, 1.0);
        }
        self.life.vit.sent_xp = None;
        self.send_health();
    }

    /// Levels given (`xp add … levels`): the game's level-up sound.
    pub(crate) fn give_experience_levels(&mut self, n: i32) {
        if let Some(vol) = self.life.vit.give_levels_at(n, self.mobs.player_ticks) {
            self.play_sound("minecraft:entity.player.levelup", 7, self.pos, vol, 1.0);
        }
        self.life.vit.sent_xp = None;
        self.send_health();
    }

    fn block_name_at(&self, p: [f64; 3]) -> String {
        let b = [p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32];
        self.state_at(b).and_then(|s| self.data.block_of(s)).map(|b| b.name.clone()).unwrap_or_default()
    }

    /// "Fall onto" rule of the block `st` (player and mobs): effective distance, damage
    /// factor and damage type; `None` without damage (powder snow). `no_bounce`: the entity
    /// cancels the bounce (sneaking player), the slime becomes an ordinary block again.
    pub(crate) fn fall_rule(&self, st: u32, dist: f64, no_bounce: bool) -> Option<(f64, f64, &'static str)> {
        let v = self.view();
        let name = self.data.block_of(st).map_or("", |b| b.name.as_str());
        Some(match name {
            "minecraft:powder_snow" => return None,
            "minecraft:pointed_dripstone"
                if v.get(st, "vertical_direction") == Some("up") && v.get(st, "thickness") == Some("tip") =>
            {
                (dist + 2.5, 2.0, "minecraft:stalagmite")
            }
            // Factor as a 32-bit float like the game (0.2 is not exactly 0.2 there).
            "minecraft:hay_block" | "minecraft:honey_block" => (dist, f64::from(0.2f32), "minecraft:fall"),
            "minecraft:slime_block" if !no_bounce => (dist, 0.0, "minecraft:fall"),
            // Reduction of the distance (not of the damage): beds, shelf fungus.
            n if n.ends_with("_bed") || n == "minecraft:shelf_mushroom" => {
                (dist * f64::from(1.0f32 - 0.5), 1.0, "minecraft:fall")
            }
            _ => (dist, 1.0, "minecraft:fall"),
        })
    }

    /// Landing dust of a living entity at `at` on the effect block `on`: power = integer part of
    /// the fall beyond the safe distance; 150 × min(0.2 + p/15, 2.5) block shards, speed 0.15,
    /// brought back to the block edge when the entity is not above it.
    pub(crate) fn fall_dust(&self, at: [f64; 3], on: [i32; 3], dist: f64, safe: f64) {
        let power = f64::from((dist + 1.0e-6 - safe).floor().max(0.0) as i32);
        if power <= 0.0 || self.is_air(on) {
            return;
        }
        let (mut x, y, mut z) = (at[0], at[1], at[2]);
        if on[0] != x.floor() as i32 || on[2] != z.floor() as i32 {
            let (dx, dz) = (x - f64::from(on[0]) - 0.5, z - f64::from(on[2]) - 0.5);
            let m = dx.abs().max(dz.abs());
            x = f64::from(on[0]) + 0.5 + dx / m * 0.5;
            z = f64::from(on[2]) + 0.5 + dz / m * 0.5;
        }
        let scale = (f64::from(0.2f32) + power / 15.0).min(2.5);
        self.block_particles(self.st(on), [x, y, z], [0.0; 3], 0.15, (150.0 * scale) as i32);
    }

    /// Fall sound of the block 0.2 below `at` (volume ½, pitch ¾ of its sound group); nothing
    /// on air. Name, volume, pitch.
    pub(crate) fn block_fall_sound_at(&self, at: [f64; 3]) -> Option<(String, f32, f32)> {
        let below = [at[0].floor() as i32, (at[1] - f64::from(0.2f32)).floor() as i32, at[2].floor() as i32];
        if self.is_air(below) {
            return None;
        }
        let st = self.st(below);
        let t = self.data.states.get(st as usize).and_then(|i| self.data.sound_types.get(usize::from(i.sound)))?;
        (!t.fall.is_empty()).then(|| (t.fall.clone(), t.volume * 0.5, t.pitch * 0.75))
    }

    /// Fall sound of the block under the player (players category), after the fall sound.
    fn player_block_fall_sound(&self) {
        if let Some((s, v, p)) = self.block_fall_sound_at(self.pos) {
            self.play_sound(&s, 7, self.pos, v, p);
        }
    }

    /// Player tick (hunger, regeneration, air, fire, void, death, item use).
    pub(crate) fn vitals_tick(&mut self) {
        if !self.player_ready() {
            return;
        }
        // W8a: equipment modifiers (changes spotted at every player tick),
        // health brought under a maximum that drops.
        self.equipment_tick();
        let max = self.max_health();
        if self.life.vit.health > max {
            self.life.vit.health = max;
        }
        self.life.vit.age += 1;
        if self.life.vit.impulse_grace > 0 {
            self.life.vit.impulse_grace -= 1;
        }
        self.send_score();
        if self.life.vit.age.is_multiple_of(20) {
            self.combat_recheck(); // W8a: end of combat after 5 s (15 s in combat) without a hit
        }
        // The damage cooldown counts down in the player's world part (`player_world_rest`).
        if self.life.vit.dead {
            self.life.vit.death_time += 1;
            // End of death (20 ticks): the player is removed from the world like any living entity
            // (puff of smoke, removal effects).
            if self.life.vit.death_time == 20 {
                let _ = self.send(Event::Entity(EntityEvent::Event { id: self.player_id, event: 60 }));
                self.player_removed_effects();
            }
            self.send_health();
            self.criteria_tick();
            self.inside_forget_player();
            return;
        }
        let peaceful = self.level.difficulty_id() == 0;
        let regen = self.rule("minecraft:natural_health_regeneration", true);
        let max = self.max_health(); // S7: health boost
        let hurt = |v: &crate::vitals::Vitals| v.health > 0.0 && v.health < max;
        // Peaceful: healing and hunger restored.
        if peaceful && regen {
            let age = self.life.vit.age;
            let v = &mut self.life.vit;
            if age.is_multiple_of(20) {
                if hurt(v) {
                    v.health = (v.health + 1.0).min(max);
                }
                if v.saturation < 20.0 {
                    v.saturation += 1.0;
                }
            }
            if age.is_multiple_of(10) && v.food < 20 {
                v.food += 1;
            }
        }
        // Hunger.
        {
            let v = &mut self.life.vit;
            if v.exhaustion > 4.0 {
                v.exhaustion -= 4.0;
                if v.saturation > 0.0 {
                    v.saturation = (v.saturation - 1.0).max(0.0);
                } else if !peaceful {
                    v.food = (v.food - 1).max(0);
                }
            }
        }
        let invul = self.abilities().invulnerable;
        let mut starve = false;
        {
            let v = &mut self.life.vit;
            if regen && v.saturation > 0.0 && hurt(v) && v.food >= 20 {
                v.food_timer += 1;
                if v.food_timer >= 10 {
                    let spent = v.saturation.min(6.0);
                    v.health = (v.health + spent / 6.0).min(max);
                    v.exhaust(spent, invul);
                    v.food_timer = 0;
                }
            } else if regen && v.food >= 18 && hurt(v) {
                v.food_timer += 1;
                if v.food_timer >= 80 {
                    v.health = (v.health + 1.0).min(max);
                    v.exhaust(6.0, invul);
                    v.food_timer = 0;
                }
            } else if v.food <= 0 {
                v.food_timer += 1;
                if v.food_timer >= 80 {
                    starve = true;
                    v.food_timer = 0;
                }
            } else {
                v.food_timer = 0;
            }
        }
        if starve {
            let d = self.level.difficulty_id();
            let h = self.life.vit.health;
            if h > 10.0 || d == 3 || (h > 1.0 && d == 2) {
                self.hurt("minecraft:starve", 1.0);
            }
        }
        self.player_boots_tick(); // G5-F02e: Frost Walker, Soul Speed
        if self.game_mode != 3 {
            self.environment_damage();
            self.player_step_on(); // block stepped on: magma, redstone ore
        } else {
            self.inside_forget_player(); // spectator: no crossed block
            // The fire counter runs down and the shared flags go out in the environment pass,
            // skipped here: a spectator does both itself. Its flags are "invisible", plus "on
            // fire" for the one tick its capped counter can still hold (never drawn: no flames on
            // a spectator).
            if self.life.vit.fire > 0 {
                self.set_player_fire_ticks(self.life.vit.fire - 1);
            }
            self.send_shared_flags(self.life.vit.fire > 0);
        }
        self.effects_tick(); // S7
        self.use_item_tick();
        self.turtle_helmet_tick(); // after the base tick, like the game
        self.sync_living_flags();
        self.sleep_tick();
        self.send_health();
        self.criteria_tick(); // health/food/air/armor/xp/level criteria
    }

    /// Air, suffocation, fire, lava, void.
    fn environment_damage(&mut self) {
        let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
        let eye_block = self.block_name_at(eye);
        let feet_block = self.block_name_at(self.pos);
        // In a wall: suffocating block touching the eyes' box (same rule as
        // for the mobs).
        let in_wall = self.game_mode != 3 && self.in_wall(eye, 0.6);
        // Same rule as the turtle helmet and mining: fluid height compared to the eye.
        let eye_water = self.eye_in_water();
        let invul = self.abilities().invulnerable;
        let in_lava = feet_block == "minecraft:lava";
        // Game order: entity fire, lava that cushions the fall, void; then suffocation and
        // drowning; finally the crossed-block pass (water puts out there, lava and fire ignite
        // there) and freezing.
        if self.life.vit.fire > 0 {
            if self.life.vit.fire % 20 == 0 && !in_lava {
                self.hurt("minecraft:on_fire", 1.0);
            }
            self.set_player_fire_ticks(self.life.vit.fire - 1);
        }
        if in_lava {
            self.life.vit.fall *= 0.5;
        }
        if self.pos[1] < f64::from(self.dim.height.min_y - 64) {
            self.hurt("minecraft:out_of_world", 4.0);
        }
        if in_wall {
            self.hurt("minecraft:in_wall", 1.0);
        } else if let Some(d) = self.border_damage() {
            self.hurt("minecraft:outside_border", d); // outside the border
        }
        if eye_water && eye_block != "minecraft:bubble_column" {
            if !invul && !self.water_breathing() {
                // W8a: oxygen bonus read from the attribute (Respiration via the tracked equipment).
                let resp = self.player_attribute("minecraft:oxygen_bonus");
                let keep = resp > 0.0 && self.life.rng.next_double() >= 1.0 / (resp + 1.0);
                if !keep {
                    self.life.vit.air -= 1;
                }
                if self.life.vit.air <= -20 {
                    self.life.vit.air = 0;
                    let _ = self.send(Event::Entity(EntityEvent::Event { id: self.player_id, event: 67 }));
                    self.hurt("minecraft:drown", 2.0);
                }
            } else if self.life.vit.air < 300 && self.effects_refill_air() {
                // S7: water breathing: the air rises underwater.
                self.life.vit.air = (self.life.vit.air + 4).min(300);
            }
        } else if self.life.vit.air < 300 {
            self.life.vit.air = (self.life.vit.air + 4).min(300);
        }
        self.send_air();
        // Crossed blocks (fire, cactus, campfire, powder snow, water, lava, rain).
        let inside = self.inside_blocks(&mut crate::inside_blocks::Who::Player);
        self.player_freezing(inside.in_powder_snow);
        // Not for an invulnerable player: standing in fire it holds its one capped tick here, after
        // the crossed blocks, where the game reads the flag before them (counter back to zero).
        let burning = self.life.vit.fire > 0 && !invul;
        // S7: shared flags (fire, invisibility, glowing) sent if they change.
        self.life.vit.on_fire_sent = burning;
        self.send_shared_flags(burning);
    }

    /// Can freeze: neither a spectator nor wearing an armor piece in `#freeze_immune_wearables`.
    pub(crate) fn player_can_freeze(&self) -> bool {
        self.game_mode != 3
            && !self.slots[5..=8].iter().flatten().any(|s| self.item_tag(&s.id, "minecraft:freeze_immune_wearables"))
    }

    /// Player freezing (like the mobs), after the crossed-block pass, which adds 1 per tick in
    /// powder snow (up to 140): −2 outside snow or if it cannot freeze;
    /// 1 freeze damage every 40 ticks once frozen; counter sent to the client (data 7: frost
    /// overlay).
    fn player_freezing(&mut self, in_snow: bool) {
        const REQUIRED: i32 = crate::inside_blocks::FREEZE_REQUIRED;
        let can_freeze = self.player_can_freeze();
        let v = &mut self.life.vit;
        if !in_snow || !can_freeze {
            v.ticks_frozen = (v.ticks_frozen - 2).max(0);
        }
        if v.age.is_multiple_of(40) && v.ticks_frozen >= REQUIRED && can_freeze {
            self.hurt("minecraft:freeze", 1.0);
        }
        let t = self.life.vit.ticks_frozen;
        if self.life.vit.sent_frozen != Some(t) && (self.life.vit.sent_frozen.is_some() || t != 0) {
            let _ = self.send(Event::Entity(EntityEvent::Data {
                id: self.player_id,
                values: vec![(7, rc_proto_v777::entity::Meta::Int(t))],
            }));
        }
        self.life.vit.sent_frozen = Some(t);
    }

    /// Air supply published to the client (data 1: HUD bubbles) as soon as it changes, like
    /// the synced entity data; the default value (300) is not sent as long
    /// as nothing has been published (the client starts from there).
    fn send_air(&mut self) {
        let a = self.life.vit.air;
        if self.life.vit.sent_air != Some(a) && (self.life.vit.sent_air.is_some() || a != 300) {
            let _ = self.send(Event::Entity(EntityEvent::Data {
                id: self.player_id,
                values: vec![(1, rc_proto_v777::entity::Meta::Int(a))],
            }));
        }
        self.life.vit.sent_air = Some(a);
    }

    /// Player score published to the client (data 18, read by the death screen) as soon as it changes;
    /// 0 (the client's default value) is not sent as long as nothing has been published.
    fn send_score(&mut self) {
        let sc = self.life.vit.score;
        if self.life.vit.sent_score != Some(sc) && (self.life.vit.sent_score.is_some() || sc != 0) {
            let _ = self.send(Event::Entity(EntityEvent::Data {
                id: self.player_id,
                values: vec![(18, rc_proto_v777::entity::Meta::Int(sc))],
            }));
        }
        self.life.vit.sent_score = Some(sc);
    }

    fn water_logged_at(&self, p: [f64; 3]) -> bool {
        let b = [p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32];
        self.state_at(b).is_some_and(|s| self.data.state_properties(s).any(|(k, v)| k == "waterlogged" && v == "true"))
    }

    /// After an accepted movement: fall distance, landing damage, jumps and
    /// movement exhaustion (the movement statistics check).
    pub(crate) fn after_move(&mut self, before: [f64; 3], was_on_ground: bool) {
        if self.life.vit.dead || self.teleport.is_some() {
            return;
        }
        let d = [self.pos[0] - before[0], self.pos[1] - before[1], self.pos[2] - before[2]];
        if d == [0.0; 3] {
            self.impulse_after_move(0.0, false);
            return;
        }
        self.after_move_inner(was_on_ground, d);
        let mut climbing = false;
        // W8a: last climbing block, reread by the death messages (fall location);
        // forgotten on the ground (after the fall damage), noted in the air in a climbable, like the
        // server's check after each movement.
        if self.on_ground {
            self.life.combat.climbable = None;
        } else if self.game_mode != 3 {
            let feet = crate::mobs::mth::block_of(self.pos);
            let s = self.get(feet);
            let v = self.view();
            let climb = self.tag_id(crate::tag_bits::TagId::Climbable, v.block(s));
            let trap_ladder = v.get(s, "open") == Some("true")
                && self.data.block_of(s).is_some_and(|b| b.name.ends_with("trapdoor"))
                && {
                    let below = self.get([feet[0], feet[1] - 1, feet[2]]);
                    self.data.block_of(below).is_some_and(|b| b.name == "minecraft:ladder")
                        && v.get(below, "facing") == v.get(s, "facing")
                };
            climbing = climb || trap_ladder;
            if climbing {
                self.life.combat.climbable = Some(feet);
            }
        }
        self.impulse_after_move(d[1], climbing);
    }

    /// After a movement: the impulse context is forgotten (once the grace has elapsed) on the
    /// ground, when settled in a liquid, when climbing, as a spectator, when gliding or during a
    /// spin attack.
    fn impulse_after_move(&mut self, dy: f64, climbing: bool) {
        // Without a context, nothing to forget: no test (the case for almost all movements).
        if self.life.vit.impulse_impact.is_none() && self.life.vit.impulse_grace == 0 {
            return;
        }
        // Liquid tested last (the only test that reads the world).
        if self.on_ground
            || climbing
            || self.game_mode == 3
            || self.proj_player.fall_flying
            || self.proj_player.spin > 0
            || (dy < f64::from(1.0e-5f32) && matches!(self.fluid_at(crate::mobs::mth::block_of(self.pos)).kind, 1 | 2))
        {
            self.life.vit.impulse_try_reset();
        }
    }

    fn after_move_inner(&mut self, was_on_ground: bool, d: [f64; 3]) {
        let invul = self.abilities().invulnerable;
        let feet = self.block_name_at(self.pos);
        let in_water = feet == "minecraft:water" || self.water_logged_at(self.pos);
        // Jump.
        if was_on_ground && !self.on_ground && d[1] > 0.0 {
            let n = if self.life.vit.sprinting { 0.2 } else { 0.05 };
            self.life.vit.exhaust(n, invul);
        }
        // Distance travelled.
        let horiz = (d[0] * d[0] + d[2] * d[2]).sqrt();
        if in_water {
            let cm = (((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32) * 100.0).round();
            self.life.vit.exhaust(0.01 * cm * 0.01, invul);
        } else if self.on_ground && self.life.vit.sprinting {
            let cm = ((horiz as f32) * 100.0).round();
            self.life.vit.exhaust(0.1 * cm * 0.01, invul);
        }
        // Fall.
        if in_water
            || self.flying
            || feet == "minecraft:ladder"
            || feet == "minecraft:vine"
            || feet == "minecraft:scaffolding"
        {
            self.life.vit.fall = 0.0;
            return;
        }
        // The descent of this movement counts before the landing (converted to 32-bit float
        // like in the game).
        let before = self.life.vit.fall;
        if d[1] < 0.0 {
            self.life.vit.fall -= f64::from(d[1] as f32);
        }
        if self.on_ground {
            // Effect block: 0.2 below the feet, in the column of the main supporting block (fall
            // rule, dust, trampling; edge of a full block above a field: the full block).
            let support = self.player_support();
            let effect = self.on_pos(self.pos, support, 0.2);
            let effect_st = self.state_at(effect).unwrap_or(0);
            // G5-F04: landing after a smash on a target on the ground: up to 200 block particles
            // on top of the block (50 per block of fall), before the dust.
            if self.life.vit.extra_fall_particles && before > 0.0 {
                let top = [f64::from(effect[0]) + 0.5, f64::from(effect[1]) + 1.0, f64::from(effect[2]) + 0.5];
                let n = (50.0 * before).clamp(0.0, 200.0) as i32;
                self.block_particles(effect_st, top, [0.3; 3], 0.15, n);
                self.life.vit.extra_fall_particles = false;
            }
            // Landing dust, before the block's rule, on the fall from before this movement.
            if before > 0.0 {
                self.fall_dust(self.pos, effect, before, self.safe_fall_distance());
            }
            let fall = self.life.vit.fall;
            if fall > 0.0 {
                // Honey block: the slide sound is predicted by the player's client;
                // event 54 (jump particles) goes out to everyone, player included.
                let honey = self.data.block_of(effect_st).is_some_and(|b| b.name == "minecraft:honey_block");
                if honey {
                    let id = self.player_id;
                    let _ = self.send(Event::Entity(EntityEvent::Event { id, event: 54 }));
                }
                let (w, h, _) = self.player_dims();
                self.trample_farmland(effect, effect_st, fall, None, w as f32, h as f32);
                self.turtle_egg_fall_on(effect, None); // G6-F08: before the damage
                let rule = self.fall_rule(effect_st, fall, self.sneaking);
                // Player who can fly: no statistic and no damage, impulse context kept.
                let may_fly = self.game_mode == 1 || self.game_mode == 3;
                // `fall_one_cm` statistic: any fall "onto" the block of at least 2 m
                // (distance after the block's rule, before the impulse bound).
                if let Some((f, ..)) = rule
                    && f >= 2.0
                    && !may_fly
                {
                    self.award_custom("fall_one_cm", (f * 100.0).round() as i32);
                }
                if let Some((mut fall, mult, ty)) = rule
                    && !may_fly
                {
                    // Wind charge jump: fall bounded to the height of the impact point;
                    // landed above it, the context is forgotten, otherwise once the grace has elapsed.
                    if let Some(imp) = self.life.vit.impulse_impact {
                        fall = fall.min(imp[1] - self.pos[1]);
                        if fall <= 0.0 {
                            self.life.vit.impulse_reset();
                        } else {
                            self.life.vit.impulse_try_reset();
                        }
                    }
                    let power = fall + 1.0e-6 - self.safe_fall_distance(); // S7: improved jump
                    let dmg = (power * mult * self.player_attribute("minecraft:fall_damage_multiplier")).floor();
                    if dmg > 0.0 {
                        self.life.vit.impulse_reset();
                        let sound = if dmg > 4.0 {
                            "minecraft:entity.player.big_fall"
                        } else {
                            "minecraft:entity.player.small_fall"
                        };
                        self.play_sound(sound, 7, self.pos, 1.0, 1.0);
                        self.player_block_fall_sound();
                        self.hurt(ty, dmg as f32);
                        // Honey: block fall sound replayed after the damage.
                        if honey {
                            self.play_sound("minecraft:block.honey_block.fall", 7, self.pos, 0.5, 0.75);
                        }
                    }
                }
                // Landing: current state of the main supporting block, otherwise of the block at
                // 0.2 (positions read before a trampling that raises the player, like the game).
                let st = self.st(support.unwrap_or(effect));
                let pos = self.pos;
                let src = Some(crate::mobs::recent::vibration::Src::Living(crate::mobs::mob::Ent::Player));
                self.game_event_src("minecraft:hit_ground", pos, src, Some(st));
            }
            self.life.vit.fall = 0.0;
        }
    }

    /// `client_command` 0: respawn (§ 1.5).
    pub(crate) fn perform_respawn(&mut self) {
        if self.dims.won_game {
            self.respawn_after_credits(); // S6: end of credits
            return;
        }
        if !self.life.vit.dead {
            return;
        }
        // Respawn before the end of death: the old player is removed now (removal
        // effects; list already emptied if it was at 20 ticks).
        self.player_removed_effects();
        // The respawned player is new: it no longer rides anything.
        self.player_remove_vehicle();
        let keep = self.rule("minecraft:keep_inventory", false) || self.game_mode == 3;
        let (pos, yaw, pitch, msg) = self.respawn_point();
        let v = &mut self.life.vit;
        v.dead = false;
        v.death_time = 0;
        v.health = 20.0;
        v.food = 20;
        v.saturation = 5.0;
        v.exhaustion = 0.0;
        v.food_timer = 0;
        // New player: the client restarts from default entity data (full air, no freeze).
        v.air = 300;
        v.sent_air = None;
        v.ticks_frozen = 0;
        v.sent_frozen = None;
        v.fire = -20;
        v.fall = 0.0;
        v.impulse_reset();
        v.cooldown = 0;
        v.last_damage = None;
        v.sent_health = None;
        v.sent_xp = None;
        v.on_fire_sent = false;
        v.crit = Default::default(); // new game player: criteria all rewritten
        if !keep {
            v.score = 0;
            v.sent_score = None;
        }
        // S7: the respawned player no longer has effects or absorption.
        self.reset_effects_for_respawn();
        // W8a: new player, equipment to rediscover; the client restarts from fresh attributes.
        self.life.attrs = crate::attrs::EquipAttrs::default();
        self.life.combat = crate::vitals::Combat::default();
        // G3-F08: the new player inherits the screen effects of the old one, sent again.
        self.pdata.post_effects_dirty = true;
        let height = rc_proto_v777::config::DimensionHeight {
            min_y: self.dim.height.min_y,
            height: self.dim.height.sections as i32 * 16,
            nether_light: self.data.dimension_types.iter().any(|t| t.name == self.dim.name && t.nether_light),
        };
        let flat = self.is_flat();
        let last_death = self.life.vit.last_death.clone();
        let _ = self.send(Event::Respawn { height, dimension: self.dim.name.clone(), flat, keep: 0, last_death });
        self.yaw = yaw;
        self.pitch = pitch;
        self.loaded = false;
        self.joined_tick = self.tick;
        self.mining = None; // new player: mining and tick counter restart from zero
        self.mine_ticks = 0;
        // New player entity: it joins the end of the world's entity list.
        self.walk.player = crate::walk::next_rank();
        // Demo: fresh state like the recreated player (the welcome screen returns 20 ticks later).
        self.demo = crate::demo::Demo::default();
        self.sent.clear();
        self.center = None;
        self.teleport_to(pos);
        self.last_good = pos;
        let _ = self.send(Event::Abilities(self.abilities()));
        let _ = self.send(Event::HeldSlot(self.held));
        // Level info like the game (border, clocks, spawn point, rain) AND the chunk
        // wait (event 13): without it the client's "Loading terrain" screen
        // stays displayed until its maximum delay (30 s).
        self.send_level_info();
        self.life_send_inventory();
        self.send_attributes();
        self.send_health();
        if let Some(m) = msg {
            let _ = self.send(Event::Chat {
                component: Box::new(rc_text::Component::translate(m, Vec::new())),
                tag: rc_client::ChatTag::System,
            });
        }
        self.update_tracking();
        // Client entities cleared: items and orbs re-announced.
        let drops = std::mem::take(&mut self.life.drops);
        for d in &drops {
            self.reannounce_item(d);
        }
        self.life.drops = drops;
    }

    pub(crate) fn reannounce_item(&self, d: &crate::drops::ItemEnt) {
        let kind = self
            .data
            .registry("minecraft:entity_type")
            .and_then(|r| r.entries.iter().position(|e| e == "minecraft:item"))
            .map_or(0, |i| i as i32);
        let add = rc_proto_v777::entity::AddEntity {
            id: d.id,
            uuid: d.id as u128,
            kind,
            pos: d.pos,
            velocity: d.vel,
            pitch: 0.0,
            yaw: d.yaw,
            head_yaw: 0.0,
            data: 0,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        let net = self.net(Some(&d.stack));
        let _ = self.send(Event::Entity(EntityEvent::Data {
            id: d.id,
            values: vec![(8, rc_proto_v777::entity::Meta::Item(net))],
        }));
    }

    pub(crate) fn is_flat(&self) -> bool {
        self.wgs.is_flat(&self.dim.name)
    }

    /// Respawn place (S6: bed, anchor or forced point in its dimension, otherwise world
    /// point; the destination dimension becomes active).
    fn respawn_point(&mut self) -> ([f64; 3], f32, f32, Option<&'static str>) {
        self.respawn_point_dims()
    }

    /// World point in the active dimension: free column around, radius `respawn_radius`.
    pub(crate) fn world_spawn_point(&mut self) -> ([f64; 3], f32, f32) {
        let s = self.level.spawn_pos;
        let (yaw, pitch) = (self.level.spawn_yaw, self.level.spawn_pitch);
        let at = |p: [i32; 3]| [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        // World in adventure: no search, height only corrected.
        if self.level.game_type == 2 {
            self.load_now(&[(s[0] >> 4, s[2] >> 4)]);
            return (at(self.spawn_fixup_height(s)), yaw, pitch);
        }
        // Radius capped by the distance to the border (at least 1 right next to it).
        let mut r = self.rules.get("minecraft:respawn_radius").and_then(Nbt::as_i64).unwrap_or(10).max(0) as i32;
        let k = self.border_bounds();
        let (x, z) = (f64::from(s[0]), f64::from(s[2]));
        let to_border = (x - k[0]).min(k[1] - x).min(z - k[2]).min(k[3] - z).floor() as i32;
        if to_border < r {
            r = to_border;
        }
        if to_border <= 1 {
            r = 1;
        }
        let chunks: Vec<(i32, i32)> = ((s[0] - r) >> 4..=(s[0] + r) >> 4)
            .flat_map(|x| ((s[2] - r) >> 4..=(s[2] + r) >> 4).map(move |z| (x, z)))
            .collect();
        self.load_now(&chunks); // S6: chunks generated right away if absent
        let side = i64::from(r) * 2 + 1;
        let n = (side * side).min(1024) as i32;
        let coprime = if n <= 16 { n - 1 } else { 17 };
        // Offset drawn on a thread-local random source in the game (not reproducible): player's random source.
        let offset = self.life.rng.next_int_bound(n.max(1));
        for i in 0..n {
            let v = (offset + coprime * i) % n;
            let (cx, cz) = (s[0] + v % (2 * r + 1) - r, s[2] + v / (2 * r + 1) - r);
            if let Some(p) = self.level_respawn_pos(cx, cz)
                && self.spawn_room_free(p)
            {
                return (at(p), yaw, pitch);
            }
        }
        (at(self.spawn_fixup_height(s)), yaw, pitch)
    }

    /// Place for a standing player at `p`: neither block nor liquid in their box.
    fn spawn_room_free(&self, p: [i32; 3]) -> bool {
        let b = rc_physics::Aabb {
            min: [f64::from(p[0]) + 0.2, f64::from(p[1]), f64::from(p[2]) + 0.2],
            max: [f64::from(p[0]) + 0.8, f64::from(p[1]) + 1.8, f64::from(p[2]) + 0.8],
        };
        rc_physics::is_free(&self.world(), &b) && !self.box_has_liquid(&b)
    }

    /// Corrected height: rises to a free spot, then goes back down to the ground.
    fn spawn_fixup_height(&self, s: [i32; 3]) -> [i32; 3] {
        // Highest block of the world included (min_y + height − 1).
        let (min, max) = (self.dim.height.min_y, self.dim.height.min_y + self.dim.height.sections as i32 * 16 - 1);
        let mut p = s;
        while !self.spawn_room_free(p) && p[1] < max {
            p[1] += 1;
        }
        p[1] -= 1;
        while self.spawn_room_free(p) && p[1] > min {
            p[1] -= 1;
        }
        p[1] += 1;
        p
    }

    /// Spawn point of a column: under the surface ("blocks movement" map, sea level for a
    /// world with a ceiling), refused if the surface is above the water floor, first
    /// block with a full top face going down (stop at the first liquid).
    fn level_respawn_pos(&self, x: i32, z: i32) -> Option<[i32; 3]> {
        let min = self.dim.height.min_y;
        let top = if self.dim_kind().ceiling { crate::mobs::surface::sea_level(self) } else { self.surface_y(x, z, 4) };
        if top < min {
            return None;
        }
        let surface = self.surface_y(x, z, 1);
        if surface <= top && surface > self.surface_y(x, z, 3) {
            return None;
        }
        for y in (min..=top + 1).rev() {
            let st = self.get([x, y, z]);
            if !self.fluid_of(st).is_empty() {
                break;
            }
            if self.face_full_up(st) {
                return Some([x, y + 1, z]);
            }
        }
        None
    }

    /// Hand actions (`player_action` 3 drop all, 4 drop one, 5 release, 6 swap).
    pub(crate) fn on_hand_action(&mut self, action: i32) {
        if self.game_mode == 3 && action != 5 {
            return;
        }
        match action {
            3 | 4 => {
                let slot = 36 + self.held as usize;
                let Some(mut s) = self.slots[slot].take() else { return };
                let out = if action == 3 || s.count <= 1 {
                    s
                } else {
                    let one = Stack { count: 1, ..s.clone() };
                    s.count -= 1;
                    self.slots[slot] = Some(s);
                    one
                };
                self.s9_dropped(&out); // S9
                self.drop_from_player_with(out, false);
                // The use only stops if the stack held in the main hand is emptied by the
                // throw, and without vibration (the item in use is then empty); dropping an item
                // from a stack that keeps some lets the use continue.
                if self.life.vit.using.is_some_and(|(h, _)| h == 0) && self.slots[slot].is_none() {
                    self.life.vit.using = None;
                }
            }
            5 => self.stop_using(),
            6 => {
                let main = 36 + self.held as usize;
                self.slots.swap(main, 45);
                self.stop_using();
            }
            _ => {}
        }
        self.broadcast_menus();
    }

    /// Slot set in creative (`set_creative_mode_slot`), components included.
    pub(crate) fn on_creative_slot(&mut self, slot: i16, stack: Option<Stack>) {
        if self.game_mode != 1 {
            return;
        }
        let valid = stack.as_ref().is_none_or(|s| s.count <= self.life.items.max_stack(s));
        if (1..=45).contains(&slot) && valid {
            // Placement by the player: the equipment slot reacts (sound, event).
            let before = crate::life::EQUIP_SLOTS.map(|k| self.slots[k].clone());
            self.slots[slot as usize] = stack.clone();
            self.equip_changed(&before);
            self.life.inv_menu.set_remote_exact(slot as usize, crate::menus::Remote::Exact(stack));
            self.broadcast_menus();
        } else if slot < 0
            && valid
            && let Some(s) = stack
        {
            // Throw from the hand: "dropped" and "items dropped" statistics.
            self.s9_dropped(&s);
            self.drop_from_player_with(s, false);
        }
    }

    /// `use_item`: start eating or drinking.
    pub(crate) fn on_use_item(&mut self, hand: i32) {
        if self.game_mode == 3 || self.life.vit.dead {
            return;
        }
        // F12: warped fungus on a stick (strider) or carrot on a stick (pig): boost.
        if self.stick_boost_use(hand) {
            return;
        }
        let slot = if hand == 0 { 36 + self.held as usize } else { 45 };
        let Some(s) = self.slots[slot].clone() else { return };
        let Some(info) = self.life.items.id(&s.id).and_then(|i| self.data.items.get(i as usize)) else { return };
        let Some(cons) = &info.consumable else { return };
        if let Some((_, _, always)) = self.life.items.food(&s)
            && !(always || self.life.vit.food < 20 || self.abilities().invulnerable)
        {
            return;
        }
        let ticks = (cons.seconds * 20.0) as i32;
        self.start_using(hand, ticks.max(0));
        if ticks <= 0 {
            self.finish_using();
        }
    }

    fn use_item_tick(&mut self) {
        let Some((hand, left)) = self.life.vit.using else { return };
        let slot = if hand == 0 { 36 + self.held as usize } else { 45 };
        // The hand no longer holds the item of the use (moved, slot changed by the item
        // choice): end of the use. A hand emptied by the use itself (last eye thrown)
        // keeps the use, which only counts down.
        if self.slots[slot].as_ref().map_or("", |s| s.id.as_str()) != self.life.vit.use_item {
            self.stop_using();
            return;
        }
        self.w8c_use_tick(hand, left); // W8c: bundle emptying
        // Use released during its own tick (brush without a targeted block): nothing else.
        if self.life.vit.using.is_none() {
            return;
        }
        // Completed when the countdown passes exactly to 0: a zero-duration use (eye of
        // Ender) goes negative and lasts until release, like in the game.
        if left == 1 {
            self.finish_using();
        } else {
            self.life.vit.using = Some((hand, left.wrapping_sub(1)));
        }
    }

    /// End of consumption: food, stack −1, remainder (bowl, bottle, bucket).
    /// Then end of the use: `item_interact_finish` vibration, unless the consumed stack is
    /// empty (last item eaten in survival).
    fn finish_using(&mut self) {
        let Some((hand, _)) = self.life.vit.using else { return };
        if self.finish_consuming(hand) && self.life.vit.use_vibrations {
            let pos = self.pos;
            self.game_event_at("minecraft:item_interact_finish", pos, Some(crate::mobs::mob::Ent::Player));
        }
    }

    /// Effects of consumption; true if the stack in use is not emptied.
    fn finish_consuming(&mut self, hand: i32) -> bool {
        self.life.vit.using = None;
        let slot = if hand == 0 { 36 + self.held as usize } else { 45 };
        let Some(mut s) = self.slots[slot].clone() else { return true };
        // W8c: an item without a consumable (shield raised for an hour) ends without effect.
        let Some(cons) = self.comp_json(&s, "minecraft:consumable") else {
            return true;
        };
        if let Some((nutrition, sat, _)) = self.life.items.food(&s) {
            // Food: the consumable's sound is broadcast to everyone (the eater included), neutral
            // category, volume 1, triangular pitch around 1 (spread 0.4), before the burp.
            let sound = match &cons["sound"] {
                serde_json::Value::String(t) => Some(t.as_str()),
                o => o["sound_id"].as_str(),
            }
            .unwrap_or("minecraft:entity.generic.eat")
            .to_owned();
            let rng = &mut self.life.rng;
            let pitch = 1.0 + 0.4 * (rng.next_float() - rng.next_float());
            self.play_sound(&sound, 6, self.pos, 1.0, pitch);
            let v = &mut self.life.vit;
            v.food = (v.food + nutrition).clamp(0, 20);
            v.saturation = (v.saturation + sat).clamp(0.0, v.food as f32);
            let pitch = self.life.rng.next_float() * 0.1 + 0.9;
            self.play_sound("minecraft:entity.player.burp", 7, self.pos, 0.5, pitch);
        }
        self.consume_effects(&s); // S7: potions, milk, golden apple…
        self.s9_consumed(&s); // S9: statistic, `consume_item`
        // `drink` or `eat` event depending on the consumable's animation.
        let drink = self
            .life
            .items
            .default_component(&s, "minecraft:consumable")
            .is_some_and(|c| c["animation"].as_str() == Some("drink"));
        let pos = self.pos;
        let ev = if drink { "minecraft:drink" } else { "minecraft:eat" };
        self.game_event_at(ev, pos, Some(crate::mobs::mob::Ent::Player));
        let remainder = self.life.items.default_component(&s, "minecraft:use_remainder").and_then(|r| {
            Some(Stack {
                id: r["id"].as_str()?.to_owned(),
                count: r["count"].as_i64().unwrap_or(1) as i32,
                components: None,
            })
        });
        let _ = self.send(Event::Entity(EntityEvent::Event { id: self.player_id, event: 9 }));
        if self.game_mode == 1 {
            return true;
        }
        s.count -= 1;
        let left = s.count > 0;
        self.slots[slot] = left.then_some(s);
        if let Some(r) = remainder {
            if self.slots[slot].is_none() {
                self.slots[slot] = Some(r);
            } else {
                let creative = self.game_mode == 1;
                let held = self.held as usize;
                if let Some(rest) = crate::menus::inventory_add(&mut self.slots, &self.life.items, held, r, creative) {
                    self.drop_from_player_with(rest, false);
                }
            }
        }
        self.broadcast_menus();
        left
    }

    /// Game mode change (§ 1.6).
    pub(crate) fn set_game_mode(&mut self, mode: i32) {
        if !(0..4).contains(&mode) || mode == self.game_mode {
            return;
        }
        self.pdata.previous_game_mode = Some(self.game_mode);
        self.game_mode = mode;
        // Effect particles hang on the mode (none for a spectator): sent again if they change.
        self.life.fx.dirty = true;
        if mode != 1 && mode != 3 {
            self.flying = false;
        }
        if mode == 3 {
            self.flying = true;
        }
        let _ = self.send(Event::GameEvent { kind: 3, param: mode as f32 });
        if mode == 1 {
            self.life.vit.impulse_reset();
        }
        if mode == 3 {
            // Dismount (stop riding), after the mode announcement.
            self.player_stop_riding();
            self.stop_using();
            self.cushion_stand_up();
        }
        let _ = self.send(Event::Abilities(self.abilities()));
        use rc_proto_v777::players::{InfoEntry, InfoUpdate, action};
        let _ = self.send(Event::PlayerInfo(InfoUpdate {
            actions: action::UPDATE_GAME_MODE,
            entries: vec![InfoEntry { uuid: self.uuid, game_mode: Some(mode), ..Default::default() }],
        }));
        // A spectator no longer counts among the active sleeping players.
        self.update_sleeping_list();
        self.send_attributes();
    }
}

/// Seed of a positional sound (variant and drift chosen by the client): sound-specific
/// generator, without a fixed seed, independent of the world's random source (like the game's).
/// In tests, a fixed per-thread sequence, distinct from `rng::test_seed` (a sound shifts no
/// other seed): same packets from one run to the next.
pub(crate) fn sound_seed() -> i64 {
    const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
    #[cfg(not(test))]
    let base = {
        use std::sync::OnceLock;
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        static SALT: OnceLock<u64> = OnceLock::new();
        let salt = *SALT.get_or_init(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0x5DEE_CE66, |d| d.as_nanos() as u64)
        });
        NEXT.fetch_add(GOLDEN, Ordering::Relaxed) ^ salt
    };
    #[cfg(test)]
    let base = {
        thread_local! {
            static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
        }
        NEXT.with(|c| {
            let v = c.get();
            c.set(v.wrapping_add(GOLDEN));
            v ^ 0x500D_5EED
        })
    };
    // SplitMix64.
    let mut z = base.wrapping_add(GOLDEN);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as i64
}

#[cfg(test)]
mod tests_last_death {
    use super::*;

    /// Last place of death reread from the save (as written), absent otherwise.
    #[test]
    fn last_death_round_trip() {
        let mut root = Nbt::Compound(Vec::new());
        let mut v = Vitals::read(&root);
        assert_eq!(v.last_death, None);
        v.last_death = Some(("minecraft:the_nether".into(), [12, 40, -7]));
        v.write(&mut root);
        assert_eq!(Vitals::read(&root).last_death, Some(("minecraft:the_nether".into(), [12, 40, -7])));
    }
}