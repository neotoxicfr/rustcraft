//! Raids (`Raid`, `Raids`): Bad Omen → raid omen (600 ticks) at the village, raid
//! created at the center of occupied village POIs (64 blocks), waves by difficulty (3, 5, 7,
//! plus a bonus wave beyond omen level 1), boss bar (red, 10 notches),
//! recruiting nearby pillagers, banner leaders, hero of the village on victory,
//! pillager celebration on defeat. Recorded in `data/minecraft/raids.dat`.

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto_v777::play::{BossEvent, BossOp, SoundPacket, SoundRef};

use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::super::mth::block_of;
use super::super::spawn::{Group, Reason};
use super::super::species::Kind;
use super::is_raider;
use crate::nbtio::{self, int};
use crate::player::Stack;
use crate::server::Server;

const STATUS: [&str; 4] = ["ongoing", "victory", "loss", "stopped"];
const ONGOING: u8 = 0;
const VICTORY: u8 = 1;
const LOSS: u8 = 2;
const STOPPED: u8 = 3;

/// Wave species, in the game's order, and base count per wave (index = wave).
const WAVES: [(Kind, [i32; 8]); 5] = [
    (Kind::Vindicator, [0, 0, 2, 0, 1, 4, 2, 5]),
    (Kind::Evoker, [0, 0, 0, 0, 0, 1, 1, 2]),
    (Kind::Pillager, [0, 4, 3, 3, 4, 4, 4, 2]),
    (Kind::Witch, [0, 0, 0, 0, 3, 0, 0, 1]),
    (Kind::Ravager, [0, 0, 0, 1, 0, 1, 0, 2]),
];

/// A raid.
#[derive(Debug, Clone)]
pub struct Raid {
    pub id: i32,
    pub started: bool,
    pub active: bool,
    pub ticks_active: i64,
    pub omen: i32,
    pub groups_spawned: i32,
    pub cooldown: i32,
    pub post_raid_ticks: i32,
    pub total_health: f32,
    pub num_groups: i32,
    pub status: u8,
    pub center: [i32; 3],
    pub heroes: Vec<u128>,
    /// Pillagers per wave (ids), leaders per wave.
    pub raiders: Vec<(i32, i32)>,
    pub leaders: Vec<(i32, i32)>,
    pub celebration_ticks: i32,
    pub wave_spawn_pos: Option<[i32; 3]>,
    /// Own random (seeded from the clock).
    pub rng: JRand,
    /// Boss bar: UUID, name, progress, visible, registered player.
    pub bar: u128,
    pub bar_name: Nbt,
    pub bar_progress: f32,
    pub bar_visible: bool,
    pub bar_player: bool,
}

fn raid_name() -> Nbt {
    Nbt::Compound(vec![("translate".into(), Nbt::String("event.minecraft.raid".into()))])
}

impl Raid {
    fn new(id: i32, center: [i32; 3], difficulty: i32) -> Raid {
        let mut rng = JRand::from_time(i64::from(id) ^ 0x5241_4944);
        // Two draws.
        let hi = rng.next_long() as u64 & !0xF000 | 0x4000;
        let lo = rng.next_long() as u64 & 0x3FFF_FFFF_FFFF_FFFF | 0x8000_0000_0000_0000;
        Raid {
            id,
            started: false,
            active: true,
            ticks_active: 0,
            omen: 0,
            groups_spawned: 0,
            cooldown: 300,
            post_raid_ticks: 0,
            total_health: 0.0,
            num_groups: num_groups(difficulty),
            status: ONGOING,
            center,
            heroes: Vec::new(),
            raiders: Vec::new(),
            leaders: Vec::new(),
            celebration_ticks: 0,
            wave_spawn_pos: None,
            rng,
            bar: u128::from(hi) << 64 | u128::from(lo),
            bar_name: raid_name(),
            bar_progress: 0.0,
            bar_visible: true,
            bar_player: false,
        }
    }

    pub fn is_over(&self) -> bool {
        self.status == VICTORY || self.status == LOSS
    }

    pub fn is_loss(&self) -> bool {
        self.status == LOSS
    }

    pub fn alive(&self) -> i32 {
        self.raiders.len() as i32
    }

    pub fn is_victory(&self) -> bool {
        self.status == VICTORY
    }

    pub fn is_stopped(&self) -> bool {
        self.status == STOPPED
    }

    pub fn first_wave_spawned(&self) -> bool {
        self.groups_spawned > 0
    }

    /// Between two waves: first wave launched, no living pillager, delay in progress.
    pub fn between_waves(&self) -> bool {
        self.first_wave_spawned() && self.alive() == 0 && self.cooldown > 0
    }

    fn has_bonus_wave(&self) -> bool {
        self.omen > 1
    }

    fn is_final_wave(&self) -> bool {
        self.groups_spawned == self.num_groups
    }

    fn has_more_waves(&self) -> bool {
        if self.has_bonus_wave() { self.groups_spawned <= self.num_groups } else { !self.is_final_wave() }
    }

    fn should_spawn_bonus_group(&self) -> bool {
        self.is_final_wave() && self.alive() == 0 && self.has_bonus_wave()
    }

    fn should_spawn_group(&self) -> bool {
        self.cooldown == 0
            && (self.groups_spawned < self.num_groups || self.should_spawn_bonus_group())
            && self.alive() == 0
    }

    pub fn leader(&self, wave: i32) -> Option<i32> {
        self.leaders.iter().find(|l| l.0 == wave).map(|l| l.1)
    }

    /// The enchantment chance by omen level.
    pub fn enchant_odds(&self) -> f32 {
        match self.omen {
            2 => 0.1,
            3 => 0.25,
            4 => 0.5,
            5 => 0.75,
            _ => 0.0,
        }
    }

    fn to_nbt(&self) -> Nbt {
        let heroes = Nbt::List(self.heroes.iter().map(|&u| nbtio::uuid_nbt(u)).collect());
        Nbt::Compound(vec![
            ("id".into(), Nbt::Int(self.id)),
            ("started".into(), Nbt::Byte(i8::from(self.started))),
            ("active".into(), Nbt::Byte(i8::from(self.active))),
            ("ticks_active".into(), Nbt::Long(self.ticks_active)),
            ("raid_omen_level".into(), Nbt::Int(self.omen)),
            ("groups_spawned".into(), Nbt::Int(self.groups_spawned)),
            ("cooldown_ticks".into(), Nbt::Int(self.cooldown)),
            ("post_raid_ticks".into(), Nbt::Int(self.post_raid_ticks)),
            ("total_health".into(), Nbt::Float(self.total_health)),
            ("group_count".into(), Nbt::Int(self.num_groups)),
            ("status".into(), Nbt::String(STATUS[self.status as usize].into())),
            ("center".into(), Nbt::IntArray(self.center.to_vec())),
            ("heroes_of_the_village".into(), heroes),
        ])
    }

    fn from_nbt(e: &Nbt) -> Option<Raid> {
        let id = int(e, "id")? as i32;
        let center = match e.get("center")? {
            Nbt::IntArray(a) if a.len() == 3 => [a[0], a[1], a[2]],
            _ => return None,
        };
        let mut r = Raid::new(id, center, 2);
        let flag = |k: &str| int(e, k).is_some_and(|v| v != 0);
        r.started = flag("started");
        r.active = flag("active");
        r.ticks_active = e.get("ticks_active").and_then(super::super::mob::f64_of).unwrap_or(0.0) as i64;
        r.omen = int(e, "raid_omen_level").unwrap_or(0) as i32;
        r.groups_spawned = int(e, "groups_spawned").unwrap_or(0) as i32;
        r.cooldown = int(e, "cooldown_ticks").unwrap_or(0) as i32;
        r.post_raid_ticks = int(e, "post_raid_ticks").unwrap_or(0) as i32;
        r.total_health = nbtio::float(e, "total_health").unwrap_or(0.0);
        r.num_groups = int(e, "group_count").unwrap_or(5) as i32;
        let st = e.get("status").and_then(Nbt::as_str).unwrap_or("ongoing");
        r.status = STATUS.iter().position(|s| *s == st).unwrap_or(0) as u8;
        r.heroes = nbtio::list(e.get("heroes_of_the_village")).iter().filter_map(|h| nbtio::uuid(Some(h))).collect();
        Some(r)
    }
}

/// Easy 3, normal 5, hard 7.
pub fn num_groups(difficulty: i32) -> i32 {
    match difficulty {
        0 => 0,
        1 => 3,
        2 => 5,
        _ => 7,
    }
}

/// World raids state.
#[derive(Debug, Clone, Default)]
pub struct Raids {
    pub list: Vec<Raid>,
    pub next_id: i32,
    pub tick: i32,
    pub loaded: bool,
    pub dirty: bool,
    /// Player omen position (`raid_omen_position`).
    pub omen_pos: Option<[i32; 3]>,
}

impl Server {
    // ---------- Access ----------

    fn raids_load(&mut self) {
        if self.mobs.ill.raids.loaded {
            return;
        }
        let r = &mut self.mobs.ill.raids;
        r.loaded = true;
        r.next_id = 1;
        let path = self.world.join("data").join("minecraft").join("raids.dat");
        if let Ok(root) = crate::nbtio::read_dat(&path)
            && let Some(d) = root.get("data")
        {
            let r = &mut self.mobs.ill.raids;
            r.list = nbtio::list(d.get("raids")).iter().filter_map(Raid::from_nbt).collect();
            r.next_id = int(d, "next_id").unwrap_or(1) as i32;
            r.tick = int(d, "tick").unwrap_or(0) as i32;
        }
        if let Some(Nbt::IntArray(a)) = self.pdata.root.get("raid_omen_position")
            && a.len() == 3
        {
            self.mobs.ill.raids.omen_pos = Some([a[0], a[1], a[2]]);
        }
    }

    pub(crate) fn raid_ref(&self, id: i32) -> Option<&Raid> {
        self.mobs.ill.raids.list.iter().find(|r| r.id == id)
    }

    fn raid_mut(&mut self, id: i32) -> Option<&mut Raid> {
        self.mobs.ill.raids.list.iter_mut().find(|r| r.id == id)
    }

    pub(crate) fn raid_is_active(&self, id: i32) -> bool {
        self.raid_ref(id).is_some_and(|r| r.active)
    }

    pub(crate) fn raid_is_loss(&self, id: i32) -> bool {
        self.raid_ref(id).is_some_and(Raid::is_loss)
    }

    /// The pillager's "active raid" test.
    pub(crate) fn raider_active_raid(&self, m: &Mob) -> bool {
        m.ill.raid.is_some_and(|id| self.raid_is_active(id))
    }

    pub(crate) fn raider_raid_over(&self, m: &Mob) -> bool {
        m.ill.raid.and_then(|id| self.raid_ref(id)).is_some_and(Raid::is_over)
    }

    /// Closest active raid whose center is within 96 blocks.
    pub(crate) fn raid_at(&self, p: [i32; 3]) -> Option<i32> {
        let mut best: Option<(i64, i32)> = None;
        for r in &self.mobs.ill.raids.list {
            let d: i64 = (0..3).map(|k| i64::from(r.center[k] - p[k]).pow(2)).sum();
            if r.active && d < best.map_or(9216, |b| b.0) {
                best = Some((d, r.id));
            }
        }
        best.map(|b| b.1)
    }

    /// Pillager "in a raid": current raid, or active raid at its position (predicate
    /// `type_specific/raider` of advancements and loot tables).
    pub(crate) fn raider_has_raid(&self, m: &Mob) -> bool {
        m.ill.raid.is_some() || self.is_raided(m.block_pos())
    }

    pub(crate) fn is_raided(&self, p: [i32; 3]) -> bool {
        self.raid_at(p).is_some()
    }

    /// The village proximity test.
    pub(crate) fn close_to_village(&mut self, p: [i32; 3], d: i32) -> bool {
        d <= 6 && self.sections_to_village(p) <= d
    }

    // ---------- Omens ----------

    /// Bad Omen tick; false: the effect is removed.
    pub(crate) fn bad_omen_tick(&mut self, amp: i32) -> bool {
        self.raids_load();
        if self.game_mode == 3 || self.level.difficulty_id() == 0 {
            return true;
        }
        let p = block_of(self.pos);
        if !self.is_village(p) {
            return true;
        }
        let raid = self.raid_at(p);
        if raid.is_none_or(|id| self.raid_ref(id).is_some_and(|r| r.omen < 5)) {
            self.give_player_effect("minecraft:raid_omen", 600, amp);
            self.set_raid_omen_pos(Some(p));
            return false;
        }
        true
    }

    /// Raid omen tick (1 tick from the end): the raid starts.
    pub(crate) fn raid_omen_tick(&mut self) -> bool {
        self.raids_load();
        if self.game_mode == 3 {
            return true;
        }
        if let Some(p) = self.mobs.ill.raids.omen_pos {
            self.create_or_extend_raid(p);
            self.set_raid_omen_pos(None);
            return false;
        }
        true
    }

    fn set_raid_omen_pos(&mut self, p: Option<[i32; 3]>) {
        self.mobs.ill.raids.omen_pos = p;
        match p {
            Some(p) => nbtio::set(&mut self.pdata.root, "raid_omen_position", Nbt::IntArray(p.to_vec())),
            None => nbtio::remove(&mut self.pdata.root, "raid_omen_position"),
        }
    }

    /// Center = average of the occupied village POIs within 64 blocks.
    fn create_or_extend_raid(&mut self, at: [i32; 3]) {
        if !self.rule("minecraft:raids", true) || self.dim.name != "minecraft:overworld" {
            return;
        }
        let recs = self.poi_in_range(|t| t.is_village(), at, 64, crate::poi::Occupancy::IsOccupied);
        let center = if recs.is_empty() {
            at
        } else {
            let n = recs.len() as f64;
            let s = recs.iter().fold([0.0f64; 3], |a, r| {
                [a[0] + f64::from(r.pos[0]), a[1] + f64::from(r.pos[1]), a[2] + f64::from(r.pos[2])]
            });
            block_of([s[0] * (1.0 / n), s[1] * (1.0 / n), s[2] * (1.0 / n)])
        };
        let id = match self.raid_at(center) {
            Some(id) => id,
            None => {
                let r = &mut self.mobs.ill.raids;
                r.next_id += 1;
                let id = r.next_id;
                let diff = self.level.difficulty_id();
                self.mobs.ill.raids.list.push(Raid::new(id, center, diff));
                id
            }
        };
        let amp = self.fx().amp("minecraft:raid_omen");
        let mut first = false;
        if let Some(r) = self.raid_mut(id)
            && (!r.started || r.omen < 5)
            && let Some(a) = amp
        {
            r.omen = (r.omen + a + 1).clamp(0, 5);
            first = r.groups_spawned == 0;
        }
        // W8c: omen absorbed before the first wave: statistic, `voluntary_exile` trigger.
        if first {
            self.award_custom("raid_trigger", 1);
            self.trig_simple("minecraft:voluntary_exile");
        }
        self.mobs.ill.raids.dirty = true;
    }

    // ---------- Tick ----------

    /// The raids tick (before the mobs).
    pub(crate) fn raids_tick(&mut self) {
        self.raids_load();
        self.mobs.ill.raids.tick += 1;
        let ids: Vec<i32> = self.mobs.ill.raids.list.iter().map(|r| r.id).collect();
        let rule = self.rule("minecraft:raids", true);
        for id in ids {
            if !rule {
                self.raid_stop(id);
            }
            if self.raid_ref(id).is_some_and(|r| r.status == STOPPED) {
                self.mobs.ill.raids.list.retain(|r| r.id != id);
                self.mobs.ill.raids.dirty = true;
            } else {
                self.raid_tick(id);
            }
        }
        if self.mobs.ill.raids.tick % 200 == 0 {
            self.mobs.ill.raids.dirty = true;
        }
    }

    fn raid_stop(&mut self, id: i32) {
        if let Some(r) = self.raid_mut(id) {
            r.active = false;
            r.status = STOPPED;
        }
        self.bar_remove_player(id);
    }

    fn raid_tick(&mut self, id: i32) {
        let Some(r) = self.raid_ref(id) else { return };
        if r.status == STOPPED {
            return;
        }
        if r.status != ONGOING {
            if r.is_over() {
                let victory = r.status == VICTORY;
                let r = self.raid_mut(id).expect("raid");
                r.celebration_ticks += 1;
                if r.celebration_ticks >= 600 {
                    self.raid_stop(id);
                    return;
                }
                if r.celebration_ticks % 20 == 0 {
                    self.raid_update_players(id);
                    self.bar_set_visible(id, true);
                    if victory {
                        self.bar_set_progress(id, 0.0);
                        self.bar_set_name(id, translate("event.minecraft.raid.victory.full"));
                    } else {
                        self.bar_set_name(id, translate("event.minecraft.raid.defeat.full"));
                    }
                }
            }
            return;
        }
        let center = r.center;
        let old_active = r.active;
        let active = self.chunks.get(&(center[0] >> 4, center[2] >> 4)).is_some_and(Option::is_some);
        self.raid_mut(id).expect("raid").active = active;
        if self.level.difficulty_id() == 0 {
            self.raid_stop(id);
            return;
        }
        if old_active != active {
            self.bar_set_visible(id, active);
        }
        if !active {
            return;
        }
        if !self.is_village(center) {
            self.move_raid_center(id);
        }
        let center = self.raid_ref(id).map_or(center, |r| r.center);
        if !self.is_village(center) {
            if self.raid_ref(id).is_some_and(|r| r.groups_spawned > 0) {
                self.raid_mut(id).expect("raid").status = LOSS;
            } else {
                self.raid_stop(id);
            }
        }
        let r = self.raid_mut(id).expect("raid");
        r.ticks_active += 1;
        if r.ticks_active >= 48000 {
            self.raid_stop(id);
            return;
        }
        let alive = r.alive();
        if alive == 0 && r.has_more_waves() {
            if r.cooldown <= 0 {
                if r.cooldown == 0 && r.groups_spawned > 0 {
                    r.cooldown = 300;
                    self.bar_set_name(id, raid_name());
                    return;
                }
            } else {
                let cached = r.wave_spawn_pos.is_some();
                let mut find = !cached && r.cooldown % 5 == 0;
                if let Some(p) = r.wave_spawn_pos
                    && !self.entity_ticking(p)
                {
                    find = true;
                }
                if find {
                    let p = self.raid_find_spawn_pos(id, 8);
                    self.raid_mut(id).expect("raid").wave_spawn_pos = p;
                }
                let cd = self.raid_ref(id).map_or(0, |r| r.cooldown);
                if cd == 300 || cd % 20 == 0 {
                    self.raid_update_players(id);
                }
                let r = self.raid_mut(id).expect("raid");
                r.cooldown -= 1;
                let p = ((300 - r.cooldown) as f32 / 300.0).clamp(0.0, 1.0);
                self.bar_set_progress(id, p);
            }
        }
        let ticks = self.raid_ref(id).map_or(0, |r| r.ticks_active);
        if ticks % 20 == 0 {
            self.raid_update_players(id);
            self.raid_update_raiders(id);
            let alive = self.raid_ref(id).map_or(0, Raid::alive);
            let name = if alive > 0 && alive <= 2 {
                Nbt::Compound(vec![
                    ("translate".into(), Nbt::String("event.minecraft.raid".into())),
                    (
                        "extra".into(),
                        Nbt::List(vec![
                            Nbt::String(" - ".into()),
                            Nbt::Compound(vec![
                                ("translate".into(), Nbt::String("event.minecraft.raid.raiders_remaining".into())),
                                ("with".into(), Nbt::List(vec![Nbt::Int(alive)])),
                            ]),
                        ]),
                    ),
                ])
            } else {
                raid_name()
            };
            self.bar_set_name(id, name);
        }
        let mut sound_played = false;
        let mut attempt = 0;
        while self.raid_ref(id).is_some_and(Raid::should_spawn_group) {
            let pos = match self.raid_ref(id).and_then(|r| r.wave_spawn_pos) {
                Some(p) => Some(p),
                None => self.raid_find_spawn_pos(id, 20),
            };
            match pos {
                Some(p) => {
                    self.raid_mut(id).expect("raid").started = true;
                    self.raid_spawn_group(id, p);
                    if !sound_played {
                        self.raid_horn(id, p);
                        sound_played = true;
                    }
                }
                None => attempt += 1,
            }
            if attempt > 5 {
                self.raid_stop(id);
                break;
            }
        }
        let Some(r) = self.raid_ref(id) else { return };
        if r.started && !r.has_more_waves() && r.alive() == 0 {
            let r = self.raid_mut(id).expect("raid");
            if r.post_raid_ticks < 40 {
                r.post_raid_ticks += 1;
            } else {
                r.status = VICTORY;
                let (heroes, omen) = (r.heroes.clone(), r.omen);
                if heroes.contains(&self.uuid) && self.game_mode != 3 && self.ent_alive(Ent::Player) {
                    self.give_player_effect("minecraft:hero_of_the_village", 48000, omen - 1);
                    // W8c: `raid_win` statistic and `hero_of_the_village` trigger.
                    self.award_custom("raid_win", 1);
                    self.trig_simple("minecraft:hero_of_the_village");
                }
            }
        }
        self.mobs.ill.raids.dirty = true;
    }

    /// Center moved to the closest village section (cube of ±2 sections).
    fn move_raid_center(&mut self, id: i32) {
        let Some(c) = self.raid_ref(id).map(|r| r.center) else { return };
        let s = [c[0] >> 4, c[1] >> 4, c[2] >> 4];
        let mut best: Option<(i64, [i32; 3])> = None;
        for dx in -2..=2 {
            for dy in -2..=2 {
                for dz in -2..=2 {
                    let p = [((s[0] + dx) << 4) + 7, ((s[1] + dy) << 4) + 7, ((s[2] + dz) << 4) + 7];
                    if !self.is_village(p) {
                        continue;
                    }
                    let d: i64 = (0..3).map(|k| i64::from(p[k] - c[k]).pow(2)).sum();
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, p));
                    }
                }
            }
        }
        if let Some((_, p)) = best {
            self.raid_mut(id).expect("raid").center = p;
        }
    }

    /// Position in a simulated chunk.
    fn entity_ticking(&self, p: [i32; 3]) -> bool {
        let c = (p[0] >> 4, p[2] >> 4);
        self.mob_chunk_ticking(c) && self.chunks.get(&c).is_some_and(Option::is_some)
    }

    /// On a circle around the center, closer when approaching the
    /// departure; outside the village as long as more than 7 s remain; place of a ground ravager.
    fn raid_find_spawn_pos(&mut self, id: i32, tries: i32) -> Option<[i32; 3]> {
        let (center, cooldown) = self.raid_ref(id).map(|r| (r.center, r.cooldown))?;
        let secs = cooldown / 20;
        let how_far = 0.22f32 * secs as f32 - 0.24;
        let start = self.raid_mut(id)?.rng.next_float() * (std::f32::consts::PI * 2.0);
        for i in 0..tries {
            let angle = start + std::f32::consts::PI * i as f32 / 8.0;
            let r = self.raid_mut(id)?;
            let ox = (super::super::mth::cos(angle) * 32.0 * how_far).floor() as i32
                + r.rng.next_int_bound(3) * how_far.floor() as i32;
            let oz = (super::super::mth::sin(angle) * 32.0 * how_far).floor() as i32
                + r.rng.next_int_bound(3) * how_far.floor() as i32;
            let x = center[0] + ox;
            let z = center[2] + oz;
            let y = self.surface_y(x, z, 1);
            if (y - center[1]).abs() > 96 {
                continue;
            }
            let p = [x, y, z];
            if self.is_village(p) && secs > 7 {
                continue;
            }
            if self.chunks_loaded_box(x - 10, z - 10, x + 10, z + 10)
                && self.entity_ticking(p)
                && (self.ravager_spawn_ok(p) || (self.bname([x, y - 1, z]) == "minecraft:snow" && self.is_air(p)))
            {
                return Some(p);
            }
        }
        None
    }

    /// "Ground" placement of the ravager.
    fn ravager_spawn_ok(&self, p: [i32; 3]) -> bool {
        self.valid_spawn_on([p[0], p[1] - 1, p[2]], Kind::Ravager)
            && self.valid_empty_spawn(p, Kind::Ravager)
            && self.valid_empty_spawn([p[0], p[1] + 1, p[2]], Kind::Ravager)
    }

    /// Raid horn: 13 blocks from the player toward the origin, volume 64, if it is within 64 blocks or
    /// registered in the raid.
    fn raid_horn(&mut self, id: i32, origin: [i32; 3]) {
        let Some(r) = self.raid_mut(id) else { return };
        let seed = r.rng.next_long();
        let inside = r.bar_player;
        let o = [f64::from(origin[0]) + 0.5, f64::from(origin[1]) + 0.5, f64::from(origin[2]) + 0.5];
        let p = self.pos;
        let d = ((o[0] - p[0]).powi(2) + (o[2] - p[2]).powi(2)).sqrt();
        if d <= 64.0 || inside {
            let x = p[0] + 13.0 / d * (o[0] - p[0]);
            let z = p[2] + 13.0 / d * (o[2] - p[2]);
            let sid = self
                .data
                .registry("minecraft:sound_event")
                .and_then(|r| r.entries.iter().position(|e| e == "minecraft:event.raid.horn"))
                .map(|i| i as i32);
            if let Some(sid) = sid {
                let _ = self.send(Event::Sound(SoundPacket {
                    sound: SoundRef::Id(sid),
                    category: 6,
                    pos: Some([x, p[1], z]),
                    entity: None,
                    volume: 64.0,
                    pitch: 1.0,
                    seed,
                }));
            }
        }
    }

    /// Each wave species, base count plus bonus, leader designated,
    /// ravager riders at advanced waves.
    fn raid_spawn_group(&mut self, id: i32, pos: [i32; 3]) {
        let Some(r) = self.raid_ref(id) else { return };
        let wave = r.groups_spawned + 1;
        let bonus_group = r.should_spawn_bonus_group();
        let num = r.num_groups;
        self.raid_mut(id).expect("raid").total_health = 0.0;
        let diff = self.level.difficulty_id();
        let mut leader_set = false;
        for (kind, counts) in WAVES {
            let base = if bonus_group { counts[num as usize] } else { counts[wave.min(7) as usize] };
            let extra = self.raid_bonus_spawns(id, kind, wave, diff, bonus_group);
            let mut ravagers = 0;
            for _ in 0..base + extra {
                let mut m = self.new_mob(kind, [0.0; 3], 0.0);
                if !leader_set && super::can_be_leader(kind) {
                    m.ill.patrol_leader = true;
                    m.ill.patrolling = true;
                    self.raid_set_leader(id, wave, &mut m);
                    leader_set = true;
                }
                self.raid_join(id, wave, &mut m, Some(pos), false);
                if kind == Kind::Ravager {
                    let rider = if wave == num_groups(2) {
                        Some(Kind::Pillager)
                    } else if wave >= num_groups(3) {
                        Some(if ravagers == 0 { Kind::Evoker } else { Kind::Vindicator })
                    } else {
                        None
                    };
                    ravagers += 1;
                    if let Some(rk) = rider {
                        let mut rm = self.new_mob(rk, [0.0; 3], 0.0);
                        self.raid_join(id, wave, &mut rm, Some(pos), false);
                        rm.pos = [f64::from(pos[0]) + 0.5, f64::from(pos[1]), f64::from(pos[2]) + 0.5];
                        rm.old = rm.pos;
                        rm.yaw = 0.0;
                        self.mob_start_riding(&mut rm, &mut m, false);
                        self.add_mob(m);
                        self.add_mob(rm);
                        continue;
                    }
                }
                self.add_mob(m);
            }
        }
        let r = self.raid_mut(id).expect("raid");
        r.wave_spawn_pos = None;
        r.groups_spawned += 1;
        self.raid_update_bossbar(id);
        self.mobs.ill.raids.dirty = true;
    }

    /// The possible bonus spawns (raid random).
    fn raid_bonus_spawns(&mut self, id: i32, kind: Kind, wave: i32, diff: i32, bonus: bool) -> i32 {
        let easy = diff == 1;
        let normal = diff == 2;
        let r = self.raid_mut(id).expect("raid");
        let b = match kind {
            Kind::Vindicator | Kind::Pillager => {
                if easy {
                    r.rng.next_int_bound(2)
                } else if normal {
                    1
                } else {
                    2
                }
            }
            Kind::Witch => {
                if easy || wave <= 2 || wave == 4 {
                    return 0;
                }
                1
            }
            Kind::Ravager => i32::from(!easy && bonus),
            _ => return 0,
        };
        if b > 0 { r.rng.next_int_bound(b + 1) } else { 0 }
    }

    /// Registers the pillager; newcomer placed at the spawn point (+0.5, +1,
    /// +0.5), finalized (reason "event"), reinforced, set on the ground.
    fn raid_join(&mut self, id: i32, wave: i32, m: &mut Mob, pos: Option<[i32; 3]>, exists: bool) {
        self.raid_add_wave_mob(id, wave, m, true);
        m.ill.raid = Some(id);
        m.ill.wave = wave;
        m.ill.can_join_raid = true;
        m.ill.ticks_outside_raid = 0;
        if !exists && let Some(p) = pos {
            m.pos = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 1.0, f64::from(p[2]) + 0.5];
            m.old = m.pos;
            let mut g = Group::default();
            self.finalize_spawn(m, Reason::Event, &mut g);
            self.raid_buffs(id, wave, m);
            m.on_ground = true;
        }
    }

    /// Replaces a copy with the same UUID, counts its HP.
    pub(crate) fn raid_add_wave_mob(&mut self, id: i32, wave: i32, m: &Mob, update_health: bool) {
        let Some(r) = self.raid_mut(id) else { return };
        if !r.raiders.iter().any(|x| x.1 == m.id) {
            r.raiders.push((wave, m.id));
        }
        if update_health {
            r.total_health += m.health;
        }
        self.raid_update_bossbar(id);
        self.mobs.ill.raids.dirty = true;
    }

    /// Ominous banner on the head, certain to drop.
    pub(crate) fn raid_set_leader(&mut self, id: i32, wave: i32, m: &mut Mob) {
        if let Some(r) = self.raid_mut(id) {
            r.leaders.retain(|l| l.0 != wave);
            r.leaders.push((wave, m.id));
        }
        m.equipment[5] = Some(super::ominous_banner());
        m.drop_chance[5] = 2.0;
        self.send_mob_equipment(m, &[5]);
    }

    /// The application of raid bonuses (pillager, vindicator; entity random).
    fn raid_buffs(&mut self, id: i32, wave: i32, m: &mut Mob) {
        let Some(odds) = self.raid_ref(id).map(Raid::enchant_odds) else { return };
        match m.kind {
            Kind::Pillager => {
                if m.rng.next_float() <= odds {
                    let lvl = if wave > num_groups(2) {
                        2
                    } else if wave > num_groups(1) {
                        1
                    } else {
                        0
                    };
                    if lvl > 0 {
                        let mut s = Stack { id: "minecraft:crossbow".into(), count: 1, components: None };
                        crate::enchanting::set_enchants(
                            &self.life.items,
                            &mut s,
                            &[("minecraft:quick_charge".into(), lvl)],
                        );
                        m.equipment[0] = Some(s);
                    }
                }
            }
            Kind::Vindicator => {
                let mut s = Stack { id: "minecraft:iron_axe".into(), count: 1, components: None };
                if m.rng.next_float() <= odds {
                    let lvl = if wave > num_groups(2) { 2 } else { 1 };
                    crate::enchanting::set_enchants(&self.life.items, &mut s, &[("minecraft:sharpness".into(), lvl)]);
                }
                m.equipment[0] = Some(s);
            }
            _ => {}
        }
    }

    /// The removal from the raid.
    pub(crate) fn raid_remove(&mut self, id: i32, m: &mut Mob, from_total: bool) {
        let Some(r) = self.raid_mut(id) else { return };
        let before = r.raiders.len();
        r.raiders.retain(|x| x.1 != m.id);
        if r.raiders.len() != before {
            if from_total {
                r.total_health -= m.health;
            }
            m.ill.raid = None;
            self.raid_update_bossbar(id);
            self.mobs.ill.raids.dirty = true;
        }
    }

    /// Pillagers removed, too far (112 blocks), or absent from the village and
    /// inactive more than 30 times.
    fn raid_update_raiders(&mut self, id: i32) {
        let Some(r) = self.raid_ref(id) else { return };
        let center = r.center;
        let members: Vec<i32> = r.raiders.iter().map(|x| x.1).collect();
        let mut out: Vec<i32> = Vec::new();
        for mid in members {
            let Some(i) = self.mob_index(mid) else {
                out.push(mid);
                continue;
            };
            let Some(o) = self.mobs.list[i].as_ref() else { continue };
            let p = o.block_pos();
            let d: i64 = (0..3).map(|k| i64::from(center[k] - p[k]).pow(2)).sum();
            if o.removed || d >= 12544 {
                out.push(mid);
            } else if o.tick_count > 600 {
                let (idle, tc) = (o.no_action_time, o.ill.ticks_outside_raid);
                let village = self.is_village(p);
                let o = self.mobs.list[i].as_mut().expect("mob");
                if !village && idle > 2400 {
                    o.ill.ticks_outside_raid = tc + 1;
                }
                if o.ill.ticks_outside_raid >= 30 {
                    out.push(mid);
                }
            }
        }
        for mid in out {
            match self.mob_index(mid) {
                Some(i) => {
                    let mut o = self.mobs.list[i].take().expect("mob");
                    self.raid_remove(id, &mut o, true);
                    if o.ill.patrol_leader
                        && let Some(r) = self.raid_mut(id)
                    {
                        r.leaders.retain(|l| l.0 != o.ill.wave);
                    }
                    self.mobs.list[i] = Some(o);
                }
                None => {
                    if let Some(r) = self.raid_mut(id) {
                        r.raiders.retain(|x| x.1 != mid);
                        r.leaders.retain(|l| l.1 != mid);
                    }
                }
            }
        }
    }

    /// Living HP / total on the bar.
    pub(crate) fn raid_update_bossbar(&mut self, id: i32) {
        let Some(r) = self.raid_ref(id) else { return };
        let ids: Vec<i32> = r.raiders.iter().map(|x| x.1).collect();
        let total = r.total_health;
        let health: f32 = ids.iter().filter_map(|&i| self.mob_ref(i)).map(|o| o.health).sum();
        self.bar_set_progress(id, (health / total).clamp(0.0, 1.0));
    }

    // ---------- Pillagers ----------

    /// The AI step of pillagers outside a raid: join the raid of the place (every 20 ticks).
    pub(crate) fn raider_try_join(&mut self, m: &mut Mob) {
        self.raids_load();
        let Some(id) = self.raid_at(m.block_pos()) else { return };
        if m.alive() && m.ill.can_join_raid && m.no_action_time <= 2400 {
            let wave = self.raid_ref(id).map_or(0, |r| r.groups_spawned);
            self.raid_join(id, wave, m, None, true);
        }
    }

    /// Pillager read back with a raid: registered again (without HP) and leader restored; raid gone: forgotten.
    pub(crate) fn raider_relink(&mut self, m: &mut Mob) {
        let Some(id) = m.ill.raid else { return };
        self.raids_load();
        if self.raid_ref(id).is_none() {
            m.ill.raid = None;
            return;
        }
        if self.raid_ref(id).is_some_and(|r| r.raiders.iter().any(|x| x.1 == m.id)) {
            return;
        }
        self.raid_add_wave_mob(id, m.ill.wave, m, false);
        if m.ill.patrol_leader
            && let Some(r) = self.raid_mut(id)
        {
            r.leaders.retain(|l| l.0 != m.ill.wave);
            r.leaders.push((m.ill.wave, m.id));
        }
    }

    /// Leader removed, hero noted, pillager removed (without touching the total).
    pub(crate) fn raider_died(&mut self, m: &mut Mob, by: Option<Ent>) {
        let Some(id) = m.ill.raid else { return };
        if m.ill.patrol_leader
            && let Some(r) = self.raid_mut(id)
        {
            r.leaders.retain(|l| l.0 != m.ill.wave);
        }
        if by == Some(Ent::Player) {
            let u = self.uuid;
            if let Some(r) = self.raid_mut(id)
                && !r.heroes.contains(&u)
            {
                r.heroes.push(u);
            }
        }
        self.raid_remove(id, m, false);
    }

    /// The tick of the walk-to-raid objective.
    pub(crate) fn pathfind_to_raid_tick(&mut self, m: &mut Mob, recruit: &mut i32) {
        let Some(id) = m.ill.raid.filter(|&id| self.raid_is_active(id)) else { return };
        if m.tick_count > *recruit {
            *recruit = m.tick_count + 20;
            self.raid_recruit(id, m);
        }
        if m.nav.is_done() {
            let Some(c) = self.raid_ref(id).map(|r| r.center) else { return };
            let to = [f64::from(c[0]) + 0.5, f64::from(c[1]), f64::from(c[2]) + 0.5];
            if let Some(p) = self.pos_towards(m, 15, 4, to, std::f32::consts::PI / 2.0) {
                self.move_to_pos(m, p, 1.0);
            }
        }
    }

    /// Recruiting pillagers within 16 blocks without an active raid and fit.
    fn raid_recruit(&mut self, id: i32, m: &Mob) {
        if !self.raid_is_active(id) {
            return;
        }
        let b = m.bbox();
        let area = rc_physics::Aabb { min: b.min.map(|v| v - 16.0), max: b.max.map(|v| v + 16.0) };
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| {
                is_raider(o.kind)
                    && o.touches(&area)
                    && !o.ill.raid.is_some_and(|r| self.raid_is_active(r))
                    && o.alive()
                    && o.ill.can_join_raid
                    && o.no_action_time <= 2400
            })
            .map(|o| o.id)
            .collect();
        let wave = self.raid_ref(id).map_or(0, |r| r.groups_spawned);
        for oid in ids {
            if let Some(i) = self.mob_index(oid) {
                let mut o = self.mobs.list[i].take().expect("mob");
                self.raid_join(id, wave, &mut o, None, true);
                self.mobs.list[i] = Some(o);
            }
        }
    }

    /// Random unvisited house within 48 blocks (list shuffled by the
    /// pillager's random).
    pub(crate) fn random_home_poi(&mut self, m: &mut Mob, visited: &[[i32; 3]]) -> Option<[i32; 3]> {
        let p = m.block_pos();
        let mut list: Vec<[i32; 3]> = self
            .poi_in_range(|t| t == crate::poi::PoiType::Home, p, 48, crate::poi::Occupancy::Any)
            .into_iter()
            .map(|r| r.pos)
            .collect();
        m.rng.shuffle(&mut list);
        list.into_iter().find(|h| !visited.contains(h))
    }

    // ---------- Banner ----------

    /// The refusal to pick up the banner.
    pub(crate) fn cannot_pick_up_banner(&self, m: &Mob) -> bool {
        let Some(id) = m.ill.raid.filter(|&id| self.raid_is_active(id)) else { return true };
        let Some(r) = self.raid_ref(id) else { return true };
        if r.is_over() || !super::can_be_leader(m.kind) {
            return true;
        }
        if m.equipment[5].as_ref().is_some_and(super::is_ominous_banner) {
            return true;
        }
        r.leader(m.ill.wave).is_some_and(|l| self.mob_ref(l).is_some_and(Mob::alive))
    }

    /// Start condition of the "retrieve the raid leader's banner" goal: ominous banner on the ground, reachable.
    pub(crate) fn banner_can_use(&mut self, m: &mut Mob, item: &mut Option<i32>, store: &mut Vec<(i32, i64)>) -> bool {
        if self.cannot_pick_up_banner(m) {
            return false;
        }
        let f = m.follow_range();
        let b = m.bbox();
        let area = rc_physics::Aabb {
            min: [b.min[0] - f, b.min[1] - 8.0, b.min[2] - f],
            max: [b.max[0] + f, b.max[1] + 8.0, b.max[2] + f],
        };
        let banners: Vec<(i32, [f64; 3])> = self
            .life
            .drops
            .iter()
            .filter(|d| {
                !d.is_removed()
                    && d.pickup_delay <= 0
                    && super::is_ominous_banner(&d.stack)
                    && d.pos[0] >= area.min[0]
                    && d.pos[0] <= area.max[0]
                    && d.pos[1] >= area.min[1]
                    && d.pos[1] <= area.max[1]
                    && d.pos[2] >= area.min[2]
                    && d.pos[2] <= area.max[2]
            })
            .map(|d| (d.id, d.pos))
            .collect();
        let now = self.time;
        let mut cache: Vec<(i32, i64)> = Vec::new();
        for (bid, bp) in banners {
            let until = store.iter().find(|c| c.0 == bid).map_or(i64::MIN, |c| c.1);
            if now < until {
                cache.push((bid, until));
                continue;
            }
            let path = self.create_path(m, block_of(bp), 1);
            if path.as_ref().is_some_and(|p| p.reached) {
                *item = Some(bid);
                return true;
            }
            cache.push((bid, now + 600));
        }
        *store = cache;
        false
    }

    /// A pillager's pickup of an ominous banner: becomes leader of its wave.
    pub(crate) fn raider_pick_up_banner(&mut self, m: &mut Mob, item: i32) {
        let Some(id) = m.ill.raid.filter(|&id| self.raid_is_active(id)) else { return };
        if self.raid_ref(id).and_then(|r| r.leader(m.ill.wave)).is_some() {
            return;
        }
        let Some(stack) = self.life.drops.iter().find(|d| d.id == item).map(|d| d.stack.clone()) else { return };
        if !super::is_ominous_banner(&stack) {
            return;
        }
        if let Some(cur) = m.equipment[5].clone()
            && (m.rng.next_float() - 0.1).max(0.0) < m.drop_chance[5]
        {
            let pos = m.pos;
            let r = &mut self.mobs.rng;
            let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
            self.spawn_item(pos, v, cur, 10);
        }
        m.equipment[5] = Some(stack.clone());
        self.mob_take(item, m.id, stack.count);
        self.raid_set_leader(id, m.ill.wave, m);
        m.ill.patrol_leader = true;
        m.ill.patrolling = true;
    }

    // ---------- Doors ----------

    pub(crate) fn door_is_open(&self, p: [i32; 3]) -> bool {
        self.wooden_door(p) && self.v().get_bool(self.st(p), "open")
    }

    pub(crate) fn wooden_door(&self, p: [i32; 3]) -> bool {
        let n = self.bname(p);
        n.ends_with("_door") && n != "minecraft:iron_door"
    }

    /// Horizontal collision, wooden door on the next 2
    /// nodes (at 1.5 blocks) or just above the mob; breaking: rule, minimum
    /// difficulty (`breaking`), closed door.
    pub(crate) fn door_goal_can_use(&self, m: &Mob, door: &mut Option<[i32; 3]>, breaking: Option<i32>) -> bool {
        if !m.horizontal_collision {
            return false;
        }
        let Some(path) = m.nav.path.as_ref().filter(|p| !p.is_done()) else { return false };
        let mut found = None;
        for i in 0..(path.next + 2).min(path.nodes.len()) {
            let n = path.nodes[i].0;
            let p = [n[0], n[1] + 1, n[2]];
            let d = (f64::from(p[0]) - m.pos[0]).powi(2) + (f64::from(p[2]) - m.pos[2]).powi(2);
            if d <= 2.25 && self.wooden_door(p) {
                found = Some(p);
                break;
            }
        }
        let p = match found {
            Some(p) => p,
            None => {
                let b = m.block_pos();
                let above = [b[0], b[1] + 1, b[2]];
                if !self.wooden_door(above) {
                    return false;
                }
                above
            }
        };
        *door = Some(p);
        if let Some(min_diff) = breaking {
            return self.rule("minecraft:mob_griefing", true)
                && self.level.difficulty_id() >= min_diff
                && !self.door_is_open(p);
        }
        true
    }

    /// Door crack.
    pub(crate) fn door_break_progress(&self, breaker: i32, p: [i32; 3], progress: i32) {
        // Like the game: to the players only whose feet are within 32 blocks of the door.
        let e = || Event::BlockDestruction { id: breaker, pos: p, stage: progress as u8 };
        let at = p.map(f64::from);
        let d = [at[0] - self.pos[0], at[1] - self.pos[1], at[2] - self.pos[2]];
        if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < 1024.0 {
            self.send_personal(e());
        }
        self.send_others_near(at, 32.0, e());
    }

    /// Hits (event 1019), crack, door destroyed at 240 ticks.
    pub(crate) fn break_door_tick(&mut self, m: &mut Mob, d: [i32; 3], time: &mut i32, last: &mut i32, min_diff: i32) {
        if m.rng.next_int_bound(20) == 0 {
            let _ = self.send(Event::LevelEvent { kind: 1019, pos: d, data: 0, global: false });
            if m.swing_time <= 0 {
                self.mob_swing(m);
            }
        }
        *time += 1;
        let progress = (*time as f32 / 240.0 * 10.0) as i32;
        if progress != *last {
            self.door_break_progress(m.id, d, progress);
            *last = progress;
        }
        if *time == 240 && self.level.difficulty_id() >= min_diff {
            self.remove_block(d, false);
            let _ = self.send(Event::LevelEvent { kind: 1021, pos: d, data: 0, global: false });
            // Like the game, the state is read AFTER the removal (of air): no door shards.
            let state = self.st(d) as i32;
            let _ = self.send(Event::LevelEvent { kind: 2001, pos: d, data: state, global: false });
        }
    }

    // ---------- Boss bar ----------

    /// The living player in the raid zone is registered, otherwise removed.
    fn raid_update_players(&mut self, id: i32) {
        let p = block_of(self.pos);
        let valid = self.ent_alive(Ent::Player) && self.raid_at(p) == Some(id);
        let Some(r) = self.raid_ref(id) else { return };
        if valid && !r.bar_player {
            self.raid_mut(id).expect("raid").bar_player = true;
            self.bar_send_add(id);
        } else if !valid && r.bar_player {
            self.bar_remove_player(id);
        }
    }

    fn bar_send_add(&self, id: i32) {
        let Some(r) = self.raid_ref(id) else { return };
        if !r.bar_visible || !r.bar_player {
            return;
        }
        let op = BossOp::Add { name: Some(r.bar_name.clone()), progress: r.bar_progress, color: 2, style: 2, flags: 0 };
        let _ = self.send(Event::Boss(BossEvent { id: r.bar, op }));
    }

    fn bar_remove_player(&mut self, id: i32) {
        let Some(r) = self.raid_mut(id) else { return };
        if !r.bar_player {
            return;
        }
        r.bar_player = false;
        let (vis, bar) = (r.bar_visible, r.bar);
        if vis {
            let _ = self.send(Event::Boss(BossEvent { id: bar, op: BossOp::Remove }));
        }
    }

    fn bar_set_visible(&mut self, id: i32, v: bool) {
        let Some(r) = self.raid_mut(id) else { return };
        if r.bar_visible == v {
            return;
        }
        r.bar_visible = v;
        if r.bar_player {
            if v {
                self.bar_send_add(id);
            } else {
                let bar = r.bar;
                let _ = self.send(Event::Boss(BossEvent { id: bar, op: BossOp::Remove }));
            }
        }
    }

    fn bar_set_progress(&mut self, id: i32, p: f32) {
        let Some(r) = self.raid_mut(id) else { return };
        if r.bar_progress == p {
            return;
        }
        r.bar_progress = p;
        if r.bar_player && r.bar_visible {
            let bar = r.bar;
            let _ = self.send(Event::Boss(BossEvent { id: bar, op: BossOp::Progress(p) }));
        }
    }

    fn bar_set_name(&mut self, id: i32, n: Nbt) {
        let Some(r) = self.raid_mut(id) else { return };
        if r.bar_name == n {
            return;
        }
        r.bar_name = n.clone();
        if r.bar_player && r.bar_visible {
            let bar = r.bar;
            let _ = self.send(Event::Boss(BossEvent { id: bar, op: BossOp::Name(Some(n)) }));
        }
    }

    // ---------- Saving ----------

    /// `data/minecraft/raids.dat` (if modified).
    pub(crate) fn raids_save_jobs(&mut self, jobs: &mut Vec<crate::save::Job>) {
        if !self.mobs.ill.raids.dirty || !self.mobs.ill.raids.loaded {
            return;
        }
        self.mobs.ill.raids.dirty = false;
        let r = &self.mobs.ill.raids;
        let data = Nbt::Compound(vec![
            ("raids".into(), Nbt::List(r.list.iter().map(Raid::to_nbt).collect())),
            ("next_id".into(), Nbt::Int(r.next_id)),
            ("tick".into(), Nbt::Int(r.tick)),
        ]);
        let root =
            Nbt::Compound(vec![("data".into(), data), ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION))]);
        let path = self.world.join("data").join("minecraft").join("raids.dat");
        jobs.push(crate::save::Job::Dat { path, nbt: nbtio::encode_root(&root), backup: false });
    }
}

fn translate(key: &str) -> Nbt {
    Nbt::Compound(vec![("translate".into(), Nbt::String(key.into()))])
}

#[cfg(test)]
pub(crate) fn test_raid(center: [i32; 3], diff: i32) -> Raid {
    Raid::new(2, center, diff)
}

#[cfg(test)]
pub(crate) fn test_dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    use super::super::mth::dist2;
    dist2(a, b)
}
