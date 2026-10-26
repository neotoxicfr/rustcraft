//! Mount goals in the game's insertion order (goal and target selectors). The
//! camel and the camel husk have no goals: their brain is in `camel.rs`.

use super::super::goals::mount_goals::MGoal;
use super::super::goals::{Food, Goal, TBase, Who};
use super::super::mob::Mob;
use super::super::mth::reduced;
use super::super::species::Kind;
use super::super::surface::{breed_goal, follow_parent, look_around, look_player, nearest, panic, stroll, tempt};
use crate::server::Server;

fn hurt_by() -> Goal {
    Goal::HurtBy { base: TBase::new(true, false), alert: false, stamp: 0 }
}

fn run_around() -> Goal {
    Goal::Mount(MGoal::RunAround { speed: 1.2, pos: [0.0; 3] })
}

/// Goals of each species of the family.
pub fn register(m: &mut Mob) {
    match m.kind {
        Kind::Camel | Kind::CamelHusk => {}
        Kind::Llama | Kind::TraderLlama => register_llama(m),
        _ => register_horse(m),
    }
}

/// Registration of a mount's goals then the addition of the behavior goals (species-specific).
fn register_horse(m: &mut Mob) {
    let g = &mut m.goals;
    g.add(1, run_around());
    g.add(2, breed_goal(1.0));
    g.add(4, follow_parent(1.0));
    g.add(6, stroll(0.7));
    g.add(7, look_player(6.0));
    g.add(8, look_around());
    if super::can_rear(m.kind) {
        g.add(9, Goal::Mount(MGoal::RandomStand { next: -m.kind.info().ambient_interval }));
    }
    match m.kind {
        Kind::SkeletonHorse => {}
        Kind::ZombieHorse => {
            g.add(0, Goal::Float);
            g.add(3, tempt(1.25, "minecraft:zombie_horse_food", false));
        }
        _ => {
            g.add(0, Goal::Float);
            // Mount panic: never controlled by a mob for these species.
            g.add(1, panic(1.2, "minecraft:panic_causes"));
            g.add(3, tempt(1.25, "minecraft:horse_tempt_items", false));
        }
    }
    // Skeleton horse read back as a trap: goal added after the others.
    if m.kind == Kind::SkeletonHorse && m.mount.trap {
        Server::add_trap_goal(m);
    }
}

/// Registration of the llama's goals (without the equines' common goals), then the trader's llama.
fn register_llama(m: &mut Mob) {
    let g = &mut m.goals;
    g.add(0, Goal::Float);
    g.add(1, run_around());
    g.add(2, Goal::Mount(MGoal::Caravan { speed: 2.1, dist_check: 0 }));
    g.add(3, Goal::Mount(MGoal::Spit { speed: 1.25, target: None, attack_time: -1, see: 0 }));
    g.add(3, panic(1.2, "minecraft:panic_causes"));
    g.add(4, breed_goal(1.0));
    g.add(
        5,
        Goal::Tempt {
            speed: 1.25,
            food: Food::Tag("minecraft:llama_tempt_items"),
            scare: false,
            stop: 2.5,
            calm: 0,
            p: [0.0; 3],
            prot: [0.0; 2],
            on: false,
        },
    );
    g.add(6, follow_parent(1.0));
    g.add(7, stroll(0.7));
    g.add(8, look_player(6.0));
    g.add(9, look_around());
    let t = &mut m.targets;
    t.add(1, Goal::Mount(MGoal::LlamaHurtBy(Box::new(hurt_by()))));
    t.add(
        2,
        Goal::Mount(MGoal::WolfTarget(Box::new(Goal::Nearest {
            base: TBase::new(false, true),
            who: Who::WildWolf,
            interval: reduced(16),
            target: None,
            angry_only: false,
            non_tame: false,
        }))),
    );
    if m.kind == Kind::TraderLlama {
        m.goals.add(1, panic(2.0, "minecraft:panic_causes"));
        // Defense of the wandering trader (leash not simulated: never usable); zombies,
        // illagers (none simulated).
        m.targets.add(2, nearest(Who::Zombie, true, 10));
        m.targets.add(2, nearest(Who::Illager, true, 10));
    }
}
