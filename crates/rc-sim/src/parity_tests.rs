//! Singleplayer tick rate on a new world (game data required): `tick freeze` stops the world
//! time, `tick step` advances it by that many ticks, `tick unfreeze` restarts it.

use crate::server::Server;

fn ticks(s: &mut Server, n: usize) {
    for _ in 0..n {
        s.game_tick();
        s.tick += 1;
    }
}

#[test]
fn freeze_stops_world_and_step_advances_it() {
    let Some((mut s, dir)) = crate::portal_tests::open_new_world("tickfreeze", 7) else { return };
    s.loaded = true;
    s.teleport = None;
    s.cmd.op = 4;
    let t0 = s.time;
    ticks(&mut s, 5);
    assert_eq!(s.time, t0 + 5, "world running");
    s.player_command("tick freeze");
    assert!(s.cmd.ticking.frozen());
    ticks(&mut s, 10);
    assert_eq!(s.time, t0 + 5, "frozen: world time no longer moves");
    s.player_command("tick step 3");
    ticks(&mut s, 10);
    assert_eq!(s.time, t0 + 8, "three steps requested, three world ticks");
    s.player_command("tick unfreeze");
    ticks(&mut s, 2);
    assert_eq!(s.time, t0 + 10);
    s.player_command("tick rate 5");
    assert_eq!(s.cmd.ticking.nanos_per_tick(), 200_000_000, "tick duration following the tick rate");
    s.player_command("tick sprint 20");
    assert!(s.cmd.ticking.sprinting(), "sprint recognised");
    let _ = std::fs::remove_dir_all(dir);
}

fn stack(id: &str, count: i32) -> crate::player::Stack {
    crate::player::Stack { id: id.into(), count, components: None }
}

/// A piglin surrounded by three stacks of gold nuggets takes them all in the same tick.
#[test]
fn piglin_picks_up_every_wanted_pile_in_one_tick() {
    let Some((mut s, dir)) = crate::portal_tests::open_new_world("pigpickup", 7) else { return };
    s.loaded = true;
    s.teleport = None;
    s.cmd.op = 4;
    s.load_now(&[s.chunk_pos()]);
    let [x, y, z] = s.pos;
    s.player_command(&format!("summon minecraft:piglin {} {y} {z} {{NoAI:1b}}", x + 3.0));
    let i = s
        .mobs
        .list
        .iter()
        .position(|m| m.as_ref().is_some_and(|m| m.kind == crate::mobs::species::Kind::Piglin))
        .expect("piglin summoned");
    let mut m = s.mobs.list[i].take().unwrap();
    m.can_pickup = true;
    let p = m.pos;
    for dx in [-0.5, 0.0, 0.5] {
        s.spawn_item([p[0] + dx, p[1] + 0.2, p[2]], [0.0; 3], stack("minecraft:gold_nugget", 10), 0);
    }
    s.piglin_pickup(&mut m);
    s.mobs.list[i] = Some(m);
    let left = s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == "minecraft:gold_nugget").count();
    assert_eq!(left, 0, "the three stacks picked up in the same tick");
    let _ = std::fs::remove_dir_all(dir);
}

/// Hopper with no container above: it sucks in the item placed on it at its tick; an item
/// falling into another hopper enters it right away, without waiting for its tick.
#[test]
fn hopper_takes_items_lying_on_or_falling_into_it() {
    let Some((mut s, dir)) = crate::portal_tests::open_new_world("hopperitems", 7) else { return };
    s.loaded = true;
    s.teleport = None;
    s.cmd.op = 4;
    s.load_now(&[s.chunk_pos(), ((s.pos[0] as i32 + 5) >> 4, s.pos[2].floor() as i32 >> 4)]);
    let [x, y, z] = [s.pos[0].floor() as i32, s.pos[1].floor() as i32 + 3, s.pos[2].floor() as i32];
    let a = [x + 2, y, z];
    let b = [x + 5, y, z];
    for p in [a, b] {
        s.player_command(&format!("setblock {} {} {} minecraft:hopper", p[0], p[1], p[2]));
        assert_eq!(s.bname(p), "minecraft:hopper");
    }
    let count = |s: &Server, p: [i32; 3]| s.be_item_counts(p).iter().map(|c| c.1).sum::<i32>();
    // (1) Item placed in the basin: taken by the hopper's tick.
    let at = [f64::from(a[0]) + 0.5, f64::from(a[1]) + 0.7, f64::from(a[2]) + 0.5];
    s.spawn_item(at, [0.0; 3], stack("minecraft:cobblestone", 5), 0);
    s.hopper_tick(a);
    assert_eq!(count(&s, a), 5, "stack sucked in whole");
    assert!(s.life.drops.iter().all(|d| d.is_removed() || d.stack.id != "minecraft:cobblestone"));
    // (2) Item dropped above the other: enters during the items' tick.
    let at = [f64::from(b[0]) + 0.5, f64::from(b[1]) + 1.6, f64::from(b[2]) + 0.5];
    s.spawn_item(at, [0.0; 3], stack("minecraft:dirt", 3), 0);
    for _ in 0..30 {
        s.drops_tick();
    }
    assert_eq!(count(&s, b), 3, "item fell into the hopper, taken on entry");
    let _ = std::fs::remove_dir_all(dir);
}
