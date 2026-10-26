//! Statistics on the integrated server side (mission S9, `docs/specs/advancements-stats.md` § 2 and § 8):
//! counters per type (`mined`, `crafted`, `used`, `broken`, `picked_up`, `dropped`, `killed`,
//! `killed_by`, `custom`), linked scoreboard objectives, `award_stats` on client request
//! (only the values changed since the last send), `stats/<uuid>.json` file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rc_client::Event;
use serde_json::Value;

use crate::server::Server;

/// Statistic types in the order of the `stat_type` registry.
pub const TYPES: [&str; 9] = [
    "minecraft:mined",
    "minecraft:crafted",
    "minecraft:used",
    "minecraft:broken",
    "minecraft:picked_up",
    "minecraft:dropped",
    "minecraft:killed",
    "minecraft:killed_by",
    "minecraft:custom",
];

/// Registry of the values of a type.
fn value_registry(ty: &str) -> &'static str {
    match ty {
        "minecraft:mined" => "minecraft:block",
        "minecraft:killed" | "minecraft:killed_by" => "minecraft:entity_type",
        "minecraft:custom" => "minecraft:custom_stat",
        _ => "minecraft:item",
    }
}

/// Custom statistics in registry order (fallback if the data has none).
pub const CUSTOM: [&str; 78] = [
    "leave_game",
    "play_time",
    "total_world_time",
    "time_since_death",
    "time_since_rest",
    "sneak_time",
    "walk_one_cm",
    "crouch_one_cm",
    "sprint_one_cm",
    "walk_on_water_one_cm",
    "fall_one_cm",
    "climb_one_cm",
    "fly_one_cm",
    "walk_under_water_one_cm",
    "minecart_one_cm",
    "boat_one_cm",
    "pig_one_cm",
    "happy_ghast_one_cm",
    "horse_one_cm",
    "aviate_one_cm",
    "swim_one_cm",
    "strider_one_cm",
    "nautilus_one_cm",
    "jump",
    "drop",
    "damage_dealt",
    "damage_dealt_absorbed",
    "damage_dealt_resisted",
    "damage_taken",
    "damage_blocked_by_shield",
    "damage_absorbed",
    "damage_resisted",
    "deaths",
    "mob_kills",
    "animals_bred",
    "player_kills",
    "fish_caught",
    "talked_to_villager",
    "traded_with_villager",
    "eat_cake_slice",
    "fill_cauldron",
    "use_cauldron",
    "clean_armor",
    "clean_banner",
    "clean_shulker_box",
    "interact_with_brewingstand",
    "interact_with_beacon",
    "inspect_dropper",
    "inspect_hopper",
    "inspect_dispenser",
    "play_noteblock",
    "tune_noteblock",
    "pot_flower",
    "trigger_trapped_chest",
    "open_enderchest",
    "enchant_item",
    "play_record",
    "interact_with_furnace",
    "interact_with_crafting_table",
    "open_chest",
    "sleep_in_bed",
    "sleep_in_straw_bed",
    "open_shulker_box",
    "open_barrel",
    "interact_with_blast_furnace",
    "interact_with_smoker",
    "interact_with_lectern",
    "interact_with_campfire",
    "interact_with_cartography_table",
    "interact_with_loom",
    "interact_with_stonecutter",
    "bell_ring",
    "raid_trigger",
    "raid_win",
    "interact_with_anvil",
    "interact_with_grindstone",
    "target_hit",
    "interact_with_smithing_table",
];

/// Counters of a player.
#[derive(Debug, Default)]
pub struct Stats {
    /// (type, value) → count.
    pub map: BTreeMap<(String, String), i32>,
    /// Changed since the last send (`dirty`).
    pub dirty: BTreeSet<(String, String)>,
    pub path: PathBuf,
}

impl Stats {
    pub fn read(world: &Path, uuid: u128) -> Stats {
        // 26.3: `players/stats/`.
        let path = world.join("players").join("stats").join(format!("{}.json", crate::nbtio::uuid_text(uuid)));
        let mut s = Stats { path, ..Stats::default() };
        if let Ok(bytes) = std::fs::read(&s.path) {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(v) => {
                    if let Some(Value::Object(types)) = v.get("stats") {
                        for (ty, vals) in types {
                            let Value::Object(vals) = vals else { continue };
                            for (k, n) in vals {
                                if let Some(n) = n.as_i64() {
                                    s.map.insert((id(ty), id(k)), n as i32);
                                }
                            }
                        }
                    }
                }
                Err(e) => eprintln!("singleplayer: statistics unreadable ({}): {e}", s.path.display()),
            }
        }
        s
    }

    pub fn get(&self, ty: &str, key: &str) -> i32 {
        self.map.get(&(ty.to_owned(), key.to_owned())).copied().unwrap_or(0)
    }

    pub fn set(&mut self, ty: &str, key: &str, v: i32) {
        let k = (ty.to_owned(), key.to_owned());
        self.map.insert(k.clone(), v);
        self.dirty.insert(k);
    }

    /// All to resend (entering the game).
    pub fn mark_all(&mut self) {
        self.dirty = self.map.keys().cloned().collect();
    }

    /// JSON file (the game's form: `stats` → type → value → count, then `DataVersion`).
    pub fn to_json(&self, data_version: i32) -> String {
        let mut by_type: BTreeMap<&str, Vec<(&str, i32)>> = BTreeMap::new();
        for ((t, k), v) in &self.map {
            by_type.entry(t.as_str()).or_default().push((k.as_str(), *v));
        }
        let mut out = String::from("{\n  \"stats\": {");
        let mut first = true;
        for (t, vals) in by_type {
            out.push_str(if first { "\n" } else { ",\n" });
            first = false;
            out.push_str(&format!("    \"{t}\": {{"));
            let mut f2 = true;
            for (k, v) in vals {
                out.push_str(if f2 { "\n" } else { ",\n" });
                f2 = false;
                out.push_str(&format!("      \"{k}\": {v}"));
            }
            out.push_str("\n    }");
        }
        if first {
            out.push('}');
        } else {
            out.push_str("\n  }");
        }
        out.push_str(&format!(",\n  \"DataVersion\": {data_version}\n}}"));
        out
    }
}

fn id(s: &str) -> String {
    crate::commands::args::id(s)
}

/// Scoreboard criterion name of a statistic (`minecraft.mined:minecraft.stone`).
pub fn criteria_name(ty: &str, key: &str) -> String {
    format!("{}:{}", ty.replace(':', "."), key.replace(':', "."))
}

impl Server {
    /// Value of a player statistic.
    pub(crate) fn stat_value(&self, ty: &str, key: &str) -> i32 {
        self.prog.stats.get(ty, key)
    }

    /// Adds `n` (capped at `i32::MAX`) and updates the objectives with the same criterion.
    pub(crate) fn award_stat(&mut self, ty: &str, key: &str, n: i32) {
        if n == 0 {
            return;
        }
        let v = (i64::from(self.prog.stats.get(ty, key)) + i64::from(n)).min(i64::from(i32::MAX)) as i32;
        self.prog.stats.set(ty, key, v);
        if !self.cmd.scores.objectives.is_empty() {
            self.score_criteria_add(&criteria_name(ty, key), n);
        }
    }

    pub(crate) fn award_custom(&mut self, key: &str, n: i32) {
        self.award_stat("minecraft:custom", &format!("minecraft:{key}"), n);
    }

    /// Reset to zero: the scores of this criterion are 0 (created as needed, never
    /// erased).
    pub(crate) fn reset_stat(&mut self, ty: &str, key: &str) {
        self.prog.stats.set(ty, key, 0);
        if !self.cmd.scores.objectives.is_empty() {
            let holder = self.name.clone();
            self.criteria_update(&criteria_name(ty, key), &holder, |_| 0);
        }
    }

    /// Addition to the player's scores for the objectives of this criterion.
    pub(crate) fn score_criteria_add(&mut self, crit: &str, n: i32) {
        if !self.cmd.scores.objectives.iter().any(|o| o.criteria == crit) {
            return;
        }
        let holder = self.name.clone();
        self.criteria_update(crit, &holder, |v| v.wrapping_add(n));
    }

    /// Paused singleplayer tick: the world is frozen, but the time spent in the world keeps
    /// counting for the player, like the game.
    pub(crate) fn paused_tick(&mut self) {
        self.award_custom("total_world_time", 1);
    }

    /// Time statistics of each player tick.
    pub(crate) fn stats_tick(&mut self) {
        if self.game_mode == 3 && !self.is_loaded(crate::mobs::mth::block_of(self.pos)) {
            return;
        }
        self.award_custom("play_time", 1);
        self.award_custom("total_world_time", 1);
        if !self.life.vit.dead {
            self.award_custom("time_since_death", 1);
        }
        if self.sneaking {
            self.award_custom("sneak_time", 1);
        }
        if self.life.vit.sleeping.is_none() {
            self.award_custom("time_since_rest", 1);
        }
    }

    /// `award_stats`: values changed since the last send.
    pub(crate) fn send_stats(&mut self) {
        let dirty = std::mem::take(&mut self.prog.stats.dirty);
        let mut out = Vec::with_capacity(dirty.len());
        for (ty, key) in dirty {
            let Some(t) = TYPES.iter().position(|x| *x == ty) else { continue };
            let reg = value_registry(&ty);
            let idx = match self.data.registry(reg) {
                Some(r) => r.entries.iter().position(|e| *e == key),
                None if reg == "minecraft:custom_stat" => {
                    CUSTOM.iter().position(|c| key.strip_prefix("minecraft:") == Some(*c))
                }
                None => None,
            };
            let idx = idx.or_else(|| {
                (reg == "minecraft:custom_stat")
                    .then(|| CUSTOM.iter().position(|c| key.strip_prefix("minecraft:") == Some(*c)))
                    .flatten()
            });
            let Some(i) = idx else { continue };
            out.push((t as i32, i as i32, self.prog.stats.get(&ty, &key)));
        }
        let _ = self.send(Event::Progress(rc_client::progress::Progress::Stats(out)));
    }

    pub(crate) fn stats_save_jobs(&mut self, jobs: &mut Vec<crate::save::Job>) {
        jobs.push(crate::save::Job::Raw {
            path: self.prog.stats.path.clone(),
            bytes: self.prog.stats.to_json(crate::chunk::DATA_VERSION).into_bytes(),
        });
    }

    /// The mount statistics check (G5-F06): distance of the passenger player since `before`, counted
    /// according to the vehicle (`key`: `minecart_one_cm`, `pig_one_cm`…, nothing for the others).
    pub(crate) fn riding_stats(&mut self, before: [f64; 3], key: Option<&str>) {
        let d = [self.pos[0] - before[0], self.pos[1] - before[1], self.pos[2] - before[2]];
        let Some(key) = key else { return };
        if d == [0.0; 3] {
            return;
        }
        let dist = ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32 * 100.0).round() as i32;
        self.award_custom(key, dist);
    }

    /// Travel statistic of a ridden mob (pig, strider, happy
    /// ghast, nautiluses).
    pub(crate) fn riding_stat_key(kind: crate::mobs::species::Kind) -> Option<&'static str> {
        use crate::mobs::mounts::{handles, is_camel};
        use crate::mobs::species::Kind;
        match kind {
            Kind::Pig => Some("pig_one_cm"),
            Kind::Strider => Some("strider_one_cm"),
            Kind::HappyGhast => Some("happy_ghast_one_cm"),
            Kind::Nautilus | Kind::ZombieNautilus => Some("nautilus_one_cm"),
            k if handles(k) || is_camel(k) => Some("horse_one_cm"),
            _ => None,
        }
    }

    // ---- Game events (statistics and advancement triggers) -----------------------

    /// Movement accepted (jump, fall, blocks crossed).
    pub(crate) fn progress_after_move(&mut self, before: [f64; 3], was_on_ground: bool) {
        if self.teleport.is_some() || self.life.vit.dead {
            return;
        }
        let d = [self.pos[0] - before[0], self.pos[1] - before[1], self.pos[2] - before[2]];
        if was_on_ground && !self.on_ground && d[1] > 0.0 {
            self.award_custom("jump", 1);
        }
        if d != [0.0; 3] {
            let full = ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32 * 100.0).round() as i32;
            let horiz = ((d[0] * d[0] + d[2] * d[2]).sqrt() as f32 * 100.0).round() as i32;
            let feet = crate::mobs::mth::block_of(self.pos);
            let eyes = [self.pos[0], self.pos[1] + self.ent_eye_height(crate::commands::ents::E::Player), self.pos[2]];
            let eye_water = self.fluid_at(crate::mobs::mth::block_of(eyes)).kind == 1;
            let in_water = self.fluid_at(feet).kind == 1;
            let feet_name = self.data.block_of(self.get(feet)).map_or("", |b| b.name.as_str()).to_owned();
            let climbable = self.p_tag("minecraft:block", "minecraft:climbable", &feet_name);
            if self.life.vit.sprinting && in_water && eye_water {
                if full > 0 {
                    self.award_custom("swim_one_cm", full);
                }
            } else if eye_water {
                if full > 0 {
                    self.award_custom("walk_under_water_one_cm", full);
                }
            } else if in_water {
                if horiz > 0 {
                    self.award_custom("walk_on_water_one_cm", horiz);
                }
            } else if climbable {
                if d[1] > 0.0 {
                    self.award_custom("climb_one_cm", (d[1] * 100.0).round() as i32);
                }
            } else if self.on_ground {
                if horiz > 0 {
                    let k = if self.life.vit.sprinting {
                        "sprint_one_cm"
                    } else if self.sneaking {
                        "crouch_one_cm"
                    } else {
                        "walk_one_cm"
                    };
                    self.award_custom(k, horiz);
                }
            } else if self.proj_player.fall_flying {
                self.award_custom("aviate_one_cm", full);
            } else if horiz > 25 {
                self.award_custom("fly_one_cm", horiz);
            }
        }
        self.trig_after_move(before, was_on_ground);
    }
}
