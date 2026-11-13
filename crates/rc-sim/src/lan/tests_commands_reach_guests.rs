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
    // Its data as `data` and `execute if data` read it (written with the guest installed).
    assert_eq!(host_runs(&mut p, "data get entity Guest XpLevel"), (true, 5), "data get of the guest");
    assert_eq!(host_runs(&mut p, "execute if data entity Guest {XpLevel:5}"), (true, 1), "if data of the guest");
    assert_eq!(host_runs(&mut p, "execute positioned 6.5 -60 0.5 if entity @a[distance=..1]"), (true, 1));
    assert!(host_runs(&mut p, "execute positioned 0.5 -60 0.5 run tag @a[sort=furthest,limit=1] add far").0);
    assert!(tags(&guest(&p, 0).pdata.root).contains(&"far".to_owned()), "the furthest player is the guest");
    p.s.lan_close();
}

/// The options read with each guest installed (here `nbt=`) are tested before the limit: with
/// `limit=1`, a first guest failing them does not hide a second one passing them, for a players
/// selector (`execute if entity`, which wants one match, and a final command) as for `@e`. The
/// host and the first guest stand at level 0, the second guest at level 5. Fails if the search
/// stops at the limit before those options are tested (it keeps the first guest, then drops it).
#[test]
fn installed_options_are_tested_before_the_limit() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-installed-before-limit");
    let (_second_packets, _keep_second) = add_guest(&mut p, "Second", [0.5, -60.0, 6.5]);
    assert!(host_runs(&mut p, "experience set Second 5 levels").0);
    let one_match = host_runs(&mut p, "execute if entity @a[nbt={XpLevel:5},limit=1]");
    assert_eq!(one_match, (true, 1), "execute if entity: the second guest");
    assert!(host_runs(&mut p, "tag @a[nbt={XpLevel:5},limit=1] add players_selector").0, "@a: the second guest");
    assert!(host_runs(&mut p, "tag @e[nbt={XpLevel:5},limit=1] add entity_selector").0, "@e: the second guest");
    let second = tags(&guest(&p, 1).pdata.root);
    for t in ["players_selector", "entity_selector"] {
        assert!(second.contains(&t.to_owned()), "tag {t} not on the second guest: {second:?}");
    }
    assert!(tags(&guest(&p, 0).pdata.root).is_empty(), "the first guest was tagged");
    p.s.lan_close();
}

/// The same with a search of every world (a pig in a forced chunk of the Nether opens a second
/// world to search): the overworld's part of the search tests the guests' installed options
/// before its budget.
#[test]
fn installed_options_are_tested_before_the_limit_in_every_world() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-installed-every-world");
    let (_second_packets, _keep_second) = add_guest(&mut p, "Second", [0.5, -60.0, 6.5]);
    assert!(host_runs(&mut p, "execute in minecraft:the_nether run forceload add 0 0").0);
    p.s.in_world(crate::dims::NETHER, |s| s.load_now(&[(0, 0)]));
    assert!(host_runs(&mut p, "execute in minecraft:the_nether run summon minecraft:pig 2 70 0 {NoAI:1b,NoGravity:1b}").0);
    assert!(host_runs(&mut p, "experience set Second 5 levels").0);
    assert!(host_runs(&mut p, "execute as @e[nbt={XpLevel:5},limit=1] run tag @s add every_world").0);
    assert!(tags(&guest(&p, 1).pdata.root).contains(&"every_world".to_owned()), "the second guest was not found");
    assert!(tags(&guest(&p, 0).pdata.root).is_empty(), "the first guest was tagged");
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
    let counted = host_runs(&mut p, "execute if items entity Guest container.* minecraft:diamond");
    assert_eq!(counted, (true, 2), "the guest's diamonds as `execute if items` counts them");
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

/// A custom bar's progress goes to its players only when it changes, like the game which compares
/// the new progress with the old one: a store of the same value every tick, or a value past the
/// max when the bar is already full, sends nothing. Before: one packet per player at each write.
#[test]
fn an_unchanged_bar_progress_is_not_sent_again() {
    let (mut p, _dir, guest_packets, _keep) = host_and_guest("lan-bar-progress");
    let ids = *p.s.peer.ids();
    for c in [
        "bossbar add test:bar \"Bar\"",
        "bossbar set test:bar players Guest",
        "scoreboard objectives add obj dummy",
        "scoreboard players set x obj 30",
    ] {
        assert!(host_runs(&mut p, c).0, "setup: {c}");
    }
    let boss_packets = |r: &Receiver<Vec<u8>>| r.try_iter().filter(|b| super::tests_g3f05::id_of(b) == ids.boss_event).count();
    let _ = boss_packets(&guest_packets);
    let store = "execute store result bossbar test:bar value run scoreboard players get x obj";
    assert!(host_runs(&mut p, store).0);
    assert_eq!(boss_packets(&guest_packets), 1, "a new value: its progress");
    assert!(host_runs(&mut p, store).0);
    assert_eq!(boss_packets(&guest_packets), 0, "the same value stored again");
    assert!(host_runs(&mut p, "bossbar set test:bar value 150").0);
    assert_eq!(boss_packets(&guest_packets), 1, "the bar filled");
    assert!(host_runs(&mut p, "bossbar set test:bar value 200").0, "the value changes");
    assert_eq!(boss_packets(&guest_packets), 0, "the bar stays full");
    p.s.lan_close();
}

/// Summons a frozen mob of `kind` tagged `tag` (`summon` text in `command`): its entity id.
fn tagged_mob(p: &mut Probe, kind: crate::mobs::species::Kind, command: &str) -> i32 {
    assert!(host_runs(p, command).0, "setup: {command}");
    p.s.mobs.list.iter().rev().find(|m| m.kind == kind).map(|m| m.id).expect("summoned mob")
}

/// `ride` mounts another player and gets it off, its vehicle seen by `execute on vehicle` and
/// by a second mount. Before: the guest was "not riding" and could not be mounted.
#[test]
fn ride_mounts_and_dismounts_another_player() {
    use crate::mobs::species::Kind;
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-ride-guest");
    let pig = tagged_mob(&mut p, Kind::Pig, "summon minecraft:pig 6.5 -60 3.5 {NoAI:1b,Tags:[\"mount\"]}");
    assert!(host_runs(&mut p, "ride Guest mount @e[tag=mount,limit=1]").0, "ride … mount");
    assert_eq!((guest(&p, 0).ride.vehicle, p.s.mobs.mounts.riding.vehicle), (Some(pig), None), "vehicles (guest, host)");
    assert_eq!(host_runs(&mut p, "execute as Guest on vehicle if entity @s[tag=mount]"), (true, 1), "on vehicle");
    assert!(!host_runs(&mut p, "ride Guest mount @e[tag=mount,limit=1]").0, "already riding");
    assert!(host_runs(&mut p, "ride Guest dismount").0, "ride … dismount");
    assert_eq!(guest(&p, 0).ride.vehicle, None, "the guest got off");
    p.s.lan_close();
}

/// `trigger` run as another player counts for that player. Before: it asked for a player.
#[test]
fn trigger_counts_for_another_player_as_source() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-trigger-guest");
    assert!(host_runs(&mut p, "scoreboard objectives add press trigger").0);
    assert!(host_runs(&mut p, "scoreboard players enable Guest press").0);
    assert!(host_runs(&mut p, "execute as Guest run trigger press set 4").0, "trigger as the guest");
    assert_eq!(host_runs(&mut p, "scoreboard players get Guest press"), (true, 4));
    p.s.lan_close();
}

/// `loot … mainhand` run as another player reads that player's hand. Before: "no held items".
#[test]
fn loot_reads_the_hand_of_another_player_as_source() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-loot-guest");
    let mined = host_runs(&mut p, "execute as Guest run loot spawn 6.5 -59 0.5 mine 6 -61 0 mainhand");
    assert!(mined.0, "loot … mainhand as the guest");
    p.s.lan_close();
}

/// `damage … by <another player>` credits that player with the hit, as its own hit in play
/// would. Before: the hit had no author.
#[test]
fn damage_by_another_player_credits_that_player() {
    use crate::mobs::species::Kind;
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-damage-by-guest");
    let cow = tagged_mob(&mut p, Kind::Cow, "summon minecraft:cow 0.5 -60 6.5 {NoAI:1b,Tags:[\"hit\"]}");
    assert!(host_runs(&mut p, "damage @e[tag=hit,limit=1] 1 minecraft:player_attack by Guest").0);
    let credited = p.s.mobs.get(cow).map(|m| m.last_hurt_by_player);
    assert!(credited.is_some_and(|t| t > 0), "the cow's hit has no player author: {credited:?}");
    p.s.lan_close();
}

/// `execute on attacker` from another player reads the mob that last hurt that player. Before:
/// nothing.
#[test]
fn the_attacker_of_another_player_is_found() {
    use crate::mobs::species::Kind;
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-guest-attacker");
    let cow = tagged_mob(&mut p, Kind::Cow, "summon minecraft:cow 0.5 -60 6.5 {NoAI:1b,Tags:[\"hit\"]}");
    if let Some(c) = p.s.lan.as_mut().and_then(|l| l.guests[0].ctx.as_mut()) {
        c.player_hurt_by = Some((crate::mobs::mob::Ent::Mob(cow), 0));
    }
    assert_eq!(host_runs(&mut p, "execute as Guest on attacker if entity @s[tag=hit]"), (true, 1), "on attacker");
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
