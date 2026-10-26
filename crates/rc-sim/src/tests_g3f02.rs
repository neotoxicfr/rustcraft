//! G3-F02: mining (hit events, cracks, deferred breaking), block hitting, block
//! pick, player packets (client information, spectator action), breaking (ice,
//! decorated pot, unstable TNT, experience), bucket on a double slab, height message, push
//! upwards. Fresh world created by the generator: only the version data is
//! required (absent: explicit failure).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use crate::input::Input;
use crate::player::Stack;
use crate::server::Server;

struct T {
    s: Server,
    session: rc_client::Session,
    dir: std::path::PathBuf,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const FLOOR: i32 = 199;

fn open(tag: &str) -> T {
    let data = Arc::new(rc_testdata::version_data().expect("version data missing: G3-F02 tests impossible"));
    crate::registries::client_jar(&data).expect("client jar missing: G3-F02 tests impossible");
    let dir = std::env::temp_dir().join(format!("rc-g3f02-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 7;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    crate::tests::solo_slot();
    let uuid = rc_client::offline_uuid("G3F02");
    let (session, peer) = rc_client::memory::session(data.clone(), uuid).expect("session");
    let opts = crate::Options {
        world: dir.clone(),
        name: "G3F02".into(),
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
    s.game_mode = 0;
    s.on_ground = true;
    s.load_now(&[(-1, -1), (-1, 0), (0, -1), (0, 0)]);
    let stone = s.default_state("minecraft:stone");
    let air = s.default_state("minecraft:air");
    for x in -6..=6 {
        for z in -6..=6 {
            s.set_block([x, FLOOR, z], stone, 2);
            for y in FLOOR + 1..=FLOOR + 6 {
                s.set_block([x, y, z], air, 2);
            }
        }
    }
    s.pos = [0.5, f64::from(FLOOR + 1), 0.5];
    s.last_good = s.pos;
    s.yaw = 0.0;
    s.pitch = 0.0;
    s.slots.fill(None);
    let t = T { s, session, dir };
    events(&t);
    t
}

fn events(t: &T) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(e) = t.session.events.try_recv() {
        out.push(e);
    }
    out
}

/// State `minecraft:name[p=v,…]`.
fn st(s: &Server, spec: &str) -> u32 {
    let (name, props) = match spec.split_once('[') {
        Some((n, rest)) => (n, rest.trim_end_matches(']')),
        None => (spec, ""),
    };
    let mut x = s.default_state(name);
    assert!(s.data.block_of(x).is_some_and(|b| b.name == name), "unknown block: {name}");
    for kv in props.split(',').filter(|p| !p.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap();
        x = s.v().set(x, k, v);
    }
    x
}

fn name_at(s: &Server, p: [i32; 3]) -> String {
    crate::blocks::probe::state_string(s, s.get(p))
}

fn hold(s: &mut Server, stack: Option<Stack>) {
    let h = 36 + s.held as usize;
    s.slots[h] = stack;
}

fn item(id: &str) -> Option<Stack> {
    Some(Stack { id: id.into(), count: 1, components: None })
}

fn enchanted(id: &str, ench: &str) -> Option<Stack> {
    let mut s = Stack { id: id.into(), count: 1, components: None };
    crate::items::set_component(&mut s, "minecraft:enchantments", Nbt::Compound(vec![(ench.into(), Nbt::Int(1))]));
    Some(s)
}

/// One tick of mining alone (the server counter advances afterwards, as in `run`).
fn mining_tick(s: &mut Server) {
    s.tick_mining();
    s.tick += 1;
}

fn hit_events(evs: &[Event], p: [i32; 3]) -> Vec<(i32, i32)> {
    evs.iter()
        .filter_map(|e| match e {
            Event::LevelEvent { kind: k @ (2019 | 2020), pos, data, .. } if *pos == p => Some((*k, *data)),
            _ => None,
        })
        .collect()
}

/// Hit in progress: 2019 every tick, 2020 every 4 ticks, data = face hit; the
/// face change (action 1) is tracked; abandoning stops everything; the hitter never
/// receives their own crack.
#[test]
fn mining_emits_hit_events_with_the_face() {
    let mut t = open("hits");
    let p = [1, FLOOR + 1, 0];
    let stone = t.s.default_state("minecraft:stone");
    t.s.set_block(p, stone, 2);
    t.s.tick = 100;
    t.s.joined_tick = 0;
    events(&t);
    t.s.on_action(0, p, 2, 1);
    for _ in 0..8 {
        mining_tick(&mut t.s);
    }
    let evs = events(&t);
    let want = [(2019, 2), (2019, 2), (2019, 2), (2020, 2), (2019, 2), (2019, 2), (2019, 2), (2020, 2)];
    assert_eq!(hit_events(&evs, p), want);
    assert!(!evs.iter().any(|e| matches!(e, Event::BlockDestruction { .. })), "crack sent back to the hitter");
    t.s.on_action(1, p, 4, 2);
    mining_tick(&mut t.s);
    assert_eq!(hit_events(&events(&t), p), [(2019, 4)]);
    t.s.on_action(2, p, 4, 3);
    mining_tick(&mut t.s);
    assert!(hit_events(&events(&t), p).is_empty(), "events after abandoning");
    assert_eq!(t.s.get(p), stone);
}

/// 26.3 quirk: deferred breaking takes the start tick as the duration; player who arrived long
/// ago: breaks at the next tick; player who has just arrived: never.
#[test]
fn delayed_destroy_uses_the_start_tick() {
    let mut t = open("delayed");
    let p = [1, FLOOR + 1, 0];
    let stone = t.s.default_state("minecraft:stone");
    t.s.set_block(p, stone, 2);
    t.s.tick = 1000;
    t.s.mine_ticks = 1000;
    // A portal does not make a new player: their counter continues.
    t.s.reset_view();
    t.s.loaded = true;
    t.s.on_action(0, p, 1, 1);
    t.s.on_action(3, p, 1, 2);
    assert_eq!(t.s.get(p), stone, "broken too early");
    mining_tick(&mut t.s);
    assert!(t.s.v().is_air(t.s.get(p)), "deferred breaking not done at the next tick");

    t.s.set_block(p, stone, 2);
    t.s.mining = None;
    t.s.mine_ticks = 5;
    t.s.on_action(0, p, 1, 3);
    t.s.on_action(3, p, 1, 4);
    for _ in 0..400 {
        mining_tick(&mut t.s);
    }
    assert_eq!(t.s.get(p), stone, "deferred breaking done while progress stays frozen");
}

/// Block hitting: note block (note + statistic), lit redstone ore, dragon egg
/// teleported (event 2015) onto a non-empty block.
#[test]
fn block_attack_on_start() {
    let mut t = open("attack");
    let p = [1, FLOOR + 1, 0];
    let note = st(&t.s, "minecraft:note_block");
    t.s.set_block(p, note, 2);
    t.s.on_action(0, p, 1, 1);
    assert!(t.s.bs.events.iter().any(|e| e.pos == p && e.a == 0), "note not played");
    assert_eq!(t.s.stat_value("minecraft:custom", "minecraft:play_noteblock"), 1);
    t.s.on_action(2, p, 1, 2);

    let ore = st(&t.s, "minecraft:redstone_ore");
    t.s.set_block(p, ore, 2);
    t.s.on_action(0, p, 1, 3);
    assert_eq!(name_at(&t.s, p), "minecraft:redstone_ore[lit=true]");
    t.s.on_action(2, p, 1, 4);

    let egg = st(&t.s, "minecraft:dragon_egg");
    t.s.set_block(p, egg, 2);
    events(&t);
    t.s.on_action(0, p, 1, 5);
    assert!(t.s.v().is_air(t.s.get(p)), "egg stayed in place");
    let evs = events(&t);
    let packed = evs
        .iter()
        .find_map(|e| match e {
            Event::LevelEvent { kind: 2015, pos, data, .. } if *pos == p => Some(*data),
            _ => None,
        })
        .expect("event 2015 missing");
    let to = [p[0] + ((packed >> 16) & 255) - 16, p[1] + ((packed >> 8) & 255) - 8, p[2] + (packed & 255) - 16];
    assert_eq!(t.s.get(to), egg, "egg missing from the announced cell");
    assert!(!t.s.v().is_air(t.s.get([to[0], to[1] - 1, to[2]])), "egg placed in the air");
}

/// Block pick: clone item (wall variants, crops, pots, piston…); block entity data
/// only in creative with Ctrl.
#[test]
fn pick_block_uses_the_clone_item() {
    let mut t = open("pick");
    t.s.game_mode = 1;
    let p = [1, FLOOR + 1, 0];
    let cases = [
        ("minecraft:wall_torch", Some("minecraft:torch")),
        ("minecraft:carrots", Some("minecraft:carrot")),
        ("minecraft:wheat", Some("minecraft:wheat_seeds")),
        ("minecraft:redstone_wire", Some("minecraft:redstone")),
        ("minecraft:potted_dandelion", Some("minecraft:dandelion")),
        ("minecraft:piston_head[type=sticky]", Some("minecraft:sticky_piston")),
        ("minecraft:cave_vines_plant", Some("minecraft:glow_berries")),
        ("minecraft:white_wall_banner", Some("minecraft:white_banner")),
        ("minecraft:water_cauldron", Some("minecraft:cauldron")),
        ("minecraft:oak_wall_sign", Some("minecraft:oak_sign")),
        ("minecraft:stone", Some("minecraft:stone")),
        ("minecraft:water", None),
        ("minecraft:nether_portal", None),
    ];
    for (spec, want) in cases {
        let s = st(&t.s, spec);
        assert_eq!(t.s.clone_item(p, s, false).map(|x| x.id), want.map(str::to_owned), "{spec}");
    }
    // Light: level in the item's state.
    let light = st(&t.s, "minecraft:light[level=7]");
    let c = t.s.clone_item(p, light, false).and_then(|x| x.components).expect("components");
    assert_eq!(c.get("minecraft:block_state").and_then(|b| b.get("level")).and_then(Nbt::as_str), Some("7"));

    // Named chest: data copied with Ctrl in creative only.
    let chest = st(&t.s, "minecraft:chest");
    t.s.set_block(p, chest, 3);
    let be = t.s.be_get_mut(p).expect("chest block entity");
    crate::nbtio::set(be, "CustomName", Nbt::String("box".into()));
    crate::nbtio::set(be, "extra", Nbt::Int(3));
    t.s.pick_block(p, true);
    let held = t.s.slots[36 + t.s.held as usize].clone().expect("picked item");
    assert_eq!(held.id, "minecraft:chest");
    let c = held.components.expect("copied data");
    assert!(c.get("minecraft:custom_name").is_some(), "{c:?}");
    let data = c.get("minecraft:block_entity_data").expect("block_entity_data");
    assert_eq!(data.get("id").and_then(Nbt::as_str), Some("minecraft:chest"));
    assert_eq!(data.get("extra").and_then(Nbt::as_i64), Some(3));
    assert!(data.get("CustomName").is_none(), "name left in the data");
    t.s.slots.fill(None);
    t.s.game_mode = 0;
    t.s.pick_block(p, true);
    assert!(t.s.slots.iter().all(Option::is_none), "item added in survival");
    t.s.game_mode = 1;
    t.s.pick_block(p, false);
    let held = t.s.slots[36 + t.s.held as usize].clone().expect("picked item");
    assert!(held.components.is_none(), "data copied without Ctrl");
}

/// Block pick with Ctrl: each block entity type removes its own fields and carries them
/// as components (sign: texts and wax; head: name; command block: name, and its
/// `conditionMet`/`powered` removed); the replaced sign reads its components back.
#[test]
fn pick_block_moves_type_fields_to_components() {
    let mut t = open("pickbe");
    t.s.game_mode = 1;
    let p = [1, FLOOR + 1, 0];
    let pick = |t: &mut T, spec: &str, fields: Vec<(&str, Nbt)>| -> Nbt {
        let s = st(&t.s, spec);
        t.s.set_block(p, s, 3);
        let be = t.s.be_get_mut(p).unwrap_or_else(|| panic!("block entity of {spec}"));
        for (k, v) in fields {
            crate::nbtio::set(be, k, v);
        }
        t.s.slots.fill(None);
        t.s.pick_block(p, true);
        t.s.slots[36 + t.s.held as usize].clone().and_then(|h| h.components).unwrap_or(Nbt::Compound(Vec::new()))
    };
    let text = |line: &str| {
        let msg = Nbt::String(line.into());
        Nbt::Compound(vec![
            ("messages".into(), Nbt::List(vec![msg.clone(), msg.clone(), msg.clone(), msg])),
            ("color".into(), Nbt::String("black".into())),
            ("has_glowing_text".into(), Nbt::Byte(0)),
        ])
    };
    let c = pick(
        &mut t,
        "minecraft:oak_sign",
        vec![("front_text", text("front")), ("back_text", text("back")), ("is_waxed", Nbt::Byte(1))],
    );
    assert_eq!(c.get("minecraft:sign_text_front"), Some(&text("front")), "{c:?}");
    assert_eq!(c.get("minecraft:sign_text_back"), Some(&text("back")), "{c:?}");
    assert!(c.get("minecraft:waxed").is_some(), "{c:?}");
    assert!(c.get("minecraft:block_entity_data").is_none(), "sign data left: {c:?}");
    // Placed again: the components become the sign's fields again.
    let mut be = Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:sign".into()))]);
    crate::loot::be::apply(&mut be, &c, false);
    assert_eq!(be.get("front_text"), Some(&text("front")));
    assert_eq!(be.get("is_waxed").and_then(Nbt::as_i64), Some(1));
    assert!(be.get("components").is_none(), "sign components not read back: {be:?}");

    let name = Nbt::String("skull".into());
    let c = pick(&mut t, "minecraft:skeleton_skull", vec![("custom_name", name.clone())]);
    assert_eq!(c.get("minecraft:custom_name"), Some(&name), "{c:?}");
    assert!(c.get("minecraft:block_entity_data").is_none(), "head name left: {c:?}");

    let c = pick(
        &mut t,
        "minecraft:command_block",
        vec![
            ("CustomName", name.clone()),
            ("conditionMet", Nbt::Byte(1)),
            ("powered", Nbt::Byte(1)),
            ("Command", Nbt::String("say a".into())),
        ],
    );
    assert_eq!(c.get("minecraft:custom_name"), Some(&name), "{c:?}");
    let data = c.get("minecraft:block_entity_data").expect("command block data");
    assert_eq!(data.get("Command").and_then(Nbt::as_str), Some("say a"));
    for k in ["CustomName", "conditionMet", "powered"] {
        assert!(data.get(k).is_none(), "{k} left: {data:?}");
    }
}

/// Client information: full decoding, metadata 15/16 sent on change,
/// hat in the player list; on arrival, skin parts sent.
#[test]
fn client_information_sets_model_parts() {
    let mut t = open("info");
    let ids = rc_client::ids::Ids::resolve(&t.s.data).expect("identifiers");
    let mut pk = Vec::new();
    rc_proto::varint::write_varint(&mut pk, ids.client_information_out);
    let info = rc_proto_v777::play::ClientInformation {
        view_distance: 7,
        skin_parts: 0x3f,
        main_hand: 0,
        ..Default::default()
    };
    rc_proto_v777::play::write_client_information(&mut pk, &info);
    assert_eq!(
        crate::input::decode(&ids, &pk).expect("decoding"),
        Input::ClientInfo { view: 7, skin_parts: 0x3f, main_hand: 0 }
    );
    let mut pk = Vec::new();
    rc_proto::varint::write_varint(&mut pk, ids.pick_item_from_block);
    rc_proto_v777::interact::write_pick_block(&mut pk, [1, 2, 3], true);
    assert_eq!(crate::input::decode(&ids, &pk).unwrap(), Input::PickBlock { pos: [1, 2, 3], with_data: true });
    let mut pk = Vec::new();
    rc_proto::varint::write_varint(&mut pk, ids.spectator_action);
    rc_proto_v777::interact::write_spectator_action(&mut pk, Some(41));
    assert_eq!(crate::input::decode(&ids, &pk).unwrap(), Input::SpectatorAction(Some(41)));

    t.s.join();
    let id = t.s.player_id;
    let evs = events(&t);
    assert!(
        evs.iter().any(|e| matches!(e, Event::Entity(EntityEvent::Data { id: i, values })
            if *i == id && values == &vec![(16, Meta::Byte(0x7f))])),
        "skin parts missing on arrival"
    );
    t.s.handle(Input::ClientInfo { view: 2, skin_parts: 0x3f, main_hand: 0 });
    let evs = events(&t);
    assert!(
        evs.iter().any(|e| matches!(e, Event::Entity(EntityEvent::Data { id: i, values })
            if *i == id && values == &vec![(15, Meta::Variant(0, 42)), (16, Meta::Byte(0x3f))])),
        "metadata 15/16 missing: {evs:?}"
    );
    let hat = evs.iter().find_map(|e| match e {
        Event::PlayerInfo(u) if u.actions == rc_proto_v777::players::action::UPDATE_HAT => u.entries[0].show_hat,
        _ => None,
    });
    assert_eq!(hat, Some(false), "hat not updated");
    t.s.handle(Input::ClientInfo { view: 2, skin_parts: 0x3f, main_hand: 0 });
    let evs = events(&t);
    assert!(
        !evs.iter().any(|e| matches!(e, Event::Entity(EntityEvent::Data { .. }) | Event::PlayerInfo(_))),
        "resend without change"
    );
}

/// Spectator action: camera on a mob in range; refused outside spectator.
#[test]
fn spectator_action_sets_the_camera() {
    let mut t = open("spectate");
    let mut src = t.s.player_source();
    src.level = 4;
    assert!(t.s.perform(src.clone(), "summon minecraft:pig 2.5 200 0.5 {NoAI:1b}").0);
    let pig =
        t.s.mobs
            .list
            .iter()
            .flatten()
            .find(|m| m.kind == crate::mobs::species::Kind::Pig)
            .map(|m| m.id)
            .expect("pig");
    events(&t);
    t.s.on_spectator_action(Some(pig));
    assert!(!events(&t).iter().any(|e| matches!(e, Event::SetCamera(_))), "camera outside spectator");
    t.s.game_mode = 3;
    t.s.on_spectator_action(Some(pig));
    assert!(events(&t).iter().any(|e| matches!(e, Event::SetCamera(i) if *i == pig)), "camera not set");

    // Armor stand: selectable, except as a marker.
    assert!(t.s.perform(src.clone(), "summon minecraft:armor_stand 1.5 200 1.5 {NoGravity:1b}").0);
    assert!(t.s.perform(src, "summon minecraft:armor_stand 1.5 200 -0.5 {NoGravity:1b,Marker:1b}").0);
    let stands: Vec<(i32, bool)> = t
        .s
        .all_ents()
        .into_iter()
        .filter(|e| t.s.ent_type(*e) == "minecraft:armor_stand")
        .map(|e| (t.s.ent_id(e), t.s.ent_nbt(e).get("Marker").and_then(Nbt::as_i64) == Some(1)))
        .collect();
    assert_eq!(stands.len(), 2, "{stands:?}");
    for (id, marker) in stands {
        events(&t);
        t.s.on_spectator_action(Some(id));
        let set = events(&t).iter().any(|e| matches!(e, Event::SetCamera(i) if *i == id));
        assert_eq!(set, !marker, "armor stand {id} (marker: {marker})");
    }
}

/// Ice broken in survival: water above a full block, air above the void or with
/// Silk Touch, nothing in creative.
#[test]
fn broken_ice_turns_to_water() {
    let mut t = open("ice");
    let ice = st(&t.s, "minecraft:ice");
    let p = [1, FLOOR + 1, 0];
    t.s.set_block(p, ice, 2);
    t.s.try_break(p);
    assert_eq!(name_at(&t.s, p), "minecraft:water[level=0]");
    t.s.set_block(p, ice, 2);
    hold(&mut t.s, enchanted("minecraft:iron_pickaxe", "minecraft:silk_touch"));
    t.s.try_break(p);
    assert_eq!(name_at(&t.s, p), "minecraft:air", "Silk Touch");
    hold(&mut t.s, None);
    let high = [1, FLOOR + 2, 0];
    t.s.set_block(high, ice, 2);
    t.s.try_break(high);
    assert_eq!(name_at(&t.s, high), "minecraft:air", "ice above air");
    t.s.game_mode = 1;
    t.s.set_block(p, ice, 2);
    t.s.try_break(p);
    assert_eq!(name_at(&t.s, p), "minecraft:air", "creative");
}

/// Water bucket aimed at a double slab: the slab stays dry, the water goes in front; a
/// bottom slab, on the other hand, is waterlogged. The two bucket paths of the server.
#[test]
fn water_bucket_skips_double_slab() {
    let mut t = open("slab");
    let under = [0, FLOOR, 0];
    let above = [0, FLOOR + 1, 0];
    t.s.pitch = 90.0;
    for path in 0..2 {
        let double = st(&t.s, "minecraft:oak_slab[type=double]");
        let air = t.s.default_state("minecraft:air");
        t.s.set_block(under, double, 2);
        t.s.set_block(above, air, 2);
        hold(&mut t.s, item("minecraft:water_bucket"));
        let used = if path == 0 { t.s.ray_item_use(0) } else { t.s.w8c_use_item(0) };
        assert!(used, "path {path}");
        assert_eq!(name_at(&t.s, under), "minecraft:oak_slab[type=double,waterlogged=false]", "path {path}");
        assert_eq!(name_at(&t.s, above), "minecraft:water[level=0]", "path {path}");
        let bottom = st(&t.s, "minecraft:oak_slab[type=bottom]");
        t.s.set_block(under, bottom, 2);
        t.s.set_block(above, air, 2);
        hold(&mut t.s, item("minecraft:water_bucket"));
        if path == 0 {
            t.s.ray_item_use(0);
        } else {
            t.s.w8c_use_item(0);
        }
        assert_eq!(name_at(&t.s, under), "minecraft:oak_slab[type=bottom,waterlogged=true]", "path {path}");
    }
}

fn drops_of(s: &Server, id: &str) -> i32 {
    s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == id).map(|d| d.stack.count).sum()
}

fn xp(s: &Server) -> i32 {
    s.life.orbs.iter().map(|o| o.value * o.count).sum()
}

/// Decorated pot broken with a pickaxe: cracked, loot as shards; by hand: the pot.
#[test]
fn decorated_pot_cracks_when_mined_with_a_tool() {
    let mut t = open("pot");
    let p = [1, FLOOR + 1, 0];
    let pot = st(&t.s, "minecraft:decorated_pot");
    let sherds = |t: &mut T| {
        let be = t.s.be_get_mut(p).expect("pot block entity");
        let side = |id: &str| Nbt::String(id.into());
        let list = vec![
            side("minecraft:brick"),
            side("minecraft:angler_pottery_sherd"),
            side("minecraft:brick"),
            side("minecraft:brick"),
        ];
        crate::nbtio::set(be, "sherds", Nbt::List(list));
    };
    t.s.set_block(p, pot, 3);
    sherds(&mut t);
    hold(&mut t.s, item("minecraft:iron_pickaxe"));
    t.s.try_break(p);
    assert_eq!(drops_of(&t.s, "minecraft:angler_pottery_sherd"), 1, "shard");
    assert_eq!(drops_of(&t.s, "minecraft:decorated_pot"), 0);
    t.s.set_block(p, pot, 3);
    sherds(&mut t);
    hold(&mut t.s, enchanted("minecraft:iron_pickaxe", "minecraft:silk_touch"));
    t.s.try_break(p);
    assert_eq!(drops_of(&t.s, "minecraft:decorated_pot"), 1, "whole pot with Silk Touch");
    t.s.set_block(p, pot, 3);
    sherds(&mut t);
    hold(&mut t.s, None);
    t.s.try_break(p);
    assert_eq!(drops_of(&t.s, "minecraft:decorated_pot"), 2, "whole pot by hand");
}

/// Experience: sculk 1, catalyst 5, nothing with Silk Touch; spawner 15 to 43, even
/// with Silk Touch.
#[test]
fn sculk_and_spawner_drop_experience() {
    let mut t = open("xp");
    let p = [1, FLOOR + 1, 0];
    let take = |t: &mut T, spec: &str, tool: Option<Stack>| {
        t.s.life.orbs.clear();
        let s = st(&t.s, spec);
        t.s.set_block(p, s, 3);
        hold(&mut t.s, tool);
        t.s.try_break(p);
        xp(&t.s)
    };
    assert_eq!(take(&mut t, "minecraft:sculk", None), 1);
    assert_eq!(take(&mut t, "minecraft:sculk_catalyst", None), 5);
    assert_eq!(take(&mut t, "minecraft:sculk_shrieker", None), 5);
    assert_eq!(take(&mut t, "minecraft:sculk", enchanted("minecraft:iron_hoe", "minecraft:silk_touch")), 0);
    for _ in 0..5 {
        let n = take(&mut t, "minecraft:spawner", item("minecraft:iron_pickaxe"));
        assert!((15..=43).contains(&n), "spawner: {n}");
    }
    let n = take(&mut t, "minecraft:spawner", enchanted("minecraft:iron_pickaxe", "minecraft:silk_touch"));
    assert!((15..=43).contains(&n), "spawner with Silk Touch: {n}");
}

/// Unstable TNT broken in survival: primed; in creative: not.
#[test]
fn unstable_tnt_primes_when_broken() {
    let mut t = open("tnt");
    let p = [1, FLOOR + 1, 0];
    let tnt = st(&t.s, "minecraft:tnt[unstable=true]");
    let before = t.s.bs.entities.list.len();
    t.s.set_block(p, tnt, 2);
    t.s.try_break(p);
    assert_eq!(t.s.bs.entities.list.len(), before + 1, "TNT not primed");
    t.s.game_mode = 1;
    t.s.set_block(p, tnt, 2);
    t.s.try_break(p);
    assert_eq!(t.s.bs.entities.list.len(), before + 1, "primed in creative");
}

/// Placement on the top face at the ceiling: "too high" with the max height, in red, twice;
/// teleport pending: nothing used, "too high".
#[test]
fn build_limit_message_is_red_with_the_right_limit() {
    let mut t = open("limit");
    let top = t.s.top_y() - 1;
    let p = [1, top, 0];
    let stone = t.s.default_state("minecraft:stone");
    t.s.load_now(&[(0, 0)]);
    t.s.set_block(p, stone, 2);
    t.s.pos = [0.5, f64::from(top - 1), 0.5];
    hold(&mut t.s, item("minecraft:stone"));
    events(&t);
    t.s.on_use_item_on(0, p, 1, [0.5, 1.0, 0.5], false, 1);
    let mut want = rc_text::Component::translate("build.tooHigh", vec![rc_text::Arg::Text(top.to_string())]);
    want.style.color = Some(0xFF5555);
    let bars: Vec<_> =
        events(&t).into_iter().filter_map(|e| if let Event::ActionBar(c) = e { Some(c) } else { None }).collect();
    assert_eq!(bars, vec![want.clone(), want.clone()]);

    let q = [1, FLOOR + 1, 0];
    t.s.pos = [0.5, f64::from(FLOOR + 1), 0.5];
    let lever = st(&t.s, "minecraft:lever[face=floor]");
    t.s.set_block(q, lever, 2);
    t.s.teleport = Some((9, t.s.pos, t.s.tick));
    hold(&mut t.s, None);
    events(&t);
    t.s.on_use_item_on(0, q, 1, [0.5, 0.5, 0.5], false, 2);
    assert_eq!(t.s.get(q), lever, "lever operated despite the pending teleport");
    let bars: Vec<_> =
        events(&t).into_iter().filter_map(|e| if let Event::ActionBar(c) = e { Some(c) } else { None }).collect();
    assert_eq!(bars.len(), 1);
    assert!(matches!(&bars[0].content, rc_text::Content::Translate { key, .. } if key == "build.tooHigh"), "{bars:?}");
}

/// Push upwards (snow that thickens, farmland that becomes dirt again): the
/// player is placed on the new top by a relative teleport.
#[test]
fn growing_collision_pushes_entities_up() {
    let mut t = open("push");
    let p = [0, FLOOR + 1, 0];
    t.s.pos = [0.5, f64::from(FLOOR + 1) + 0.125, 0.5];
    events(&t);
    t.s.push_entities_up(p, 1.0 / 8.0, 2.0 / 8.0);
    assert!((t.s.pos[1] - (f64::from(FLOOR + 1) + 0.25)).abs() < 1e-9, "{}", t.s.pos[1]);
    let tp = events(&t)
        .into_iter()
        .find_map(|e| if let Event::Teleport(p) = e { Some(p) } else { None })
        .expect("teleport");
    assert_eq!(tp.relative, 0x1ff);
    assert!((tp.pos[1] - 0.125).abs() < 1e-9 && tp.pos[0] == 0.0 && tp.pos[2] == 0.0, "{tp:?}");
    // Player above the gained slice: nothing.
    t.s.pos = [0.5, f64::from(FLOOR + 1) + 0.5, 0.5];
    t.s.push_entities_up(p, 2.0 / 8.0, 3.0 / 8.0);
    assert!((t.s.pos[1] - (f64::from(FLOOR + 1) + 0.5)).abs() < 1e-9);
    // Spectator in the gained slice: never pushed, no pending teleport.
    t.s.game_mode = 3;
    t.s.teleport = None;
    t.s.pos = [0.5, f64::from(FLOOR + 1) + 0.125, 0.5];
    events(&t);
    t.s.push_entities_up(p, 1.0 / 8.0, 2.0 / 8.0);
    assert!((t.s.pos[1] - (f64::from(FLOOR + 1) + 0.125)).abs() < 1e-9, "spectator pushed: {}", t.s.pos[1]);
    assert!(t.s.teleport.is_none(), "teleport sent to the spectator");
    assert!(!events(&t).iter().any(|e| matches!(e, Event::Teleport(_))));
}
