//! Terrain adaptation to the structures ("beard", `generation-decoration.md` § 9.3): additive
//! contribution to the final density of a chunk, from the pieces of the starts it references.

use std::sync::OnceLock;

use crate::jigsaw::{Junction, Projection};
use crate::structure::{Adapt, Piece, Start, StructureDef};
use crate::template::BBox;
use crate::terrain::StructureDensity;
use crate::volume::Volume;

struct Rigid {
    b: BBox,
    a: Adapt,
    delta: i32,
}

/// Contribution of a chunk.
pub struct Beard {
    rigids: Vec<Rigid>,
    junctions: Vec<Junction>,
    zone: Option<BBox>,
}

fn kernel() -> &'static [f32] {
    static K: OnceLock<Vec<f32>> = OnceLock::new();
    K.get_or_init(|| {
        let mut k = vec![0f32; 24 * 24 * 24];
        for zi in 0..24 {
            for xi in 0..24 {
                for yi in 0..24 {
                    let (dx, dy, dz) = (f64::from(xi - 12), f64::from(yi - 12) + 0.5, f64::from(zi - 12));
                    let d = dx * dx + dy * dy + dz * dz;
                    k[(zi * 576 + xi * 24 + yi) as usize] = std::f64::consts::E.powf(-d / 16.0) as f32;
                }
            }
        }
        k
    })
}

fn fast_inv_sqrt(x: f64) -> f64 {
    let half = 0.5 * x;
    let i = 0x5FE6_EB50_C7B5_37AAi64 - ((x.to_bits() as i64) >> 1);
    let y = f64::from_bits(i as u64);
    y * (1.5 - half * y * y)
}

fn bury(dx: f32, dy: f32, dz: f32) -> f32 {
    let q = dx * dx + dy * dy + dz * dz;
    if q >= 36.0 { 0.0 } else { 1.0 - q.sqrt() / 6.0 }
}

fn beard(dx: i32, dy: i32, dz: i32, to_ground: i32) -> f32 {
    let (xi, yi, zi) = (dx + 12, dy + 12, dz + 12);
    if !(0..24).contains(&xi) || !(0..24).contains(&yi) || !(0..24).contains(&zi) {
        return 0.0;
    }
    let h = to_ground as f32 + 0.5;
    let (fx, fz) = (dx as f32, dz as f32);
    let q = fx * fx + h * h + fz * fz;
    let v = -h * (fast_inv_sqrt(f64::from(q / 2.0)) as f32) / 2.0;
    v * kernel()[(zi * 576 + xi * 24 + yi) as usize]
}

impl Beard {
    /// Nearby pieces (within 12 blocks) of the adapted starts referenced by the chunk.
    pub fn new(cx: i32, cz: i32, starts: &[(usize, Vec<Start>)], defs: &[StructureDef]) -> Option<Beard> {
        let (x0, z0) = (cx * 16, cz * 16);
        let mut rigids = Vec::new();
        let mut junctions = Vec::new();
        let mut any: Option<BBox> = None;
        let include = |b: BBox, any: &mut Option<BBox>| match any {
            Some(a) => a.encapsulate(&b),
            None => *any = Some(b),
        };
        for (s, list) in starts {
            let a = defs[*s].adapt;
            if a == Adapt::None {
                continue;
            }
            for st in list {
                for p in &st.pieces {
                    let bb = p.bbox();
                    if !bb.intersects_xz(x0 - 12, z0 - 12, x0 + 15 + 12, z0 + 15 + 12) {
                        continue;
                    }
                    match p {
                        Piece::Jigsaw(jp) => {
                            if jp.element.projection() == Projection::Rigid {
                                rigids.push(Rigid { b: bb, a, delta: jp.ground_delta });
                                include(bb, &mut any);
                            }
                            for j in &jp.junctions {
                                if j.x > x0 - 12 && j.z > z0 - 12 && j.x < x0 + 15 + 12 && j.z < z0 + 15 + 12 {
                                    junctions.push(*j);
                                    let p = [j.x, j.ground_y, j.z];
                                    include(BBox::new(p, p), &mut any);
                                }
                            }
                        }
                        _ => {
                            rigids.push(Rigid { b: bb, a, delta: 0 });
                            include(bb, &mut any);
                        }
                    }
                }
            }
        }
        let zone = any?.inflated(24);
        Some(Beard { rigids, junctions, zone: Some(zone) })
    }

    fn value(&self, x: i32, y: i32, z: i32) -> f32 {
        let mut v = 0f32;
        for r in &self.rigids {
            let b = &r.b;
            let dx = 0.max((b.min[0] - x).max(x - b.max[0]));
            let dz = 0.max((b.min[2] - z).max(z - b.max[2]));
            let ground = b.min[1] + r.delta;
            let ds = y - ground;
            let dy = match r.a {
                Adapt::None => 0,
                Adapt::Bury | Adapt::BeardThin => ds,
                Adapt::BeardBox => 0.max((ground - y).max(y - b.max[1])),
                Adapt::Encapsulate => 0.max((b.min[1] - y).max(y - b.max[1])),
            };
            v += match r.a {
                Adapt::None => 0.0,
                Adapt::Bury => bury(dx as f32, dy as f32 / 2.0, dz as f32),
                Adapt::BeardThin | Adapt::BeardBox => beard(dx, dy, dz, ds) * 0.8,
                Adapt::Encapsulate => bury(dx as f32 / 2.0, dy as f32 / 2.0, dz as f32 / 2.0) * 0.8,
            };
        }
        for j in &self.junctions {
            let dy = y - j.ground_y;
            v += beard(x - j.x, dy, z - j.z, dy) * 0.4;
        }
        v
    }
}

impl StructureDensity for Beard {
    fn add_to_volume(&self, out: &mut [f32], v: &Volume) {
        let Some(zone) = self.zone else { return };
        for iz in 0..v.sz {
            let z = v.bz(iz);
            if z < zone.min[2] || z > zone.max[2] {
                continue;
            }
            for ix in 0..v.sx {
                let x = v.bx(ix);
                if x < zone.min[0] || x > zone.max[0] {
                    continue;
                }
                for iy in 0..v.sy {
                    let y = v.by(iy);
                    if y < zone.min[1] || y > zone.max[1] {
                        continue;
                    }
                    out[v.index(ix, iy, iz)] += self.value(x, y, z);
                }
            }
        }
    }
}
