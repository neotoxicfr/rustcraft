//! Structures with a template piece (`generation-decoration.md` § 8.2–8.4): ruined portals, shipwrecks,
//! ocean ruins. Start (choice of template, settings, height) and placement (template,
//! data markers, junction blocks, type-specific processing).

use std::sync::{Arc, Mutex};

use rc_proto::nbt::Nbt;

use crate::decor::Ctx;
use crate::decor_blocks::{Dir, State};
use crate::decor_pred::{IntP, Rule};
use crate::decor_region::{BeData, Hm, Region};
use crate::decor_rng::DecoRng;
use crate::json::Json;
use crate::structure::{GenStart, Piece, StructureGen};
use crate::structure_coded::CodedPiece;
use crate::structure_place::{BeMod, PRule, Placing, Proc, place_template};
use crate::template::{BBox, Mirror, Rot, Settings, Template, transform};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VPlace {
    Land,
    PartlyBuried,
    OceanFloor,
    Mountain,
    Underground,
    Nether,
}

#[derive(Debug, Clone)]
pub struct PortalSetup {
    pub placement: VPlace,
    pub air_pocket: f32,
    pub mossiness: f32,
    pub overgrown: bool,
    pub vines: bool,
    pub can_be_cold: bool,
    pub blackstone: bool,
    pub weight: f32,
}

impl PortalSetup {
    pub fn parse_all(j: &Json) -> Vec<PortalSetup> {
        j.get("setups")
            .and_then(Json::as_array)
            .unwrap_or(&[])
            .iter()
            .map(|s| {
                let f = |k: &str| s.get(k).and_then(Json::as_f32).unwrap_or(0.0);
                let bo = |k: &str| s.get(k).and_then(Json::as_bool).unwrap_or(false);
                PortalSetup {
                    placement: match s.get("placement").and_then(Json::as_str).unwrap_or("") {
                        "partly_buried" => VPlace::PartlyBuried,
                        "on_ocean_floor" => VPlace::OceanFloor,
                        "in_mountain" => VPlace::Mountain,
                        "underground" => VPlace::Underground,
                        "in_nether" => VPlace::Nether,
                        _ => VPlace::Land,
                    },
                    air_pocket: f("air_pocket_probability"),
                    mossiness: f("mossiness"),
                    overgrown: bo("overgrown"),
                    vines: bo("vines"),
                    can_be_cold: bo("can_be_cold"),
                    blackstone: bo("replace_with_blackstone"),
                    weight: f("weight"),
                }
            })
            .collect()
    }
}

/// Own data of a template piece.
#[derive(Debug)]
pub enum TKind {
    Portal {
        placement: VPlace,
        cold: bool,
        overgrown: bool,
        vines: bool,
    },
    Shipwreck {
        beached: bool,
        adjusted: Mutex<bool>,
    },
    OceanRuin {
        large: bool,
    },
    /// Igloo: top (trapdoor), offset of the part.
    Igloo {
        top: bool,
        offset: [i32; 3],
    },
    /// Nether fossil: placed whole as soon as a chunk touches it, desiccated ghast.
    NetherFossil,
    /// End city piece (chests, sentinel shulkers, elytra frame).
    EndCity,
    /// Mansion piece (oriented chests, evokers, vindicators, allays).
    Mansion,
}

/// Template piece: position modifiable at placement (the game's height adjustments).
#[derive(Debug)]
pub struct TemplPiece {
    pub location: String,
    pub pos: Mutex<[i32; 3]>,
    pub mirror: Mirror,
    pub rot: Rot,
    pub pivot: [i32; 3],
    pub procs: Arc<Vec<Proc>>,
    pub kind: TKind,
}

impl TemplPiece {
    fn settings(&self) -> Settings {
        Settings { mirror: self.mirror, rot: self.rot, pivot: self.pivot }
    }
}

fn piece(id: &'static str, t: &Template, tp: TemplPiece) -> Piece {
    let bbox = t.bbox(*tp.pos.lock().expect("position"), &tp.settings());
    Piece::Coded { id, bbox, data: Arc::new(CodedPiece::Template(Arc::new(tp))) }
}

fn ignore(g: &GenStart, air: bool) -> Proc {
    let b = g.b;
    let mut v: Vec<u16> = b.block_id("minecraft:structure_block").into_iter().collect();
    if air {
        v.extend(b.block_id("minecraft:air"));
    }
    Proc::Ignore(v)
}

fn st(g: &GenStart, n: &str) -> State {
    g.b.block_id(n).map(|i| g.b.default_state(i)).unwrap_or(0)
}

/// Occupied height (first free − 1) without decoration.
fn occupied(g: &GenStart, x: i32, z: i32, ocean: bool) -> i32 {
    g.first_free(x, z, ocean) - 1
}

/// A valid biome cell in the column at the centre of the chunk ("on the centre").
fn biome_on_center(g: &GenStart) -> bool {
    let (x, z) = (16 * g.cx + 8, 16 * g.cz + 8);
    let (y0, y1) = (g.t.world_min_y - 1, g.t.world_min_y + g.t.world_height - 1);
    let mut qy = y0 >> 2;
    while qy <= y1 >> 2 {
        if g.def.biomes.get(g.biome_at(x, qy << 2, z) as usize).copied().unwrap_or(false) {
            return true;
        }
        qy += 1;
    }
    false
}

// ---------------------------------------------------------------- Ruined portal (§ 8.3)

pub fn ruined_portal(g: &mut GenStart, setups: &[PortalSetup]) -> Option<([i32; 3], Vec<Piece>)> {
    let setup = if setups.len() > 1 {
        let total: f32 = setups.iter().map(|s| s.weight).sum();
        let mut pick = g.rng.next_float();
        let mut chosen = None;
        for s in setups {
            pick -= s.weight / total;
            if pick < 0.0 {
                chosen = Some(s);
                break;
            }
        }
        chosen?.clone()
    } else {
        setups.first()?.clone()
    };
    let air_pocket = match setup.air_pocket {
        0.0 => false,
        1.0 => true,
        p => g.rng.next_float() < p,
    };
    let loc = if g.rng.next_float() < 0.05 {
        format!("minecraft:ruined_portal/giant_portal_{}", g.rng.next_int_bounded(3) + 1)
    } else {
        format!("minecraft:ruined_portal/portal_{}", g.rng.next_int_bounded(10) + 1)
    };
    let t = g.sg.templates.get(&loc, g.b)?;
    let rot = Rot::from_index(g.rng.next_int_bounded(4));
    let mirror = if g.rng.next_float() < 0.5 { Mirror::None } else { Mirror::FrontBack };
    let pivot = [t.size[0] / 2, 0, t.size[2] / 2];
    let base = [16 * g.cx, 0, 16 * g.cz];
    let bb = t.bbox(base, &Settings { mirror, rot, pivot });
    let c = bb.center();
    let ocean = setup.placement == VPlace::OceanFloor;
    let surface = g.first_free(c[0], c[2], ocean) - 1;
    let y = portal_y(g, setup.placement, air_pocket, surface, bb.span(1), &bb);
    let origin = [base[0], y, base[2]];
    let cold = setup.can_be_cold
        && g.sg
            .deco
            .as_ref()
            .is_some_and(|d| !d.warm_enough_to_rain(g.biome_at(origin[0], origin[1], origin[2]), origin, g.t.gc.sea));
    let b = g.b;
    let mut rules = vec![PRule::simple(Rule::RandomBlock(b.block_id("gold_block")?, 0.3), st(g, "air"), BeMod::Pass)];
    let lava = b.block_id("lava")?;
    rules.push(match (setup.placement, cold) {
        (VPlace::OceanFloor, _) => PRule::simple(Rule::Block(lava), st(g, "magma_block"), BeMod::Pass),
        (_, true) => PRule::simple(Rule::Block(lava), st(g, "netherrack"), BeMod::Pass),
        _ => PRule::simple(Rule::RandomBlock(lava, 0.2), st(g, "magma_block"), BeMod::Pass),
    });
    if !cold {
        rules.push(PRule::simple(
            Rule::RandomBlock(b.block_id("netherrack")?, 0.07),
            st(g, "magma_block"),
            BeMod::Pass,
        ));
    }
    let mut procs = vec![
        ignore(g, !air_pocket),
        Proc::Rules(rules),
        Proc::Age { mossiness: setup.mossiness },
        Proc::Protected(b.tag("minecraft:features_cannot_replace")),
        Proc::LavaSubmerged,
    ];
    if setup.blackstone {
        procs.push(Proc::Blackstone);
    }
    let tp = TemplPiece {
        location: loc,
        pos: Mutex::new(origin),
        mirror,
        rot,
        pivot,
        procs: Arc::new(procs),
        kind: TKind::Portal { placement: setup.placement, cold, overgrown: setup.overgrown, vines: setup.vines },
    };
    Some((origin, vec![piece("ruined_portal", &t, tp)]))
}

fn portal_y(g: &mut GenStart, p: VPlace, air_pocket: bool, surface: i32, span: i32, bb: &BBox) -> i32 {
    let min_y = g.t.world_min_y + 15;
    let between = |r: &mut DecoRng, a: i32, b: i32| a + r.next_int_bounded(b - a + 1);
    let within = |r: &mut DecoRng, a: i32, b: i32| if a < b { between(r, a, b) } else { b };
    let y = match p {
        VPlace::Nether => {
            if air_pocket {
                between(&mut g.rng, 32, 100)
            } else if g.rng.next_float() < 0.5 {
                between(&mut g.rng, 27, 29)
            } else {
                between(&mut g.rng, 29, 100)
            }
        }
        VPlace::Mountain => within(&mut g.rng, 70, surface - span),
        VPlace::Underground => within(&mut g.rng, min_y, surface - span),
        VPlace::PartlyBuried => surface - span + between(&mut g.rng, 2, 8),
        _ => surface,
    };
    let corners = [[bb.min[0], bb.min[2]], [bb.max[0], bb.min[2]], [bb.min[0], bb.max[2]], [bb.max[0], bb.max[2]]];
    let cols: Vec<Vec<u16>> = corners.iter().map(|c| g.t.base_column(c[0], c[1])).collect();
    let ocean = p == VPlace::OceanFloor;
    let tb = &g.t.blocks;
    let mut py = y;
    while py > min_y {
        let mut n = 0;
        for col in &cols {
            let id = col.get((py - g.t.world_min_y) as usize).copied().unwrap_or(0);
            if if ocean { tb.blocks_motion(id) } else { !tb.is_air(id) } {
                n += 1;
                if n == 3 {
                    return py;
                }
            }
        }
        py -= 1;
    }
    min_y
}

// ---------------------------------------------------------------- Nether fossil (§ 8.4)

pub fn nether_fossil(g: &mut GenStart, height: &crate::decor_pred::HeightP) -> Option<([i32; 3], Vec<Piece>)> {
    let x = 16 * g.cx + g.rng.next_int_bounded(16);
    let z = 16 * g.cz + g.rng.next_int_bounded(16);
    let sea = g.t.gc.sea;
    let mut y = height.sample(&mut g.rng, g.hctx);
    let col = g.t.base_column(x, z);
    let tb = &g.t.blocks;
    let b = g.b;
    let at = |y: i32| -> State {
        let id = col.get((y - g.t.world_min_y) as usize).copied().unwrap_or(0);
        b.state_from_str(tb.name(id)).unwrap_or(0)
    };
    while y > sea {
        let cur = at(y);
        y -= 1;
        let below = at(y);
        if b.is_air(cur) && (b.block_id("soul_sand") == Some(b.block(below)) || b.sturdy(below, Dir::Up)) {
            break;
        }
    }
    if y <= sea {
        return None;
    }
    let rot = Rot::from_index(g.rng.next_int_bounded(4));
    let loc = format!("minecraft:nether_fossils/fossil_{}", g.rng.next_int_bounded(14) + 1);
    let t = g.sg.templates.get(&loc, g.b)?;
    let pos = [x, y, z];
    let tp = TemplPiece {
        location: loc,
        pos: Mutex::new(pos),
        mirror: Mirror::None,
        rot,
        pivot: [0, 0, 0],
        procs: Arc::new(vec![ignore(g, true)]),
        kind: TKind::NetherFossil,
    };
    Some((pos, vec![piece("nether_fossil", &t, tp)]))
}

/// Desiccated ghast of a fossil (position randomness drawn from the world seed, at the centre).
fn dried_ghast(r: &mut Region, tr: &crate::template::StateTransforms, seed: i64, bb: BBox, zone: BBox) {
    let c = bb.center();
    let factory = DecoRng::legacy(seed).next_long();
    let mut pr = DecoRng::legacy(crate::rng::position_seed(c[0], c[1], c[2]) ^ factory);
    if pr.next_float() >= 0.5 {
        return;
    }
    let x = bb.min[0] + pr.next_int_bounded(bb.span(0));
    let y = bb.min[1];
    let z = bb.min[2] + pr.next_int_bounded(bb.span(2));
    let b = r.b;
    if b.is_air(r.get(x, y, z)) && zone.contains([x, y, z]) {
        let Some(id) = b.block_id("dried_ghast") else { return };
        let s = tr.rotate(b.default_state(id), Rot::from_index(pr.next_int_bounded(4)));
        r.set(x, y, z, s, 2);
    }
}

// ---------------------------------------------------------------- Shipwreck (§ 8.4)

const BEACHED: [&str; 11] = [
    "with_mast",
    "sideways_full",
    "sideways_fronthalf",
    "sideways_backhalf",
    "rightsideup_full",
    "rightsideup_fronthalf",
    "rightsideup_backhalf",
    "with_mast_degraded",
    "rightsideup_full_degraded",
    "rightsideup_fronthalf_degraded",
    "rightsideup_backhalf_degraded",
];

const OCEAN: [&str; 20] = [
    "with_mast",
    "upsidedown_full",
    "upsidedown_fronthalf",
    "upsidedown_backhalf",
    "sideways_full",
    "sideways_fronthalf",
    "sideways_backhalf",
    "rightsideup_full",
    "rightsideup_fronthalf",
    "rightsideup_backhalf",
    "with_mast_degraded",
    "upsidedown_full_degraded",
    "upsidedown_fronthalf_degraded",
    "upsidedown_backhalf_degraded",
    "sideways_full_degraded",
    "sideways_fronthalf_degraded",
    "sideways_backhalf_degraded",
    "rightsideup_full_degraded",
    "rightsideup_fronthalf_degraded",
    "rightsideup_backhalf_degraded",
];

fn corner_heights(g: &GenStart, x: i32, sx: i32, z: i32, sz: i32) -> [i32; 4] {
    [
        occupied(g, x, z, false),
        occupied(g, x, z + sz, false),
        occupied(g, x + sx, z, false),
        occupied(g, x + sx, z + sz, false),
    ]
}

pub fn shipwreck(g: &mut GenStart, beached: bool) -> Option<([i32; 3], Vec<Piece>)> {
    if !biome_on_center(g) {
        return None;
    }
    let (cx, cz) = (16 * g.cx + 8, 16 * g.cz + 8);
    let start = [cx, occupied(g, cx, cz, !beached), cz];
    let rot = Rot::from_index(g.rng.next_int_bounded(4));
    let list: &[&str] = if beached { &BEACHED } else { &OCEAN };
    let loc = format!("minecraft:shipwreck/{}", list[g.rng.next_int_bounded(list.len() as i32) as usize]);
    let t = g.sg.templates.get(&loc, g.b)?;
    let mut pos = [16 * g.cx, 90, 16 * g.cz];
    let settings = Settings { mirror: Mirror::None, rot, pivot: [4, 0, 15] };
    let mut adjusted = false;
    if t.size[0] > 32 || t.size[1] > 32 {
        let bb = t.bbox(pos, &settings);
        let h = corner_heights(g, bb.min[0], bb.span(0), bb.min[2], bb.span(2));
        let y = if beached {
            let min = h.iter().copied().min().unwrap_or(0);
            min - t.size[1] / 2 - g.rng.next_int_bounded(3)
        } else {
            (h[0] + h[1] + h[2] + h[3]) / 4
        };
        pos[1] = y;
        adjusted = true;
    }
    let tp = TemplPiece {
        location: loc,
        pos: Mutex::new(pos),
        mirror: Mirror::None,
        rot,
        pivot: [4, 0, 15],
        procs: Arc::new(vec![ignore(g, true)]),
        kind: TKind::Shipwreck { beached, adjusted: Mutex::new(adjusted) },
    };
    Some((start, vec![piece("shipwreck", &t, tp)]))
}

// ---------------------------------------------------------------- Ocean ruins (§ 8.4)

fn ruin_piece(
    g: &mut GenStart,
    pos: [i32; 3],
    rot: Rot,
    warm: bool,
    large: bool,
    integrity: f32,
    loc: String,
) -> Option<Piece> {
    let t = g.sg.templates.get(&loc, g.b)?;
    let b = g.b;
    let (from, to, table) = if warm {
        ("sand", "suspicious_sand", "minecraft:archaeology/ocean_ruin_warm")
    } else {
        ("gravel", "suspicious_gravel", "minecraft:archaeology/ocean_ruin_cold")
    };
    let rule = PRule::simple(Rule::Block(b.block_id(from)?), st(g, to), BeMod::Loot(table.into()));
    let procs = vec![
        Proc::Rot { integrity, rottable: None },
        ignore(g, true),
        Proc::Capped { delegate: Box::new(Proc::Rules(vec![rule])), limit: IntP::Const(5) },
    ];
    let tp = TemplPiece {
        location: loc,
        pos: Mutex::new(pos),
        mirror: Mirror::None,
        rot,
        pivot: [0, 0, 0],
        procs: Arc::new(procs),
        kind: TKind::OceanRuin { large },
    };
    Some(piece("ocean_ruin", &t, tp))
}

fn add_ruin(g: &mut GenStart, out: &mut Vec<Piece>, pos: [i32; 3], rot: Rot, warm: bool, large: bool, integrity: f32) {
    if warm {
        let loc = if large {
            format!("minecraft:underwater_ruin/big_warm_{}", 4 + g.rng.next_int_bounded(4))
        } else {
            format!("minecraft:underwater_ruin/warm_{}", 1 + g.rng.next_int_bounded(8))
        };
        out.extend(ruin_piece(g, pos, rot, warm, large, integrity, loc));
    } else {
        let names: [&str; 4] = if large { ["1", "2", "3", "8"] } else { ["1", "2", "3", "4"] };
        let i = g.rng.next_int_bounded(if large { 4 } else { 8 }) as usize;
        let n = if large { names[i].to_owned() } else { (i + 1).to_string() };
        let pre = if large { "big_" } else { "" };
        for (kind, integ) in [("brick", integrity), ("cracked", 0.7), ("mossy", 0.5)] {
            let loc = format!("minecraft:underwater_ruin/{pre}{kind}_{n}");
            out.extend(ruin_piece(g, pos, rot, warm, large, integ, loc));
        }
    }
}

pub fn ocean_ruin(g: &mut GenStart, warm: bool, large_p: f32, cluster_p: f32) -> Option<([i32; 3], Vec<Piece>)> {
    if !biome_on_center(g) {
        return None;
    }
    let (cx, cz) = (16 * g.cx + 8, 16 * g.cz + 8);
    let start = [cx, occupied(g, cx, cz, true), cz];
    let p = [16 * g.cx, 90, 16 * g.cz];
    let rot = Rot::from_index(g.rng.next_int_bounded(4));
    let large = g.rng.next_float() <= large_p;
    let integrity = if large { 0.9 } else { 0.8 };
    let mut out = Vec::new();
    add_ruin(g, &mut out, p, rot, warm, large, integrity);
    if large && g.rng.next_float() <= cluster_p {
        let corner = transform([15, 0, 15], Mirror::None, rot, [0, 0, 0]);
        let corner = [corner[0] + p[0], corner[1] + 90, corner[2] + p[2]];
        let parent = BBox::new(p, corner);
        let bl = [p[0].min(corner[0]), 90, p[2].min(corner[2])];
        let r = &mut g.rng;
        let mut nb = |a: i32, b: i32| a + r.next_int_bounded(b - a + 1);
        let mut pos: Vec<[i32; 3]> = Vec::new();
        for (ax, bx, az, bz, ox, oz) in [
            (1, 8, 1, 7, -16, 16),
            (1, 8, 1, 7, -16, 0),
            (1, 8, 4, 8, -16, -16),
            (1, 7, 1, 7, 0, 16),
            (1, 7, 4, 6, 0, -16),
            (1, 7, 3, 8, 16, 16),
            (1, 7, 1, 7, 16, 0),
            (1, 7, 4, 8, 16, -16),
        ] {
            let x = ox + nb(ax, bx);
            let z = oz + nb(az, bz);
            pos.push([bl[0] + x, 90, bl[2] + z]);
        }
        let n = 4 + g.rng.next_int_bounded(5);
        for _ in 0..n {
            if pos.is_empty() {
                break;
            }
            let q = pos.remove(g.rng.next_int_bounded(pos.len() as i32) as usize);
            let r2 = Rot::from_index(g.rng.next_int_bounded(4));
            let c2 = transform([5, 0, 6], Mirror::None, r2, [0, 0, 0]);
            let bb = BBox::new(q, [q[0] + c2[0], q[1] + c2[1], q[2] + c2[2]]);
            if !bb.intersects(&parent) {
                add_ruin(g, &mut out, q, r2, warm, false, 0.8);
            }
        }
    }
    Some((start, out))
}

// ---------------------------------------------------------------- Placement

/// Placement of a template piece in the chunk (§ 8.2).
pub fn place(cx: &mut Ctx, sg: &StructureGen, tp: &TemplPiece, rng: &mut DecoRng, zone: BBox, reference: [i32; 3]) {
    let b = cx.r.b;
    let Some(t) = sg.templates.get(&tp.location, b) else { return };
    let mut zone = zone;
    match &tp.kind {
        TKind::Portal { .. } => {
            let bb = t.bbox(*tp.pos.lock().expect("position"), &tp.settings());
            if !zone.contains(bb.center()) {
                return;
            }
            zone.encapsulate(&bb);
        }
        TKind::Shipwreck { beached, adjusted } => {
            let mut adj = adjusted.lock().expect("shipwreck");
            if !*adj && !(t.size[0] > 32 || t.size[1] > 32) {
                let hm = if *beached { Hm::WorldSurfaceWg } else { Hm::OceanFloorWg };
                let p0 = *tp.pos.lock().expect("position");
                let base = t.size[0] * t.size[2];
                let (mut mean, mut min) = (0, cx.r.max_y() + 1);
                if base == 0 {
                    mean = cx.r.height_at(hm, p0[0], p0[2]);
                } else {
                    for z in p0[2]..p0[2] + t.size[2] {
                        for x in p0[0]..p0[0] + t.size[0] {
                            let h = cx.r.height_at(hm, x, z);
                            mean += h;
                            min = min.min(h);
                        }
                    }
                    mean /= base;
                }
                let y = if *beached { min - t.size[1] / 2 - rng.next_int_bounded(3) } else { mean };
                tp.pos.lock().expect("position")[1] = y;
                *adj = true;
            }
        }
        TKind::OceanRuin { .. } => {
            let mut p = *tp.pos.lock().expect("position");
            p[1] = cx.r.height_at(Hm::OceanFloorWg, p[0], p[2]);
            let c = transform([t.size[0] - 1, 0, t.size[2] - 1], Mirror::None, tp.rot, [0, 0, 0]);
            let corner = [c[0] + p[0], p[1], c[2] + p[2]];
            p[1] = ruin_height(cx.r, p, corner);
            *tp.pos.lock().expect("position") = p;
        }
        TKind::Igloo { .. } | TKind::EndCity | TKind::Mansion => {}
        TKind::NetherFossil => {
            // The game widens the cut-out to the whole fossil (placed whole, neighbours included).
            zone.encapsulate(&t.bbox(*tp.pos.lock().expect("position"), &tp.settings()));
        }
    }
    let mut pos = *tp.pos.lock().expect("position");
    if let TKind::Igloo { offset, .. } = &tp.kind {
        let e = transform([3 - offset[0], 0, -offset[2]], tp.mirror, tp.rot, tp.pivot);
        let h = cx.r.height_at(Hm::WorldSurfaceWg, pos[0] + e[0], pos[2] + e[2]);
        pos[1] += h - 90 - 1;
    }
    let s = Placing {
        mirror: tp.mirror,
        rot: tp.rot,
        pivot: tp.pivot,
        bbox: Some(zone),
        procs: crate::structure_place::ProcChain::of(tp.procs.clone()),
        waterlog: !matches!(tp.kind, TKind::Igloo { .. }),
        seed: None,
        shared: false,
        known_shape: false,
        entities: if matches!(tp.kind, TKind::EndCity | TKind::Mansion) { None } else { Some(None) },
    };
    let placed = place_template(cx.r, b, &sg.templates.transforms, sg.seed, &t, pos, reference, &s, rng, 2);
    if let TKind::NetherFossil = &tp.kind {
        dried_ghast(cx.r, &sg.templates.transforms, sg.seed, t.bbox(pos, &tp.settings()), zone);
    }
    if !placed {
        return;
    }
    // Data markers then remaining junction blocks (palette of the position).
    let pi = crate::decor_rng::DecoRng::legacy(crate::rng::position_seed(pos[0], pos[1], pos[2]))
        .next_int_bounded(t.palettes.len() as i32) as usize;
    let sb = b.block_id("minecraft:structure_block");
    let jig = b.block_id("minecraft:jigsaw");
    let mut markers: Vec<(String, [i32; 3])> = Vec::new();
    let mut jigsaws: Vec<([i32; 3], String)> = Vec::new();
    for tb in &t.palettes[pi] {
        let w = transform(tb.pos, tp.mirror, tp.rot, tp.pivot);
        let w = [w[0] + pos[0], w[1] + pos[1], w[2] + pos[2]];
        if !zone.contains(w) {
            continue;
        }
        let blk = Some(b.block(tb.state));
        if blk == sb
            && let Some(n) = &tb.nbt
            && n.get("mode").and_then(Nbt::as_str) == Some("DATA")
        {
            markers.push((n.get("metadata").and_then(Nbt::as_str).unwrap_or("").to_owned(), w));
        } else if blk == jig
            && let Some(n) = &tb.nbt
        {
            jigsaws.push((w, n.get("final_state").and_then(Nbt::as_str).unwrap_or("minecraft:air").to_owned()));
        }
    }
    for (m, w) in markers {
        data_marker(cx.r, tp, &m, w, rng, zone);
    }
    for (w, fs) in jigsaws {
        let s = b.state_from_str(&fs).or_else(|| crate::data::with_full_id(&fs, |f| b.state_from_str(f))).unwrap_or(0);
        cx.r.set_p(w, s, 3);
    }
    if let TKind::Igloo { top: true, .. } = &tp.kind {
        let t5 = transform([3, 0, 5], tp.mirror, tp.rot, tp.pivot);
        let door = [pos[0] + t5[0], pos[1] + t5[1], pos[2] + t5[2]];
        let below = cx.r.get(door[0], door[1] - 1, door[2]);
        if !b.is_air(below)
            && b.block_id("ladder") != Some(b.block(below))
            && let Some(sn) = b.block_id("snow_block")
        {
            cx.r.set_p(door, b.default_state(sn), 3);
        }
    }
    if let TKind::Portal { placement, cold, overgrown, vines } = &tp.kind {
        let bb = t.bbox(pos, &tp.settings());
        portal_after(cx.r, bb, *placement, *cold, *overgrown, *vines, rng);
    }
}

fn data_marker(r: &mut Region, tp: &TemplPiece, m: &str, w: [i32; 3], rng: &mut DecoRng, zone: BBox) {
    let b = r.b;
    match &tp.kind {
        TKind::Shipwreck { .. } => {
            let table = match m {
                "map_chest" => "minecraft:chests/shipwreck_map",
                "treasure_chest" => "minecraft:chests/shipwreck_treasure",
                "supply_chest" => "minecraft:chests/shipwreck_supply",
                _ => return,
            };
            let below = [w[0], w[1] - 1, w[2]];
            if b.facts(r.get_p(below)).block_entity {
                let seed = rng.next_long();
                r.note_block_entity(below, BeData::Loot { table, seed });
            }
        }
        TKind::OceanRuin { large } => {
            if m == "chest" {
                let water = b.fluid(r.get_p(w)) == 1;
                let mut s = b.default_state(b.k.chest);
                if let Some(p) = b.prop("waterlogged") {
                    s = b.with_bool(s, p, water);
                }
                r.set_p(w, s, 2);
                let table = if *large {
                    "minecraft:chests/underwater_ruin_big"
                } else {
                    "minecraft:chests/underwater_ruin_small"
                };
                let seed = rng.next_long();
                r.note_block_entity(w, BeData::Loot { table, seed });
            } else if m == "drowned" {
                // Persistent drowned at the bottom centre of the cell, which becomes air or water; the game
                // finalises it (equipment, baby, bonus): done by the server at first load.
                let at = [f64::from(w[0]) + 0.5, f64::from(w[1]), f64::from(w[2]) + 0.5];
                let e = crate::structure_entities::with(
                    crate::structure_entities::mob("minecraft:drowned", at, 0.0, true),
                    vec![("rc_finalize_structure".into(), rc_proto::nbt::Nbt::Byte(1))],
                );
                r.add_entity(w[0], w[2], e);
                let s = if w[1] > r.sea_level { b.default_state(b.k.air) } else { b.default_state(b.k.water) };
                r.set_p(w, s, 2);
            }
        }
        TKind::Igloo { .. } => {
            if m == "chest" {
                r.set_p(w, b.default_state(b.k.air), 3);
                let below = [w[0], w[1] - 1, w[2]];
                if b.is(r.get_p(below), b.k.chest) {
                    let seed = rng.next_long();
                    r.note_block_entity(below, BeData::Loot { table: "minecraft:chests/igloo_chest", seed });
                }
            }
        }
        TKind::Portal { .. } | TKind::NetherFossil => {}
        TKind::Mansion => {
            if m.starts_with("Chest") {
                let mut s = b.default_state(b.k.chest);
                let f = match m {
                    "ChestWest" => Some(Dir::West),
                    "ChestEast" => Some(Dir::East),
                    "ChestSouth" => Some(Dir::South),
                    "ChestNorth" => Some(Dir::North),
                    _ => None,
                };
                if let (Some(f), Some(p)) = (f, b.prop("facing")) {
                    s = b.with_str(s, p, tp.rot.dir(f).name());
                }
                if zone.contains(w) && !b.is(r.get_p(w), b.k.chest) {
                    r.set_p(w, s, 2);
                    let seed = rng.next_long();
                    r.note_block_entity(w, BeData::Loot { table: "minecraft:chests/woodland_mansion", seed });
                }
            } else {
                let at = [f64::from(w[0]) + 0.5, f64::from(w[1]), f64::from(w[2]) + 0.5];
                let (id, n) = match m {
                    "Mage" => ("minecraft:evoker", 1),
                    "Warrior" => ("minecraft:vindicator", 1),
                    // 1 to 3 allays drawn from the region's randomness (not the piece's), at that moment.
                    "Group of Allays" => ("minecraft:allay", r.region_rng.next_int_bounded(3) + 1),
                    _ => return,
                };
                for _ in 0..n {
                    // Every mob of the mansion is finalised by the game (equipment, hand, bonus):
                    // done by the server at its first load.
                    let e = crate::structure_entities::with(
                        crate::structure_entities::mob(id, at, 0.0, true),
                        vec![("rc_finalize_structure".into(), rc_proto::nbt::Nbt::Byte(1))],
                    );
                    r.add_entity(w[0], w[2], e);
                }
                r.set_p(w, b.default_state(b.k.air), 2);
            }
        }
        TKind::EndCity => {
            if m.starts_with("Chest") {
                let below = [w[0], w[1] - 1, w[2]];
                if zone.contains(below) {
                    let seed = rng.next_long();
                    r.note_block_entity(below, BeData::Loot { table: "minecraft:chests/end_city_treasure", seed });
                }
            } else if zone.contains(w)
                && (-30_000_000..30_000_000).contains(&w[0])
                && (-30_000_000..30_000_000).contains(&w[2])
            {
                use crate::structure_entities as se;
                let at = [f64::from(w[0]) + 0.5, f64::from(w[1]), f64::from(w[2]) + 0.5];
                if m.starts_with("Sentry") {
                    r.add_entity(w[0], w[2], se::mob("minecraft:shulker", at, 0.0, false));
                } else if m.starts_with("Elytra") {
                    let f = tp.rot.dir(Dir::South);
                    let e = se::base("minecraft:item_frame", at, 0.0);
                    let item = Nbt::Compound(vec![
                        ("id".into(), Nbt::String("minecraft:elytra".into())),
                        ("count".into(), Nbt::Int(1)),
                    ]);
                    let e = se::with(
                        Nbt::Compound(e),
                        vec![
                            ("TileX".into(), Nbt::Int(w[0])),
                            ("TileY".into(), Nbt::Int(w[1])),
                            ("TileZ".into(), Nbt::Int(w[2])),
                            ("Facing".into(), Nbt::Byte(f as i8)),
                            ("Item".into(), item),
                        ],
                    );
                    r.add_entity(w[0], w[2], e);
                }
            }
        }
    }
}

fn portal_replaceable(b: &crate::decor_blocks::Blocks, s: State, nether: bool) -> bool {
    !b.is(s, b.k.air)
        && b.block_id("obsidian") != Some(b.block(s))
        && !b.tag("minecraft:features_cannot_replace").is_some_and(|t| b.in_tag(s, t))
        && (nether || !b.is(s, b.k.lava))
}

fn netherrack_or_magma(r: &mut Region, p: [i32; 3], cold: bool, nether: bool, rng: &mut DecoRng) {
    let b = r.b;
    if !portal_replaceable(b, r.get_p(p), nether) {
        return;
    }
    let magma = !cold && rng.next_float() < 0.07;
    let s = if magma {
        b.default_state(b.k.magma_block)
    } else {
        b.block_id("netherrack").map(|i| b.default_state(i)).unwrap_or(0)
    };
    r.set_p(p, s, 3);
}

fn drip_column(r: &mut Region, p: [i32; 3], cold: bool, nether: bool, rng: &mut DecoRng) {
    let mut q = p;
    netherrack_or_magma(r, q, cold, nether, rng);
    let mut cap = 8;
    while cap > 0 && rng.next_float() < 0.5 {
        q[1] -= 1;
        cap -= 1;
        netherrack_or_magma(r, q, cold, nether, rng);
    }
}

fn leaves_above(r: &mut Region, p: [i32; 3], rng: &mut DecoRng) {
    let b = r.b;
    let net = b.block_id("netherrack");
    if rng.next_float() < 0.5
        && Some(b.block(r.get_p(p))) == net
        && b.is_air(r.get(p[0], p[1] + 1, p[2]))
        && let Some(l) = b.block_id("jungle_leaves")
    {
        let mut s = b.default_state(l);
        if let Some(pp) = b.prop("persistent") {
            s = b.with_bool(s, pp, true);
        }
        r.set(p[0], p[1] + 1, p[2], s, 3);
    }
}

fn portal_after(
    r: &mut Region,
    bb: BBox,
    placement: VPlace,
    cold: bool,
    overgrown: bool,
    vines: bool,
    rng: &mut DecoRng,
) {
    let b = r.b;
    let nether = placement == VPlace::Nether;
    // Netherrack spread.
    let follow = matches!(placement, VPlace::Land | VPlace::OceanFloor);
    let c = bb.center();
    const P: [f32; 14] = [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.9, 0.9, 0.8, 0.7, 0.6, 0.4, 0.2];
    let avg = (bb.span(0) + bb.span(2)) / 2;
    let adj = rng.next_int_bounded(1.max(8 - avg / 2));
    let hm = if placement == VPlace::OceanFloor { Hm::OceanFloorWg } else { Hm::WorldSurfaceWg };
    for x in c[0] - 14..=c[0] + 14 {
        for z in c[2] - 14..=c[2] + 14 {
            let d = (0).max((x - c[0]).abs() + (z - c[2]).abs() + adj);
            if d >= 14 {
                continue;
            }
            if rng.next_double() < f64::from(P[d as usize]) {
                let s = r.height_at(hm, x, z) - 1;
                let y = if follow { s } else { bb.min[1].min(s) };
                let p = [x, y, z];
                if (y - bb.min[1]).abs() <= 3 && portal_replaceable(b, r.get_p(p), nether) {
                    netherrack_or_magma(r, p, cold, nether, rng);
                    if overgrown {
                        leaves_above(r, p, rng);
                    }
                    drip_column(r, [x, y - 1, z], cold, nether, rng);
                }
            }
        }
    }
    // Columns under the portal.
    let net = b.block_id("netherrack");
    for x in bb.min[0] + 1..bb.max[0] {
        for z in bb.min[2] + 1..bb.max[2] {
            if Some(b.block(r.get(x, bb.min[1], z))) == net {
                drip_column(r, [x, bb.min[1] - 1, z], cold, nether, rng);
            }
        }
    }
    if vines || overgrown {
        let vine = b.block_id("vine");
        for z in bb.min[2]..=bb.max[2] {
            for y in bb.min[1]..=bb.max[1] {
                for x in bb.min[0]..=bb.max[0] {
                    let p = [x, y, z];
                    if vines {
                        let s = r.get_p(p);
                        if !b.is_air(s) && Some(b.block(s)) != vine {
                            let d = [Dir::North, Dir::East, Dir::South, Dir::West][rng.next_int_bounded(4) as usize];
                            let o = d.offset();
                            let q = [x + o[0], y + o[1], z + o[2]];
                            if b.is_air(r.get_p(q))
                                && b.facts(s).coll_faces & (1 << d as u8) != 0
                                && let Some(v) = vine
                            {
                                let mut vs = b.default_state(v);
                                if let Some(pr) = b.prop(d.opposite().name()) {
                                    vs = b.with_bool(vs, pr, true);
                                }
                                r.set_p(q, vs, 3);
                            }
                        }
                    }
                    if overgrown {
                        leaves_above(r, p, rng);
                    }
                }
            }
        }
    }
}

fn ruin_height(r: &Region, pos: [i32; 3], corner: [i32; 3]) -> i32 {
    let b = r.b;
    let mut new_y = pos[1];
    let mut min_y = 512;
    let top = new_y - 1;
    let mut area = 0;
    let ice = b.tag("minecraft:ice");
    for z in pos[2].min(corner[2])..=pos[2].max(corner[2]) {
        for x in pos[0].min(corner[0])..=pos[0].max(corner[0]) {
            let mut fy = pos[1] - 1;
            loop {
                let s = r.get(x, fy, z);
                let empty = b.is_air(s) || b.fluid(s) == 1 || ice.is_some_and(|t| b.in_tag(s, t));
                if !(empty && fy > r.min_y + 1) {
                    break;
                }
                fy -= 1;
            }
            min_y = min_y.min(fy);
            if fy < top - 2 {
                area += 1;
            }
        }
    }
    let width = (pos[0] - corner[0]).abs();
    if top - min_y > 2 && area > width - 2 {
        new_y = min_y + 1;
    }
    new_y
}
