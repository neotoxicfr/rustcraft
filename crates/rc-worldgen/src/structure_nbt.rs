//! Structure data of a chunk in the disk format (`structures`: `References` and `starts`)
//! : chunk keys of the starts that intersect it, and starts born in the chunk with their
//! pieces (common fields `id`, `BB`, `O`, `GD`, plus those of templates and jigsaw pieces).

use rc_proto::nbt::Nbt;

use crate::decor_blocks::{Blocks, Dir};
use crate::jigsaw::{Element, Projection};
use crate::structure::{Piece, Start, StructureGen, chunk_key};
use crate::structure_coded::CodedPiece;
use crate::structure_fortress::FKind;
use crate::structure_stronghold::ShKind;
use crate::structure_templ::TKind;
use crate::template::Mirror;
use crate::terrain::TerrainGenerator;

/// The game's 2D value of a horizontal direction (south 0, west 1, north 2, east 3).
fn dir_2d(d: Dir) -> i32 {
    match d {
        Dir::South => 0,
        Dir::West => 1,
        Dir::North => 2,
        Dir::East => 3,
        _ => -1,
    }
}

fn mirror_name(m: Mirror) -> &'static str {
    match m {
        Mirror::None => "NONE",
        Mirror::LeftRight => "LEFT_RIGHT",
        Mirror::FrontBack => "FRONT_BACK",
    }
}

fn projection(p: Projection) -> Nbt {
    Nbt::String(if p == Projection::TerrainMatching { "terrain_matching" } else { "rigid" }.into())
}

fn element_nbt(e: &Element) -> Nbt {
    match e {
        Element::Single { location, processors, projection: p, legacy, liquid, .. } => {
            let mut f = vec![
                (
                    "element_type".into(),
                    Nbt::String(
                        if *legacy { "minecraft:legacy_single_pool_element" } else { "minecraft:single_pool_element" }
                            .into(),
                    ),
                ),
                ("location".into(), Nbt::String(location.clone())),
                (
                    "processors".into(),
                    match processors {
                        crate::json::Json::Null => Nbt::String("minecraft:empty".into()),
                        j => crate::structure_place::json_to_nbt(j),
                    },
                ),
                ("projection".into(), projection(*p)),
            ];
            // Liquid setting specific to the element, written only if given (optional field).
            if let Some(w) = liquid {
                let s = if *w { "apply_waterlogging" } else { "ignore_waterlogging" };
                f.push(("override_liquid_settings".into(), Nbt::String(s.into())));
            }
            Nbt::Compound(f)
        }
        Element::List { elements, projection: p } => Nbt::Compound(vec![
            ("element_type".into(), Nbt::String("minecraft:list_pool_element".into())),
            ("elements".into(), Nbt::List(elements.iter().map(|e| element_nbt(e)).collect())),
            ("projection".into(), projection(*p)),
        ]),
        Element::Feature { feature, projection: p } => Nbt::Compound(vec![
            ("element_type".into(), Nbt::String("minecraft:feature_pool_element".into())),
            ("feature".into(), Nbt::String(feature.clone())),
            ("projection".into(), projection(*p)),
        ]),
        Element::Empty => {
            Nbt::Compound(vec![("element_type".into(), Nbt::String("minecraft:empty_pool_element".into()))])
        }
    }
}

fn fortress_id(k: FKind) -> &'static str {
    match k {
        FKind::Start => "nestart",
        FKind::BridgeCrossing => "nebcr",
        FKind::BridgeStraight => "nebs",
        FKind::RoomCrossing => "nerc",
        FKind::StairsRoom => "nesr",
        FKind::Throne => "nemt",
        FKind::Entrance => "nece",
        FKind::SmallCorridor => "nesc",
        FKind::SmallCrossing => "nescsc",
        FKind::RightTurn => "nescrt",
        FKind::LeftTurn => "nesclt",
        FKind::CorridorStairs => "neccs",
        FKind::TBalcony => "nectb",
        FKind::StalkRoom => "necsr",
        FKind::EndFiller { .. } => "nebef",
    }
}

fn stronghold_id(k: ShKind, depth: i32) -> &'static str {
    match k {
        ShKind::StairsDown { .. } if depth == 0 => "shstart",
        ShKind::Straight { .. } => "shs",
        ShKind::PrisonHall => "shph",
        ShKind::LeftTurn => "shlt",
        ShKind::RightTurn => "shrt",
        ShKind::RoomCrossing(_) => "shrc",
        ShKind::StraightStairsDown => "shssd",
        ShKind::StairsDown { .. } => "shsd",
        ShKind::FiveCrossing { .. } => "sh5c",
        ShKind::ChestCorridor => "shcc",
        ShKind::Library { .. } => "shli",
        ShKind::PortalRoom => "shpr",
        ShKind::Filler { .. } => "shfc",
    }
}

/// Compound of a piece: type identifier, box, orientation, depth and own fields.
fn piece_nbt(p: &Piece) -> Nbt {
    let mut bb = p.bbox();
    let mut f: Vec<(String, Nbt)> = Vec::new();
    let mut extra: Vec<(String, Nbt)> = Vec::new();
    let (id, o, gd): (String, i32, i32) = match p {
        Piece::Jigsaw(j) => {
            extra.push(("PosX".into(), Nbt::Int(j.pos[0])));
            extra.push(("PosY".into(), Nbt::Int(j.pos[1])));
            extra.push(("PosZ".into(), Nbt::Int(j.pos[2])));
            extra.push(("ground_level_delta".into(), Nbt::Int(j.ground_delta)));
            extra.push(("pool_element".into(), element_nbt(&j.element)));
            extra.push(("rotation".into(), Nbt::String(j.rot.name().into())));
            let junctions = j
                .junctions
                .iter()
                .map(|k| {
                    Nbt::Compound(vec![
                        ("source_x".into(), Nbt::Int(k.x)),
                        ("source_ground_y".into(), Nbt::Int(k.ground_y)),
                        ("source_z".into(), Nbt::Int(k.z)),
                        ("delta_y".into(), Nbt::Int(k.dy)),
                        ("dest_proj".into(), projection(k.projection)),
                    ])
                })
                .collect();
            extra.push(("junctions".into(), Nbt::List(junctions)));
            let liquid = if j.waterlog { "apply_waterlogging" } else { "ignore_waterlogging" };
            extra.push(("liquid_settings".into(), Nbt::String(liquid.into())));
            ("jigsaw".into(), -1, 0)
        }
        Piece::Coded { data, .. } => match &**data {
            CodedPiece::BuriedTreasure => ("btp".into(), -1, 0),
            // Mineshaft: type (0 normal, 1 badlands) and own fields, read back by the game
            // when it generates the continuation of a singleplayer world.
            CodedPiece::MsRoom { mesa, entrances } => {
                extra.push(("MST".into(), Nbt::Int(i32::from(*mesa))));
                let boxes = entrances
                    .iter()
                    .map(|b| Nbt::IntArray(vec![b.min[0], b.min[1], b.min[2], b.max[0], b.max[1], b.max[2]]))
                    .collect();
                extra.push(("Entrances".into(), Nbt::List(boxes)));
                ("msroom".into(), -1, 0)
            }
            CodedPiece::MsCorridor { mesa, dir, rails, spider, sections, spider_placed } => {
                extra.push(("MST".into(), Nbt::Int(i32::from(*mesa))));
                extra.push(("hr".into(), Nbt::Byte(i8::from(*rails))));
                extra.push(("sc".into(), Nbt::Byte(i8::from(*spider))));
                let hps = spider_placed.load(std::sync::atomic::Ordering::Relaxed);
                extra.push(("hps".into(), Nbt::Byte(i8::from(hps))));
                extra.push(("Num".into(), Nbt::Int(*sections)));
                ("mscorridor".into(), dir_2d(*dir), 0)
            }
            CodedPiece::MsCrossing { mesa, dir, two_floors } => {
                extra.push(("MST".into(), Nbt::Int(i32::from(*mesa))));
                extra.push(("tf".into(), Nbt::Byte(i8::from(*two_floors))));
                extra.push(("D".into(), Nbt::Int(dir_2d(*dir))));
                ("mscrossing".into(), -1, 0)
            }
            CodedPiece::MsStairs { mesa, dir } => {
                extra.push(("MST".into(), Nbt::Int(i32::from(*mesa))));
                ("msstairs".into(), dir_2d(*dir), 0)
            }
            CodedPiece::Stronghold(sp) => (stronghold_id(sp.kind, sp.depth).into(), dir_2d(sp.orient), sp.depth),
            CodedPiece::Fortress(fp) => {
                if let FKind::EndFiller { seed } = fp.kind {
                    extra.push(("Seed".into(), Nbt::Int(seed)));
                }
                (fortress_id(fp.kind).into(), dir_2d(fp.orient), fp.depth)
            }
            CodedPiece::Monument(m) => ("omb".into(), dir_2d(m.orient), 0),
            // Scattered piece: box moved at first placement, dimensions, kept height (−1:
            // not yet placed) and placement flags, read back at load (`restore_scattered`).
            CodedPiece::Scattered(sp) => {
                use crate::structure_scattered::SKind;
                let st = sp.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                bb = st.bbox;
                extra.push(("Width".into(), Nbt::Int(sp.width)));
                extra.push(("Height".into(), Nbt::Int(sp.height)));
                extra.push(("Depth".into(), Nbt::Int(sp.depth)));
                extra.push(("HPos".into(), Nbt::Int(st.hpos)));
                let flag = |k: &str, v: bool| (k.to_owned(), Nbt::Byte(i8::from(v)));
                let id = match &st.kind {
                    SKind::Pyramid { chests, .. } => {
                        for (i, c) in chests.iter().enumerate() {
                            extra.push(flag(&format!("hasPlacedChest{i}"), *c));
                        }
                        "tedp"
                    }
                    SKind::Jungle { trap1, trap2, main, hidden } => {
                        extra.push(flag("placedMainChest", *main));
                        extra.push(flag("placedHiddenChest", *hidden));
                        extra.push(flag("placedTrap1", *trap1));
                        extra.push(flag("placedTrap2", *trap2));
                        "tejp"
                    }
                    SKind::SwampHut { witch, cat } => {
                        extra.push(flag("Witch", *witch));
                        extra.push(flag("Cat", *cat));
                        "tesh"
                    }
                };
                (id.into(), dir_2d(sp.orient), 0)
            }
            CodedPiece::Template(tp) => {
                let pos = tp.pos.lock().map(|p| *p).unwrap_or_default();
                let (id, name) = match &tp.kind {
                    TKind::Portal { .. } => ("rupo", tp.location.clone()),
                    TKind::Shipwreck { .. } => ("shipwreck", tp.location.clone()),
                    TKind::OceanRuin { .. } => ("orp", tp.location.clone()),
                    TKind::Igloo { .. } => ("iglu", tp.location.clone()),
                    TKind::NetherFossil => ("nefos", tp.location.clone()),
                    TKind::EndCity => ("ecp", tp.location.trim_start_matches("minecraft:end_city/").to_owned()),
                    TKind::Mansion => ("wmp", tp.location.trim_start_matches("minecraft:woodland_mansion/").to_owned()),
                };
                extra.push(("TPX".into(), Nbt::Int(pos[0])));
                extra.push(("TPY".into(), Nbt::Int(pos[1])));
                extra.push(("TPZ".into(), Nbt::Int(pos[2])));
                extra.push(("Template".into(), Nbt::String(name)));
                extra.push(("Rot".into(), Nbt::String(tp.rot.name().into())));
                if matches!(tp.kind, TKind::Mansion) {
                    extra.push(("Mi".into(), Nbt::String(mirror_name(tp.mirror).into())));
                }
                if matches!(tp.kind, TKind::EndCity) {
                    let ow =
                        !tp.procs.iter().any(|p| matches!(p, crate::structure_place::Proc::Ignore(v) if v.len() > 1));
                    extra.push(("OW".into(), Nbt::Byte(i8::from(ow))));
                }
                (id.into(), -1, 0)
            }
        },
    };
    f.push(("id".into(), Nbt::String(format!("minecraft:{id}"))));
    f.push(("BB".into(), Nbt::IntArray(vec![bb.min[0], bb.min[1], bb.min[2], bb.max[0], bb.max[1], bb.max[2]])));
    f.push(("O".into(), Nbt::Int(o)));
    f.push(("GD".into(), Nbt::Int(gd)));
    f.extend(extra);
    Nbt::Compound(f)
}

fn start_nbt(sg: &StructureGen, s: &Start) -> Nbt {
    Nbt::Compound(vec![
        ("id".into(), Nbt::String(sg.defs[s.structure].id.clone())),
        ("ChunkX".into(), Nbt::Int(s.cx)),
        ("ChunkZ".into(), Nbt::Int(s.cz)),
        ("references".into(), Nbt::Int(0)),
        ("Children".into(), Nbt::List(s.pieces.iter().map(piece_nbt).collect())),
    ])
}

/// `structures` compound of the chunk (starts born here, references of the neighbouring starts).
pub fn chunk_structures(sg: &StructureGen, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks) -> Nbt {
    chunk_structures_in(sg, cx, cz, t, b, None)
}

/// Same, with a height band (bottom, top excluded) outside which the starts born here are
/// rejected: converted chunk of an old world, regenerated below y = 0.
pub fn chunk_structures_in(
    sg: &StructureGen,
    cx: i32,
    cz: i32,
    t: &TerrainGenerator,
    b: &Blocks,
    band: Option<(i32, i32)>,
) -> Nbt {
    let starts: Vec<(String, Nbt)> = sg
        .starts(cx, cz, t, b)
        .iter()
        .filter(|s| band.is_none_or(|(lo, hi)| s.bbox.min[1] >= lo && s.bbox.max[1] < hi))
        .map(|s| (sg.defs[s.structure].id.clone(), start_nbt(sg, s)))
        .collect();
    let refs: Vec<(String, Nbt)> = sg
        .references(cx, cz, t, b)
        .into_iter()
        .map(|(s, lists)| {
            let keys: Vec<i64> = lists
                .iter()
                .filter_map(|l| l.iter().find(|st| st.structure == s).map(|st| chunk_key(st.cx, st.cz)))
                .collect();
            (sg.defs[s].id.clone(), Nbt::LongArray(keys))
        })
        .collect();
    Nbt::Compound(vec![("References".into(), Nbt::Compound(refs)), ("starts".into(), Nbt::Compound(starts))])
}

/// Scattered pieces (pyramid, temple, hut) of the starts born in the chunk and recorded in
/// `structures`: `f(start, name, piece index, piece, recorded compound)`. Nothing is read
/// from the generator as long as the compound contains no scattered piece (common case).
fn each_scattered(
    sg: &StructureGen,
    cx: i32,
    cz: i32,
    t: &TerrainGenerator,
    b: &Blocks,
    structures: &Nbt,
    mut f: impl FnMut(&str, usize, &Piece, &crate::structure_scattered::ScatPiece, &Nbt),
) {
    let Some(Nbt::Compound(starts)) = structures.get("starts") else { return };
    let scattered = |n: &Nbt| matches!(n.get("id").and_then(Nbt::as_str), Some("minecraft:tedp" | "minecraft:tejp" | "minecraft:tesh"));
    let has = |s: &Nbt| matches!(s.get("Children"), Some(Nbt::List(ch)) if ch.iter().any(scattered));
    if !starts.iter().any(|(_, s)| has(s)) {
        return;
    }
    let live = sg.starts(cx, cz, t, b);
    for (name, saved) in starts {
        let Some(Nbt::List(children)) = saved.get("Children") else { continue };
        let Some(st) = live.iter().find(|s| sg.defs[s.structure].id == *name) else { continue };
        for (i, child) in children.iter().enumerate() {
            if !scattered(child) {
                continue;
            }
            if let Some(p) = st.pieces.get(i)
                && let Piece::Coded { data, .. } = p
                && let CodedPiece::Scattered(sp) = &**data
            {
                f(name, i, p, sp, child);
            }
        }
    }
}

/// Reads back the state of the scattered pieces recorded in the `structures` compound of the chunk (kept
/// height, moved box, flags) into the generator's pieces never yet placed: after
/// a restart or a clear of the start cache, the rest of a pyramid is placed at the
/// height of its first half instead of being re-measured. A compound without `HPos` (old
/// Rustcraft save) or with a negative `HPos` leaves the piece as it is.
pub fn restore_scattered(sg: &StructureGen, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks, structures: &Nbt) {
    use crate::structure_scattered::SKind;
    each_scattered(sg, cx, cz, t, b, structures, |_, _, _, sp, child| {
        let Some(hpos) = child.get("HPos").and_then(Nbt::as_i64).and_then(|h| i32::try_from(h).ok()).filter(|h| *h >= 0) else { return };
        let Some(Nbt::IntArray(bb)) = child.get("BB") else { return };
        let [x0, y0, z0, x1, y1, z1] = bb[..] else { return };
        let mut st = sp.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if st.hpos >= 0 {
            return;
        }
        st.hpos = hpos;
        st.bbox = crate::template::BBox::new([x0, y0, z0], [x1, y1, z1]);
        let flag = |k: &str| child.get(k).and_then(Nbt::as_i64).is_some_and(|v| v != 0);
        match &mut st.kind {
            SKind::Pyramid { chests, .. } => {
                for (i, c) in chests.iter_mut().enumerate() {
                    *c = flag(&format!("hasPlacedChest{i}"));
                }
            }
            SKind::Jungle { trap1, trap2, main, hidden } => {
                *main = flag("placedMainChest");
                *hidden = flag("placedHiddenChest");
                *trap1 = flag("placedTrap1");
                *trap2 = flag("placedTrap2");
            }
            SKind::SwampHut { witch, cat } => {
                *witch = flag("Witch");
                *cat = flag("Cat");
            }
        }
    });
}

/// Rewrites into the `structures` compound of the chunk the scattered pieces already placed (height, moved
/// box, flags), as the game records the current state of its pieces: the spawns of
/// the hut (witch, black cat) read this box.
pub fn refresh_scattered(sg: &StructureGen, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks, structures: &mut Nbt) {
    let mut fresh: Vec<(String, usize, Nbt)> = Vec::new();
    each_scattered(sg, cx, cz, t, b, structures, |name, i, p, sp, _| {
        if sp.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).hpos >= 0 {
            fresh.push((name.to_owned(), i, piece_nbt(p)));
        }
    });
    if fresh.is_empty() {
        return;
    }
    let Nbt::Compound(root) = structures else { return };
    let Some((_, Nbt::Compound(starts))) = root.iter_mut().find(|(k, _)| k == "starts") else { return };
    for (name, i, n) in fresh {
        if let Some((_, Nbt::Compound(s))) = starts.iter_mut().find(|(k, _)| *k == name)
            && let Some((_, Nbt::List(ch))) = s.iter_mut().find(|(k, _)| k == "Children")
            && let Some(slot) = ch.get_mut(i)
        {
            *slot = n;
        }
    }
}

/// Structures born in a chunk from its disk compound (`None`: nothing recorded).
pub fn saved_starts(structures: &Nbt) -> Option<Vec<String>> {
    match structures.get("starts") {
        Some(Nbt::Compound(f)) => Some(
            f.iter()
                .filter(|(_, v)| v.get("id").and_then(Nbt::as_str) != Some("INVALID"))
                .map(|(k, _)| k.clone())
                .collect(),
        ),
        _ => None,
    }
}

#[cfg(test)]
#[path = "structure_save_tests.rs"]
mod save_tests;
