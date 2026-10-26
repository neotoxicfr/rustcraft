// Direct read allowed (tests and benches: settings read at launch, off the measured path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! In-place writing of the partial remeshes (`in_place_diff`) and exact-size flattening
//! (`pack_exact`): a simulated buffer (GPU bytes, other columns around) that receives only the announced
//! range must become identical to a full upload of the new mesh. Randomly drawn meshes
//! (always), then real columns of the reference capture when it is there.

use rc_mesh::blocks::{BlockMesh, InPlace, in_place_diff, pack, pack_exact, packed_len};
use rc_mesh::fluid::FluidQuad;

struct Rng(u64);

impl Rng {
    fn next(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }
}

fn quad(r: &mut Rng) -> FluidQuad {
    FluidQuad {
        corners: std::array::from_fn(|_| std::array::from_fn(|_| r.next(64) as f32 * 0.25)),
        texture: r.next(900) as u32,
        colors: std::array::from_fn(|_| r.next(u64::from(u32::MAX)) as u32),
        light: std::array::from_fn(|_| r.next(0xffff) as u16),
    }
}

fn random_mesh(r: &mut Rng) -> BlockMesh {
    let sections = 1 + r.next(24) as usize;
    let faces = |r: &mut Rng| (0..r.next(400)).map(|_| std::array::from_fn(|_| r.next(1 << 20) as u32)).collect();
    let fluids = |r: &mut Rng| (0..r.next(12)).map(|_| quad(r)).collect();
    BlockMesh {
        x: r.next(64) as i32 - 32,
        z: r.next(64) as i32 - 32,
        version: 1,
        layers: [faces(r), faces(r), faces(r)],
        fluids: [fluids(r), fluids(r), fluids(r)],
        starts: std::array::from_fn(|_| (0..7 * (sections + 1)).map(|_| r.next(400) as u32).collect()),
        fluid_starts: std::array::from_fn(|_| (0..=sections).map(|_| r.next(12) as u32).collect()),
        vis: (0..sections).map(|_| r.next(u64::MAX)).collect(),
    }
}

/// Change of a partial remesh: a few modified elements (often at the start or the
/// end), sometimes none, sometimes elements moved from one layer to another (same total).
fn mutate(r: &mut Rng, m: &BlockMesh) -> BlockMesh {
    let mut n = BlockMesh {
        x: m.x,
        z: m.z,
        version: 2,
        layers: m.layers.clone(),
        fluids: m.fluids.clone(),
        starts: m.starts.clone(),
        fluid_starts: m.fluid_starts.clone(),
        vis: m.vis.clone(),
    };
    for _ in 0..r.next(4) {
        let l = r.next(3) as usize;
        let len = n.layers[l].len() as u64;
        if len == 0 {
            continue;
        }
        let i = match r.next(4) {
            0 => 0,
            1 => len - 1,
            _ => r.next(len),
        } as usize;
        n.layers[l][i][r.next(6) as usize] ^= 1 << r.next(32);
    }
    if r.next(5) == 0 && !n.layers[0].is_empty() {
        let f = n.layers[0].pop().expect("face");
        n.layers[1].insert(0, f);
    }
    for _ in 0..r.next(3) {
        let l = r.next(3) as usize;
        let len = n.fluids[l].len() as u64;
        if len > 0 {
            let i = r.next(len) as usize;
            n.fluids[l][i].colors[r.next(4) as usize] ^= 1 << r.next(32);
        }
    }
    n
}

/// Simulated buffer: other columns before and after (known pattern), then the announced range alone.
fn apply<T: Copy + PartialEq + std::fmt::Debug>(old: &[T], new: &[T], span: &Option<std::ops::Range<u32>>, pad: T) {
    let mut gpu: Vec<T> =
        std::iter::repeat_n(pad, 7).chain(old.iter().copied()).chain(std::iter::repeat_n(pad, 5)).collect();
    if let Some(s) = span {
        let s = s.start as usize..s.end as usize;
        gpu[7 + s.start..7 + s.end].copy_from_slice(&new[s]);
    }
    let full: Vec<T> =
        std::iter::repeat_n(pad, 7).chain(new.iter().copied()).chain(std::iter::repeat_n(pad, 5)).collect();
    assert_eq!(gpu, full, "range {span:?}: buffer differs from a full upload");
}

fn words(q: &FluidQuad) -> [u32; 27] {
    let mut w = Vec::new();
    w.extend(q.corners.iter().flatten().map(|c| c.to_bits()));
    w.push(q.texture);
    w.extend(q.colors);
    w.push(u32::from(q.light[0]) | u32::from(q.light[1]) << 16);
    w.push(u32::from(q.light[2]) | u32::from(q.light[3]) << 16);
    w.try_into().expect("27 words")
}

/// Checks a base → new mesh transition; true if it passes in place.
fn check(old: &BlockMesh, new: &BlockMesh) -> bool {
    let base = pack_exact(old);
    let Some(d) = in_place_diff(&base, new) else {
        let n = |m: &BlockMesh| {
            (m.layers.iter().map(Vec::len).sum::<usize>(), m.fluids[0].len() + m.fluids[1].len(), m.fluids[2].len())
        };
        assert_ne!(n(old), n(new), "numbers unchanged but in-place write refused");
        return false;
    };
    let faces = |m: &BlockMesh| m.layers.iter().flatten().copied().collect::<Vec<_>>();
    apply(&faces(old), &faces(new), &d.faces, [0xdead; 6]);
    let fl =
        |m: &BlockMesh, ls: std::ops::Range<usize>| ls.flat_map(|l| m.fluids[l].iter().map(words)).collect::<Vec<_>>();
    apply(&fl(old, 0..2), &fl(new, 0..2), &d.fluids, [0xbeef; 27]);
    apply(&fl(old, 2..3), &fl(new, 2..3), &d.trans, [0xcafe; 27]);
    // Shortest range: its two ends really differ.
    if let Some(s) = &d.faces {
        let (a, b) = (faces(old), faces(new));
        assert!(a[s.start as usize] != b[s.start as usize] && a[s.end as usize - 1] != b[s.end as usize - 1]);
    }
    true
}

#[test]
fn in_place_writes_match_full_upload() {
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    let (mut in_place, mut identical) = (0, 0);
    for _ in 0..1000 {
        let old = random_mesh(&mut r);
        let mut w = Vec::new();
        pack(&old, &mut w);
        assert_eq!(packed_len(&old), w.len(), "flat length");
        assert_eq!(pack_exact(&old), w, "exact flattening differs");
        let new = if r.next(6) == 0 { random_mesh(&mut r) } else { mutate(&mut r, &old) };
        if check(&old, &new) {
            in_place += 1;
            let base = pack_exact(&old);
            identical += usize::from(in_place_diff(&base, &new) == Some(InPlace::default()));
        }
    }
    eprintln!("1000 remeshes: {in_place} in place, of which {identical} with no byte to send");
    assert!(in_place > 600 && identical > 20, "draw too poor: {in_place} / {identical}");
}

/// Real columns (reference capture and installed game): exact flattening, identical base
/// = nothing to send, one changed face = that face alone.
#[test]
fn in_place_on_reference_columns() {
    let Some(meshes) = reference_meshes() else {
        let required = std::env::var("RUSTCRAFT_OPT_REQUIRE").as_deref() == Ok("1");
        assert!(!required, "capture or game absent (RUSTCRAFT_OPT_REQUIRE=1)");
        eprintln!("capture, data cache or game absent: test ignored");
        return;
    };
    assert!(meshes.len() > 100, "too few columns: {}", meshes.len());
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let mut faces = 0usize;
    for m in &meshes {
        let mut w = Vec::new();
        pack(m, &mut w);
        assert_eq!(pack_exact(m), w);
        assert_eq!(in_place_diff(&w, m), Some(InPlace::default()), "({}, {}): identical base", m.x, m.z);
        let n = mutate(&mut r, m);
        check(m, &n);
        faces += m.layers.iter().map(Vec::len).sum::<usize>();
    }
    eprintln!("{} real columns, {faces} faces", meshes.len());
}

fn reference_meshes() -> Option<Vec<BlockMesh>> {
    use std::collections::HashMap;
    use std::sync::Arc;
    let (cap, d) = (rc_testdata::reference_capture()?, rc_testdata::version_data()?);
    let ctx = rc_proto_v777::chunk::ChunkContext::new(24, d.states.len() as u32, 67);
    let mut cols: HashMap<(i32, i32), Arc<rc_world::Column>> = HashMap::new();
    for p in rc_testdata::play_packets(&cap, &d, "minecraft:level_chunk_with_light") {
        let c = rc_proto_v777::chunk::read_chunk(&mut rc_proto::Reader::new(&p), &ctx).ok()?;
        let col = rc_world::Column {
            x: c.x,
            z: c.z,
            sections: c.sections.into_boxed_slice(),
            sky_light: c.light.sky.into_boxed_slice(),
            block_light: c.light.block.into_boxed_slice(),
            heightmaps: c.heightmaps,
        };
        cols.insert((c.x, c.z), Arc::new(col));
    }
    let install = rc_data::locate("26.3").ok()?;
    let assets = rc_assets::Assets::for_install(&install.client_jar, &[], None).ok()?;
    let models = Arc::new(rc_assets::build(&assets, &d));
    let maps = rc_assets::texture::Colormaps::load(&assets);
    let names: Vec<String> = d.biomes.iter().map(|b| b.name.clone()).collect();
    let palette = rc_mesh::blocks::biome_palette(&d, &maps, &names);
    let t = rc_mesh::blocks::Tables::new(&d, models);
    let mut keys: Vec<_> = cols.keys().copied().collect();
    keys.sort_unstable();
    Some(
        keys.iter()
            .map(|&(x, z)| {
                let mut j = rc_mesh::wire::MeshJob::gather(cols[&(x, z)].clone(), 0, |x, z| cols.get(&(x, z)).cloned());
                j.biome_zoom_seed = 293_737_985_876_514_017;
                j.min_y = -64;
                rc_mesh::blocks::mesh(&j, &t, &palette, -64)
            })
            .collect(),
    )
}
