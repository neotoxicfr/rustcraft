//! What creatures meet in the world, as the game has it: a fast fall through water or a cobweb
//! restarts the fall from zero (mobs and the player); a block blown up gives its silverfish and,
//! when a player set the blast off, its experience; the purely visual fire shows; a cow milked
//! with a stack of buckets fills the inventory; copper chests and statues are not walked through.
//! World tests run on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, area 200..231); without
//! it they are skipped.

use rc_proto::nbt::Nbt;

use super::mob::{Ent, Mob};
use super::species::Kind;
use super::tests_spawn_paths::open;
use crate::blocks::probe::Probe;
use crate::player::Stack;

fn spawn(p: &mut Probe, kind: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(kind, pos, 0.0);
    let mut group = super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::spawn::Reason::SpawnEgg, &mut group);
    m.set_pos(pos);
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn with(p: &mut Probe, id: i32, f: impl FnOnce(&mut Mob)) {
    let i = p.s.mob_index(id).expect("mob");
    f(p.s.mobs.list.get_mut(i).expect("mob"));
}

fn mob(p: &Probe, id: i32) -> &Mob {
    p.s.mob_ref(id).expect("mob")
}

fn world(tag: &str) -> Option<Probe> {
    let (mut p, _dir) = open(tag)?;
    p.s.pos = [216.5, -59.0, 202.5];
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    Some(p)
}

/// A zombie falling from 10 blocks, 2 blocks per tick, crosses a water block on this tick's
/// path: its fall restarts from zero before its landing is worked out. Fails on the code before
/// (the fall kept growing: about 12 blocks).
#[test]
fn a_fast_fall_through_water_restarts_a_mobs_fall() {
    let Some(mut p) = world("mob-fall-ray") else { return };
    p.run("setblock 205 -52 205 minecraft:water\n");
    let zombie = spawn(&mut p, Kind::Zombie, [205.5, -50.0, 205.5]);
    with(&mut p, zombie, |z| {
        z.fall_distance = 10.0;
        z.vel = [0.0, -2.0, 0.0];
        z.on_ground = false;
    });
    p.s.mobs_world_tick();
    let fall = mob(&p, zombie).fall_distance;
    assert!(fall < 5.0, "fall kept through the water: {fall}");
}

/// The player falls 6 blocks in one move through a cobweb: the fall restarts from zero, only
/// the rest of the move counts. Fails on the code before (16 blocks of fall).
#[test]
fn a_fast_fall_through_a_cobweb_restarts_the_players_fall() {
    let Some(mut p) = world("player-fall-ray") else { return };
    p.run("setblock 210 -55 210 minecraft:cobweb\n");
    p.s.game_mode = 0;
    p.s.on_ground = false;
    p.s.life.vit.fall = 10.0;
    let before = [210.5, -52.0, 210.5];
    p.s.pos = [210.5, -58.0, 210.5];
    p.s.after_move(before, false);
    let fall = p.s.life.vit.fall;
    assert!((fall - 6.0).abs() < 1.0e-4, "fall after the cobweb: {fall}");
}

/// A blast set off by the player destroys an infested stone and a sculk block next to it: the
/// stone lets out a silverfish, the sculk drops its point of experience. The same blast from a
/// mob leaves no experience. Fails on the code before (neither silverfish nor experience).
#[test]
fn a_blown_block_has_its_after_break_effects() {
    for (by, experience) in [(Some(Ent::Player), 1), (None, 0)] {
        let Some(mut p) = world("blast-after-break") else { return };
        p.run("fill 214 -59 214 218 -55 218 minecraft:air\nsetblock 216 -59 216 minecraft:infested_stone\nsetblock 216 -58 216 minecraft:sculk\n");
        let silverfish_before = p.s.mobs.list.iter().filter(|m| m.kind == Kind::Silverfish).count();
        let ty = if by.is_some() { "minecraft:player_explosion" } else { "minecraft:explosion" };
        p.s.explode_core(None, by, None, [216.5, -57.5, 216.5], 3.0, false, ty);
        assert!(p.s.is_air([216, -59, 216]) && p.s.is_air([216, -58, 216]), "blocks not blown");
        let silverfish = p.s.mobs.list.iter().filter(|m| m.kind == Kind::Silverfish).count() - silverfish_before;
        assert_eq!(silverfish, 1, "silverfish of the infested stone (author {by:?})");
        let points: i32 = p.s.life.orbs.iter().filter(|o| o.health > 0).map(|o| o.value * o.count).sum();
        assert_eq!(points, experience, "experience of the sculk (author {by:?})");
    }
}

/// A zombie read with `HasVisualFire:1b` shows flames from its first tick without burning, and
/// keeps the flag when saved. Fails on the code before (no flames, the flag only kept by chance).
#[test]
fn visual_fire_shows_on_a_mob() {
    let Some(mut p) = world("visual-fire") else { return };
    let zombie = spawn(&mut p, Kind::Zombie, [212.5, -59.0, 212.5]);
    with(&mut p, zombie, |z| {
        let mut e = z.save();
        crate::nbtio::set(&mut e, "HasVisualFire", Nbt::Byte(1));
        z.load(&e);
        // Saved again from a fresh compound: the flag must come from the mob itself.
        z.nbt = None;
    });
    p.s.mobs_world_tick();
    let z = mob(&p, zombie);
    assert!(z.fire_flag && !z.on_fire(), "flames {} while burning {}", z.fire_flag, z.on_fire());
    assert_eq!(crate::nbtio::int(&z.save(), "HasVisualFire"), Some(1), "flag not saved");
}

/// Milking an adult cow with 16 buckets: one bucket used, the milk bucket goes to the inventory,
/// nothing falls on the ground; in creative the buckets stay and one milk bucket is added, once.
/// Fails on the code before (the milk was thrown on the ground, creative got nothing).
#[test]
fn milking_with_a_stack_of_buckets_fills_the_inventory() {
    let Some(mut p) = world("milk-bucket-stack") else { return };
    p.s.pos = [216.5, -59.0, 214.5];
    let cow = spawn(&mut p, Kind::Cow, [216.5, -59.0, 216.5]);
    let has_milk = |p: &Probe| p.s.slots.iter().flatten().filter(|s| s.id == "minecraft:milk_bucket").map(|s| s.count).sum::<i32>();
    for creative in [false, true] {
        p.s.game_mode = if creative { 1 } else { 0 };
        p.s.held = 0;
        for slot in p.s.slots.iter_mut() {
            *slot = None;
        }
        p.s.slots[36] = Some(Stack { id: "minecraft:bucket".into(), count: 16, components: None });
        let drops_before = p.s.life.drops.len();
        assert!(p.s.player_interact(cow, 0), "milking refused (creative {creative})");
        assert_eq!(p.s.life.drops.len(), drops_before, "milk thrown on the ground (creative {creative})");
        let buckets = p.s.slots[36].as_ref().map_or(0, |s| s.count);
        assert_eq!(buckets, if creative { 16 } else { 15 }, "buckets left (creative {creative})");
        assert_eq!(has_milk(&p), 1, "milk buckets (creative {creative})");
        if creative {
            assert!(p.s.player_interact(cow, 0), "second milking refused");
            assert_eq!(has_milk(&p), 1, "creative added a second milk bucket");
        }
    }
}

/// Swing events the current player's own client received since the last read: (hand, kind,
/// duration).
fn own_swings(p: &Probe) -> Vec<(i32, i32, i32)> {
    let me = p.s.player_id;
    p.take_events()
        .into_iter()
        .filter_map(|e| match e {
            rc_client::Event::Entity(rc_client::EntityEvent::Swing { id, hand, kind, duration }) if id == me => {
                Some((hand, kind, duration))
            }
            _ => None,
        })
        .collect()
}

/// After a right click on a mob that only the server knows worked (shearing a sheep, feeding an
/// adult cow into love), the server swings the player's arm for the player too and resets the
/// attack charge; milking (foreseen by the client) resets the charge without telling the player;
/// a villager without offers gives no swing at all. Fails on the code before (no swing, the attack
/// charge kept).
#[test]
fn a_successful_click_on_a_mob_swings_the_arm() {
    let Some(mut p) = world("interaction-swing") else { return };
    p.s.pos = [216.5, -59.0, 214.5];
    p.s.held = 0;
    let sheep = spawn(&mut p, Kind::Sheep, [216.5, -59.0, 216.5]);
    let cow = spawn(&mut p, Kind::Cow, [217.5, -59.0, 216.5]);
    /// `item` alone in hand, a charged attack, then the click: swings received and charge left.
    fn click(p: &mut Probe, id: i32, item: &str) -> (Vec<(i32, i32, i32)>, i32) {
        p.s.slots[36] = Some(Stack { id: item.into(), count: 1, components: None });
        p.s.mobs.attack_ticker = 50;
        let _ = own_swings(p);
        assert!(p.s.player_interact(id, 0), "{item} on mob {id} refused");
        (own_swings(p), p.s.mobs.attack_ticker)
    }
    assert_eq!(click(&mut p, sheep, "minecraft:shears"), (vec![(0, 1, 6)], 0), "shearing");
    assert_eq!(click(&mut p, cow, "minecraft:wheat"), (vec![(0, 1, 6)], 0), "feeding into love");
    assert_eq!(click(&mut p, cow, "minecraft:bucket"), (vec![], 0), "milking");
    let villager = spawn(&mut p, Kind::Villager, [215.5, -59.0, 216.5]);
    with(&mut p, villager, |v| v.age = 0);
    let (swings, charge) = click(&mut p, villager, "minecraft:stick");
    assert_eq!((swings, charge), (vec![], 50), "villager without offers");
}

/// Effect packets about entity `id` the current player's own client received: (effect, added).
fn effects_about(p: &Probe, id: i32) -> Vec<(i32, bool)> {
    p.take_events()
        .into_iter()
        .filter_map(|e| match e {
            rc_client::Event::Entity(rc_client::EntityEvent::Effect { id: of, effect, .. }) if of == id => Some((effect, true)),
            rc_client::Event::Entity(rc_client::EntityEvent::RemoveEffect { id: of, effect }) if of == id => {
                Some((effect, false))
            }
            _ => None,
        })
        .collect()
}

/// A horse with jump boost: the player who gets on it receives the effect, then a slow falling
/// given while riding, and both are taken back when getting off; a horse nobody rides sends
/// nothing. Fails on the code before (a mob's effects never reached its rider).
#[test]
fn a_mounts_effects_reach_its_rider() {
    let Some(mut p) = world("mount-effects-rider") else { return };
    p.s.pos = [216.5, -59.0, 214.5];
    p.s.sneaking = false;
    let horse = spawn(&mut p, Kind::Horse, [216.5, -59.0, 216.5]);
    let id_of = |p: &Probe, name: &str| {
        p.s.data.registry("minecraft:mob_effect").and_then(|r| r.entries.iter().position(|e| e == name)).expect("effect") as i32
    };
    let (jump, slow) = (id_of(&p, "minecraft:jump_boost"), id_of(&p, "minecraft:slow_falling"));
    let i = p.s.mob_index(horse).expect("horse");
    let mut h = p.s.mobs.list.take(i).expect("horse");
    assert!(p.s.give_mob_effect(&mut h, "minecraft:jump_boost", 600, 1), "jump boost refused");
    assert_eq!(effects_about(&p, horse), vec![], "effect sent with no rider");
    assert!(p.s.player_start_riding(&mut h), "player not on the horse");
    assert_eq!(effects_about(&p, horse), vec![(jump, true)], "effects on mounting");
    assert!(p.s.give_mob_effect(&mut h, "minecraft:slow_falling", 600, 0), "slow falling refused");
    assert_eq!(effects_about(&p, horse), vec![(slow, true)], "effect added while riding");
    p.s.mobs.list.put(i, h);
    p.s.player_stop_riding();
    let mut taken_back = effects_about(&p, horse);
    taken_back.sort_unstable();
    let mut expected = vec![(jump, false), (slow, false)];
    expected.sort_unstable();
    assert_eq!(taken_back, expected, "effects on getting off");
}

/// Copper chests and copper golem statues of every stage are never walked through; a statue is
/// swum through only when it holds water. Fails on the code before (their non-full collision made
/// them walkable, the statue swimmable).
#[test]
fn copper_chests_and_statues_block_paths() {
    let Some(p) = world("copper-paths") else { return };
    for name in [
        "minecraft:copper_chest",
        "minecraft:weathered_copper_chest",
        "minecraft:waxed_oxidized_copper_chest",
        "minecraft:copper_golem_statue",
        "minecraft:waxed_exposed_copper_golem_statue",
    ] {
        let state = p.s.default_state(name);
        assert_ne!(state, 0, "{name} unknown");
        for table in [false, true] {
            assert!(!p.s.land_ok(state, table), "{name} walkable (table {table})");
        }
    }
    let statue = p.s.default_state("minecraft:oxidized_copper_golem_statue");
    let dry = p.s.view().set_named(statue, "waterlogged", "false");
    let wet = p.s.view().set_named(statue, "waterlogged", "true");
    for table in [false, true] {
        assert!(!p.s.water_ok(dry, table), "dry statue swimmable (table {table})");
        assert!(p.s.water_ok(wet, table), "waterlogged statue not swimmable (table {table})");
    }
}
