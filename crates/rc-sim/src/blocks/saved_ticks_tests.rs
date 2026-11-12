//! Scheduled ticks read back from a chunk like the game: sub-orders -n..-1 in the file's order
//! (before every tick scheduled since the server started), the entries the game refuses left
//! out, and the save writing them back in that order.

use rc_proto::nbt::Nbt;

use super::{FLUID_NAMES, Tick};

fn saved(name: &str, pos: [i32; 3], delay: Option<i32>, priority: i32) -> Nbt {
    let mut fields = vec![
        ("i".into(), Nbt::String(name.into())),
        ("x".into(), Nbt::Int(pos[0])),
        ("y".into(), Nbt::Int(pos[1])),
        ("z".into(), Nbt::Int(pos[2])),
        ("p".into(), Nbt::Int(priority)),
    ];
    if let Some(t) = delay {
        fields.push(("t".into(), Nbt::Int(t)));
    }
    Nbt::Compound(fields)
}

/// (relative delay, position, priority) of each saved entry of `list`, in the file's order.
fn written(list: Option<&Nbt>) -> Vec<(i64, [i32; 3], i64)> {
    let Some(Nbt::List(l)) = list else { panic!("no tick list") };
    let int = |t: &Nbt, k: &str| crate::nbtio::int(t, k).expect("field");
    l.iter().map(|t| (int(t, "t"), [int(t, "x"), int(t, "y"), int(t, "z")].map(|v| v as i32), int(t, "p"))).collect()
}

#[test]
fn reloaded_ticks_keep_their_file_order_before_newer_ticks() {
    let (mut p, dir) = crate::tests_g3f13::open("saved-ticks");
    let at = (30, 0);
    let pos = |dx: i32| [480 + dx, -61, 3];
    let mut chunk = p.s.chunks.get(&(9, 0)).cloned().flatten().expect("chunk (9, 0)");
    (chunk.x, chunk.z) = at;
    let blocks = vec![
        saved("minecraft:repeater", pos(0), Some(5), 0),
        saved("minecraft:observer", pos(1), Some(2), 0),
        saved("minecraft:repeater", [0, -61, 0], Some(1), 0), // another chunk's position
        saved("minecraft:not_a_block", pos(2), Some(1), 0),
        saved("minecraft:observer", pos(3), Some(2), 9), // priority brought to 3
        saved("minecraft:observer", pos(4), None, 0),    // no delay
    ];
    crate::nbtio::set(&mut chunk.root, "block_ticks", Nbt::List(blocks));
    crate::nbtio::set(&mut chunk.root, "fluid_ticks", Nbt::List(vec![saved("minecraft:water", pos(5), Some(1), 0)]));
    p.s.chunks.insert(at, Some(chunk));
    // A tick scheduled since the server started, due with the observer at the same priority.
    let observer = p.s.bs.t.block_id("minecraft:observer");
    p.s.schedule_tick(pos(6), observer, 2);
    p.s.import_chunk_ticks(at);
    let now = p.s.time;
    let mut ticks = p.s.bs.block_ticks.chunk_ticks(at);
    let newer = ticks.iter().position(|t| t.pos == pos(6)).map(|i| ticks.remove(i)).expect("newer tick");
    let read: Vec<(i64, [i32; 3], i8, i64)> = ticks.iter().map(|t: &Tick| (t.at - now, t.pos, t.priority, t.sub)).collect();
    assert_eq!(read, [(5, pos(0), 0, -3), (2, pos(1), 0, -2), (2, pos(3), 3, -1)]);
    assert!(newer.sub >= 0, "ticks scheduled by the server count up from 0");
    let water = FLUID_NAMES.iter().position(|n| *n == "minecraft:water").unwrap() as u32;
    let fluids: Vec<(i64, u32, i64)> = p.s.bs.fluid_ticks.chunk_ticks(at).iter().map(|t| (t.at - now, t.kind, t.sub)).collect();
    assert_eq!(fluids, [(1, water, -1)], "each queue counts from -n");
    assert!(!p.s.bs.block_ticks.has([0, -61, 0], u32::from(p.s.bs.t.block_id("minecraft:repeater"))), "another chunk's tick read");
    // Saved back in sub-order (the file's order, then the newer tick), not by due tick.
    p.s.export_chunk_ticks(at);
    let root = &p.s.chunks.get(&at).cloned().flatten().expect("chunk").root;
    assert_eq!(written(root.get("block_ticks")), [(5, pos(0), 0), (2, pos(1), 0), (2, pos(3), 3), (2, pos(6), 0)]);
    crate::tests_g3f13::finish(p, dir);
}

/// Micro-bench: 100 chunks read back with 64 block ticks and 32 fluid ticks each (chunk load), then
/// their export (save); median of 31 runs. Measurement, not proof: run alone.
#[test]
#[ignore = "measurement bench"]
fn bench_saved_ticks_read_and_write() {
    let (mut p, dir) = crate::tests_g3f13::open("bench-saved-ticks");
    let template = p.s.chunks.get(&(9, 0)).cloned().flatten().expect("chunk (9, 0)");
    let chunks: Vec<(i32, i32)> = (0..100).map(|i| (100 + i % 10, 20 + i / 10)).collect();
    for &(cx, cz) in &chunks {
        let mut c = template.clone();
        (c.x, c.z) = (cx, cz);
        let at = |k: i32| [cx * 16 + k % 16, -60 + k / 16, cz * 16 + 5];
        let blocks = (0..64).map(|k| saved("minecraft:repeater", at(k), Some(k % 7), 0)).collect();
        let fluids = (0..32).map(|k| saved("minecraft:water", at(k), Some(k % 5), 0)).collect();
        crate::nbtio::set(&mut c.root, "block_ticks", Nbt::List(blocks));
        crate::nbtio::set(&mut c.root, "fluid_ticks", Nbt::List(fluids));
        p.s.chunks.insert((cx, cz), Some(c));
    }
    let median = |mut t: Vec<f64>| {
        t.sort_by(f64::total_cmp);
        t[t.len() / 2]
    };
    let (mut read, mut write) = (Vec::new(), Vec::new());
    for _ in 0..31 {
        p.s.bs.block_ticks = Default::default();
        p.s.bs.fluid_ticks = Default::default();
        p.s.bs.imported.clear();
        let t = std::time::Instant::now();
        for &ch in &chunks {
            p.s.import_chunk_ticks(ch);
        }
        read.push(t.elapsed().as_secs_f64() * 1e6);
        let t = std::time::Instant::now();
        for &ch in &chunks {
            p.s.export_chunk_ticks(ch);
        }
        write.push(t.elapsed().as_secs_f64() * 1e6);
    }
    assert_eq!(p.s.bs.block_ticks.chunk_ticks(chunks[0]).len(), 64);
    eprintln!("bench_saved_ticks 100 chunks: read {:.1} µs, write {:.1} µs", median(read), median(write));
    // The save's sort alone, the game's order against the former one (by due tick), taken in
    // turns on the same 64 ticks in heap order so the machine's load hits both alike.
    let heap_order: Vec<Tick> = p.s.bs.block_ticks.chunk_ticks(chunks[0]).into_iter().rev().collect();
    let (mut by_sub, mut by_due) = (Vec::new(), Vec::new());
    for _ in 0..201 {
        let t = std::time::Instant::now();
        for _ in 0..100 {
            let mut v = heap_order.clone();
            v.sort_unstable_by(|a, b| a.sub.cmp(&b.sub).then_with(|| a.cmp(b)));
            std::hint::black_box(&v);
        }
        by_sub.push(t.elapsed().as_secs_f64() * 1e6);
        let t = std::time::Instant::now();
        for _ in 0..100 {
            let mut v = heap_order.clone();
            v.sort_unstable();
            std::hint::black_box(&v);
        }
        by_due.push(t.elapsed().as_secs_f64() * 1e6);
    }
    eprintln!("bench_saved_ticks sort of 100 × 64 ticks: by sub-order {:.1} µs, by due tick {:.1} µs", median(by_sub), median(by_due));
    crate::tests_g3f13::finish(p, dir);
}
