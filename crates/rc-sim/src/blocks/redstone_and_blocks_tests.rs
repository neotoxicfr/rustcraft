//! Block and redstone behaviours checked against the game: the author of a TNT chain, water
//! poured on lit candles, the falling bit of a liquid left by a removal, sky light and endless
//! fire read from the dimension type. On a copy of the test world (`RUSTCRAFT_SOLO_WORLD`).

use super::*;
use crate::blocks::entities::EntKind;
use crate::blocks::probe::{Probe, state_string};
use crate::mobs::explode::Blast;
use crate::mobs::mob::Ent;
use crate::mobs::species::Kind as MobKind;
use rc_data::PropKey;
use rc_proto::nbt::Nbt;

const BASE: &str = "fill 100 -60 100 131 -60 131 minecraft:stone\nfill 100 -59 100 131 -40 131 minecraft:air\n";

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    assert_eq!(name_at(&p, [100, -60, 100]), "minecraft:stone", "test zone missing");
    // Far from the blasts and in survival, so that kills are the player's own.
    p.s.pos = [102.5, -59.0, 129.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    Some((p, dir))
}

fn name_at(p: &Probe, at: P) -> String {
    state_string(&p.s, p.s.get(at))
}

/// Entity `ty` summoned at `pos` with the SNBT `nbt`; its identifier.
fn summon(p: &mut Probe, ty: &str, pos: [f64; 3], nbt: &str) -> i32 {
    let src = p.s.player_source();
    let n = crate::commands::snbt::parse(nbt).expect("snbt");
    match p.s.summon_entity(&src, ty, pos, Some(n)) {
        Ok(crate::commands::ents::E::Other(id)) => id,
        r => panic!("summon {ty}: {r:?}"),
    }
}

/// Primed TNTs still in the world, with their owner.
fn primed_tnt_owners(p: &Probe) -> Vec<Option<Ent>> {
    p.s.bs.entities.list.iter().filter(|e| !e.removed && matches!(e.kind, EntKind::Tnt(_))).map(|e| e.owner).collect()
}

// ---------- The author of a TNT chain ----------

/// A TNT the player primed blows up a second TNT: the chained one is primed for the player too,
/// so a pig that only the second blast reaches dies as a player kill and drops experience.
/// Before, the chained TNT had no owner: no experience.
#[test]
fn a_chained_tnt_keeps_the_player_as_its_author() {
    let Some((mut p, _dir)) = probe("chain-tnt-author") else { return };
    p.run("setblock 112 -59 110 minecraft:tnt\n");
    p.s.tnt_prime_by([110, -59, 110], Some(Ent::Player));
    p.run("tick 81\n");
    assert_eq!(name_at(&p, [112, -59, 110]), "minecraft:air", "second TNT blown up");
    assert_eq!(primed_tnt_owners(&p), [Some(Ent::Player)], "chained TNT primed for the player");
    // Out of the first blast's reach (8.5 blocks), inside the chained one's (6.5 blocks).
    let mut pig = p.s.new_mob(MobKind::Pig, [119.0, -59.0, 110.5], 0.0);
    pig.health = 1.0;
    let pig_id = pig.id;
    p.s.add_mob(pig);
    // The chained fuse lasts at most 29 ticks (the probe ticks blocks and their entities, not
    // the mobs: the pig's death is read from its state, its experience follows from it).
    p.run("tick 30\n");
    assert!(primed_tnt_owners(&p).is_empty(), "chained TNT exploded");
    let pig = p.s.mob_ref(pig_id).expect("pig");
    assert!(pig.health <= 0.0, "pig killed by the chained blast");
    assert!(pig.last_hurt_by_player > 0, "killed by the player: it drops experience");
}

/// The other authors of a chain: a crystal's blast names nobody (its own entity is no living
/// author), and an owner mob that has left the world (a creeper is gone right after its blast)
/// no longer answers when the TNT it chained explodes.
#[test]
fn a_chained_tnt_follows_the_game_for_crystals_and_vanished_mobs() {
    let Some((mut p, _dir)) = probe("chain-tnt-others") else { return };
    p.run("setblock 112 -59 110 minecraft:tnt\n");
    let center = [110.5, -59.0, 110.5];
    p.s.explode_with(None, Some(Ent::Player), None, center, 6.0, false, "minecraft:player_explosion", Blast::Block);
    assert_eq!(primed_tnt_owners(&p), [None], "behind a crystal's blast: no author");
    assert_eq!(p.s.tnt_owner_still_present(Some(Ent::Player)), Some(Ent::Player));
    assert_eq!(p.s.tnt_owner_still_present(Some(Ent::Mob(987_654))), None, "gone creeper: no author");
}

// ---------- Water on candles ----------

/// Water poured on lit candles waterlogs them and puts them out with the candle's own sound.
/// Before, they stayed lit under the water.
#[test]
fn water_poured_on_lit_candles_puts_them_out() {
    let Some((mut p, _dir)) = probe("candle-water") else { return };
    let at = [110, -59, 110];
    p.run("setblock 110 -59 110 minecraft:candle[candles=3,lit=true]\n");
    let _ = p.take_events();
    let s = p.s.get(at);
    p.s.place_water_in(at, s);
    assert_eq!(name_at(&p, at), "minecraft:candle[candles=3,lit=false,waterlogged=true]");
    let put_out = p.s.data.registry_id("minecraft:sound_event", "minecraft:block.candle.extinguish").expect("sound") as i32;
    let heard = p.take_events().iter().any(|e| match e {
        rc_client::Event::Sound(s) => s.sound == rc_proto_v777::play::SoundRef::Id(put_out),
        _ => false,
    });
    assert!(heard, "candle put-out sound");
}

// ---------- The liquid left by a removal ----------

/// Removing or destroying a falling liquid block leaves it falling (the game rebuilds the block
/// from the fluid as it is). Before, the falling bit was dropped: a falling water block (amount
/// 8) came back as a water source.
#[test]
fn removing_falling_water_keeps_it_falling() {
    let Some((mut p, _dir)) = probe("falling-water-removal") else { return };
    let falling = p.s.view().set_int(p.s.default_state("minecraft:water"), PropKey::Level, 8);
    for (at, destroy) in [([110, -55, 110], true), ([112, -55, 110], false)] {
        p.s.set_block(at, falling, 2);
        if destroy {
            p.s.destroy_block(at, false, 512);
        } else {
            p.s.remove_block(at, false);
        }
        assert_eq!(name_at(&p, at), "minecraft:water[level=8]", "destroyed: {destroy}");
    }
}

// ---------- Decorations on trigger blocks ----------

/// The touch box of a pressure plate at `at`.
fn plate_box(at: P) -> rc_physics::Aabb {
    let [x, y, z] = at.map(f64::from);
    rc_physics::Aabb { min: [x + 0.0625, y, z + 0.0625], max: [x + 0.9375, y + 0.25, z + 0.9375] }
}

/// An armor stand or a mannequin standing on a stone pressure plate presses it (both are living
/// entities for the plate); a marker stand ignores trigger blocks. Before, plates and tripwire
/// never counted the decorations, and a living decoration never touched the blocks it stood on.
#[test]
fn armor_stands_and_mannequins_press_pressure_plates() {
    let Some((mut p, _dir)) = probe("plate-decorations") else { return };
    let (stand_at, mannequin_at, marker_at) = ([110, -59, 110], [114, -59, 110], [118, -59, 110]);
    for at in [stand_at, mannequin_at, marker_at] {
        p.run(&format!("setblock {} {} {} minecraft:stone_pressure_plate\n", at[0], at[1], at[2]));
    }
    let feet = |at: P| [f64::from(at[0]) + 0.5, f64::from(at[1]), f64::from(at[2]) + 0.5];
    summon(&mut p, "minecraft:armor_stand", feet(stand_at), "{}");
    summon(&mut p, "minecraft:mannequin", feet(mannequin_at), "{}");
    summon(&mut p, "minecraft:armor_stand", feet(marker_at), "{Marker:1b}");
    assert_eq!(p.s.count_entities_in(&plate_box(stand_at), true), 1, "armor stand counted as living");
    assert_eq!(p.s.count_entities_in(&plate_box(mannequin_at), true), 1, "mannequin counted as living");
    assert_eq!(p.s.count_entities_in(&plate_box(marker_at), false), 0, "marker stand ignored");
    p.s.deco_tick();
    assert_eq!(name_at(&p, stand_at), "minecraft:stone_pressure_plate[powered=true]", "pressed by the stand");
    assert_eq!(name_at(&p, mannequin_at), "minecraft:stone_pressure_plate[powered=true]", "pressed by the mannequin");
    assert_eq!(name_at(&p, marker_at), "minecraft:stone_pressure_plate[powered=false]", "marker: no press");
}

// ---------- Placeholder block entities ----------

/// A placeholder entry (id `DUMMY`) at `at`, as the game's generator writes it.
fn placeholder_entry(at: P, packed: bool) -> Nbt {
    Nbt::Compound(vec![
        ("id".into(), Nbt::String("DUMMY".into())),
        ("x".into(), Nbt::Int(at[0])),
        ("y".into(), Nbt::Int(at[1])),
        ("z".into(), Nbt::Int(at[2])),
        ("keepPacked".into(), Nbt::Byte(i8::from(packed))),
    ])
}

/// Ids of the block entity entries of the chunk holding `at`, among positions `of`.
fn entry_ids(p: &Probe, at: P, of: &[P]) -> Vec<String> {
    let c = p.s.chunks.get(&(at[0] >> 4, at[2] >> 4)).cloned().flatten().expect("chunk");
    let pos = |b: &Nbt| ["x", "y", "z"].map(|k| crate::nbtio::int(b, k).unwrap_or(0) as i32);
    let id = |b: &Nbt| b.get("id").and_then(Nbt::as_str).unwrap_or("").to_owned();
    c.block_entities.iter().filter(|b| of.contains(&pos(b))).map(id).collect()
}

/// Placeholder block entities (id `DUMMY`) the game's generator leaves for the deep dark's sculk
/// sensors become, when promoted (packed: by the chunk save; unpacked: when the chunk is read),
/// real sensor entities that listen to vibrations; on a block without a block entity they are
/// dropped. Before, they stayed deaf `DUMMY` entries and were written back as such, which the game
/// then discards for good.
#[test]
fn placeholder_block_entities_become_the_blocks_own() {
    let Some((mut p, _dir)) = probe("placeholder-block-entity") else { return };
    let (packed_sensor, stone, plain_sensor) = ([110, -59, 110], [108, -59, 110], [106, -59, 110]);
    p.run(
        "setblock 110 -59 110 minecraft:sculk_sensor\nsetblock 108 -59 110 minecraft:stone\n\
         setblock 106 -59 110 minecraft:sculk_sensor\n",
    );
    let ch = (packed_sensor[0] >> 4, packed_sensor[2] >> 4);
    let all = [packed_sensor, stone, plain_sensor];
    // As read from a world the game generated: no listener yet, placeholders instead of entities.
    for at in [packed_sensor, plain_sensor] {
        p.s.mobs.rec.vib_blocks.remove(&at);
    }
    {
        let c = p.s.chunks.get_mut(&ch).and_then(Option::as_mut).expect("chunk");
        c.block_entities.clear();
        c.block_entities.push(placeholder_entry(packed_sensor, true));
        c.block_entities.push(placeholder_entry(stone, true));
    }
    p.s.bs.packed.insert(ch, vec![packed_sensor, stone]);
    p.s.promote_packed_for_save(ch);
    assert_eq!(entry_ids(&p, packed_sensor, &all), ["minecraft:sculk_sensor"], "packed: replaced or dropped");
    assert!(p.s.mobs.rec.vib_blocks.contains_key(&packed_sensor), "the promoted sensor listens");
    if let Some(Some(c)) = p.s.chunks.get_mut(&ch) {
        c.block_entities.push(placeholder_entry(plain_sensor, false));
    }
    p.s.register_chunk_tickers(ch);
    assert_eq!(entry_ids(&p, plain_sensor, &[plain_sensor]), ["minecraft:sculk_sensor"], "unpacked: replaced");
    assert!(p.s.mobs.rec.vib_blocks.contains_key(&plain_sensor), "the unpacked one listens too");
}

// ---------- Adventure mode predicates ----------

/// A tool whose `can_break` holds the single predicate `predicate`.
fn tool_that_breaks(predicate: Nbt) -> crate::player::Stack {
    let components = Nbt::Compound(vec![("minecraft:can_break".into(), predicate)]);
    crate::player::Stack { id: "minecraft:diamond_pickaxe".into(), count: 1, components: Some(components) }
}

fn text(v: &str) -> Nbt {
    Nbt::String(v.into())
}

/// In adventure, a pickaxe whose `can_break` names oak logs of the vertical axis breaks those and
/// only those: the player starts mining a vertical log, a horizontal one is refused. Before, the
/// predicate's state was not read, and the singleplayer server refused every adventure break.
#[test]
fn adventure_can_break_reads_the_block_state() {
    let Some((mut p, _dir)) = probe("adventure-state") else { return };
    let (upright, lying) = ([110, -59, 110], [112, -59, 110]);
    p.run("setblock 110 -59 110 minecraft:oak_log[axis=y]\nsetblock 112 -59 110 minecraft:oak_log[axis=x]\n");
    let predicate = Nbt::Compound(vec![
        ("blocks".into(), text("minecraft:oak_log")),
        ("state".into(), Nbt::Compound(vec![("axis".into(), text("y"))])),
    ]);
    p.s.game_mode = 2;
    p.s.pos = [111.5, -59.0, 111.5];
    let held = p.s.held as usize;
    p.s.slots[36 + held] = Some(tool_that_breaks(predicate));
    assert!(!p.s.player_destroy_block(lying), "horizontal log refused");
    assert_eq!(name_at(&p, lying), "minecraft:oak_log[axis=x]");
    p.s.on_action(0, upright, 1, 1);
    assert!(p.s.mining.is_some_and(|m| m.active && m.pos == upright), "mining the vertical log started");
    assert!(p.s.player_destroy_block(upright), "vertical log broken");
    assert_eq!(name_at(&p, upright), "minecraft:air");
}

/// A `can_break` predicate on block entity data accepts a chest holding a diamond and refuses an
/// empty one; the game's memory of the last block then answers for every chest of the same
/// state: refused on the empty one, the diamond chest stays refused until another state is
/// tested. Before, the data was not read: every chest was accepted.
#[test]
fn adventure_can_break_reads_block_data_and_remembers_the_last_block() {
    let Some((mut p, _dir)) = probe("adventure-data") else { return };
    let (rich, empty, stone) = ([110, -59, 110], [112, -59, 110], [114, -59, 110]);
    p.run(
        "setblock 110 -59 110 minecraft:chest[facing=north]\nsetblock 112 -59 110 minecraft:chest[facing=north]\n\
         setblock 114 -59 110 minecraft:stone\ndata merge block 110 -59 110 {Items:[{Slot:0b,id:\"minecraft:diamond\",count:1}]}\n",
    );
    let wanted = Nbt::Compound(vec![("Items".into(), Nbt::List(vec![Nbt::Compound(vec![("id".into(), text("minecraft:diamond"))])]))]);
    let tool = tool_that_breaks(Nbt::Compound(vec![("blocks".into(), text("minecraft:chest")), ("nbt".into(), wanted)]));
    let mut verdicts = Vec::new();
    for at in [rich, rich, empty, rich, stone, rich] {
        verdicts.push(p.s.adventure_can_break(Some(&tool), at));
    }
    assert_eq!(verdicts, [true, true, false, false, false, true]);
}

// ---------- Sculk catalyst ----------

/// A zombie the player kills 2 blocks from a sculk catalyst drops no experience: the catalyst
/// blooms (8 ticks) and keeps the 5 points as a charge just above the body. On its next tick
/// the charge lays a vein in the empty cell and the vein turns the stone under it into sculk.
/// The same zombie far from any catalyst drops its orbs. Before, catalysts did nothing.
#[test]
fn a_sculk_catalyst_takes_a_nearby_death_and_spreads_sculk() {
    let Some((mut p, _dir)) = probe("sculk-catalyst") else { return };
    let catalyst = [110, -59, 110];
    p.run("setblock 110 -59 110 minecraft:sculk_catalyst\n");
    let die_at = |p: &mut Probe, at: [f64; 3]| {
        let mut zombie = p.s.new_mob(MobKind::Zombie, at, 0.0);
        zombie.last_hurt_by_player = 100;
        zombie.health = 0.0;
        p.s.mob_die(&mut zombie, "minecraft:player_attack", Some(Ent::Player));
    };
    let fresh = p.s.be_get(catalyst).and_then(|be| be.get("cursors").cloned());
    assert_eq!(fresh, Some(Nbt::List(Vec::new())), "a new catalyst saves an empty charge list");
    crate::nbtio::set(&mut p.s.rules, "minecraft:mob_drops", Nbt::Byte(1));
    die_at(&mut p, [125.5, -59.0, 125.5]);
    assert!(!p.s.life.orbs.is_empty(), "far from the catalyst: orbs dropped");
    p.s.life.orbs.clear();
    die_at(&mut p, [112.5, -59.0, 110.5]);
    assert!(p.s.life.orbs.is_empty(), "near the catalyst: no orb");
    assert_eq!(name_at(&p, catalyst), "minecraft:sculk_catalyst[bloom=true]");
    let cursors = p.s.be_get(catalyst).and_then(|be| be.get("cursors").cloned());
    // The delays still hold their defaults (1 and 0), which the game leaves out of the save.
    let charge = |at: [i32; 3], amount: i32| {
        Nbt::Compound(vec![("pos".into(), Nbt::IntArray(at.to_vec())), ("charge".into(), Nbt::Int(amount))])
    };
    assert_eq!(cursors, Some(Nbt::List(vec![charge([112, -59, 110], 5)])), "the zombie's 5 points as a charge");
    p.s.tick_block_entities();
    assert_eq!(name_at(&p, [112, -60, 110]), "minecraft:sculk", "the stone under the vein turned into sculk");
    p.run("tick 9\n");
    assert_eq!(name_at(&p, catalyst), "minecraft:sculk_catalyst[bloom=false]", "bloom over after 8 ticks");
}

/// The charges a catalyst's block entity holds.
fn catalyst_charges(p: &Probe, at: P) -> Vec<i32> {
    let Some(Nbt::List(cursors)) = p.s.be_get(at).and_then(|be| be.get("cursors")) else { return Vec::new() };
    cursors.iter().filter_map(|c| c.get("charge").and_then(Nbt::as_i64)).map(|c| c as i32).collect()
}

/// An armor stand broken by a player: two hits in a row.
fn break_stand(p: &mut Probe, id: i32) {
    for _ in 0..2 {
        p.s.mobs.attack_ticker = 100;
        assert!(p.s.deco_attacked(id), "stand out of reach");
    }
    assert!(p.s.deco_nbt(id).is_none(), "stand still standing");
}

/// A level 10 player breaks an armor stand 3 blocks from a catalyst. The game names the player
/// as the dead, so the catalyst takes the player's 70 points as charges at the stand; a second
/// stand broken next to another catalyst then blooms nothing, and the player's own death, far
/// from both, drops no orb. Before, the first catalyst bloomed with nothing and the death
/// dropped the orbs.
#[test]
fn a_stand_broken_by_the_player_gives_the_players_experience_to_a_catalyst() {
    let Some((mut p, _dir)) = probe("catalyst-stand-player") else { return };
    let (first, second) = ([110, -59, 110], [120, -59, 110]);
    p.run("setblock 110 -59 110 minecraft:sculk_catalyst\nsetblock 120 -59 110 minecraft:sculk_catalyst\n");
    let near_first = summon(&mut p, "minecraft:armor_stand", [113.5, -59.0, 110.5], "{}");
    let near_second = summon(&mut p, "minecraft:armor_stand", [122.5, -59.0, 110.5], "{}");
    p.s.life.vit.xp_level = 10;
    p.s.pos = [113.5, -59.0, 112.5];
    break_stand(&mut p, near_first);
    assert_eq!(name_at(&p, first), "minecraft:sculk_catalyst[bloom=true]");
    let cursors = p.s.be_get(first).and_then(|be| be.get("cursors").cloned());
    let charge = Nbt::Compound(vec![("pos".into(), Nbt::IntArray(vec![113, -59, 110])), ("charge".into(), Nbt::Int(70))]);
    assert_eq!(cursors, Some(Nbt::List(vec![charge])), "the player's 70 points, at the stand");
    p.s.pos = [122.5, -59.0, 112.5];
    break_stand(&mut p, near_second);
    assert_eq!(name_at(&p, second), "minecraft:sculk_catalyst[bloom=false]", "the experience is already taken");
    assert!(catalyst_charges(&p, second).is_empty());
    p.s.pos = [102.5, -59.0, 129.5];
    p.s.life.orbs.clear();
    let src = p.s.server_source();
    assert!(p.s.perform(src, "kill @p").0, "kill refused");
    assert!(p.s.life.vit.dead, "the player did not die");
    assert!(p.s.life.orbs.is_empty(), "the player's experience went to the catalyst: no orb");
}

/// A blast's author is the dead the catalysts hear for the decorations it destroys. Without an
/// author, an item frame is not heard and an armor stand blooms the catalyst with nothing to
/// give. The player's blast gives the player's 70 points; a creeper's blast gives its 5 points,
/// and its own death later drops no orb. Before, no blast ever reached a catalyst.
#[test]
fn a_blast_gives_its_authors_experience_for_the_decorations_it_destroys() {
    let Some((mut p, _dir)) = probe("catalyst-blast-author") else { return };
    let catalyst = [110, -59, 110];
    p.run("setblock 110 -59 110 minecraft:sculk_catalyst\n");
    crate::nbtio::set(&mut p.s.rules, "minecraft:mob_drops", Nbt::Byte(1));
    let (at, center) = ([107.5, -59.0, 110.5], [108.5, -59.0, 110.5]);
    let frame = "{Facing:1b,block_pos:[I;107,-59,110]}";
    let blast = |p: &mut Probe, ty: &str, by: Option<Ent>| p.s.deco_explosion(center, 4.0, ty, by, false, true);

    summon(&mut p, "minecraft:item_frame", at, frame);
    blast(&mut p, "minecraft:explosion", None);
    assert_eq!(name_at(&p, catalyst), "minecraft:sculk_catalyst[bloom=false]", "a frame without author is not heard");

    summon(&mut p, "minecraft:armor_stand", at, "{}");
    blast(&mut p, "minecraft:explosion", None);
    assert_eq!(name_at(&p, catalyst), "minecraft:sculk_catalyst[bloom=true]", "a stand without author is its own dead");
    assert!(catalyst_charges(&p, catalyst).is_empty(), "nothing to give");
    p.run("tick 9\n");

    p.s.life.vit.xp_level = 10;
    summon(&mut p, "minecraft:item_frame", at, frame);
    blast(&mut p, "minecraft:player_explosion", Some(Ent::Player));
    assert_eq!(name_at(&p, catalyst), "minecraft:sculk_catalyst[bloom=true]");
    assert_eq!(catalyst_charges(&p, catalyst), [70], "the player's points for the frame");

    let src = p.s.player_source();
    let creeper = match p.s.summon_entity(&src, "minecraft:creeper", [125.5, -59.0, 125.5], None) {
        Ok(crate::commands::ents::E::Mob(id)) => id,
        r => panic!("summon creeper: {r:?}"),
    };
    summon(&mut p, "minecraft:armor_stand", at, "{}");
    blast(&mut p, "minecraft:player_explosion", Some(Ent::Mob(creeper)));
    assert_eq!(catalyst_charges(&p, catalyst), [70, 5], "the creeper's points for the stand");
    let i = p.s.mob_index(creeper).expect("creeper");
    let mut dying = p.s.mobs.list.take(i).expect("creeper");
    dying.last_hurt_by_player = 100;
    dying.health = 0.0;
    p.s.life.orbs.clear();
    p.s.mob_die(&mut dying, "minecraft:player_attack", Some(Ent::Player));
    p.s.mobs.list.put(i, dying);
    assert!(p.s.life.orbs.is_empty(), "the creeper's experience went to the catalyst: no orb");
}

// ---------- The dimension type decides ----------

/// A data pack dimension whose type has sky light ticks its daylight detectors like the
/// overworld. Before, only the dimension named `minecraft:overworld` did.
#[test]
fn a_daylight_detector_works_in_any_dimension_with_sky_light() {
    let Some((mut p, _dir)) = probe("daylight-any-sky") else { return };
    let at = [110, -59, 110];
    p.s.clocks.retain(|c| c.0 != "minecraft:overworld");
    p.s.clocks.push(("minecraft:overworld".into(), 6_000));
    p.run("setblock 110 -59 110 minecraft:daylight_detector\ntick 2\n");
    assert_eq!(p.s.light_at(at, true), 15, "open sky above the detector");
    p.s.wgs.add_dimension_for_test("test:sunny_meadow", "minecraft:overworld");
    p.s.dim.rename("test:sunny_meadow");
    p.s.daylight_update(at);
    let power = p.s.view().get_int(p.s.get(at), PropKey::Power);
    p.s.dim.rename("minecraft:overworld");
    assert!(power > 0, "detector powered at noon: {power}");
}

/// Endless fire follows the `infiniburn` tag the dimension type names: bedrock burns forever in
/// a data pack dimension of the End's type. Before, the tag was chosen by the dimension's name.
#[test]
fn endless_fire_follows_the_dimension_type() {
    let Some((mut p, _dir)) = probe("infiniburn-type") else { return };
    let bedrock = p.s.default_state("minecraft:bedrock");
    let netherrack = p.s.default_state("minecraft:netherrack");
    assert!(!p.s.infiniburn(bedrock), "overworld: bedrock does not burn forever");
    assert!(p.s.infiniburn(netherrack), "overworld: netherrack does");
    p.s.wgs.add_dimension_for_test("test:dark_isles", "minecraft:the_end");
    p.s.dim.rename("test:dark_isles");
    let end_like = p.s.infiniburn(bedrock);
    p.s.dim.rename("minecraft:overworld");
    assert!(end_like, "End type: bedrock burns forever");
}
