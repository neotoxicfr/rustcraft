//! Weather and sleep (`integrated-server.md` § 7, `server-gameplay.md` § 6.3 `/weather`): the game's
//! rain and thunderstorm cycle (`weather.dat`), levels sent to the client (`game_event` 1, 2, 7,
//! 8), bed (respawn point, sleep that skips the night to the "wake-up" marker).

use std::path::{Path, PathBuf};

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use crate::blocks::{Dir, P, rel};
use crate::nbtio::{int, set};
use crate::respawn::SleepRule;
use crate::server::Server;

#[derive(Debug, Clone)]
pub struct Weather {
    pub raining: bool,
    pub thundering: bool,
    pub rain_time: i32,
    pub thunder_time: i32,
    pub clear_time: i32,
    pub rain_level: f32,
    pub thunder_level: f32,
    loaded: (bool, bool, i32, i32, i32),
    /// List of the world's sleepers (active players, in bed), sampled at changes.
    pub sleep: (i32, i32),
}

fn path(world: &Path) -> PathBuf {
    world.join("data").join("minecraft").join("weather.dat")
}

impl Weather {
    pub fn read(world: &Path) -> Weather {
        let d = crate::nbtio::read_dat(&path(world)).ok().and_then(|v| v.get("data").cloned());
        let i = |k: &str| d.as_ref().and_then(|d| int(d, k)).unwrap_or(0);
        let raining = i("raining") != 0;
        let thundering = i("thundering") != 0;
        let (rain_time, thunder_time, clear_time) =
            (i("rain_time") as i32, i("thunder_time") as i32, i("clear_weather_time") as i32);
        Weather {
            raining,
            thundering,
            rain_time,
            thunder_time,
            clear_time,
            rain_level: if raining { 1.0 } else { 0.0 },
            thunder_level: if raining && thundering { 1.0 } else { 0.0 },
            loaded: (raining, thundering, rain_time, thunder_time, clear_time),
            sleep: (0, 0),
        }
    }

    /// Effective thunder level: thunder multiplied by rain (the two timers are independent).
    pub fn thunder_eff(&self) -> f32 {
        self.thunder_level * self.rain_level
    }

    fn state(&self) -> (bool, bool, i32, i32, i32) {
        (self.raining, self.thundering, self.rain_time, self.thunder_time, self.clear_time)
    }

    /// `weather.dat` to write if it changed.
    pub fn to_nbt(&mut self, world: &Path) -> Option<(PathBuf, Nbt)> {
        if self.state() == self.loaded {
            return None;
        }
        self.loaded = self.state();
        let p = path(world);
        let mut root = crate::nbtio::read_dat(&p).unwrap_or(Nbt::Compound(Vec::new()));
        let mut d = root.get("data").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        set(&mut d, "raining", Nbt::Byte(i8::from(self.raining)));
        set(&mut d, "thundering", Nbt::Byte(i8::from(self.thundering)));
        set(&mut d, "rain_time", Nbt::Int(self.rain_time));
        set(&mut d, "thunder_time", Nbt::Int(self.thunder_time));
        set(&mut d, "clear_weather_time", Nbt::Int(self.clear_time));
        set(&mut root, "data", d);
        set(&mut root, "DataVersion", Nbt::Int(crate::chunk::DATA_VERSION));
        Some((p, root))
    }
}

impl Server {
    fn has_weather(&self) -> bool {
        // Dimensions with a sky (Overworld): rain exists there.
        self.dim_kind().weather && self.dim.name != "minecraft:the_end"
    }

    /// Full weather state (entering the game, respawn).
    pub(crate) fn send_weather_state(&self) {
        let w = &self.life.weather;
        if w.rain_level > 0.0 || w.raining {
            let _ = self.send(Event::GameEvent { kind: 1, param: 0.0 });
            let _ = self.send(Event::GameEvent { kind: 7, param: w.rain_level });
            let _ = self.send(Event::GameEvent { kind: 8, param: w.thunder_level });
        }
    }

    /// Advancement of the weather cycle.
    pub(crate) fn weather_tick(&mut self) {
        let was_raining = self.life.weather.rain_level > 0.2;
        if self.has_weather() {
            let advance = self.rule("minecraft:advance_weather", true);
            let rng = &mut self.life.rng;
            let w = &mut self.life.weather;
            if advance {
                if w.clear_time > 0 {
                    w.clear_time -= 1;
                    w.thunder_time = if w.thundering { 0 } else { 1 };
                    w.rain_time = if w.raining { 0 } else { 1 };
                    w.thundering = false;
                    w.raining = false;
                } else {
                    if w.thunder_time > 0 {
                        w.thunder_time -= 1;
                        if w.thunder_time == 0 {
                            w.thundering = !w.thundering;
                        }
                    } else if w.thundering {
                        w.thunder_time = rng.int_between(3600, 15600);
                    } else {
                        w.thunder_time = rng.int_between(12000, 180000);
                    }
                    if w.rain_time > 0 {
                        w.rain_time -= 1;
                        if w.rain_time == 0 {
                            w.raining = !w.raining;
                        }
                    } else if w.raining {
                        w.rain_time = rng.int_between(12000, 24000);
                    } else {
                        w.rain_time = rng.int_between(12000, 180000);
                    }
                }
            }
            let (o_rain, o_thunder) = (w.rain_level, w.thunder_level);
            w.thunder_level = (w.thunder_level + if w.thundering { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
            w.rain_level = (w.rain_level + if w.raining { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
            let (rain, thunder) = (w.rain_level, w.thunder_level);
            if o_rain != rain {
                let _ = self.send(Event::GameEvent { kind: 7, param: rain });
            }
            if o_thunder != thunder {
                // Raw thunder level: the client multiplies it by rain itself.
                let _ = self.send(Event::GameEvent { kind: 8, param: thunder });
            }
        }
        let raining = self.life.weather.rain_level > 0.2;
        if was_raining != raining {
            let _ = self.send(Event::GameEvent { kind: if was_raining { 2 } else { 1 }, param: 0.0 });
            let _ = self.send(Event::GameEvent { kind: 7, param: self.life.weather.rain_level });
            let _ = self.send(Event::GameEvent { kind: 8, param: self.life.weather.thunder_level });
        }
    }

    /// Time of day (Overworld clock, 0..24000).
    pub(crate) fn day_time(&self) -> i64 {
        self.clocks.iter().find(|(n, _)| n == "minecraft:overworld").map_or(self.time, |c| c.1).rem_euclid(24000)
    }

    /// It is dark outside (bed rule "when it is dark"): sky darkening of at least 4
    /// (time, rain and thunder included), never in a dimension with a fixed time.
    pub(crate) fn is_dark_outside(&self) -> bool {
        self.has_weather() && self.mob_sky_darken() >= 4
    }

    /// Clicked bed half: (head, orientation) if the other half is the same block, of the other
    /// part; otherwise `None` (the click is consumed with no effect).
    fn bed_head(&self, pos: P) -> Option<(P, Dir)> {
        let s = self.get(pos);
        let v = self.v();
        let facing = v.get_dir(s, "facing")?;
        let foot = v.get(s, "part") == Some("foot");
        let other = rel(pos, if foot { facing } else { facing.opposite() });
        let o = self.get(other);
        let ok = v.block(o) == v.block(s) && v.get(o, "part").is_some() && v.get(o, "part") != v.get(s, "part");
        ok.then_some((if foot { other } else { pos }, facing))
    }

    /// Sleeping height of a bed at `head`: top of its shape (straw bed: that of its
    /// foot, absent if the head no longer has its foot); `None` if it is not a bed.
    pub(crate) fn bed_sleep_height(&self, head: P) -> Option<f64> {
        let s = self.get(head);
        let v = self.v();
        let name = self.data.block_of(s)?.name.as_str();
        if !name.ends_with("_bed") {
            return None;
        }
        let mut on = s;
        if name == "minecraft:straw_bed" && v.get(s, "part") == Some("head") {
            let foot = self.get(rel(head, v.get_dir(s, "facing")?.opposite()));
            if v.block(foot) != v.block(s) || v.get(foot, "part") != Some("foot") {
                return None;
            }
            on = foot;
        }
        let mut boxes = Vec::new();
        self.phys.collision(on, &mut boxes);
        boxes.iter().map(|b| b.max[1]).reduce(f64::max)
    }

    fn overlay(&self, c: rc_text::Component) {
        let _ = self.send(Event::ActionBar(c));
    }

    fn overlay_key(&self, key: &str) {
        self.overlay(rc_text::Component::translate(key, Vec::new()));
    }

    /// Right click on a bed: dimension bed rule (destruction on use), occupied bed,
    /// then lying down.
    pub(crate) fn use_bed(&mut self, pos: P) -> bool {
        let Some((head, facing)) = self.bed_head(pos) else { return true };
        let straw = self.bname(head) == "minecraft:straw_bed";
        let dim = self.dim.name.clone();
        let rule = self.bed_rule(&dim, straw);
        if rule.destroy_on_use {
            if let Some(c) = rule.error.as_ref().and_then(rc_client::status::json_component) {
                self.overlay(c);
            }
            self.bed_destroy_on_use(head);
            return true;
        }
        if self.prop(head, "occupied").as_deref() == Some("true") {
            if !self.kick_villager_out_of_bed(head) {
                self.overlay_key("block.minecraft.bed.occupied");
            }
            return true;
        }
        self.start_sleep_in_bed(head, facing, &rule, straw);
        true
    }

    /// Attempt to lie down (current player): range, obstruction, respawn point according to
    /// the rule, night, monsters, then in bed; the sleepers list is always updated.
    fn start_sleep_in_bed(&mut self, head: P, facing: Dir, rule: &SleepRule, straw: bool) {
        if self.life.vit.sleeping.is_some() || self.life.vit.dead {
            return;
        }
        let can_sleep = self.bed_when(rule.can_sleep);
        let can_set_spawn = self.bed_when(rule.can_set_spawn);
        // Range: head OR foot within |dx| ≤ 3, |dy| ≤ 2, |dz| ≤ 3 of the bottom centre of the block.
        let foot_pos = rel(head, facing.opposite());
        let reach = |b: P| {
            (self.pos[0] - (f64::from(b[0]) + 0.5)).abs() <= 3.0
                && (self.pos[1] - f64::from(b[1])).abs() <= 2.0
                && (self.pos[2] - (f64::from(b[2]) + 0.5)).abs() <= 3.0
        };
        if !reach(head) && !reach(foot_pos) {
            self.overlay_key("block.minecraft.bed.too_far_away");
            return;
        }
        // Obstruction: the two cells above the bed must not suffocate.
        let blocked = [head, foot_pos].iter().any(|b| {
            let above = [b[0], b[1] + 1, b[2]];
            self.state_at(above).is_some_and(|s| self.suffocating(s))
        });
        if blocked {
            self.overlay_key("block.minecraft.bed.obstructed");
            return;
        }
        if can_set_spawn {
            // Respawn point taken (orientation included), message if it changed.
            let same = self.life.vit.respawn.as_ref().is_some_and(|r| r.1 == head && r.0 == self.dim.name);
            self.life.vit.respawn = Some((self.dim.name.clone(), head, self.yaw, self.pitch, false));
            if !same {
                let _ = self.send(Event::Chat {
                    component: rc_text::Component::translate("block.minecraft.set_spawn", Vec::new()),
                    tag: rc_client::ChatTag::System,
                });
            }
        }
        if !can_sleep {
            if let Some(c) = rule.error.as_ref().and_then(rc_client::status::json_component) {
                self.overlay(c);
            }
            return;
        }
        if self.game_mode != 1 && self.monster_prevents_rest(head) {
            self.overlay_key("block.minecraft.bed.not_safe");
            return;
        }
        if self.player_start_sleeping(head) {
            self.s9_slept(straw, can_set_spawn); // S9
        }
        if !self.players_can_skip_night() {
            self.overlay_key("sleep.not_possible");
        }
        self.update_sleeping_list();
    }

    /// In bed (current player): position on the bed (sleeping height + 0.125), bed occupied,
    /// "sleeping" pose, time since rest reset to zero; false if the bed has no height.
    fn player_start_sleeping(&mut self, head: P) -> bool {
        let Some(h) = self.bed_sleep_height(head) else { return false };
        self.player_stop_riding();
        self.life.vit.sleeping = Some(head);
        self.life.vit.sleep_timer = 0;
        self.reset_stat("minecraft:custom", "minecraft:time_since_rest");
        let at = [f64::from(head[0]) + 0.5, f64::from(head[1]) + h + 0.125, f64::from(head[2]) + 0.5];
        self.teleport_to(at);
        self.set_bed_occupied(head, true);
        let _ = self.send(Event::Entity(EntityEvent::Data {
            id: self.player_id,
            values: vec![(6, Meta::Pose(2)), (14, Meta::BlockPos(Some(head)))],
        }));
        true
    }

    /// Does the required percentage of sleepers allow skipping the night (≤ 100 %).
    fn players_can_skip_night(&self) -> bool {
        self.sleep_percentage() <= 100
    }

    fn sleep_percentage(&self) -> i32 {
        self.rules.get("minecraft:players_sleeping_percentage").and_then(Nbt::as_i64).unwrap_or(100) as i32
    }

    /// A monster (the game's "monster" family) in the 16×10×16 box centred on the bottom of the
    /// bed prevents resting; the zombified piglin only if it is angry at the player.
    pub(crate) fn monster_prevents_rest(&self, head: [i32; 3]) -> bool {
        let c = [f64::from(head[0]) + 0.5, f64::from(head[1]), f64::from(head[2]) + 0.5];
        let area =
            rc_physics::Aabb { min: [c[0] - 8.0, c[1] - 5.0, c[2] - 8.0], max: [c[0] + 8.0, c[1] + 5.0, c[2] + 8.0] };
        let universal = self.rule("minecraft:universal_anger", false);
        self.mobs.list.iter().flatten().any(|m| {
            m.alive()
                && rest_breaker(m.kind)
                && m.touches(&area)
                && (m.kind != crate::mobs::species::Kind::ZombifiedPiglin || {
                    let angry = crate::mobs::piglins::zombified::is_angry(self, m);
                    m.pig.anger_uuid == Some(self.uuid) || (universal && angry && m.pig.anger_uuid.is_none())
                })
        })
    }

    /// A villager sleeping in this bed is woken (true if there was one).
    fn kick_villager_out_of_bed(&mut self, head: [i32; 3]) -> bool {
        let Some(i) = self.mobs.list.iter().position(|m| {
            m.as_ref()
                .is_some_and(|m| m.kind == crate::mobs::species::Kind::Villager && self.npc_sleeping(m) == Some(head))
        }) else {
            return false;
        };
        let Some(mut m) = self.mobs.list[i].take() else { return false };
        if let Some(mut b) = m.npc.as_mut().and_then(|n| n.brain.take()) {
            self.stop_sleeping(&mut m, &mut b);
            if let Some(n) = m.npc.as_mut() {
                n.brain = Some(b);
            }
        }
        self.mobs.list[i] = Some(m);
        true
    }

    pub(crate) fn set_bed_occupied(&mut self, head: [i32; 3], occupied: bool) {
        if let Some(s) = self.state_at(head)
            && let Some(n) = self.with_prop(s, "occupied", if occupied { "true" } else { "false" })
        {
            self.set_update(head, n); // flags 3 like the game
        }
    }

    /// Getting out of a bed (mob or player): bed freed, destruction on waking according to the
    /// rule (straw bed), then wake-up position next to it (box `dims`, yaw `yaw`) or, failing that,
    /// above the bed; returns (position, yaw turned towards the bed), `None` if the bed
    /// is no longer there or its chunk is not loaded.
    pub(crate) fn leave_bed(&mut self, bed: P, yaw: f32, dims: (f32, f32), player: bool) -> Option<([f64; 3], f32)> {
        if !self.is_loaded(bed) {
            return None;
        }
        let s = self.get(bed);
        let name = self.data.block_of(s)?.name.clone();
        if !name.ends_with("_bed") {
            return None;
        }
        let facing = self.v().get_dir(s, "facing")?;
        self.set_bed_occupied(bed, false);
        let straw = name == "minecraft:straw_bed";
        let dim = self.dim.name.clone();
        if self.bed_rule(&dim, straw).destroy_on_leave {
            // Only the straw bed is destroyed on waking (the ordinary bed does not have this effect).
            if straw {
                self.straw_bed_break(bed);
            }
        }
        let stand = self.bed_stand_up_sized(bed, facing, yaw, dims, player).unwrap_or_else(|| {
            // Block above, then 0.1: same float as the game (y + 1.1 differs by one ulp at y = -1).
            [f64::from(bed[0]) + 0.5, f64::from(bed[1] + 1) + 0.1, f64::from(bed[2]) + 0.5]
        });
        Some((stand, crate::respawn::look_at_yaw(stand, bed)))
    }

    /// Unforced waking of the current player (leaving the bed requested, day returned): the sleepers
    /// list is updated.
    pub(crate) fn wake_up(&mut self) {
        self.stop_sleep_in_bed(false, true);
    }

    /// Waking of the current player: arm animation (0) seen by everyone, getting up, standing pose, then
    /// teleport (position and look); `update_list`: sleepers list updated.
    /// Exit sleep counter: 0 on a forced waking (bed gone, departure), 100 otherwise
    /// (it drops back to 0 ten ticks later, see `sleep_tick`).
    pub(crate) fn stop_sleep_in_bed(&mut self, forced: bool, update_list: bool) {
        let Some(head) = self.life.vit.sleeping else { return };
        let _ = self.send(Event::Entity(EntityEvent::Animate { id: self.player_id, action: 0 }));
        if let Some((stand, yaw)) = self.leave_bed(head, self.yaw, crate::respawn::PLAYER_DIMS, true) {
            self.pos = stand;
            self.yaw = yaw;
            self.pitch = 0.0;
        }
        self.life.vit.sleeping = None;
        let _ = self.send(Event::Entity(EntityEvent::Data {
            id: self.player_id,
            values: vec![(6, Meta::Pose(0)), (14, Meta::BlockPos(None))],
        }));
        if update_list {
            self.update_sleeping_list();
        }
        self.life.vit.sleep_timer = if forced { 0 } else { 100 };
        self.teleport_to(self.pos);
    }

    /// Tick of the current player: sleep counter (capped at 100 ticks); the sleeper gets up
    /// if their bed has disappeared (forced waking), or if the bed rule no longer allows sleeping there
    /// (day returned). Standing, a non-zero counter advances up to 110 then drops back to 0.
    pub(crate) fn sleep_tick(&mut self) {
        let Some(bed) = self.life.vit.sleeping else {
            let t = &mut self.life.vit.sleep_timer;
            if *t > 0 {
                *t += 1;
                if *t >= 110 {
                    *t = 0;
                }
            }
            return;
        };
        self.life.vit.sleep_timer = (self.life.vit.sleep_timer + 1).min(100);
        if self.bed_sleep_height(bed).is_none() {
            self.stop_sleep_in_bed(true, true);
            return;
        }
        let straw = self.bname(bed) == "minecraft:straw_bed";
        let dim = self.dim.name.clone();
        if !self.bed_when(self.bed_rule(&dim, straw).can_sleep) {
            self.wake_up();
        }
    }

    /// Players of the world: (game mode, survival state) of the host and the guests who entered (the
    /// current player included during a swap).
    fn sleepers(&self) -> Vec<(i32, bool, i32)> {
        let mut out = vec![(self.game_mode, self.life.vit.sleeping.is_some(), self.life.vit.sleep_timer)];
        if let Some(l) = &self.lan {
            for (k, g) in l.guests.iter().enumerate() {
                if Some(k) != l.active && (!g.joined || g.gone) {
                    continue;
                }
                if let Some(c) = &g.ctx {
                    out.push((c.game_mode, c.vit.sleeping.is_some(), c.vit.sleep_timer));
                }
            }
        }
        out
    }

    /// List of the world's sleepers (active players = non-spectators, and in bed among them),
    /// sampled on lying down, getting up, a player's arrival or departure and on
    /// mode change; announces if it changed while someone sleeps or slept.
    pub(crate) fn update_sleeping_list(&mut self) {
        let (mut active, mut sleeping) = (0, 0);
        for (mode, asleep, _) in self.sleepers() {
            if mode != 3 {
                active += 1;
                sleeping += i32::from(asleep);
            }
        }
        let (old_active, old_sleeping) = self.life.weather.sleep;
        self.life.weather.sleep = (active, sleeping);
        if (old_sleeping > 0 || sleeping > 0) && (old_active != active || old_sleeping != sleeping) {
            self.announce_sleep_status();
        }
    }

    /// Sleepers required: `max(1, ⌈active × percentage / 100⌉)` (the game's integer product,
    /// overflow included, then float division).
    fn sleepers_needed(&self, pct: i32) -> i32 {
        ((self.life.weather.sleep.0.wrapping_mul(pct) as f32 / 100.0).ceil() as i32).max(1)
    }

    /// Announcement to the world's players (only a world open to the LAN, and a skippable night):
    /// night skipped, or "n/m players sleeping".
    fn announce_sleep_status(&self) {
        if !self.players_can_skip_night() || self.lan.is_none() {
            return;
        }
        let pct = self.sleep_percentage();
        let needed = self.sleepers_needed(pct);
        let sleeping = self.life.weather.sleep.1;
        let msg = if sleeping >= needed {
            rc_text::Component::translate("sleep.skipping_night", Vec::new())
        } else {
            let n = |v: i32| rc_text::Arg::Text(v.to_string());
            rc_text::Component::translate("sleep.players_sleeping", vec![n(sleeping), n(needed)])
        };
        self.send_all_players(Event::ActionBar(msg));
    }

    /// Night skip, decided once per world tick (after the weather, before game time):
    /// enough players in bed according to the sleepers list AND enough in deep sleep (100
    /// ticks, spectators included); the night then passes to the "wake-up" marker (multiple of
    /// 24,000), all the sleepers get up (without sampling the list, emptied), then the rain
    /// stops.
    pub(crate) fn sleep_world_tick(&mut self) {
        let pct = self.sleep_percentage();
        let needed = self.sleepers_needed(pct);
        if self.life.weather.sleep.1 < needed {
            return;
        }
        let deep = self.sleepers().iter().filter(|(_, asleep, t)| *asleep && *t >= 100).count() as i32;
        if deep < needed {
            return;
        }
        if self.rule("minecraft:advance_time", true) {
            for c in &mut self.clocks {
                if c.0 == "minecraft:overworld" && c.1.rem_euclid(24000) != 0 {
                    c.1 += 24000 - c.1.rem_euclid(24000);
                }
            }
            self.send_time(true);
        }
        self.life.weather.sleep.1 = 0;
        if self.life.vit.sleeping.is_some() {
            self.stop_sleep_in_bed(false, false);
        }
        let n = self.lan.as_ref().map_or(0, |l| l.guests.len());
        for i in 0..n {
            let asleep = self.lan.as_ref().and_then(|l| l.guests[i].ctx.as_ref()).is_some_and(|c| {
                c.vit.sleeping.is_some()
            });
            let entered = self.lan.as_ref().is_some_and(|l| l.guests[i].joined && !l.guests[i].gone);
            if asleep && entered {
                self.as_guest(i, |sv| sv.stop_sleep_in_bed(false, false));
            }
        }
        if self.rule("minecraft:advance_weather", true) && self.life.weather.raining {
            let w = &mut self.life.weather;
            w.rain_time = 0;
            w.raining = false;
            w.thunder_time = 0;
            w.thundering = false;
        }
    }
}

/// Species of the game's "monster" family (those that prevent sleeping); neither slimes,
/// ghasts, phantoms, shulkers, nor hoglins are among them.
fn rest_breaker(k: crate::mobs::species::Kind) -> bool {
    use crate::mobs::species::Kind::*;
    matches!(
        k,
        Zombie
            | Husk
            | Drowned
            | ZombieVillager
            | Giant
            | ZombifiedPiglin
            | Skeleton
            | Stray
            | Bogged
            | Parched
            | WitherSkeleton
            | Creeper
            | Spider
            | CaveSpider
            | Enderman
            | Endermite
            | Silverfish
            | Blaze
            | Breeze
            | Creaking
            | Guardian
            | ElderGuardian
            | Witch
            | Warden
            | Wither
            | Zoglin
            | Pillager
            | Vindicator
            | Evoker
            | Illusioner
            | Ravager
            | Vex
            | Piglin
            | PiglinBrute
    )
}
