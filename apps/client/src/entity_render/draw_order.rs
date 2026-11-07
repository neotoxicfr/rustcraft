//! Orders of the frame without moving big rows around during the sort: the entities from
//! farthest to nearest (`DISTANCE_SORT_KEYS`) and the instances grouped by geometry
//! (`INSTANCE_BUCKETS`). Each gives exactly the order of the stable sort it replaces.
//! Allocations per frame: the geometry buckets and the entity sort keys are kept between
//! frames; the sorted entity list is a new vector (its rows borrow the frame's entities, so it
//! cannot be kept), and the translucent sort allocates its table of keys at each call.

use glam::{Mat4, Vec3};
use hashbrown::HashMap;

use super::{Instance, PartMat, SRC_BLOCK_CENTER};

/// Instance placed in a pass: (source, first vertex, vertex count, instance).
pub(super) type Placed = (u8, u32, u32, Instance);

/// Key ordering distances from farthest to nearest exactly like `b.total_cmp(&a)`: the IEEE
/// total order read as an unsigned integer (negatives flipped, the sign bit of the others
/// set), then reversed.
pub(super) fn far_to_near_key(d: f64) -> u64 {
    let bits = d.to_bits();
    let ascending = if bits >> 63 == 1 { !bits } else { bits | 1 << 63 };
    !ascending
}

/// [`far_to_near_key`] for a single-precision distance.
fn far_to_near_key_f32(d: f32) -> u32 {
    let bits = d.to_bits();
    let ascending = if bits >> 31 == 1 { !bits } else { bits | 1 << 31 };
    !ascending
}

/// Reorders `rows` from farthest to nearest like a stable sort on `distance`: compact
/// (key, rank) pairs are sorted instead of the rows (the rank breaks ties, so an unstable sort
/// gives the stable order), then the rows are copied once into a new vector. `keys` is kept
/// between frames.
pub(super) fn sort_far_to_near<T: Copy>(rows: &mut Vec<T>, distance: impl Fn(&T) -> f64, keys: &mut Vec<(u64, u32)>) {
    keys.clear();
    keys.extend(rows.iter().enumerate().map(|(rank, r)| (far_to_near_key(distance(r)), rank as u32)));
    keys.sort_unstable();
    let sorted: Vec<T> = keys.iter().map(|&(_, rank)| rows[rank as usize]).collect();
    *rows = sorted;
}

/// Translucent instances from farthest to nearest according to their pose, distance computed
/// once per instance (the comparator of the old sort rebuilt both matrices at every comparison).
pub(super) fn sort_translucent_far_to_near(list: &mut [Placed], mats: &[PartMat], eye: Vec3) {
    list.sort_by_cached_key(|&(src, _, _, inst)| {
        let Some(p) = mats.get(inst.base as usize) else { return far_to_near_key_f32(0.0) };
        let m = Mat4::from_cols_array_2d(&p.m);
        let at = if src & SRC_BLOCK_CENTER != 0 { Vec3::splat(0.5) } else { Vec3::ZERO };
        far_to_near_key_f32(m.transform_point3(at).distance_squared(eye))
    });
}

/// Geometry order of an instance: layer rank (high bits of the source), real source, first
/// vertex, packed so that integer order is the order of the tuple.
pub(super) fn geometry_key(&(src, first, _, _): &Placed) -> u64 {
    u64::from(src >> 4) << 40 | u64::from(src & 15) << 32 | u64::from(first)
}

/// Buffers of [`group_by_geometry`], kept between frames.
#[derive(Default)]
pub(super) struct GeometryBuckets {
    /// Bucket of each distinct key.
    bucket_of: HashMap<u64, u32>,
    /// Per bucket: its key and its size, then its write position.
    buckets: Vec<(u64, u32)>,
    /// Bucket of each instance, in placement order.
    bucket_per_instance: Vec<u32>,
    /// Buckets sorted by key.
    by_key: Vec<u32>,
    grouped: Vec<Placed>,
}

/// Orders `list` by [`geometry_key`], placement order kept among equal keys (the order of
/// a stable sort), in one counting pass: a frame holds few distinct geometries for many
/// instances. A list already in order (one kind of model) is left as it is.
pub(super) fn group_by_geometry(list: &mut [Placed], b: &mut GeometryBuckets) {
    if list.is_sorted_by_key(geometry_key) {
        return;
    }
    b.bucket_of.clear();
    b.buckets.clear();
    b.bucket_per_instance.clear();
    for placed in list.iter() {
        let key = geometry_key(placed);
        let next = b.buckets.len() as u32;
        let bucket = *b.bucket_of.entry(key).or_insert(next);
        if bucket == next {
            b.buckets.push((key, 0));
        }
        b.buckets[bucket as usize].1 += 1;
        b.bucket_per_instance.push(bucket);
    }
    b.by_key.clear();
    b.by_key.extend(0..b.buckets.len() as u32);
    b.by_key.sort_unstable_by_key(|&i| b.buckets[i as usize].0);
    // Sizes become write positions, in key order.
    let mut at = 0;
    for &i in &b.by_key {
        let size = b.buckets[i as usize].1;
        b.buckets[i as usize].1 = at;
        at += size;
    }
    b.grouped.clear();
    b.grouped.extend_from_slice(list);
    for (placed, &bucket) in list.iter().zip(&b.bucket_per_instance) {
        let slot = &mut b.buckets[bucket as usize].1;
        b.grouped[*slot as usize] = *placed;
        *slot += 1;
    }
    list.copy_from_slice(&b.grouped);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Floats of every class: zeros of both signs, subnormals, infinities, NaN of both signs.
    fn awkward_distances(seed: u64, n: usize) -> Vec<f64> {
        let mut s = seed;
        let mut next = || {
            s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            s
        };
        let specials =
            [0.0, -0.0, f64::MIN_POSITIVE / 4.0, -f64::MIN_POSITIVE / 4.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, -f64::NAN];
        (0..n)
            .map(|i| match next() % 6 {
                0 => specials[i % specials.len()],
                // Repeated values: ties must keep their placement order.
                1 => (next() % 8) as f64,
                _ => f64::from_bits(next()),
            })
            .collect()
    }

    #[test]
    fn far_to_near_key_orders_like_reversed_total_cmp() {
        let d = awkward_distances(0x5eed, 4000);
        for (i, a) in d.iter().enumerate() {
            let b = d[(i * 7919 + 13) % d.len()];
            assert_eq!(far_to_near_key(*a).cmp(&far_to_near_key(b)), b.total_cmp(a), "{a:e} {b:e}");
            let (a32, b32) = (*a as f32, b as f32);
            assert_eq!(far_to_near_key_f32(a32).cmp(&far_to_near_key_f32(b32)), b32.total_cmp(&a32), "{a32:e} {b32:e}");
        }
    }

    #[test]
    fn keyed_sort_gives_the_stable_order() {
        for seed in 0..20 {
            let rows: Vec<(f64, usize)> =
                awkward_distances(seed, 300).into_iter().enumerate().map(|(i, d)| (d, i)).collect();
            let mut stable = rows.clone();
            stable.sort_by(|a, b| b.0.total_cmp(&a.0));
            let mut keyed = rows;
            sort_far_to_near(&mut keyed, |r| r.0, &mut Vec::new());
            let ranks = |v: &[(f64, usize)]| v.iter().map(|r| r.1).collect::<Vec<_>>();
            assert_eq!(ranks(&keyed), ranks(&stable), "seed {seed}");
        }
    }

    fn placed(src: u8, first: u32, tag: u32) -> Placed {
        (src, first, 36, Instance { base: tag, rect: [0.0; 4], color: 0, light: 0, overlay: 0 })
    }

    #[test]
    fn buckets_give_the_stable_sort_order() {
        let mut b = GeometryBuckets::default();
        let mut s = 0x0bad_5eedu64;
        for round in 0..50 {
            let n = 1 + round * 37 % 900;
            let list: Vec<Placed> = (0..n as u32)
                .map(|i| {
                    s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                    let src = [0u8, 1, 0x11, 0x21, 0x10][(s >> 33) as usize % 5];
                    placed(src, (s >> 40) as u32 % (1 + round as u32 % 9) * 36, i)
                })
                .collect();
            let mut stable = list.clone();
            stable.sort_by_key(|i| (i.0 >> 4, i.0 & 15, i.1));
            let mut grouped = list;
            group_by_geometry(&mut grouped, &mut b);
            let tags = |v: &[Placed]| v.iter().map(|p| (p.0, p.1, p.3.base)).collect::<Vec<_>>();
            assert_eq!(tags(&grouped), tags(&stable), "round {round}");
        }
        // Sabotage seen: a placement order lost among equal keys would no longer pass.
        let mut l = vec![placed(1, 36, 0), placed(0, 0, 1), placed(1, 36, 2)];
        group_by_geometry(&mut l, &mut b);
        assert_eq!(l.iter().map(|p| p.3.base).collect::<Vec<_>>(), [1, 0, 2]);
    }

    #[test]
    fn translucent_order_matches_the_comparator_sort() {
        let mut s = 0x7a11u64;
        let mut next = || {
            s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            ((s >> 40) as f32 / (1u64 << 24) as f32) * 20.0 - 10.0
        };
        let mats: Vec<PartMat> =
            (0..64).map(|i| PartMat::new(&Mat4::from_translation(Vec3::new(next(), next(), (i % 5) as f32)))).collect();
        let list: Vec<Placed> =
            (0..200u32).map(|i| placed(if i % 3 == 0 { 1 | SRC_BLOCK_CENTER } else { 1 }, i, i % 70)).collect();
        let eye = Vec3::new(0.5, 1.0, -3.0);
        let mut old = list.clone();
        super::super::sort_far_to_near(&mut old, &mats, eye);
        let mut new = list;
        sort_translucent_far_to_near(&mut new, &mats, eye);
        assert_eq!(new.iter().map(|p| p.1).collect::<Vec<_>>(), old.iter().map(|p| p.1).collect::<Vec<_>>());
    }
}
