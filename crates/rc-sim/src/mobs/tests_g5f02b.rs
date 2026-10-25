//! G5-F02b: mob steps without a sound of their own (block sound, amethyst chime), trampled
//! farmland, burning magma and redstone ore stepped on, NoGravity in the bounce and the squid,
//! team collision rules in the push, refit after growth; entity sound seeds independent of the
//! world's random source.

use rc_client::Event;
use rc_proto_v777::play::SoundRef;

use super::mob::Mob;
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

/// 3-block fall onto farmland: a zombie turns it into dirt; a chicken (box
/// < 0.512) does not; without `mob_griefing`, neither does the zombie; the player always does.
#[test]
fn farmland_trampled() {
    let Some((mut p, dir)) = probe("g5f02b-farmland") else { return };
    let top = -60.0 + 15.0 / 16.0;
    let fall = |p: &mut Probe, k: Kind, x: i32| -> String {
        p.run(&format!("setblock {x} -60 210 minecraft:farmland"));
        let mut m = mob(p, k, [f64::from(x) + 0.5, top + 0.05, 210.5]);
        m.on_ground = false;
        m.fall_distance = 3.0;
        m.vel = [0.0, -0.3, 0.0];
        p.s.mob_move(&mut m, [0.0, -0.3, 0.0]);
        assert!(m.on_ground, "{k:?} not landed");
        state_string(&p.s, p.s.get([x, -60, 210]))
    };
    assert_eq!(fall(&mut p, Kind::Zombie, 205), "minecraft:dirt", "zombie");
    assert!(fall(&mut p, Kind::Chicken, 207).starts_with("minecraft:farmland"), "chicken");
    cmd(&mut p, "gamerule mob_griefing false");
    assert!(fall(&mut p, Kind::Zombie, 209).starts_with("minecraft:farmland"), "without mob_griefing");
    // Player: landing announced by its movement.
    p.run("setblock 216 -60 210 minecraft:farmland");
    p.s.pos = [216.5, top, 210.5];
    p.s.on_ground = true;
    p.s.life.vit.fall = 2.9;
    p.s.after_move([216.5, top + 0.1, 210.5], false);
    assert_eq!(state_string(&p.s, p.s.get([216, -60, 210])), "minecraft:dirt", "player");
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
    assert_eq!(z3.body_fx.effect.map(|e| e.2), Some(p.s.mob_below(&z3, 0.2)));
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
    // The second zombie is not in the world: added directly to the team.
    p.s.cmd.scores.teams.iter_mut().find(|t| t.name == "t").unwrap().players.push(crate::nbtio::uuid_text(ub));
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
