//! The pickup of ground items by every mob allowed to pick up loot (end of the game's AI step:
//! equip the item when it beats what is worn), the fox's split of a picked-up stack, and the
//! lightning bolt a command summons. Fresh flat world: the 26.3 version data and the client jar
//! are required (absent: FAILURE, never a silent pass). Mobs are frozen (no AI): they stay put,
//! and the game still runs their pickup.

use rc_client::Event;
use rc_proto::nbt::Nbt;

use crate::blocks::probe::Probe;
use crate::commands::ents::E;
use crate::mobs::goals::Goal;
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;
use crate::mobs::wild::fox;
use crate::player::Stack;
use crate::tests_g3f13::{finish, open, reopen};

/// Feet height of a land mob on the flat world.
const FEET_Y: f64 = -60.0;
const MAIN_HAND: usize = 0;
const HEAD: usize = 5;

fn stack(id: &str, count: i32) -> Stack {
    Stack { id: id.into(), count, components: None }
}

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

/// A world at midnight (no undead burning in the sun) with the default `mob_griefing` rule.
fn night_world(tag: &str) -> (Probe, std::path::PathBuf) {
    let (mut p, dir) = open(tag);
    cmd(&mut p, "time set midnight");
    (p, dir)
}

/// A frozen mob of `kind` at `x` (no AI, so it stays put), allowed to pick up loot.
fn frozen_mob(p: &mut Probe, kind: Kind, x: f64, setup: impl FnOnce(&mut Mob)) -> i32 {
    let mut m = p.s.new_mob(kind, [x, FEET_Y, 0.5], 0.0);
    m.no_ai = true;
    m.can_pickup = true;
    setup(&mut m);
    let id = m.id;
    p.s.add_mob(m);
    id
}

/// Stack `st` laid in the pickup reach of the mob standing at `x`, ready to be taken.
fn lay_item(p: &mut Probe, x: f64, st: Stack) -> i32 {
    p.s.spawn_item([x + 0.4, FEET_Y, 0.5], [0.0; 3], st, 0);
    p.s.life.drops.last().expect("item laid").id
}

/// One complete tick of mob `id`, taken out of the list like the entity walk does.
fn tick(p: &mut Probe, id: i32) {
    let i = p.s.mob_index(id).expect("mob in the list");
    let mut m = p.s.mobs.list.take(i).expect("mob");
    p.s.tick_mob(&mut m);
    p.s.mobs.list.put(i, m);
}

fn mob(p: &Probe, id: i32) -> &Mob {
    p.s.mob_ref(id).expect("mob alive")
}

/// Item id held in `slot` of mob `id`.
fn held(p: &Probe, id: i32, slot: usize) -> Option<String> {
    mob(p, id).equipment[slot].as_ref().map(|s| s.id.clone())
}

/// Count left in ground item `item`, `None` once it is gone.
fn on_ground(p: &Probe, item: i32) -> Option<i32> {
    p.s.life.drops.iter().find(|d| d.id == item && !d.is_removed()).map(|d| d.stack.count)
}

/// The pickup packets sent: (item, collector, amount).
fn pickups(events: &[Event]) -> Vec<(i32, i32, i32)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Misc(rc_client::misc::Misc::Take { item, collector, amount }) => Some((*item, *collector, *amount)),
            _ => None,
        })
        .collect()
}

/// A zombie allowed to pick up loot takes the sword lying beside it: in its main hand, a sure
/// drop, persistent from now on, one pickup packet, the item gone. Before: the item stayed on the
/// ground (only a few species ever picked anything up).
#[test]
fn zombie_takes_a_sword_lying_beside_it() {
    let (mut p, dir) = night_world("loot-zombie-sword");
    let zombie = frozen_mob(&mut p, Kind::Zombie, 20.5, |_| {});
    assert!(!mob(&p, zombie).persistent, "a fresh zombie may despawn");
    let sword = lay_item(&mut p, 20.5, stack("minecraft:iron_sword", 1));
    p.s.spawn_item([24.5, FEET_Y, 0.5], [0.0; 3], stack("minecraft:diamond_sword", 1), 0); // out of reach
    let _ = p.take_events();
    tick(&mut p, zombie);
    assert_eq!(held(&p, zombie, MAIN_HAND).as_deref(), Some("minecraft:iron_sword"));
    let z = mob(&p, zombie);
    assert_eq!(z.drop_chance[MAIN_HAND], 2.0, "sure drop at death");
    assert!(z.persistent, "persistent once it picked something up");
    assert_eq!(on_ground(&p, sword), None, "the sword left the ground");
    assert_eq!(pickups(&p.take_events()), [(sword, zombie, 1)]);
    assert_eq!(p.s.life.drops.iter().filter(|d| !d.is_removed()).count(), 1, "the far sword stays");
    finish(p, dir);
}

/// The comparison of the game: better armor replaces the worn piece and the old one may fall
/// (drop chance 1 against a draw lowered by 0.1: always); worse armor goes to an empty main hand
/// (the game's fallback); a weaker weapon is left alone.
#[test]
fn armor_and_weapons_are_compared_like_the_game() {
    let (mut p, dir) = night_world("loot-compare");
    let better = frozen_mob(&mut p, Kind::Zombie, 10.5, |m| {
        m.equipment[HEAD] = Some(stack("minecraft:leather_helmet", 1));
        m.drop_chance[HEAD] = 1.0;
    });
    let worse = frozen_mob(&mut p, Kind::Zombie, 16.5, |m| m.equipment[HEAD] = Some(stack("minecraft:diamond_helmet", 1)));
    let weaker = frozen_mob(&mut p, Kind::Zombie, 22.5, |m| m.equipment[MAIN_HAND] = Some(stack("minecraft:iron_sword", 1)));
    let iron = lay_item(&mut p, 10.5, stack("minecraft:iron_helmet", 1));
    let leather = lay_item(&mut p, 16.5, stack("minecraft:leather_helmet", 1));
    let wooden = lay_item(&mut p, 22.5, stack("minecraft:wooden_sword", 1));
    let before = p.s.life.drops.len();
    for id in [better, worse, weaker] {
        tick(&mut p, id);
    }
    assert_eq!(held(&p, better, HEAD).as_deref(), Some("minecraft:iron_helmet"), "better helmet worn");
    let fell = &p.s.life.drops[before..];
    assert_eq!(fell.len(), 1, "one replaced piece fell");
    assert_eq!((fell[0].stack.id.as_str(), fell[0].pickup_delay), ("minecraft:leather_helmet", 10));
    assert_eq!(on_ground(&p, iron), None);
    assert_eq!(held(&p, worse, HEAD).as_deref(), Some("minecraft:diamond_helmet"), "diamond kept on the head");
    assert_eq!(held(&p, worse, MAIN_HAND).as_deref(), Some("minecraft:leather_helmet"), "worse helmet held in hand");
    assert_eq!(on_ground(&p, leather), None);
    assert_eq!(held(&p, weaker, MAIN_HAND).as_deref(), Some("minecraft:iron_sword"), "iron sword kept");
    assert_eq!(on_ground(&p, wooden), Some(1), "the wooden sword stays on the ground");
    finish(p, dir);
}

/// Common creatures pick up too when allowed (a cow given the right by its data wears a helmet);
/// nothing is taken without the right, nor under `mob_griefing false`.
#[test]
fn any_mob_allowed_to_pick_up_does_it_and_only_then() {
    let (mut p, dir) = night_world("loot-common");
    let cow = frozen_mob(&mut p, Kind::Cow, 10.5, |_| {});
    let unwilling = frozen_mob(&mut p, Kind::Zombie, 16.5, |m| m.can_pickup = false);
    lay_item(&mut p, 10.5, stack("minecraft:golden_helmet", 1));
    let left = lay_item(&mut p, 16.5, stack("minecraft:golden_helmet", 1));
    tick(&mut p, cow);
    tick(&mut p, unwilling);
    assert_eq!(held(&p, cow, HEAD).as_deref(), Some("minecraft:golden_helmet"), "the cow wears it");
    assert_eq!(on_ground(&p, left), Some(1), "no right to pick up: nothing taken");
    cmd(&mut p, "gamerule minecraft:mob_griefing false");
    let zombie = frozen_mob(&mut p, Kind::Zombie, 22.5, |_| {});
    let kept = lay_item(&mut p, 22.5, stack("minecraft:golden_helmet", 1));
    tick(&mut p, zombie);
    assert_eq!(on_ground(&p, kept), Some(1), "mob_griefing false: nothing taken");
    finish(p, dir);
}

/// Species, item laid beside it, whether it is taken, setup of the mob.
type RefusalCase = (Kind, &'static str, bool, fn(&mut Mob));

/// Each species' refusals: a zombie leaves glow ink sacs (the zombified piglin takes them), a
/// skeleton and a drowned leave spears, a drowned never gives up its nautilus shell, a wither
/// skeleton leaves bows, a baby zombie riding something holds no egg.
#[test]
fn species_refuse_what_the_game_makes_them_refuse() {
    let (mut p, dir) = night_world("loot-refusals");
    assert!(p.s.item_tag("minecraft:iron_spear", "minecraft:spears"), "spear tag missing");
    let cases: [RefusalCase; 7] = [
        (Kind::Zombie, "minecraft:glow_ink_sac", false, |_| {}),
        (Kind::Zombie, "minecraft:ink_sac", true, |_| {}),
        (Kind::ZombifiedPiglin, "minecraft:glow_ink_sac", true, |_| {}),
        (Kind::Skeleton, "minecraft:iron_spear", false, |m| m.equipment[MAIN_HAND] = None),
        (Kind::Drowned, "minecraft:iron_spear", false, |_| {}),
        (Kind::Drowned, "minecraft:trident", false, |m| {
            m.equipment[MAIN_HAND] = Some(Stack { id: "minecraft:nautilus_shell".into(), count: 1, components: None });
        }),
        (Kind::WitherSkeleton, "minecraft:bow", false, |m| m.equipment[MAIN_HAND] = None),
    ];
    for (k, (kind, item, taken, setup)) in cases.into_iter().enumerate() {
        let x = 8.5 + 6.0 * k as f64;
        let id = frozen_mob(&mut p, kind, x, setup);
        let laid = lay_item(&mut p, x, stack(item, 1));
        tick(&mut p, id);
        assert_eq!(on_ground(&p, laid).is_none(), taken, "{kind:?} and {item}: taken {taken} expected");
    }
    // The rider is read without a tick (a passenger is ticked by its vehicle).
    let mut rider = p.s.new_mob(Kind::Zombie, [60.5, FEET_Y, 0.5], 0.0);
    rider.baby = true;
    let egg = stack("minecraft:egg", 1);
    assert!(p.s.species_may_hold(&rider, &egg), "a baby zombie on foot holds an egg");
    rider.mount.vehicle = Some(i32::MAX);
    assert!(!p.s.species_may_hold(&rider, &egg), "a riding baby zombie holds no egg");
    finish(p, dir);
}

/// A skeleton that picks up a bow switches to its bow goal (any equipment change makes it choose
/// again, like the game); one holding its preferred bow ignores a sword.
#[test]
fn skeleton_with_a_new_bow_takes_up_the_bow_goal() {
    let (mut p, dir) = night_world("loot-skeleton");
    let unarmed = frozen_mob(&mut p, Kind::Skeleton, 10.5, |m| m.equipment[MAIN_HAND] = None);
    let archer = frozen_mob(&mut p, Kind::Skeleton, 16.5, |m| m.equipment[MAIN_HAND] = Some(stack("minecraft:bow", 1)));
    for id in [unarmed, archer] {
        let i = p.s.mob_index(id).expect("skeleton");
        let mut m = p.s.mobs.list.take(i).expect("skeleton");
        crate::mobs::brains::reassess_weapon(&p.s, &mut m);
        p.s.mobs.list.put(i, m);
    }
    let bow_goal = |p: &Probe, id| mob(p, id).goals.goals.iter().any(|w| matches!(w.goal, Goal::Bow { .. }));
    assert!(!bow_goal(&p, unarmed), "unarmed skeleton: melee goal");
    lay_item(&mut p, 10.5, stack("minecraft:bow", 1));
    let sword = lay_item(&mut p, 16.5, stack("minecraft:diamond_sword", 1));
    tick(&mut p, unarmed);
    tick(&mut p, archer);
    assert_eq!(held(&p, unarmed, MAIN_HAND).as_deref(), Some("minecraft:bow"));
    assert!(bow_goal(&p, unarmed), "bow goal after picking up a bow");
    assert_eq!(on_ground(&p, sword), Some(1), "a bow beats any sword for a skeleton");
    finish(p, dir);
}

/// A skeleton drawing its bow at a target, then given an item: the running weapon goal is stopped
/// like the game's goal removal (bow let go, no longer aggressive), and every control lock points
/// at a running goal that holds that flag (the removal shifted the list). Then it takes the bow goal
/// up again and draws.
#[test]
fn equipment_change_stops_the_drawn_bow_and_recomputes_the_locks() {
    use crate::mobs::goals::{JUMP, LOOK, MOVE, TARGET};
    let (mut p, dir) = night_world("loot-skeleton-running");
    let target = frozen_mob(&mut p, Kind::Zombie, 16.5, |m| m.can_pickup = false);
    let archer = frozen_mob(&mut p, Kind::Skeleton, 10.5, |m| {
        m.no_ai = false;
        m.can_pickup = false;
        m.equipment[MAIN_HAND] = Some(stack("minecraft:bow", 1));
        m.target = Some(crate::mobs::mob::Ent::Mob(target));
    });
    let i = p.s.mob_index(archer).expect("skeleton");
    let mut m = p.s.mobs.list.take(i).expect("skeleton");
    crate::mobs::brains::reassess_weapon(&p.s, &mut m);
    let bow_running = |m: &Mob| m.goals.running(|g| matches!(g, Goal::Bow { .. }));
    let locks_hold = |m: &Mob| {
        m.goals.locks().iter().zip([MOVE, LOOK, JUMP, TARGET]).all(|(lock, flag)| {
            lock.is_none_or(|k| m.goals.goals.get(k).is_some_and(|w| w.running && w.goal.flags() & flag != 0))
        })
    };
    p.s.selector_tick(&mut m, false);
    assert!(bow_running(&m) && m.aggressive && m.use_ticks >= 0, "the skeleton draws its bow at the target");
    let bow_index = m.goals.goals.iter().position(|w| matches!(w.goal, Goal::Bow { .. }));
    assert_eq!(m.goals.locks()[0], bow_index, "the bow goal holds the move lock");
    p.s.mob_set_item_slot(&mut m, MAIN_HAND, Some(stack("minecraft:bow", 1)));
    assert!(!bow_running(&m), "weapon goal rebuilt, not running");
    assert_eq!((m.use_ticks, m.aggressive), (-1, false), "the running bow goal was stopped");
    assert!(locks_hold(&m), "a lock points at a goal that does not hold it: {:?}", m.goals.locks());
    p.s.selector_tick(&mut m, false);
    assert!(bow_running(&m) && m.use_ticks >= 0, "the bow goal starts again");
    assert!(locks_hold(&m), "locks after the restart: {:?}", m.goals.locks());
    p.s.mobs.list.put(i, m);
    finish(p, dir);
}

/// Mobs standing by items their species wants but their comparison refuses (a zombie holding an
/// iron sword by rotten flesh; one with a diamond helmet and a full hand by a leather helmet) take
/// nothing and allocate nothing at their pickup once warm: the stacks are read in place. Before:
/// each item was looked up again by id and its stack copied, at every tick.
#[test]
fn items_refused_by_the_comparison_cost_no_allocation() {
    let (mut p, dir) = night_world("loot-refused-allocations");
    let swordsman = frozen_mob(&mut p, Kind::Zombie, 10.5, |m| m.equipment[MAIN_HAND] = Some(stack("minecraft:iron_sword", 1)));
    let helmeted = frozen_mob(&mut p, Kind::Zombie, 16.5, |m| {
        m.equipment[HEAD] = Some(stack("minecraft:diamond_helmet", 1));
        m.equipment[MAIN_HAND] = Some(stack("minecraft:iron_sword", 1));
    });
    let flesh = lay_item(&mut p, 10.5, stack("minecraft:rotten_flesh", 3));
    let leather = lay_item(&mut p, 16.5, stack("minecraft:leather_helmet", 1));
    let pick_up = |p: &mut Probe| {
        for id in [swordsman, helmeted] {
            let i = p.s.mob_index(id).expect("mob");
            let mut m = p.s.mobs.list.take(i).expect("mob");
            p.s.mob_pick_up_loot(&mut m);
            p.s.mobs.list.put(i, m);
        }
    };
    pick_up(&mut p); // lazily built tables and switches
    let counter_check = crate::test_alloc::allocations();
    drop(std::hint::black_box(Box::new(0u64)));
    assert!(crate::test_alloc::allocations() > counter_check, "the test allocator does not count (memstats feature?)");
    let before = crate::test_alloc::allocations();
    for _ in 0..10 {
        pick_up(&mut p);
    }
    let allocations = crate::test_alloc::allocations() - before;
    assert_eq!((on_ground(&p, flesh), on_ground(&p, leather)), (Some(3), Some(1)), "both refused");
    assert_eq!(held(&p, helmeted, HEAD).as_deref(), Some("minecraft:diamond_helmet"));
    assert_eq!(allocations, 0, "allocations of 10 pickups of two mobs by refused items");
    finish(p, dir);
}

/// Attribute modifiers the stack itself carries count, applied in their order like the game's
/// estimate (a total multiplier applies to the value reached so far). Before: the piglin's
/// comparison read only the item's default modifiers.
#[test]
fn stack_attribute_modifiers_count_in_the_comparison() {
    let (mut p, dir) = night_world("loot-modifiers");
    let modifier = |op: &str, amount: f64, n: usize| {
        Nbt::Compound(vec![
            ("type".into(), Nbt::String("minecraft:attack_damage".into())),
            ("id".into(), Nbt::String(format!("rustcraft:test_{n}"))),
            ("amount".into(), Nbt::Double(amount)),
            ("operation".into(), Nbt::String(op.into())),
            ("slot".into(), Nbt::String("mainhand".into())),
        ])
    };
    let stick = |mods: Vec<Nbt>| Stack {
        id: "minecraft:stick".into(),
        count: 1,
        components: Some(Nbt::Compound(vec![("minecraft:attribute_modifiers".into(), Nbt::List(mods))])),
    };
    // Baby piglin (no preferred weapon): a stick of +20 damage beats a wooden sword.
    let piglin = frozen_mob(&mut p, Kind::Piglin, 10.5, |m| {
        m.baby = true;
        m.equipment[MAIN_HAND] = Some(stack("minecraft:wooden_sword", 1));
    });
    let strong = stick(vec![modifier("add_value", 20.0, 0)]);
    assert!(crate::mobs::piglins::items::wants(&p.s, mob(&p, piglin), None, &strong), "piglin: +20 stick wanted");
    // Zombie (attack base 3): ×2 on the total then +4 = 10, below +8.5 = 11.5; summed in another
    // order it would be (3 + 4) × 2 = 14.
    let zombie = frozen_mob(&mut p, Kind::Zombie, 16.5, |m| {
        m.equipment[MAIN_HAND] = Some(stick(vec![modifier("add_multiplied_total", 1.0, 1), modifier("add_value", 4.0, 2)]));
    });
    assert_eq!(mob(&p, zombie).attrs.attack.base, 3.0, "zombie attack base");
    let laid = lay_item(&mut p, 16.5, stick(vec![modifier("add_value", 8.5, 3)]));
    tick(&mut p, zombie);
    assert_eq!(on_ground(&p, laid), None, "the +8.5 stick replaces the ×2 then +4 one");
    finish(p, dir);
}

/// The fox takes one piece of a stack of ten: one in its mouth, the nine others leave in a new
/// item at its feet (random push, no delay), the item on the ground is gone and the pickup packet
/// says 0, what was left in it. Ten items before, ten after. Before: a still item of nine appeared
/// and the original kept nine: nineteen.
#[test]
fn fox_splits_a_stack_without_duplicating_it() {
    let (mut p, dir) = night_world("loot-fox");
    let fox_id = frozen_mob(&mut p, Kind::Fox, 20.5, |_| {});
    let wheat = lay_item(&mut p, 20.5, stack("minecraft:wheat", 10));
    let before = p.s.life.drops.len();
    let _ = p.take_events();
    let i = p.s.mob_index(fox_id).expect("fox");
    let mut m = p.s.mobs.list.take(i).expect("fox");
    fox::pickup(&mut p.s, &mut m);
    p.s.mobs.list.put(i, m);
    assert_eq!(mob(&p, fox_id).equipment[MAIN_HAND].as_ref().map(|s| (s.id.as_str(), s.count)), Some(("minecraft:wheat", 1)));
    assert_eq!(on_ground(&p, wheat), None, "the original item is gone");
    let rest = &p.s.life.drops[before..];
    assert_eq!(rest.len(), 1, "one new item");
    assert_eq!((rest[0].stack.count, rest[0].pickup_delay, rest[0].vel[1]), (9, 0, 0.2), "rest: 9, no delay, pushed up");
    let total: i32 = p.s.life.drops.iter().filter(|d| !d.is_removed()).map(|d| d.stack.count).sum::<i32>() + 1;
    assert_eq!(total, 10, "wheat neither created nor lost");
    assert_eq!(pickups(&p.take_events()), [(wheat, fox_id, 0)], "pickup packet with what was left");
    finish(p, dir);
}

/// `/summon minecraft:lightning_bolt` strikes for real (a creeper beside it is charged), then
/// vanishes; it is never kept as a frozen compound, so never saved nor announced again. The
/// command names it. Before: an inert entity, kept and saved forever.
#[test]
fn summoned_lightning_strikes_and_vanishes() {
    let (mut p, dir) = night_world("loot-lightning");
    let creeper = frozen_mob(&mut p, Kind::Creeper, 40.5, |m| m.persistent = true);
    let src = p.s.server_source();
    let bolt = p.s.summon_entity(&src, "minecraft:lightning_bolt", [40.5, FEET_Y, 0.5], None).expect("summoned");
    assert_eq!(p.s.ent_type(bolt), "minecraft:lightning_bolt", "the summon message names it");
    let E::Other(id) = bolt else { panic!("not an entity of its own: {bolt:?}") };
    assert!(p.s.live_bolt(id).is_some(), "a simulated bolt");
    let kept_bolt = |p: &Probe| {
        p.s.entities.values().flatten().any(|(_, n)| crate::mobs::mounts::trap::is_lightning_bolt(n))
    };
    assert!(!kept_bolt(&p), "kept as a frozen compound");
    for _ in 0..40 {
        p.s.game_tick();
        p.s.tick += 1;
    }
    assert!(mob(&p, creeper).powered, "the creeper was struck");
    assert!(p.s.live_bolt(id).is_none(), "the bolt vanished");
    assert!(!kept_bolt(&p), "never kept");
    finish(p, dir);
}

/// Micro-bench (quiet machine; `--ignored --nocapture`): the common pickup of 100 mobs allowed to
/// pick up, per tick, with no item in the world, with 500 items out of everyone's reach, with 500
/// items of which a hundred lie in the mobs' reach but are refused by the species (glow ink sacs),
/// and with a hundred in reach wanted but refused by the comparison (rotten flesh by zombies holding
/// an iron sword). The count of reach tests (mobs × items listed) and of allocations per tick are
/// printed too (deterministic).
#[test]
#[ignore]
fn bench_common_pickup_scan() {
    let (mut p, dir) = night_world("loot-bench");
    let mobs: Vec<i32> = (0..100).map(|k| frozen_mob(&mut p, Kind::Zombie, 2.5 + 1.5 * f64::from(k), |_| {})).collect();
    let measure = |p: &mut Probe, label: &str| {
        let rounds = 200u32;
        let mut best = f64::MAX;
        let mut allocations = 0;
        for _ in 0..9 {
            let (t, before) = (std::time::Instant::now(), crate::test_alloc::allocations());
            for _ in 0..rounds {
                for &id in &mobs {
                    let i = p.s.mob_index(id).expect("mob");
                    let mut m = p.s.mobs.list.take(i).expect("mob");
                    p.s.mob_pick_up_loot(&mut m);
                    p.s.mobs.list.put(i, m);
                }
            }
            best = best.min(t.elapsed().as_secs_f64() * 1e6 / f64::from(rounds));
            allocations = (crate::test_alloc::allocations() - before) / u64::from(rounds);
        }
        let tests = mobs.len() * p.s.life.drops.len();
        println!("{label}: best {best:.1} µs per tick for {} mobs, {tests} reach tests, {allocations} allocations per tick", mobs.len());
    };
    measure(&mut p, "no item");
    for k in 0..500 {
        p.s.spawn_item([2.5 + 0.3 * f64::from(k), FEET_Y, 20.5], [0.0; 3], stack("minecraft:stone", 1), 0);
    }
    measure(&mut p, "500 items out of reach");
    p.s.life.drops.clear();
    for k in 0..500 {
        let x = 2.5 + 1.5 * f64::from(k % 100);
        let z = if k < 100 { 0.5 } else { 20.5 };
        p.s.spawn_item([x + 0.4, FEET_Y, z], [0.0; 3], stack("minecraft:glow_ink_sac", 1), 0);
    }
    measure(&mut p, "500 items, 100 refused ones in reach");
    for &id in &mobs {
        p.s.mobs.get_mut(id).expect("mob").equipment[MAIN_HAND] = Some(stack("minecraft:iron_sword", 1));
    }
    for d in p.s.life.drops.iter_mut().take(100) {
        d.stack = stack("minecraft:rotten_flesh", 1);
    }
    measure(&mut p, "500 items, 100 in reach refused by the comparison");
    finish(p, dir);
}

/// A lightning bolt an older Rustcraft wrote in an entity file is left out when the chunk is
/// read, and the chunk is written again without it.
#[test]
fn saved_lightning_bolt_is_ignored_when_read() {
    let (mut p, dir) = night_world("loot-lightning-file");
    let old_bolt = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:lightning_bolt".into())),
        ("Pos".into(), Nbt::List(vec![Nbt::Double(8.5), Nbt::Double(FEET_Y), Nbt::Double(8.5)])),
    ]);
    p.s.entities.entry((0, 0)).or_default().push((i32::MAX - 1, old_bolt));
    p.s.life.drop_chunks.insert((0, 0));
    p.s.save(true);
    drop(p);
    let data = std::sync::Arc::new(rc_testdata::version_data().expect("26.3 version data missing"));
    let mut p = reopen(&dir, data);
    let bolts = |p: &Probe| {
        p.s.entities.values().flatten().filter(|(_, n)| crate::mobs::mounts::trap::is_lightning_bolt(n)).count()
    };
    assert_eq!(bolts(&p), 0, "bolt read back from the file");
    assert!(p.s.life.drop_chunks.contains(&(0, 0)), "chunk to write again");
    let mut jobs = Vec::new();
    p.s.drops_save_jobs(&mut jobs);
    let written = jobs.iter().any(|j| match j {
        crate::save::Job::Chunk { cx: 0, cz: 0, nbt, .. } => !nbt.windows(24).any(|w| w == b"minecraft:lightning_bolt"),
        _ => false,
    });
    assert!(written, "chunk written again without the bolt");
    finish(p, dir);
}
