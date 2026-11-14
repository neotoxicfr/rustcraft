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
