//! Scenarios of the village, golem and raid searches on a copy of the test world
//! (`RUSTCRAFT_SOLO_WORLD`, zone 200..231), recorded tick by tick: every mob's position, yaw,
//! health, target and random state, the world random, the player, the raids and the fangs. Each
//! run has its own thread, so the test seeds (`rng::test_seed`) start from the same value.
//!
//! The traces of the code before the search optimisations are kept as fingerprints: the current
//! code must reproduce them (Windows only: the system sine and cosine differ elsewhere).

use super::super::mob::Ent;
use super::super::species::Kind;
use crate::blocks::probe::Probe;
use crate::server::Server;

/// Stone floor at −60, air above, player at (216.5, −59, 202.5), survival, normal difficulty.
pub(super) fn open(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = super::super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(
        "fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -50 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n",
    );
    if crate::blocks::probe::state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("zone 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.view = 8;
    Some((p, dir))
}

fn step(p: &mut Probe) {
    p.tick();
    p.s.effects_tick();
    p.s.mobs_world_tick();
    p.s.projectiles_tick();
}

fn spawn(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(k, pos, 0.0);
    let mut g = super::super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::super::spawn::Reason::SpawnEgg, &mut g);
    m.persistent = true;
    let id = m.id;
    p.s.add_mob(m);
    id
}

/// What a client and a server would see of the world at the end of a tick.
fn snapshot(s: &Server) -> String {
    let mut out = format!(
        "t {} player {:?} health {} world random {:?}\n",
        s.time,
        s.pos.map(f64::to_bits),
        s.life.vit.health.to_bits(),
        s.mobs.rng
    );
    for m in s.mobs.list.iter() {
        out += &format!(
            "{} {:?} at {:?} yaw {} health {} target {:?} random {:?} removed {}\n",
            m.id,
            m.kind,
            m.pos().map(f64::to_bits),
            m.yaw.to_bits(),
            m.health.to_bits(),
            m.target,
            m.rng,
            m.removed
        );
    }
    for r in &s.mobs.ill.raids.list {
        out += &format!("{r:?}\n");
    }
    for f in &s.mobs.ill.fangs {
        out += &format!("fangs {} at {:?} warmup {} life {}\n", f.id, f.pos.map(f64::to_bits), f.warmup, f.life);
    }
    out
}

/// Block states of the zone's walking layer (snow trails, broken crops, doors).
pub(super) fn walking_layer(s: &Server) -> String {
    let states: Vec<u32> = (200..=231).flat_map(|z| (200..=231).map(move |x| s.get([x, -59, z]))).collect();
    format!("walking layer {states:?}")
}

/// 64-bit FNV-1a: a fingerprint that does not depend on the toolchain.
pub(super) fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// A change made to the world at a given tick of a scenario.
type ChangeAt = (i32, fn(&mut Probe));

/// A scenario: its blocks and mobs, the number of ticks to record, a change made at a given
/// tick, and the new search paths it must go through (named as `poi::note_search_path` notes
/// them).
pub(super) struct Scenario {
    pub name: &'static str,
    pub setup: fn(&mut Probe),
    pub ticks: i32,
    pub change: Option<ChangeAt>,
    pub exercises: &'static [&'static str],
}

/// Runs a scenario in its own thread (`before` first, in that thread); one snapshot per tick,
/// then what `at_end` reads of the server. `None`: test world absent.
pub(super) fn record(sc: &Scenario, variant: &str, before: fn(), at_end: fn(&Server) -> String) -> Option<Vec<String>> {
    let tag = format!("{}-{variant}", sc.name);
    let (setup, ticks, change) = (sc.setup, sc.ticks, sc.change);
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            before();
            let (mut p, _dir) = open(&tag)?;
            setup(&mut p);
            let mut trace = Vec::with_capacity(ticks as usize + 1);
            for t in 0..ticks {
                if let Some((at, f)) = change
                    && at == t
                {
                    f(&mut p);
                }
                step(&mut p);
                trace.push(snapshot(&p.s));
            }
            trace.push(at_end(&p.s));
            Some(trace)
        })
        .expect("scenario thread")
        .join()
        .expect("scenario")
}

/// Village with occupied bell, job sites and beds in the corner (200..207, 200..207); villagers in
/// it; two iron golems two sections away (moving back, strolling toward the village); a zombie at
/// night (village walk).
fn village_with_golems(p: &mut Probe) {
    p.run(
        "setblock 202 -59 202 minecraft:bell\nsetblock 204 -59 201 minecraft:composter\nsetblock 206 -59 201 minecraft:lectern\nsetblock 201 -59 205 minecraft:red_bed[part=foot,facing=south]\nsetblock 201 -59 206 minecraft:red_bed[part=head,facing=south]\nsetblock 204 -59 205 minecraft:blue_bed[part=foot,facing=south]\nsetblock 204 -59 206 minecraft:blue_bed[part=head,facing=south]\n",
    );
    step(p);
    for at in [[202, -59, 202], [204, -59, 201], [201, -59, 206]] {
        p.s.poi_acquire(at);
    }
    spawn(p, Kind::Villager, [203.5, -59.0, 203.5]);
    spawn(p, Kind::Villager, [205.5, -59.0, 204.5]);
    spawn(p, Kind::IronGolem, [229.5, -59.0, 229.5]);
    spawn(p, Kind::IronGolem, [228.5, -59.0, 214.5]);
    // Night (world time and Overworld clock).
    p.s.time = 18_000;
    p.s.clocks.retain(|c| c.0 != "minecraft:overworld");
    p.s.clocks.push(("minecraft:overworld".into(), 18_000));
    spawn(p, Kind::Zombie, [224.5, -59.0, 224.5]);
}

/// Raid: Bad Omen in a village around the player; the waves come, walk to the centre and
/// through the houses.
fn raid_at_village(p: &mut Probe) {
    p.run(
        "setblock 214 -59 205 minecraft:bell\nsetblock 218 -59 205 minecraft:composter\nsetblock 210 -59 208 minecraft:red_bed[part=foot,facing=south]\nsetblock 210 -59 209 minecraft:red_bed[part=head,facing=south]\nsetblock 222 -59 208 minecraft:blue_bed[part=foot,facing=south]\nsetblock 222 -59 209 minecraft:blue_bed[part=head,facing=south]\n",
    );
    step(p);
    for at in [[214, -59, 205], [218, -59, 205], [210, -59, 209], [222, -59, 209]] {
        p.s.poi_acquire(at);
    }
    p.s.give_player_effect("minecraft:bad_omen", 6000, 0);
}

/// During the raid the houses move two sections east: the centre is no longer in the village and
/// moves to the closest village section.
fn village_moves_east(p: &mut Probe) {
    p.run(
        "setblock 214 -59 205 minecraft:air\nsetblock 218 -59 205 minecraft:air\nsetblock 210 -59 208 minecraft:air\nsetblock 210 -59 209 minecraft:air\nsetblock 222 -59 208 minecraft:air\nsetblock 222 -59 209 minecraft:air\nsetblock 250 -59 207 minecraft:bell\n",
    );
    step(p);
    p.s.poi_acquire([250, -59, 207]);
}

/// Illagers and golems around the player: evoker (fangs, vexes), witch, ravager, a patrol of
/// three pillagers, a snow golem, a blue sheep for the evoker's spell.
fn illagers_and_golems(p: &mut Probe) {
    spawn(p, Kind::Evoker, [216.5, -59.0, 214.5]);
    spawn(p, Kind::Witch, [210.5, -59.0, 210.5]);
    spawn(p, Kind::Ravager, [224.5, -59.0, 222.5]);
    spawn(p, Kind::SnowGolem, [206.5, -59.0, 226.5]);
    let sheep = spawn(p, Kind::Sheep, [212.5, -59.0, 224.5]);
    if let Some(m) = p.s.mobs.get_mut(sheep) {
        m.wool = (m.wool & !15) | 11;
    }
    let leader = spawn(p, Kind::Pillager, [228.5, -59.0, 204.5]);
    for (i, at) in [[229.5, -59.0, 206.5], [227.5, -59.0, 207.5]].into_iter().enumerate() {
        let id = spawn(p, Kind::Pillager, at);
        if let Some(m) = p.s.mobs.get_mut(id) {
            m.ill.patrolling = true;
            m.ill.patrol_target = Some([240 + i as i32, -59, 180]);
        }
    }
    if let Some(m) = p.s.mobs.get_mut(leader) {
        m.ill.patrol_leader = true;
        m.ill.patrolling = true;
        m.ill.patrol_target = Some([260, -59, 170]);
        m.equipment[5] = Some(super::ominous_banner());
    }
    // The witch throws at the player from the start; the ravager's roar hurts her (a last hit
    // for her fire check to read).
    if let Some(m) = p.s.mobs.list.iter_mut().find(|m| m.kind == Kind::Witch) {
        m.target = Some(Ent::Player);
    }
}

pub(super) const SCENARIOS: [Scenario; 3] = [
    Scenario {
        name: "village-golems",
        setup: village_with_golems,
        ticks: 2000,
        change: None,
        exercises: &[
            "closest village section batch",
            "range search in kept order",
            "bounded village test by columns",
            "indexed square skipped",
            "village batch",
        ],
    },
    Scenario {
        name: "village-raid",
        setup: raid_at_village,
        ticks: 1500,
        change: Some((1100, village_moves_east)),
        exercises: &["raid centre test reused", "raid centre move batch", "range search in kept order"],
    },
    Scenario {
        name: "illagers-golems",
        setup: illagers_and_golems,
        ticks: 800,
        change: None,
        exercises: &["fangs without caster copy", "snow state from the block table", "witch fire check where used"],
    },
];

/// Fingerprints of the scenarios' traces recorded with the code before the search optimisations
/// (commit 2e768c2f, this file added alone).
const TRACES_BEFORE: [u64; 3] = [0x9a9b_c327_269e_10ed, 0x51bd_1f9b_650f_d34d, 0x9234_c4d3_841b_8866];

/// The current code reproduces, tick by tick, what the code before the search optimisations did.
#[test]
fn scenarios_replay_the_traces_of_the_code_before() {
    let mut computed = Vec::new();
    for sc in &SCENARIOS {
        let Some(trace) = record(sc, "replay", || {}, walking_layer) else { return };
        let fp = fnv1a(trace.concat().as_bytes());
        eprintln!("scenario {}: {} ticks, fingerprint {fp:#018x}", sc.name, trace.len());
        computed.push(fp);
    }
    if cfg!(windows) {
        assert_eq!(computed, TRACES_BEFORE, "traces of the code before");
    }
}
