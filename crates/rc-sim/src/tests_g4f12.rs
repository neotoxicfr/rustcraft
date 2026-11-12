//! G4-F12: iron and copper golems. Village defence and offered flower with line of sight,
//! spawning (world border, bubble column counted as a liquid), copper golem opening the doors
//! on its path. Fresh flat world: no external data other than the 26.3 cache and the client
//! jar, whose absence makes the test FAIL.

use std::path::PathBuf;
use std::sync::Arc;

use crate::blocks::probe::{Probe, state_string};
use crate::mobs::goals::TBase;
use crate::mobs::golems::GolemGoal;
use crate::mobs::mob::{Ent, Mob};
use crate::mobs::species::Kind;

struct W {
    p: Probe,
    dir: PathBuf,
}

impl Drop for W {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// First layer of air of the flat world (grass at y = −61).
const Y: i32 = -60;

fn world(tag: &str) -> W {
    let data = rc_testdata::version_data().expect("G4-F12: 26.3 data cache missing");
    let data = Arc::new(data);
    assert!(crate::registries::client_jar(&data).is_some(), "G4-F12: 26.3 client jar missing");
    let dir = std::env::temp_dir().join(format!("rc-g4f12-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 7;
    w.preset = crate::newworld::Preset::Flat;
    w.allow_commands = true;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    let want: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    p.s.ensure_loaded(&want);
    let t = std::time::Instant::now();
    while !want.iter().all(|c| p.s.chunks.get(c).is_some_and(Option::is_some)) {
        assert!(t.elapsed().as_secs() < 300, "G4-F12: anti-hang guard: flat world generation stuck for 300 s");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    p.run(&format!("fill -8 {Y} -8 23 {} 23 minecraft:air\n", Y + 4));
    p.s.pos = [8.5, f64::from(Y), 8.5];
    p.s.on_ground = true;
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    let _ = p.take_events();
    W { p, dir }
}

impl W {
    fn add(&mut self, k: Kind, pos: [f64; 3]) -> i32 {
        let m = self.p.s.new_mob(k, pos, 0.0);
        let id = m.id;
        self.p.s.add_mob(m);
        id
    }

    fn mob_mut(&mut self, id: i32) -> &mut Mob {
        let i = self.p.s.mob_index(id).expect("mob");
        self.p.s.mobs.list.get_mut(i).expect("mob")
    }

    fn name(&self, at: [i32; 3]) -> String {
        state_string(&self.p.s, self.p.s.get(at))
    }

    fn count(&self, k: Kind) -> usize {
        self.p.s.mobs.list.iter().filter(|m| m.kind == k && !m.removed).count()
    }

    fn wall(&mut self, x: i32, block: &str) {
        self.p.run(&format!("fill {x} {Y} -8 {x} {} 23 {block}\n", Y + 3));
    }
}

/// Village defence: the golem only targets the player a villager dislikes if it can see them
/// (combat conditions, line of sight included); a wall between them prevents it.
#[test]
fn defend_village_needs_line_of_sight() {
    let mut w = world("defend");
    let uuid = w.p.s.uuid;
    let v = w.add(Kind::Villager, [5.5, f64::from(Y), 8.5]);
    w.mob_mut(v).npc.as_mut().expect("villager").gossips.add(uuid, crate::mobs::villager::G_MAJOR_NEG, 100);
    let mut golem = w.p.s.new_mob(Kind::IronGolem, [3.5, f64::from(Y), 8.5], 0.0);
    w.p.s.pos = [11.5, f64::from(Y), 8.5];
    let can = |w: &mut W, golem: &mut Mob| {
        golem.sight.clear();
        let mut g = GolemGoal::DefendVillage { base: TBase::new(false, true), potential: None };
        w.p.s.iron_can_use(&mut g, golem)
    };
    w.wall(8, "minecraft:stone");
    assert!(!can(&mut w, &mut golem), "golem defends the village through a wall");
    w.wall(8, "minecraft:air");
    assert!(can(&mut w, &mut golem), "golem does not defend the village in the open");
    // Player visible on the golem's side, ill-disposed villager alone behind the wall: sight of
    // the villager is required too.
    w.mob_mut(v).set_pos([11.5, f64::from(Y), 8.5]);
    w.p.s.pos = [6.5, f64::from(Y), 8.5];
    w.wall(8, "minecraft:stone");
    assert!(!can(&mut w, &mut golem), "golem defends a villager it cannot see");
    w.wall(8, "minecraft:air");
    assert!(can(&mut w, &mut golem), "golem ignores a visible villager");
}

/// Village defence: the player is only retained if their feet are in the area (upper bounds
/// excluded); a box that overlaps the area is not enough.
#[test]
fn defend_village_player_feet_in_area() {
    let mut w = world("defendfeet");
    let uuid = w.p.s.uuid;
    let v = w.add(Kind::Villager, [5.5, f64::from(Y), 8.5]);
    w.mob_mut(v).npc.as_mut().expect("villager").gossips.add(uuid, crate::mobs::villager::G_MAJOR_NEG, 100);
    let mut golem = w.p.s.new_mob(Kind::IronGolem, [3.5, f64::from(Y), 8.5], 0.0);
    let edge = golem.bbox().max[0] + 10.0;
    let mut can_at = |w: &mut W, x: f64| {
        w.p.s.pos = [x, f64::from(Y), 8.5];
        golem.sight.clear();
        let mut g = GolemGoal::DefendVillage { base: TBase::new(false, true), potential: None };
        w.p.s.iron_can_use(&mut g, &mut golem)
    };
    assert!(can_at(&mut w, edge - 0.1), "player with feet in the area ignored");
    assert!(!can_at(&mut w, edge), "upper bound of the area included");
    assert!(!can_at(&mut w, edge + 0.2), "player with feet outside the area retained through their box");
}

/// Same player query for the phantom's targeting: feet just outside the edge of the area,
/// box entering it, no target (within 20 blocks, the targeting range does not come into play).
#[test]
fn phantom_target_player_feet_in_area() {
    let mut w = world("phantomfeet");
    let mut ph = w.p.s.new_mob(Kind::Phantom, [0.5, f64::from(Y) + 10.0, 8.5], 0.0);
    let edge = ph.bbox().max[0] + 16.0;
    let mut can_at = |w: &mut W, x: f64| {
        w.p.s.pos = [x, f64::from(Y), 8.5];
        ph.sight.clear();
        ph.target = None;
        w.p.s.phantom_target_can_use(&mut ph, &mut 0)
    };
    assert!(can_at(&mut w, edge - 0.1), "player with feet in the area ignored");
    assert!(!can_at(&mut w, edge + 0.2), "player with feet outside the area retained through their box");
}

/// Micro-bench (ignored): cost of the village defence start test, 8 villagers in the area,
/// player outside the area and then inside it in the open (sight cache cleared at every call,
/// as at every tick).
#[test]
#[ignore]
fn bench_defend_village_can_use() {
    let mut w = world("bdefend");
    for i in 0..8 {
        w.add(Kind::Villager, [2.5 + f64::from(i), f64::from(Y), 12.5]);
    }
    let mut golem = w.p.s.new_mob(Kind::IronGolem, [6.5, f64::from(Y), 8.5], 0.0);
    for (label, at) in [("player far", [40.5, f64::from(Y), 40.5]), ("player nearby", [9.5, f64::from(Y), 8.5])] {
        w.p.s.pos = at;
        let n = 100_000;
        let t = std::time::Instant::now();
        let mut hits = 0;
        for _ in 0..n {
            golem.sight.clear();
            let mut g = GolemGoal::DefendVillage { base: TBase::new(false, true), potential: None };
            hits += usize::from(w.p.s.iron_can_use(&mut g, &mut golem));
        }
        eprintln!("{label}: {:.0} ns/call ({hits} true)", t.elapsed().as_nanos() as f64 / f64::from(n));
    }
}

/// Offered flower: only a villager the golem can see is a candidate (non-combat conditions at
/// 6 blocks, line of sight included).
#[test]
fn offer_flower_needs_line_of_sight() {
    let mut w = world("flower");
    let v = w.add(Kind::Villager, [9.5, f64::from(Y), 8.5]);
    let mut golem = w.p.s.new_mob(Kind::IronGolem, [5.5, f64::from(Y), 8.5], 0.0);
    w.wall(7, "minecraft:stone");
    golem.sight.clear();
    assert_eq!(w.p.s.flower_candidate(&mut golem), None, "flower offered through a wall");
    w.wall(7, "minecraft:air");
    golem.sight.clear();
    assert_eq!(w.p.s.flower_candidate(&mut golem), Some(Ent::Mob(v)));
}

/// "Legacy" golem spawn: a bubble column above the support counts as a liquid (like water and
/// lava).
#[test]
fn golem_ground_accepts_bubble_column_above() {
    let mut w = world("bubble");
    w.p.run(&format!(
        "setblock 4 {} 4 minecraft:soul_sand\nsetblock 4 {Y} 4 minecraft:bubble_column[drag=false]\n",
        Y - 1
    ));
    assert_eq!(w.name([4, Y, 4]), "minecraft:bubble_column[drag=false]");
    assert_eq!(w.p.s.legacy_golem_ground([4, Y + 6, 4], 6), Some([4, Y, 4]));
}

/// Spawning by attempts (villagers' golem, shared helper): a start block outside the world
/// border loses the attempt.
#[test]
fn spawn_attempts_respect_world_border() {
    let mut w = world("border");
    w.p.s.cmd.border.center = [1000.0, 1000.0];
    w.p.s.cmd.border.size = 10.0;
    assert!(!w.p.s.summon_golem_for_villager([8, Y, 8]), "golem spawned outside the border");
    let r = w.p.s.spawn_util(Kind::Cow, crate::mobs::spawn::Reason::Natural, [8, Y, 8], 10, 4, 3, true, false);
    assert_eq!(r, None, "cow spawned outside the border");
    assert_eq!(w.count(Kind::IronGolem) + w.count(Kind::Cow), 0);
    w.p.s.cmd.border.center = [0.0, 0.0];
    w.p.s.cmd.border.size = 59_999_968.0;
    assert!(w.p.s.summon_golem_for_villager([8, Y, 8]), "golem missing inside the border");
    let r = w.p.s.spawn_util(Kind::Cow, crate::mobs::spawn::Reason::Natural, [8, Y, 8], 10, 4, 3, true, false);
    assert!(r.is_some(), "cow missing inside the border");
}

/// Copper golem in front of a wall pierced by a closed wooden door, walk target behind it:
/// it opens the door on its path and goes through.
#[test]
fn copper_golem_opens_doors() {
    let mut w = world("door");
    w.wall(10, "minecraft:stone");
    w.p.run(&format!(
        "setblock 10 {Y} 8 minecraft:oak_door[facing=east,half=lower]\nsetblock 10 {} 8 minecraft:oak_door[facing=east,half=upper]\n",
        Y + 1
    ));
    // Player off the path (it would push the golem away).
    w.p.s.pos = [8.5, f64::from(Y), 1.5];
    let id = w.add(Kind::CopperGolem, [6.5, f64::from(Y), 8.5]);
    {
        let m = w.mob_mut(id);
        // Transport on cooldown: only the walk toward the target drives the golem.
        m.golem.brain.transport_cd = Some(100_000);
        m.golem.brain.walk = Some(crate::mobs::golems::copper::Walk { pos: [14, Y, 8], speed: 1.0, close: 0 });
        m.persistent = true;
    }
    let (mut opened, mut max_x) = (false, 0.0f64);
    for _ in 0..400 {
        w.p.tick();
        w.p.s.mobs_world_tick();
        opened |= w.name([10, Y, 8]).contains("open=true");
        let m = w.p.s.mob_ref(id).expect("golem");
        max_x = max_x.max(m.pos()[0]);
        if m.golem.brain.walk.is_none() && max_x < 11.0 {
            let m = w.mob_mut(id);
            m.golem.brain.walk = Some(crate::mobs::golems::copper::Walk { pos: [14, Y, 8], speed: 1.0, close: 0 });
        }
        if max_x > 12.0 {
            break;
        }
    }
    eprintln!("door open {opened}, max x {max_x:.2}");
    assert!(opened, "the copper golem does not open the door");
    assert!(max_x > 12.0, "the copper golem does not go through the door");
    // Door passed, outside the current nodes and within 3 blocks: closed again by the golem.
    assert_eq!(w.name([10, Y, 8]), "minecraft:oak_door[facing=east,half=lower,hinge=left,open=false,powered=false]");
}
