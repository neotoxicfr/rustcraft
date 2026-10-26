//! Families F01–F06 wired into the singleplayer systems (S5 commands, S7 effects) and among themselves
//! (F05 → F06): end-to-end tests on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`).
//! Absent: tests skipped.

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::tests::{inject, kind_id, mob_nbt, move_player, prep};
use crate::tests::{start_test, test_world};

fn wait_for(s: &rc_client::Session, secs: u64, mut f: impl FnMut(&Event) -> bool) -> bool {
    crate::tests::wait_game(s, secs, &mut f)
}

/// Commands allowed (`allowCommands`); returns the world time (`Time`).
fn allow_commands(dir: &std::path::Path) -> i64 {
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    let mut time = 0;
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
        time = d.get("Time").and_then(Nbt::as_i64).unwrap_or(0);
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    time
}

/// Singleplayer demonstration tool: commands allowed in the world `RUSTCRAFT_INTEG_ALLOW`.
#[test]
#[ignore]
fn prep_allow_commands() {
    if let Ok(d) = std::env::var("RUSTCRAFT_INTEG_ALLOW") {
        allow_commands(std::path::Path::new(&d));
    }
}

/// `effect give` on a mob (S5 → F01/S7): glow and particles in its data,
/// instant heal inverted for an undead, then `effect clear`.
#[test]
fn effect_command_reaches_mob() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "integfx", 1, "normal", &[]);
    move_player(&dir, [210.5, -59.0, 210.5]);
    allow_commands(&dir);
    inject(&dir, &[mob_nbt("husk", [214.5, -59.0, 210.5], vec![("NoAI".into(), Nbt::Byte(1))])]);
    let husk_kind = kind_id(&data, "minecraft:husk");
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    let mut husk = None;
    wait_for(&s, 20, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == husk_kind
        {
            husk = Some(a.id);
        }
        husk.is_some()
    });
    let husk = husk.expect("husk not announced");
    s.link.send_command("effect give @e[type=minecraft:husk] minecraft:glowing 30").unwrap();
    let (mut glow, mut parts) = (false, false);
    wait_for(&s, 10, |e| {
        if let Event::Entity(EntityEvent::Data { id, values }) = e
            && *id == husk
        {
            for (i, v) in values {
                match (i, v) {
                    (0, Meta::Byte(b)) => glow |= b & 0x40 != 0,
                    (10, Meta::Particles(p)) => parts |= !p.is_empty(),
                    _ => {}
                }
            }
        }
        glow && parts
    });
    // Instant heal on an undead: damage (health < 20).
    s.link.send_command("effect give @e[type=minecraft:husk] minecraft:instant_health 1 0").unwrap();
    let hurt = wait_for(&s, 10, |e| {
        matches!(e, Event::Entity(EntityEvent::Data { id, values })
            if *id == husk && values.iter().any(|(i, v)| *i == 9 && matches!(v, Meta::Float(h) if *h < 20.0)))
    });
    s.link.send_command("effect clear @e[type=minecraft:husk]").unwrap();
    let cleared = wait_for(&s, 10, |e| {
        matches!(e, Event::Entity(EntityEvent::Data { id, values })
            if *id == husk && values.iter().any(|(i, v)| *i == 0 && matches!(v, Meta::Byte(b) if b & 0x40 == 0)))
    });
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(glow, "glow absent");
    assert!(parts, "effect particles absent");
    assert!(hurt, "instant heal not inverted on the husk");
    assert!(cleared, "effect not removed");
}

/// Three villagers who slept recently panic in front of a zombie: an iron golem appears
/// (F05's spawn of a golem if needed → F06's "legacy" golem spawn).
#[test]
fn panicking_villagers_summon_golem() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "integgolem", 1, "normal", &[]);
    move_player(&dir, [200.5, -59.0, 200.5]);
    let time = allow_commands(&dir);
    let brain = || {
        let slept = Nbt::Compound(vec![("value".into(), Nbt::Long(time))]);
        Nbt::Compound(vec![("memories".into(), Nbt::Compound(vec![("minecraft:last_slept".into(), slept)]))])
    };
    let mut ents: Vec<Nbt> = (0..3)
        .map(|i| mob_nbt("villager", [214.5 + f64::from(i), -59.0, 210.5], vec![("Brain".into(), brain())]))
        .collect();
    ents.push(mob_nbt("zombie", [216.5, -59.0, 214.5], vec![("NoAI".into(), Nbt::Byte(1))]));
    inject(&dir, &ents);
    let golem_kind = kind_id(&data, "minecraft:iron_golem");
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    // Glass pen (9 × 9, two high) around the villagers and the zombie, placed as soon as
    // their chunk is sent: the game only attempts the golem call during the panic, at a tick
    // multiple of 100; villagers fleeing out of the zombie's reach within three seconds
    // missed that tick depending on the phase of the world time. Enclosed within 8 blocks of the
    // zombie, they panic until the call.
    assert!(wait_for(&s, 10, |e| matches!(e, Event::Chunk(c) if (c.x, c.z) == (13, 13))), "pen chunk");
    for c in [
        "fill 211 -59 207 221 -58 207 minecraft:glass",
        "fill 211 -59 217 221 -58 217 minecraft:glass",
        "fill 211 -59 207 211 -58 217 minecraft:glass",
        "fill 221 -59 207 221 -58 217 minecraft:glass",
    ] {
        s.link.send_command(c).unwrap();
    }
    let mut refused = Vec::new();
    let got = wait_for(&s, 60, |e| {
        if let Event::Chat { component, .. } = e
            && component.style.color == Some(0xFF5555)
        {
            refused.push(format!("{component:?}"));
        }
        matches!(e, Event::Entity(EntityEvent::Add(a)) if a.kind == golem_kind)
    });
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(refused.is_empty(), "pen refused: {refused:?}");
    assert!(got, "no iron golem called by the villagers");
}
