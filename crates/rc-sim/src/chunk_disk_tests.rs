//! Saved chunk compound: `/fillbiome` reaches the disk, on a chunk read from an official world and
//! on a chunk the generator has just made; the compound a chunk keeps for its save, however much
//! of it is left to memory (`chunk_disk`), gives the bytes of the old one.

use std::sync::Arc;

use rc_data::VersionData;
use rc_proto::nbt::Nbt;

use crate::chunk::{Chunk, Height};
use crate::chunk_disk::DiskKeeping;
use crate::server::Server;
use crate::states::States;
use crate::world::{Dimension, Load};

/// Biome of the 4×4×4 cell holding block `p` in the chunk `c`.
fn biome_at(c: &crate::chunk::Chunk, p: [i32; 3], min_y: i32) -> u32 {
    let ly = p[1] - min_y;
    let cell = (((ly & 15) >> 2) << 4) | (((p[2] & 15) >> 2) << 2) | ((p[0] & 15) >> 2);
    c.sections[(ly >> 4) as usize].biomes.get(cell as usize)
}

/// Runs `/fillbiome` over the 4×4×4 cell at `p` (as the server).
fn fill_cell(s: &mut Server, p: [i32; 3], biome: &str) {
    let text = format!("fillbiome {} {} {} {} {} {} {biome}", p[0], p[1], p[2], p[0] + 3, p[1] + 3, p[2] + 3);
    let src = s.server_source();
    let (ok, n) = s.perform(src, &text);
    assert!(ok && n > 0, "{text}: refused or nothing changed");
}

/// Chunk `col` as it reads back from the disk.
fn reread(
    dir: &std::path::Path,
    data: &rc_data::VersionData,
    states: &crate::states::States,
    col: (i32, i32),
) -> crate::chunk::Chunk {
    let h = crate::dimension_height(data, crate::dims::OVERWORLD);
    let dim = Dimension::new(dir, crate::dims::OVERWORLD, h);
    let loaded = dim.load(&[col], data, states);
    let Load::Loaded(c) = &loaded[0].1 else { panic!("chunk {col:?} not read back: {:?}", loaded[0].1) };
    (**c).clone()
}

/// A biome changed by `/fillbiome` is the one read back after the save, in a chunk of the official
/// world (its sections already hold biomes on disk) and in a chunk generated during the session
/// (the generator wrote them). The old save re-encoded the blocks of the marked sections but kept
/// the biomes read: the change was lost at the next opening.
#[test]
fn fillbiome_survives_the_save() {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else { return };
    let dir = crate::tests::fresh_copy(&world, "fillbiome-save");
    let data = Arc::new(data);
    let mut s = crate::portal_tests::open_world(&dir, data.clone());
    let states = s.states.clone();
    let min_y = s.dim.height.min_y;
    let near = ((s.pos[0].floor() as i32) >> 4, (s.pos[2].floor() as i32) >> 4);
    // Far from anything saved: made by the generator, never written yet.
    let far = (near.0 + 300, near.1 + 300);
    s.load_now(&[near, far]);
    assert!(s.chunks.get(&far).is_some_and(Option::is_some), "generated chunk {far:?} not loaded");
    let desert = states.biomes["minecraft:desert"];
    let cells: Vec<((i32, i32), [i32; 3])> =
        [near, far].iter().map(|&(cx, cz)| ((cx, cz), [cx * 16 + 4, 64, cz * 16 + 8])).collect();
    for &(col, p) in &cells {
        let c = s.chunks[&col].as_ref().unwrap();
        assert_ne!(biome_at(c, p, min_y), desert, "{col:?} already desert: the test would prove nothing");
    }
    for &(_, p) in &cells {
        fill_cell(&mut s, p, "minecraft:desert");
    }
    s.save(true);
    drop(s);
    for &(col, p) in &cells {
        let c = reread(&dir, &data, &states, col);
        assert_eq!(biome_at(&c, p, min_y), desert, "{col:?}: biome set by /fillbiome lost at the save");
    }
}

/// Deterministic draws (xorshift): the same edits on both copies of a chunk.
struct Draws(u64);

impl Draws {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// States as the server builds them: the world's biome registry and the height rules.
fn server_states(data: &VersionData) -> Option<States> {
    let regs = crate::registries::load(&crate::registries::client_jar(data)?, data).ok()?;
    let tags: hashbrown::HashMap<String, Vec<u32>> = regs
        .tags
        .iter()
        .find(|(r, _)| r == "minecraft:block")
        .map(|(_, t)| {
            t.iter()
                .map(|(n, ids)| {
                    let mut v: Vec<u32> = ids.iter().map(|&i| i as u32).collect();
                    v.sort_unstable();
                    (n.clone(), v)
                })
                .collect()
        })
        .unwrap_or_default();
    Some(States::new(data, regs.entries("minecraft:worldgen/biome").to_vec()).with_heightmap_rules(data, &tags))
}

/// The bench model world (`RUSTCRAFT_OPT_WORLD`) and the test world (`RUSTCRAFT_SOLO_WORLD`),
/// those present.
fn model_and_test_worlds() -> Vec<std::path::PathBuf> {
    let model = rc_data::env_os!("RUSTCRAFT_OPT_WORLD").map(std::path::PathBuf::from);
    [model, crate::tests::test_world()].into_iter().flatten().filter(|w| w.join("level.dat").is_file()).collect()
}

/// Every stored full chunk of the worlds: (dimension height, compound as stored).
fn stored_chunks(worlds: &[std::path::PathBuf], data: &VersionData, states: &States) -> Vec<(Height, Nbt)> {
    let mut out = Vec::new();
    for world in worlds {
        for name in crate::tests::DIMENSIONS {
            let h = crate::dimension_height(data, name);
            let dim = Dimension::new(world, name, h);
            out.extend(crate::tests::raw_and_parsed_chunks(&dim, data, states).into_iter().map(|(root, _)| (h, root)));
        }
    }
    out
}

/// A block placed like the server places one (height maps kept, section marked, block entity
/// of the old state dropped).
fn place(c: &mut Chunk, data: &VersionData, states: &States, h: Height, (x, y, z): (usize, i32, usize), state: u32) {
    let ly = (y - h.min_y) as usize;
    let idx = rc_world::block_index(x, ly & 15, z);
    if c.sections[ly >> 4].blocks.get(idx) == state {
        return;
    }
    c.sections[ly >> 4].set_blocks(&[(idx as u16, state)], data.state_bits() as u8, |s| states.is_air(s));
    c.update_heightmaps(x, y, z, state, &states.heightmap_rules, h);
    c.changed[ly >> 4] = true;
    c.dirty = true;
    let at = [c.x * 16 + x as i32, y, c.z * 16 + z as i32].map(|v| Some(i64::from(v)));
    c.block_entities.retain(|b| ["x", "y", "z"].map(|k| crate::nbtio::int(b, k)) != at);
}

/// One random edit of the kinds a session makes: a block (some with properties, some carrying
/// a block entity), a biome cell, a light layer replaced, emptied (relight giving none) or
/// shared, a block entity added.
fn random_edit(c: &mut Chunk, r: &mut Draws, data: &VersionData, states: &States, h: Height, palette: &[u32]) {
    match r.below(10) {
        0..=5 => {
            let y = h.min_y + r.below(h.sections as u64 * 16) as i32;
            let p = (r.below(16) as usize, y, r.below(16) as usize);
            let state = palette[r.below(palette.len() as u64) as usize];
            place(c, data, states, h, p, state);
        }
        6 => {
            let si = r.below(h.sections as u64) as usize;
            let biome = r.below(states.biome_names.len() as u64) as u32;
            let direct = (usize::BITS - states.biome_names.len().saturating_sub(1).leading_zeros()) as u8;
            c.sections[si].biomes.set(r.below(64) as usize, biome, 1, 3, direct);
            c.biomes_changed[si] = true;
            c.dirty = true;
        }
        7 | 8 => {
            let li = r.below(c.sky.len() as u64) as usize;
            let layer = match r.below(4) {
                0 => None,
                1 => Some(rc_world::zero_layer()),
                _ => Some(Arc::new([r.below(256) as u8; rc_world::LIGHT_BYTES])),
            };
            if r.below(2) == 0 {
                c.sky[li] = layer
            } else {
                c.block[li] = layer
            }
            c.dirty = true;
        }
        _ => {
            let p = [c.x * 16 + r.below(16) as i32, h.min_y + r.below(64) as i32, c.z * 16 + r.below(16) as i32];
            c.block_entities.push(Nbt::Compound(vec![
                ("id".into(), Nbt::String("minecraft:chest".into())),
                ("x".into(), Nbt::Int(p[0])),
                ("y".into(), Nbt::Int(p[1])),
                ("z".into(), Nbt::Int(p[2])),
            ]));
            c.dirty = true;
        }
    }
}

/// Bytes the save writes for `c`, the chunk then reset like the save resets it.
fn saved(
    c: &mut Chunk,
    without_copies: bool,
    keeping: DiskKeeping,
    data: &VersionData,
    states: &States,
    h: Height,
) -> Option<Vec<u8>> {
    let bytes = c.encode_for_save_with(without_copies, keeping, (data, states, h, 7), crate::nbtio::encode_root);
    c.changed.iter_mut().chain(c.biomes_changed.iter_mut()).for_each(|x| *x = false);
    c.dirty = false;
    bytes
}

/// Block states the edits draw from: air, plain blocks, states with properties (written as
/// compounds), block entity carriers.
fn edit_palette(data: &VersionData) -> Vec<u32> {
    let mut out = Vec::new();
    for name in [
        "minecraft:air",
        "minecraft:stone",
        "minecraft:glass",
        "minecraft:chest",
        "minecraft:oak_stairs",
        "minecraft:water",
    ] {
        if let Some(b) = data.blocks.iter().find(|b| b.name == name) {
            out.push(b.default_state);
            // The next state of the same block: one written with its properties.
            let other = b.default_state + 1;
            if data.block_of(other).is_some_and(|o| o.name == name) {
                out.push(other);
            }
        }
    }
    out
}

/// Every chunk of the bench model world and of the test world, read with everything kept as
/// read (all the switches at 0) and with everything memory holds left to memory: the save
/// writes the same bytes at once, after random edits (blocks, biomes, light, block entities),
/// after a second save that follows a relight leaving layers empty, after a save with nothing
/// changed, and in both ways of building the compound (copies or moves).
#[test]
fn kept_compound_saves_the_same_bytes() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some(states) = server_states(&data) else { return };
    let worlds = model_and_test_worlds();
    if worlds.is_empty() {
        return;
    }
    let palette = edit_palette(&data);
    // Each lead alone on top of the old representation, then all of them.
    let slim_only = DiskKeeping { slim: true, ..DiskKeeping::OLD };
    let no_duplicate_only = DiskKeeping { no_duplicate_block_entities: true, ..DiskKeeping::OLD };
    let rebuilt_without_lists = DiskKeeping { no_duplicate_block_entities: false, ..DiskKeeping::SLIMMEST };
    let variants = [DiskKeeping::SLIMMEST, slim_only, no_duplicate_only, rebuilt_without_lists];
    let mut r = Draws(0x2545_f491_4f6c_dd1d);
    let (mut chunks, mut edits, mut rebuilt, mut kept) = (0usize, 0usize, 0usize, 0usize);
    for (h, root) in stored_chunks(&worlds, &data, &states) {
        let read = |keeping| crate::chunk::parse_with(root.clone(), &data, &states, h, true, keeping).expect("chunk");
        let mut old = read(DiskKeeping::OLD);
        let mut news: Vec<Chunk> = variants.iter().map(|&k| read(k)).collect();
        let at = format!("({}, {})", old.x, old.z);
        for d in &news[0].disk_sections {
            match d.form {
                crate::chunk_disk::SectionForm::Rebuilt(_) => rebuilt += 1,
                crate::chunk_disk::SectionForm::Kept(_) => kept += 1,
                crate::chunk_disk::SectionForm::Absent => {}
            }
        }
        let first = old.encode(&data, &states, h, 7);
        for (new, k) in news.iter().zip(&variants) {
            assert!(first == new.encode(&data, &states, h, 7), "{at} {k:?}: compound at once");
        }
        // A few chunks take many edits, every chunk at least a save.
        let many = chunks % 40 == 0;
        for round in 0..if many { 3 } else { 1 } {
            let n = if many { 100 } else { r.below(4) as usize };
            for _ in 0..n {
                let seed = r.0;
                random_edit(&mut old, &mut r, &data, &states, h, &palette);
                for new in &mut news {
                    r.0 = seed;
                    random_edit(new, &mut r, &data, &states, h, &palette);
                }
                edits += 1;
            }
            let a = saved(&mut old, round % 2 == 1, DiskKeeping::OLD, &data, &states, h);
            for (new, &k) in news.iter_mut().zip(&variants) {
                assert!(
                    a == saved(new, round % 2 == 0, k, &data, &states, h),
                    "{at} {k:?}: save {round} after {n} edits"
                );
            }
        }
        // Nothing changed since the last save: what each kept alone gives the same bytes.
        let a = saved(&mut old, true, DiskKeeping::OLD, &data, &states, h);
        for (new, &k) in news.iter_mut().zip(&variants) {
            assert!(a == saved(new, false, k, &data, &states, h), "{at} {k:?}: unchanged save");
            assert!(
                old.encode(&data, &states, h, 7) == new.encode(&data, &states, h, 7),
                "{at} {k:?}: copied compound"
            );
        }
        chunks += 1;
    }
    eprintln!("{chunks} chunks, {edits} edits; sections rebuilt from memory {rebuilt}, kept {kept}");
    assert!(chunks > 1000 && edits > 10_000, "{chunks} chunks, {edits} edits");
    assert!(rebuilt > 50 * kept.max(1), "few sections rebuilt from memory: {rebuilt} (kept {kept})");
}

/// Kept compound of the bench model world, all the switches at 0 against everything memory
/// holds left to memory (MB, printed; a fall of at least four fifths expected).
#[test]
fn kept_compound_of_the_model_world() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some(states) = server_states(&data) else { return };
    let Some(model) = rc_data::env_os!("RUSTCRAFT_OPT_WORLD").map(std::path::PathBuf::from) else { return };
    let mut totals = [crate::mem_stats::RootBytes::default(); 2];
    for (h, root) in stored_chunks(&[model], &data, &states) {
        for (total, keeping) in totals.iter_mut().zip([DiskKeeping::OLD, DiskKeeping::SLIMMEST]) {
            let c = crate::chunk::parse_with(root.clone(), &data, &states, h, true, keeping).expect("chunk");
            *total += crate::mem_stats::kept_compound(&c);
        }
    }
    let mb = |b: &crate::mem_stats::RootBytes| b.0.iter().sum::<u64>() as f64 / 1_048_576.0;
    for (label, t) in ["all kept", "left to memory"].iter().zip(&totals) {
        let parts: Vec<String> = crate::mem_stats::ROOT_PARTS
            .iter()
            .zip(t.0)
            .map(|(k, b)| format!("{k} {:.1}", b as f64 / 1_048_576.0))
            .collect();
        eprintln!("{label}: kept compound {:.1} MB ({})", mb(t), parts.join(", "));
    }
    assert!(
        mb(&totals[1]) * 5.0 < mb(&totals[0]),
        "kept compound {:.1} MB against {:.1} MB",
        mb(&totals[1]),
        mb(&totals[0])
    );
}

/// Conversions of the overworld with the decoration block table (as the generator builds them).
fn overworld_conv<'a>(
    data: &'a VersionData,
    states: &'a States,
    blocks: &'a rc_worldgen::decor_blocks::Blocks,
) -> crate::generation_nbt::Conv<'a> {
    let h = crate::dimension_height(data, crate::dims::OVERWORLD);
    crate::generation_nbt::Conv::new(data, states, blocks, h, &states.biome_names)
}

fn decoration_blocks(data: &VersionData) -> Option<rc_worldgen::decor_blocks::Blocks> {
    let jar = crate::registries::client_jar(data)?;
    rc_worldgen::decor_blocks::Blocks::new(data, &rc_worldgen::decor_data::DecorData::from_jar(&jar).ok()?).ok()
}

/// Full chunks made by the generator's conversion from overworld chunks of the bench model world
/// (villages, mineshafts, caves: blocks, biomes, height maps, ticks and block entities read back
/// as decoration chunks), with every way of keeping their compound: the save writes the bytes of
/// the old conversion at once, after edits and after a second save. The block entity lists are
/// the same with and without skipping the sections that hold none, and a chunk read back gives
/// the same decoration chunk whether all its sections or only the absent ones are filled first.
#[test]
fn generated_chunks_save_the_same_bytes() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some(states) = server_states(&data) else { return };
    let Some(blocks) = decoration_blocks(&data) else { return };
    let Some(model) = rc_data::env_os!("RUSTCRAFT_OPT_WORLD").map(std::path::PathBuf::from) else { return };
    let conv = overworld_conv(&data, &states, &blocks);
    let h = conv.height;
    let dim = Dimension::new(&model, crate::dims::OVERWORLD, h);
    let lazy_only = DiskKeeping { slim: true, lazy_generated_sections: true, ..DiskKeeping::OLD };
    let slim_not_lazy = DiskKeeping { lazy_generated_sections: false, ..DiskKeeping::SLIMMEST };
    let variants = [DiskKeeping::SLIMMEST, lazy_only, slim_not_lazy];
    let palette = edit_palette(&data);
    let mut r = Draws(0x9e37_79b9_7f4a_7c15);
    let (mut chunks, mut with_entities) = (0usize, 0usize);
    for (root, read) in crate::tests::raw_and_parsed_chunks(&dim, &data, &states).into_iter().step_by(7).take(600) {
        let at = format!("({}, {})", read.x, read.z);
        let (dc, _, disk) = conv.decoration_chunk_with(&root, true).expect("decoration chunk");
        let (filled, _, _) = conv.decoration_chunk_with(&root, false).expect("decoration chunk");
        assert!(
            dc.blocks == filled.blocks && dc.biomes == filled.biomes && dc.heights == filled.heights,
            "{at} read back"
        );
        let refs: Vec<&rc_world::Container> = read.sections.iter().map(|s| &s.blocks).collect();
        let skipped = conv.block_entities_with(&dc, false, &disk, &refs, true);
        assert!(skipped == conv.block_entities_with(&dc, false, &disk, &refs, false), "{at} block entities");
        with_entities += usize::from(!skipped.is_empty());
        let light = (read.sky.clone(), read.block.clone());
        let mut old = conv.full_chunk_with(&dc, light.clone(), 0, &disk, DiskKeeping::OLD);
        let mut news: Vec<Chunk> =
            variants.iter().map(|&k| conv.full_chunk_with(&dc, light.clone(), 0, &disk, k)).collect();
        for round in 0..2 {
            let a = saved(&mut old, false, DiskKeeping::OLD, &data, &states, h);
            for (new, &k) in news.iter_mut().zip(&variants) {
                assert!(a == saved(new, round == 0, k, &data, &states, h), "{at} {k:?}: save {round}");
            }
            for _ in 0..20 {
                let seed = r.0;
                random_edit(&mut old, &mut r, &data, &states, h, &palette);
                for new in &mut news {
                    r.0 = seed;
                    random_edit(new, &mut r, &data, &states, h, &palette);
                }
            }
        }
        chunks += 1;
    }
    eprintln!("{chunks} generated chunks ({with_entities} with block entities)");
    assert!(chunks >= 500 && with_entities > 10, "{chunks} chunks, {with_entities} with block entities");
}

/// A chest alone in an otherwise empty section is still listed when the sections without block
/// entity state are skipped (the palette of its section holds it).
#[test]
fn lone_chest_section_is_not_skipped() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some(states) = server_states(&data) else { return };
    let Some(blocks) = decoration_blocks(&data) else { return };
    let conv = overworld_conv(&data, &states, &blocks);
    let h = conv.height;
    let mut dc = rc_worldgen::decor_region::DecoChunk::new(0, 0, h.min_y, h.sections as i32 * 16);
    dc.blocks.fill(states.air_state);
    let chest = data.blocks.iter().find(|b| b.name == "minecraft:chest").expect("chest").default_state;
    let at = dc.index(3, 70, 5);
    dc.blocks[at] = chest;
    let direct = data.state_bits() as u8;
    let containers: Vec<rc_world::Container> =
        dc.blocks.chunks(rc_world::SECTION_BLOCKS).map(|s| rc_world::Container::pack(s, 4, 8, direct)).collect();
    let refs: Vec<&rc_world::Container> = containers.iter().collect();
    for skip in [false, true] {
        let list = conv.block_entities_with(&dc, false, &[], &refs, skip);
        assert_eq!(list.len(), 1, "skip {skip}: {list:?}");
        assert_eq!(crate::nbtio::int(&list[0], "y"), Some(70));
    }
}

/// Packed words of `indices` on `bits` bits (disk layout: no index across two words).
fn packed_words(indices: &[u64], bits: usize) -> Nbt {
    let per_word = 64 / bits;
    Nbt::LongArray(
        indices
            .chunks(per_word)
            .map(|w| w.iter().enumerate().fold(0u64, |acc, (j, &i)| acc | (i << (j * bits))) as i64)
            .collect(),
    )
}

/// A disk block container: palette, then (with several entries) the words of `indices`.
fn crafted_blocks(palette: Vec<Nbt>, indices: &[u64]) -> Nbt {
    let bits = crate::chunk::disk_bits(palette.len(), true) as usize;
    let mut f = vec![("palette".to_owned(), Nbt::List(palette))];
    if !indices.is_empty() {
        f.push(("data".to_owned(), packed_words(indices, bits)));
    }
    Nbt::Compound(f)
}

fn plains_biomes() -> Nbt {
    Nbt::Compound(vec![("palette".into(), Nbt::List(vec![Nbt::String("minecraft:plains".into())]))])
}

/// Section compounds the game never writes but a world edited by a tool may hold, each with
/// whether memory can rebuild it byte for byte (otherwise it is kept as read).
fn crafted_sections(data: &VersionData, states: &States) -> Vec<(&'static str, Vec<Nbt>, bool)> {
    let block = |n: &str| data.blocks.iter().find(|b| b.name == n).expect(n);
    let name = |n: &str| Nbt::String(n.into());
    let stairs = block("minecraft:oak_stairs");
    let stairs_state = stairs.default_state + 5;
    let ours = states.palette_entry(data, stairs_state);
    // Properties in an order that is neither ours nor the game's.
    let mut reversed = ours.clone();
    if let Some(Nbt::Compound(p)) = crate::nbtio::get_mut(&mut reversed, "properties") {
        p.reverse();
    }
    let mut legacy = ours.clone();
    if let Nbt::Compound(f) = &mut legacy {
        f[0].0 = "Name".into();
        f[1].0 = "Properties".into();
    }
    // Alternating stone and glass, an unused dirt between them.
    let alternate: Vec<u64> = (0..4096).map(|i| if i % 2 == 0 { 0 } else { 2 }).collect();
    let section =
        |fields: Vec<(&str, Nbt)>| Nbt::Compound(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
    let usual = |blocks: Nbt| section(vec![("Y", Nbt::Byte(0)), ("block_states", blocks), ("biomes", plains_biomes())]);
    let three = || vec![name("minecraft:stone"), name("minecraft:dirt"), name("minecraft:glass")];
    let with_stairs = |entry: Nbt| {
        crafted_blocks(vec![name("minecraft:stone"), entry], &alternate.iter().map(|&i| i / 2).collect::<Vec<_>>())
    };
    vec![
        ("unused palette entry", vec![usual(crafted_blocks(three(), &alternate))]),
        (
            "palette out of order",
            vec![usual(crafted_blocks(three(), &alternate.iter().map(|&i| 2 - i).collect::<Vec<_>>()))],
        ),
        ("our property order", vec![usual(with_stairs(ours))]),
        ("third property order", vec![usual(with_stairs(reversed))]),
        ("legacy entry keys", vec![usual(with_stairs(legacy))]),
        (
            "unknown key",
            vec![section(vec![
                ("Y", Nbt::Byte(0)),
                ("block_states", crafted_blocks(three(), &alternate)),
                ("biomes", plains_biomes()),
                ("custom", Nbt::Int(7)),
            ])],
        ),
        (
            "Y as an int",
            vec![section(vec![
                ("Y", Nbt::Int(0)),
                ("block_states", crafted_blocks(three(), &alternate)),
                ("biomes", plains_biomes()),
            ])],
        ),
        (
            "height twice",
            vec![usual(crafted_blocks(three(), &alternate)), usual(crafted_blocks(vec![name("minecraft:glass")], &[]))],
        ),
        ("single entry with words", vec![usual(crafted_blocks(vec![name("minecraft:stone")], &[0; 4096]))]),
        (
            "height twice, the first with empty words",
            vec![
                usual(Nbt::Compound(vec![
                    ("palette".into(), Nbt::List(vec![name("minecraft:stone")])),
                    ("data".into(), Nbt::LongArray(Vec::new())),
                ])),
                usual(crafted_blocks(three(), &alternate)),
            ],
        ),
        (
            "blocks without biomes",
            vec![section(vec![("Y", Nbt::Byte(0)), ("block_states", crafted_blocks(three(), &alternate))])],
        ),
        (
            "light only",
            vec![section(vec![("Y", Nbt::Byte(0)), ("SkyLight", Nbt::ByteArray(vec![0x37; rc_world::LIGHT_BYTES]))])],
        ),
    ]
    .into_iter()
    .map(|(case, sections)| {
        let rebuilt = matches!(
            case,
            "unused palette entry"
                | "palette out of order"
                | "our property order"
                | "blocks without biomes"
                | "light only"
        );
        (case, sections, rebuilt)
    })
    .collect()
}

/// Sections no tool of ours or the game writes (unused palette entry, palette out of order, a
/// third property order, legacy entry keys, an unknown key, `Y` as an int, a height written twice,
/// a single entry with words, blocks without biomes, light only): those memory rebuilds exactly
/// are rebuilt, the others kept, and the save writes the bytes of the old one at once and after
/// edits, with every way of keeping the compound.
#[test]
fn crafted_sections_save_the_same_bytes() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some(states) = server_states(&data) else { return };
    let Some((h, base)) =
        stored_chunks(&model_and_test_worlds(), &data, &states).into_iter().find(|(h, _)| h.min_y == -64)
    else {
        return;
    };
    let palette = edit_palette(&data);
    let variants = [DiskKeeping::SLIMMEST, DiskKeeping { slim: true, ..DiskKeeping::OLD }];
    let mut r = Draws(0x51ed_270b_2715_a001);
    for (case, crafted, rebuildable) in crafted_sections(&data, &states) {
        let mut root = base.clone();
        let Some(Nbt::List(sections)) = crate::nbtio::get_mut(&mut root, "sections") else { panic!("sections") };
        sections.retain(|s| crate::nbtio::int(s, "Y") != Some(0));
        sections.extend(crafted);
        let read = |k| crate::chunk::parse_with(root.clone(), &data, &states, h, true, k).expect(case);
        let mut old = read(DiskKeeping::OLD);
        let li = (0 - (h.min_section() - 1)) as usize;
        let rebuilt =
            matches!(read(DiskKeeping::SLIMMEST).disk_sections[li].form, crate::chunk_disk::SectionForm::Rebuilt(_));
        assert_eq!(rebuilt, rebuildable, "{case}: rebuilt from memory");
        let mut news: Vec<Chunk> = variants.iter().map(|&k| read(k)).collect();
        for round in 0..3 {
            let a = saved(&mut old, false, DiskKeeping::OLD, &data, &states, h);
            for (new, &k) in news.iter_mut().zip(&variants) {
                assert!(a == saved(new, round == 1, k, &data, &states, h), "{case} {k:?}: save {round}");
            }
            for _ in 0..30 {
                let seed = r.0;
                random_edit(&mut old, &mut r, &data, &states, h, &palette);
                for new in &mut news {
                    r.0 = seed;
                    random_edit(new, &mut r, &data, &states, h, &palette);
                }
            }
        }
    }
}

/// Time of one run of `f` over `items`, split between `threads` threads, in µs per item.
fn us_per_item<T: Sync>(items: &[T], threads: usize, f: &(impl Fn(&T) + Sync)) -> f64 {
    let t = std::time::Instant::now();
    std::thread::scope(|s| {
        for part in items.chunks(items.len().div_ceil(threads)) {
            s.spawn(move || part.iter().for_each(f));
        }
    });
    t.elapsed().as_secs_f64() * 1e6 / items.len() as f64
}

/// `[bench]` line of an old/new pair: best of 15 runs alternated between the two (a loaded
/// machine drifts between runs), µs per chunk on 1 thread, then 2, 4… up to the logical cores.
fn bench_pair<T: Sync>(case: &str, items: &[T], old: impl Fn(&T) + Sync, new: impl Fn(&T) + Sync) {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut line = format!("[bench] kept_compound {case:24} old/new µs per chunk:");
    let mut threads = 1;
    while threads <= cores {
        let (mut a, mut b) = (f64::MAX, f64::MAX);
        for _ in 0..15 {
            a = a.min(us_per_item(items, threads, &old));
            b = b.min(us_per_item(items, threads, &new));
        }
        line += &format!("  {threads}t {a:.1}/{b:.1}");
        threads *= 2;
    }
    eprintln!("{line} ({} chunks)", items.len());
}

/// A chunk read with all the switches at 0 and the same chunk read with everything memory holds
/// left to memory, each marked changed or not before every save.
struct SavedPair {
    old: std::sync::Mutex<Chunk>,
    new: std::sync::Mutex<Chunk>,
    changed: bool,
}

fn save_again(
    c: &std::sync::Mutex<Chunk>,
    changed: bool,
    without_copies: bool,
    keeping: DiskKeeping,
    (data, states, h): (&VersionData, &States, Height),
) {
    let mut c = c.lock().unwrap();
    c.changed.iter_mut().for_each(|x| *x = changed);
    std::hint::black_box(c.encode_for_save_with(
        without_copies,
        keeping,
        (data, states, h, 7),
        crate::nbtio::encode_root,
    ));
}

/// Micro-bench (`cargo test --release -p rc-sim chunk_disk_tests::bench_kept_compound -- --ignored
/// --nocapture`, bench model world), all the switches at 0 against each lead: µs per chunk to read
/// a stored chunk, to write it at a save (sections unchanged, as at an autosave of a chunk an
/// entity or a block entity dirtied; then every section changed), for the generator to make a
/// full chunk, to read one back, to list its block entities, to count blocks and read light.
#[test]
#[ignore = "micro-bench"]
fn bench_kept_compound() {
    use std::hint::black_box;
    let data = rc_testdata::version_data().expect("26.3 data");
    let states = server_states(&data).expect("registries");
    let model = rc_data::env_os!("RUSTCRAFT_OPT_WORLD").map(std::path::PathBuf::from).expect("RUSTCRAFT_OPT_WORLD");
    let h = crate::dimension_height(&data, crate::dims::OVERWORLD);
    let roots: Vec<Nbt> = stored_chunks(&[model], &data, &states)
        .into_iter()
        .filter(|(hh, _)| *hh == h)
        .map(|(_, r)| r)
        .take(3000)
        .collect();
    let read = |r: &Nbt, k| crate::chunk::parse_with(r.clone(), &data, &states, h, true, k).expect("chunk");
    bench_pair(
        "read",
        &roots,
        |r| drop(black_box(read(r, DiskKeeping::OLD))),
        |r| drop(black_box(read(r, DiskKeeping::SLIMMEST))),
    );
    bench_pair(
        "read, slim only",
        &roots,
        |r| drop(black_box(read(r, DiskKeeping::OLD))),
        |r| drop(black_box(read(r, DiskKeeping { slim: true, ..DiskKeeping::OLD }))),
    );
    let ctx = (&data, &states, h);
    for changed in [false, true] {
        let pairs: Vec<SavedPair> = roots
            .iter()
            .map(|r| SavedPair {
                old: std::sync::Mutex::new(read(r, DiskKeeping::OLD)),
                new: std::sync::Mutex::new(read(r, DiskKeeping::SLIMMEST)),
                changed,
            })
            .collect();
        let case = if changed { "save, all changed" } else { "save, unchanged" };
        bench_pair(
            case,
            &pairs,
            |p| save_again(&p.old, p.changed, false, DiskKeeping::OLD, ctx),
            |p| save_again(&p.new, p.changed, true, DiskKeeping::SLIMMEST, ctx),
        );
    }
    let parsed: Vec<Chunk> = roots.iter().map(|r| read(r, DiskKeeping::OLD)).collect();
    let count = |c: &Chunk, by_histogram: bool| {
        for s in &c.sections {
            black_box(crate::chunk::count_non_empty_with(&s.blocks, &states, by_histogram));
        }
    };
    bench_pair("count blocks", &parsed, |c| count(c, false), |c| count(c, true));
    let light: Vec<Vec<Nbt>> = roots
        .iter()
        .map(|r| {
            let sections = crate::nbtio::list(r.get("sections"));
            sections.iter().flat_map(|s| ["SkyLight", "BlockLight"].map(|k| s.get(k).cloned())).flatten().collect()
        })
        .collect();
    let read_light = |layers: &Vec<Nbt>, one_copy: bool| {
        for l in layers {
            black_box(crate::chunk::read_light_layer_with(Some(l), true, one_copy).ok());
        }
    };
    bench_pair("light read", &light, |l| read_light(l, false), |l| read_light(l, true));
    let Some(blocks) = decoration_blocks(&data) else { return };
    let conv = overworld_conv(&data, &states, &blocks);
    let decorated: Vec<_> = roots.iter().take(1000).filter_map(|r| conv.from_nbt(r)).collect();
    let full = |(dc, _, disk): &(rc_worldgen::decor_region::DecoChunk, u8, Vec<Nbt>), k| {
        drop(black_box(conv.full_chunk_with(dc, (Vec::new(), Vec::new()), 0, disk, k)));
    };
    bench_pair("full chunk", &decorated, |d| full(d, DiskKeeping::OLD), |d| full(d, DiskKeeping::SLIMMEST));
    bench_pair(
        "read back",
        &roots[..1000],
        |r| drop(black_box(conv.decoration_chunk_with(r, false))),
        |r| drop(black_box(conv.decoration_chunk_with(r, true))),
    );
    let lists: Vec<(&rc_worldgen::decor_region::DecoChunk, &Vec<Nbt>, Vec<rc_world::Container>)> = decorated
        .iter()
        .map(|(dc, _, disk)| {
            let c = conv.full_chunk_with(dc, (Vec::new(), Vec::new()), 0, disk, DiskKeeping::OLD);
            (dc, disk, c.sections.into_iter().map(|s| s.blocks).collect())
        })
        .collect();
    let list =
        |(dc, disk, containers): &(&rc_worldgen::decor_region::DecoChunk, &Vec<Nbt>, Vec<rc_world::Container>),
         skip| {
            let refs: Vec<&rc_world::Container> = containers.iter().collect();
            black_box(conv.block_entities_with(dc, false, disk, &refs, skip));
        };
    bench_pair("block entity list", &lists, |l| list(l, false), |l| list(l, true));
}
