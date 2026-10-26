//! Systems called per tick (pattern "a system called twice per tick, or zero times"):
//! `#minecraft:tick`/`#minecraft:load` functions, functions scheduled before the entities,
//! command block minecart once per world tick, night skip decided over all the
//! players, player-specific systems run for the LAN guests; projectiles
//! that stay findable during another's tick (End crystal and shulker bullet targetable,
//! bullet touched by the explosion of a fireball).
//! Test world required (`RUSTCRAFT_SOLO_WORLD`).

use crate::player::Stack;
use crate::projectiles::{PKind, Proj};
use crate::server::Server;

fn new_proj(s: &mut Server, kind: PKind, pos: [f64; 3], vel: [f64; 3]) -> i32 {
    s.next_entity += 1;
    let id = s.next_entity;
    let mut p = Proj::new(kind, id, pos, Stack { id: "minecraft:arrow".into(), count: 1, components: None });
    p.vel = vel;
    p.has_been_shot = true;
    s.proj_add(p);
    id
}

fn proj_alive(s: &Server, id: i32) -> bool {
    s.proj.list.iter().any(|p| p.id == id && !p.removed)
}

#[test]
fn projectiles_hit_crystal_and_bullet() {
    let Some(mut s) = open("proj-other", &[]) else { return };
    let o = s.pos;
    // Arrow shot at an End crystal: it explodes.
    let c = s.spawn_crystal([o[0] + 6.5, o[1] + 4.0, o[2]], 0.0, false);
    new_proj(&mut s, PKind::Arrow, [o[0] + 3.0, o[1] + 5.0, o[2]], [1.5, 0.0, 0.0]);
    // Snowball thrown at a shulker bullet: it bursts.
    let b = new_proj(&mut s, PKind::ShulkerBullet, [o[0] - 6.0, o[1] + 5.0, o[2] + 4.0], [0.0; 3]);
    new_proj(&mut s, PKind::Snowball, [o[0] - 3.0, o[1] + 5.0, o[2] + 4.0], [-1.5, 0.0, 0.0]);
    for _ in 0..4 {
        step(&mut s, 1);
    }
    let crystal = s.mobs.end.crystals.iter().any(|x| x.id == c && !x.removed);
    let mut bad = Vec::new();
    if crystal {
        bad.push("crystal intact after the arrow");
    }
    if proj_alive(&s, b) {
        bad.push("shulker bullet intact after the snowball");
    }
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn fireball_explosion_hits_bullets() {
    let Some(mut s) = open("proj-blast", &[]) else { return };
    let o = s.pos.map(|v| v.floor() as i32);
    cmd(
        &mut s,
        &format!("fill {} {} {} {} {} {} minecraft:stone", o[0] + 6, o[1], o[2] - 2, o[0] + 6, o[1] + 4, o[2] + 2),
    );
    let base = [f64::from(o[0]) + 0.5, f64::from(o[1]) + 2.5, f64::from(o[2]) + 0.5];
    // Immobile bullet near the wall, fireball that crashes into it during the projectile tick.
    let b = new_proj(&mut s, PKind::ShulkerBullet, [base[0] + 4.5, base[1], base[2] + 1.0], [0.0; 3]);
    let f = new_proj(&mut s, PKind::LargeFireball, [base[0] + 3.0, base[1], base[2]], [1.0, 0.0, 0.0]);
    s.proj.list.iter_mut().filter(|p| p.id == f).for_each(|p| p.power = 1);
    for _ in 0..4 {
        step(&mut s, 1);
    }
    assert!(!proj_alive(&s, f), "fireball still flying");
    assert!(!proj_alive(&s, b), "shulker bullet intact in the explosion radius");
}

fn open(tag: &str, pack: &[(&str, &str)]) -> Option<Server> {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else {
        eprintln!("test world or data missing: test skipped");
        return None;
    };
    let dir = crate::tests::fresh_copy(&world, tag);
    for (path, text) in pack {
        let p = dir.join("datapacks").join("t").join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    if !pack.is_empty() {
        std::fs::write(
            dir.join("datapacks").join("t").join("pack.mcmeta"),
            r#"{"pack":{"pack_format":121,"description":"t"}}"#,
        )
        .unwrap();
    }
    let mut s = crate::portal_tests::open_world(&dir, std::sync::Arc::new(data));
    s.loaded = true;
    s.teleport = None;
    let c = ((s.pos[0].floor() as i32) >> 4, (s.pos[2].floor() as i32) >> 4);
    let around: Vec<(i32, i32)> = (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| (c.0 + dx, c.1 + dz))).collect();
    s.load_now(&around);
    Some(s)
}

fn step(s: &mut Server, n: usize) {
    for _ in 0..n {
        s.game_tick();
        if s.lan.is_some() {
            s.lan_tick();
        }
        s.tick += 1;
    }
}

fn cmd(s: &mut Server, c: &str) {
    let src = s.server_source();
    let (ok, _) = s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

/// Opens the world to the LAN and adds a guest without a real connection (state installed).
fn add_guest(s: &mut Server, name: &str) -> crossbeam_channel::Sender<Vec<u8>> {
    crate::lan::announce::LOCAL_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
    if s.lan.is_none() {
        let req = crate::lan::Publish { open: true, port: 0, guest_commands: false, force_mode: Some(0) };
        s.lan_publish(&req).expect("opening");
    }
    let uuid = rc_client::offline_uuid(name);
    let mut ctx = s.new_player_ctx(name, uuid, 2, 0, Some(0));
    ctx.loaded = true;
    let (out, _) = crossbeam_channel::unbounded();
    let (keep, inp) = crossbeam_channel::unbounded();
    s.lan.as_mut().unwrap().guests.push(crate::lan::Guest {
        name: name.into(),
        uuid,
        out,
        inp,
        ctx: Some(Box::new(ctx)),
        enc: Default::default(),
        gone: false,
        joined: true,
        net: Default::default(),
        req_view: 2,
        chunks: Default::default(),
        moves: Default::default(),
    });
    keep
}

#[test]
fn tick_and_load_tags() {
    let pack = [
        ("data/t/function/load.mcfunction", "scoreboard objectives add t dummy\nscoreboard players add #load t 1\n"),
        ("data/t/function/tick.mcfunction", "scoreboard players add #tick t 1\n"),
        ("data/minecraft/tags/function/load.json", r#"{"values":["t:load"]}"#),
        ("data/minecraft/tags/function/tick.json", r#"{"values":["t:tick"]}"#),
    ];
    let Some(mut s) = open("tick-tags", &pack) else { return };
    step(&mut s, 3);
    assert_eq!(s.cmd.scores.get("#load", "t"), Some(1), "#minecraft:load once after loading");
    assert_eq!(s.cmd.scores.get("#tick", "t"), Some(3), "#minecraft:tick at every tick");
    s.s9_reload();
    step(&mut s, 1);
    assert_eq!(s.cmd.scores.get("#load", "t"), Some(2), "#minecraft:load after /reload");
    assert_eq!(s.cmd.scores.get("#tick", "t"), Some(4));
}

#[test]
fn scheduled_function_before_entities() {
    let pack = [("data/t/function/spawn.mcfunction", "summon minecraft:pig ~ ~ ~ {Tags:[\"t3\"]}\n")];
    let Some(mut s) = open("tick-sched", &pack) else { return };
    // The server source is at the spawn point: the function summons at the player.
    let p = s.pos;
    std::fs::write(
        s.world.join("datapacks").join("t").join("data/t/function/spawn.mcfunction"),
        format!("summon minecraft:pig {} {} {}\n", p[0], p[1], p[2]),
    )
    .unwrap();
    s.s9_reload();
    let before: Vec<i32> = s.mobs.list.iter().flatten().map(|m| m.id).collect();
    cmd(&mut s, "schedule function t:spawn 1t");
    step(&mut s, 1);
    let pig = s.mobs.list.iter().flatten().find(|m| !before.contains(&m.id)).expect("pig summoned");
    // Game: scheduled functions run before the entity tick; the pig is ticked as of the tick
    // of its spawn.
    assert_eq!(pig.tick_count, 1, "pig summoned by a scheduled function ticked this tick");
}

#[test]
fn command_minecart_once_per_world_tick() {
    let Some(mut s) = open("tick-cart", &[]) else { return };
    let b = s.pos.map(|v| v.floor() as i32);
    cmd(&mut s, "scoreboard objectives add c dummy");
    cmd(&mut s, &format!("setblock {} {} {} minecraft:redstone_block", b[0] + 2, b[1] - 1, b[2]));
    cmd(&mut s, &format!("setblock {} {} {} minecraft:activator_rail[powered=true]", b[0] + 2, b[1], b[2]));
    cmd(
        &mut s,
        &format!(
            "summon minecraft:command_block_minecart {} {} {} {{Command:\"scoreboard players add #n c 1\"}}",
            f64::from(b[0] + 2) + 0.5,
            b[1],
            f64::from(b[2]) + 0.5
        ),
    );
    let _keep = add_guest(&mut s, "Invite1");
    step(&mut s, 9);
    // Game: tick counter − last activation tick ≥ 4, with that last tick at 0 at the start (G5-F06):
    // ages 4 and 8, two executions, whatever the number of players.
    assert_eq!(s.cmd.scores.get("#n", "c"), Some(2), "minecart command");
}

fn overworld(s: &Server) -> i64 {
    s.clocks.iter().find(|c| c.0 == "minecraft:overworld").map_or(-1, |c| c.1)
}

#[test]
fn night_skip_over_all_players_and_guest_systems() {
    let Some(mut s) = open("tick-sleep", &[]) else { return };
    let _keep = add_guest(&mut s, "Invite2");
    cmd(&mut s, "gamerule minecraft:advance_time true");
    for c in &mut s.clocks {
        if c.0 == "minecraft:overworld" {
            c.1 = 18000;
        }
    }
    // The guest has slept for 100 ticks, the host is standing: the night does not pass (100 %).
    // A real bed: a sleeper whose bed has disappeared gets up on the next tick (game).
    let head = s.pos.map(|v| v.floor() as i32 + 3);
    let head = [head[0], s.pos[1].floor() as i32, head[2]];
    cmd(&mut s, &format!("setblock {} {} {} minecraft:red_bed[part=head]", head[0], head[1], head[2]));
    {
        let c = s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap();
        c.vit.sleeping = Some(head);
        c.vit.sleep_timer = 100;
    }
    // Sleepers list sampled as at lying down.
    s.update_sleeping_list();
    step(&mut s, 3);
    let t = overworld(&s);
    assert!((18000..18010).contains(&t), "night passed by a single guest in bed: {t}");
    let c = s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap();
    assert!(c.vit.sleeping.is_some(), "guest woken wrongly");
    // Player-specific systems run for the guest (memory, attack).
    assert!(c.player_ticks >= 3, "guest's own tick missing: {}", c.player_ticks);
    // The host lies down too: everyone sleeps enough, the night passes and everyone gets up.
    s.life.vit.sleeping = Some(head);
    s.life.vit.sleep_timer = 100;
    s.update_sleeping_list();
    step(&mut s, 1);
    let t = overworld(&s);
    assert_eq!(t.rem_euclid(24000), 0, "morning after the night skip: {t}");
    assert!(s.life.vit.sleeping.is_none());
    assert!(s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().vit.sleeping.is_none());
}
