//! Targeting rules of the game: the wolf's prey class, the creeper that refuses a goat, allies
//! never targeted (teams, a tamed animal's owner), a chase that lets go of a target no longer
//! attackable, and the prey chase that keeps testing sight and filter. World tests run on a copy
//! of the test world (`RUSTCRAFT_SOLO_WORLD`, area 200..231); without it they are skipped.

use super::goals::{Goal, TBase, Who, who_fits, who_species};
use super::jrand::JRand;
use super::mob::{Ent, Mob};
use super::species::Kind;
use super::tests_spawn_paths::open;
use crate::blocks::probe::Probe;

fn spawn(p: &mut Probe, kind: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(kind, pos, 0.0);
    let mut group = super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::spawn::Reason::SpawnEgg, &mut group);
    m.set_pos(pos);
    let id = m.id;
    p.s.add_mob(m);
    id
}

/// The mob taken out of the list, as during its own tick.
fn take(p: &mut Probe, id: i32) -> (usize, Box<Mob>) {
    let i = p.s.mob_index(id).expect("mob");
    (i, p.s.mobs.list.take(i).expect("mob"))
}

fn with(p: &mut Probe, id: i32, f: impl FnOnce(&mut Mob)) {
    let i = p.s.mob_index(id).expect("mob");
    f(p.s.mobs.list.get_mut(i).expect("mob"));
}

fn mob(p: &Probe, id: i32) -> &Mob {
    p.s.mob_ref(id).expect("mob")
}

fn command(p: &mut Probe, text: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, text);
    assert!(ok, "command refused: {text}");
}

/// Survival player at the platform's edge, the test world's normal difficulty.
fn world(tag: &str) -> Option<Probe> {
    let (mut p, _dir) = open(tag)?;
    p.s.pos = [216.5, -59.0, 202.5];
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    Some(p)
}

/// The fox is wolf prey, like the sheep and the rabbit; the per-species index serves it too.
/// Fails on the code before (the class held sheep and rabbit only).
#[test]
fn the_fox_is_wolf_prey() {
    let fox = Mob::new(Kind::Fox, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    assert!(who_fits(Who::WolfPrey, &fox), "fox refused as wolf prey");
    let species = who_species(Who::WolfPrey).expect("closed class");
    assert!(species.contains(&Kind::Fox), "fox missing from the index species {species:?}");
}

/// A goat hits a creeper: the creeper's retaliation refuses the goat and keeps the target it had;
/// a zombie hit the same way takes the goat. Fails on the code before (any attacker was taken).
#[test]
fn a_creeper_never_takes_a_goat_as_target() {
    let Some(mut p) = world("creeper-refuses-goat") else { return };
    let goat = spawn(&mut p, Kind::Goat, [210.5, -59.0, 210.5]);
    for (kind, takes_goat) in [(Kind::Creeper, false), (Kind::Zombie, true)] {
        let id = spawn(&mut p, kind, [214.5, -59.0, 214.5]);
        let (i, mut m) = take(&mut p, id);
        m.last_hurt_by_mob = Some((Ent::Mob(goat), 1));
        let mut retaliation = Goal::HurtBy { base: TBase::new(true, false), alert: false, stamp: 0 };
        p.s.goal_start(&mut retaliation, &mut m);
        let expected = takes_goat.then_some(Ent::Mob(goat));
        assert_eq!(m.target, expected, "{kind:?}: target after a goat's hit");
        p.s.mobs.list.put(i, m);
    }
}

/// A tamed wolf hit by its owner never takes them as a target, a wild one does. Fails on the
/// code before (the owner was a target like any player).
#[test]
fn a_tamed_wolf_never_targets_its_owner() {
    let Some(mut p) = world("wolf-spares-owner") else { return };
    let wolf = spawn(&mut p, Kind::Wolf, [216.5, -59.0, 206.5]);
    let owner = p.s.uuid;
    let (i, mut w) = take(&mut p, wolf);
    assert!(p.s.targeting_ok(&mut w, Ent::Player, true, -1.0, false, false), "wild wolf refuses the player");
    w.tame = true;
    w.owner = Some(owner);
    assert!(!p.s.targeting_ok(&mut w, Ent::Player, true, -1.0, false, false), "tamed wolf targets its owner");
    p.s.mobs.list.put(i, w);
}

/// A zombie on the player's scoreboard team does not choose them as a target, and one that
/// already chases them lets go once both are on the same team. Fails on the code before (teams
/// were ignored by targeting).
#[test]
fn a_zombie_on_the_players_team_spares_them() {
    let Some(mut p) = world("team-ally-zombie") else { return };
    let zombie = spawn(&mut p, Kind::Zombie, [216.5, -59.0, 208.5]);
    let (i, mut z) = take(&mut p, zombie);
    assert!(p.s.targeting_ok(&mut z, Ent::Player, true, 35.0, false, true), "zombie refuses the player");
    z.target = Some(Ent::Player);
    let mut chase = TBase::new(false, false);
    assert!(p.s.target_continue(&mut chase, &mut z), "chase stopped before any team");
    p.s.mobs.list.put(i, z);
    command(&mut p, "team add allies");
    command(&mut p, "team join allies S3");
    command(&mut p, "team join allies @e[type=minecraft:zombie]");
    let (i, mut z) = take(&mut p, zombie);
    assert!(!p.s.targeting_ok(&mut z, Ent::Player, true, 35.0, false, true), "team-mate chosen as a target");
    z.target = Some(Ent::Player);
    assert!(!p.s.target_continue(&mut chase, &mut z), "team-mate still chased");
    p.s.mobs.list.put(i, z);
}

/// A chase lets go of a mob target that became invulnerable (no longer seen as an enemy). Fails
/// on the code before (the chase only tested that the target was alive).
#[test]
fn a_chase_lets_go_of_an_invulnerable_target() {
    let Some(mut p) = world("chase-invulnerable") else { return };
    let skeleton = spawn(&mut p, Kind::Skeleton, [212.5, -59.0, 212.5]);
    let golem = spawn(&mut p, Kind::IronGolem, [214.5, -59.0, 212.5]);
    let (i, mut g) = take(&mut p, golem);
    g.target = Some(Ent::Mob(skeleton));
    let mut chase = TBase::new(false, false);
    assert!(p.s.target_continue(&mut chase, &mut g), "chase of a plain skeleton stopped");
    p.s.mobs.list.put(i, g);
    with(&mut p, skeleton, |s| s.invulnerable = true);
    let (i, mut g) = take(&mut p, golem);
    g.target = Some(Ent::Mob(skeleton));
    assert!(!p.s.target_continue(&mut chase, &mut g), "invulnerable target still chased");
    p.s.mobs.list.put(i, g);
}

/// A zombie hit by the player calls the zombies around, except the one on the player's team.
/// Fails on the code before (allies of the aggressor were called too).
#[test]
fn a_call_for_help_skips_the_aggressors_allies() {
    let Some(mut p) = world("alert-skips-ally") else { return };
    let caller = spawn(&mut p, Kind::Zombie, [216.5, -59.0, 212.5]);
    let ally = spawn(&mut p, Kind::Zombie, [218.5, -59.0, 212.5]);
    let other = spawn(&mut p, Kind::Zombie, [214.5, -59.0, 212.5]);
    command(&mut p, "team add allies");
    command(&mut p, "team join allies S3");
    let ally_uuid = crate::nbtio::uuid_text(mob(&p, ally).uuid);
    command(&mut p, &format!("team join allies {ally_uuid}"));
    let (i, mut c) = take(&mut p, caller);
    c.last_hurt_by_mob = Some((Ent::Player, c.tick_count));
    p.s.alert_others_with(&c, true);
    p.s.mobs.list.put(i, c);
    assert_eq!(mob(&p, other).target, Some(Ent::Player), "plain zombie not called");
    assert_eq!(mob(&p, ally).target, None, "the aggressor's ally was called");
}

/// A wild wolf chasing a sheep gives up once a wall hides it, like the game's chase that repeats
/// the whole choice test (sight included). Fails on the code before (sight was not required).
#[test]
fn a_prey_chase_ends_when_the_prey_is_out_of_sight() {
    let Some(mut p) = world("prey-out-of-sight") else { return };
    let wolf = spawn(&mut p, Kind::Wolf, [216.5, -59.0, 210.5]);
    let sheep = spawn(&mut p, Kind::Sheep, [216.5, -59.0, 216.5]);
    let mut chase = Goal::Nearest {
        base: TBase::new(false, false),
        who: Who::WolfPrey,
        interval: 1,
        target: Some(Ent::Mob(sheep)),
        angry_only: false,
        non_tame: true,
    };
    let (i, mut w) = take(&mut p, wolf);
    assert!(p.s.goal_can_continue(&mut chase, &mut w), "chase in plain sight stopped");
    p.s.mobs.list.put(i, w);
    p.run("fill 213 -59 213 219 -56 213 minecraft:stone\n");
    let (i, mut w) = take(&mut p, wolf);
    w.sight.clear();
    assert!(!p.s.goal_can_continue(&mut chase, &mut w), "chase of a hidden sheep goes on");
    p.s.mobs.list.put(i, w);
}

/// A stray cat chasing a baby turtle on land lets go once the turtle is in the water: the prey
/// filter is read again. Fails on the code before (the filter was only read at the choice).
#[test]
fn a_prey_chase_ends_when_the_prey_leaves_the_filter() {
    let Some(mut p) = world("prey-leaves-filter") else { return };
    let cat = spawn(&mut p, Kind::Cat, [216.5, -59.0, 212.5]);
    let turtle = spawn(&mut p, Kind::Turtle, [216.5, -59.0, 215.5]);
    with(&mut p, turtle, |t| t.age = -24000);
    let mut chase = Goal::Nearest {
        base: TBase::new(false, false),
        who: Who::BabyTurtleOnLand,
        interval: 1,
        target: Some(Ent::Mob(turtle)),
        angry_only: false,
        non_tame: true,
    };
    let (i, mut c) = take(&mut p, cat);
    assert!(p.s.goal_can_continue(&mut chase, &mut c), "chase of a turtle on land stopped");
    p.s.mobs.list.put(i, c);
    with(&mut p, turtle, |t| t.in_water = true);
    let (i, mut c) = take(&mut p, cat);
    assert!(!p.s.goal_can_continue(&mut chase, &mut c), "chase of a turtle in the water goes on");
    p.s.mobs.list.put(i, c);
}
