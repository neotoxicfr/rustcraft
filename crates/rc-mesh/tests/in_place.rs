// Direct read allowed (tests and benches: settings read at launch, off the measured path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! In-place writing of the partial remeshes (`in_place_diff`, `in_place_diff_with`) and exact-size
//! flattening (`pack_exact`): a simulated buffer (GPU bytes, other columns around) that receives only
//! the announced ranges must become identical to a full upload of the new mesh, on every path (one
//! range or several, whole column or remeshed sections only compared). Randomly drawn meshes
//! (always), then real columns of the reference capture when it is there.

mod common;

use common::check_in_place;
use rc_mesh::blocks::{BUCKETS, BlockMesh, Face, InPlace, InPlacePaths, in_place_diff, in_place_diff_with, pack, pack_exact, packed_len};
use rc_mesh::fluid::FluidQuad;
use rc_mesh::wire::Sections;

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

/// Checks a base → new mesh transition on every path (whole column compared: the random meshes
/// do not come from the mesher); true if it passes in place.
fn check(old: &BlockMesh, new: &BlockMesh) -> bool {
    let n = |m: &BlockMesh| {
        (m.layers.iter().map(Vec::len).sum::<usize>(), m.fluids[0].len() + m.fluids[1].len(), m.fluids[2].len())
    };
    let in_place = check_in_place(old, new, None).is_some();
    assert!(in_place || n(old) != n(new), "numbers unchanged but in-place write refused");
    in_place
}

/// A mesh made of cells: faces per layer and (group × (sections + 1) + section), the last cell of
/// each group empty; fluid faces per layer and section.
struct Cells {
    sections: usize,
    faces: [Vec<Vec<Face>>; 3],
    fluids: [Vec<Vec<FluidQuad>>; 3],
}

impl Cells {
    fn random(r: &mut Rng) -> Cells {
        let sections = 1 + r.next(24) as usize;
        let n = sections + 1;
        let face = |r: &mut Rng| std::array::from_fn(|_| r.next(1 << 20) as u32);
        let faces = std::array::from_fn(|l| {
            (0..BUCKETS * n)
                .map(|k| if k % n == sections { Vec::new() } else { (0..r.next([30, 8, 3][l])).map(|_| face(r)).collect() })
                .collect()
        });
        let fluids = std::array::from_fn(|_| (0..sections).map(|_| (0..r.next(3)).map(|_| quad(r)).collect()).collect());
        Cells { sections, faces, fluids }
    }

    fn mesh(&self, version: u64) -> BlockMesh {
        let flat = |cells: &[Vec<Face>]| -> (Vec<Face>, Vec<u32>) {
            let mut starts = Vec::new();
            let mut all = Vec::new();
            for c in cells {
                starts.push(all.len() as u32);
                all.extend_from_slice(c);
            }
            (all, starts)
        };
        let (layers, starts): (Vec<_>, Vec<_>) = self.faces.iter().map(|l| flat(l)).unzip();
        let mut fluid_starts: [Vec<u32>; 3] = Default::default();
        let fluids = std::array::from_fn(|l| {
            let mut all = Vec::new();
            for c in &self.fluids[l] {
                fluid_starts[l].push(all.len() as u32);
                all.extend_from_slice(c);
            }
            fluid_starts[l].push(all.len() as u32);
            all
        });
        BlockMesh {
            x: 3,
            z: -5,
            version,
            layers: layers.try_into().expect("3 layers"),
            fluids,
            starts: starts.try_into().expect("3 layers"),
            fluid_starts,
            vis: vec![0; self.sections],
        }
    }

    /// Cells of `layer` in remeshed sections.
    fn remeshed_cells(&self, layer: usize, mask: Sections) -> Vec<usize> {
        let n = self.sections + 1;
        (0..self.faces[layer].len()).filter(|&k| k % n != self.sections && mask.contains(k % n)).collect()
    }

    /// Remesh of the sections of `mask` only, like the mesher's: their faces and fluid faces change,
    /// some move to another group or layer of a remeshed section (same totals: the cells between
    /// them move in the flat sequence); the other sections stay as they are.
    fn remesh(&mut self, r: &mut Rng, mask: Sections) {
        for _ in 0..r.next(6) {
            let l = r.next(3) as usize;
            let cells = self.remeshed_cells(l, mask);
            let Some(&k) = cells.get(r.next(cells.len() as u64) as usize) else { continue };
            if let Some(f) = self.faces[l][k].first_mut() {
                f[r.next(6) as usize] ^= 1 << r.next(32);
            }
        }
        if r.next(3) == 0 {
            let (from, to) = (r.next(3) as usize, r.next(3) as usize);
            let (a, b) = (self.remeshed_cells(from, mask), self.remeshed_cells(to, mask));
            if let (Some(&ka), Some(&kb)) = (a.get(r.next(a.len() as u64) as usize), b.get(r.next(b.len() as u64) as usize))
                && let Some(f) = self.faces[from][ka].pop()
            {
                self.faces[to][kb].insert(0, f);
            }
        }
        for s in (0..self.sections).filter(|&s| mask.contains(s)) {
            for l in 0..3 {
                if let Some(q) = self.fluids[l][s].first_mut().filter(|_| r.next(2) == 0) {
                    q.colors[r.next(4) as usize] ^= 1 << r.next(32);
                }
            }
            // Fluid face moved between the opaque and cutout layers (same flat sequence).
            if r.next(4) == 0
                && let Some(q) = self.fluids[0][s].pop()
            {
                self.fluids[1][s].push(q);
            }
        }
    }
}

fn random_mask(r: &mut Rng, sections: usize) -> Sections {
    let mut mask = Sections::default();
    for _ in 0..1 + r.next(3) {
        mask.insert(r.next(sections as u64) as i32);
    }
    mask
}

/// Partial remeshes of random columns whose other sections are copies: comparing only the remeshed
/// sections (and the cells that moved) gives the whole comparison's ranges, for every gap, and the
/// ranges give a full upload. Several ranges for a change spread over groups.
#[test]
fn touched_sections_only_match_the_whole_comparison() {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut in_place, mut several) = (0, 0);
    for _ in 0..2000 {
        let mut cells = Cells::random(&mut r);
        let old = cells.mesh(1);
        let mask = random_mask(&mut r, cells.sections);
        cells.remesh(&mut r, mask);
        let new = cells.mesh(2);
        if let Some(writes) = check_in_place(&old, &new, Some(mask)) {
            in_place += 1;
            several += usize::from(writes > 3);
        }
    }
    eprintln!("2000 partial remeshes: {in_place} in place, {several} with several ranges");
    assert!(in_place > 1200 && several > 200, "draw too poor: {in_place} / {several}");
}

/// A base flattened with a GPU tag on its faces (`pack_tagged`) reads like the plain one: same mesh
/// once unpacked, same in-place ranges on every path; its face words are the tagged faces, layer by
/// layer (`packed_face_words`). Sabotage: its raw faces do differ from the plain ones (the equality
/// comes from taking the tag off), and a tag left on is seen.
#[test]
fn tagged_base_reads_like_the_plain_one() {
    use rc_mesh::blocks::{GPU_TAG_BITS, pack_exact_tagged, packed_face_words, unpack};
    let mut r = Rng(0x0dd_ba11_cafe_f00d);
    let mut tagged_faces = 0;
    for round in 0..500u32 {
        let mut cells = Cells::random(&mut r);
        let old = cells.mesh(1);
        let mask = random_mask(&mut r, cells.sections);
        cells.remesh(&mut r, mask);
        let new = cells.mesh(2);
        let tag = (1 + round % 255) << 20;
        let (plain, tagged) = (pack_exact(&old), pack_exact_tagged(&old, tag));
        let back = unpack(&tagged, 1).expect("flat");
        assert!(back.layers == old.layers && back.starts == old.starts && back.fluids == old.fluids, "tag not taken off");
        let words = packed_face_words(&tagged).expect("flat");
        for (l, w) in words.iter().enumerate() {
            let faces = tagged[w.clone()].as_chunks::<6>().0;
            assert_eq!(faces.len(), old.layers[l].len());
            assert!(faces.iter().zip(&old.layers[l]).all(|(t, f)| t[0] == f[0] | tag && t[1..] == f[1..]));
            assert!(faces.iter().all(|t| t[0] & GPU_TAG_BITS == tag));
            tagged_faces += faces.len();
        }
        for gaps in common::IN_PLACE_GAPS {
            for (remeshed, touched_only) in [(None, false), (Some(mask), true)] {
                let paths = InPlacePaths { touched_only, gaps };
                assert_eq!(in_place_diff_with(&tagged, &new, remeshed, paths), in_place_diff_with(&plain, &new, remeshed, paths));
            }
        }
        assert_eq!(in_place_diff(&tagged, &new), in_place_diff(&plain, &new));
    }
    assert!(tagged_faces > 100_000, "draw too poor: {tagged_faces}");
    // A face of the same mesh, read raw from the tagged base, is not the plain face.
    let old = Cells::random(&mut r).mesh(1);
    let tagged = pack_exact_tagged(&old, 7 << 20);
    let w = packed_face_words(&tagged).expect("flat")[0].clone();
    if let Some(f) = tagged[w].as_chunks::<6>().0.first() {
        assert_ne!(*f, old.layers[0][0], "tag not written");
    }
}

/// Sabotage: a face changed in a section outside the remesh (a mesher fault the full check would
/// see) is missed by the touched-only comparison, seen by the whole one.
#[test]
fn face_outside_the_remeshed_sections_is_only_seen_by_the_whole_comparison() {
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    let mut seen = 0;
    for _ in 0..500 {
        let mut cells = Cells::random(&mut r);
        let old = cells.mesh(1);
        let mask = random_mask(&mut r, cells.sections);
        let mut outside = Sections::ALL;
        outside.subtract(mask);
        let Some(&k) = cells.remeshed_cells(0, outside).first() else { continue };
        let Some(f) = cells.faces[0][k].first_mut() else { continue };
        f[2] ^= 1;
        let new = cells.mesh(2);
        let base = pack_exact(&old);
        let gaps = Some([0; 3]);
        let whole = in_place_diff_with(&base, &new, None, InPlacePaths { touched_only: false, gaps }).expect("same counts");
        let touched =
            in_place_diff_with(&base, &new, Some(mask), InPlacePaths { touched_only: true, gaps }).expect("same counts");
        assert!(!whole.faces.is_empty() && touched.faces.is_empty(), "sabotage outside the mask not isolated");
        seen += 1;
    }
    assert!(seen > 100, "draw too poor: {seen}");
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
    use hashbrown::HashMap;
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
