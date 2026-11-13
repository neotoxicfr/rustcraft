// Direct read allowed (tests: settings read at launch); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Saving of the structure pieces and placed mobs (G6-F19). Game data required
//! (jar 26.3 and rc-data cache); absent: tests skipped, except `RUSTCRAFT_OPT_REQUIRE=1`.

use std::sync::{Arc, Mutex};

use hashbrown::HashMap;

use rc_proto::nbt::Nbt;

use crate::chunkgen::DimGen;
use crate::decor_blocks::Dir;
use crate::decor_region::DecoChunk;
use crate::structure::tests::{dim, world_data};
use crate::structure::{Piece, Placement, Start};
use crate::structure_coded::CodedPiece;
use crate::structure_fortress::{FKind, FPiece};
use crate::structure_stronghold::{Door, ShKind, ShPiece};
use crate::structure_templ::{TKind, TemplPiece, VPlace};
use crate::template::{BBox, Mirror, Rot};
use crate::terrain::BiomeCache;

const SEED: i64 = 12345;

/// Seeds tried in order to find a rare structure near the origin.
const SEEDS: [i64; 6] = [SEED, 1, 42, 20261011, -4172144997902289642, 7];

/// First seed that has a start `id` at `radius` chunks or fewer: generator, start, seed.
fn find_seeded(w: &crate::chunkgen::WorldData, id: &str, radius: i32) -> (DimGen, Start, i64) {
    for seed in SEEDS {
        let dg = dim(w, Some(seed));
        if let Some(s) = find(&dg, id, radius) {
            return (dg, s, seed);
        }
    }
    panic!("{id} not found for the seeds {SEEDS:?}")
}

/// First start `id` in a spiral around the origin (radius in chunks).
fn find(dg: &DimGen, id: &str, radius: i32) -> Option<Start> {
    find_with(dg, |s| s == id, radius)
}

fn find_with(dg: &DimGen, ok: impl Fn(&str) -> bool, radius: i32) -> Option<Start> {
    let sg = dg.structures.clone()?;
    for r in 0..radius {
        for x in -r..=r {
            for z in -r..=r {
                if x.abs() != r && z.abs() != r {
                    continue;
                }
                if let Some(s) = dg.structure_starts(x, z).iter().find(|s| ok(&sg.defs[s.structure].id)) {
                    return Some(s.clone());
                }
            }
        }
    }
    None
}

/// Chunks of the start's box (in chunks).
fn box_chunks(s: &Start) -> Vec<(i32, i32)> {
    let (x0, z0, x1, z1) = (s.bbox.min[0] >> 4, s.bbox.min[2] >> 4, s.bbox.max[0] >> 4, s.bbox.max[2] >> 4);
    (x0..=x1).flat_map(|x| (z0..=z1).map(move |z| (x, z))).collect()
}

/// Terrain (with its `structures` compound, like the server) of the given chunks and their neighbours.
fn terrain(dg: &DimGen, centers: &[(i32, i32)]) -> HashMap<(i32, i32), DecoChunk> {
    let mut all: Vec<(i32, i32)> =
        centers.iter().flat_map(|&(x, z)| (0..9).map(move |i| (x + i % 3 - 1, z + i / 3 - 1))).collect();
    all.sort_unstable();
    all.dedup();
    let mut cache = BiomeCache::default();
    dg.prepare_biomes(&mut cache, &all);
    all.iter()
        .map(|&(x, z)| {
            let mut c = dg.terrain(x, z, &cache, None);
            c.structures = Some(dg.structures_nbt(x, z));
            ((x, z), c)
        })
        .collect()
}

fn decorate(dg: &DimGen, chunks: &mut HashMap<(i32, i32), DecoChunk>, (x, z): (i32, i32)) {
    let mut picked: Vec<DecoChunk> =
        (0..9).map(|i| chunks.remove(&(x + i % 3 - 1, z + i / 3 - 1)).expect("neighbouring terrain")).collect();
    {
        let mut it = picked.iter_mut();
        let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().unwrap());
        dg.decorate(refs);
    }
    for c in picked {
        chunks.insert((c.cx, c.cz), c);
    }
}

/// Piece `n` of the start `id` in the `structures` compound.
fn saved_child<'a>(structures: &'a Nbt, id: &str, n: usize) -> &'a Nbt {
    let Some(Nbt::List(ch)) = structures.get("starts").and_then(|s| s.get(id)).and_then(|s| s.get("Children")) else {
        panic!("start {id} missing from the compound");
    };
    &ch[n]
}

fn int(n: &Nbt, k: &str) -> i64 {
    n.get(k).and_then(Nbt::as_i64).unwrap_or_else(|| panic!("field {k} missing: {n:?}"))
}

fn marked(e: &Nbt) -> bool {
    e.get("rc_finalize_structure").is_some()
}

fn id_of(e: &Nbt) -> &str {
    e.get("id").and_then(Nbt::as_str).unwrap_or("?")
}

/// Swamp hut placed: recorded compound with height, moved box, dimensions and
/// flags; witch and cat marked for finalisation; the compound read back in a new
/// generator gives the same compound.
#[test]
fn swamp_hut_saves_moved_box_and_marks_mobs() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEED));
    let s = find(&dg, "minecraft:swamp_hut", 200).expect("swamp hut");
    let o = (s.cx, s.cz);
    let mut chunks = terrain(&dg, &box_chunks(&s));
    for p in box_chunks(&s) {
        decorate(&dg, &mut chunks, p);
    }
    let saved = chunks[&o].structures.clone().expect("compound");
    let hut = saved_child(&saved, "minecraft:swamp_hut", 0);
    eprintln!("hut {o:?}: {hut:?}");
    assert_eq!(hut.get("id").and_then(Nbt::as_str), Some("minecraft:tesh"));
    let hpos = int(hut, "HPos");
    assert!(hpos >= 0, "height not kept");
    assert_eq!((int(hut, "Width"), int(hut, "Height"), int(hut, "Depth")), (7, 7, 9));
    let Some(Nbt::IntArray(bb)) = hut.get("BB") else { panic!("BB") };
    assert_eq!((i64::from(bb[1]), i64::from(bb[4])), (hpos, hpos + 6), "box not moved");
    assert_eq!((int(hut, "Witch"), int(hut, "Cat")), (1, 1));
    let mobs: Vec<&Nbt> = chunks
        .values()
        .flat_map(|c| c.entities.iter())
        .filter(|e| matches!(id_of(e), "minecraft:witch" | "minecraft:cat"))
        .collect();
    assert_eq!(mobs.len(), 2, "{mobs:?}");
    for e in &mobs {
        assert!(marked(e), "mob not marked: {e:?}");
        // The recorded box contains the mob (spawns and cat variant read it).
        let Some(Nbt::List(pos)) = e.get("Pos") else { panic!("Pos") };
        let Nbt::Double(y) = pos[1] else { panic!("Pos") };
        assert!((f64::from(bb[1])..=f64::from(bb[4])).contains(&y), "mob outside the box: {y}");
    }
    // Re-read: new generator (restart), piece never yet placed.
    let dg2 = dim(w, Some(SEED));
    let (sg2, t2) = (dg2.structures.clone().unwrap(), dg2.structure_terrain().unwrap());
    let fresh = crate::structure_nbt::chunk_structures(&sg2, o.0, o.1, t2, &dg2.blocks);
    assert_eq!(int(saved_child(&fresh, "minecraft:swamp_hut", 0), "HPos"), -1);
    crate::structure_nbt::restore_placement_state(&sg2, o.0, o.1, t2, &dg2.blocks, &saved);
    let back = crate::structure_nbt::chunk_structures(&sg2, o.0, o.1, t2, &dg2.blocks);
    assert_eq!(saved_child(&back, "minecraft:swamp_hut", 0), hut);
}

/// Pyramid straddling two chunks, placed in two goes with a restart between them:
/// the second half takes up the height recorded by the first (same blocks as without
/// restart). Without re-read, the second half re-measures the ground and draws another offset.
#[test]
fn desert_pyramid_resumes_after_restart() {
    let Some(w) = world_data() else { return };
    let (dg, s, seed) = find_seeded(w, "minecraft:desert_pyramid", 120);
    let o = (s.cx, s.cz);
    let n = *box_chunks(&s).iter().find(|&&p| p != o).expect("pyramid over several chunks");
    let order = [n, o];
    // Reference: a single generator.
    let mut a = terrain(&dg, &order);
    for p in order {
        decorate(&dg, &mut a, p);
    }
    // Restart between the two placements.
    let dg_b = dim(w, Some(seed));
    let mut b = terrain(&dg_b, &order);
    decorate(&dg_b, &mut b, n);
    let child = saved_child(b[&o].structures.as_ref().unwrap(), "minecraft:desert_pyramid", 0).clone();
    eprintln!("pyramid {o:?} after the first half: {child:?}");
    assert!(int(&child, "HPos") >= 0, "height not recorded in the starting chunk");
    drop(dg_b);
    let dg_c = dim(w, Some(seed));
    decorate(&dg_c, &mut b, o);
    for p in box_chunks(&s) {
        assert!(a[&p].blocks == b[&p].blocks, "chunk {p:?} differs after restart");
    }
    let (ca, cb) = (a[&o].structures.as_ref().unwrap(), b[&o].structures.as_ref().unwrap());
    assert_eq!(saved_child(ca, "minecraft:desert_pyramid", 0), saved_child(cb, "minecraft:desert_pyramid", 0));
}

/// Shipwreck small enough to be measured at placement (the large ones are measured at
/// generation) and spread over several chunks.
fn is_measured_at_placement(s: &Start) -> bool {
    let Some(Piece::Coded { data, .. }) = s.pieces.first() else { return false };
    let CodedPiece::Template(tp) = &**data else { return false };
    matches!(&tp.kind, TKind::Shipwreck { adjusted, .. } if !*adjusted.lock().unwrap()) && box_chunks(s).len() > 1
}

/// Shipwreck placed in two goes with a restart between them, through the generator's own
/// decoration: the starting chunk's compound holds the height measured by the first part
/// (`height_adjusted` 1, `TPY` and `BB` at that height, what the official reader takes back), and
/// the second part placed after a restart is the same as without restart.
#[test]
fn shipwreck_saves_its_adjusted_height_and_resumes_after_restart() {
    let Some(w) = world_data() else { return };
    let (dg, s, seed) = SEEDS
        .iter()
        .find_map(|&seed| {
            let dg = dim(w, Some(seed));
            let sg = dg.structures.clone().expect("structures");
            // Beached: its height takes a random draw of the placing chunk, so a second measure
            // after a restart would differ (an ocean one measures the same generation heights).
            let wreck = |s: &Start| sg.defs[s.structure].id == "minecraft:shipwreck_beached" && is_measured_at_placement(s);
            let s = (-60..=60)
                .flat_map(|x| (-60..=60).map(move |z| (x, z)))
                .find_map(|(x, z)| dg.structure_starts(x, z).iter().find(|s| wreck(s)).cloned())?;
            Some((dg, s, seed))
        })
        .expect("a beached shipwreck over several chunks measured at placement");
    let name = dg.structures.as_ref().unwrap().defs[s.structure].id.clone();
    let o = (s.cx, s.cz);
    let n = *box_chunks(&s).iter().find(|&&p| p != o).expect("shipwreck over several chunks");
    let order = [n, o];
    // Reference: a single generator.
    let mut a = terrain(&dg, &order);
    for p in order {
        decorate(&dg, &mut a, p);
    }
    // Restart between the two placements.
    let dg_b = dim(w, Some(seed));
    let mut b = terrain(&dg_b, &order);
    let before = saved_child(b[&o].structures.as_ref().unwrap(), &name, 0).clone();
    assert_eq!(int(&before, "height_adjusted"), 0, "measured before any placement: {before:?}");
    decorate(&dg_b, &mut b, n);
    let child = saved_child(b[&o].structures.as_ref().unwrap(), &name, 0).clone();
    eprintln!("shipwreck {o:?} ({seed}) after the first part: {child:?}");
    assert_eq!(int(&child, "height_adjusted"), 1, "adjusted height not written in the starting chunk");
    let live_starts = dg_b.structure_starts(o.0, o.1);
    let live = live_starts.iter().find(|st| st.structure == s.structure).expect("live shipwreck");
    let Some(Piece::Coded { data, .. }) = live.pieces.first() else { panic!("shipwreck piece") };
    let CodedPiece::Template(tp) = &**data else { panic!("shipwreck template") };
    let live_y = tp.pos.lock().unwrap()[1];
    assert_eq!(int(&child, "TPY"), i64::from(live_y), "height written is not the placed one");
    let Some(Nbt::IntArray(bb)) = child.get("BB") else { panic!("BB") };
    assert_eq!(bb[1], live_y, "box not moved to the placed height");
    assert_ne!(int(&before, "TPY"), int(&child, "TPY"), "pointless check: the height did not move");
    drop(dg_b);
    // Read back by a new generator: the piece takes the recorded height instead of measuring.
    let dg_r = dim(w, Some(seed));
    let (sg_r, t_r) = (dg_r.structures.clone().unwrap(), dg_r.structure_terrain().unwrap());
    crate::structure_nbt::restore_placement_state(&sg_r, o.0, o.1, t_r, &dg_r.blocks, b[&o].structures.as_ref().unwrap());
    let back = crate::structure_nbt::chunk_structures(&sg_r, o.0, o.1, t_r, &dg_r.blocks);
    assert_eq!(saved_child(&back, &name, 0), &child, "recorded height not read back");
    drop(dg_r);
    let dg_c = dim(w, Some(seed));
    decorate(&dg_c, &mut b, o);
    for p in box_chunks(&s) {
        assert!(a[&p].blocks == b[&p].blocks, "chunk {p:?} differs after restart");
    }
    let (ca, cb) = (a[&o].structures.as_ref().unwrap(), b[&o].structures.as_ref().unwrap());
    assert_eq!(saved_child(ca, &name, 0), saved_child(cb, &name, 0));
}

/// Stronghold chest corridor placed far from the starting chunk: the starting chunk's
/// compound, written again before it is saved, holds `Chest` 1 (the game saves its live start).
#[test]
fn stronghold_chest_written_when_the_start_chunk_is_saved() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEED));
    let (sg, t) = (dg.structures.clone().expect("structures"), dg.structure_terrain().expect("terrain"));
    let rings = sg.sets.iter().position(|s| matches!(s.placement, Placement::Rings { .. })).expect("ring set");
    let (start, index, piece_box) = sg
        .ring_positions(rings, t)
        .iter()
        .take(3)
        .find_map(|&(x, z)| {
            let starts = dg.structure_starts(x, z);
            let st = starts.iter().find(|s| sg.defs[s.structure].id == "minecraft:stronghold")?;
            let (i, p) = st.pieces.iter().enumerate().find(|(_, p)| {
                matches!(p, Piece::Coded { data, .. } if matches!(&**data, CodedPiece::Stronghold(sp) if matches!(sp.kind, ShKind::ChestCorridor)))
            })?;
            Some(((x, z), i, p.bbox()))
        })
        .expect("chest corridor in the first strongholds");
    let corridor: Vec<(i32, i32)> = (piece_box.min[0] >> 4..=piece_box.max[0] >> 4)
        .flat_map(|x| (piece_box.min[2] >> 4..=piece_box.max[2] >> 4).map(move |z| (x, z)))
        .collect();
    // Starting chunk at the terrain stage, before the corridor is placed.
    let mut saved = DecoChunk::new(start.0, start.1, -64, 384);
    saved.structures = Some(dg.structures_nbt(start.0, start.1));
    let chest = |c: &DecoChunk| int(saved_child(c.structures.as_ref().unwrap(), "minecraft:stronghold", index), "Chest");
    assert_eq!(chest(&saved), 0, "compound of the terrain stage");
    let mut chunks = terrain(&dg, &corridor);
    for &p in &corridor {
        decorate(&dg, &mut chunks, p);
    }
    dg.refresh_placement_state(&mut saved);
    assert_eq!(chest(&saved), 1, "chest placed at {piece_box:?} not written in the starting chunk {start:?}");
}

/// Decorated chunks of the whole box of a start: entities placed.
fn structure_entities(id: &str, radius: i32) -> Option<Vec<Nbt>> {
    let w = world_data()?;
    let (dg, s, _) = find_seeded(w, id, radius);
    let mut chunks = terrain(&dg, &box_chunks(&s));
    for p in box_chunks(&s) {
        decorate(&dg, &mut chunks, p);
    }
    Some(chunks.into_values().flat_map(|c| c.entities).collect())
}

/// Elder guardians of the monument: marked for the game's finalisation.
#[test]
fn monument_elders_marked() {
    let Some(es) = structure_entities("minecraft:monument", 200) else { return };
    let elders: Vec<&Nbt> = es.iter().filter(|e| id_of(e) == "minecraft:elder_guardian").collect();
    assert_eq!(elders.len(), 3);
    assert!(elders.iter().all(|e| marked(e)), "{elders:?}");
}

/// Mansion: all its mobs (illagers and allays) marked; allay groups of 1 to 3.
#[test]
fn mansion_mobs_all_marked() {
    let Some(es) = structure_entities("minecraft:mansion", 160) else { return };
    let mut count: HashMap<&str, usize> = HashMap::new();
    for e in &es {
        *count.entry(id_of(e)).or_default() += 1;
    }
    eprintln!("mansion: {count:?}");
    let mobs: Vec<&Nbt> = es
        .iter()
        .filter(|e| matches!(id_of(e), "minecraft:evoker" | "minecraft:vindicator" | "minecraft:allay"))
        .collect();
    assert!(count.get("minecraft:allay").is_some_and(|n| *n > 0), "no allay: {count:?}");
    assert!(mobs.iter().all(|e| marked(e)), "mansion mob not marked");
}

/// Jigsaw piece: junctions and liquid setting written.
#[test]
fn jigsaw_piece_writes_junctions_and_liquids() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEED));
    let s = find_with(&dg, |id| id.starts_with("minecraft:village_"), 160).expect("village");
    let n = dg.structures_nbt(s.cx, s.cz);
    let id = dg.structures.as_ref().unwrap().defs[s.structure].id.clone();
    let Some(Nbt::List(ch)) = n.get("starts").and_then(|x| x.get(&id)).and_then(|x| x.get("Children")) else { panic!() };
    let mut junctions = 0;
    for c in ch {
        assert_eq!(c.get("liquid_settings").and_then(Nbt::as_str), Some("apply_waterlogging"));
        let Some(Nbt::List(j)) = c.get("junctions") else { panic!("junctions") };
        junctions += j.len();
        for k in j {
            for f in ["source_x", "source_ground_y", "source_z", "delta_y"] {
                int(k, f);
            }
            assert!(matches!(k.get("dest_proj").and_then(Nbt::as_str), Some("rigid" | "terrain_matching")));
        }
    }
    assert!(junctions > 0, "no junction recorded ({} pieces)", ch.len());
}

/// Exclusion zone also read for the rings: a stronghold near an excluded village disappears.
#[test]
fn rings_honour_exclusion_zone() {
    let Some(w) = world_data() else { return };
    let mut dg = dim(w, Some(SEED));
    let sg = std::sync::Arc::get_mut(dg.structures.as_mut().unwrap()).unwrap();
    let rings = sg.sets.iter().position(|s| matches!(s.placement, Placement::Rings { .. })).unwrap();
    let villages = sg.sets.iter().position(|s| s.id == "minecraft:villages").expect("village set");
    if let Placement::Rings { exclusion, .. } = &mut sg.sets[rings].placement {
        *exclusion = Some(("minecraft:villages".into(), 16));
    }
    let sg = dg.structures.clone().unwrap();
    let t = dg.structure_terrain().unwrap();
    let mut excluded = 0;
    for &(x, z) in sg.ring_positions(rings, t).iter().take(16) {
        let near = (x - 16..=x + 16).any(|a| (z - 16..=z + 16).any(|b| sg.is_structure_chunk(villages, a, b, t)));
        assert_eq!(sg.is_structure_chunk(rings, x, z, t), !near, "stronghold ({x}, {z})");
        excluded += usize::from(near);
    }
    assert!(excluded > 0, "no stronghold excluded: test without effect");
}

/// Density function bounds: values at ±1e6, increasing thresholds, search floor.
#[test]
fn density_bounds_validated() {
    let p = |s: &str| crate::density::parse(&crate::json::parse(s).unwrap());
    assert!(p("1000000").is_ok());
    assert!(p("1000000.5e1").is_err());
    assert!(p(r#"{"type":"minecraft:constant","value":-2e6}"#).is_err());
    assert!(p(r#"{"type":"minecraft:clamp","input":0,"min":-1,"max":2e6}"#).is_err());
    assert!(p(r#"{"type":"minecraft:range_choice","input":0,"min_inclusive":-3e6,"max_exclusive":1,"when_in_range":0,"when_out_of_range":1}"#).is_err());
    let sel = |t: &str| format!(r#"{{"type":"minecraft:interval_select","input":0,"thresholds":{t},"functions":[0,1,2]}}"#);
    assert!(p(&sel("[0,1]")).is_ok());
    assert!(p(&sel("[1,1]")).is_ok());
    assert!(p(&sel("[1,0]")).is_err());
    assert!(p(&sel("[0,2e6]")).is_err());
    let top = |lb: i32| format!(r#"{{"type":"minecraft:find_top_surface","density":0,"upper_bound":0,"lower_bound":{lb},"cell_height":1}}"#);
    assert!(p(&top(-4064)).is_ok() && p(&top(4062)).is_ok());
    assert!(p(&top(-4065)).is_err() && p(&top(4063)).is_err());
    let grad = |v: &str| format!(r#"{{"type":"minecraft:gradient","axis":"y","from_coordinate":0,"to_coordinate":1,"from_value":{v},"to_value":0}}"#);
    assert!(p(&grad("1")).is_ok() && p(&grad("1e7")).is_err());
}

// ---------------------------------------------------------------- Fields read back by the game

/// NBT kind of a field the game reads back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    Byte,
    Int,
    Float,
    Text,
    Compound,
}

fn tag_of(n: &Nbt) -> Option<Tag> {
    match n {
        Nbt::Byte(_) => Some(Tag::Byte),
        Nbt::Int(_) => Some(Tag::Int),
        Nbt::Float(_) => Some(Tag::Float),
        Nbt::String(_) => Some(Tag::Text),
        Nbt::Compound(_) => Some(Tag::Compound),
        _ => None,
    }
}

const TEMPLATE_BASE: [(&str, Tag); 4] =
    [("TPX", Tag::Int), ("TPY", Tag::Int), ("TPZ", Tag::Int), ("Template", Tag::Text)];

const PORTAL_PROPERTIES: [(&str, Tag); 6] = [
    ("cold", Tag::Byte),
    ("mossiness", Tag::Float),
    ("air_pocket", Tag::Byte),
    ("overgrown", Tag::Byte),
    ("vines", Tag::Byte),
    ("replace_with_blackstone", Tag::Byte),
];

/// Fields the game 26.3 reads back from a saved piece of the given type (template, stronghold
/// and Nether fortress pieces), read in its code; `None`: type outside this check.
fn official_fields(id: &str) -> Option<Vec<(&'static str, Tag)>> {
    use Tag::*;
    let own: &[(&str, Tag)] = match id {
        "minecraft:rupo" => {
            &[("Rotation", Text), ("Mirror", Text), ("VerticalPlacement", Text), ("Properties", Compound)]
        }
        "minecraft:orp" => &[("Rot", Text), ("Integrity", Float), ("BiomeType", Text), ("IsLarge", Byte)],
        "minecraft:shipwreck" => &[("isBeached", Byte), ("Rot", Text), ("height_adjusted", Byte)],
        "minecraft:shs" => &[("Left", Byte), ("Right", Byte)],
        "minecraft:shrc" => &[("Type", Int)],
        "minecraft:shsd" | "minecraft:shstart" => &[("Source", Byte)],
        "minecraft:sh5c" => &[("leftLow", Byte), ("leftHigh", Byte), ("rightLow", Byte), ("rightHigh", Byte)],
        "minecraft:shcc" => &[("Chest", Byte)],
        "minecraft:shli" => &[("Tall", Byte)],
        "minecraft:shpr" => &[("Mob", Byte)],
        "minecraft:shfc" => &[("Steps", Int)],
        "minecraft:shph" | "minecraft:shlt" | "minecraft:shrt" | "minecraft:shssd" => &[],
        "minecraft:nesclt" | "minecraft:nescrt" => &[("Chest", Byte)],
        "minecraft:nemt" => &[("Mob", Byte)],
        "minecraft:nebef" => &[("Seed", Int)],
        _ => return None,
    };
    let base: &[(&str, Tag)] = match id {
        "minecraft:rupo" | "minecraft:orp" | "minecraft:shipwreck" => &TEMPLATE_BASE,
        _ if id.starts_with("minecraft:sh") => &[("EntryDoor", Text)],
        _ => &[],
    };
    Some(base.iter().chain(own).copied().collect())
}

/// Allowed text values of the enumerated fields (names the game's reader accepts).
fn allowed_text(key: &str) -> Option<&'static [&'static str]> {
    Some(match key {
        "Rotation" | "Rot" => &["NONE", "CLOCKWISE_90", "CLOCKWISE_180", "COUNTERCLOCKWISE_90"],
        "Mirror" => &["NONE", "LEFT_RIGHT", "FRONT_BACK"],
        "VerticalPlacement" => {
            &["on_land_surface", "partly_buried", "on_ocean_floor", "in_mountain", "underground", "in_nether"]
        }
        "BiomeType" => &["WARM", "COLD"],
        "EntryDoor" => &["OPENING", "WOOD_DOOR", "GRATES", "IRON_DOOR"],
        _ => return None,
    })
}

fn check_fields(piece: &Nbt, fields: &[(&str, Tag)]) -> Result<(), String> {
    for &(key, tag) in fields {
        let value = piece.get(key).ok_or_else(|| format!("field {key} missing"))?;
        if tag_of(value) != Some(tag) {
            return Err(format!("field {key}: {value:?} is not a {tag:?}"));
        }
        if let (Some(allowed), Some(text)) = (allowed_text(key), value.as_str())
            && !allowed.contains(&text)
        {
            return Err(format!("field {key}: unknown name {text}"));
        }
    }
    Ok(())
}

/// The piece compound holds every field the game reads back for its type, with the right kind
/// (and the ruined portal's properties), and none of the names it does not read.
fn check_official_fields(piece: &Nbt) -> Result<bool, String> {
    let id = piece.get("id").and_then(Nbt::as_str).unwrap_or("?");
    let Some(fields) = official_fields(id) else { return Ok(false) };
    check_fields(piece, &fields).map_err(|e| format!("{id}: {e}"))?;
    if id == "minecraft:rupo" {
        check_fields(piece.get("Properties").expect("checked above"), &PORTAL_PROPERTIES)
            .map_err(|e| format!("{id} properties: {e}"))?;
        if piece.get("Rot").is_some() {
            return Err(format!("{id}: rotation written under the name the portal does not read"));
        }
    }
    Ok(true)
}

/// Saved compound of a hand-built coded piece.
fn coded(data: CodedPiece) -> Nbt {
    let bbox = BBox::new([16, 40, -32], [24, 50, -20]);
    super::piece_nbt(&Piece::Coded { id: "test", bbox, data: Arc::new(data) })
}

fn stronghold(kind: ShKind, door: Door, depth: i32, placed: bool) -> Nbt {
    let sp = ShPiece { kind, orient: Dir::West, door, depth, placed: Mutex::new(placed) };
    coded(CodedPiece::Stronghold(Arc::new(sp)))
}

fn byte(n: &Nbt, k: &str) -> i8 {
    match n.get(k) {
        Some(Nbt::Byte(b)) => *b,
        other => panic!("byte {k}: {other:?} in {n:?}"),
    }
}

fn text_field<'a>(n: &'a Nbt, k: &str) -> &'a str {
    n.get(k).and_then(Nbt::as_str).unwrap_or_else(|| panic!("text {k} missing: {n:?}"))
}

/// Every stronghold piece writes its entry door (without it the game rejects the piece, and the
/// whole stronghold with it) and the fields of its type, with the values of the piece.
#[test]
fn stronghold_pieces_save_entry_door_and_own_fields() {
    let start = stronghold(ShKind::StairsDown { source: true }, Door::Opening, 0, false);
    assert_eq!(text_field(&start, "id"), "minecraft:shstart");
    assert_eq!((text_field(&start, "EntryDoor"), byte(&start, "Source")), ("OPENING", 1));
    let stairs = stronghold(ShKind::StairsDown { source: false }, Door::Iron, 3, false);
    assert_eq!((text_field(&stairs, "EntryDoor"), byte(&stairs, "Source")), ("IRON_DOOR", 0));
    let straight = stronghold(ShKind::Straight { left: true, right: false }, Door::Wood, 1, false);
    let straight_fields = (text_field(&straight, "EntryDoor"), byte(&straight, "Left"), byte(&straight, "Right"));
    assert_eq!(straight_fields, ("WOOD_DOOR", 1, 0));
    let room = stronghold(ShKind::RoomCrossing(3), Door::Grates, 2, false);
    assert_eq!((text_field(&room, "EntryDoor"), int(&room, "Type")), ("GRATES", 3));
    let five = stronghold(
        ShKind::FiveCrossing { left_low: true, left_high: false, right_low: false, right_high: true },
        Door::Opening,
        4,
        false,
    );
    let five_flags = ["leftLow", "leftHigh", "rightLow", "rightHigh"].map(|k| byte(&five, k));
    assert_eq!(five_flags, [1, 0, 0, 1]);
    assert_eq!(byte(&stronghold(ShKind::Library { tall: true }, Door::Opening, 5, false), "Tall"), 1);
    assert_eq!(int(&stronghold(ShKind::Filler { steps: 7 }, Door::Opening, 5, false), "Steps"), 7);
    // Chest and spawner flags: the current state of the piece (placed or not).
    assert_eq!(byte(&stronghold(ShKind::ChestCorridor, Door::Opening, 5, true), "Chest"), 1);
    assert_eq!(byte(&stronghold(ShKind::ChestCorridor, Door::Opening, 5, false), "Chest"), 0);
    assert_eq!(byte(&stronghold(ShKind::PortalRoom, Door::Iron, 6, true), "Mob"), 1);
    for kind in [ShKind::PrisonHall, ShKind::LeftTurn, ShKind::RightTurn, ShKind::StraightStairsDown] {
        let n = stronghold(kind, Door::Grates, 2, false);
        assert_eq!(check_official_fields(&n), Ok(true), "{kind:?}");
        assert_eq!(text_field(&n, "EntryDoor"), "GRATES");
    }
    for n in [&start, &stairs, &straight, &room, &five] {
        assert_eq!(check_official_fields(n), Ok(true));
    }
}

/// Nether fortress corridor turns write whether their chest is still to place, the throne room
/// whether its spawner is placed (the game would otherwise read "no chest" and "not placed").
#[test]
fn fortress_turns_and_throne_save_their_flags() {
    let fortress = |kind: FKind, flag: bool| {
        let fp = FPiece { kind, orient: Dir::South, depth: 2, flag: Mutex::new(flag) };
        coded(CodedPiece::Fortress(Arc::new(fp)))
    };
    assert_eq!(byte(&fortress(FKind::LeftTurn, true), "Chest"), 1);
    assert_eq!(byte(&fortress(FKind::RightTurn, false), "Chest"), 0);
    assert_eq!(byte(&fortress(FKind::Throne, true), "Mob"), 1);
    assert_eq!(int(&fortress(FKind::EndFiller { seed: -5 }, false), "Seed"), -5);
    assert!(fortress(FKind::BridgeStraight, true).get("Chest").is_none(), "flag of a piece without chest");
}

fn template(location: &str, rot: Rot, mirror: Mirror, kind: TKind) -> Nbt {
    let pos = Mutex::new([16, 41, -32]);
    let tp = TemplPiece { location: location.into(), pos, mirror, rot, pivot: [0, 0, 0], procs: Arc::default(), kind };
    coded(CodedPiece::Template(Arc::new(tp)))
}

/// Ruined portal, ocean ruin and shipwreck write what the game reads back: rotation and mirror
/// under the portal's own names, its placement and properties; the ruin's integrity, biome
/// type and size; the shipwreck's beached and height-adjusted flags.
#[test]
fn template_pieces_save_what_the_official_reader_needs() {
    let portal = template(
        "minecraft:ruined_portal/portal_4",
        Rot::Cw90,
        Mirror::FrontBack,
        TKind::Portal {
            placement: VPlace::PartlyBuried,
            cold: true,
            overgrown: false,
            vines: true,
            air_pocket: true,
            mossiness: 0.2,
            blackstone: false,
        },
    );
    assert_eq!(check_official_fields(&portal), Ok(true));
    assert_eq!(
        (text_field(&portal, "Rotation"), text_field(&portal, "Mirror"), text_field(&portal, "VerticalPlacement")),
        ("CLOCKWISE_90", "FRONT_BACK", "partly_buried")
    );
    let properties = portal.get("Properties").expect("properties");
    let flags = ["cold", "air_pocket", "overgrown", "vines", "replace_with_blackstone"].map(|k| byte(properties, k));
    assert_eq!(flags, [1, 1, 0, 1, 0]);
    assert_eq!(properties.get("mossiness"), Some(&Nbt::Float(0.2)));
    assert_eq!(text_field(&portal, "Template"), "minecraft:ruined_portal/portal_4");
    for (i, placement) in [VPlace::Land, VPlace::OceanFloor, VPlace::Mountain, VPlace::Underground, VPlace::Nether]
        .into_iter()
        .enumerate()
    {
        let kind = TKind::Portal {
            placement,
            cold: false,
            overgrown: false,
            vines: false,
            air_pocket: false,
            mossiness: 0.0,
            blackstone: false,
        };
        let n = template("minecraft:ruined_portal/portal_1", Rot::from_index(i as i32), Mirror::None, kind);
        assert_eq!(check_official_fields(&n), Ok(true), "{placement:?}");
    }
    let ruin = template(
        "minecraft:underwater_ruin/big_mossy_3",
        Rot::Cw180,
        Mirror::None,
        TKind::OceanRuin { large: true, warm: false, integrity: 0.5 },
    );
    assert_eq!(check_official_fields(&ruin), Ok(true));
    let ruin_fields = (text_field(&ruin, "Rot"), text_field(&ruin, "BiomeType"), byte(&ruin, "IsLarge"));
    assert_eq!(ruin_fields, ("CLOCKWISE_180", "COLD", 1));
    assert_eq!(ruin.get("Integrity"), Some(&Nbt::Float(0.5)));
    let warm = template(
        "minecraft:underwater_ruin/warm_2",
        Rot::None,
        Mirror::None,
        TKind::OceanRuin { large: false, warm: true, integrity: 0.8 },
    );
    assert_eq!((text_field(&warm, "BiomeType"), byte(&warm, "IsLarge")), ("WARM", 0));
    let wreck = template(
        "minecraft:shipwreck/rightsideup_backhalf",
        Rot::Ccw90,
        Mirror::None,
        TKind::Shipwreck { beached: true, adjusted: Mutex::new(true) },
    );
    assert_eq!(check_official_fields(&wreck), Ok(true));
    assert_eq!((byte(&wreck, "isBeached"), byte(&wreck, "height_adjusted")), (1, 1));
    assert_eq!(text_field(&wreck, "Rot"), "COUNTERCLOCKWISE_90");
    assert_eq!([int(&wreck, "TPX"), int(&wreck, "TPY"), int(&wreck, "TPZ")], [16, 41, -32]);
}

/// The fields kept for the game's reader fit in the room of the largest template piece kind
/// (the igloo's offset): no growth per piece.
#[test]
fn template_piece_kind_keeps_its_size() {
    assert_eq!(std::mem::size_of::<TKind>(), 16);
}

/// Piece compounds of the generated starts of every structure in a square around the origin
/// that the check covers: number checked per piece type.
fn check_generated_square(dg: &DimGen, radius: i32, seen: &mut HashMap<String, usize>) {
    for cx in -radius..=radius {
        for cz in -radius..=radius {
            if dg.structure_starts(cx, cz).is_empty() {
                continue;
            }
            check_compound(&dg.structures_nbt(cx, cz), seen);
        }
    }
}

fn check_compound(structures: &Nbt, seen: &mut HashMap<String, usize>) {
    let Some(Nbt::Compound(starts)) = structures.get("starts") else { return };
    for (name, start) in starts {
        let Some(Nbt::List(children)) = start.get("Children") else { continue };
        for child in children {
            match check_official_fields(child) {
                Ok(true) => *seen.entry(id_of(child).to_owned()).or_default() += 1,
                Ok(false) => {}
                Err(e) => panic!("start {name}: {e}"),
            }
        }
    }
}

/// Generated ruined portals, ocean ruins, shipwrecks and strongholds: every piece compound
/// holds what the game reads back (the game would otherwise drop the start: piece rejected).
#[test]
fn generated_pieces_hold_the_fields_the_game_reads_back() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEED));
    let mut seen: HashMap<String, usize> = HashMap::new();
    check_generated_square(&dg, 48, &mut seen);
    // Strongholds lie on the rings, far from the origin: the first ring positions.
    let (sg, t) = (dg.structures.clone().expect("structures"), dg.structure_terrain().expect("terrain"));
    let rings = sg.sets.iter().position(|s| matches!(s.placement, Placement::Rings { .. })).expect("ring set");
    for &(x, z) in sg.ring_positions(rings, t).iter().take(3) {
        check_compound(&dg.structures_nbt(x, z), &mut seen);
    }
    eprintln!("pieces checked: {seen:?}");
    for id in ["minecraft:rupo", "minecraft:orp", "minecraft:shipwreck", "minecraft:shstart", "minecraft:shpr"] {
        assert!(seen.contains_key(id), "no {id} piece generated: pointless check ({seen:?})");
    }
}

fn field_count(n: &Nbt) -> usize {
    match n {
        Nbt::Compound(f) => f.iter().map(|(_, v)| 1 + field_count(v)).sum(),
        Nbt::List(l) => l.iter().map(field_count).sum(),
        _ => 0,
    }
}

fn encoded_bytes(n: &Nbt) -> usize {
    let mut out = Vec::new();
    rc_proto::nbt::write(&mut out, Some(n));
    out.len()
}

/// Bench (ignored): cost of the fields the game reads back, on the structure compound of the
/// chunk of the first stronghold and of a 9 × 9 chunk square around the origin (starts cached:
/// compound writing only); fields and encoded bytes now and in the old form.
/// `cargo test --release -p rc-worldgen --lib official_fields_write_cost -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn official_fields_write_cost() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEED));
    let (sg, t) = (dg.structures.clone().expect("structures"), dg.structure_terrain().expect("terrain"));
    let rings = sg.sets.iter().position(|s| matches!(s.placement, Placement::Rings { .. })).expect("ring set");
    let stronghold = sg.ring_positions(rings, t)[0];
    let square: Vec<(i32, i32)> = (-4..=4).flat_map(|x| (-4..=4).map(move |z| (x, z))).collect();
    for (name, chunks) in [("stronghold chunk", vec![stronghold]), ("9 x 9 chunks", square)] {
        let compounds: Vec<Nbt> = chunks.iter().map(|&(x, z)| dg.structures_nbt(x, z)).collect();
        let old: Vec<Nbt> = compounds.iter().map(crate::structure::tests::without_official_reader_fields).collect();
        let sum = |v: &[Nbt], f: fn(&Nbt) -> usize| v.iter().map(f).sum::<usize>();
        const N: u32 = 200;
        let t0 = std::time::Instant::now();
        for _ in 0..N {
            for &(x, z) in &chunks {
                std::hint::black_box(dg.structures_nbt(x, z));
            }
        }
        eprintln!(
            "{name}: {:.1} us per pass; fields {} (old {}), encoded bytes {} (old {})",
            t0.elapsed().as_secs_f64() * 1e6 / f64::from(N),
            sum(&compounds, field_count),
            sum(&old, field_count),
            sum(&compounds, encoded_bytes),
            sum(&old, encoded_bytes)
        );
    }
}

/// Bench (ignored): cost of reading back and rewriting the scattered pieces per chunk, compound
/// without scattered piece (common case) and hut compound.
#[test]
#[ignore = "benchmark"]
fn scattered_sync_cost() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEED));
    let s = find(&dg, "minecraft:swamp_hut", 200).expect("swamp hut");
    let (sg, t) = (dg.structures.clone().unwrap(), dg.structure_terrain().unwrap());
    let hut = dg.structures_nbt(s.cx, s.cz);
    let empty = dg.structures_nbt(s.cx + 3, s.cz + 3);
    for (name, n0, (cx, cz)) in [("empty", &empty, (s.cx + 3, s.cz + 3)), ("hut", &hut, (s.cx, s.cz))] {
        let mut n = n0.clone();
        let t0 = std::time::Instant::now();
        const N: u32 = 100_000;
        for _ in 0..N {
            crate::structure_nbt::restore_placement_state(&sg, cx, cz, t, &dg.blocks, &n);
            crate::structure_nbt::refresh_placement_state(&sg, cx, cz, t, &dg.blocks, &mut n);
        }
        eprintln!("{name}: {:.1} ns per chunk (re-read + rewrite)", t0.elapsed().as_nanos() as f64 / f64::from(N));
    }
}