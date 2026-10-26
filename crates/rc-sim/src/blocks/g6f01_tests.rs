//! G6-F01: click blocks of the integrated server (jukebox, lectern, decorated pot, campfire,
//! chiseled bookshelf, shelves, copper golem statue, dragon egg, note block,
//! light block, water bottle on dirt, sounds of the bulb and the dried ghast, loot of
//! redstone dust and repeaters deprived of support). Fresh flat world: no external data
//! other than the 26.3 cache and the client jar, whose absence makes the test FAIL.

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::Event;
use rc_proto::nbt::Nbt;

use super::*;
use crate::blocks::probe::{Probe, state_string};
use crate::player::Stack;

/// Fresh flat world (commands allowed), grass floor at y = −61, player at the center.
pub(super) struct W {
    pub(super) p: Probe,
    dir: PathBuf,
}

impl Drop for W {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub(super) const Y: i32 = -60;

pub(super) fn world(tag: &str) -> W {
    let data = rc_testdata::version_data().expect("G6-F01: 26.3 data cache missing");
    let data = Arc::new(data);
    assert!(crate::registries::client_jar(&data).is_some(), "G6-F01: 26.3 client jar missing");
    let dir = std::env::temp_dir().join(format!("rc-g6f01-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 7;
    w.preset = crate::newworld::Preset::Flat;
    w.allow_commands = true;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    // Chunks −2..2 generated (the block entities and ticks of chunk 0 want their neighbours).
    let want: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    p.s.ensure_loaded(&want);
    let t = std::time::Instant::now();
    while !want.iter().all(|c| p.s.chunks.get(c).is_some_and(Option::is_some)) {
        assert!(t.elapsed().as_secs() < 120, "G6-F01: flat world generation too slow");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    p.run(&format!("fill 0 {Y} 0 15 {} 15 minecraft:air\n", Y + 3));
    p.s.pos = [8.5, f64::from(Y), 8.5];
    p.s.game_mode = 0;
    let _ = p.take_events();
    W { p, dir }
}

impl W {
    pub(super) fn name(&self, at: P) -> String {
        state_string(&self.p.s, self.p.s.get(at))
    }

    pub(super) fn set(&mut self, at: P, state: &str) {
        self.p.run(&format!("setblock {} {} {} {state}\n", at[0], at[1], at[2]));
    }

    /// Real player command (full parsing and execution); test fails if refused.
    pub(super) fn cmd(&mut self, text: &str) {
        let src = self.p.s.player_source();
        let (ok, _) = self.p.s.perform(src, text.trim_end());
        assert!(ok, "command refused: {text}");
    }

    pub(super) fn hold(&mut self, s: Option<Stack>) {
        let slot = 36 + self.p.s.held as usize;
        self.p.s.slots[slot] = s;
    }

    pub(super) fn held(&self) -> Option<Stack> {
        self.p.s.slots[36 + self.p.s.held as usize].clone()
    }

    pub(super) fn click(&mut self, at: P, face: Dir, hit: [f32; 3]) {
        self.p.s.use_item_on_block(at, face, hit);
    }

    /// Block ticks (events kept).
    pub(super) fn ticks(&mut self, n: i32, out: &mut Vec<Event>) {
        for _ in 0..n {
            self.p.s.time += 1;
            self.p.s.tick_world_blocks();
            self.p.s.flush_block_changes();
            out.extend(self.p.take_events());
        }
    }

    pub(super) fn events(&mut self) -> Vec<Event> {
        self.p.s.flush_block_changes();
        self.p.take_events()
    }

    pub(super) fn sounds(&self, evs: &[Event], name: &str) -> usize {
        let id = self.p.s.sound_id(name).expect("known sound");
        evs.iter()
            .filter(
                |e| matches!(e, Event::Sound(s) if matches!(s.sound, rc_proto_v777::play::SoundRef::Id(i) if i == id)),
            )
            .count()
    }

    pub(super) fn comparator(&self, at: P, from: Dir) -> i32 {
        let s = self.p.s.get(at);
        self.p.s.analog_output(s, at, from)
    }

    fn items_on_ground(&self, id: &str) -> i32 {
        self.p.s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == id).map(|d| d.stack.count).sum()
    }
}

pub(super) fn stack(id: &str, count: i32) -> Option<Stack> {
    Some(Stack { id: id.to_owned(), count, components: None })
}

fn level_events(evs: &[Event], kind: i32) -> Vec<i32> {
    evs.iter()
        .filter_map(|e| match e {
            Event::LevelEvent { kind: k, data, .. } if *k == kind => Some(*data),
            _ => None,
        })
        .collect()
}

pub(super) fn particles(w: &W, evs: &[Event], name: &str) -> Vec<rc_proto_v777::entity::LevelParticles> {
    let kind = w.p.s.data.registry("minecraft:particle_type").unwrap().entries.iter().position(|e| e == name).unwrap();
    evs.iter()
        .filter_map(|e| match e {
            Event::Particles(p) if p.particle.kind == kind as i32 => Some(p.clone()),
            _ => None,
        })
        .collect()
}

/// Jukebox: disc inserted (1010 track index, signal 15, comparator of the track,
/// notes every 20 ticks), ejected with an empty hand (1011, item), end of the track alone.
#[test]
fn jukebox_plays_and_ejects() {
    let mut w = world("jukebox");
    let j = [4, Y, 4];
    w.set(j, "minecraft:jukebox");
    w.hold(stack("minecraft:music_disc_13", 2));
    w.click(j, Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(w.name(j), "minecraft:jukebox[has_record=true]");
    assert_eq!(w.held().map(|s| s.count), Some(1), "one disc taken");
    let evs = w.events();
    let idx = w.p.s.regs.entries("minecraft:jukebox_song").iter().position(|e| e == "minecraft:13").unwrap();
    assert_eq!(level_events(&evs, 1010), vec![idx as i32], "1010 with the index of track 13");
    assert_eq!(w.p.s.prog.stats.get("minecraft:custom", "minecraft:play_record"), 1);
    // Signal 15 during playback (weak, towards the neighbours); comparator: output of 13 = 1.
    assert_eq!(w.p.s.weak(w.p.s.get(j), j, Dir::Up), 15);
    assert_eq!(w.comparator(j, Dir::North), 1);
    // Notes: t = 0 then every 20 ticks (y = block + 1.2f).
    let mut evs = Vec::new();
    w.ticks(41, &mut evs);
    let notes = particles(&w, &evs, "minecraft:note");
    assert_eq!(notes.len(), 3, "notes at 0, 20, 40");
    assert_eq!(notes[0].pos[1], f64::from(Y) + f64::from(1.2f32));
    // Empty hand: disc ejected, stop.
    w.hold(None);
    w.click(j, Dir::Up, [0.5, 1.0, 0.5]);
    let evs = w.events();
    assert_eq!(w.name(j), "minecraft:jukebox[has_record=false]");
    assert_eq!(level_events(&evs, 1011).len(), 1, "1011 on stop");
    assert_eq!(w.items_on_ground("minecraft:music_disc_13"), 1);
    assert_eq!(w.p.s.weak(w.p.s.get(j), j, Dir::Up), 0);
    // Track that ends by itself: 11 (71 s → 1420 + 20 ticks).
    w.hold(stack("minecraft:music_disc_11", 1));
    w.click(j, Dir::Up, [0.5, 1.0, 0.5]);
    let mut evs = Vec::new();
    w.ticks(1440, &mut evs);
    assert!(level_events(&evs, 1011).is_empty(), "not finished yet at 1440");
    w.ticks(1, &mut evs);
    assert_eq!(level_events(&evs, 1011).len(), 1, "end at ceil(71 × 20) + 20");
    assert_eq!(w.name(j), "minecraft:jukebox[has_record=true]", "the disc stays");
    // Breaking: disc given back.
    w.p.s.destroy_block(j, false, 512);
    assert_eq!(w.items_on_ground("minecraft:music_disc_11"), 1, "disc dropped on breaking");
}

fn book(pages: usize) -> Option<Stack> {
    let list = (0..pages).map(|i| Nbt::Compound(vec![("raw".into(), Nbt::String(format!("p{i}")))])).collect();
    Some(Stack {
        id: "minecraft:writable_book".into(),
        count: 1,
        components: Some(Nbt::Compound(vec![(
            "minecraft:writable_book_content".into(),
            Nbt::Compound(vec![("pages".into(), Nbt::List(list))]),
        )])),
    })
}

/// Lectern: book placed (sound), menu opened (statistic), next page (2-tick pulse,
/// 1043, comparator), book taken back (button 3).
#[test]
fn lectern_book_pages_and_signal() {
    let mut w = world("lectern");
    let l = [6, Y, 6];
    w.set(l, "minecraft:lectern[facing=north]");
    w.hold(None);
    w.click(l, Dir::Up, [0.5, 1.0, 0.5]);
    assert!(w.p.s.life.menu.is_none(), "no book: nothing");
    w.hold(book(3));
    w.click(l, Dir::Up, [0.5, 1.0, 0.5]);
    let evs = w.events();
    assert_eq!(w.name(l), "minecraft:lectern[facing=north,has_book=true,powered=false]");
    assert!(w.held().is_none(), "book placed");
    assert_eq!(w.sounds(&evs, "minecraft:item.book.put"), 1);
    assert_eq!(w.comparator(l, Dir::North), 1, "page 0 of 3: 0 + 1");
    w.hold(None);
    w.click(l, Dir::Up, [0.5, 1.0, 0.5]);
    let window = w.p.s.life.menu.as_ref().map(|m| (m.window, m.menu_type()));
    assert_eq!(window.map(|x| x.1), Some(super::lectern::MENU_TYPE), "lectern menu");
    assert_eq!(w.p.s.prog.stats.get("minecraft:custom", "minecraft:interact_with_lectern"), 1);
    w.p.s.lectern_button(2);
    let evs = w.events();
    assert_eq!(level_events(&evs, 1043).len(), 1, "page turned");
    assert!(w.p.s.v().get_bool(w.p.s.get(l), "powered"), "pulse");
    assert_eq!(w.p.s.strong(w.p.s.get(l), l, Dir::Up), 15, "strong downwards");
    assert_eq!(w.comparator(l, Dir::North), 8, "page 1 of 3: floor(0.5 × 14) + 1");
    let mut evs = Vec::new();
    w.ticks(1, &mut evs);
    assert!(w.p.s.v().get_bool(w.p.s.get(l), "powered"), "still powered at 1 tick");
    w.ticks(1, &mut evs);
    assert!(!w.p.s.v().get_bool(w.p.s.get(l), "powered"), "end of the pulse at 2 ticks");
    w.p.s.lectern_button(3);
    assert_eq!(w.name(l), "minecraft:lectern[facing=north,has_book=false,powered=false]");
    assert!(w.p.s.slots.iter().flatten().any(|s| s.id == "minecraft:writable_book"), "book given back");
}

/// Decorated pot: items deposited (wobble, sound, dust, comparator), another
/// item refused, empty hand (failure sound), item given back when broken.
#[test]
fn decorated_pot_insert() {
    let mut w = world("pot");
    let at = [3, Y, 8];
    w.set(at, "minecraft:decorated_pot");
    w.hold(stack("minecraft:diamond", 10));
    w.click(at, Dir::Up, [0.5, 1.0, 0.5]);
    w.click(at, Dir::Up, [0.5, 1.0, 0.5]);
    let mut evs = w.events();
    w.ticks(1, &mut evs);
    assert_eq!(w.held().map(|s| s.count), Some(8));
    assert_eq!(w.sounds(&evs, "minecraft:block.decorated_pot.insert"), 2);
    let dust = particles(&w, &evs, "minecraft:dust_plume");
    assert_eq!(dust.len(), 2);
    let top = [f64::from(at[0]) + 0.5, f64::from(at[1]) + 1.2, f64::from(at[2]) + 0.5];
    assert!(dust.iter().all(|d| d.count == 7 && d.pos == top), "7 dust plume particles above the pot: {dust:?}");
    assert!(evs.iter().any(|e| matches!(e, Event::BlockEvent { pos, b0: 1, b1: 0, .. } if *pos == at)), "wobble");
    assert_eq!(w.comparator(at, Dir::North), 1, "2/64: 1");
    w.hold(stack("minecraft:emerald", 1));
    w.click(at, Dir::Up, [0.5, 1.0, 0.5]);
    w.hold(None);
    w.click(at, Dir::Up, [0.5, 1.0, 0.5]);
    let evs = w.events();
    // Other item: attempt fails; empty hand: failure as well.
    assert_eq!(w.sounds(&evs, "minecraft:block.decorated_pot.insert_fail"), 2, "two failures");
    assert_eq!(w.held(), None);
    w.p.s.destroy_block(at, false, 512);
    assert_eq!(w.items_on_ground("minecraft:diamond"), 2, "contents scattered");
}

/// Campfire: food placed (statistic), cooked after the recipe duration and
/// dropped; shovel that puts it out (1009, wear); water bucket that puts it out too.
#[test]
fn campfire_cooks_and_douses() {
    let mut w = world("campfire");
    let c = [10, Y, 10];
    w.set(c, "minecraft:campfire[lit=true]");
    w.hold(stack("minecraft:beef", 2));
    w.click(c, Dir::Up, [0.5, 0.5, 0.5]);
    // Contents sent back to the client with the state change: only
    // `Items` (the test world does not send its chunks, the update is re-read here).
    assert!(w.p.s.changed.contains(&c), "change announced");
    let tag = w.p.s.be_update_tag(c, w.p.s.get(c)).expect("campfire update");
    let keys: Vec<&str> = match &tag {
        Nbt::Compound(v) => v.iter().map(|(k, _)| k.as_str()).collect(),
        _ => Vec::new(),
    };
    assert_eq!(keys, vec!["Items"]);
    assert_eq!(crate::nbtio::list(tag.get("Items")).len(), 1);
    let _ = w.events();
    assert_eq!(w.held().map(|s| s.count), Some(1));
    assert_eq!(w.p.s.prog.stats.get("minecraft:custom", "minecraft:interact_with_campfire"), 1);
    let mut evs = Vec::new();
    w.ticks(599, &mut evs);
    assert_eq!(w.items_on_ground("minecraft:cooked_beef"), 0, "not cooked yet");
    w.ticks(1, &mut evs);
    assert_eq!(w.items_on_ground("minecraft:cooked_beef"), 1, "cooked at 600 ticks");
    // Shovel: puts it out.
    w.hold(Some(Stack { id: "minecraft:iron_shovel".into(), count: 1, components: None }));
    w.click(c, Dir::Up, [0.5, 0.5, 0.5]);
    let evs = w.events();
    assert_eq!(level_events(&evs, 1009), vec![0]);
    assert!(!w.p.s.v().get_bool(w.p.s.get(c), "lit"), "put out with the shovel");
    assert_eq!(crate::items::component(&w.held().unwrap(), "minecraft:damage").and_then(Nbt::as_i64), Some(1));
    // Water: fire relit then drowned.
    let lit = w.p.s.v().set_bool(w.p.s.get(c), "lit", true);
    w.p.s.set_update(c, lit);
    let _ = w.events();
    w.p.s.bucket_empty_at(c, true, None, None);
    let evs = w.events();
    let s = w.p.s.get(c);
    assert!(w.p.s.v().get_bool(s, "waterlogged") && !w.p.s.v().get_bool(s, "lit"), "drowned and out");
    assert_eq!(w.sounds(&evs, "minecraft:entity.generic.extinguish_fire"), 1);
}

/// Chiseled bookshelf: targeted slot (2 × 3 on the front face), book stored then taken back,
/// comparator = last slot + 1 (0 before any click).
#[test]
fn chiseled_bookshelf_slots() {
    let mut w = world("bookshelf");
    let b = [12, Y, 4];
    w.set(b, "minecraft:chiseled_bookshelf[facing=north]");
    assert_eq!(w.comparator(b, Dir::South), 0, "no slot touched");
    w.hold(stack("minecraft:book", 3));
    // North face: x = 1 − relative x; top left seen from the front = high x, high y.
    w.click(b, Dir::North, [0.9, 0.9, 0.0]);
    w.click(b, Dir::North, [0.1, 0.2, 0.0]);
    let s = w.p.s.get(b);
    assert!(w.p.s.v().get_bool(s, "slot_0_occupied"), "slot 0 (top left)");
    assert!(w.p.s.v().get_bool(s, "slot_5_occupied"), "slot 5 (bottom right)");
    assert_eq!(w.comparator(b, Dir::South), 6);
    // Side face: nothing.
    w.click(b, Dir::East, [1.0, 0.5, 0.5]);
    assert_eq!(w.held().map(|s| s.count), Some(1));
    w.hold(None);
    w.click(b, Dir::North, [0.9, 0.9, 0.0]);
    assert!(!w.p.s.v().get_bool(w.p.s.get(b), "slot_0_occupied"), "book taken back");
    assert_eq!(w.comparator(b, Dir::South), 1);
    assert!(w.p.s.slots.iter().flatten().any(|s| s.id == "minecraft:book" && s.count == 1));
}

/// Shelf: simple swap (targeted slot, sounds), comparator from the back only;
/// powered side by side: chain (left, right) and hotbar swap.
#[test]
fn shelf_swaps_and_chain() {
    let mut w = world("shelf");
    let a = [5, Y, 12];
    w.set(a, "minecraft:oak_shelf[facing=north]");
    w.hold(stack("minecraft:apple", 5));
    // North face, relative x 0.9 → column 0.
    w.click(a, Dir::North, [0.9, 0.5, 0.0]);
    let evs = w.events();
    assert!(w.held().is_none(), "whole stack placed");
    assert_eq!(w.sounds(&evs, "minecraft:block.shelf.place_item"), 1);
    assert_eq!(w.comparator(a, Dir::South), 1, "slot 0 from the back");
    assert_eq!(w.comparator(a, Dir::North), 0, "from the front: 0");
    w.click(a, Dir::North, [0.9, 0.5, 0.0]);
    let evs = w.events();
    assert_eq!(w.held().map(|s| s.count), Some(5), "taken back");
    assert_eq!(w.sounds(&evs, "minecraft:block.shelf.take_item"), 1);
    // Two shelves powered side by side (left = east for a north face).
    let b = [4, Y, 12];
    w.set(b, "minecraft:oak_shelf[facing=north]");
    w.set([5, Y, 13], "minecraft:redstone_block");
    w.set([4, Y, 13], "minecraft:redstone_block");
    let (sa, sb) = (w.p.s.get(a), w.p.s.get(b));
    assert!(w.p.s.v().get_bool(sa, "powered") && w.p.s.v().get_bool(sb, "powered"));
    assert_eq!(w.p.s.v().get(sa, "side_chain"), Some("left"), "a on the left");
    assert_eq!(w.p.s.v().get(sb, "side_chain"), Some("right"), "b on the right");
    // Hotbar: slots 3..8 swapped with a (3..5) then b (6..8).
    for i in 0..9 {
        w.p.s.slots[36 + i] = stack("minecraft:stick", i as i32 + 1);
    }
    w.p.s.held = 0;
    w.click(a, Dir::North, [0.5, 0.5, 0.0]);
    let evs = w.events();
    assert_eq!(w.sounds(&evs, "minecraft:block.shelf.multi_swap"), 1);
    let ia = crate::containers::read_items(w.p.s.be_get(a).unwrap(), 3);
    let ib = crate::containers::read_items(w.p.s.be_get(b).unwrap(), 3);
    assert_eq!(ia.iter().map(|s| s.as_ref().map_or(0, |s| s.count)).collect::<Vec<_>>(), vec![4, 5, 6]);
    assert_eq!(ib.iter().map(|s| s.as_ref().map_or(0, |s| s.count)).collect::<Vec<_>>(), vec![7, 8, 9]);
    assert!(w.p.s.slots[36 + 3..36 + 9].iter().all(Option::is_none), "hotbar emptied into the chain");
    // Power cut: chain undone.
    w.set([5, Y, 13], "minecraft:air");
    assert_eq!(w.p.s.v().get(w.p.s.get(a), "side_chain"), Some("unconnected"));
    assert_eq!(w.p.s.v().get(w.p.s.get(b), "side_chain"), Some("unconnected"));
}

/// Copper golem statue: next pose on click (sound, comparator), axe on an
/// intact statue: golem given back, block removed.
#[test]
fn copper_statue_pose_and_axe() {
    let mut w = world("statue");
    let at = [8, Y, 2];
    w.set(at, "minecraft:copper_golem_statue[facing=south]");
    assert_eq!(w.comparator(at, Dir::North), 1);
    w.hold(stack("minecraft:stone", 1));
    w.click(at, Dir::Up, [0.5, 0.875, 0.5]);
    let evs = w.events();
    assert_eq!(w.p.s.v().get(w.p.s.get(at), "copper_golem_pose"), Some("sitting"));
    assert_eq!(w.sounds(&evs, "minecraft:entity.copper_golem_become_statue"), 1);
    assert_eq!(w.comparator(at, Dir::North), 2);
    assert_eq!(w.held().map(|s| s.count), Some(1), "the block is not placed");
    let before = w.p.s.mobs.list.iter().flatten().count();
    w.hold(Some(Stack { id: "minecraft:iron_axe".into(), count: 1, components: None }));
    w.click(at, Dir::Up, [0.5, 0.875, 0.5]);
    assert_eq!(w.name(at), "minecraft:air");
    assert_eq!(w.p.s.mobs.list.iter().flatten().count(), before + 1, "golem given back");
}

/// Dragon egg: 2015 (packed difference), arrival on a non-empty block.
#[test]
fn dragon_egg_teleport_event() {
    let mut w = world("egg");
    let at = [8, Y, 8];
    w.set(at, "minecraft:dragon_egg");
    w.hold(None);
    w.click(at, Dir::Up, [0.5, 1.0, 0.5]);
    let evs = w.events();
    let packed = level_events(&evs, 2015);
    assert_eq!(packed.len(), 1, "one 2015 event");
    let d = [(packed[0] >> 16 & 0xFF) - 16, (packed[0] >> 8 & 0xFF) - 8, (packed[0] & 0xFF) - 16];
    let t = [at[0] + d[0], at[1] + d[1], at[2] + d[2]];
    assert_eq!(w.name(t), "minecraft:dragon_egg", "egg at the announced cell");
    assert!(!w.p.s.v().is_air(w.p.s.get(below(t))), "placed on a block");
}

/// Note block: instrument head on the top face → placement (no note); click → note,
/// statistic.
#[test]
fn note_block_head_and_stat() {
    let mut w = world("note");
    let at = [2, Y, 2];
    w.set(at, "minecraft:note_block");
    w.hold(stack("minecraft:zombie_head", 1));
    w.click(at, Dir::Up, [0.5, 1.0, 0.5]);
    assert!(w.name(above(at)).starts_with("minecraft:zombie_head"), "head placed: {}", w.name(above(at)));
    assert_eq!(w.p.s.v().get_int(w.p.s.get(at), "note"), 0, "note unchanged");
    w.hold(stack("minecraft:stone", 1));
    w.click(at, Dir::North, [0.5, 0.5, 0.0]);
    assert_eq!(w.p.s.v().get_int(w.p.s.get(at), "note"), 1);
    assert_eq!(w.p.s.prog.stats.get("minecraft:custom", "minecraft:tune_noteblock"), 1);
}

/// Light block: next level in creative with commands; survival: nothing.
#[test]
fn light_block_cycles_level() {
    let mut w = world("light");
    let at = [3, Y, 3];
    w.set(at, "minecraft:light[level=15]");
    w.hold(None);
    w.click(at, Dir::Up, [0.5, 0.5, 0.5]);
    assert_eq!(w.p.s.v().get_int(w.p.s.get(at), "level"), 15, "survival: nothing");
    w.p.s.game_mode = 1;
    w.p.s.level.allow_commands = true;
    w.p.s.cmd.op = 4;
    w.click(at, Dir::Up, [0.5, 0.5, 0.5]);
    assert_eq!(w.p.s.v().get_int(w.p.s.get(at), "level"), 0, "15 → 0");
}

/// Water bottle on dirt: mud, empty bottle, sounds, 5 drops; from below: nothing.
#[test]
fn water_bottle_makes_mud() {
    let mut w = world("mud");
    let at = [7, Y, 7];
    w.set(at, "minecraft:dirt");
    let water = Stack {
        id: "minecraft:potion".into(),
        count: 1,
        components: Some(Nbt::Compound(vec![(
            "minecraft:potion_contents".into(),
            Nbt::Compound(vec![("potion".into(), Nbt::String("minecraft:water".into()))]),
        )])),
    };
    w.hold(Some(water.clone()));
    w.click(at, Dir::Down, [0.5, 0.0, 0.5]);
    assert_eq!(w.name(at), "minecraft:dirt", "from below: nothing");
    w.click(at, Dir::Up, [0.5, 1.0, 0.5]);
    let evs = w.events();
    assert_eq!(w.name(at), "minecraft:mud");
    assert_eq!(w.held().map(|s| s.id), Some("minecraft:glass_bottle".to_owned()));
    assert_eq!(w.sounds(&evs, "minecraft:entity.generic.splash"), 1);
    assert_eq!(w.sounds(&evs, "minecraft:item.bottle.empty"), 1);
    assert_eq!(particles(&w, &evs, "minecraft:splash").len(), 5);
}

/// Sounds: copper bulb that lights up, dried ghast drowned by a bucket.
#[test]
fn copper_bulb_and_dried_ghast_sounds() {
    let mut w = world("sounds");
    let bulb = [9, Y, 5];
    w.set(bulb, "minecraft:copper_bulb");
    let _ = w.events();
    w.set([9, Y, 6], "minecraft:redstone_block");
    let evs = w.events();
    assert_eq!(w.sounds(&evs, "minecraft:block.copper_bulb.turn_on"), 1);
    let g = [11, Y, 5];
    w.set(g, "minecraft:dried_ghast");
    let _ = w.events();
    w.p.s.bucket_empty_at(g, true, None, None);
    let evs = w.events();
    assert_eq!(w.sounds(&evs, "minecraft:block.dried_ghast.place_in_water"), 1);
}

/// Redstone dust, repeater and rail deprived of support, removed by "neighbour changed"
/// (without shape update): loot dropped.
#[test]
fn unsupported_redstone_drops() {
    let mut w = world("wire");
    let cases = [
        ([1, Y + 1, 9], "minecraft:redstone_wire", "minecraft:redstone"),
        ([3, Y + 1, 9], "minecraft:repeater", "minecraft:repeater"),
        ([5, Y + 1, 9], "minecraft:rail", "minecraft:rail"),
    ];
    for (at, block, _) in cases {
        w.set(below(at), "minecraft:stone");
        w.set(at, block);
    }
    let air = w.p.s.default_state("minecraft:air");
    for (at, block, _) in cases {
        w.p.s.set_block(below(at), air, flag::CLIENTS | flag::KNOWN_SHAPE);
        let b = w.p.s.tb().block_id("minecraft:stone");
        w.p.s.neighbor_changed_at(at, b);
        assert_eq!(w.name(at), "minecraft:air", "{block} removed");
    }
    for (_, _, item) in cases {
        assert_eq!(w.items_on_ground(item), 1, "{item} dropped");
    }
}

/// Lit and empty campfire, food written by `/data merge`: the ticker flags follow
/// the re-read NBT, the cooked beef drops at the end of the written duration.
#[test]
fn campfire_food_written_by_command_cooks() {
    let mut w = world("campfire-data");
    let c = [10, Y, 10];
    w.set(c, "minecraft:campfire[lit=true]");
    w.cmd(&format!(
        "data merge block {} {} {} {{Items:[{{Slot:0b,id:\"minecraft:beef\",count:1}}],CookingTimes:[I;0,0,0,0],CookingTotalTimes:[I;600,0,0,0]}}\n",
        c[0], c[1], c[2]
    ));
    let mut evs = Vec::new();
    w.ticks(599, &mut evs);
    assert_eq!(w.items_on_ground("minecraft:cooked_beef"), 0, "not cooked yet");
    w.ticks(1, &mut evs);
    assert_eq!(w.items_on_ground("minecraft:cooked_beef"), 1, "cooked at 600 ticks");
}

/// Disc poured by a hopper (G5-F01): same placement as on click (G6-F01): `has_record`,
/// 1010 and signal 15; the full jukebox receives no other.
#[test]
fn hopper_inserts_disc_and_jukebox_plays() {
    let mut w = world("jukebox-hopper");
    let j = [4, Y, 4];
    w.set(j, "minecraft:jukebox");
    w.cmd(&format!(
        r#"setblock 4 {} 4 minecraft:hopper[facing=down]{{Items:[{{Slot:0b,id:"minecraft:music_disc_13",count:2}}]}}"#,
        Y + 1
    ));
    let mut evs = Vec::new();
    w.ticks(20, &mut evs);
    assert_eq!(w.name(j), "minecraft:jukebox[has_record=true]");
    let left = w.p.s.slots([4, Y + 1, 4])[0].as_ref().and_then(Stack::from_nbt).map(|s| s.count);
    assert_eq!(left, Some(1), "only one disc poured");
    assert_eq!(level_events(&evs, 1010).len(), 1, "playback started once");
    assert_eq!(w.p.s.weak(w.p.s.get(j), j, Dir::Up), 15);
    assert_eq!(w.comparator(j, Dir::North), 1);
}

fn level_events_at(evs: &[Event], kind: i32, at: P) -> usize {
    evs.iter().filter(|e| matches!(e, Event::LevelEvent { kind: k, pos, .. } if *k == kind && *pos == at)).count()
}

/// Playing jukebox copied by `/clone` (and counter rewritten by `/data`): the copy plays,
/// finishes its track (1011) and its signal falls back to 0, like the original.
#[test]
fn jukebox_clone_keeps_playing() {
    let mut w = world("jukebox-clone");
    let j = [2, Y, 2];
    let k = [2, Y, 6];
    w.set(j, "minecraft:jukebox");
    w.hold(stack("minecraft:music_disc_cat", 1));
    w.click(j, Dir::Up, [0.5, 1.0, 0.5]);
    // cat: 185 s → end at 3720; we skip to near the end.
    w.cmd(&format!("data merge block {} {} {} {{ticks_since_song_started:3700L}}\n", j[0], j[1], j[2]));
    w.cmd(&format!("clone {0} {1} {2} {0} {1} {2} {3} {4} {5}\n", j[0], j[1], j[2], k[0], k[1], k[2]));
    assert_eq!(w.name(k), "minecraft:jukebox[has_record=true]");
    assert_eq!(w.p.s.weak(w.p.s.get(k), k, Dir::Up), 15, "the copy plays");
    let _ = w.events();
    let mut evs = Vec::new();
    w.ticks(20, &mut evs);
    assert_eq!(level_events_at(&evs, 1011, k), 0, "not finished yet");
    w.ticks(1, &mut evs);
    assert_eq!(level_events_at(&evs, 1011, j), 1, "original finished at 3720");
    assert_eq!(level_events_at(&evs, 1011, k), 1, "copy finished at 3720");
    assert_eq!(w.p.s.weak(w.p.s.get(k), k, Dir::Up), 0, "signal of the copy fell back");
    // Save taken at the tick when the counter reaches the end: re-read, it is no longer playing.
    let s = w.p.s.get(j);
    w.p.s.bs.be_ticks.remove(&j);
    if let Some(be) = w.p.s.be_get_mut(j) {
        crate::nbtio::set(be, "ticks_since_song_started", Nbt::Long(3720));
    }
    w.p.s.jukebox_loaded_from_save(j);
    w.p.s.jukebox_ticker_sync(j, s);
    assert_eq!(w.p.s.weak(s, j, Dir::Up), 0, "finished track re-read: signal 0");
}

/// Comparator reading the jukebox through a full block: updated on insertion (track
/// output) and on ejection.
#[test]
fn jukebox_comparator_through_block() {
    let mut w = world("jukebox-cmp");
    let j = [2, Y, 2];
    let cmp = [4, Y, 2];
    w.set(j, "minecraft:jukebox");
    w.set([3, Y, 2], "minecraft:stone");
    w.set(cmp, "minecraft:comparator[facing=west]");
    let mut evs = Vec::new();
    w.ticks(4, &mut evs);
    let out = |w: &W| w.p.s.be_get(cmp).and_then(|b| crate::nbtio::int(b, "OutputSignal")).unwrap_or(0);
    assert_eq!(out(&w), 0);
    w.hold(stack("minecraft:music_disc_cat", 1));
    w.click(j, Dir::Up, [0.5, 1.0, 0.5]);
    w.ticks(4, &mut evs);
    assert_eq!(out(&w), 2, "output of the cat track through the stone");
    w.hold(None);
    w.click(j, Dir::Up, [0.5, 1.0, 0.5]);
    w.ticks(4, &mut evs);
    assert_eq!(out(&w), 0, "disc ejected");
}

/// Decorated pot: comparator divided by the item's maximum stack size (component included).
#[test]
fn decorated_pot_comparator_own_stack_size() {
    let mut w = world("pot-cmp");
    let at = [3, Y, 8];
    w.set(at, "minecraft:decorated_pot");
    w.cmd(&format!(
        "data merge block {} {} {} {{item:{{id:\"minecraft:dirt\",count:70,components:{{\"minecraft:max_stack_size\":99}}}}}}\n",
        at[0], at[1], at[2]
    ));
    assert_eq!(w.comparator(at, Dir::North), 10, "1 + floor(70/99 × 14)");
}

/// Same cause in ordinary containers: barrel full of stacks of 70 dirt with
/// `max_stack_size` 99 → 1 + floor(70/99 × 14) = 10 (16 if divided by 64).
#[test]
fn container_comparator_own_stack_size() {
    let mut w = world("barrel-cmp");
    let at = [3, Y, 8];
    w.set(at, "minecraft:barrel[facing=up]");
    let dirt = Stack {
        id: "minecraft:dirt".into(),
        count: 70,
        components: Some(Nbt::Compound(vec![("minecraft:max_stack_size".into(), Nbt::Int(99))])),
    };
    let items = vec![Some(dirt); 27];
    crate::containers::write_items(w.p.s.be_get_mut(at).unwrap(), &items);
    assert_eq!(w.comparator(at, Dir::North), 10);
}

/// Decorated pot with sealed loot: the loot is drawn on click, the dirt does not go in (failure), and
/// breaking drops only the loot.
#[test]
fn decorated_pot_loot_unpacked_on_use() {
    let mut w = world("pot-loot");
    let at = [3, Y, 8];
    w.cmd(&format!(
        "setblock {} {} {} minecraft:decorated_pot{{LootTable:\"minecraft:pots/trial_chambers/corridor\",LootTableSeed:5L}}",
        at[0], at[1], at[2]
    ));
    w.hold(stack("minecraft:dirt", 4));
    w.click(at, Dir::Up, [0.5, 1.0, 0.5]);
    let evs = w.events();
    assert_eq!(w.held().map(|s| s.count), Some(4), "dirt refused");
    assert_eq!(w.sounds(&evs, "minecraft:block.decorated_pot.insert_fail"), 1);
    let be = w.p.s.be_get(at).unwrap();
    assert!(be.get("LootTable").is_none(), "loot drawn");
    let loot = be.get("item").and_then(Stack::from_nbt).expect("loot in the pot");
    w.p.s.destroy_block(at, false, 512);
    assert_eq!(w.items_on_ground("minecraft:dirt"), 0);
    assert_eq!(w.items_on_ground(&loot.id), loot.count, "loot dropped");
}

/// Micro-bench (ignored by default): per-tick cost of a lit empty campfire, ticker skipped
/// (flags kept) versus re-reading the block entity at each tick.
#[test]
#[ignore]
fn bench_campfire_empty_tick() {
    let mut w = world("bench-campfire");
    let mut fires = Vec::new();
    for x in 0..16 {
        for z in 0..16 {
            let at = [x, Y + 2, z];
            w.p.s.set_block(at, w.p.s.default_state("minecraft:campfire"), flag::CLIENTS);
            fires.push(at);
        }
    }
    for (label, flags) in [("skipped", false), ("re-read", true)] {
        let t = std::time::Instant::now();
        for _ in 0..2000 {
            for &at in &fires {
                w.p.s.campfire_tick(at, flags, flags);
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / (2000.0 * fires.len() as f64);
        eprintln!("empty campfire {label}: {ns:.1} ns/tick");
    }
}
