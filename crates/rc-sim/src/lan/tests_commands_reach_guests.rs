//! Commands of a world open to the local network reach every player of the list, like the game's:
//! the selectors (`@a`, `@p`, `@e`, a name, a UUID) find the guests with their options read in each
//! guest's own state, the player commands act on the player they name, `msg` and custom boss bars
//! go to their players alone, and the chat tags are those the host's own client gives. Before: the
//! selectors and the names found the player typing the command only. Fresh flat world: the 26.3
//! data cache and the client jar are required (absent: FAILURE).

use crossbeam_channel::{Receiver, Sender};
use rc_client::{ChatTag, Event};

use crate::blocks::probe::Probe;
use crate::lan::ctx::PlayerCtx;
use crate::other_dimension_send_tests::new_world;

const HOST_AT: [f64; 3] = [0.5, -60.0, 0.5];
const GUEST_AT: [f64; 3] = [6.5, -60.0, 0.5];

/// Opens the world to the network and adds a joined guest named `name` at `pos`: what it is sent,
/// and the sender that keeps its connection open.
fn add_guest(p: &mut Probe, name: &str, pos: [f64; 3]) -> (Receiver<Vec<u8>>, Sender<Vec<u8>>) {
    crate::mobs::tests_spawn_paths::add_guest_with_packets(&mut p.s, name, pos)
}

/// The host at `HOST_AT` and one guest "Guest" at `GUEST_AT`.
fn host_and_guest(tag: &str) -> (Probe, std::path::PathBuf, Receiver<Vec<u8>>, Sender<Vec<u8>>) {
    let (mut p, dir) = new_world(tag);
    p.s.pos = HOST_AT;
    let (packets, keep) = add_guest(&mut p, "Guest", GUEST_AT);
    (p, dir, packets, keep)
}

fn guest(p: &Probe, seat: usize) -> &PlayerCtx {
    p.s.lan.as_ref().and_then(|l| l.guests[seat].ctx.as_deref()).expect("guest in game")
}

/// A command typed by the host (level 4): success and result.
fn host_runs(p: &mut Probe, text: &str) -> (bool, i32) {
    let mut src = p.s.player_source();
    src.level = 4;
    p.s.perform(src, text)
}

fn tags(root: &rc_proto::nbt::Nbt) -> Vec<String> {
    crate::nbtio::list(root.get("Tags")).iter().filter_map(|t| t.as_str().map(str::to_owned)).collect()
}

fn count_of(slots: &[Option<crate::player::Stack>], id: &str) -> i32 {
    slots.iter().flatten().filter(|s| s.id == id).map(|s| s.count).sum()
}

/// Whether a packet holds `text` (a translation key of a component, say).
fn holds(packet: &[u8], text: &str) -> bool {
    packet.windows(text.len()).any(|w| w == text.as_bytes())
}

/// `@a`, `@e[type=player]`, `@p`, a name (case ignored), a UUID and `@s` under `execute as` find
/// the guest; the options read its own game mode, level, team, scores, position and (installed)
/// NBT. Before: `@a` counted the host alone and the guest's name was an unknown player.
#[test]
fn selectors_find_the_guests_with_their_own_state() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-selectors");
    assert_eq!(host_runs(&mut p, "execute if entity @a"), (true, 2), "@a: the host and the guest");
    assert_eq!(host_runs(&mut p, "execute if entity @e[type=minecraft:player]"), (true, 2), "@e of type player");
    assert!(host_runs(&mut p, "execute positioned 6.5 -60 0.5 run tag @p add nearest").0);
    assert!(tags(&guest(&p, 0).pdata.root).contains(&"nearest".to_owned()), "@p by the guest is the guest");
    assert!(!tags(&p.s.pdata.root).contains(&"nearest".to_owned()), "@p by the guest took the host");
    assert!(host_runs(&mut p, "tag gUeSt add named").0, "the guest's name, case ignored");
    let uuid = crate::nbtio::uuid_text(guest(&p, 0).uuid);
    assert!(host_runs(&mut p, &format!("tag {uuid} add by_uuid")).0, "the guest's UUID");
    assert!(host_runs(&mut p, "execute as Guest run tag @s add own").0, "@s of the guest");
    let guest_tags = tags(&guest(&p, 0).pdata.root);
    for t in ["named", "by_uuid", "own"] {
        assert!(guest_tags.contains(&t.to_owned()), "tag {t} not on the guest: {guest_tags:?}");
    }
    assert!(tags(&p.s.pdata.root).is_empty(), "the host was tagged: {:?}", tags(&p.s.pdata.root));

    assert!(host_runs(&mut p, "gamemode creative Guest").0);
    assert!(host_runs(&mut p, "experience set Guest 5 levels").0);
    assert!(host_runs(&mut p, "team add red").0 && host_runs(&mut p, "team join red Guest").0);
    assert!(host_runs(&mut p, "scoreboard objectives add points dummy").0);
    assert!(host_runs(&mut p, "scoreboard players set Guest points 3").0);
    for option in ["gamemode=creative", "level=5", "team=red", "scores={points=3}", "nbt={XpLevel:5}", "name=Guest"] {
        let found = host_runs(&mut p, &format!("execute if entity @a[{option}]"));
        assert_eq!(found, (true, 1), "@a[{option}] does not find the guest alone");
    }
    // The NBT option read with the guest installed, then limited: the limit counts after it.
    assert_eq!(host_runs(&mut p, "execute if entity @a[nbt={XpLevel:5},limit=1]"), (true, 1));
    assert_eq!(host_runs(&mut p, "execute positioned 6.5 -60 0.5 if entity @a[distance=..1]"), (true, 1));
    assert!(host_runs(&mut p, "execute positioned 0.5 -60 0.5 run tag @a[sort=furthest,limit=1] add far").0);
    assert!(tags(&guest(&p, 0).pdata.root).contains(&"far".to_owned()), "the furthest player is the guest");
    p.s.lan_close();
}

/// `gamemode`, `give`, `tp`, `effect`, `experience`, `clear`, `spawnpoint`, `advancement`,
/// `recipe` and `kill` act on the player they name, the guest as the host. Before: `gamemode
/// creative Guest` failed (unknown player) and `give @a` or `tp @a` touched the host alone.
#[test]
fn player_commands_act_on_the_named_player() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-player-commands");
    assert!(host_runs(&mut p, "gamemode creative Guest").0);
    assert_eq!((p.s.game_mode, guest(&p, 0).game_mode), (0, 1), "game modes (host, guest)");
    assert_eq!(host_runs(&mut p, "give @a minecraft:diamond 2"), (true, 2));
    assert_eq!(count_of(&p.s.slots, "minecraft:diamond"), 2, "the host's diamonds");
    assert_eq!(count_of(&guest(&p, 0).slots, "minecraft:diamond"), 2, "the guest's diamonds");
    assert!(host_runs(&mut p, "clear Guest minecraft:diamond").0);
    assert_eq!(count_of(&guest(&p, 0).slots, "minecraft:diamond"), 0, "the guest's diamonds after clear");
    assert_eq!(count_of(&p.s.slots, "minecraft:diamond"), 2, "clear Guest took the host's diamonds");
    assert_eq!(host_runs(&mut p, "tp @a 3.5 -60 3.5"), (true, 2));
    assert_eq!(p.s.pos, [3.5, -60.0, 3.5], "the host was not moved");
    assert_eq!(guest(&p, 0).pos, [3.5, -60.0, 3.5], "the guest was not moved");
    assert!(host_runs(&mut p, "effect give Guest minecraft:speed 10").0);
    assert!(guest(&p, 0).fx.has("minecraft:speed") && !p.s.life.fx.has("minecraft:speed"), "speed on the guest alone");
    assert!(host_runs(&mut p, "experience add Guest 3 levels").0);
    assert_eq!((p.s.life.vit.xp_level, guest(&p, 0).vit.xp_level), (0, 3), "levels (host, guest)");
    assert!(host_runs(&mut p, "spawnpoint Guest 10 -60 10").0);
    let respawn = guest(&p, 0).vit.respawn.as_ref().map(|r| r.1);
    assert_eq!(respawn, Some([10, -60, 10]), "the guest's spawn point");
    assert!(host_runs(&mut p, "advancement grant Guest only minecraft:story/root").0);
    let root = p.s.prog.tree.get("minecraft:story/root").expect("advancement in the tree");
    assert!(guest(&p, 0).prog.done(root) && !p.s.prog.done(root), "advancement granted to the guest alone");
    assert!(host_runs(&mut p, "recipe give Guest minecraft:crafting_table").0);
    let table = "minecraft:crafting_table";
    assert!(guest(&p, 0).known.contains(table) && !p.s.life.known.contains(table), "recipe given to the guest alone");
    assert!(host_runs(&mut p, "kill Guest").0);
    assert!(guest(&p, 0).vit.dead && !p.s.life.vit.dead, "kill Guest killed the guest alone");
    p.s.lan_close();
}

/// `msg` goes to its target alone, its echo to the player who typed it; a custom bar shows to the
/// players it names, and its changes reach them alone. Before: both lines of a whisper reached
/// every player and a bar naming a guest showed to the host only.
#[test]
fn whispers_and_custom_bars_reach_their_players_alone() {
    let (mut p, _dir, guest_packets, _keep) = host_and_guest("lan-whispers");
    let (other_packets, _keep_other) = add_guest(&mut p, "Other", [0.5, -60.0, 6.5]);
    let _ = (p.take_events(), guest_packets.try_iter().count(), other_packets.try_iter().count());
    p.s.player_command("msg Guest hello");
    let host_lines: Vec<(String, ChatTag)> = p
        .take_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Chat { component, tag } => Some((format!("{component:?}"), tag)),
            _ => None,
        })
        .collect();
    assert_eq!(host_lines.len(), 1, "the host's echo alone: {host_lines:?}");
    assert!(host_lines[0].0.contains("commands.message.display.outgoing"), "{host_lines:?}");
    assert_eq!(host_lines[0].1, ChatTag::Secure, "the host's own whisper carries no bar");
    let to_guest: Vec<Vec<u8>> = guest_packets.try_iter().collect();
    assert!(to_guest.iter().any(|b| holds(b, "commands.message.display.incoming")), "the guest got no whisper");
    assert!(
        !other_packets.try_iter().any(|b| holds(&b, "commands.message.display")),
        "the other guest got the whisper"
    );

    let ids = *p.s.peer.ids();
    assert!(host_runs(&mut p, "bossbar add test:bar \"Bar\"").0);
    let _ = p.take_events();
    assert!(host_runs(&mut p, "bossbar set test:bar players Guest").0);
    assert!(host_runs(&mut p, "bossbar set test:bar value 50").0);
    let boss_packets = |r: &Receiver<Vec<u8>>| r.try_iter().filter(|b| super::tests_g3f05::id_of(b) == ids.boss_event).count();
    assert_eq!(boss_packets(&guest_packets), 2, "the guest: the bar added, then its progress");
    assert_eq!(boss_packets(&other_packets), 0, "the other guest was sent the bar");
    assert!(!p.take_events().iter().any(|e| matches!(e, Event::Boss(_))), "the host was sent the bar");
    let (_, players) = host_runs(&mut p, "bossbar get test:bar players");
    assert_eq!(players, 1, "players in game the bar names");
    p.s.lan_close();
}

/// Chat tags as the host's own client gives them on its local server: its own messages (chat,
/// `say`, `me`) are secure (no bar, shown even with "only secure chat"), a guest's unsigned message
/// is not secure, and a `say` from no player (a command block, the server) is disguised as a
/// system message. Before: every one of them was "not secure".
#[test]
fn chat_tags_are_those_of_the_hosts_own_client() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-chat-tags");
    let _ = p.take_events();
    let tags_of = |events: Vec<Event>| -> Vec<ChatTag> {
        events.into_iter().filter_map(|e| if let Event::Chat { tag, .. } = e { Some(tag) } else { None }).collect()
    };
    p.s.player_chat("hello");
    assert_eq!(tags_of(p.take_events()), [ChatTag::Secure], "the host's chat message");
    p.s.player_command("say hi");
    p.s.player_command("me waves");
    assert_eq!(tags_of(p.take_events()), [ChatTag::Secure, ChatTag::Secure], "the host's say and me");
    let server = p.s.server_reply_source();
    assert!(p.s.perform(server, "say from the server").0);
    assert_eq!(tags_of(p.take_events()), [ChatTag::Disguised], "a say without a player sender");
    p.s.as_player(Some(0), |s| s.player_chat("hey"));
    assert_eq!(tags_of(p.take_events()), [ChatTag::NotSecure], "the guest's message at the host");
    p.s.lan_close();
}

/// In singleplayer too: the player's own chat and commands carry no bar.
#[test]
fn singleplayer_messages_of_the_player_are_secure() {
    let (mut p, _dir) = new_world("solo-chat-tags");
    let _ = p.take_events();
    p.s.player_chat("hello");
    p.s.player_command("say hi");
    let tags: Vec<ChatTag> =
        p.take_events().into_iter().filter_map(|e| if let Event::Chat { tag, .. } = e { Some(tag) } else { None }).collect();
    assert_eq!(tags, [ChatTag::Secure, ChatTag::Secure]);
}

/// The scoreboard (teams, displayed objectives and their scores) and the custom bars naming a
/// player go to the arriving player alone: the players already there hold them, and an official
/// client given an objective it already knows drops the connection. Before: they were broadcast
/// to every player at each arrival.
#[test]
fn an_arriving_player_alone_is_sent_the_scoreboard() {
    use super::tests_g3f05::{drain, guest_packets, id_of, ids, join, open, publish};
    let mut t = open("join-scoreboard");
    let mut src = t.s.server_reply_source();
    src.level = 4;
    for c in [
        "scoreboard objectives add shown dummy",
        "scoreboard objectives setdisplay sidebar shown",
        "scoreboard players set Someone shown 5",
        "team add blue",
        "bossbar add test:bar \"Bar\"",
    ] {
        assert!(t.s.perform(src.clone(), c).0, "setup: {c}");
    }
    t.s.cmd.bars[0].players.push(rc_client::offline_uuid("Second"));
    publish(&mut t, false);
    let first = join(&mut t, "First", 2);
    let _ = (drain(&t), guest_packets(&first));
    let second = join(&mut t, "Second", 2);
    let ids = ids(&t);
    let scoreboard = [ids.set_objective, ids.set_display_objective, ids.set_score, ids.set_player_team, ids.boss_event];
    let first_got: Vec<i32> = guest_packets(&first).iter().map(|b| id_of(b)).filter(|i| scoreboard.contains(i)).collect();
    assert!(first_got.is_empty(), "the guest already there was sent the scoreboard again: {first_got:?}");
    let host_got = drain(&t).into_iter().filter(|e| matches!(e, Event::Score(_) | Event::Boss(_))).count();
    assert_eq!(host_got, 0, "the host was sent the arrival's scoreboard");
    let second_got: Vec<i32> = guest_packets(&second).iter().map(|b| id_of(b)).collect();
    for (id, what) in [
        (ids.set_player_team, "team"),
        (ids.set_objective, "objective"),
        (ids.set_display_objective, "display slot"),
        (ids.set_score, "score"),
        (ids.boss_event, "custom bar"),
    ] {
        assert!(second_got.contains(&id), "the arriving player got no {what}");
    }
}
