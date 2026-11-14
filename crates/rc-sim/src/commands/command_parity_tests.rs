//! Commands that act like the game's: an effect given with hidden particles hides its icon,
//! `recipe` counts book displays, stopwatches resume at the world's opening, a block entity
//! written by `data`, `setblock` or `clone` tells its comparators, `damage` reaches decorations
//! and the shulker bullet, the server flattens texts in its client's language, a stack created
//! by a command is checked strictly, and a player in another world cannot `ride` here. Fresh flat
//! world; the 26.3 data cache is required (the tests fail without it).

use std::sync::Arc;

use rc_proto::nbt::Nbt;

use crate::blocks::probe::Probe;

const Y: i32 = -60;

struct World {
    p: Probe,
    dir: std::path::PathBuf,
}

fn world(tag: &str) -> World {
    let (mut p, dir) = crate::tests_g3f13::open(&format!("parity-{tag}"));
    let src = p.s.server_source();
    let _ = p.s.perform(src, "weather clear");
    World { p, dir }
}

impl World {
    /// Real command run by the player as an operator (full parsing): success and result.
    fn run(&mut self, text: &str) -> (bool, i32) {
        let mut src = self.p.s.player_source();
        src.level = 4;
        self.p.s.perform(src, text)
    }

    fn cmd(&mut self, text: &str) -> i32 {
        let (ok, r) = self.run(text);
        assert!(ok, "command refused: {text}");
        r
    }

    /// Whether an entity matches the selector (the count of `execute if entity`).
    fn exists(&mut self, selector: &str) -> bool {
        self.run(&format!("execute if entity {selector}")).0
    }

    /// `n` whole server ticks (entities, block entities, scheduled block ticks).
    fn ticks(&mut self, n: usize) {
        for _ in 0..n {
            self.p.s.game_tick();
            self.p.s.tick += 1;
        }
    }

    /// Texts of the chat messages sent since the last read (translation keys included).
    fn chat(&self) -> Vec<String> {
        self.p
            .take_events()
            .into_iter()
            .filter_map(|e| match e {
                rc_client::Event::Chat { component, .. } => Some(format!("{component:?}")),
                _ => None,
            })
            .collect()
    }

    fn state(&self, at: [i32; 3]) -> String {
        crate::blocks::probe::state_string(&self.p.s, self.p.s.get(at))
    }
}

fn pos(at: [i32; 3]) -> String {
    format!("{} {} {}", at[0], at[1], at[2])
}

/// `effect give … true`: no particles and no icon, the icon saved hidden too.
#[test]
fn hidden_particles_hide_the_effect_icon() {
    let mut w = world("effect");
    w.cmd("effect give @s minecraft:night_vision infinite 0 true");
    w.cmd("effect give @s minecraft:speed 30 1");
    let fx = |w: &World, id: &str| w.p.s.life.fx.list.iter().find(|e| e.id == id).map(|e| (e.visible, e.icon));
    assert_eq!(fx(&w, "minecraft:night_vision"), Some((false, false)), "hidden particles hide the icon");
    assert_eq!(fx(&w, "minecraft:speed"), Some((true, true)), "shown particles show the icon");
}

/// `recipe give|take` count the book displays: the map copy shows 8 (one per number of maps).
#[test]
fn recipe_counts_book_displays() {
    let mut w = world("recipe");
    let _ = w.run("recipe take @s *");
    assert_eq!(w.cmd("recipe give @s minecraft:map_cloning"), 8, "8 displays learned");
    assert_eq!(w.run("recipe give @s minecraft:map_cloning"), (false, 0), "known: nothing learned");
    assert_eq!(w.cmd("recipe take @s minecraft:map_cloning"), 8, "8 displays forgotten");
    assert_eq!(w.cmd("recipe give @s minecraft:stick"), 1, "a plain recipe shows one");
}

/// Stopwatches are read when the world opens, not at the first stopwatch command: a session
/// without any stopwatch command still writes them back at its save.
#[test]
fn stopwatches_resume_at_world_opening() {
    let w = world("stopwatch");
    let World { p, dir } = w;
    let data = p.s.data.clone();
    drop(p);
    let file = dir.join("data").join("minecraft").join("stopwatches.dat");
    let root = Nbt::Compound(vec![
        ("data".into(), Nbt::Compound(vec![("stopwatches".into(), Nbt::Compound(vec![("t:a".into(), Nbt::Long(5000))]))])),
        ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
    ]);
    std::fs::create_dir_all(file.parent().expect("data folder")).expect("data folder");
    std::fs::write(&file, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).expect("stopwatches written");
    let mut p = crate::tests_g3f13::reopen(&dir, data);
    let (_, accumulated) = p.s.cmd.s9.stopwatches.get("t:a").copied().expect("read at the opening, before any command");
    assert_eq!(accumulated, 5000);
    std::fs::remove_file(&file).expect("stopwatches removed");
    p.s.save(true);
    let back = crate::nbtio::read_dat(&file).expect("written back by the save without any stopwatch command");
    let ms = back.get("data").and_then(|d| d.get("stopwatches")).and_then(|s| s.get("t:a")).and_then(Nbt::as_i64);
    assert!(ms.is_some_and(|v| v >= 5000), "the elapsed time kept and counting: {ms:?}");
}

/// A block entity whose data a command writes (`data merge`, `setblock` over the same block,
/// `clone … strict`) is marked changed: the comparator behind it reads the new contents.
#[test]
fn block_data_written_by_commands_tells_comparators() {
    let mut w = world("comparators");
    let (merged, set, cloned, source) = ([2, Y, 6], [6, Y, 6], [10, Y, 6], [14, Y, 2]);
    for at in [merged, set, cloned] {
        w.cmd(&format!("setblock {} minecraft:chest", pos(at)));
        // The comparator reads the block north of it.
        w.cmd(&format!("setblock {} {} {} minecraft:comparator[facing=north]", at[0], at[1], at[2] + 1));
    }
    let stones = "{Items:[{Slot:0b,id:\"minecraft:stone\",count:64}]}";
    w.cmd(&format!("setblock {} minecraft:chest{stones}", pos(source)));
    w.ticks(4);
    let lit = |w: &World, at: [i32; 3]| w.state([at[0], at[1], at[2] + 1]).contains("powered=true");
    assert!(!lit(&w, merged) && !lit(&w, set) && !lit(&w, cloned), "empty chests: comparators off");
    w.cmd(&format!("data merge block {} {stones}", pos(merged)));
    // Strict: no neighbour update after the placement, only the "changed" mark can tell it.
    w.cmd(&format!("setblock {} minecraft:chest{stones} strict", pos(set)));
    w.cmd(&format!("clone {} {} {} strict", pos(source), pos(source), pos(cloned)));
    w.ticks(4);
    assert_eq!(
        (lit(&w, merged), lit(&w, set), lit(&w, cloned)),
        (true, true, true),
        "data merge, setblock with new data and clone each tell their comparator"
    );
}

/// `damage` on the decorations and the shulker bullet, each family as the game takes it.
#[test]
fn damage_reaches_decorations_and_the_bullet() {
    let mut w = world("damage");
    // Armor stand: a wobble (the hit lands), then out-of-world damage kills it but answers as
    // invulnerable, like the game.
    w.cmd("summon minecraft:armor_stand 2 -60 2 {Tags:[\"hit\"]}");
    // Past the world's first 5 ticks: within them the game's last-hit time (0) breaks it at once.
    w.ticks(10);
    assert!(w.run("damage @e[tag=hit,limit=1] 1 minecraft:player_attack").0, "the stand wobbles");
    assert!(w.exists("@e[tag=hit]"), "one hit does not break it");
    assert!(!w.run("damage @e[tag=hit,limit=1] 1 minecraft:out_of_world").0, "answered as invulnerable");
    assert!(!w.exists("@e[tag=hit]"), "killed all the same");
    // Fire lights a stand (answered as invulnerable) and burns it down within 5 seconds.
    w.cmd("summon minecraft:armor_stand 4 -60 2 {Tags:[\"fire\"]}");
    assert!(!w.run("damage @e[tag=fire,limit=1] 1 minecraft:in_fire").0, "fire answers as invulnerable");
    assert_eq!(w.run("data get entity @e[tag=fire,limit=1] Fire"), (true, 100), "lit for 5 seconds");
    w.ticks(100);
    assert!(!w.exists("@e[tag=fire]"), "burnt down by its fire");
    // Mannequin: a living entity.
    w.cmd("summon minecraft:mannequin 6 -60 2 {Tags:[\"m\"]}");
    assert!(w.run("damage @e[tag=m,limit=1] 5").0, "the mannequin is hurt");
    assert_eq!(w.run("data get entity @e[tag=m,limit=1] Health"), (true, 15));
    // Item frame: the item drops first, then the frame breaks.
    w.cmd("summon minecraft:item_frame 8 -60 2 {Facing:1b,Tags:[\"f\"],Item:{id:\"minecraft:diamond\",count:1}}");
    assert!(w.run("damage @e[tag=f,limit=1] 1").0, "the frame drops its item");
    assert!(w.exists("@e[tag=f]") && !w.exists("@e[tag=f,nbt={Item:{}}]"), "the frame stays, empty");
    assert!(w.exists("@e[type=minecraft:item,nbt={Item:{id:\"minecraft:diamond\"}}]"), "the diamond dropped");
    assert!(w.run("damage @e[tag=f,limit=1] 1").0, "then the frame breaks");
    assert!(!w.exists("@e[tag=f]"));
    // Paintings on a wall: one breaks, an invulnerable one refuses.
    w.cmd("fill 9 -60 3 13 -58 3 minecraft:stone");
    w.cmd("summon minecraft:painting 10 -59 2 {facing:2b,variant:\"minecraft:kebab\",Tags:[\"p\"]}");
    w.cmd("summon minecraft:painting 12 -59 2 {facing:2b,variant:\"minecraft:kebab\",Tags:[\"q\"],Invulnerable:1b}");
    assert!(w.exists("@e[tag=p]") && w.exists("@e[tag=q]"), "paintings hung");
    assert!(w.run("damage @e[tag=p,limit=1] 1").0, "the painting breaks");
    assert!(!w.exists("@e[tag=p]"));
    assert!(!w.run("damage @e[tag=q,limit=1] 1").0, "an invulnerable painting refuses");
    assert!(w.exists("@e[tag=q]"));
    // Shulker bullet: destroyed by any hit. Live projectiles are not selector candidates here
    // yet, so the hit is dealt the way `damage` routes it.
    w.cmd("summon minecraft:shulker_bullet 14 -58 2");
    let bullet = w.p.s.proj.list.iter().find(|p| !p.removed).map(|p| p.id).expect("bullet summoned");
    assert!(w.p.s.other_entity_hurt(bullet, "minecraft:generic", 1.0, None, None), "the bullet takes the hit");
    assert!(w.p.s.proj.list.iter().all(|p| p.id != bullet || p.removed), "destroyed");
    // A marker never takes damage.
    w.cmd("summon minecraft:marker 16 -60 2 {Tags:[\"k\"]}");
    assert!(!w.run("damage @e[tag=k,limit=1] 1").0);
}

/// The integrated server flattens its texts in the language its client handed over: `name=`
/// finds an unnamed pig by its French name, and no longer by its English one.
#[test]
fn server_texts_follow_the_client_language() {
    let mut w = world("language");
    w.cmd("summon minecraft:pig 2 -60 2");
    assert!(w.exists("@e[type=minecraft:pig,name=Pig]"), "English before any table");
    let french = rc_text::Lang::load(b"{}", Some(br#"{"entity.minecraft.pig":"Cochon"}"#), None);
    *w.p.s.language_req.lock().expect("language request") = Some(Arc::new(french));
    w.p.s.take_language_change();
    let found = (w.exists("@e[type=minecraft:pig,name=Cochon]"), w.exists("@e[type=minecraft:pig,name=Pig]"));
    // The table belongs to this test's thread: dropped before any check can fail.
    crate::commands::cmds_ents::set_client_language(None);
    assert_eq!(found, (true, false), "the pig is named in the client's language");
}

/// A stack a command creates is checked strictly, with the game's reason.
#[test]
fn command_stacks_are_checked_strictly() {
    let mut w = world("strict");
    let _ = w.chat();
    assert!(!w.run("give @s minecraft:diamond_sword[max_stack_size=64] 64").0, "damageable and stackable refused");
    let said = w.chat();
    assert!(
        said.iter().any(|m| m.contains("arguments.item.malformed") && m.contains("Item cannot be both damageable and stackable")),
        "{said:?}"
    );
    assert!(w.p.s.slots.iter().flatten().all(|s| s.id != "minecraft:diamond_sword"), "nothing given");
    let full = "minecraft:bundle[bundle_contents=[{id:\"minecraft:diamond_sword\",count:2}]]";
    assert!(!w.run(&format!("give @s {full}")).0, "held stack over its maximum refused");
    let said = w.chat();
    assert!(said.iter().any(|m| m.contains("Item stack with count of 2 was larger than maximum: 1")), "{said:?}");
    assert!(!w.run("item replace entity @s weapon.mainhand with minecraft:diamond_sword[max_stack_size=2]").0);
    assert!(!w.run("give @s minecraft:stone[enchantments=\"x\"]").0, "a plain value for a compound refused");
    let said = w.chat();
    assert!(said.iter().any(|m| m.contains("arguments.item.component.malformed") && m.contains("Not a map")), "{said:?}");
    assert!(w.run("give @s minecraft:stone[enchantments={\"minecraft:sharpness\":2}]").0, "a compound is accepted");
}

/// `ride` refuses a player standing in another world than the vehicle's.
#[test]
fn ride_refuses_a_player_in_another_world() {
    let mut w = world("ride");
    w.cmd("execute in minecraft:the_nether run forceload add 0 0");
    w.cmd("execute in minecraft:the_nether run summon minecraft:pig 0 70 0 {NoAI:1b,Tags:[\"v\"]}");
    let _ = w.chat();
    assert!(!w.run("execute in minecraft:the_nether run ride @s mount @e[tag=v,limit=1]").0, "another world refused");
    let said = w.chat();
    assert!(said.iter().any(|m| m.contains("commands.ride.mount.failure.wrong_dimension")), "{said:?}");
}
