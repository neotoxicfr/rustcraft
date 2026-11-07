//! Containers seen by the commands the way the game sees them: furnaces and brewing stands read
//! and written in their live state (their NBT lags until the save), the jukebox and the
//! decorated pot through their single slot, the slot write's own "changed" rule per family
//! (comparators), sealed loot drawn by a slot read or write only, and the command storage
//! edited in place (a failed edit keeps what it created). Fresh flat world; the 26.3 data
//! cache is required (the test fails without it).

use std::sync::Arc;

use rc_proto::nbt::Nbt;

use crate::blocks::probe::Probe;
use crate::player::Stack;

const Y: i32 = -60;

/// Test world (its folder is removed when the test thread ends).
struct World {
    p: Probe,
}

fn world(tag: &str) -> World {
    let data = Arc::new(rc_testdata::version_data().expect("26.3 data cache missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-containercmd-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 7;
    w.preset = crate::newworld::Preset::Flat;
    w.allow_commands = true;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    let want: Vec<(i32, i32)> = (-1..=1).flat_map(|x| (-1..=1).map(move |z| (x, z))).collect();
    p.s.ensure_loaded(&want);
    let t = std::time::Instant::now();
    while !want.iter().all(|c| p.s.chunks.get(c).is_some_and(Option::is_some)) {
        assert!(t.elapsed().as_secs() < 120, "flat world generation too slow");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    p.s.game_mode = 1;
    World { p }
}

impl World {
    /// Real command (full parsing); returns success and result.
    fn try_cmd(&mut self, text: &str) -> (bool, i32) {
        let src = self.p.s.player_source();
        self.p.s.perform(src, text)
    }

    fn cmd(&mut self, text: &str) -> i32 {
        let (ok, r) = self.try_cmd(text);
        assert!(ok, "command refused: {text}");
        r
    }

    fn ticks(&mut self, n: usize) {
        for _ in 0..n {
            self.p.tick();
        }
    }

    fn state(&self, at: [i32; 3]) -> String {
        crate::blocks::probe::state_string(&self.p.s, self.p.s.get(at))
    }

    fn block_data_int(&self, at: [i32; 3], key: &str) -> Option<i64> {
        self.p.s.data_of_block(at).and_then(|n| n.get(key).and_then(Nbt::as_i64))
    }
}

fn pos(at: [i32; 3]) -> String {
    format!("{} {} {}", at[0], at[1], at[2])
}

/// Blast furnace cooking raw iron (100 ticks): reads, block data, block predicates, clone,
/// setblock and writes of every slot work on the live furnace; only another ingredient (an
/// emptied slot included) restarts cooking.
#[test]
fn furnace_commands_use_the_live_furnace() {
    let mut w = world("furnace");
    let at = [2, Y, 2];
    let f = pos(at);
    w.cmd(&format!("setblock {f} minecraft:blast_furnace"));
    w.cmd(&format!("item replace block {f} container.0 with minecraft:raw_iron 8"));
    w.cmd(&format!("item replace block {f} container.1 with minecraft:coal 4"));
    w.ticks(30);
    let live_at = |w: &World, at: [i32; 3]| {
        let f = &w.p.s.life.furnaces[&at];
        (f.timer, f.total, f.lit_remaining)
    };
    let live = |w: &World| live_at(w, at);
    let (timer, total, lit) = live(&w);
    assert!(timer >= 20 && total == 100 && lit > 0, "the furnace cooks: timer {timer}, total {total}, lit {lit}");
    // Reads: the coal burning is in the live slot, not in the NBT written at the last save.
    assert_eq!(w.cmd(&format!("execute if items block {f} container.1 minecraft:coal")), 3, "live fuel read");
    assert_eq!(w.block_data_int(at, "cooking_time_spent"), Some(i64::from(timer)), "block data = live cooking");
    assert_eq!(w.block_data_int(at, "lit_time_remaining"), Some(i64::from(lit)), "block data = live burning");
    // A block predicate with data compares the live furnace.
    let cooking = |t: i32| format!("execute if block {f} minecraft:blast_furnace{{cooking_time_spent:{t}}}");
    assert!(w.try_cmd(&cooking(timer)).0, "the predicate sees the live cooking time");
    assert!(!w.try_cmd(&cooking(0)).0, "the predicate does not see the saved cooking time");
    // A clone copies the live furnace: fuel burnt and cooking time carried over.
    let copy = [2, Y, 5];
    w.cmd(&format!("clone {f} {f} {}", pos(copy)));
    assert_eq!(live_at(&w, copy), (timer, total, lit), "the copy cooks where the source was");
    assert_eq!(w.cmd(&format!("execute if items block {} container.1 minecraft:coal", pos(copy))), 3, "the burnt coal is not copied");
    assert_eq!(w.cmd(&format!("execute if blocks {f} {f} {} all", pos(copy))), 1, "source and copy have the same live data");
    // A setblock giving the live data again changes nothing, so it fails.
    let mut same = w.p.s.data_of_block(at).expect("furnace data");
    for k in ["id", "x", "y", "z"] {
        crate::nbtio::remove(&mut same, k);
    }
    let same = crate::commands::snbt::to_snbt(&same);
    let (ok, _) = w.try_cmd(&format!("setblock {f} minecraft:blast_furnace[lit=true]{same}"));
    assert!(!ok, "the same block with its live data is not a change");
    // Writes that keep cooking: result slot, block data merge, same ingredient.
    w.cmd(&format!("item replace block {f} container.2 with minecraft:iron_ingot"));
    assert_eq!(live(&w), (timer, total, lit), "a result write keeps cooking");
    w.cmd(&format!("data merge block {f} {{CustomName:\"Oven\"}}"));
    assert_eq!(live(&w), (timer, total, lit), "a data merge keeps cooking");
    assert_eq!(w.cmd(&format!("execute if items block {f} container.0 minecraft:raw_iron")), 8, "ingredient kept by the merge");
    w.cmd(&format!("item replace block {f} container.0 with minecraft:raw_iron 2"));
    assert_eq!(live(&w).0, timer, "the same ingredient keeps cooking");
    // An emptied ingredient slot restarts cooking with the time of no recipe.
    w.cmd(&format!("item replace block {f} container.0 with minecraft:air"));
    let (timer, total, _) = live(&w);
    assert_eq!((timer, total), (0, 200), "an empty write restarts cooking");
    assert_eq!(w.cmd(&format!("execute if items block {f} container.2 minecraft:iron_ingot")), 1, "result kept");
}

/// Brewing stand mid-brew: the block data shows the live brewing time and a merge keeps it.
#[test]
fn brewing_stand_commands_use_the_live_stand() {
    let mut w = world("brewing");
    let at = [4, Y, 2];
    let b = pos(at);
    w.cmd(&format!("setblock {b} minecraft:brewing_stand"));
    w.cmd(&format!("item replace block {b} container.4 with minecraft:blaze_powder"));
    w.cmd(&format!("item replace block {b} container.0 with minecraft:potion[minecraft:potion_contents={{potion:\"minecraft:water\"}}]"));
    w.cmd(&format!("item replace block {b} container.3 with minecraft:nether_wart"));
    w.ticks(20);
    let brew = w.p.s.life.brewers[&at].brew_time;
    assert!(brew > 0 && brew < 400, "the stand brews: {brew}");
    assert_eq!(w.block_data_int(at, "BrewTime"), Some(i64::from(brew)), "block data = live brewing");
    // The powder became fuel: the live slot is empty (the NBT still holds it).
    w.cmd(&format!("execute unless items block {b} container.4 minecraft:blaze_powder"));
    w.cmd(&format!("data merge block {b} {{CustomName:\"Stand\"}}"));
    assert_eq!(w.p.s.life.brewers[&at].brew_time, brew, "a data merge keeps brewing");
    w.ticks(1);
    assert_eq!(w.p.s.life.brewers[&at].brew_time, brew - 1, "brewing goes on after the merge");
}

/// Jukebox and decorated pot: one slot under their own key (`RecordItem`, `item`).
#[test]
fn jukebox_and_pot_slot_is_their_own_key() {
    let mut w = world("single");
    let jukebox = [6, Y, 2];
    let pot = [8, Y, 2];
    let (j, p) = (pos(jukebox), pos(pot));
    w.cmd(&format!("setblock {j} minecraft:jukebox"));
    w.cmd(&format!("setblock {p} minecraft:decorated_pot"));
    w.cmd(&format!("item replace block {j} container.0 with minecraft:music_disc_cat"));
    w.cmd(&format!("item replace block {p} container.0 with minecraft:diamond 3"));
    let be = |at| w.p.s.be_get(at).cloned().unwrap();
    let disc = be(jukebox).get("RecordItem").and_then(Stack::from_nbt).map(|s| s.id);
    assert_eq!(disc.as_deref(), Some("minecraft:music_disc_cat"), "disc stored under RecordItem");
    assert!(be(jukebox).get("Items").is_none(), "no Items list in a jukebox");
    let item = be(pot).get("item").and_then(Stack::from_nbt).map(|s| (s.id, s.count));
    assert_eq!(item, Some(("minecraft:diamond".to_owned(), 3)), "pot item stored under item");
    assert!(w.state(jukebox).contains("has_record=true"), "the jukebox shows its disc: {}", w.state(jukebox));
    assert_eq!(w.cmd(&format!("execute if items block {j} container.0 minecraft:music_disc_cat")), 1);
    assert_eq!(w.cmd(&format!("execute if items block {p} container.0 minecraft:diamond")), 3);
    w.cmd(&format!("item replace block {j} container.0 with minecraft:air"));
    assert!(w.state(jukebox).contains("has_record=false"), "the disc is out: {}", w.state(jukebox));
}

/// A slot write marks a chest changed (its comparator lights) but not a hopper nor a decorated
/// pot (the game's quirk: their comparator is not told).
#[test]
fn slot_write_tells_comparators_except_for_the_hopper() {
    let mut w = world("comparator");
    let (chest, hopper, pot) = ([2, Y, 6], [6, Y, 6], [10, Y, 6]);
    for (at, block) in [(chest, "minecraft:chest"), (hopper, "minecraft:hopper"), (pot, "minecraft:decorated_pot")] {
        w.cmd(&format!("setblock {} {block}", pos(at)));
        // The comparator reads the block north of it.
        w.cmd(&format!("setblock {} {} {} minecraft:comparator[facing=north]", at[0], at[1], at[2] + 1));
    }
    w.ticks(4);
    w.cmd(&format!("item replace block {} container.0 with minecraft:stone 64", pos(chest)));
    w.cmd(&format!("item replace block {} container.0 with minecraft:stone 64", pos(hopper)));
    w.cmd(&format!("item replace block {} container.0 with minecraft:stone 64", pos(pot)));
    w.ticks(4);
    let lit = |w: &World, at: [i32; 3]| w.state([at[0], at[1], at[2] + 1]).contains("powered=true");
    assert!(lit(&w, chest), "the chest's comparator lights");
    assert!(!lit(&w, hopper), "the hopper's comparator is not told");
    assert!(!lit(&w, pot), "the decorated pot's comparator is not told");
    // `loot insert` marks every container it changes, the hopper included.
    w.cmd(&format!("loot insert {} loot minecraft:blocks/dirt", pos(hopper)));
    w.ticks(4);
    assert!(lit(&w, hopper), "loot insert marks the hopper changed");
}

/// Sealed loot is drawn by a slot read or write (`execute if items`, `item replace`), never by a
/// slot count (`execute if slots`) or the block data (`data get`).
#[test]
fn sealed_loot_is_drawn_by_slot_access_only() {
    let mut w = world("sealed");
    let (a, b) = ([2, Y, 10], [4, Y, 10]);
    for at in [a, b] {
        w.cmd(&format!("setblock {} minecraft:chest{{LootTable:\"minecraft:chests/simple_dungeon\",LootTableSeed:3L}}", pos(at)));
    }
    let sealed = |w: &World, at| w.p.s.be_get(at).is_some_and(|be| be.get("LootTable").is_some());
    assert_eq!(w.cmd(&format!("execute if slots block {} container.*", pos(a))), 27);
    w.cmd(&format!("data get block {}", pos(a)));
    assert!(sealed(&w, a), "counting slots or reading the data draws nothing");
    assert!(w.cmd(&format!("execute if items block {} container.* *", pos(a))) > 0, "the drawn loot is counted");
    assert!(!sealed(&w, a), "a slot read draws the loot");
    w.cmd(&format!("item replace block {} container.26 with minecraft:stone", pos(b)));
    assert!(!sealed(&w, b), "a slot write draws the loot");
    assert!(w.cmd(&format!("execute if items block {} container.* *", pos(b))) > 1, "the loot is there beside the stone");
}

/// The storage is edited in place: a failed edit keeps the nodes it created when the identifier
/// existed; an emptied identifier is removed; only touched namespaces are saved.
#[test]
fn storage_edits_happen_in_place() {
    let mut w = world("storage");
    w.cmd("data merge storage t:a {k:1}");
    w.p.s.cmd.storage_dirty.clear();
    let (ok, _) = w.try_cmd("data modify storage t:a x merge value {}");
    assert!(!ok, "nothing changed: the command fails");
    let a = w.p.s.cmd.storage.get("t:a").map(crate::commands::snbt::to_snbt);
    assert_eq!(a.as_deref(), Some("{k:1,x:{}}"), "the created node stays");
    assert!(w.p.s.cmd.storage_dirty.is_empty(), "a failed edit marks nothing to save");
    let (ok, _) = w.try_cmd("data modify storage t:b x merge value {}");
    assert!(!ok && !w.p.s.cmd.storage.contains_key("t:b"), "a missing identifier stays missing");
    w.cmd("execute store result storage t:a n.l[0].v int 1 run seed");
    let a = w.p.s.cmd.storage.get("t:a").map(crate::commands::snbt::to_snbt);
    assert_eq!(a.as_deref(), Some("{k:1,n:{l:[]},x:{}}"), "a failed store keeps what it created");
    w.cmd("data merge storage u:c {v:2}");
    let dirty: Vec<&str> = w.p.s.cmd.storage_dirty.iter().map(String::as_str).collect();
    assert_eq!(dirty, ["u"], "only the written namespace is saved");
    w.cmd("data remove storage u:c v");
    assert!(!w.p.s.cmd.storage.contains_key("u:c"), "an emptied identifier is removed");
    let mut jobs = Vec::new();
    w.p.s.commands_save_jobs(&mut jobs);
    let storage_files: Vec<String> = jobs
        .iter()
        .filter_map(|j| match j {
            crate::save::Job::Dat { path, .. } if path.ends_with("command_storage.dat") => {
                path.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned())
            }
            _ => None,
        })
        .collect();
    assert_eq!(storage_files, ["u"], "the emptied namespace is written, the other one is not");
}

/// Chiseled bookshelf: a book written by a command shows in the block state, an empty write
/// takes it out. Crafter: any write to a disabled empty slot, an empty one included, enables it;
/// a load never disables an occupied slot (only sealed loot drawn by the slot read just before
/// a write can leave a disabled slot occupied).
#[test]
fn bookshelf_and_crafter_slot_writes_follow_their_block() {
    let mut w = world("bookshelf");
    let (shelf, crafter) = ([2, Y, 14], [4, Y, 14]);
    let (s, c) = (pos(shelf), pos(crafter));
    w.cmd(&format!("setblock {s} minecraft:chiseled_bookshelf"));
    w.cmd(&format!("item replace block {s} container.0 with minecraft:book"));
    assert!(w.state(shelf).contains("slot_0_occupied=true"), "the book shows: {}", w.state(shelf));
    w.cmd(&format!("item replace block {s} container.0 with minecraft:air"));
    assert!(w.state(shelf).contains("slot_0_occupied=false"), "the book is out: {}", w.state(shelf));
    w.cmd(&format!("setblock {c} minecraft:crafter{{disabled_slots:[I;0,4]}}"));
    let disabled = |w: &World| w.p.s.be_get(crafter).and_then(|b| b.get("disabled_slots").cloned());
    w.cmd(&format!("item replace block {c} container.0 with minecraft:stone"));
    assert_eq!(disabled(&w), Some(Nbt::IntArray(vec![4])), "slot 0 enabled, slot 4 still disabled");
    // An empty write enables a disabled slot too.
    w.cmd(&format!("item replace block {c} container.4 with minecraft:air"));
    assert_eq!(disabled(&w), Some(Nbt::IntArray(vec![])), "an empty write enables slot 4");
    // Loading drops a disabled index whose slot holds an item: the slot is active from the start
    // and stays so after a stone write, then an empty write.
    w.cmd(&format!("setblock {c} minecraft:air"));
    w.cmd(&format!("setblock {c} minecraft:crafter{{disabled_slots:[I;2],Items:[{{Slot:2b,id:\"minecraft:dirt\",count:1}}]}}"));
    assert_eq!(disabled(&w), Some(Nbt::IntArray(vec![])), "an occupied slot is never disabled by a load");
    w.cmd(&format!("item replace block {c} container.2 with minecraft:stone"));
    assert_eq!(disabled(&w), Some(Nbt::IntArray(vec![])), "still active after the stone write");
    w.cmd(&format!("item replace block {c} container.2 with minecraft:air"));
    assert_eq!(disabled(&w), Some(Nbt::IntArray(vec![])), "still active after the empty write");
    assert_eq!(w.p.s.crafter_data(crafter)[2], 0, "the menu shows slot 2 active");
}
