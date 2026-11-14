//! A player as another player's attacker, as the game records any player source on any victim:
//! after `damage <guest> … by <host>`, `execute on attacker` from the guest finds the host, the
//! guest's death names the host and credits it with the kill (before any mob that hurt the guest
//! since, for 100 ticks), and nothing is recorded when `pvp` refuses the hit. Before: the guest's
//! health went down with no attacker at all. Fresh flat world: the 26.3 data cache and the client
//! jar are required (absent: FAILURE).

use rc_client::Event;

use super::tests_commands_reach_guests::{guest, host_and_guest, host_runs, tagged_mob};
use crate::blocks::probe::Probe;
use crate::mobs::species::Kind;

const FINDS_THE_HOST: &str = "execute as Guest on attacker if entity @s[tag=host]";

/// The host's hit by `damage … by @s` makes it the guest's last attacker; a mob's later hit takes
/// its place, as the game keeps one last attacker.
#[test]
fn damage_by_the_host_makes_it_the_guests_attacker() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-host-attacks-guest");
    assert!(host_runs(&mut p, "tag @s add host").0);
    assert!(host_runs(&mut p, "damage Guest 1 minecraft:player_attack by @s").0, "damage by the host");
    assert_eq!(guest(&p, 0).vit.health, 19.0);
    assert_eq!(host_runs(&mut p, FINDS_THE_HOST), (true, 1), "the host is the guest's attacker");
    tagged_mob(&mut p, Kind::Zombie, "summon minecraft:zombie 0.5 -60 6.5 {NoAI:1b,Tags:[\"hit\"]}");
    end_invulnerability(&mut p);
    assert!(host_runs(&mut p, "damage Guest 1 minecraft:mob_attack by @e[tag=hit,limit=1]").0, "damage by the zombie");
    assert_eq!(
        host_runs(&mut p, "execute as Guest on attacker if entity @s[tag=hit]"),
        (true, 1),
        "the zombie took its place"
    );
    p.s.lan_close();
}

/// The guest killed by the host's `damage`: its death message names the host, it counts
/// `killed_by` player and the host a player kill (statistic and `playerKillCount`).
#[test]
fn death_by_the_hosts_damage_credits_the_host() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-host-kills-guest");
    assert!(host_runs(&mut p, "scoreboard objectives add kills playerKillCount").0);
    let _ = p.take_events();
    assert!(host_runs(&mut p, "damage Guest 100 minecraft:player_attack by @s").0);
    assert!(guest(&p, 0).vit.dead, "the guest died");
    assert_eq!(death_message(&mut p), Some(("death.attack.player".into(), 2)), "the host named in the guest's death");
    assert_kill_credited_to_the_host(&mut p);
    let host = p.s.name.clone();
    assert_eq!(p.s.cmd.scores.get(&host, "kills"), Some(1), "playerKillCount of the host");
    p.s.lan_close();
}

/// A zombie's killing hit less than 100 ticks after the host's hit: the host is credited, not the
/// zombie (the game prefers the player who hurt the victim lately to its last mob).
#[test]
fn the_host_is_credited_before_the_zombie_that_finished_the_guest() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-host-then-zombie");
    tagged_mob(&mut p, Kind::Zombie, "summon minecraft:zombie 0.5 -60 6.5 {NoAI:1b,Tags:[\"hit\"]}");
    assert!(host_runs(&mut p, "damage Guest 1 minecraft:player_attack by @s").0);
    end_invulnerability(&mut p);
    assert!(host_runs(&mut p, "damage Guest 100 minecraft:mob_attack by @e[tag=hit,limit=1]").0);
    assert!(guest(&p, 0).vit.dead, "the guest died");
    let by_zombie = p.s.as_guest(0, |sv| sv.stat_value("minecraft:killed_by", "minecraft:zombie"));
    assert_eq!(by_zombie, 0, "the zombie credited although the host hurt the guest lately");
    assert_kill_credited_to_the_host(&mut p);
    p.s.lan_close();
}

/// The host's hit is forgotten 100 ticks later: no attacker, and a death then credits nobody.
#[test]
fn a_player_attacker_is_forgotten_after_100_ticks() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-attacker-forgotten");
    assert!(host_runs(&mut p, "tag @s add host").0);
    assert!(host_runs(&mut p, "damage Guest 1 minecraft:player_attack by @s").0);
    p.s.as_guest(0, |sv| {
        for _ in 0..101 {
            sv.player_memory_tick();
        }
    });
    assert_eq!(host_runs(&mut p, FINDS_THE_HOST), (false, 0), "remembered past 100 ticks");
    end_invulnerability(&mut p);
    assert!(host_runs(&mut p, "damage Guest 100 minecraft:generic").0);
    let by_player = p.s.as_guest(0, |sv| sv.stat_value("minecraft:killed_by", "minecraft:player"));
    assert_eq!(
        (by_player, p.s.stat_value("minecraft:custom", "minecraft:player_kills")),
        (0, 0),
        "a forgotten hit credited"
    );
    p.s.lan_close();
}

/// `pvp` off: the host's `damage` on the guest is refused (invulnerable) and records nothing, as
/// the game refuses a player source the victim may not be harmed by.
#[test]
fn pvp_off_refuses_the_hosts_damage_and_records_nothing() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-damage-pvp-off");
    assert!(host_runs(&mut p, "tag @s add host").0);
    crate::nbtio::set(&mut p.s.rules, "minecraft:pvp", rc_proto::nbt::Nbt::Byte(0));
    assert!(!host_runs(&mut p, "damage Guest 1 minecraft:player_attack by @s").0, "refused like a hit");
    assert_eq!(guest(&p, 0).vit.health, 20.0);
    assert_eq!(host_runs(&mut p, FINDS_THE_HOST), (false, 0), "a refused hit recorded its author");
    p.s.lan_close();
}

/// The guest's invulnerability window over: the next hit lands in full.
fn end_invulnerability(p: &mut Probe) {
    if let Some(c) = p.s.lan.as_mut().and_then(|l| l.guests[0].ctx.as_mut()) {
        c.vit.cooldown = 0;
    }
}

/// The host credited with the guest's death, once: `killed_by` player on the guest's side, the
/// player kill on the host's.
fn assert_kill_credited_to_the_host(p: &mut Probe) {
    let by_player = p.s.as_guest(0, |sv| sv.stat_value("minecraft:killed_by", "minecraft:player"));
    assert_eq!(by_player, 1, "the guest not counted killed by a player");
    assert_eq!(p.s.stat_value("minecraft:custom", "minecraft:player_kills"), 1, "the host not credited");
}

/// Translation key and argument count of the first death message the host was told.
fn death_message(p: &mut Probe) -> Option<(String, usize)> {
    p.take_events().into_iter().find_map(|e| match e {
        Event::Chat { component, .. } => match &component.content {
            rc_text::Content::Translate { key, with, .. } if key.starts_with("death.attack.") => {
                Some((key.clone(), with.len()))
            }
            _ => None,
        },
        _ => None,
    })
}
