//! Chunks of an older version on disk, met by the generation: a world opened once in the official
//! game keeps the old data of every chunk it did not visit.

use std::path::Path;

use rc_proto::nbt::Nbt;

use super::Out;
use super::inflight_tests::{template, worker_in};

const DIM: &str = "minecraft:overworld";

fn region_path(world: &Path, (x, z): (i32, i32)) -> std::path::PathBuf {
    crate::world::dimension_dir(world, DIM).join("region").join(crate::region::region_name(x, z))
}

fn write_raw(world: &Path, p: (i32, i32), root: &Nbt) {
    let path = region_path(world, p);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = crate::region::RegionFile::open_or_create(&path).unwrap();
    f.write(crate::region::local_index(p.0, p.1), p.0, p.1, &crate::nbtio::encode_root(root)).unwrap();
}

fn read_raw(world: &Path, p: (i32, i32)) -> Option<Vec<u8>> {
    let mut f = crate::region::RegionFile::open(&region_path(world, p), false).ok()?;
    f.read(crate::region::local_index(p.0, p.1), p.0, p.1).ok().flatten()
}

/// Outputs of the generation for `targets`, flushed (every proto-chunk it would write included).
fn generate(w: &mut super::Worker, rx: &crossbeam_channel::Receiver<Out>, targets: &[(i32, i32)]) -> Vec<Out> {
    w.process(targets);
    w.flush();
    rx.try_iter().collect()
}

fn made(outs: &[Out]) -> Vec<(i32, i32)> {
    outs.iter().filter_map(|o| if let Out::Full(c, _) = o { Some((c.x, c.z)) } else { None }).collect()
}

fn written(outs: &[Out]) -> Vec<(i32, i32)> {
    outs.iter().filter_map(|o| if let Out::Proto(p, _) = o { Some(*p) } else { None }).collect()
}

/// A finished 1.18-style chunk and a proto-chunk keeping everything under the old `Level`
/// compound (before 1.18), both older than the conversion covers, next to the chunk asked for.
/// Neither is generated over nor written: the chunks asked for whose generation reaches them are
/// refused, a distant one is still made. The old code read both as absent, made them again and
/// wrote the proto-chunk over the player's.
#[test]
fn chunks_too_old_to_convert_are_never_generated_over() {
    let (data, template, biomes) = template(41).expect("26.3 data missing (rc-data cache and client jar)");
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-gen-old-chunks-{}", std::process::id())));
    let (mut w, rx) = worker_in(&data, &template, &biomes, &dir);
    let old_full = Nbt::Compound(vec![
        ("DataVersion".into(), Nbt::Int(3700)),
        ("xPos".into(), Nbt::Int(1)),
        ("zPos".into(), Nbt::Int(0)),
        ("Status".into(), Nbt::String("minecraft:full".into())),
        ("sections".into(), Nbt::List(Vec::new())),
    ]);
    let old_proto = Nbt::Compound(vec![
        ("DataVersion".into(), Nbt::Int(2586)),
        (
            "Level".into(),
            Nbt::Compound(vec![
                ("xPos".into(), Nbt::Int(2)),
                ("zPos".into(), Nbt::Int(0)),
                ("Status".into(), Nbt::String("features".into())),
            ]),
        ),
    ]);
    write_raw(&dir, (1, 0), &old_full);
    write_raw(&dir, (2, 0), &old_proto);
    let before = (read_raw(&dir, (1, 0)), read_raw(&dir, (2, 0)));
    let outs = generate(&mut w, &rx, &[(0, 0), (4, 0), (30, 0)]);
    let refused: Vec<(i32, i32)> = outs.iter().filter_map(|o| if let Out::Refused(p) = o { Some(*p) } else { None }).collect();
    // (4, 0) itself is clear, but its neighbour (3, 0) still to decorate touches the old proto-chunk.
    assert_eq!(refused, [(0, 0), (4, 0)], "the chunks whose generation reaches the old ones are refused");
    assert_eq!(made(&outs), [(30, 0)], "only the distant chunk is made");
    let near_old = |p: &(i32, i32)| (-1..=3).contains(&p.0) && (-2..=2).contains(&p.1);
    assert!(!written(&outs).iter().any(near_old), "a chunk around the old ones written: {:?}", written(&outs));
    assert_eq!((read_raw(&dir, (1, 0)), read_raw(&dir, (2, 0))), before, "old chunks changed on the disk");
    drop(w);
    let _ = std::fs::remove_dir_all(&template);
}

/// A finished chunk of 26.2 (data version 4950, block states still written `Name` and
/// `Properties`) next to the chunk asked for is converted and kept: never made again nor written
/// by the generation, its blocks read as they were. The old code read it as absent, made a new
/// terrain there and wrote it as a proto-chunk over the player's.
#[test]
fn convertible_old_chunk_is_kept_not_generated_again() {
    let (data, template, biomes) = template(42).expect("26.3 data missing (rc-data cache and client jar)");
    // A real finished chunk, made by the generation itself, then written back as 26.2 would.
    let source = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-gen-old-source-{}", std::process::id())));
    let (mut w, rx) = worker_in(&data, &template, &biomes, &source);
    let outs = generate(&mut w, &rx, &[(1, 0)]);
    let full = outs
        .into_iter()
        .find_map(|o| if let Out::Full(c, _) = o { Some(c) } else { None })
        .expect("chunk (1, 0) made");
    let mut root = full.encode(&data, w.states(), crate::dimension_height(&data, DIM), 0).expect("encoding");
    let gold = w.states().state_of(&data, &Nbt::String("minecraft:gold_block".into())).expect("gold block");
    drop(w);
    crate::nbtio::set(&mut root, "DataVersion", Nbt::Int(4950));
    // The player's own blocks: every solid block turned to gold, which a regenerated chunk lacks.
    let swapped = age_palette(&mut root, "minecraft:gold_block");
    assert!(swapped > 0, "no block to swap in the chunk");
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-gen-old-kept-{}", std::process::id())));
    let (mut w, rx) = worker_in(&data, &template, &biomes, &dir);
    write_raw(&dir, (1, 0), &root);
    let outs = generate(&mut w, &rx, &[(0, 0)]);
    assert_eq!(made(&outs), [(0, 0)]);
    assert!(!written(&outs).contains(&(1, 0)), "the old finished chunk was made again and written");
    assert!(w.chunk((1, 0)).is_some_and(|c| c.blocks.contains(&gold)), "old chunk's blocks not kept");
    drop(w);
    let _ = std::fs::remove_dir_all(&template);
}

/// The chunk's palettes written as before 5006 (a block state compound names its block `Name`),
/// every block but air replaced by `to`; returns how many entries were replaced.
fn age_palette(root: &mut Nbt, to: &str) -> usize {
    let mut swapped = 0;
    let Some(Nbt::List(sections)) = crate::nbtio::get_mut(root, "sections") else { return 0 };
    for section in sections {
        let Some(Nbt::List(palette)) =
            crate::nbtio::get_mut(section, "block_states").and_then(|b| crate::nbtio::get_mut(b, "palette"))
        else {
            continue;
        };
        for entry in palette.iter_mut() {
            // A state without properties may be written as its bare name (wrapped or not).
            let name = match &mut *entry {
                Nbt::Compound(fields) => fields.iter_mut().find(|(k, _)| k.is_empty() || k == "id").map(|(_, v)| v),
                v => Some(v),
            };
            if let Some(name) = name
                && name.as_str().is_some_and(|n| n != "minecraft:air")
            {
                *name = Nbt::String(to.into());
                swapped += 1;
            }
            if let Nbt::Compound(fields) = entry {
                fields.retain(|(k, _)| k != "properties");
                for (k, _) in fields.iter_mut().filter(|(k, _)| k == "id") {
                    *k = "Name".into();
                }
            }
        }
    }
    swapped
}
