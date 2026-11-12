//! G5-F02e: contacts of entities with the blocks they cross (berry bush, cobweb and Weaving,
//! wither rose, cauldrons, plates and tripwire for all entities) and enchanted boots
//! (Frost Walker, Soul Speed). Fresh flat world: no external data other than the 26.3
//! cache and the client jar, whose absence makes the test FAIL. Expected values taken from
//! the blocks and the enchantment files of the 26.3 game.

use std::path::PathBuf;
use std::sync::Arc;

use rc_proto::nbt::Nbt;

use crate::blocks::probe::{Probe, state_string};
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;
use crate::player::Stack;

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
    let data = rc_testdata::version_data().expect("G5-F02e: 26.3 data cache missing");
    let data = Arc::new(data);
    assert!(crate::registries::client_jar(&data).is_some(), "G5-F02e: 26.3 client jar missing");
    let dir = std::env::temp_dir().join(format!("rc-g5f02e-{tag}-{}", std::process::id()));
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
        assert!(t.elapsed().as_secs() < 300, "G5-F02e: anti-hang guard: flat world generation stuck for 300 s");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    p.run(&format!("fill 0 {Y} 0 15 {} 15 minecraft:air\n", Y + 3));
    // Player far from the tests (at x = 40), on the ground.
    p.s.pos = [40.5, f64::from(Y), 40.5];
    p.s.on_ground = true;
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    let mut w = W { p, dir };
    w.p.s.set_difficulty(2, true);
    w.cmd("time set midnight"); // no zombie burning in the sun
    w
}

impl W {
    fn cmd(&mut self, c: &str) {
        let src = self.p.s.player_source();
        assert!(self.p.s.perform(src, c).0, "command refused: {c}");
    }

    fn block(&self, p: [i32; 3]) -> String {
        state_string(&self.p.s, self.p.s.get(p))
    }

    /// Mob without AI placed at `pos` (it only moves by its velocity when `ai`).
    fn mob(&mut self, k: Kind, pos: [f64; 3], ai: bool) -> Mob {
        let mut m = self.p.s.new_mob(k, pos, 0.0);
        m.no_ai = !ai;
        m.on_ground = true;
        m
    }

    /// The player placed at `pos`, on the ground, known movement `mv`, then one life tick.
    fn player_tick_at(&mut self, pos: [f64; 3], mv: [f64; 3]) {
        self.p.s.pos = pos;
        self.p.s.on_ground = true;
        self.p.s.proj_player.known_move = mv;
        self.p.s.vitals_tick();
    }
}

fn boots(ench: &str, level: i32) -> Stack {
    let mut s = Stack { id: "minecraft:diamond_boots".into(), count: 1, components: None };
    crate::items::set_component(&mut s, "minecraft:enchantments", Nbt::Compound(vec![(ench.into(), Nbt::Int(level))]));
    s
}

fn at(x: i32, z: i32) -> [f64; 3] {
    [f64::from(x) + 0.5, f64::from(Y), f64::from(z) + 0.5]
}

/// Berry bush: 1 `sweet_berry_bush` damage to whoever moves inside (age ≥ 1), except fox and
/// bee; nothing when still or in an age 0 bush.
#[test]
fn berries_sting_whoever_moves_inside() {
    let mut w = world("berries");
    w.p.run(&format!(
        "setblock 2 {Y} 2 minecraft:sweet_berry_bush[age=1]\nsetblock 6 {Y} 2 minecraft:sweet_berry_bush[age=0]\nsetblock 10 {Y} 2 minecraft:sweet_berry_bush[age=3]\nsetblock 2 {Y} 6 minecraft:sweet_berry_bush[age=2]\n"
    ));
    // Pig crossing (given velocity): stung.
    let mut z = w.mob(Kind::Pig, at(2, 2), true);
    z.vel = [0.08, 0.0, 0.0];
    w.p.s.tick_mob(&mut z);
    assert_eq!(z.health, 9.0, "pig moving in the age 1 bush");
    // Motionless zombie (no AI): no damage.
    let mut still = w.mob(Kind::Zombie, at(10, 2), false);
    w.p.s.tick_mob(&mut still);
    assert_eq!(still.health, 20.0, "motionless zombie");
    // Age 0: no damage even when moving.
    let mut young = w.mob(Kind::Zombie, at(6, 2), true);
    young.vel = [0.08, 0.0, 0.0];
    w.p.s.tick_mob(&mut young);
    assert_eq!(young.health, 20.0, "age 0 bush");
    // Fox: never stung.
    let mut fox = w.mob(Kind::Fox, at(2, 6), true);
    fox.vel = [0.08, 0.0, 0.0];
    let full = fox.health;
    w.p.s.tick_mob(&mut fox);
    assert_eq!(fox.health, full, "fox");
    // Player moving in the age 3 bush (movement known to the client).
    w.player_tick_at(at(10, 2), [0.1, 0.0, 0.0]);
    assert_eq!(w.p.s.life.vit.health, 19.0, "player moving");
    // Under the threshold (0.002 per axis): nothing.
    w.p.s.life.vit.health = 20.0;
    w.p.s.life.vit.cooldown = 0;
    w.player_tick_at(at(10, 2), [0.002, 0.0, 0.002]);
    assert_eq!(w.p.s.life.vit.health, 20.0, "player almost still");
}

/// Cobweb: the mob is slowed in it (0.25 horizontally), half as much under Weaving.
#[test]
fn cobweb_slows_mobs_and_weaving() {
    let mut w = world("cobweb");
    w.p.run(&format!("setblock 2 {Y} 2 minecraft:cobweb\nsetblock 2 {Y} 8 minecraft:cobweb\n"));
    let mut plain = w.mob(Kind::Zombie, at(2, 2), true);
    let mut woven = w.mob(Kind::Zombie, at(2, 8), true);
    assert!(w.p.s.give_mob_effect(&mut woven, "minecraft:weaving", 100_000, 0));
    let mut free = w.mob(Kind::Zombie, at(8, 14), true);
    let mut dx = Vec::new();
    for m in [&mut plain, &mut woven, &mut free] {
        m.vel = [0.2, 0.0, 0.0];
        w.p.s.tick_mob(m); // contact set
        m.vel = [0.2, 0.0, 0.0];
        let x0 = m.pos()[0];
        w.p.s.tick_mob(m); // slowed movement
        dx.push(m.pos()[0] - x0);
    }
    assert!(dx[0] > 0.0 && dx[0] < 0.06, "cobweb: {dx:?}");
    assert!((dx[1] / dx[0] - 2.0).abs() < 1.0e-6, "Weaving = half the slowdown: {dx:?}");
    assert!(dx[2] > 0.15, "outside the cobweb: {dx:?}");
}

/// Wither rose: Wither effect 40 ticks on living entities, except in Peaceful and for the
/// invulnerable.
#[test]
fn wither_rose_gives_the_effect() {
    let mut w = world("rose");
    w.p.run(&format!("setblock 2 {Y} 2 minecraft:wither_rose\nsetblock 10 {Y} 10 minecraft:wither_rose\n"));
    let wither = |m: &Mob| m.x.effects.iter().find(|e| e.id == "minecraft:wither").map(|e| e.dur);
    let mut z = w.mob(Kind::Zombie, at(2, 2), false);
    w.p.s.tick_mob(&mut z);
    assert!(wither(&z).is_some_and(|d| (38..=40).contains(&d)), "zombie : {:?}", wither(&z));
    let mut inv = w.mob(Kind::Zombie, at(2, 2), false);
    inv.invulnerable = true;
    w.p.s.tick_mob(&mut inv);
    assert_eq!(wither(&inv), None, "invulnerable mob");
    // Survival player.
    w.player_tick_at(at(10, 10), [0.0; 3]);
    assert!(w.p.s.fx().has("minecraft:wither"), "survival player");
    // Peaceful: nothing.
    w.cmd("effect clear @s");
    w.p.s.set_difficulty(0, true);
    let mut z = w.mob(Kind::Pig, at(2, 2), false);
    w.p.s.tick_mob(&mut z);
    assert_eq!(wither(&z), None, "peaceful (mob)");
    w.player_tick_at(at(10, 10), [0.0; 3]);
    assert!(!w.p.s.fx().has("minecraft:wither"), "peaceful (player)");
}

/// Cauldrons: the burning entity in water lowers the level (powder snow becomes water)
/// and is extinguished, only if it touches the contents; lava ignites (15 s) and hurts.
#[test]
fn cauldrons_extinguish_and_burn() {
    let mut w = world("cauldrons");
    w.p.run(&format!(
        "setblock 2 {Y} 2 minecraft:water_cauldron[level=3]\nsetblock 6 {Y} 2 minecraft:powder_snow_cauldron[level=1]\nsetblock 10 {Y} 2 minecraft:lava_cauldron\nsetblock 2 {Y} 6 minecraft:water_cauldron[level=1]\nsetblock 6 {Y} 6 minecraft:water_cauldron[level=2]\nsetblock 10 {Y} 6 minecraft:lava_cauldron\n"
    ));
    let floor = |x: i32, z: i32| [f64::from(x) + 0.5, f64::from(Y) + 0.25, f64::from(z) + 0.5];
    // Burning zombie at the bottom of a full cauldron.
    let mut z = w.mob(Kind::Zombie, floor(2, 2), false);
    z.fire = 100;
    w.p.s.tick_mob(&mut z);
    assert_eq!(w.block([2, Y, 2]), "minecraft:water_cauldron[level=2]");
    assert!(z.fire <= 0, "extinguished: {}", z.fire);
    // Powder snow level 1: water level 1 then one less = empty cauldron.
    let mut z = w.mob(Kind::Zombie, floor(6, 2), false);
    z.fire = 100;
    w.p.s.tick_mob(&mut z);
    assert_eq!(w.block([6, Y, 2]), "minecraft:cauldron");
    assert!(z.fire <= 0);
    // Above the contents (level 1 = 9 px): nothing.
    let mut z = w.mob(Kind::Zombie, [2.5, f64::from(Y) + 0.6, 6.5], false);
    z.fire = 100;
    w.p.s.tick_mob(&mut z);
    assert_eq!(w.block([2, Y, 6]), "minecraft:water_cauldron[level=1]");
    assert!(z.fire > 0, "outside the contents, still burning");
    // Lava: ignition 15 s, 4 damage, thaw.
    let mut z = w.mob(Kind::Pig, floor(10, 2), false);
    z.ticks_frozen = 50;
    w.p.s.tick_mob(&mut z);
    assert!(z.fire >= 299, "ignited: {}", z.fire);
    assert_eq!(z.health, 6.0, "lava damage");
    assert_eq!(z.ticks_frozen, 0, "thaw");
    // Player burning in water, then in lava.
    w.p.s.life.vit.fire = 100;
    w.player_tick_at(floor(6, 6), [0.0; 3]);
    assert_eq!(w.block([6, Y, 6]), "minecraft:water_cauldron[level=1]");
    assert!(w.p.s.life.vit.fire <= 0, "player extinguished: {}", w.p.s.life.vit.fire);
    w.player_tick_at(floor(10, 6), [0.0; 3]);
    assert!(w.p.s.life.vit.fire >= 299, "player ignited: {}", w.p.s.life.vit.fire);
    assert!(w.p.s.life.vit.health <= 16.0, "player hurt: {}", w.p.s.life.vit.health);
}

/// L9-TICK: cauldrons share the crossed-block pass's deferred effects, in the order of its
/// steps: the player leaving a water cauldron for lava in one tick is extinguished (first step)
/// then set on fire (later step), and the cauldron keeps its level (not burning when it was
/// entered). Before: cauldrons came in a second group after the whole pass, so the player ended
/// extinguished and the cauldron lowered.
#[test]
fn cauldron_and_lava_in_one_tick_follow_the_pass_order() {
    let mut w = world("cauldron-order");
    w.p.run(&format!("setblock 2 {Y} 2 minecraft:water_cauldron[level=3]\nsetblock 3 {Y} 2 minecraft:lava\n"));
    let floor = |x: f64| [x, f64::from(Y) + 0.25, 2.5];
    w.player_tick_at(floor(2.5), [0.0; 3]);
    assert!(w.p.s.life.vit.fire <= 0);
    w.player_tick_at(floor(3.5), [1.0, 0.0, 0.0]);
    assert!(w.p.s.life.vit.fire >= 299, "set on fire by the lava after the cauldron: {}", w.p.s.life.vit.fire);
    assert_eq!(w.block([2, Y, 2]), "minecraft:water_cauldron[level=3]", "not burning when it entered the cauldron");
}

/// L9-TICK: the direct contacts (berry bush) come before the deferred effects (lava ignition
/// and damage), as in the game where they play while it walks the cells: a pig at 1 health
/// running from a berry bush into lava dies of the bush, not set on fire. Before: lava first
/// (burning, killed by the lava: cooked drops).
#[test]
fn direct_contacts_before_the_deferred_effects() {
    let mut w = world("contact-order");
    w.p.run(&format!("setblock 2 {Y} 6 minecraft:sweet_berry_bush[age=1]\nsetblock 3 {Y} 6 minecraft:lava\n"));
    let mut pig = w.mob(Kind::Pig, at(2, 6), true);
    pig.health = 1.0;
    pig.vel = [1.0, 0.0, 0.0];
    w.p.s.tick_mob(&mut pig);
    assert!(pig.pos()[0] > 3.0, "pig in the lava cell: {:?}", pig.pos());
    assert!(pig.health <= 0.0, "killed by the bush");
    assert!(pig.fire <= 0, "set on fire after its death: {}", pig.fire);
}

/// Plates and tripwire: a mob and a ground item press the plate; a mob tightens the
/// tripwire, a bat does not.
#[test]
fn plates_and_tripwire_for_all_entities() {
    let mut w = world("plates");
    w.p.run(&format!(
        "setblock 2 {Y} 2 minecraft:stone_pressure_plate\nsetblock 6 {Y} 2 minecraft:oak_pressure_plate\nsetblock 10 {Y} 2 minecraft:tripwire\nsetblock 10 {Y} 6 minecraft:tripwire\n"
    ));
    let mut z = w.mob(Kind::Zombie, [2.5, f64::from(Y) + 0.0625, 2.5], false);
    w.p.s.tick_mob(&mut z);
    assert_eq!(w.block([2, Y, 2]), "minecraft:stone_pressure_plate[powered=true]", "mob on the plate");
    // Item dropped on the wooden plate.
    let s = Stack { id: "minecraft:stick".into(), count: 1, components: None };
    w.p.s.spawn_item([6.5, f64::from(Y) + 0.07, 6.5 - 4.0], [0.0; 3], s, 40);
    if let Some(d) = w.p.s.life.drops.last_mut() {
        d.pos = [6.5, f64::from(Y) + 0.0625, 2.5];
    }
    w.p.s.drops_tick();
    assert_eq!(w.block([6, Y, 2]), "minecraft:oak_pressure_plate[powered=true]", "item on the plate");
    // Tripwire: the bat does not count, the zombie does.
    let mut bat = w.mob(Kind::Bat, [10.5, f64::from(Y) + 0.1, 6.5], false);
    w.p.s.tick_mob(&mut bat);
    assert!(w.block([10, Y, 6]).contains("powered=false"), "bat: {}", w.block([10, Y, 6]));
    let mut z = w.mob(Kind::Zombie, [10.5, f64::from(Y), 2.5], false);
    w.p.s.tick_mob(&mut z);
    assert!(w.block([10, Y, 2]).contains("powered=true"), "zombie: {}", w.block([10, Y, 2]));
}

/// Projectile added to the world, launched at `vel`.
fn shoot(w: &mut W, kind: crate::projectiles::PKind, pos: [f64; 3], vel: [f64; 3]) -> i32 {
    let id = w.p.s.proj_new_id();
    let item = Stack { id: kind.default_item().into(), count: 1, components: None };
    let mut p = crate::projectiles::Proj::new(kind, id, pos, item);
    p.vel = vel;
    p.has_been_shot = true;
    w.p.s.proj_add(p);
    id
}

/// Tripwire: only the box that intersects its shape tightens it (1 to 2.5 px attached, 0 to 8 px
/// otherwise). Player and mob passing above an attached wire, arrow 0.6 above a loose wire:
/// nothing; player on the ground and snowball at 0.3: tightened.
#[test]
fn tripwire_tightened_only_by_its_shape() {
    use crate::projectiles::PKind;
    let mut w = world("wire-shape");
    w.p.run(&format!(
        "setblock 10 {Y} 10 minecraft:tripwire[attached=true]\nsetblock 14 {Y} 10 minecraft:tripwire[attached=true]\nsetblock 10 {Y} 14 minecraft:tripwire[attached=true]\nsetblock 6 {Y} 14 minecraft:tripwire\nsetblock 2 {Y} 14 minecraft:tripwire\n"
    ));
    let y = f64::from(Y);
    // Player jumping over (feet at 0.3, above the 2.5 px).
    w.p.s.pos = [10.5, y + 0.3, 10.5];
    w.p.s.player_touch_blocks([9.9, y + 0.3, 10.5]);
    assert!(w.block([10, Y, 10]).contains("powered=false"), "player above: {}", w.block([10, Y, 10]));
    // Player walking on it: tightened.
    w.p.s.pos = [14.5, y, 10.5];
    w.p.s.player_touch_blocks([13.9, y, 10.5]);
    assert!(w.block([14, Y, 10]).contains("powered=true"), "player on the ground: {}", w.block([14, Y, 10]));
    w.p.s.pos = [40.5, y, 40.5];
    // Mob passing above the attached wire.
    let mut z = w.mob(Kind::Zombie, [10.5, y + 0.3, 14.5], true);
    z.on_ground = false;
    z.vel = [0.2, 0.0, 0.0];
    w.p.s.tick_mob(&mut z);
    assert!(z.pos()[1] > y + 2.5 / 16.0, "zombie stayed above: {}", z.pos()[1]);
    assert!(w.block([10, Y, 14]).contains("powered=false"), "mob above: {}", w.block([10, Y, 14]));
    // Arrow at chest height (box from 0.6 to 1.1) through a loose wire (0 to 0.5).
    shoot(&mut w, PKind::Arrow, [5.6, y + 0.6, 14.5], [0.8, 0.0, 0.0]);
    w.p.s.projectiles_tick();
    assert!(w.block([6, Y, 14]).contains("powered=false"), "high arrow: {}", w.block([6, Y, 14]));
    // Low snowball (thrown item) through a loose wire: tightened.
    shoot(&mut w, PKind::Snowball, [1.6, y + 0.3, 14.5], [0.8, 0.0, 0.0]);
    w.p.s.projectiles_tick();
    assert!(w.block([2, Y, 14]).contains("powered=true"), "low snowball: {}", w.block([2, Y, 14]));
}

/// Snowball on a wooden plate: it presses it on the tick of its impact, before being
/// removed (block effects before the impact, like the game).
#[test]
fn snowball_presses_the_plate() {
    use crate::projectiles::PKind;
    let mut w = world("snow-plate");
    w.p.run(&format!("setblock 2 {Y} 10 minecraft:oak_pressure_plate\n"));
    let id = shoot(&mut w, PKind::Snowball, [2.5, f64::from(Y) + 0.5, 10.5], [0.0, -0.8, 0.0]);
    w.p.s.projectiles_tick();
    assert!(!w.p.s.proj.list.iter().any(|p| p.id == id && !p.removed), "snowball removed on impact");
    assert_eq!(w.block([2, Y, 10]), "minecraft:oak_pressure_plate[powered=true]");
}

/// Projectiles: blocks crossed along a single straight line, from the start of the tick to the
/// position taken (no per-axis split, specific to movements with collisions). Attached wire
/// (1 to 2.5 px). Arrow going down past the wire (bottom of its box still 0.22 above it):
/// relaxed; snowball rising across: tightened; arrow at an angle through a head-height wire
/// (cell never seen by a Y then X split): tightened.
#[test]
fn tripwire_tightened_by_projectiles_in_a_straight_line() {
    use crate::projectiles::PKind;
    let mut w = world("straight-wire");
    w.p.run(&format!(
        "setblock 3 {Y} 2 minecraft:tripwire[attached=true]\nsetblock 3 {Y} 6 minecraft:tripwire[attached=true]\nsetblock 3 {} 10 minecraft:tripwire[attached=true]\n",
        Y + 1
    ));
    let y = f64::from(Y);
    shoot(&mut w, PKind::Arrow, [2.5, y + 0.5, 2.5], [2.5, -0.4, 0.0]);
    w.p.s.projectiles_tick();
    // Snowball: gravity then inertia before the step, which goes from (2.6; 0.01) to (4.1; 0.41).
    let k = f64::from(0.99f32);
    shoot(&mut w, PKind::Snowball, [2.6, y + 0.01, 6.5], [1.5 / k, 0.4 / k + PKind::Snowball.gravity(), 0.0]);
    w.p.s.projectiles_tick();
    shoot(&mut w, PKind::Arrow, [2.5, y + 1.6, 10.5], [2.5, -1.5, 0.0]);
    w.p.s.projectiles_tick();
    let got = [w.block([3, Y, 2]), w.block([3, Y, 6]), w.block([3, Y + 1, 10])];
    // Arrow going down past the wire, snowball going up, arrow at an angle.
    assert_eq!(got.clone().map(|s| s.contains("powered=true")), [false, true, true], "{got:?}");
}

/// Cap of the crossed-block walk (16 iterations for the whole path), then only the arrival
/// box, like the game; values computed apart (outside the repository) from the game's walk,
/// not from this code. Arrow at 20 blocks per tick along the ground, loose wires on its line at
/// x = 15, 16 and 20: tightened at 15 and 20. TNT rushing 9.9 in X then 9.9 in Z (per-axis
/// segments), loose wires on the Z leg from z = 21 to 30: tightened from 21 to 24 then at 29
/// and 30 (arrival box).
#[test]
fn crossed_blocks_iteration_cap() {
    use crate::projectiles::PKind;
    let mut w = world("cap");
    let wires = |w: &mut W, cells: &[[i32; 3]]| {
        let cmds: String =
            cells.iter().map(|c| format!("setblock {} {} {} minecraft:tripwire\n", c[0], c[1], c[2])).collect();
        w.p.run(&cmds);
    };
    let powered = |w: &W, cells: &[[i32; 3]]| -> Vec<bool> {
        cells.iter().map(|&c| w.block(c).contains("powered=true")).collect()
    };
    let y = f64::from(Y);
    let line: Vec<[i32; 3]> = [15, 16, 20].iter().map(|&x| [x, Y, 34]).collect();
    wires(&mut w, &line);
    shoot(&mut w, PKind::Arrow, [0.5, y + 0.05, 34.5], [20.0, 0.0, 0.0]);
    w.p.s.projectiles_tick();
    assert_eq!(powered(&w, &line), [true, false, true], "arrow in a straight line, x = 15, 16, 20");
    let leg: Vec<[i32; 3]> = (21..=30).map(|z| [12, Y, z]).collect();
    wires(&mut w, &leg);
    // Gravity (0.04) before the movement: step of (9.9; 0; 9.9).
    w.cmd(&format!("summon minecraft:tnt 2.5 {y} 20.5 {{Motion:[9.9d,0.04d,9.9d]}}"));
    w.p.s.tick_sim_entities();
    let want = [true, true, true, true, false, false, false, false, true, true];
    assert_eq!(powered(&w, &leg), want, "TNT per axis, z = 21 to 30");
}

/// Projectiles: fire and lava crossed in flight, on the straight line of the step (not only at
/// the arrival); lava only if the box touches the fluid (8/9 of a block for a source). Arrows
/// at 3 blocks per tick through a lava source then a fire: ignited; arrow passing through the
/// lava cell above the fluid: not. The fire is alone in its segment (the palette scan must
/// see it without the help of the lava).
#[test]
fn arrows_ignited_by_crossed_lava_and_fire() {
    use crate::projectiles::PKind;
    let mut w = world("arrow-fire");
    w.p.run(&format!(
        "setblock 3 {Y} 2 minecraft:lava\nsetblock 3 {Y} 22 minecraft:fire\nsetblock 3 {Y} 10 minecraft:lava\n"
    ));
    let y = f64::from(Y);
    let ids = [
        shoot(&mut w, PKind::Arrow, [2.5, y + 0.2, 2.5], [3.0, 0.0, 0.0]),
        shoot(&mut w, PKind::Arrow, [2.5, y + 0.2, 22.5], [3.0, 0.0, 0.0]),
        shoot(&mut w, PKind::Arrow, [2.5, y + 0.9, 10.5], [3.0, 0.0, 0.0]),
    ];
    w.p.s.projectiles_tick();
    let lit = ids.map(|id| w.p.s.proj.list.iter().find(|p| p.id == id).map(|p| p.fire > 0));
    assert_eq!(lit, [Some(true), Some(true), Some(false)], "lava crossed, fire crossed, above the lava");
}

/// Falling block and TNT: portals touched per axis (Y, then X), like their movement with
/// collisions, not in a straight line. TNT rushing at an angle from (2.5; 2) to (3.6; 0.9): the
/// cell (3, 2) of a portal is only seen in a straight line, so not marked; TNT falling into a
/// portal: marked.
#[test]
fn portals_touched_per_axis_for_falling_block() {
    let mut w = world("portal-axis");
    let top = Y + 2;
    w.p.run(&format!("setblock 3 {top} 2 minecraft:nether_portal\nsetblock 7 {top} 2 minecraft:nether_portal\n"));
    assert!(w.block([3, Y + 2, 2]).starts_with("minecraft:nether_portal"), "portal placed: {}", w.block([3, Y + 2, 2]));
    let y = f64::from(Y);
    let (from, to) = ([2.5, y + 2.0, 2.5], [3.6, y + 0.9, 2.5]);
    let n = |w: &W, straight: bool| w.p.s.portals_touched_swept(from, to, 0.98, 0.98, straight).len();
    assert_eq!((n(&w, true), n(&w, false)), (1, 0), "straight line, per axis");
    // Gravity (0.04) before the movement: step of (1.1; −1.1).
    w.cmd(&format!("summon minecraft:tnt 2.5 {} 2.5 {{Motion:[1.1d,-1.06d,0.0d]}}", y + 2.0));
    w.cmd(&format!("summon minecraft:tnt 7.5 {} 2.5", y + 2.2));
    let ids: Vec<i32> = w.p.s.bs.entities.list.iter().map(|e| e.id).collect();
    w.p.s.tick_sim_entities();
    let marked: Vec<bool> = ids.iter().map(|&id| w.p.s.entity_inside_portal_now(id)).collect();
    assert_eq!(marked, [false, true], "TNT at an angle, TNT falling into the portal");
}

/// Falling block and TNT: plates touched right after the movement, before placement or breaking
/// and before the fuse. Sand without dropped item (`DropItem:0b`) removed on arriving on an oak
/// plate, TNT defused on its last tick (`tnt_explodes` false): the plate is pressed on that tick.
#[test]
fn sand_and_tnt_press_the_plate_before_vanishing() {
    let mut w = world("sand-plate");
    w.p.run(&format!(
        "setblock 10 {Y} 2 minecraft:oak_pressure_plate\nsetblock 14 {Y} 2 minecraft:oak_pressure_plate\n"
    ));
    w.cmd("gamerule tnt_explodes false");
    let y = f64::from(Y);
    w.cmd(&format!(
        "summon minecraft:falling_block 10.5 {} 2.5 {{BlockState:{{Name:\"minecraft:sand\"}},DropItem:0b,Motion:[0.0d,-0.6d,0.0d]}}",
        y + 0.5
    ));
    w.cmd(&format!("summon minecraft:tnt 14.5 {y} 2.5 {{fuse:1}}"));
    w.p.s.tick_sim_entities();
    assert!(w.p.s.bs.entities.list.is_empty(), "sand and TNT removed on that tick");
    let pressed = "minecraft:oak_pressure_plate[powered=true]";
    assert_eq!([w.block([10, Y, 2]), w.block([14, Y, 2])], [pressed, pressed], "sand, TNT");
}

/// Light weighted pressure plate: each item tests it right after its own movement. The first
/// (already settled) sees the second still in the air: signal 1, which the second, arrived on
/// the same tick, does not recompute.
#[test]
fn weighted_plate_counted_item_by_item() {
    let mut w = world("weighted");
    w.p.run(&format!("setblock 6 {Y} 10 minecraft:light_weighted_pressure_plate\n"));
    let y = f64::from(Y);
    for (id, h, vy) in [("minecraft:stick", 0.0, 0.0), ("minecraft:dirt", 0.6, -0.7)] {
        let s = Stack { id: id.into(), count: 1, components: None };
        w.p.s.spawn_item([6.5, y + h, 10.5], [0.0; 3], s, 40);
        let d = w.p.s.life.drops.last_mut().expect("item");
        d.pos = [6.5, y + h, 10.5];
        d.vel = [0.0, vy, 0.0];
        d.on_ground = h == 0.0;
    }
    w.p.s.drops_tick();
    let n = w.p.s.life.drops.iter().filter(|d| d.pos[1] < y + 0.25).count();
    assert_eq!(n, 2, "both items on the plate");
    assert_eq!(w.block([6, Y, 10]), "minecraft:light_weighted_pressure_plate[power=1]");
}

/// Frost Walker: frosted ice disc (radius 3 at level 1) on the source water under the grounded
/// player; immunity to `hot_floor` damage.
#[test]
fn frost_walker_freezes_water() {
    let mut w = world("frost");
    w.p.run(&format!("fill 0 {} 0 15 {} 15 minecraft:water\n", Y - 1, Y - 1));
    w.p.s.slots[8] = Some(boots("minecraft:frost_walker", 1));
    w.player_tick_at(at(8, 8), [0.0; 3]);
    let mut n = 0;
    for x in 0..16 {
        for z in 0..16 {
            let ice = w.block([x, Y - 1, z]).starts_with("minecraft:frosted_ice");
            let (dx, dz) = (f64::from(x) - 8.0, f64::from(z) - 8.0);
            assert_eq!(ice, dx * dx + dz * dz < 9.0, "cell {x} {z}");
            n += usize::from(ice);
        }
    }
    assert_eq!(n, 25, "disc of radius 3");
    // Immunity to the hot floor with the boots, not without.
    assert!(!w.p.s.hurt("minecraft:hot_floor", 1.0), "immune with Frost Walker");
    w.p.s.slots[8] = None;
    assert!(w.p.s.hurt("minecraft:hot_floor", 1.0), "damage without the boots");
}

/// Soul Speed: on soul sand, +0.0405 + 0.0105 per level beyond the first to speed and
/// movement efficiency 1; removed on another block on the ground.
#[test]
fn soul_speed_on_soul_sand() {
    let mut w = world("soul");
    w.p.run(&format!("setblock 8 {} 8 minecraft:soul_sand\nsetblock 9 {} 8 minecraft:stone\n", Y - 1, Y - 1));
    let base = w.p.s.player_attribute("minecraft:movement_speed");
    w.p.s.slots[8] = Some(boots("minecraft:soul_speed", 3));
    w.player_tick_at(at(8, 8), [0.0; 3]);
    let bonus = f64::from(0.0405f32 + 0.0105f32 * 2.0);
    assert_eq!(w.p.s.player_attribute("minecraft:movement_speed"), base + bonus);
    assert_eq!(w.p.s.player_attribute("minecraft:movement_efficiency"), 1.0);
    w.player_tick_at(at(9, 8), [0.0; 3]);
    assert_eq!(w.p.s.player_attribute("minecraft:movement_speed"), base, "removed off the soul sand");
    assert_eq!(w.p.s.player_attribute("minecraft:movement_efficiency"), 0.0);
    // Booted mob: speed increased on soul sand.
    w.p.run(&format!("setblock 2 {} 2 minecraft:soul_sand\n", Y - 1));
    let mut z = w.mob(Kind::Zombie, at(2, 2), false);
    let before = z.attrs.speed.value();
    z.equipment[2] = Some(boots("minecraft:soul_speed", 1));
    w.p.s.tick_mob(&mut z);
    assert_eq!(z.attrs.speed.value(), before + f64::from(0.0405f32));
}

/// Benches: rich section of chunk (0, 1) (about forty states, no contact block).
fn rich_section(w: &mut W) {
    let colors = ["white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray"];
    let colors = colors.iter().chain(&["light_gray", "cyan", "purple", "blue", "brown", "green", "red", "black"]);
    let flowers = ["dandelion", "poppy", "blue_orchid", "allium", "azure_bluet", "red_tulip", "orange_tulip"];
    let flowers =
        flowers.iter().chain(&["white_tulip", "pink_tulip", "oxeye_daisy", "cornflower", "lily_of_the_valley"]);
    let leaves = ["oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "azalea", "pale_oak"];
    let mut cmds = String::new();
    for (i, c) in colors.enumerate() {
        cmds += &format!("setblock {} {} 28 minecraft:{c}_wool\n", i % 16, Y + 2);
    }
    for (i, f) in flowers.enumerate() {
        cmds += &format!("setblock {} {Y} 30 minecraft:{f}\n", i);
    }
    for (i, l) in leaves.iter().enumerate() {
        cmds += &format!("setblock {} {} 30 minecraft:{l}_leaves[persistent=true]\n", i, Y + 3);
    }
    w.p.run(&cmds);
}

/// Bench (run by hand, `--ignored`): cost per tick of the crossed-block walk of a walking mob
/// (0.2 block per tick): empty section (flat world palette), rich section, open air in the
/// section of a cobweb, then in the cobweb.
#[test]
#[ignore]
fn bench_mob_contacts() {
    let mut w = world("bench");
    w.p.run(&format!("setblock 12 {Y} 12 minecraft:cobweb\n"));
    rich_section(&mut w);
    for (label, pos) in [("empty", at(4, 36)), ("rich", at(4, 20)), ("air", at(4, 4)), ("cobweb", at(12, 12))] {
        let mut z = w.mob(Kind::Zombie, pos, false);
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            for _ in 0..100_000 {
                z.old = [pos[0] - 0.2, pos[1], pos[2] - 0.1];
                z.set_pos(pos);
                w.p.s.mob_contacts(&mut z);
            }
            best = best.min(t.elapsed().as_secs_f64() * 1.0e9 / 100_000.0);
        }
        println!("bench contacts {label}: {best:.1} ns per tick and per mob");
    }
}

/// Bench (run by hand, `--ignored`): cost of one step of the block effects of an arrow rushing
/// at an angle (2.5 blocks per tick, 0.4 of descent): empty section (flat world palette),
/// rich section (about forty states, no contact block), open air in the section of a row of
/// plates, then right at the plates (box above their contact zone: nothing changes).
#[test]
#[ignore]
fn bench_projectile_contacts() {
    use crate::projectiles::{PKind, Proj};
    let mut w = world("bench-proj");
    w.p.run(&format!("fill 0 {Y} 10 15 {Y} 10 minecraft:oak_pressure_plate\n"));
    rich_section(&mut w);
    let y = f64::from(Y);
    for (label, z, h) in [("empty", 36.5, 1.6), ("rich", 20.5, 1.6), ("air", 4.5, 1.6), ("plates", 10.5, 0.9)] {
        let item = Stack { id: PKind::Arrow.default_item().into(), count: 1, components: None };
        let mut p = Proj::new(PKind::Arrow, w.p.s.proj_new_id(), [5.5, y + h - 0.4, z], item);
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            for _ in 0..100_000 {
                p.pos = [5.5, y + h - 0.4, z];
                w.p.s.proj_blocks_effects_from(&mut p, [3.0, y + h, z]);
            }
            best = best.min(t.elapsed().as_secs_f64() * 1.0e9 / 100_000.0);
        }
        assert!(w.block([5, Y, 10]).contains("powered=false"), "bench: plate left raised");
        println!("bench projectile {label}: {best:.1} ns per step");
    }
}
