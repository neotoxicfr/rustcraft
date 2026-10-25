//! G5-F02b: mob steps without a sound of their own (block sound, amethyst chime), trampled
//! farmland, burning magma and redstone ore stepped on, NoGravity in the bounce and the squid,
//! team collision rules in the push, refit after growth; entity sound seeds independent of the
//! world's random source.

use rc_client::Event;
use rc_proto_v777::play::SoundRef;

use super::mob::{Ent, Mob};
use super::recent::vibration::Src;
use super::species::Kind;
use crate::blocks::probe::{Probe, state_string};

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
    if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("zone 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    Some((p, dir))
}

fn cleanup(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mob sound, an entity-attached sound or a world sound draws nothing from the mobs' world
/// random source (the official game has a generator of its own for sound seeds): spawns and
/// finalizations do not depend on the number of sounds played.
#[test]
fn sounds_draw_nothing_from_world_random() {
    let Some((mut p, dir)) = probe("g5f02b-seed") else { return };
    let m = p.s.new_mob(Kind::Zombie, [210.5, -59.0, 210.5], 0.0);
    let mut before = p.s.mobs.rng;
    p.s.mob_sound(&m, "minecraft:entity.zombie.ambient", 1.0, 1.0);
    p.s.entity_sound(m.id, "minecraft:entity.zombie.ambient", 5, 1.0, 1.0);
    p.s.world_sound("minecraft:entity.zombie.ambient", 5, m.pos, 1.0, 1.0);
    assert_eq!(p.s.mobs.rng.next_long(), before.next_long(), "world random advanced by a sound");
    assert!(!p.take_events().is_empty(), "no sound sent");
    cleanup(p, dir);
}

/// Fresh mob outside the list (no jockey or baby drawn), on the ground at `pos`.
fn mob(p: &mut Probe, k: Kind, pos: [f64; 3]) -> Mob {
    let mut m = p.s.new_mob(k, pos, 0.0);
    m.on_ground = true;
    m
}

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

/// Sounds emitted: (name, volume, pitch).
fn sounds(p: &Probe) -> Vec<(String, f32, f32)> {
    let reg = p.s.data.registry("minecraft:sound_event").expect("sound registry");
    p.take_events()
        .iter()
        .filter_map(|e| match e {
            Event::Sound(s) => Some((
                match &s.sound {
                    SoundRef::Id(i) => reg.entries.get(*i as usize).cloned().unwrap_or_default(),
                    SoundRef::Named(n) => n.clone(),
                },
                s.volume,
                s.pitch,
            )),
            _ => None,
        })
        .collect()
}

/// Walks `m` east until a step (accumulated > 1); returns the sounds emitted.
fn walk(p: &mut Probe, m: &mut Mob) -> Vec<(String, f32, f32)> {
    let _ = p.take_events();
    for _ in 0..6 {
        m.on_ground = true;
        m.vel = [0.4, -0.08, 0.0];
        p.s.mob_move(m, [0.4, -0.08, 0.0]);
    }
    sounds(p)
}

/// Creeper without a step sound of its own: the block's "step" sound (stone) at 0.15 × the
/// group's volume; zombie (own sound) on amethyst: own sound then chime; allay (empty sound):
/// chime only; squid (events only): nothing.
#[test]
fn step_block_sound_and_chime() {
    let Some((mut p, dir)) = probe("g5f02b-step") else { return };
    let mut c = mob(&mut p, Kind::Creeper, [210.5, -59.0, 210.5]);
    let s = walk(&mut p, &mut c);
    assert!(
        s.iter().any(|(n, v, pi)| n == "minecraft:block.stone.step" && (*v - 0.15).abs() < 1e-6 && *pi == 1.0),
        "no stone step sound: {s:?}"
    );
    p.run("fill 200 -60 214 231 -60 214 minecraft:amethyst_block");
    let mut z = mob(&mut p, Kind::Zombie, [205.5, -59.0, 214.5]);
    z.tick_count = 100;
    let s = walk(&mut p, &mut z);
    let names: Vec<&str> = s.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(
        names.iter().filter(|n| **n == "minecraft:block.amethyst_block.chime").count(),
        1,
        "one chime (no more than one per 20 ticks): {names:?}"
    );
    assert!(names.contains(&"minecraft:entity.zombie.step"), "zombie's own sound: {names:?}");
    let chime = s.iter().find(|x| x.0 == "minecraft:block.amethyst_block.chime").unwrap();
    // Intensity 0 → 0.07: volume 0.1 + 0.07 × 1.2.
    assert!((chime.1 - (0.1 + 0.07 * 1.2)).abs() < 1e-6, "chime volume {}", chime.1);
    let mut a = mob(&mut p, Kind::Allay, [215.5, -59.0, 214.5]);
    a.tick_count = 100;
    let s = walk(&mut p, &mut a);
    assert!(
        s.iter().all(|x| x.0 == "minecraft:block.amethyst_block.chime") && !s.is_empty(),
        "allay: chime only: {s:?}"
    );
    let mut q = mob(&mut p, Kind::Squid, [210.5, -59.0, 206.5]);
    let s = walk(&mut p, &mut q);
    assert!(s.is_empty(), "squid is silent: {s:?}");
    cleanup(p, dir);
}

/// Last game event `event` of the test log: (position, source).
fn last_event(p: &Probe, event: &str) -> Option<([f64; 3], Option<Src>)> {
    p.s.mobs.rec.vib.log.iter().rev().find(|e| e.0 == event).map(|e| (e.1, e.2))
}

/// 3-block fall onto farmland: a zombie turns it into dirt, is raised onto it like any other
/// entity (before its landing) and becomes the source of the "block changed" event; a chicken
/// (box < 0.512) does not; without `mob_griefing`, neither does the zombie; the player always
/// does (source: the player).
#[test]
fn farmland_trampled() {
    let Some((mut p, dir)) = probe("g5f02b-farmland") else { return };
    let top = -60.0 + 15.0 / 16.0;
    let fall = |p: &mut Probe, k: Kind, x: i32| -> (String, f64, i32) {
        p.run(&format!("setblock {x} -60 210 minecraft:farmland"));
        let mut m = mob(p, k, [f64::from(x) + 0.5, top + 0.05, 210.5]);
        m.on_ground = false;
        m.fall_distance = 3.0;
        m.vel = [0.0, -0.3, 0.0];
        p.s.mob_move(&mut m, [0.0, -0.3, 0.0]);
        assert!(m.on_ground, "{k:?} not landed");
        (state_string(&p.s, p.s.get([x, -60, 210])), m.pos[1], m.id)
    };
    let (st, y, id) = fall(&mut p, Kind::Zombie, 205);
    assert_eq!(st, "minecraft:dirt", "zombie");
    assert_eq!(y, -59.0, "zombie raised onto the dirt");
    assert_eq!(last_event(&p, "minecraft:block_change").map(|e| e.1), Some(Some(Src::Living(Ent::Mob(id)))));
    assert_eq!(last_event(&p, "minecraft:hit_ground").map(|e| e.0[1]), Some(-59.0), "landing after the raise");
    let (st, y, _) = fall(&mut p, Kind::Chicken, 207);
    assert!(st.starts_with("minecraft:farmland") && y == top, "chicken: {st} at {y}");
    cmd(&mut p, "gamerule mob_griefing false");
    assert!(fall(&mut p, Kind::Zombie, 209).0.starts_with("minecraft:farmland"), "without mob_griefing");
    // Player: landing announced by its movement.
    p.run("setblock 216 -60 210 minecraft:farmland");
    p.s.pos = [216.5, top, 210.5];
    p.s.on_ground = true;
    p.s.life.vit.fall = 2.9;
    p.s.after_move([216.5, top + 0.1, 210.5], false);
    assert_eq!(state_string(&p.s, p.s.get([216, -60, 210])), "minecraft:dirt", "player");
    assert_eq!(last_event(&p, "minecraft:block_change").map(|e| e.1), Some(Some(Src::Living(Ent::Player))));
    cleanup(p, dir);
}

/// Block touched by the player = effect block of its support: landed on the edge of a full block
/// (top at −59), the center over a neighboring field, it tramples nothing; held by a field, the
/// center over a hole, it tramples it.
#[test]
fn player_block_touched_by_support() {
    let Some((mut p, dir)) = probe("g5f02b-support") else { return };
    let land = |p: &mut Probe, pos: [f64; 3]| {
        p.s.pos = pos;
        p.s.on_ground = true;
        p.s.life.vit.fall = 2.9;
        p.s.after_move([pos[0], pos[1] + 0.1, pos[2]], false);
    };
    // Full block (stone) at x = 216, field at x = 217; center over the field.
    p.run("setblock 217 -60 210 minecraft:farmland");
    land(&mut p, [217.05, -59.0, 210.5]);
    assert!(state_string(&p.s, p.s.get([217, -60, 210])).starts_with("minecraft:farmland"), "held by the stone");
    // Field at x = 217 as the support, hole at x = 218 under the center.
    p.run("setblock 217 -60 212 minecraft:farmland\nsetblock 218 -60 212 minecraft:air");
    land(&mut p, [217.95, -60.0 + 15.0 / 16.0, 212.5]);
    assert_eq!(state_string(&p.s, p.s.get([217, -60, 212])), "minecraft:dirt", "held by the field");
    cleanup(p, dir);
}

/// Flyers whose fall check is empty in the game: a happy ghast landed on a field (fall read from
/// a save) does not trample it, does not reset its fall to zero and emits no landing; a descent
/// in the air accumulates nothing.
#[test]
fn flyers_without_fall_check() {
    let Some((mut p, dir)) = probe("g5f02b-flyers") else { return };
    p.run("fill 202 -60 207 208 -60 213 minecraft:farmland");
    let top = -60.0 + 15.0 / 16.0;
    let mut g = mob(&mut p, Kind::HappyGhast, [205.5, top + 1.0, 210.5]);
    g.on_ground = false;
    g.vel = [0.0, -0.3, 0.0];
    p.s.mob_move(&mut g, [0.0, -0.3, 0.0]);
    assert!(!g.on_ground && g.fall_distance == 0.0, "fall accumulated: {}", g.fall_distance);
    g.fall_distance = 3.0;
    let before = p.s.mobs.rec.vib.log.len();
    for _ in 0..4 {
        g.vel = [0.0, -0.3, 0.0];
        p.s.mob_move(&mut g, [0.0, -0.3, 0.0]);
    }
    assert!(g.on_ground, "happy ghast not landed");
    assert_eq!(g.fall_distance, 3.0, "fall reset to zero");
    assert!(state_string(&p.s, p.s.get([205, -60, 210])).starts_with("minecraft:farmland"), "field trampled");
    assert!(p.s.mobs.rec.vib.log[before..].iter().all(|e| e.0 != "minecraft:hit_ground"), "landing emitted");
    cleanup(p, dir);
}

/// Magma stepped on: 1 damage to the zombie, nothing to the blaze (fire-immune); player hurt
/// unless sneaking; redstone ore lit by the step.
#[test]
fn magma_and_ore_stepped_on() {
    let Some((mut p, dir)) = probe("g5f02b-magma") else { return };
    p.run("setblock 210 -60 210 minecraft:magma_block\nsetblock 216 -60 202 minecraft:magma_block");
    let mut z = mob(&mut p, Kind::Zombie, [210.5, -59.0, 210.5]);
    let h = z.health;
    p.s.mob_step_on(&mut z);
    // 1 damage minus the zombie's natural armor.
    assert!(z.health < h && z.health > h - 1.0 - 1e-6, "zombie on magma: {} → {}", h, z.health);
    // Effect cell reused from the movement: same cell as the full computation, magma included.
    let mut z3 = mob(&mut p, Kind::Zombie, [210.5, -58.99, 210.5]);
    z3.vel = [0.0, -0.05, 0.0];
    p.s.mob_move(&mut z3, [0.0, -0.05, 0.0]);
    assert_eq!(z3.body_fx.effect.map(|e| e.3), Some(p.s.mob_below(&z3, 0.2)));
    let h3 = z3.health;
    p.s.mob_step_on(&mut z3);
    assert!(z3.health < h3, "zombie landed on magma by a movement");
    let mut b = mob(&mut p, Kind::Blaze, [210.5, -59.0, 210.5]);
    let hb = b.health;
    p.s.mob_step_on(&mut b);
    assert_eq!(b.health, hb, "blaze is immune");
    p.s.on_ground = true;
    p.s.sneaking = true;
    p.s.player_step_on();
    assert_eq!(p.s.life.vit.health, 20.0, "sneaking: no damage");
    p.s.sneaking = false;
    p.s.player_step_on();
    assert_eq!(p.s.life.vit.health, 19.0, "player on magma");
    p.run("setblock 212 -60 210 minecraft:redstone_ore");
    let mut z2 = mob(&mut p, Kind::Zombie, [212.5, -59.0, 210.5]);
    p.s.mob_step_on(&mut z2);
    let ore = state_string(&p.s, p.s.get([212, -60, 210]));
    assert!(ore.contains("lit=true"), "ore not lit: {ore}");
    cleanup(p, dir);
}

/// Effect cell reused only in the tick of its computation: landed on a carpet spread over magma,
/// a mob burns (cell under the carpet); motionless afterwards (no AI), the carpet turned into a
/// wall, the cell recomputed with the support re-read is the wall itself, like the game: nothing
/// more.
#[test]
fn effect_cell_this_tick_only() {
    let Some((mut p, dir)) = probe("g5f02b-carpet") else { return };
    p.run("setblock 210 -60 210 minecraft:magma_block\nsetblock 210 -59 210 minecraft:white_carpet");
    let mut z = mob(&mut p, Kind::Zombie, [210.5, -59.0 + 1.0 / 16.0 + 0.01, 210.5]);
    z.on_ground = false;
    z.vel = [0.0, -0.05, 0.0];
    p.s.mob_move(&mut z, [0.0, -0.05, 0.0]);
    assert!(z.on_ground && z.support == Some([210, -59, 210]), "not landed on the carpet: {:?}", z.support);
    let h = z.health;
    p.s.mob_step_on(&mut z);
    assert!(z.health < h, "the magma under the carpet burns");
    p.run("setblock 210 -59 210 minecraft:cobblestone_wall");
    // One second later, without movement or remaining invulnerability.
    z.tick_count += 20;
    z.damage_cooldown = 0;
    let h = z.health;
    p.s.mob_step_on(&mut z);
    assert_eq!(z.health, h, "cell of another tick reused: burned through the wall");
    cleanup(p, dir);
}

/// Sound seeds in tests: fixed per-thread sequence, two fresh threads draw the same sequence.
#[test]
fn sound_seeds_reproducible() {
    let run = || std::thread::spawn(|| (0..4).map(|_| crate::vitals::sound_seed()).collect::<Vec<i64>>()).join().unwrap();
    let (a, b) = (run(), run());
    assert_eq!(a, b, "seeds differ from one thread to another");
    assert!(a.windows(2).all(|w| w[0] != w[1]), "constant sequence: {a:?}");
}

/// Egg of zero size: under a ceiling, the chick is refitted just below it; inside solid stone,
/// no chick and the series of four stops at the first (a single identifier taken).
#[test]
fn egg_chicks_refitted() {
    let Some((mut p, dir)) = probe("g5f02b-egg") else { return };
    let egg = |pos: [f64; 3]| {
        let item = crate::player::Stack { id: "minecraft:egg".into(), count: 1, components: None };
        crate::projectiles::Proj::new(crate::projectiles::PKind::Egg, 900_000, pos, item)
    };
    let chickens = |p: &Probe| {
        p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Chicken).map(|m| (m.pos, m.dims_wh().1)).collect::<Vec<_>>()
    };
    p.run("setblock 210 -55 210 minecraft:stone");
    p.s.egg_chickens(&egg([210.5, -55.0, 210.5]), 1);
    let c = chickens(&p);
    assert_eq!(c.len(), 1, "one chick");
    let (pos, h) = c[0];
    // Top of the box against the underside of the ceiling (y = −55).
    assert!((pos[1] + f64::from(h) + 55.0).abs() < 1.0e-6, "chick in the ceiling: y = {} (height {h})", pos[1]);
    p.run("fill 211 -58 211 213 -56 213 minecraft:stone");
    let id0 = p.s.next_entity;
    p.s.egg_chickens(&egg([212.5, -57.0, 212.5]), 4);
    assert_eq!(chickens(&p).len(), 1, "chick born inside the stone");
    assert_eq!(p.s.next_entity, id0 + 1, "the series continues after a failure");
    cleanup(p, dir);
}

/// Tadpole stuck to a wall that becomes a frog (wider): refitted to the free edge, feet on the
/// ground.
#[test]
fn frog_refitted_from_tadpole() {
    let Some((mut p, dir)) = probe("g5f02b-tadpole") else { return };
    p.run("fill 211 -59 205 211 -55 215 minecraft:stone");
    let mut t = mob(&mut p, Kind::Tadpole, [210.5, -59.0, 210.5]);
    let (tw, _) = t.dims_wh();
    t.pos[0] = 211.0 - f64::from(tw) / 2.0 - 0.001;
    super::brainy::frog::grow_up(&mut p.s, &mut t);
    let f = p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::Frog).expect("frog");
    let (fw, _) = f.dims_wh();
    assert!((f.pos[0] - (211.0 - f64::from(fw) / 2.0)).abs() < 1.0e-6, "x = {}", f.pos[0]);
    assert!((f.pos[1] + 59.0).abs() < 1.0e-5, "y = {}", f.pos[1]);
    cleanup(p, dir);
}

/// Dolphin jump: direction of the gaze (horizontal), never that of the velocity. In a 1 × 1
/// water column, a dolphin rising never jumps; in a nine-block channel to the east, looking
/// east, with no velocity, it jumps east (+0.6; +0.7).
#[test]
fn dolphin_jump_follows_gaze() {
    use super::sealife::goals::AGoal;
    let Some((mut p, dir)) = probe("g5f02b-dolphin") else { return };
    p.run("setblock 205 -60 205 minecraft:water\nfill 210 -60 214 218 -60 214 minecraft:water");
    let mut d = mob(&mut p, Kind::Dolphin, [205.5, -59.9, 205.5]);
    d.on_ground = false;
    d.yaw = -90.0; // east
    d.vel = [0.0, 0.2, 0.0];
    let mut g = AGoal::Jump { interval: 1, breached: false };
    assert!(!p.s.dolphin_can_use(&mut g, &mut d), "jump in place in the column");
    let mut d = mob(&mut p, Kind::Dolphin, [210.5, -59.9, 214.5]);
    d.on_ground = false;
    d.yaw = -90.0;
    d.vel = [0.0; 3];
    assert!(p.s.dolphin_can_use(&mut g, &mut d), "no jump in the channel");
    p.s.dolphin_start(&mut g, &mut d);
    assert_eq!(d.vel, [0.6, 0.7, 0.0]);
    cleanup(p, dir);
}

/// Collision rule index = linear search of the teams (`team_of`): members with a canonical UUID
/// or not (uppercase: never an entity), player, member registered in two teams (the first
/// wins); rebuilt after every team change.
#[test]
fn collision_rule_index() {
    use crate::commands::scores::{Board, Team, collision_code};
    use crate::nbtio::uuid_text;
    use rc_proto::nbt::Nbt;
    let team = |name: &str, rule: &str, players: &[&str]| Team {
        name: name.into(),
        display: Nbt::String(name.into()),
        color: None,
        friendly_fire: true,
        see_invisibles: true,
        prefix: Nbt::String(String::new()),
        suffix: Nbt::String(String::new()),
        nametag: "always".into(),
        death: "always".into(),
        collision: rule.into(),
        players: players.iter().map(|s| (*s).to_owned()).collect(),
    };
    let (u1, u2) = (0x1234_5678_9abc_4def_8123_4567_89ab_cdefu128, 0x0fed_cba9_8765_4321_8000_0000_0000_0001u128);
    let (s1, s2) = (uuid_text(u1), uuid_text(u2));
    let upper = s2.to_uppercase();
    let mut b = Board::default();
    b.add_team(team("a", "always", &["Steve", &s1]));
    assert!(!b.collision_index().active, "no rule: filter inactive");
    b.add_team(team("b", "never", &[&s2, &upper, &s1]));
    let ix = b.collision_index();
    assert!(ix.active);
    for h in ["Steve", s1.as_str(), s2.as_str(), upper.as_str(), "Alex"] {
        let linear = b.team_of(h).map(|t| {
            (b.teams.iter().position(|x| x.name == t.name).unwrap() as u32, collision_code(&t.collision))
        });
        let canonical = crate::commands::snbt::parse_uuid(h).filter(|u| uuid_text(*u) == h);
        let got = match canonical {
            Some(u) => ix.by_uuid.get(&u).copied(),
            None => ix.by_name.get(h).copied(),
        };
        assert_eq!(got, linear, "member {h}");
    }
    b.leave_team(&s2);
    assert_eq!(b.collision_index().by_uuid.get(&u2), None, "stale index after a leave");
    b.teams[1].collision = "always".into();
    b.touch_teams();
    assert!(!b.collision_index().active, "stale index after a rule change");
}

/// NoGravity: a slow fall (0.05 < gravity) onto a slime block still bounces (effective gravity
/// zero); a squid out of water does not fall.
#[test]
fn no_gravity_bounce_and_squid() {
    let Some((mut p, dir)) = probe("g5f02b-nograv") else { return };
    p.run("setblock 210 -60 210 minecraft:slime_block");
    let bounce = |p: &mut Probe, no_gravity: bool| {
        let mut m = mob(p, Kind::Zombie, [210.5, -59.0 + 0.01, 210.5]);
        m.surf.no_gravity = no_gravity;
        m.on_ground = false;
        m.vel = [0.0, -0.05, 0.0];
        p.s.mob_move(&mut m, [0.0, -0.05, 0.0]);
        m.vel[1]
    };
    assert_eq!(bounce(&mut p, false), 0.0, "with gravity: no bounce below the threshold");
    assert!(bounce(&mut p, true) > 0.0, "without gravity: bounce");
    let mut q = mob(&mut p, Kind::Squid, [216.5, -55.0, 216.5]);
    q.on_ground = false;
    q.surf.no_gravity = true;
    q.vel = [0.0, 0.0, 0.0];
    super::sealife::squid::post_ai_step(&mut p.s, &mut q);
    assert_eq!(q.vel[1], 0.0, "squid without gravity");
    cleanup(p, dir);
}

/// Push: without a team, two overlapping zombies push each other; team "never": nothing more;
/// "pushOwnTeam": two members do not push each other (quirk of the game), a non-member does.
#[test]
fn push_team_rule() {
    let Some((mut p, dir)) = probe("g5f02b-team") else { return };
    let a = mob(&mut p, Kind::Zombie, [210.5, -59.0, 210.5]);
    let (ua, aid) = (crate::nbtio::uuid_text(a.uuid), a.id);
    p.s.add_mob(a);
    let push = |p: &mut Probe| {
        let i = p.s.mob_index(aid).unwrap();
        p.s.mobs.list[i].as_mut().unwrap().vel = [0.0; 3];
        let mut b = mob(p, Kind::Zombie, [210.8, -59.0, 210.5]);
        b.vel = [0.0; 3];
        (b.uuid, b)
    };
    let pushed = |p: &mut Probe, mut b: Mob| {
        let i = p.s.mob_index(aid).unwrap();
        p.s.mobs.list[i].as_mut().unwrap().vel = [0.0; 3];
        p.s.mob_push_entities(&mut b);
        p.s.mob_ref(aid).unwrap().vel[0] != 0.0
    };
    let (_, b) = push(&mut p);
    assert!(pushed(&mut p, b), "no team: pushed");
    cmd(&mut p, "team add t");
    cmd(&mut p, &format!("team join t {ua}"));
    cmd(&mut p, "team modify t collisionRule never");
    let (_, b) = push(&mut p);
    assert!(!pushed(&mut p, b), "rule never: no push");
    cmd(&mut p, "team modify t collisionRule pushOwnTeam");
    let (ub, b) = push(&mut p);
    assert!(pushed(&mut p, b.clone()), "pushOwnTeam, other team (none): pushed");
    // The second zombie is not in the world: registered through the scoreboard.
    assert!(p.s.cmd.scores.join_team(&crate::nbtio::uuid_text(ub), "t"));
    assert!(!pushed(&mut p, b), "pushOwnTeam, same team: not pushed");
    cleanup(p, dir);
}

/// Baby zombie stuck to a wall that becomes an adult: recentered on the free edge, feet on the
/// ground; at the first size reading, nothing moves.
#[test]
fn refit_after_growth() {
    let Some((mut p, dir)) = probe("g5f02b-size") else { return };
    p.run("fill 211 -59 205 211 -55 215 minecraft:stone");
    let mut z = mob(&mut p, Kind::Zombie, [210.5, -59.0, 210.5]);
    z.baby = true;
    let (bw, bh) = z.dims_wh();
    z.pos[0] = 211.0 - f64::from(bw) / 2.0 - 0.01;
    let x0 = z.pos[0];
    p.s.mob_refit(&mut z);
    assert_eq!(z.pos[0], x0, "first reading: no refit");
    assert_eq!(z.body_fx.dims, Some((bw, bh)));
    z.baby = false;
    let (w, _) = z.dims_wh();
    assert!(w > bw);
    p.s.mob_refit(&mut z);
    assert!((z.pos[0] - (211.0 - f64::from(w) / 2.0)).abs() < 1e-6, "x = {}", z.pos[0]);
    assert!((z.pos[1] + 59.0).abs() < 1e-5, "y = {}", z.pos[1]);
    // Shrinking moves nothing.
    let before = z.pos;
    z.baby = true;
    p.s.mob_refit(&mut z);
    assert_eq!(z.pos, before);
    cleanup(p, dir);
}

/// Bench (loaded machine, to re-measure when idle): cost per mob and per tick of the additions to
/// the common tick — block stepped on (`mob_step_on`, mob on the ground on stone), size reading
/// (`mob_refit`, size unchanged) and the push team filter with no team (`push_team`
/// short-circuited by the empty list).
#[test]
#[ignore]
fn bench_tick_additions() {
    let Some((mut p, dir)) = probe("g5f02b-bench") else { return };
    let mut z = mob(&mut p, Kind::Zombie, [210.5, -59.0, 210.5]);
    // Landed by a movement: support read, effect cell recorded.
    z.vel = [0.0, -0.01, 0.0];
    p.s.mob_move(&mut z, [0.0, -0.01, 0.0]);
    assert!(z.on_ground && z.support.is_some());
    let n = 2_000_000u32;
    let t = std::time::Instant::now();
    for _ in 0..n {
        p.s.mob_step_on(std::hint::black_box(&mut z));
    }
    let step = t.elapsed().as_nanos() as f64 / f64::from(n);
    p.s.mob_refit(&mut z);
    let t = std::time::Instant::now();
    for _ in 0..n {
        p.s.mob_refit(std::hint::black_box(&mut z));
    }
    let refit = t.elapsed().as_nanos() as f64 / f64::from(n);
    let t = std::time::Instant::now();
    for _ in 0..n {
        p.s.mob_push_entities(std::hint::black_box(&mut z));
    }
    let push = t.elapsed().as_nanos() as f64 / f64::from(n);
    eprintln!("step-on {step:.1} ns, size reading {refit:.1} ns, push (empty list) {push:.1} ns");
    cleanup(p, dir);
}

/// Bench (loaded machine, to re-measure when idle): push of a mob among 20 neighbors outside any
/// team, with a "never" team of 300 members (mini-game map): team filter active for every
/// neighbor. `RUSTCRAFT_OPT_TEAMIDX=0` for the old walk.
#[test]
#[ignore]
fn bench_team_push() {
    let Some((mut p, dir)) = probe("g5f02b-bench-team") else { return };
    cmd(&mut p, "team add t");
    for i in 0..300u128 {
        p.s.cmd.scores.join_team(&crate::nbtio::uuid_text(0xabcd_0000_0000_4000_8000_0000_0000_0000 | i), "t");
    }
    cmd(&mut p, "team modify t collisionRule never");
    for i in 0..20 {
        let m = mob(&mut p, Kind::Zombie, [210.3 + f64::from(i % 5) * 0.1, -59.0, 210.3 + f64::from(i / 5) * 0.1]);
        p.s.add_mob(m);
    }
    let mut z = mob(&mut p, Kind::Zombie, [210.5, -59.0, 210.5]);
    let n = 200_000u32;
    let t = std::time::Instant::now();
    for _ in 0..n {
        p.s.mob_push_entities(std::hint::black_box(&mut z));
    }
    let push = t.elapsed().as_nanos() as f64 / f64::from(n);
    eprintln!("push, 20 neighbors, never team of 300 members: {push:.1} ns");
    cleanup(p, dir);
}
