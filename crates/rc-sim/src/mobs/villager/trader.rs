//! Wandering trader (F05): goals (drink an invisibility potion at night, milk by day,
//! trade, flee, wander toward its target, stay in its area, stroll), despawn delay,
//! periodic spawner near the player and its llamas.

use rc_proto::nbt::Nbt;

use super::super::goals::{Goal, Selector, Who};
use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use crate::player::Stack;
use crate::server::Server;

const MOVE: u8 = 1;
const LOOK: u8 = 2;
const JUMP: u8 = 4;

/// Goals specific to the wandering trader.
#[derive(Debug, Clone)]
pub enum TGoal {
    /// Drink (invisibility potion at night, milk by day): remaining duration.
    UseItem {
        potion: bool,
    },
    TradeWithPlayer,
    LookAtTradingPlayer {
        time: i32,
    },
    WanderToPosition,
    MoveTowardsRestriction {
        wanted: [f64; 3],
    },
    Stroll {
        wanted: [f64; 3],
    },
    /// Look (player at 3 blocks, interaction) or any mob at 8 blocks.
    Look {
        dist: f32,
        prob: f32,
        player: bool,
        flags: u8,
        at: Option<Ent>,
        time: i32,
    },
}

impl TGoal {
    pub fn flags(&self) -> u8 {
        match self {
            TGoal::UseItem { .. } => 0,
            TGoal::TradeWithPlayer => JUMP | MOVE,
            TGoal::LookAtTradingPlayer { .. } => LOOK,
            TGoal::WanderToPosition | TGoal::MoveTowardsRestriction { .. } | TGoal::Stroll { .. } => MOVE,
            TGoal::Look { flags, .. } => *flags,
        }
    }
}

/// Goals in the game's order.
pub fn register_goals(g: &mut Selector) {
    use super::super::species::Kind;
    let avoid = |who: Who, max: f32| Goal::Avoid { who, max, walk: 0.5, sprint: 0.5, from: None, path: None };
    g.add(0, Goal::Float);
    g.add(0, Goal::Npc(TGoal::UseItem { potion: true }));
    g.add(0, Goal::Npc(TGoal::UseItem { potion: false }));
    g.add(1, Goal::Npc(TGoal::TradeWithPlayer));
    g.add(1, avoid(Who::ZombieClass, 8.0));
    g.add(1, avoid(Who::Exact(Kind::Evoker), 12.0));
    g.add(1, avoid(Who::Exact(Kind::Vindicator), 8.0));
    g.add(1, avoid(Who::Exact(Kind::Vex), 8.0));
    g.add(1, avoid(Who::Exact(Kind::Pillager), 15.0));
    g.add(1, avoid(Who::Exact(Kind::Illusioner), 12.0));
    g.add(1, avoid(Who::Exact(Kind::Zoglin), 10.0));
    g.add(1, Goal::Panic { speed: 0.5, pos: [0.0; 3], tag: "minecraft:panic_causes" });
    g.add(1, Goal::Npc(TGoal::LookAtTradingPlayer { time: 0 }));
    g.add(2, Goal::Npc(TGoal::WanderToPosition));
    g.add(4, Goal::Npc(TGoal::MoveTowardsRestriction { wanted: [0.0; 3] }));
    g.add(8, Goal::Npc(TGoal::Stroll { wanted: [0.0; 3] }));
    g.add(9, Goal::Npc(TGoal::Look { dist: 3.0, prob: 1.0, player: true, flags: LOOK | MOVE, at: None, time: 0 }));
    g.add(10, Goal::Npc(TGoal::Look { dist: 8.0, prob: 0.02, player: false, flags: LOOK, at: None, time: 0 }));
}

fn home_of(m: &Mob) -> super::rpos::Home {
    m.npc.as_ref().and_then(|n| n.home)
}

fn closer_center(p: [i32; 3], pos: [f64; 3], d: f64) -> bool {
    super::behave::closer_center(p, pos, d)
}

impl Server {
    fn dark_outside(&self) -> bool {
        self.dim.name == "minecraft:overworld" && self.mob_sky_darken() >= 4
    }

    pub(crate) fn tgoal_can_use(&mut self, g: &mut TGoal, m: &mut Mob) -> bool {
        match g {
            TGoal::UseItem { potion } => {
                if *potion {
                    self.dark_outside() && !m.invisible()
                } else {
                    self.bright_outside() && m.invisible()
                }
            }
            TGoal::TradeWithPlayer => {
                m.alive()
                    && !m.in_water
                    && m.on_ground
                    && !self.hurt_recently(m)
                    && self.npc_trading(m)
                    && super::brain::dist2(m.pos, self.pos) <= 16.0
            }
            TGoal::LookAtTradingPlayer { .. } => self.npc_trading(m),
            TGoal::WanderToPosition => {
                m.npc.as_ref().and_then(|n| n.wander).is_some_and(|w| !closer_center(w, m.pos, 2.0))
            }
            TGoal::MoveTowardsRestriction { wanted } => {
                let Some((c, r)) = home_of(m) else { return false };
                let b = m.block_pos();
                let d = [b[0] - c[0], b[1] - c[1], b[2] - c[2]];
                let within =
                    i64::from(d[0]).pow(2) + i64::from(d[1]).pow(2) + i64::from(d[2]).pow(2) < i64::from(r).pow(2);
                if within {
                    return false;
                }
                let to = [f64::from(c[0]) + 0.5, f64::from(c[1]), f64::from(c[2]) + 0.5];
                let home = home_of(m);
                match self.rpos_default_towards(m, home, 16, 7, to, f64::from(std::f32::consts::FRAC_PI_2)) {
                    Some(p) => {
                        *wanted = p;
                        true
                    }
                    None => false,
                }
            }
            TGoal::Stroll { wanted } => {
                if m.no_action_time >= 100 {
                    return false;
                }
                if m.rng.next_int_bound(super::super::mth::reduced(120)) != 0 {
                    return false;
                }
                let home = home_of(m);
                let p = if m.in_water {
                    self.rpos_land(m, home, 15, 7, &|_, _| 0.0).or_else(|| self.rpos_default(m, home, 10, 7))
                } else if m.rng.next_float() >= 0.001 {
                    self.rpos_land(m, home, 10, 7, &|_, _| 0.0)
                } else {
                    self.rpos_default(m, home, 10, 7)
                };
                match p {
                    Some(p) => {
                        *wanted = p;
                        true
                    }
                    None => false,
                }
            }
            TGoal::Look { dist, prob, player, at, .. } => {
                if m.rng.next_float() >= *prob {
                    return false;
                }
                if let Some(t) = m.target {
                    *at = Some(t);
                }
                let d = f64::from(*dist);
                *at = if *player {
                    let ok = self.game_mode != 3
                        && !self.carries_player(m)
                        && self.targeting_ok(m, Ent::Player, false, d, true, true);
                    ok.then_some(Ent::Player)
                } else {
                    let bb = m.bbox();
                    let mut best: Option<(f64, Ent)> = None;
                    let ids: Vec<(i32, [f64; 3])> = self
                        .mobs
                        .list
                        .iter()
                        .flatten()
                        .filter(|o| {
                            o.id != m.id
                                && o.alive()
                                && o.pos[0] > bb.min[0] - d - 0.5
                                && o.pos[0] < bb.max[0] + d + 0.5
                                && o.pos[1] + f64::from(o.height()) > bb.min[1] - 3.0
                                && o.pos[1] < bb.max[1] + 3.0
                                && o.pos[2] > bb.min[2] - d - 0.5
                                && o.pos[2] < bb.max[2] + d + 0.5
                        })
                        .map(|o| (o.id, o.pos))
                        .collect();
                    let eye = [m.pos[0], m.eye_y(), m.pos[2]];
                    for (id, p) in ids {
                        let e = Ent::Mob(id);
                        if !self.targeting_ok(m, e, false, d, true, true) {
                            continue;
                        }
                        let dd = super::brain::dist2(eye, p);
                        if best.is_none_or(|b| dd < b.0) {
                            best = Some((dd, e));
                        }
                    }
                    best.map(|b| b.1)
                };
                at.is_some()
            }
        }
    }

    pub(crate) fn tgoal_can_continue(&mut self, g: &mut TGoal, m: &mut Mob) -> bool {
        match g {
            TGoal::UseItem { .. } => m.npc.as_ref().is_some_and(|n| n.drinking > 0),
            TGoal::TradeWithPlayer => {
                // The default continuation test: the start condition.
                m.alive()
                    && !m.in_water
                    && m.on_ground
                    && !self.hurt_recently(m)
                    && self.npc_trading(m)
                    && super::brain::dist2(m.pos, self.pos) <= 16.0
            }
            TGoal::LookAtTradingPlayer { time } => {
                self.ent_alive(Ent::Player) && super::brain::dist2(m.pos, self.pos) <= 64.0 && *time > 0
            }
            TGoal::WanderToPosition => {
                m.npc.as_ref().and_then(|n| n.wander).is_some_and(|w| !closer_center(w, m.pos, 2.0))
            }
            TGoal::MoveTowardsRestriction { .. } | TGoal::Stroll { .. } => !m.nav.is_done(),
            TGoal::Look { dist, at, time, .. } => {
                let Some(e) = *at else { return false };
                let Some(p) = self.ent_pos(e) else { return false };
                self.ent_alive(e) && super::brain::dist2(m.pos, p) <= f64::from(*dist * *dist) && *time > 0
            }
        }
    }

    pub(crate) fn tgoal_start(&mut self, g: &mut TGoal, m: &mut Mob) {
        match g {
            TGoal::UseItem { potion } => {
                let item = if *potion {
                    let mut s = Stack { id: "minecraft:potion".into(), count: 1, components: None };
                    crate::items::set_component(
                        &mut s,
                        "minecraft:potion_contents",
                        Nbt::Compound(vec![("potion".into(), Nbt::String("minecraft:invisibility".into()))]),
                    );
                    s
                } else {
                    Stack { id: "minecraft:milk_bucket".into(), count: 1, components: None }
                };
                m.equipment[0] = Some(item);
                self.send_main_hand(m);
                if let Some(n) = m.npc.as_mut() {
                    // Use duration of a potion and a milk bucket: 32 ticks.
                    n.drinking = 32;
                }
            }
            TGoal::TradeWithPlayer => m.nav.stop(),
            TGoal::LookAtTradingPlayer { time } => *time = super::super::mth::reduced(40 + m.rng.next_int_bound(40)),
            TGoal::MoveTowardsRestriction { wanted } | TGoal::Stroll { wanted } => {
                let w = *wanted;
                self.move_to_pos(m, w, 0.35);
            }
            TGoal::Look { time, .. } => *time = super::super::mth::reduced(40 + m.rng.next_int_bound(40)),
            TGoal::WanderToPosition => {}
        }
    }

    pub(crate) fn tgoal_stop(&mut self, g: &mut TGoal, m: &mut Mob) {
        match g {
            TGoal::UseItem { potion } => {
                m.equipment[0] = None;
                self.send_main_hand(m);
                if let Some(n) = m.npc.as_mut() {
                    n.drinking = 0;
                }
                let s = if *potion {
                    "minecraft:entity.wandering_trader.disappeared"
                } else {
                    "minecraft:entity.wandering_trader.reappeared"
                };
                let pitch = m.rng.next_float() * 0.2 + 0.9;
                self.mob_sound(m, s, 1.0, pitch);
            }
            TGoal::TradeWithPlayer => self.npc_stop_trading(m),
            TGoal::WanderToPosition => {
                if let Some(n) = m.npc.as_mut() {
                    n.wander = None;
                }
                m.nav.stop();
            }
            TGoal::Stroll { .. } => m.nav.stop(),
            TGoal::Look { at, .. } => *at = None,
            _ => {}
        }
    }

    pub(crate) fn tgoal_tick(&mut self, g: &mut TGoal, m: &mut Mob) {
        match g {
            TGoal::LookAtTradingPlayer { time } => {
                let eye = self.player_eye_y();
                self.look_at(m, [self.pos[0], eye, self.pos[2]], 10.0, 40.0);
                *time -= 1;
            }
            TGoal::Look { at, time, .. } => {
                let Some(e) = *at else { return };
                if self.ent_alive(e)
                    && let (Some(p), Some(y)) = (self.ent_pos(e), self.ent_eye(e))
                {
                    self.look_at(m, [p[0], y, p[2]], 10.0, 40.0);
                    *time -= 1;
                }
            }
            TGoal::WanderToPosition => {
                let Some(w) = m.npc.as_ref().and_then(|n| n.wander) else { return };
                if !m.nav.is_done() {
                    return;
                }
                if !closer_center(w, m.pos, 10.0) {
                    let d = [f64::from(w[0]) - m.pos[0], f64::from(w[1]) - m.pos[1], f64::from(w[2]) - m.pos[2]];
                    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    let n = if l < 1.0e-4 { [0.0; 3] } else { [d[0] / l, d[1] / l, d[2] / l] };
                    let t = [n[0] * 10.0 + m.pos[0], n[1] * 10.0 + m.pos[1], n[2] * 10.0 + m.pos[2]];
                    self.move_to_pos(m, t, 0.35);
                } else {
                    self.move_to_pos(m, [f64::from(w[0]), f64::from(w[1]), f64::from(w[2])], 0.35);
                }
            }
            _ => {}
        }
    }

    /// Use of the held item (drinking) and despawn delay.
    pub(crate) fn trader_post_tick(&mut self, m: &mut Mob) {
        let drinking = m.npc.as_ref().map_or(0, |n| n.drinking);
        if drinking > 0 {
            let left = drinking - 1;
            if let Some(n) = m.npc.as_mut() {
                n.drinking = left;
            }
            if left > 0 && left % 4 == 0 {
                let snd = if m.equipment[0].as_ref().is_some_and(|s| s.id == "minecraft:milk_bucket") {
                    "minecraft:entity.wandering_trader.drink_milk"
                } else {
                    "minecraft:entity.wandering_trader.drink_potion"
                };
                let pitch = m.rng.next_float() * 0.2 + 0.9;
                self.mob_sound(m, snd, 0.5, pitch);
            }
            if left == 0 {
                // End of use: effects of the item.
                if m.equipment[0].as_ref().is_some_and(|s| s.id == "minecraft:milk_bucket") {
                    m.x.effects.clear();
                    super::super::effects::apply_mob_modifiers(m);
                } else {
                    self.give_mob_effect(m, "minecraft:invisibility", 3600, 0);
                }
            }
        }
        let trading = self.npc_trading(m);
        if let Some(n) = m.npc.as_mut()
            && n.despawn_delay > 0
            && !trading
        {
            n.despawn_delay -= 1;
            if n.despawn_delay == 0 {
                m.removed = true;
            }
        }
    }

    // ---------- Spawner ----------

    fn trader_data_path(&self) -> std::path::PathBuf {
        self.world.join("data").join("minecraft").join("wandering_trader.dat")
    }

    fn trader_load(&mut self) {
        if self.mobs.npc.loaded {
            return;
        }
        self.mobs.npc.loaded = true;
        let Ok(bytes) = std::fs::read(self.trader_data_path()) else { return };
        let raw = crate::nbtio::decompress_auto(&bytes).unwrap_or(bytes);
        let Ok(root) = crate::nbtio::parse_root(&raw) else { return };
        let d = root.get("data").unwrap_or(&root);
        self.mobs.npc.spawn_delay = crate::nbtio::int(d, "spawn_delay").map_or(24000, |v| v as i32);
        self.mobs.npc.spawn_chance = crate::nbtio::int(d, "spawn_chance").map_or(25, |v| v as i32);
    }

    /// Spawner file (`data/minecraft/wandering_trader.dat`).
    pub(crate) fn trader_save_job(&mut self, jobs: &mut Vec<crate::save::Job>) {
        if !self.mobs.npc.loaded {
            return;
        }
        let root = Nbt::Compound(vec![
            (
                "data".into(),
                Nbt::Compound(vec![
                    ("spawn_delay".into(), Nbt::Int(self.mobs.npc.spawn_delay)),
                    ("spawn_chance".into(), Nbt::Int(self.mobs.npc.spawn_chance)),
                ]),
            ),
            ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
        ]);
        jobs.push(crate::save::Job::Dat {
            path: self.trader_data_path(),
            nbt: crate::nbtio::encode_root(&root),
            backup: false,
        });
    }

    /// The wandering trader spawn tick (own random, seeded from the clock).
    pub(crate) fn trader_spawner_tick(&mut self) {
        if self.dim.name != "minecraft:overworld" || !self.rule("minecraft:spawn_wandering_traders", true) {
            return;
        }
        self.mobs.npc.tick_delay -= 1;
        if self.mobs.npc.tick_delay > 0 {
            return;
        }
        self.mobs.npc.tick_delay = 1200;
        self.trader_load();
        let delay = self.mobs.npc.spawn_delay - 1200;
        self.mobs.npc.spawn_delay = delay;
        if delay > 0 {
            return;
        }
        self.mobs.npc.spawn_delay = 24000;
        let chance = self.mobs.npc.spawn_chance;
        self.mobs.npc.spawn_chance = (chance + 25).clamp(25, 75);
        if self.mobs.npc.rng.next_int_bound(100) <= chance && self.trader_spawn() {
            self.mobs.npc.spawn_chance = 25;
        }
    }

    fn trader_spawn(&mut self) -> bool {
        if !self.ent_alive(Ent::Player) {
            return true;
        }
        if self.mobs.npc.rng.next_int_bound(10) != 0 {
            return false;
        }
        let pp = super::super::mth::block_of(self.pos);
        let bell = self
            .poi_in_range(|t| t == crate::poi::PoiType::Meeting, pp, 48, crate::poi::Occupancy::Any)
            .first()
            .map(|r| r.pos);
        let reference = bell.unwrap_or(pp);
        let Some(spawn) = self.trader_find_spawn(reference, 48) else { return false };
        if !self.trader_has_space(spawn) {
            return false;
        }
        if self.biome_tag(spawn, "minecraft:without_wandering_trader_spawns") {
            return false;
        }
        let yaw = super::super::mth::wrap(self.mobs.rng.next_float() * 360.0);
        let pos = [f64::from(spawn[0]) + 0.5, f64::from(spawn[1]), f64::from(spawn[2]) + 0.5];
        let mut t = self.new_mob(Kind::WanderingTrader, pos, yaw);
        t.head_yaw = yaw;
        t.body_yaw = yaw;
        let mut g = super::super::spawn::Group::default();
        self.finalize_spawn(&mut t, super::super::spawn::Reason::Event, &mut g);
        let tp = t.block_pos();
        let tid = t.id;
        // The trader enters the world before its llamas (like the game).
        self.add_mob(t);
        for _ in 0..2 {
            // Trader's llamas (F04): created, finalized ("event") and added; the leash
            // that attaches them to the trader is not simulated.
            if let Some(p) = self.trader_find_spawn(tp, 4) {
                let yaw = super::super::mth::wrap(self.mobs.rng.next_float() * 360.0);
                let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
                let mut l = self.new_mob(Kind::TraderLlama, pos, yaw);
                l.head_yaw = yaw;
                l.body_yaw = yaw;
                let mut lg = super::super::spawn::Group::default();
                self.finalize_spawn(&mut l, super::super::spawn::Reason::Event, &mut lg);
                self.add_mob(l);
            }
        }
        if let Some(i) = self.mob_index(tid)
            && let Some(n) = self.mobs.list[i].as_mut().and_then(|t| t.npc.as_mut())
        {
            n.despawn_delay = 48000;
            n.wander = Some(reference);
            n.home = Some((reference, 16));
        }
        true
    }

    /// 10 tries at heightmap height, "on the ground" placement.
    fn trader_find_spawn(&mut self, r: [i32; 3], radius: i32) -> Option<[i32; 3]> {
        for _ in 0..10 {
            let x = r[0] + self.mobs.npc.rng.next_int_bound(radius * 2) - radius;
            let z = r[2] + self.mobs.npc.rng.next_int_bound(radius * 2) - radius;
            let y = self.surface_y(x, z, 5);
            let p = [x, y, z];
            if self.chunks.get(&(x >> 4, z >> 4)).is_some_and(Option::is_some)
                && self.valid_spawn_on([x, y - 1, z], Kind::WanderingTrader)
                && self.valid_empty_spawn(p, Kind::WanderingTrader)
                && self.valid_empty_spawn([x, y + 1, z], Kind::WanderingTrader)
            {
                return Some(p);
            }
        }
        None
    }

    /// 2×3×2 blocks without a collision shape.
    fn trader_has_space(&self, p: [i32; 3]) -> bool {
        for x in 0..=1 {
            for y in 0..=2 {
                for z in 0..=1 {
                    if self.has_collision([p[0] + x, p[1] + y, p[2] + z]) {
                        return false;
                    }
                }
            }
        }
        true
    }
}
