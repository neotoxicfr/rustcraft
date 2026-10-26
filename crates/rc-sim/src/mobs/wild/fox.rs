//! Fox (§ 4.F11): red and snow variants, item in mouth (picked up, spat out, eaten),
//! trust (two UUIDs), sleep sheltered by day, stalking and pounce on the prey (face planted in
//! the snow), defense of trusted beings, picked berries, nocturnal cry.

use super::super::goals::{Goal, TBase, Who};
use super::super::mob::{Ent, Mob};
use super::super::mth::reduced;
use super::super::species::Kind;
use super::goals::WGoal;
use crate::player::Stack;
use crate::server::Server;

pub const SITTING: u8 = 1;
pub const CROUCHING: u8 = 4;
pub const INTERESTED: u8 = 8;
pub const POUNCING: u8 = 16;
pub const SLEEPING: u8 = 32;
pub const FACEPLANTED: u8 = 64;
pub const DEFENDING: u8 = 128;

pub fn flag(m: &Mob, f: u8) -> bool {
    m.wild.flag(f)
}

pub fn set(m: &mut Mob, f: u8, v: bool) {
    m.wild.set_flag(f, v);
}

/// First free slot, otherwise the second is replaced.
pub fn add_trusted(m: &mut Mob, u: u128) {
    if m.wild.trusted[0].is_some() {
        m.wild.trusted[1] = Some(u);
    } else {
        m.wild.trusted[0] = Some(u);
    }
}

pub fn trusts(m: &Mob, u: u128) -> bool {
    m.wild.trusted.contains(&Some(u))
}

pub fn trusts_player(s: &Server, m: &Mob) -> bool {
    trusts(m, s.uuid)
}

/// Neither asleep, nor sitting, nor face-planted.
pub fn can_move(m: &Mob) -> bool {
    !flag(m, SLEEPING) && !flag(m, SITTING) && !flag(m, FACEPLANTED)
}

/// The clearing of the states.
pub fn clear_states(m: &mut Mob) {
    for f in [INTERESTED, CROUCHING, SITTING, SLEEPING, DEFENDING, FACEPLANTED] {
        set(m, f, false);
    }
}

/// Losing its target ends the defense.
pub fn set_target(m: &mut Mob, t: Option<Ent>) {
    if flag(m, DEFENDING) && t.is_none() {
        set(m, DEFENDING, false);
    }
    m.target = t;
}

fn nearest_target(who: Who, interval: i32) -> Goal {
    Goal::Nearest {
        base: TBase::new(false, false),
        who,
        interval: reduced(interval),
        target: None,
        angry_only: false,
        non_tame: false,
    }
}

/// The setup of the targeting objectives (finalization and read-back): red, land prey first; snow,
/// fish first. Only the first call adds: the goals are unique instances in a
/// set, the second add (priorities of the final variant) changes nothing (like the game).
pub fn set_target_goals(m: &mut Mob) {
    if m.wild.fox_targets {
        return;
    }
    m.wild.fox_targets = true;
    let land = || nearest_target(Who::FoxPrey, 10);
    let turtle = || nearest_target(Who::BabyTurtleOnLand, 10);
    let fish = || nearest_target(Who::SchoolingFish, 20);
    let t = &mut m.targets;
    if m.variant == 0 {
        t.add(4, land());
        t.add(4, turtle());
        t.add(6, fish());
    } else {
        t.add(4, fish());
        t.add(6, land());
        t.add(6, turtle());
    }
}

/// Goals in the game's order; the sleep delay is drawn at construction.
pub fn register(m: &mut Mob) {
    use super::goals::Gate;
    let wrap = |gate: Gate, inner: Goal| Goal::Wild(WGoal::Gate { gate, inner: Box::new(inner) });
    let sleep_countdown = m.rng.next_int_bound(reduced(140));
    let g = &mut m.goals;
    g.add(0, Goal::Wild(WGoal::FoxFloat));
    g.add(0, Goal::Surf(super::super::goals::surface_goals::SGoal::ClimbPowderSnow));
    g.add(1, Goal::Wild(WGoal::Faceplant { countdown: 0 }));
    g.add(2, wrap(Gate::FoxPanic, Goal::Panic { speed: 2.2, pos: [0.0; 3], tag: "minecraft:panic_causes" }));
    g.add(3, wrap(Gate::FoxBreed, Goal::Breed { speed: 1.0, partner: None, love_time: 0 }));
    let avoid = |who: Who, max: f32| Goal::Avoid { who, max, walk: 1.6, sprint: 1.4, from: None, path: None };
    g.add(4, wrap(Gate::FoxAvoidPlayer, avoid(Who::Player, 16.0)));
    g.add(4, wrap(Gate::FoxNotDefending, avoid(Who::WildWolf, 8.0)));
    g.add(4, wrap(Gate::FoxNotDefending, avoid(Who::PolarBear, 8.0)));
    g.add(5, Goal::Wild(WGoal::StalkPrey));
    g.add(6, Goal::Wild(WGoal::Pounce));
    g.add(6, Goal::Wild(WGoal::SeekShelter { interval: reduced(100), wanted: [0.0; 3] }));
    let melee = Goal::Melee {
        speed: f64::from(1.2f32),
        unseen: true,
        kind: super::super::goals::Melee::Fox,
        path: None,
        pathed: [0.0; 3],
        recalc: 0,
        cd: 0,
        last: i64::MIN / 2,
    };
    g.add(7, wrap(Gate::FoxMelee, melee));
    g.add(7, Goal::Wild(WGoal::Sleep { countdown: sleep_countdown }));
    g.add(8, wrap(Gate::FoxFollowParent, Goal::FollowParent { speed: 1.25, parent: None, recalc: 0 }));
    g.add(9, Goal::Wild(WGoal::StrollVillage { interval: reduced(200), wanted: None }));
    g.add(
        10,
        Goal::Wild(WGoal::EatBerries {
            next_start: 0,
            try_ticks: 0,
            max_stay: 0,
            pos: [0; 3],
            reached: false,
            waited: 0,
        }),
    );
    g.add(10, Goal::Leap { yd: 0.4, target: None });
    g.add(11, Goal::Stroll { speed: 1.0, interval: 120, prob: Some(0.001), wanted: [0.0; 3] });
    g.add(11, Goal::Wild(WGoal::SearchItems));
    g.add(12, wrap(Gate::FoxLook, Goal::LookAtPlayer { dist: 24.0, prob: 0.02, at: None, time: 0 }));
    g.add(13, Goal::Wild(WGoal::Perch { rel: [0.0; 2], look_time: 0, looks: 0 }));
    m.targets.add(3, Goal::Wild(WGoal::DefendTrusted { base: TBase::new(false, false), by: None, ts: 0, stamp: 0 }));
}

// ---------- Spawning ----------

/// Variant of the biome (or of the group; from the 3rd member, babies), targeting goals,
/// item in mouth (20%).
pub fn finalize_spawn(s: &mut Server, m: &mut Mob, group: &mut super::super::spawn::Group) {
    let p = m.block_pos();
    let mut variant = i32::from(s.biome_tag(p, "minecraft:spawns_snow_foxes"));
    let mut baby = false;
    match group.surf_variant {
        Some(v) => {
            variant = v;
            if group.size >= 2 {
                baby = true;
            }
        }
        None => {
            group.surf_variant = Some(variant);
            group.ageable = true;
            group.no_baby = true;
        }
    }
    m.variant = variant;
    if baby {
        m.age = -24000;
    }
    set_target_goals(m);
    populate_equipment(s, m);
}

/// The default equipment (world random).
fn populate_equipment(s: &mut Server, m: &mut Mob) {
    let r = &mut s.mobs.rng;
    if r.next_float() >= 0.2 {
        return;
    }
    let odds = r.next_float();
    let item = if odds < 0.05 {
        "minecraft:emerald"
    } else if odds < 0.2 {
        "minecraft:egg"
    } else if odds < 0.4 {
        if r.next_bool() { "minecraft:rabbit_foot" } else { "minecraft:rabbit_hide" }
    } else if odds < 0.6 {
        "minecraft:wheat"
    } else if odds < 0.8 {
        "minecraft:leather"
    } else {
        "minecraft:feather"
    };
    m.equipment[0] = Some(Stack { id: item.into(), count: 1, components: None });
}

// ---------- Ticks ----------

/// Consumable item (food and consumable components).
fn consumable_food(s: &Server, st: &Stack) -> bool {
    s.life.items.default_component(st, "minecraft:food").is_some()
        && s.life.items.default_component(st, "minecraft:consumable").is_some()
}

fn can_eat(s: &Server, m: &Mob) -> bool {
    m.equipment[0].as_ref().is_some_and(|st| consumable_food(s, st))
        && m.target.is_none()
        && m.on_ground
        && !flag(m, SLEEPING)
}

/// Start of the AI step: eats the item in mouth after 600 ticks (signs after 560), loses interest
/// without a target, motionless asleep.
pub fn pre_ai_step(s: &mut Server, m: &mut Mob) {
    if m.alive() && !m.no_ai {
        m.wild.since_eaten += 1;
        if can_eat(s, m) {
            if m.wild.since_eaten > 600 {
                eat_mouth_item(s, m);
                m.wild.since_eaten = 0;
            } else if m.wild.since_eaten > 560 && m.rng.next_float() < 0.1 {
                s.mob_sound(m, "minecraft:entity.fox.eat", 1.0, 1.0);
                s.mob_event(m, 45);
            }
        }
        if m.target.is_none_or(|t| !s.ent_alive(t)) {
            set(m, CROUCHING, false);
            set(m, INTERESTED, false);
        }
    }
    if flag(m, SLEEPING) || m.dead || m.health <= 0.0 {
        m.jumping = false;
        m.xxa = 0.0;
        m.zza = 0.0;
    }
}

/// The end of use of a food item by the fox: sound and particles (draws from
/// the entity random), one piece consumed, possible use remainder (bowl).
pub(crate) fn eat_mouth_item(s: &mut Server, m: &mut Mob) {
    let Some(st) = m.equipment[0].clone() else { return };
    let cons = s.comp_json(&st, "minecraft:consumable");
    let drink = cons.as_ref().is_some_and(|c| c["animation"].as_str() == Some("drink"));
    let particles = cons.as_ref().is_none_or(|c| c["has_consume_particles"].as_bool().unwrap_or(true));
    let sound = cons
        .as_ref()
        .and_then(|c| c["sound"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "minecraft:entity.generic.eat".into());
    let eat_volume = if m.rng.next_bool() { 0.5 } else { 1.0 };
    let eat_pitch = 1.0 + 0.2 * (m.rng.next_float() - m.rng.next_float());
    let drink_pitch = m.rng.next_float() * 0.1 + 0.9;
    if particles {
        for _ in 0..16 {
            for _ in 0..4 {
                let _ = m.rng.next_float();
            }
        }
    }
    let (v, p) = if drink { (0.5, drink_pitch) } else { (eat_volume, eat_pitch) };
    s.mob_sound(m, &sound, v, p);
    // G5-F10: consumption effects of the item (chorus fruit: teleportation with the fox's sound;
    // potion effects…), then the `eat` or `drink` event.
    if let Some(list) = cons.as_ref().and_then(|c| c["on_consume_effects"].as_array()) {
        s.run_mob_consume_effects(m, list);
    }
    let ev = if drink { "minecraft:drink" } else { "minecraft:eat" };
    s.game_event_at(ev, m.pos, Some(crate::mobs::mob::Ent::Mob(m.id)));
    let remainder = s.life.items.default_component(&st, "minecraft:use_remainder");
    let mut left = st;
    left.count -= 1;
    m.equipment[0] = if left.count > 0 {
        Some(left)
    } else {
        remainder.and_then(|r| {
            let id = r["id"].as_str()?.to_owned();
            Some(Stack { id, count: r["count"].as_i64().unwrap_or(1) as i32, components: None })
        })
    };
}

/// End of `tick`: waking (water, target, thunderstorm), standing asleep or in water, face-planted dust
/// (world random, event 2001), crouching posture.
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    if !m.no_ai {
        let in_water = m.in_water;
        if in_water || m.target.is_some() || s.life.weather.thunder_eff() > 0.9 {
            set(m, SLEEPING, false);
        }
        if in_water || flag(m, SLEEPING) {
            set(m, SITTING, false);
        }
        if flag(m, FACEPLANTED) && s.mobs.rng.next_float() < 0.2 {
            let p = m.block_pos();
            let st = s.st(p);
            s.level_event(2001, p, st as i32);
        }
    }
    if flag(m, CROUCHING) {
        m.wild.crouch = (m.wild.crouch + 0.2).min(5.0);
    } else {
        m.wild.crouch = 0.0;
    }
}

/// End of the AI step: defense growl (5%), pickup.
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    if flag(m, DEFENDING) && m.rng.next_float() < 0.05 {
        s.mob_sound(m, "minecraft:entity.fox.aggro", 1.0, 1.0);
    }
    pickup(s, m);
}

/// Fox gaze control: no gaze asleep; pitch kept when stalking, pouncing, face planted.
pub fn look_control(m: &mut Mob) -> bool {
    if flag(m, SLEEPING) {
        return true;
    }
    let reset = !flag(m, POUNCING) && !flag(m, CROUCHING) && !flag(m, INTERESTED) && !flag(m, FACEPLANTED);
    super::bee::look_tick(m, reset);
    true
}

// ---------- Sounds ----------

/// Sleep; at night, 1 in 10, without a player within 16 blocks: cry; otherwise ordinary.
pub fn ambient_sound(s: &Server, m: &mut Mob) -> &'static str {
    if flag(m, SLEEPING) {
        return "minecraft:entity.fox.sleep";
    }
    if !s.bright_outside() && m.rng.next_float() < 0.1 {
        let b = m.bbox();
        let area = rc_physics::Aabb { min: b.min.map(|v| v - 16.0), max: b.max.map(|v| v + 16.0) };
        let player_near = s.game_mode != 3 && s.player_aabb().intersects(&area);
        if !player_near {
            return "minecraft:entity.fox.screech";
        }
    }
    "minecraft:entity.fox.ambient"
}

// ---------- Items ----------

/// Empty mouth, or food that replaces an inedible item.
fn can_hold(s: &Server, m: &Mob, st: &Stack) -> bool {
    match &m.equipment[0] {
        None => true,
        Some(held) => m.wild.since_eaten > 0 && consumable_food(s, st) && !consumable_food(s, held),
    }
}

/// Pickup (the mobs' common AI step then the fox's item pickup): one piece taken, the rest put back, the old
/// item spat out in front of it (delay 40).
pub fn pickup(s: &mut Server, m: &mut Mob) {
    if !m.can_pickup || !m.alive() || m.dead || !s.rule("minecraft:mob_griefing", true) {
        return;
    }
    let bb = m.bbox();
    let hit = s
        .life
        .drops
        .iter()
        .filter(|d| {
            !d.is_removed()
                && d.stack.count > 0
                && d.pickup_delay <= 0
                && d.pos[0] + 0.125 > bb.min[0] - 1.0
                && d.pos[0] - 0.125 < bb.max[0] + 1.0
                && d.pos[1] + 0.25 > bb.min[1]
                && d.pos[1] < bb.max[1]
                && d.pos[2] + 0.125 > bb.min[2] - 1.0
                && d.pos[2] - 0.125 < bb.max[2] + 1.0
        })
        .map(|d| (d.id, d.stack.clone()))
        .find(|(_, st)| can_hold(s, m, st));
    let Some((id, st)) = hit else { return };
    if st.count > 1 {
        let rest = Stack { count: st.count - 1, ..st.clone() };
        s.spawn_item(m.pos, [0.0; 3], rest, 0);
    }
    if let Some(old) = m.equipment[0].take() {
        spit_out(s, m, old);
    }
    m.equipment[0] = Some(Stack { count: 1, ..st });
    m.drop_chance[0] = 2.0;
    s.mob_take(id, m.id, 1);
    m.wild.since_eaten = 0;
}

/// In front of the fox, one cell above, pickup delay 40, sound.
fn spit_out(s: &mut Server, m: &Mob, st: Stack) {
    let (yr, xr) = (m.yaw.to_radians(), m.pitch.to_radians());
    let look = [-(yr.sin() * xr.cos()) as f64, -(xr.sin()) as f64, (yr.cos() * xr.cos()) as f64];
    let at = [m.pos[0] + look[0], m.pos[1] + 1.0, m.pos[2] + look[2]];
    let r = &mut s.life.rng;
    let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
    s.spawn_item(at, v, st, 40);
    s.mob_sound(m, "minecraft:entity.fox.spit", 1.0, 1.0);
}

// ---------- Goal tools ----------

/// Six steps toward the target, three replaceable cells above each.
pub fn path_clear(s: &Server, m: &Mob, tp: [f64; 3]) -> bool {
    let zd = tp[2] - m.pos[2];
    let xd = tp[0] - m.pos[0];
    let slope = zd / xd;
    for i in 0..6 {
        let k = f64::from(i as f32 / 6.0);
        let z = if slope == 0.0 { 0.0 } else { zd * k };
        let x = if slope == 0.0 { xd * k } else { z / slope };
        for j in 1..4 {
            let p = super::super::mth::block_of([m.pos[0] + x, m.pos[1] + f64::from(j), m.pos[2] + z]);
            if !s.sflag(p, rc_data::flags::REPLACEABLE) {
                return false;
            }
        }
    }
    true
}

/// Top cell of the box without sky, positive walk value.
pub fn has_shelter(s: &Server, m: &Mob) -> bool {
    let p = super::super::mth::block_of([m.pos[0], m.bbox().max[1], m.pos[2]]);
    !s.can_see_sky(p) && s.walk_target_value(m, p) >= 0.0
}

/// Alerting mob; other living beings alert
/// when awake and standing (sleeping villager: no).
fn alertable_mob(o: &Mob) -> bool {
    match o.kind {
        Kind::Fox => false,
        Kind::Chicken | Kind::Rabbit => true,
        k if k.is_monster() => true,
        Kind::Wolf | Kind::Cat | Kind::Parrot => !o.tame,
        _ => o.npc.as_ref().is_none_or(|n| n.sleeping.is_none()) && !o.surf.crouching,
    }
}

/// `alertable`: worrying living being at ±12/±6/±12 (sight ignored, visibility counted).
pub fn alertable(s: &Server, m: &mut Mob) -> bool {
    let b = m.bbox();
    let area = rc_physics::Aabb {
        min: [b.min[0] - 12.0, b.min[1] - 6.0, b.min[2] - 12.0],
        max: [b.max[0] + 12.0, b.max[1] + 6.0, b.max[2] + 12.0],
    };
    // Player: neither spectator nor creative, no trust, neither asleep nor crouching.
    if s.player_aabb().intersects(&area)
        && s.game_mode != 1
        && s.game_mode != 3
        && !trusts_player(s, m)
        && !s.sneaking
        && s.life.vit.sleeping.is_none()
        && s.targeting_ok(m, Ent::Player, true, 12.0, false, true)
    {
        return true;
    }
    let cands: Vec<i32> = s
        .mobs
        .list
        .iter()
        .flatten()
        .filter(|o| o.id != m.id && o.alive() && o.touches(&area) && alertable_mob(o))
        .filter(|o| !trusts(m, o.uuid))
        .map(|o| o.id)
        .collect();
    cands.into_iter().any(|id| s.targeting_ok(m, Ent::Mob(id), true, 12.0, false, true))
}

/// Stalkable prey (chicken, rabbit).
pub fn stalkable(s: &Server, t: Ent) -> bool {
    matches!(t, Ent::Mob(id) if s.mob_ref(id).is_some_and(|o| matches!(o.kind, Kind::Chicken | Kind::Rabbit)))
}

/// Jump into snow (`snow` block at the cell): face planted.
pub fn in_snow(s: &Server, m: &Mob) -> bool {
    s.bname(m.block_pos()) == "minecraft:snow"
}

/// Selector of trusted targets: the entity has a last victim still alive.
pub fn has_last_hurt_mob(s: &Server, e: Ent) -> bool {
    let victim = match e {
        Ent::Player => s.mobs.player_hit.map(|h| h.0),
        Ent::Mob(id) => s.mob_ref(id).and_then(|o| o.last_hurt_mob),
    };
    victim.is_some_and(|v| s.ent_alive(v))
}

/// Berry bush of age ≥ 2, or cave vines with berries.
pub fn berry_target(s: &Server, p: [i32; 3]) -> bool {
    let n = s.bname(p);
    let props = || s.data.state_properties(s.st(p));
    match n {
        "minecraft:sweet_berry_bush" => {
            props().find(|(k, _)| *k == "age").and_then(|(_, v)| v.parse::<i32>().ok()).unwrap_or(0) >= 2
        }
        "minecraft:cave_vines" | "minecraft:cave_vines_plant" => props().any(|kv| kv == ("berries", "true")),
        _ => false,
    }
}

/// Picking (rule `mob_griefing`): bush (1 + U(2) + 1 if ripe, one in
/// the mouth if empty, the rest on the ground, back to age 1) or vine (harvest table).
pub fn pick_berries(s: &mut Server, m: &mut Mob, p: [i32; 3]) {
    if !s.rule("minecraft:mob_griefing", true) {
        return;
    }
    let st = s.st(p);
    let n = s.bname(p).to_owned();
    if n == "minecraft:sweet_berry_bush" {
        let age = s
            .data
            .state_properties(st)
            .find(|(k, _)| *k == "age")
            .and_then(|(_, v)| v.parse::<i32>().ok())
            .unwrap_or(0);
        let mut count = 1 + s.mobs.rng.next_int_bound(2) + i32::from(age == 3);
        if m.equipment[0].is_none() {
            m.equipment[0] = Some(Stack { id: "minecraft:sweet_berries".into(), count: 1, components: None });
            count -= 1;
        }
        if count > 0 {
            s.pop_resource(p, Stack { id: "minecraft:sweet_berries".into(), count, components: None });
        }
        s.mob_sound(m, "minecraft:block.sweet_berry_bush.pick_berries", 1.0, 1.0);
        if let Some(ns) = s.with_prop(st, "age", "1") {
            s.set_block(p, ns, 2);
        }
    } else if berry_target(s, p) {
        let drops = super::super::surface::loot(s, "minecraft:harvest/cave_vine", &crate::loot::Ctx::default());
        for d in drops {
            s.pop_resource(p, d);
        }
        let pitch = s.mobs.rng.next_float() * 0.4 + 0.8;
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        s.world_sound("minecraft:block.cave_vines.pick_berries", 4, at, 1.0, pitch);
        if let Some(ns) = s.with_prop(st, "berries", "false") {
            s.set_block(p, ns, 2);
        }
    }
}

/// Pickable items within ±8 (no delay, alive), in list order.
pub fn items_near(s: &Server, m: &Mob) -> Vec<[f64; 3]> {
    let b = m.bbox();
    let area = rc_physics::Aabb { min: b.min.map(|v| v - 8.0), max: b.max.map(|v| v + 8.0) };
    s.life
        .drops
        .iter()
        .filter(|d| {
            let ib = rc_physics::Aabb {
                min: [d.pos[0] - 0.125, d.pos[1], d.pos[2] - 0.125],
                max: [d.pos[0] + 0.125, d.pos[1] + 0.25, d.pos[2] + 0.125],
            };
            !d.is_removed() && d.pickup_delay <= 0 && ib.intersects(&area)
        })
        .map(|d| d.pos)
        .collect()
}

/// First trusted being present: its last attacker and the timestamp of the hit
/// (start condition of the trusted ally's defense goal); `None`: none present.
pub fn trusted_attacker(s: &Server, m: &Mob) -> Option<(Option<Ent>, i32)> {
    for u in m.wild.trusted.iter().flatten() {
        if *u == s.uuid {
            if s.ent_alive(Ent::Player) {
                return Some(s.mobs.player_hurt_by.map_or((None, 0), |(e, t)| (Some(e), t)));
            }
            continue;
        }
        if let Some(o) = s.mobs.get_uuid(*u) {
            return Some(o.last_hurt_by_mob.map_or((None, 0), |(e, t)| (Some(e), t)));
        }
    }
    None
}
