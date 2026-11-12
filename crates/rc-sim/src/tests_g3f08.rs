//! G3-F08: commands `compute`, `posteffect`, `fetchprofile`; `attribute … base reset` (default of
//! the type); `loot insert|replace` (merge counted when full, placement rules, successive entity
//! slots, past the end of a container); `enchant` of a mob; `ride` failure named; `datapack create`
//! (rich description, reserved names); `return run debug function`; `spectate` of a marker. New
//! world made by the generator: only the version data and the jar are required (absent: explicit
//! failure).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rc_client::Event;
use rc_client::session::ProfileResult;
use rc_proto::nbt::Nbt;
use rc_text::{Arg, Component, Content};

use crate::commands::cmds_s9b::Slot;
use crate::server::Server;

struct T {
    s: Server,
    session: rc_client::Session,
    dir: std::path::PathBuf,
}

/// Inline loot table of one stone.
const STONE: &str = r#"{pools:[{rolls:1,entries:[{type:"minecraft:item",name:"minecraft:stone"}]}]}"#;

fn open(tag: &str) -> T {
    let data = Arc::new(rc_testdata::version_data().expect("version data missing: G3-F08 tests impossible"));
    crate::registries::client_jar(&data).expect("client jar missing: G3-F08 tests impossible");
    // Game folder > saves > world, like the game's: `debug` traces would land in the game
    // folder, inside the scratch folder.
    let root = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-g3f08-{tag}-{}", std::process::id())));
    let dir = root.join("saves").join("world");
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 8;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let pack = dir.join("datapacks").join("t");
    std::fs::create_dir_all(pack.join("data/t/context_int_provider")).unwrap();
    std::fs::write(pack.join("pack.mcmeta"), r#"{"pack":{"pack_format":121,"description":"t"}}"#).unwrap();
    std::fs::write(pack.join("data/t/context_int_provider/five.json"), r#"{"type":"minecraft:constant","value":5}"#)
        .unwrap();
    crate::tests::solo_slot();
    let uuid = rc_client::offline_uuid("G3F08");
    let (session, peer) = rc_client::memory::session(data.clone(), uuid).expect("session");
    let opts = crate::Options {
        world: dir.clone(),
        name: "G3F08".into(),
        uuid,
        info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
        sim_distance: 12,
    };
    let mut s = match Server::open(opts, uuid, data, peer, Arc::new(AtomicBool::new(false))) {
        Ok(s) => s,
        Err((_, e)) => panic!("opening: {e}"),
    };
    s.loaded = true;
    s.teleport = None;
    s.game_mode = 1;
    s.load_now(&[(-1, -1), (-1, 0), (0, -1), (0, 0)]);
    T { s, session, dir }
}

fn run(s: &mut Server, c: &str) -> (bool, i32) {
    let mut src = s.player_source();
    src.level = 4;
    s.perform(src, c)
}

/// Components sent in the chat since the last read (a failure, wrapped in a red empty text,
/// is unwrapped).
fn chats(t: &T) -> Vec<Component> {
    let mut out = Vec::new();
    while let Ok(e) = t.session.events.try_recv() {
        if let Event::Chat { component, .. } = e {
            let mut component = *component;
            if matches!(&component.content, Content::Text(s) if s.is_empty()) && component.extra.len() == 1 {
                component = component.extra.remove(0);
            }
            out.push(component);
        }
    }
    out
}

/// Anti-hang guard of the waits on the profile lookups (background threads): never the normal
/// path, a loaded machine only makes them slower.
const GUARD: std::time::Duration = std::time::Duration::from_secs(300);

/// Last chat message once the background lookup of `cmd` has answered, read by `read`.
fn answer<R>(t: &mut T, cmd: &str, read: impl Fn(&Component) -> R) -> R {
    let start = std::time::Instant::now();
    loop {
        t.s.fetchprofile_poll();
        if let Some(c) = chats(t).last() {
            return read(c);
        }
        assert!(start.elapsed() < GUARD, "anti-hang guard: no answer to `{cmd}`");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn key(c: &Component) -> &str {
    match &c.content {
        Content::Translate { key, .. } => key,
        _ => "",
    }
}

fn args(c: &Component) -> &[Arg] {
    match &c.content {
        Content::Translate { with, .. } => with,
        _ => &[],
    }
}

/// Plain text of a translation argument.
fn arg_text(a: &Arg) -> String {
    match a {
        Arg::Text(s) => s.clone(),
        Arg::Component(c) => crate::commands::cmds_ents::plain(c),
    }
}

/// Last chat message: key and plain arguments.
fn last(t: &T) -> (String, Vec<String>) {
    let all = chats(t);
    let c = all.last().cloned().unwrap_or_else(|| Component::text(""));
    (key(&c).to_owned(), args(&c).iter().map(arg_text).collect())
}

/// Checks collected without stopping: one run shows every difference (sabotage proof).
#[derive(Default)]
struct Fails(Vec<String>);

impl Fails {
    fn eq<V: PartialEq + std::fmt::Debug>(&mut self, what: &str, got: V, want: V) {
        if got != want {
            self.0.push(format!("{what}: got {got:?}, want {want:?}"));
        }
    }
    fn done(self) {
        assert!(self.0.is_empty(), "{} difference(s):\n{}", self.0.len(), self.0.join("\n"));
    }
}

#[test]
fn compute_values_errors_and_messages() {
    let mut t = open("compute");
    let mut f = Fails::default();
    let _ = chats(&t);
    let c = |t: &mut T, cmd: &str| {
        let r = run(&mut t.s, cmd);
        (r, last(t))
    };
    let (r, m) = c(&mut t, "compute default integer {type:\"minecraft:sub\",left:-2147483648,right:1}");
    f.eq("sub overflow", (r, m), ((false, 0), ("command.compute.result.unnamed.invalid".into(), vec!["integer overflow".into()])));
    let (r, m) = c(&mut t, "compute default integer {type:\"minecraft:div\",left:-2147483648,right:-1}");
    f.eq("min / -1 wraps", (r, m.0), ((true, i32::MIN), "command.compute.result.unnamed.exact".into()));
    let (r, m) = c(&mut t, "compute default integer {type:\"minecraft:mod\",left:7,right:0}");
    f.eq("mod 0", (r, m.1), ((false, 0), vec!["/ by zero".to_owned()]));
    let (r, m) = c(&mut t, "compute default integer {type:\"minecraft:pow\",base:2,exponent:-1}");
    f.eq("negative exponent", (r, m.1), ((false, 0), vec!["negative exponent".to_owned()]));
    let (r, m) = c(&mut t, "compute default integer {type:\"minecraft:abs\",input:-2147483648}");
    f.eq("abs min", (r, m.1), ((false, 0), vec!["Overflow to represent absolute value of Integer.MIN_VALUE".to_owned()]));
    let (r, m) = c(&mut t, "compute default integer {type:\"minecraft:from_float\",input:3.0e9}");
    f.eq("float too big", (r, m.1), ((false, 0), vec!["Value 3000000000 can't be safely converted to int".to_owned()]));
    // A bare number reads as an identifier first (like the game): unknown provider.
    let (r, m) = c(&mut t, "compute default float 2.5");
    f.eq("bare number", (r, m.0), ((false, 0), "argument.resource_or_id.no_such_element".into()));
    let k = |v: &str| format!("{{type:\"minecraft:constant\",value:{v}}}");
    let (r, m) = c(&mut t, &format!("compute default float {}", k("2.5")));
    f.eq("rounded", (r, m), ((true, 2), ("command.compute.result.unnamed.rounded".into(), vec!["2.5".into(), "2".into()])));
    let (r, m) = c(&mut t, &format!("compute default float {}", k("3.0")));
    f.eq("exact float", (r, m.0), ((true, 3), "command.compute.result.unnamed.exact".into()));
    // Scaled: compared with the unscaled value, so "rounded" although 3 × 2 is exact.
    let (r, m) = c(&mut t, &format!("compute default float {} 2", k("3.0")));
    f.eq("scaled", (r, m.0), ((true, 6), "command.compute.result.unnamed.rounded".into()));
    let (r, m) = c(&mut t, "compute default float {type:\"minecraft:div\",left:1,right:0}");
    f.eq("infinite", (r, m.1), ((false, 0), vec!["Infinity".to_owned()]));
    // Ceiling and rounding through an integer: NaN gives 0.
    let (r, _) = c(&mut t, "compute default float {type:\"minecraft:ceil\",input:{type:\"minecraft:mod\",left:1,right:0}}");
    f.eq("ceil NaN", r, (true, 0));
    let (r, m) = c(&mut t, "compute default integer t:five");
    f.eq("named", (r, m), ((true, 5), ("command.compute.result.named.exact".into(), vec!["t:five".into(), "5".into()])));
    let (r, _) = c(&mut t, &format!("compute entity @s integer {}", k("4")));
    f.eq("entity context", r, (true, 4));
    f.done();
}

#[test]
fn posteffect_kept_saved_sent_and_carried_over() {
    let mut t = open("posteffect");
    let mut f = Fails::default();
    // Joining: the list goes out once, even empty.
    t.s.post_effects_tick();
    let sent = |t: &T| -> Vec<Vec<String>> {
        let mut out = Vec::new();
        while let Ok(e) = t.session.events.try_recv() {
            if let Event::Misc(rc_client::misc::Misc::PostEffects(l)) = e {
                out.push(l);
            }
        }
        out
    };
    f.eq("sent at join", sent(&t), vec![Vec::new()]);
    let r = run(&mut t.s, "posteffect add @s minecraft:creeper");
    f.eq("add", (r, last(&t).0), ((true, 1), "commands.posteffect.add.success.single".into()));
    let r = run(&mut t.s, "posteffect add @s minecraft:creeper");
    f.eq("add again", (r, last(&t).0), ((false, 0), "commands.posteffect.add.failed".into()));
    let r = run(&mut t.s, "posteffect list @s");
    f.eq("list", (r, last(&t).1.get(2).cloned()), ((true, 1), Some("minecraft:creeper".into())));
    t.s.post_effects_tick();
    t.s.post_effects_tick();
    f.eq("sent once after the change", sent(&t), vec![vec!["minecraft:creeper".to_owned()]]);
    f.eq(
        "saved",
        t.s.player_nbt().get("post_effects").cloned(),
        Some(Nbt::List(vec![Nbt::String("minecraft:creeper".into())])),
    );
    let r = run(&mut t.s, "posteffect remove @s minecraft:creeper");
    f.eq("remove", (r, last(&t).0), ((true, 1), "commands.posteffect.remove.success.single".into()));
    let r = run(&mut t.s, "posteffect clear @s");
    f.eq("clear empty", (r, last(&t).0), ((false, 0), "commands.posteffect.clear.failed".into()));
    f.eq("saved empty", t.s.player_nbt().get("post_effects").cloned(), Some(Nbt::List(Vec::new())));
    // Respawn and end of the credits: the new player inherits the list, sent again once.
    run(&mut t.s, "posteffect add @s minecraft:spider");
    t.s.post_effects_tick();
    let _ = sent(&t);
    let spider = vec!["minecraft:spider".to_owned()];
    t.s.life.vit.dead = true;
    t.s.perform_respawn();
    t.s.post_effects_tick();
    t.s.post_effects_tick();
    f.eq("respawn: list kept", (t.s.post_effects(), sent(&t)), (spider.clone(), vec![spider.clone()]));
    t.s.respawn_after_credits();
    t.s.post_effects_tick();
    f.eq("credits: list kept", (t.s.post_effects(), sent(&t)), (spider.clone(), vec![spider.clone()]));
    let r = run(&mut t.s, "posteffect list @s");
    f.eq("list after respawn", r, (true, 1));
    f.eq(
        "saved after respawn",
        t.s.player_nbt().get("post_effects").cloned(),
        Some(Nbt::List(vec![Nbt::String("minecraft:spider".into())])),
    );
    f.done();
}

#[test]
fn fetchprofile_entity_and_lookups() {
    let mut t = open("fetchprofile");
    let mut f = Fails::default();
    let _ = chats(&t);
    let r = run(&mut t.s, "fetchprofile entity @s");
    let all = chats(&t);
    let msg = all.last().cloned().unwrap_or_else(|| Component::text(""));
    let a = args(&msg);
    f.eq("player: key", (r, key(&msg)), ((true, 1), "commands.fetchprofile.entity.success"));
    // Second argument: four buttons separated by spaces, the first copies the profile.
    let id = crate::nbtio::uuid_nbt(t.s.uuid);
    let want = format!("{{id:{},name:\"G3F08\"}}", crate::commands::snbt::to_snbt(&id));
    let first_click = match a.get(1) {
        Some(Arg::Component(c)) => c.extra.first().and_then(|b| match &b.content {
            Content::Translate { with, .. } => match with.first() {
                Some(Arg::Component(inner)) => inner.style.click.as_ref().map(|k| (k.action.clone(), k.value.clone())),
                _ => None,
            },
            _ => None,
        }),
        _ => None,
    };
    f.eq("player: copied profile", first_click, Some(("copy_to_clipboard".to_owned(), want)));
    f.eq("player: buttons", a.get(1).map(|x| matches!(x, Arg::Component(c) if c.extra.len() == 7)), Some(true));
    run(&mut t.s, "summon minecraft:marker 1 100 1");
    let _ = chats(&t);
    let r = run(&mut t.s, "fetchprofile entity @e[type=minecraft:marker,limit=1]");
    f.eq("marker", (r, last(&t).0), ((false, 0), "commands.fetchprofile.no_profile".into()));
    // Lookups through the services (fake here): answered at a following poll.
    t.s.cmd.s9.profiles.services = crate::commands::cmds_s9e::Services {
        id_by_name: |n| (n == "known").then(|| (0x1234, "Known".to_owned())),
        profile: |id| {
            (id == 0x1234).then(|| ProfileResult {
                id,
                name: "Known".into(),
                properties: Vec::new(),
                forced_name_change: false,
                banned_skin: false,
            })
        },
    };
    let ask = |t: &mut T, cmd: &str| -> ((bool, i32), (String, Vec<String>)) {
        let r = run(&mut t.s, cmd);
        (r, answer(t, cmd, |c| (key(c).to_owned(), args(c).iter().map(arg_text).collect())))
    };
    let (r, m) = ask(&mut t, "fetchprofile name Known");
    f.eq("name", (r, m.0, m.1.first().cloned()), ((true, 1), "commands.fetchprofile.name.success".into(), Some("Known".into())));
    let (r, m) = ask(&mut t, "fetchprofile name nobody");
    f.eq("unknown name", (r, m.0), ((true, 1), "commands.fetchprofile.name.failure".into()));
    let (r, m) = ask(&mut t, "fetchprofile id 00000000-0000-0000-0000-000000001234");
    f.eq("id", (r, m.0), ((true, 1), "commands.fetchprofile.id.success".into()));
    f.done();
}

/// L9-COMBAT: the name cache of the game folder (`usercache.json`, next to `saves`). The join
/// records the player and recalls another name it held there ("formerly known as");
/// `fetchprofile name` reads the cache before the profile service (a name in any case: one call
/// only) and records what it finds, an unknown name lowercased with its offline identifier. The
/// file has the game's layout: identifier, name, expiry one month later.
#[test]
fn fetchprofile_and_join_use_the_name_cache() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let mut t = open("usercache");
    let mut f = Fails::default();
    let file = t.dir.parent().and_then(std::path::Path::parent).expect("game folder").join("usercache.json");
    let uuid = t.s.uuid;
    t.s.user_cache().lock().unwrap().add(uuid, "OldName");
    let _ = chats(&t);
    t.s.join();
    let joined = chats(&t)
        .iter()
        .find(|c| key(c).starts_with("multiplayer.player.joined"))
        .map(|c| (key(c).to_owned(), args(c).iter().map(arg_text).collect::<Vec<_>>()));
    f.eq("join recalls the old name", joined, Some(("multiplayer.player.joined.renamed".into(), vec!["G3F08".into(), "OldName".into()])));
    let read = || -> Vec<serde_json::Value> {
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&file).unwrap_or_default())
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
    };
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let uuid_text = |u: u128| {
        let h = format!("{u:032x}");
        format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
    };
    f.eq("layout", text.starts_with(&format!("[{{\"uuid\":\"{}\",\"name\":\"G3F08\",\"expiresOn\":\"", uuid_text(uuid))), true);
    let entries = read();
    f.eq("one entry per player", entries.len(), 1);
    let expires = entries.first().and_then(|e| e["expiresOn"].as_str()).and_then(crate::progress::parse_date).unwrap_or(0);
    let days = (expires - crate::progress::now_ms()) / 86_400_000;
    f.eq("expiry one month later", (27..=31).contains(&days), true);
    t.s.cmd.s9.profiles.services = crate::commands::cmds_s9e::Services {
        id_by_name: |n| {
            CALLS.fetch_add(1, SeqCst);
            (n == "known").then(|| (0x1234, "Known".to_owned()))
        },
        profile: |id| {
            (id == 0x1234).then(|| ProfileResult {
                id,
                name: "Known".into(),
                properties: Vec::new(),
                forced_name_change: false,
                banned_skin: false,
            })
        },
    };
    let ask = |t: &mut T, cmd: &str| -> String {
        run(&mut t.s, cmd);
        answer(t, cmd, |c| key(c).to_owned())
    };
    f.eq("name", ask(&mut t, "fetchprofile name Known"), "commands.fetchprofile.name.success".to_owned());
    f.eq("other case", ask(&mut t, "fetchprofile name KNOWN"), "commands.fetchprofile.name.success".to_owned());
    f.eq("service asked once", CALLS.load(SeqCst), 1);
    f.eq("unknown name", ask(&mut t, "fetchprofile name Nobody"), "commands.fetchprofile.name.failure".to_owned());
    let entries = read();
    let has = |id: u128, name: &str| {
        entries.iter().any(|e| e["uuid"].as_str() == Some(uuid_text(id).as_str()) && e["name"].as_str() == Some(name))
    };
    f.eq("found name recorded", has(0x1234, "Known"), true);
    f.eq("unknown name recorded offline", has(rc_client::offline_uuid("nobody"), "nobody"), true);
    f.eq("latest first", entries.first().and_then(|e| e["name"].as_str().map(str::to_owned)), Some("nobody".to_owned()));
    f.done();
}

/// `fetchprofile id` in bulk: every lookup answered by a pool of reused threads (no thread per
/// request), and two loads of different profiles run at the same time (the cache lock is not
/// held during the network access).
#[test]
fn fetchprofile_pool_reuses_threads_and_overlaps_loads() {
    use hashbrown::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering::SeqCst;
    use std::time::{Duration, Instant};
    static THREADS: Mutex<Option<HashSet<std::thread::ThreadId>>> = Mutex::new(None);
    static A: AtomicBool = AtomicBool::new(false);
    static B: AtomicBool = AtomicBool::new(false);
    /// The other load started (anti-hang bound only: a load started late on a loaded machine
    /// still meets this one; loads that never overlap fail after it).
    fn wait(flag: &AtomicBool) -> bool {
        let end = Instant::now() + Duration::from_secs(120);
        while !flag.load(SeqCst) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(1));
        }
        flag.load(SeqCst)
    }
    let mut t = open("fetchpool");
    let mut f = Fails::default();
    t.s.cmd.s9.profiles.services = crate::commands::cmds_s9e::Services {
        id_by_name: |_| None,
        profile: |id| {
            THREADS.lock().unwrap().get_or_insert_with(HashSet::new).insert(std::thread::current().id());
            // Profiles 1 and 2 each wait for the load of the other.
            let met = match id {
                1 => {
                    A.store(true, SeqCst);
                    wait(&B)
                }
                2 => {
                    B.store(true, SeqCst);
                    wait(&A)
                }
                _ => true,
            };
            met.then(|| ProfileResult {
                id,
                name: format!("p{id}"),
                properties: Vec::new(),
                forced_name_change: false,
                banned_skin: false,
            })
        },
    };
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let n = 3 * cores + 8;
    let _ = chats(&t);
    for id in 1..=n {
        run(&mut t.s, &format!("fetchprofile id 00000000-0000-0000-0000-{id:012x}"));
    }
    let mut answers = Vec::new();
    let end = Instant::now() + GUARD;
    while answers.len() < n && Instant::now() < end {
        t.s.fetchprofile_poll();
        answers.extend(chats(&t).iter().map(|c| key(c).to_owned()));
        std::thread::sleep(Duration::from_millis(2));
    }
    let ok = answers.iter().filter(|k| *k == "commands.fetchprofile.id.success").count();
    f.eq("all answered", (answers.len(), ok), (n, if cores >= 2 { n } else { n - 1 }));
    let used = THREADS.lock().unwrap().as_ref().map_or(0, HashSet::len);
    f.eq("threads reused", used <= cores, true);
    f.done();
}

#[test]
fn attribute_reset_to_the_type_default() {
    let mut t = open("attribute");
    let mut f = Fails::default();
    run(&mut t.s, "summon minecraft:zombie 2 100 2 {NoAI:1b}");
    let z = "@e[type=minecraft:zombie,limit=1]";
    run(&mut t.s, &format!("attribute {z} minecraft:movement_speed base set 0.5"));
    let _ = chats(&t);
    let r = run(&mut t.s, &format!("attribute {z} minecraft:movement_speed base reset"));
    f.eq("reset", (r, last(&t).1.get(2).cloned()), ((true, 1), Some("0.23000000417232513".into())));
    // The zombie's own default (single-precision 0.23 widened), not the player's 0.1.
    let r = run(&mut t.s, &format!("attribute {z} minecraft:movement_speed base get 1000000000"));
    f.eq("base after reset", r, (true, 230_000_004));
    f.done();
}

#[test]
fn loot_insert_and_replace_like_the_game() {
    let mut t = open("loot");
    let mut f = Fails::default();
    // Chest full of stone stacks: inserting a stone counts as a change (game quirk).
    run(&mut t.s, "setblock 1 200 1 minecraft:chest");
    let items: Vec<String> = (0..27).map(|i| format!("{{Slot:{i}b,id:\"minecraft:stone\",count:64}}")).collect();
    run(&mut t.s, &format!("data merge block 1 200 1 {{Items:[{}]}}", items.join(",")));
    let r = run(&mut t.s, &format!("loot insert 1 200 1 loot {STONE}"));
    f.eq("insert into a full chest", r, (true, 1));
    // Past the end of the chest: the game's index error stops the command, the slots written stay.
    run(&mut t.s, "setblock 3 200 1 minecraft:chest");
    let _ = chats(&t);
    let r = run(&mut t.s, &format!("loot replace block 3 200 1 container.26 2 loot {STONE}"));
    let all = chats(&t);
    let tip = all.last().and_then(|c| c.style.hover.clone()).map(|h| match &*h {
        rc_text::HoverEvent::Text(c) => crate::commands::cmds_ents::plain(c),
        _ => String::new(),
    });
    f.eq(
        "past the end",
        (r, all.last().map(|c| key(c).to_owned()), tip),
        ((false, 0), Some("command.failed".into()), Some("Index 27 out of bounds for length 27".into())),
    );
    f.eq("slot 26 written", t.s.slot_get(Slot::Block([3, 200, 1], 26)).map(|s| s.id), Some("minecraft:stone".into()));
    // Furnace result slot: refused.
    run(&mut t.s, "setblock 5 200 1 minecraft:furnace");
    let r = run(&mut t.s, &format!("loot replace block 5 200 1 container.2 loot {STONE}"));
    f.eq("furnace result slot", (r.1, t.s.slot_get(Slot::Block([5, 200, 1], 2))), (0, None));
    // Lectern and campfire are not containers.
    run(&mut t.s, "setblock 7 200 1 minecraft:campfire");
    let _ = chats(&t);
    let r = run(&mut t.s, "item replace block 7 200 1 container.0 with minecraft:stone");
    f.eq("campfire", (r, last(&t).0), ((false, 0), "commands.item.target.not_a_container".into()));
    // Mob: successive slots from the start one; armor slots take only what goes there.
    run(&mut t.s, "summon minecraft:zombie 2 100 2 {NoAI:1b}");
    let z = "@e[type=minecraft:zombie,limit=1]";
    let two = r#"{pools:[{rolls:1,entries:[{type:"minecraft:item",name:"minecraft:iron_boots"}]},{rolls:1,entries:[{type:"minecraft:item",name:"minecraft:iron_leggings"}]}]}"#;
    let r = run(&mut t.s, &format!("loot replace entity {z} armor.feet 2 loot {two}"));
    f.eq("two slots", r, (true, 2));
    let legs = run(&mut t.s, &format!("execute if items entity {z} armor.legs minecraft:iron_leggings"));
    f.eq("legs from the second item", legs.0, true);
    let r = run(&mut t.s, &format!("loot replace entity {z} armor.chest loot {STONE}"));
    let chest = run(&mut t.s, &format!("execute if items entity {z} armor.chest *"));
    f.eq("stone refused on the chest", (r.1, chest.0), (0, false));
    f.done();
}

#[test]
fn enchant_ride_datapack_debug_spectate() {
    let mut t = open("misc");
    let mut f = Fails::default();
    // Enchant: main hand of a mob.
    run(&mut t.s, "summon minecraft:zombie 2 100 2 {NoAI:1b}");
    let z = "@e[type=minecraft:zombie,limit=1]";
    run(&mut t.s, &format!("item replace entity {z} weapon.mainhand with minecraft:iron_sword"));
    let _ = chats(&t);
    let r = run(&mut t.s, &format!("enchant {z} minecraft:sharpness 2"));
    f.eq("enchant a zombie", (r, last(&t).0), ((true, 1), "commands.enchant.success.single".into()));
    // A mannequin is living: empty hand, then its own hand enchanted; a marker is not living.
    run(&mut t.s, "summon minecraft:mannequin 3 100 3");
    let mq = "@e[type=minecraft:mannequin,limit=1]";
    let _ = chats(&t);
    let r = run(&mut t.s, &format!("enchant {mq} minecraft:sharpness"));
    f.eq("mannequin empty hand", (r, last(&t).0), ((false, 0), "commands.enchant.failed.itemless".into()));
    run(&mut t.s, &format!("item replace entity {mq} weapon.mainhand with minecraft:iron_sword"));
    run(&mut t.s, &format!("item replace entity {mq} armor.body with minecraft:diamond_horse_armor"));
    let _ = chats(&t);
    let r = run(&mut t.s, &format!("enchant {mq} minecraft:sharpness"));
    f.eq("mannequin hand", (r, last(&t).0), ((true, 1), "commands.enchant.success.single".into()));
    let body = run(&mut t.s, &format!("execute if items entity {mq} armor.body minecraft:diamond_horse_armor"));
    f.eq("mannequin body slot", body.0, true);
    run(&mut t.s, "summon minecraft:marker 1 100 1");
    let _ = chats(&t);
    let r = run(&mut t.s, "enchant @e[type=minecraft:marker,limit=1] minecraft:sharpness");
    f.eq("marker not living", (r, last(&t).0), ((false, 0), "commands.enchant.failed.entity".into()));
    // Ride on a marker: refused, rider and vehicle named.
    let _ = chats(&t);
    let r = run(&mut t.s, "ride @s mount @e[type=minecraft:marker,limit=1]");
    let (k, a) = last(&t);
    f.eq("ride marker", (r, k, a.len()), ((false, 0), "commands.ride.mount.failure.generic".into(), 2));
    // Datapack create: reserved name, then rich description written as a component.
    let r = run(&mut t.s, "datapack create con \"x\"");
    f.eq("reserved name", (r, last(&t).0), ((false, 0), "commands.datapack.create.invalid_full_name".into()));
    let r = run(&mut t.s, "datapack create rich {text:\"Hi\",color:\"red\"}");
    let meta = std::fs::read_to_string(t.dir.join("datapacks/rich/pack.mcmeta")).unwrap_or_default();
    let desc = serde_json::from_str::<serde_json::Value>(&meta).ok().map(|v| v["pack"]["description"].clone());
    f.eq("rich description", (r, desc), ((true, 1), Some(serde_json::json!({"text": "Hi", "color": "red"}))));
    // `return run debug function`: refused before tracing.
    let _ = chats(&t);
    let r = run(&mut t.s, "return run debug function t:none");
    f.eq("return run debug", (r, last(&t).0), ((false, 0), "commands.debug.function.noReturnRun".into()));
    // Spectate: a marker is never tracked by the clients.
    t.s.game_mode = 3;
    let _ = chats(&t);
    let r = run(&mut t.s, "spectate @e[type=minecraft:marker,limit=1]");
    f.eq("spectate marker", (r, last(&t).0), ((false, 0), "commands.spectate.cannot_spectate".into()));
    f.done();
}

/// Bench: numeric providers of the shapes the vanilla tables use (uniform, binomial, sum of
/// constants, float uniform) read through the safe entry, and the two hooks added to every tick
/// (screen effects, profile answers) when idle.
/// `cargo test --release -p rc-sim --lib bench_g3f08 -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_g3f08_providers_and_hooks() {
    use serde_json::json;
    use std::hint::black_box;
    use std::time::Instant;
    let mut t = open("bench");
    {
        let no_tag = |_: &str, _: &str| false;
        let loot = crate::loot::Loot { pack: &t.s.life.pack, items: &t.s.life.items, block_tag: &no_tag };
        let ctx = crate::loot::Ctx::default();
        let mut rng = crate::mobs::jrand::JRand::new(1);
        let cases = [
            ("uniform", json!({"type": "minecraft:uniform", "min": 1, "max": 3}), false),
            ("binomial", json!({"type": "minecraft:binomial", "n": 3, "p": 0.5}), false),
            ("sum", json!({"type": "minecraft:add", "inputs": [1, 2, 3]}), false),
            ("float uniform", json!({"type": "minecraft:uniform", "min": 0.0, "max": 1.0}), true),
        ];
        let n = 1_000_000u32;
        for (name, v, float) in &cases {
            let t0 = Instant::now();
            let mut acc = 0i64;
            for _ in 0..n {
                acc += if *float {
                    i64::from(loot.float(black_box(v), &ctx, &mut rng) > 0.5)
                } else {
                    i64::from(loot.int(black_box(v), &ctx, &mut rng))
                };
            }
            eprintln!("provider {name}: {:.1} ns ({acc})", t0.elapsed().as_nanos() as f64 / f64::from(n));
        }
    }
    t.s.post_effects_tick();
    let _ = chats(&t);
    let n = 10_000_000u32;
    let t0 = Instant::now();
    for _ in 0..n {
        black_box(&mut t.s).post_effects_tick();
        black_box(&mut t.s).fetchprofile_poll();
    }
    eprintln!("idle tick hooks: {:.2} ns", t0.elapsed().as_nanos() as f64 / f64::from(n));
    // `fetchprofile id` in bursts (instant fake services): cost of issuing on the server thread,
    // then time until every answer is read.
    t.s.cmd.s9.profiles.services = crate::commands::cmds_s9e::Services {
        id_by_name: |_| None,
        profile: |id| {
            Some(ProfileResult { id, name: "p".into(), properties: Vec::new(), forced_name_change: false, banned_skin: false })
        },
    };
    let burst = 2000usize;
    for pass in 0..2 {
        let cmds: Vec<String> =
            (0..burst).map(|i| format!("fetchprofile id 00000000-0000-0000-0001-{:012x}", pass * burst + i)).collect();
        let t0 = Instant::now();
        for c in &cmds {
            run(&mut t.s, c);
        }
        let issue = t0.elapsed();
        let mut got = 0;
        while got < burst && t0.elapsed().as_secs() < 30 {
            t.s.fetchprofile_poll();
            got += chats(&t).len();
        }
        eprintln!(
            "fetchprofile burst {pass}: issue {:.2} us/command, all {got} answered in {:.1} ms",
            issue.as_nanos() as f64 / burst as f64 / 1000.0,
            t0.elapsed().as_secs_f64() * 1000.0
        );
    }
}
