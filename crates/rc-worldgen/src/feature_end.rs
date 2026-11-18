//! End features: obsidian pillars, chorus plants, gateways, islands, platform
//! (`generation-decoration.md` § 6, "the rest"). The crystal of each pillar is placed as an entity
//! of the chunk (F15), with its bedrock base and its fire.

use rc_proto::nbt::Nbt;

use crate::Error;
use crate::decor::Ctx;
use crate::decor_blocks::{BlockId, Blocks, Dir, State};
use crate::decor_region::add;
use crate::decor_rng::DecoRng;
use crate::decor_rules::face_prop;
use crate::json::Json;

/// Obsidian pillar of the End.
#[derive(Debug, Clone, Copy)]
pub struct Spike {
    pub x: i32,
    pub z: i32,
    pub radius: i32,
    pub height: i32,
    pub guarded: bool,
}

/// Pillars of a configuration (empty: world pillars, drawn from the seed).
pub fn parse_spikes(j: &Json) -> Result<Vec<Spike>, Error> {
    let mut out = Vec::new();
    for s in j.get("spikes").and_then(Json::as_array).into_iter().flatten() {
        let i = |k: &str| s.get(k).and_then(Json::as_i32).unwrap_or(0);
        out.push(Spike {
            x: i("centerX"),
            z: i("centerZ"),
            radius: i("radius"),
            height: i("height"),
            guarded: s.get("guarded").and_then(Json::as_bool).unwrap_or(false),
        });
    }
    Ok(out)
}

/// The ten pillars of the world: key = low 16 bits of the first long of the historical randomness of the
/// seed, sizes shuffled by this key, centres on a circle of radius 42.
pub fn world_spikes(seed: i64) -> [Spike; 10] {
    let key = DecoRng::legacy(seed).next_long() & 65535;
    let mut sizes: [i32; 10] = std::array::from_fn(|i| i as i32);
    DecoRng::legacy(key).shuffle(&mut sizes);
    std::array::from_fn(|i| {
        let a = 2.0 * (-std::f64::consts::PI + (std::f64::consts::PI / 10.0) * i as f64);
        let size = sizes[i];
        let (sa, ca) = crate::jmath::sincos(a);
        Spike {
            x: (42.0 * ca).floor() as i32,
            z: (42.0 * sa).floor() as i32,
            radius: 2 + size / 3,
            height: 76 + size * 3,
            guarded: size == 1 || size == 2,
        }
    })
}

pub fn end_spike(cx: &mut Ctx, fixed: &[Spike], rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let world;
    let spikes: &[Spike] = if fixed.is_empty() {
        world = world_spikes(cx.r.seed);
        &world
    } else {
        fixed
    };
    for s in spikes {
        if o[0] >> 4 == s.x >> 4 && o[2] >> 4 == s.z >> 4 {
            place_spike(cx, s, rng);
        }
    }
    true
}

fn place_spike(cx: &mut Ctx, s: &Spike, rng: &mut DecoRng) {
    let b = cx.r.b;
    let obsidian = b.default_state(id(b, "obsidian"));
    let air = b.default_state(b.k.air);
    let r = s.radius;
    for z in s.z - r..=s.z + r {
        for y in cx.r.min_y..=s.height + 10 {
            for x in s.x - r..=s.x + r {
                let (dx, dz) = (f64::from(x - s.x), f64::from(z - s.z));
                if dx * dx + dz * dz <= f64::from(r * r + 1) && y < s.height {
                    cx.r.set(x, y, z, obsidian, 3);
                } else if y > 65 {
                    cx.r.set(x, y, z, air, 3);
                }
            }
        }
    }
    if s.guarded {
        let bars = b.default_state(id(b, "iron_bars"));
        for dx in -2..=2i32 {
            for dz in -2..=2i32 {
                for dy in 0..=3 {
                    let (xs, zs, top) = (dx.abs() == 2, dz.abs() == 2, dy == 3);
                    if xs || zs || top {
                        let xe = dx == -2 || dx == 2 || top;
                        let ze = dz == -2 || dz == 2 || top;
                        let mut st = b.with_bool(bars, b.p.north, xe && dz != -2);
                        st = b.with_bool(st, b.p.south, xe && dz != 2);
                        st = b.with_bool(st, b.p.west, ze && dx != -2);
                        st = b.with_bool(st, b.p.east, ze && dx != 2);
                        cx.r.set(s.x + dx, s.height + dy, s.z + dz, st, 3);
                    }
                }
            }
        }
    }
    // Crystal (F15): entity placed (orientation drawn, neither invulnerable nor beam for the
    // world configuration), bedrock base and fire.
    let yaw = rng.next_float() * 360.0;
    let at = [f64::from(s.x) + 0.5, f64::from(s.height + 1), f64::from(s.z) + 0.5];
    let crystal = crate::structure_entities::with(
        Nbt::Compound(crate::structure_entities::base("minecraft:end_crystal", at, yaw)),
        vec![("ShowBottom".into(), Nbt::Byte(1)), ("Invulnerable".into(), Nbt::Byte(0))],
    );
    cx.r.add_entity(s.x, s.z, crystal);
    cx.r.set(s.x, s.height, s.z, b.default_state(id(b, "bedrock")), 3);
    cx.r.set(s.x, s.height + 1, s.z, b.default_state(id(b, "fire")), 3);
}

fn id(b: &Blocks, n: &str) -> BlockId {
    b.block_id(n).unwrap_or_else(|| panic!("block {n} missing from the data"))
}

/// 3×5×3 gateway; a known exit is noted in the block entity of the centre (like the game,
/// which sets it right after the placement: age 0, exit, exact teleport if requested).
pub fn end_gateway(cx: &mut Ctx, o: [i32; 3], exit: Option<[i32; 3]>, exact: bool) -> bool {
    end_gateway_blocks(cx, o);
    if let Some(e) = exit {
        let mut f = vec![("Age".to_owned(), Nbt::Long(0)), ("exit_portal".to_owned(), Nbt::IntArray(e.to_vec()))];
        if exact {
            f.push(("ExactTeleport".to_owned(), Nbt::Byte(1)));
        }
        cx.r.note_block_entity(o, crate::decor_region::BeData::Raw(Nbt::Compound(f)));
    }
    true
}

fn end_gateway_blocks(cx: &mut Ctx, o: [i32; 3]) {
    let b = cx.r.b;
    let gateway = b.default_state(id(b, "end_gateway"));
    let bedrock = b.default_state(id(b, "bedrock"));
    let air = b.default_state(b.k.air);
    for z in o[2] - 1..=o[2] + 1 {
        for y in o[1] - 2..=o[1] + 2 {
            for x in o[0] - 1..=o[0] + 1 {
                let (sx, sy, sz) = (x == o[0], y == o[1], z == o[2]);
                let end = (y - o[1]).abs() == 2;
                let s = if sx && sy && sz {
                    gateway
                } else if sy {
                    air
                } else if (end && sx && sz) || ((sx || sz) && !end) {
                    bedrock
                } else {
                    air
                };
                cx.r.set(x, y, z, s, 3);
            }
        }
    }
}

pub fn end_island(cx: &mut Ctx, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    let stone = b.default_state(id(b, "end_stone"));
    let mut size = rng.next_int_bounded(3) as f32 + 4.0;
    let mut y = 0;
    while size > 0.5 {
        for x in (-size).floor() as i32..=size.ceil() as i32 {
            for z in (-size).floor() as i32..=size.ceil() as i32 {
                if ((x * x + z * z) as f32) <= (size + 1.0) * (size + 1.0) {
                    cx.r.set(o[0] + x, o[1] + y, o[2] + z, stone, 3);
                }
            }
        }
        size -= rng.next_int_bounded(2) as f32 + 0.5;
        y -= 1;
    }
    true
}

pub fn end_platform(cx: &mut Ctx, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    let obsidian = id(b, "obsidian");
    for dz in -2..=2 {
        for dx in -2..=2 {
            for dy in -1..3 {
                let p = [o[0] + dx, o[1] + dy, o[2] + dz];
                let block = if dy == -1 { obsidian } else { b.k.air };
                if !b.is(cx.r.get_p(p), block) {
                    cx.r.set_p(p, b.default_state(block), 3);
                }
            }
        }
    }
    true
}

/// State of the chorus plant according to its neighbours (connections on the six faces).
fn chorus_state(cx: &Ctx, p: [i32; 3], plant: BlockId, flower: BlockId, support: u16) -> State {
    let b = cx.r.b;
    let mut s = b.default_state(plant);
    for d in [Dir::Down, Dir::Up, Dir::North, Dir::East, Dir::South, Dir::West] {
        let n = cx.r.get_p(add(p, d));
        let on = b.is(n, plant) || b.is(n, flower) || (d == Dir::Down && b.in_tag(n, support));
        s = b.with_bool(s, face_prop(b, d), on);
    }
    s
}

fn horizontal_empty(cx: &Ctx, p: [i32; 3], ignore: Option<Dir>) -> bool {
    Dir::NESW.iter().all(|&d| Some(d) == ignore || cx.r.b.is_air(cx.r.get_p(add(p, d))))
}

pub struct Chorus {
    plant: BlockId,
    flower: BlockId,
    support: u16,
}

pub fn chorus_plant(cx: &mut Ctx, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    let support = if crate::opt::decor_ids() {
        Some(b.rules.supports_chorus).filter(|&t| t != u16::MAX)
    } else {
        b.tag("minecraft:supports_chorus_plant")
    };
    let Some(support) = support else {
        cx.note_missing("chorus_plant (tag)");
        return false;
    };
    if !b.is_air(cx.r.get_p(o)) || !b.in_tag(cx.r.get_p(add(o, Dir::Down)), support) {
        return false;
    }
    let c = Chorus { plant: id(b, "chorus_plant"), flower: id(b, "chorus_flower"), support };
    let s = chorus_state(cx, o, c.plant, c.flower, c.support);
    cx.r.set_p(o, s, 2);
    grow(cx, &c, rng, o, o, 8, 0);
    true
}

fn grow(cx: &mut Ctx, c: &Chorus, rng: &mut DecoRng, cur: [i32; 3], start: [i32; 3], spread: i32, depth: i32) {
    let b = cx.r.b;
    let mut height = rng.next_int_bounded(4) + 1;
    if depth == 0 {
        height += 1;
    }
    for i in 0..height {
        let t = [cur[0], cur[1] + i + 1, cur[2]];
        if !horizontal_empty(cx, t, None) {
            return;
        }
        let s = chorus_state(cx, t, c.plant, c.flower, c.support);
        cx.r.set_p(t, s, 2);
        let below = add(t, Dir::Down);
        let s = chorus_state(cx, below, c.plant, c.flower, c.support);
        cx.r.set_p(below, s, 2);
    }
    let mut placed = false;
    if depth < 4 {
        let mut stems = rng.next_int_bounded(4);
        if depth == 0 {
            stems += 1;
        }
        for _ in 0..stems {
            let d = Dir::NESW[rng.next_int_bounded(4) as usize];
            let t = add([cur[0], cur[1] + height, cur[2]], d);
            if (t[0] - start[0]).abs() < spread
                && (t[2] - start[2]).abs() < spread
                && b.is_air(cx.r.get_p(t))
                && b.is_air(cx.r.get_p(add(t, Dir::Down)))
                && horizontal_empty(cx, t, Some(d.opposite()))
            {
                placed = true;
                let s = chorus_state(cx, t, c.plant, c.flower, c.support);
                cx.r.set_p(t, s, 2);
                let back = add(t, d.opposite());
                let s = chorus_state(cx, back, c.plant, c.flower, c.support);
                cx.r.set_p(back, s, 2);
                grow(cx, c, rng, t, start, spread, depth + 1);
            }
        }
    }
    if !placed {
        let f = b.with_int(b.default_state(c.flower), b.p.age, 5);
        cx.r.set(cur[0], cur[1] + height, cur[2], f, 2);
    }
}
