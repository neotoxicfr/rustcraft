//! Registry entries of a world's data pack: they carry their contents (their stacked JSON as NBT)
//! to a LAN guest as to the host's client, the base pack's entries go without contents for a
//! client that confirmed the known packs and with them otherwise, and the server reads the
//! paintings' sizes from the pack stack. Fresh flat world with a data pack that adds a painting
//! and changes one of the game's: the 26.3 data cache and the client jar are required (absent:
//! FAILURE).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rc_client::Event;
use rc_proto::nbt::Nbt;

use crate::blocks::probe::Probe;
use crate::newworld::{NewWorld, Preset, create_world};

const PACK_META: &str = r#"{"pack":{"description":"registry contents tests","min_format":121,"max_format":121}}"#;
const BIG_PAINTING: &str = r#"{"asset_id":"p:big","width":4,"height":3,"title":{"text":"Big"}}"#;
const WIDE_KEBAB: &str = r#"{"asset_id":"minecraft:kebab","width":2,"height":1}"#;

fn write_pack_file(world: &Path, path: &str, text: &str) {
    let file = world.join("datapacks").join("p").join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

/// Fresh flat world whose data pack adds painting `p:big` (4 × 3) and widens `kebab` to 2 × 1.
fn world_with_paintings(tag: &str) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("registry contents tests: 26.3 data cache missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "registry contents tests: 26.3 client jar missing");
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-registry-contents-{tag}-{}", std::process::id())));
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    create_world(&dir, &data, &w).expect("world creation");
    write_pack_file(&dir, "pack.mcmeta", PACK_META);
    write_pack_file(&dir, "data/p/painting_variant/big.json", BIG_PAINTING);
    write_pack_file(&dir, "data/minecraft/painting_variant/kebab.json", WIDE_KEBAB);
    let p = Probe::open(&dir, data);
    let _ = p.take_events();
    (p, dir)
}

/// Index of the painting registry and of entry `id` in it.
fn painting(p: &Probe, id: &str) -> (usize, usize) {
    let r = p.s.regs.synced.iter().position(|(n, _)| n == "minecraft:painting_variant").expect("painting registry");
    let e = p.s.regs.synced[r].1.iter().position(|n| n == id).unwrap_or_else(|| panic!("entry {id}"));
    (r, e)
}

fn width(n: &Nbt) -> Option<i64> {
    n.get("width").and_then(Nbt::as_i64)
}

fn core() -> Vec<String> {
    vec![crate::registries::CORE_PACK.to_owned()]
}

/// The pack's new and changed paintings carry their contents for a client that knows the base
/// pack; the game's own go without; for a client that knows no pack, they all carry theirs.
/// Fails on the code before (no entry ever carried contents); sabotage verified (failure): every
/// entry taken as the base pack's.
#[test]
fn pack_entries_carry_their_contents() {
    let (p, _dir) = world_with_paintings("contents");
    let contents = &p.s.regs.contents;
    let (r, big) = painting(&p, "p:big");
    let (_, kebab) = painting(&p, "minecraft:kebab");
    let (_, alban) = painting(&p, "minecraft:alban");
    assert_eq!(contents.contents_for(r, big, &core()).as_ref().and_then(width), Some(4), "new painting");
    assert_eq!(contents.contents_for(r, kebab, &core()).as_ref().and_then(width), Some(2), "changed painting");
    assert_eq!(contents.contents_for(r, alban, &core()), None, "the game's painting, known");
    assert_eq!(contents.contents_for(r, alban, &[]).as_ref().and_then(width), Some(1), "unknown packs: everything");
}

/// The server sizes the pack's paintings from the stack: the new one 4 × 3, the changed one
/// 2 × 1. Fails on the code before (sizes from the game's extracted table: none for `p:big`,
/// 1 × 1 for `kebab`).
#[test]
fn painting_sizes_come_from_the_pack_stack() {
    let (p, _dir) = world_with_paintings("sizes");
    assert_eq!(p.s.variant_size("p:big"), Some((4, 3)));
    assert_eq!(p.s.variant_size("minecraft:kebab"), Some((2, 1)));
    assert_eq!(p.s.variant_size("minecraft:alban"), Some((1, 1)));
}

/// The LAN `registry_data` of the paintings, read back by the client's decoder: the pack's
/// entries with their contents, the game's without, for a guest that confirmed the known packs;
/// a guest whose answer differs from the question gets every contents. Fails on the code before
/// (every entry written without contents; a guest without the base pack was refused).
#[test]
fn lan_registry_data_carries_pack_contents() {
    use crate::lan::conn::{negotiated_known_packs, write_registry_data};
    let (p, _dir) = world_with_paintings("lan");
    let (r, big) = painting(&p, "p:big");
    let (_, alban) = painting(&p, "minecraft:alban");
    let asked = vec!["core".to_owned(), "trade_rebalance".to_owned()];
    let same = [("minecraft".into(), "core".into(), "26.3".into()), ("minecraft".into(), "trade_rebalance".into(), "26.3".into())];
    assert_eq!(negotiated_known_packs(&asked, "26.3", &same), asked.as_slice());
    assert!(negotiated_known_packs(&asked, "26.3", &same[..1]).is_empty(), "a partial answer agrees on nothing");
    let read = |known: &[String]| {
        let (name, entries) = &p.s.regs.synced[r];
        let mut w = Vec::new();
        write_registry_data(&mut w, name, entries, &p.s.regs.contents, r, known);
        let mut rd = rc_proto::Reader::new(&w);
        let data = rc_proto_v777::config::read_registry_data(&mut rd).expect("registry data read back");
        assert!(rd.is_empty(), "nothing left after the entries");
        data.entries
    };
    let confirmed = read(&core());
    assert_eq!(confirmed[big].1.as_ref().and_then(width), Some(4), "pack entry with its contents");
    assert_eq!(confirmed[alban].1, None, "known entry without contents");
    let unconfirmed = read(&[]);
    assert!(unconfirmed.iter().all(|(_, n)| n.is_some()), "every entry with its contents");
}

/// The host's client gets the paintings with the pack's contents, and the other registries it
/// reads (damage types among them). Fails on the code before (contents never sent, damage types
/// never handed to the host's client).
#[test]
fn host_client_gets_pack_contents() {
    let (mut p, _dir) = world_with_paintings("host");
    p.s.join();
    let events = p.take_events();
    let registry = |name: &str| {
        events.iter().find_map(|e| match e {
            Event::Registry { name: n, entries } if n == name => Some(entries.clone()),
            _ => None,
        })
    };
    let paintings = registry("minecraft:painting_variant").expect("painting registry sent");
    let contents = |id: &str| paintings.iter().find(|(n, _)| n == id).and_then(|(_, c)| c.clone());
    assert_eq!(contents("p:big").as_ref().and_then(width), Some(4));
    assert_eq!(contents("minecraft:alban"), None);
    assert!(registry("minecraft:damage_type").is_some_and(|e| !e.is_empty()), "damage types sent");
}
