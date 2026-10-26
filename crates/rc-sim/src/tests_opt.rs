//! Equivalence of the integrated server's optimizations: a world simulated N ticks in the normal
//! world then in the Nether, all its saveable state (chunks, entities, points of
//! interest, player, weather, random sequences, rules…) reduced to a SHA-256 fingerprint,
//! COMPARED to an expected fingerprint written here. An optimization must never change it
//! (same game code, same result down to the bit); an intended behavior fix changes it
//! and replaces the constant (the failure message gives the new value).
//!
//! Two worlds:
//! - a NEW world created by the test (fixed seed, all generation awaited before the first
//!   tick: nothing depends on the clock), always available as soon as the game data is;
//! - the model world of the bench `tools/bench/scenes.py` (`RUSTCRAFT_OPT_WORLD`, otherwise
//!   `target/perf/model` of the repository), recognized by the fingerprint of its files.
//!
//! Without game data, the tests skip like the others; with `RUSTCRAFT_OPT_REQUIRE=1`
//! (integration bench), missing data or an unknown model is an explicit FAILURE.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rc_testdata::{need, required};

use crate::save::Job;
use crate::server::Server;

/// Seed of the new world.
pub(crate) const SEED: i64 = 0x0B7_1C0DE;
/// Expected fingerprint of the new world: (total, number of files). New world, mechanisms and
/// regions re-recorded at the final integration of series 2 (fidelity batch 1: saves and
/// behaviors fixed; series 2: raids.dat and the dragon fight in the fingerprint), identical
/// values with all optimization switches at 0. G5-F03: initial fire of the mobs
/// at 0 (not −1) in the three fingerprints, the only cause of the change (checked by restoring it).
const EXPECTED_NEW: (&str, usize) = ("58ef8be0fcc17b8e20c942cf052a4560ac42ccc3a894072f72453b78e0f668d0", 815);
/// Known bench model worlds: (fingerprint of the input files, expected fingerprint, files).
const EXPECTED_MODEL: &[(&str, &str, usize)] = &[(
    "790b2d94653a4243e68f2e0cb0ebc3f865f9f59815ed65149067cdd374c6b800",
    "50fe98dff2e103f959adb0f342b5143554818edf090ff30401b221e2ae3be88c",
    724,
)];

fn ticks(s: &mut Server, n: usize) {
    for _ in 0..n {
        s.game_tick();
        s.tick += 1;
    }
}

fn around(s: &Server, r: i32) -> Vec<(i32, i32)> {
    let (cx, cz) = ((s.pos[0].floor() as i32) >> 4, (s.pos[2].floor() as i32) >> 4);
    (cx - r..=cx + r).flat_map(|x| (cz - r..=cz + r).map(move |z| (x, z))).collect()
}

/// Fingerprint of everything a save would write (excluding `level.dat`, which carries the real time).
fn digest(s: &mut Server, out: &mut Vec<(String, String)>) {
    for c in s.chunks.values_mut().flatten() {
        c.dirty = true;
    }
    let mut jobs = Vec::new();
    s.life_save_jobs(&mut jobs);
    s.commands_save_jobs(&mut jobs);
    s.drops_save_jobs(&mut jobs);
    s.poi_save_jobs(&mut jobs);
    s.chunk_jobs(&mut jobs);
    // raids.dat and the dragon fight (active dimension), written even without change.
    s.mobs.ill.raids.dirty = true;
    if let Some(f) = s.mobs.end.fight.as_deref_mut() {
        f.dirty = true;
    }
    s.raids_save_jobs(&mut jobs);
    s.end_save_jobs(&mut jobs);
    jobs.push(Job::Dat { path: "player".into(), nbt: crate::nbtio::encode_root(&s.player_nbt()), backup: false });
    let dim = s.dim.name.clone();
    for j in &jobs {
        let name = |p: &Path| {
            let parent = p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned());
            format!("{dim} {}/{}", parent.unwrap_or_default(), p.file_name().unwrap_or_default().to_string_lossy())
        };
        let sha = |b: &[u8]| hex(ring::digest::digest(&ring::digest::SHA256, b).as_ref());
        match j {
            Job::Chunk { path, cx, cz, nbt } => out.push((format!("{} {cx} {cz}", name(path)), sha(nbt))),
            Job::Dat { path, nbt, .. } => out.push((name(path), sha(nbt))),
            Job::Raw { path, bytes } => out.push((name(path), sha(bytes))),
            Job::Sync | Job::Done(_) | Job::Ack(..) => {}
        }
    }
}

fn total(parts: &[(String, String)]) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    for (k, v) in parts {
        ctx.update(k.as_bytes());
        ctx.update(v.as_bytes());
    }
    hex(ctx.finish().as_ref())
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Places the player at `pos` in dimension `dim`, neighboring chunks loaded (generated if
/// needed, generation fully awaited). `y = None`: placed on the surface.
///
/// New world: the chunks are requested ONE BY ONE in a fixed order. The generator returns the
/// chunks in the order its threads finish them, and their reception (generation entities,
/// ids drawn from the level's random source) follows this arrival order: in a batch, two runs
/// gave different entities. The bench world, already generated, keeps the batch loading
/// of wave 1 (the loading order changes the fingerprint: it would no longer be comparable).
fn go(s: &mut Server, dim: &str, x: f64, y: Option<f64>, z: f64, one_by_one: bool) {
    s.enter_world(dim);
    s.pos = [x, s.pos[1], z];
    let list = around(s, 6);
    if one_by_one {
        for p in &list {
            s.load_now(std::slice::from_ref(p));
        }
    }
    s.load_now(&list);
    assert!(
        s.generator.as_ref().is_none_or(|g| list.iter().all(|p| !g.pending.contains(p))),
        "generation not finished around ({x}, {z}) in {dim}"
    );
    let y = y.unwrap_or_else(|| f64::from(s.surface_y(x.floor() as i32, z.floor() as i32, 4)));
    s.pos = [x, y, z];
    s.loaded = true;
    s.teleport = None;
}

/// Rules of the reference simulation: spawns, time and weather running.
fn setup(s: &mut Server) {
    for r in ["minecraft:spawn_mobs", "minecraft:advance_time", "minecraft:advance_weather"] {
        crate::nbtio::set(&mut s.rules, r, rc_proto::nbt::Nbt::Byte(1));
    }
    s.sim_dist = 4;
    s.sim_req.store(4, std::sync::atomic::Ordering::Relaxed);
}

/// Folder deleted at the end (even if the test panics).
pub(crate) struct Scratch(pub(crate) PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// New world (fixed seed): two sites of the normal world then the Nether.
fn run_new() -> Option<Vec<(String, String)>> {
    let data = Arc::new(need(rc_testdata::version_data(), "26.3 data cache")?);
    need(crate::registries::client_jar(&data), "jar client 26.3")?;
    let dir = std::env::temp_dir().join(format!("rc-sim-optnew-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _guard = Scratch(dir.clone());
    let mut w = crate::newworld::NewWorld::new("opt");
    w.seed = SEED;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut s = crate::portal_tests::open_world(&dir, data);
    setup(&mut s);
    let mut out = Vec::new();
    go(&mut s, crate::dims::OVERWORLD, 8.5, None, 8.5, true);
    ticks(&mut s, 400);
    go(&mut s, crate::dims::OVERWORLD, 216.5, None, -87.5, true);
    ticks(&mut s, 200);
    digest(&mut s, &mut out);
    go(&mut s, crate::dims::NETHER, 8.5, Some(64.0), 8.5, true);
    ticks(&mut s, 300);
    digest(&mut s, &mut out);
    s.stop_gen();
    s.stop_parked();
    drop(s);
    Some(out)
}

/// Expected fingerprint of the "mechanisms" site: (total, number of files).
const EXPECTED_MECH: (&str, usize) = ("23967b310740166c63e1740d05c3239fc71acfc44ea9b8a55e4fa5676053f563", 380);

/// Server command (refusal = failure: the rig would be incomplete).
pub(crate) fn cmd(s: &mut Server, c: &str) {
    let src = s.server_source();
    let (ok, _) = s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

/// Fingerprint of `digest` with each name prefixed (several readouts in one dimension).
fn digest_as(s: &mut Server, tag: &str, out: &mut Vec<(String, String)>) {
    let mut v = Vec::new();
    digest(s, &mut v);
    out.extend(v.into_iter().map(|(k, h)| (format!("{tag} {k}"), h)));
}

/// "Mechanisms" site rig, placed at `y` on a 32 × 32 platform (chunks 0..1):
/// observer clocks, sticky pistons and slime blocks, long wire with repeaters of various
/// delays, subtractor comparator reading a full chest, water source falling from a pillar,
/// lava reached by the water, hoppers read by a comparator that controls a dropper.
fn mechanisms(y: i32) -> Vec<String> {
    let w = |dirs: &str| format!("minecraft:redstone_wire[{dirs}]");
    let ew = w("east=side,west=side");
    let ns = w("north=side,south=side");
    let mut c = vec![format!("fill 0 {} 0 31 {} 31 minecraft:stone", y - 1, y - 1)];
    let mut set = |x: i32, dy: i32, z: i32, b: &str| c.push(format!("setblock {x} {} {z} {b}", y + dy));
    // (1) Observer clock: sticky piston (slime and planks stuck) on one side, simple
    // piston on the other.
    set(1, 0, 2, "minecraft:sticky_piston[facing=up]");
    set(1, 1, 2, "minecraft:slime_block");
    set(1, 1, 3, "minecraft:slime_block");
    set(1, 2, 3, "minecraft:oak_planks");
    set(5, 0, 2, "minecraft:oak_planks");
    set(4, 0, 2, "minecraft:piston[facing=east]");
    set(2, 0, 2, "minecraft:observer[facing=east]");
    set(3, 0, 2, "minecraft:observer[facing=west]");
    // (2) Long wire to the west, repeaters of delays 1, 2 and 4 (input to the east), lamp at the end.
    set(3, 0, 10, "minecraft:redstone_lamp");
    for x in 4..=19 {
        let b = match x {
            16 => "minecraft:repeater[facing=east,delay=1]".to_owned(),
            12 => "minecraft:repeater[facing=east,delay=2]".to_owned(),
            8 => "minecraft:repeater[facing=east,delay=4]".to_owned(),
            _ => ew.clone(),
        };
        set(x, 0, 10, &b);
    }
    set(20, 0, 10, "minecraft:observer[facing=east]");
    set(21, 0, 10, "minecraft:observer[facing=west]");
    // (3) Subtractor comparator: full chest to the east, south side fed by a wire (power
    // decreasing) that a clock lights, output to a lamp to the west.
    let items: Vec<String> = (0..27)
        .map(|i| format!("{{Slot:{i}b,id:\"minecraft:{}\",count:64}}", if i % 3 == 0 { "dirt" } else { "stone" }))
        .collect();
    set(10, 0, 14, &format!("minecraft:chest[facing=west]{{Items:[{}]}}", items.join(",")));
    set(9, 0, 14, "minecraft:comparator[facing=east,mode=subtract]");
    set(8, 0, 14, &ew);
    set(7, 0, 14, &ew);
    set(6, 0, 14, "minecraft:redstone_lamp");
    for z in 15..=18 {
        set(9, 0, z, &ns);
    }
    set(9, 0, 19, "minecraft:observer[facing=south]");
    set(9, 0, 20, "minecraft:observer[facing=north]");
    // (4) Fluids: water falling from a pillar, lava placed in the zone the water reaches.
    set(26, 0, 26, "minecraft:stone");
    set(26, 1, 26, "minecraft:stone");
    set(26, 2, 26, "minecraft:water");
    set(21, 0, 25, "minecraft:lava");
    // (5) Hoppers: chest → hopper → hopper → chest; a comparator reads the second and
    // controls a dropper that empties its items into a hopper placed on a chest.
    let few: Vec<String> = (0..5).map(|i| format!("{{Slot:{i}b,id:\"minecraft:bone\",count:{}}}", 3 + i)).collect();
    set(4, 1, 26, &format!("minecraft:chest{{Items:[{}]}}", items[..6].join(",")));
    set(4, 0, 26, "minecraft:hopper[facing=east]");
    set(5, -1, 26, "minecraft:chest");
    set(5, 0, 26, "minecraft:hopper[facing=down]");
    set(6, 0, 26, "minecraft:comparator[facing=west]");
    set(7, 0, 26, &ew);
    set(8, -1, 25, "minecraft:chest");
    set(8, 0, 25, "minecraft:hopper[facing=down]");
    set(8, 0, 26, &format!("minecraft:dropper[facing=north]{{Items:[{}]}}", few.join(",")));
    c
}

/// "Mechanisms" site of the new world: rig placed, 401 ticks, fingerprint taken at ticks 397
/// and 401 (coprime, outside the phase of the 2- and 4-tick clocks).
fn run_mech() -> Option<Vec<(String, String)>> {
    let data = Arc::new(need(rc_testdata::version_data(), "26.3 data cache")?);
    need(crate::registries::client_jar(&data), "jar client 26.3")?;
    let dir =
        std::env::temp_dir().join(format!("rc-sim-optmech-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _guard = Scratch(dir.clone());
    let mut w = crate::newworld::NewWorld::new("opt");
    w.seed = SEED;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut s = crate::portal_tests::open_world(&dir, data);
    setup(&mut s);
    go(&mut s, crate::dims::OVERWORLD, 16.5, Some(201.0), 16.5, true);
    for c in mechanisms(200) {
        cmd(&mut s, &c);
    }
    // The rig must be alive (otherwise a sabotage of the mechanisms code would go unnoticed):
    // ticks where each indicator is active, counted during the simulation.
    let probes: [([i32; 3], &str); 5] = [
        ([1, 200, 2], "extended"), // sticky piston of the clock
        ([4, 200, 2], "extended"), // piston simple
        ([3, 200, 10], "lit"),     // lamp at the end of the repeater wire
        ([6, 200, 14], "lit"),     // subtractor comparator lamp
        ([7, 200, 26], "power"),   // wire controlled by the hopper comparator
    ];
    let mut on = [0u32; 5];
    let mut out = Vec::new();
    for t in 1..=401 {
        s.game_tick();
        s.tick += 1;
        for (n, (p, prop)) in on.iter_mut().zip(probes) {
            let st = s.state_at(p).unwrap_or(0);
            let v = s.v();
            *n += u32::from(if prop == "power" { v.get_int(st, prop) > 0 } else { v.get_bool(st, prop) });
        }
        if t == 397 {
            digest_as(&mut s, "t397", &mut out);
        }
    }
    digest_as(&mut s, "t401", &mut out);
    let rock = (19..=31)
        .flat_map(|x| (19..=31).map(move |z| [x, 200, z]))
        .filter(|&p| p != [26, 200, 26])
        .filter(|&p| matches!(s.bname(p), "minecraft:obsidian" | "minecraft:cobblestone" | "minecraft:stone"))
        .count();
    eprintln!("mechanisms: active indicators {on:?} ticks out of 401, rock formed by lava and water {rock}");
    assert!(on.iter().all(|&n| n > 0 && n < 401), "rig inert or stuck: {on:?}");
    assert!(rock > 0, "neither obsidian nor stone: the water did not reach the lava");
    s.stop_gen();
    s.stop_parked();
    drop(s);
    Some(out)
}

/// Expected fingerprint of all the regions after closing: (total, number of chunks).
const EXPECTED_REGIONS: (&str, usize) = ("180f852f7cea8f9b590faae0ee7c7a04f5e846b3f38779f244430e5c3c45ae15", 1249);

/// Content of all the regions of a world (chunks, entities, points of interest, all
/// dimensions): SHA-256 of the decompressed NBT per chunk. Sector locations and
/// timestamps ignored (only the content counts).
fn regions_digest(root: &Path) -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "mca") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(root, &mut files);
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let rel = f.strip_prefix(root).unwrap_or(&f).to_string_lossy().replace('\\', "/");
        let (rx, rz) = {
            let n: Vec<i32> = rel.rsplit('/').next().unwrap_or("").split('.').filter_map(|x| x.parse().ok()).collect();
            (n.first().copied().unwrap_or(0), n.get(1).copied().unwrap_or(0))
        };
        let mut r = crate::region::RegionFile::open(&f, false).expect("unreadable region");
        assert_eq!(r.dropped, 0, "{rel}: invalid slots");
        let present: Vec<usize> = r.present().collect();
        for i in present {
            let (cx, cz) = (rx * 32 + (i % 32) as i32, rz * 32 + (i / 32) as i32);
            let nbt = r.read(i, cx, cz).unwrap_or_else(|e| panic!("{rel} {cx} {cz} : {e}"));
            if let Some(b) = nbt {
                out.push((format!("{rel} {cx} {cz}"), hex(ring::digest::digest(&ring::digest::SHA256, &b).as_ref())));
            }
        }
    }
    out
}

/// New world: three generation batches in the normal world, a pass to the Nether, scheduled
/// ticks left pending, `/locate` (references placed on in-memory protos), then
/// closing like the game (generation stopped, save awaited) and rereading of ALL the
/// regions. `keep`: cap of the generator map (evictions and rereads); `delay_ms`:
/// latency of the writer. The result must depend on neither.
fn run_regions(keep: usize, delay_ms: u64, lazy: Option<bool>) -> Option<Vec<(String, String)>> {
    let data = Arc::new(need(rc_testdata::version_data(), "26.3 data cache")?);
    need(crate::registries::client_jar(&data), "jar client 26.3")?;
    crate::generation::TEST_KEEP.with(|k| k.set(keep));
    crate::generation::TEST_PROTO_LAZY.with(|k| k.set(lazy));
    crate::save::TEST_WRITER_DELAY_MS.with(|d| d.set(delay_ms));
    let tag = match lazy {
        None => String::new(),
        Some(l) => format!("-{}", u8::from(l)),
    };
    let dir = std::env::temp_dir().join(format!("rc-sim-optreg-{}-{keep}-{delay_ms}{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _guard = Scratch(dir.clone());
    let mut w = crate::newworld::NewWorld::new("opt");
    w.seed = SEED;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut s = crate::portal_tests::open_world(&dir, data);
    setup(&mut s);
    for (x, z) in [(8.5, 8.5), (216.5, -87.5), (-180.5, 260.5)] {
        go(&mut s, crate::dims::OVERWORLD, x, None, z, true);
        ticks(&mut s, 20);
    }
    // Ticks pending at closing: flowing water, powered repeater.
    let (x, y, z) = (s.pos[0].floor() as i32, s.pos[1].floor() as i32, s.pos[2].floor() as i32);
    cmd(&mut s, &format!("setblock {x} {} {z} minecraft:water", y + 3));
    cmd(&mut s, &format!("setblock {} {} {z} minecraft:redstone_block", x + 2, y + 3));
    cmd(&mut s, &format!("setblock {} {} {z} minecraft:repeater[facing=west,delay=4]", x + 3, y + 3));
    let src = s.server_source();
    let _ = s.perform(src, "locate structure #minecraft:village");
    go(&mut s, crate::dims::NETHER, 8.5, Some(64.0), 8.5, true);
    ticks(&mut s, 20);
    go(&mut s, crate::dims::OVERWORLD, x as f64 + 0.5, Some(y as f64), z as f64 + 0.5, true);
    cmd(&mut s, &format!("setblock {} {} {z} minecraft:water", x + 6, y + 3));
    // Batch still in flight at closing (the generation thread finishes its queue before stopping):
    // its chunks and its protos must be picked up and written.
    if let Some(g) = &mut s.generator {
        g.want((40..43).flat_map(|x| (40..43).map(move |z| (x, z))).collect());
    }
    s.stop_gen();
    s.stop_parked();
    s.save(true);
    drop(s);
    Some(regions_digest(&dir))
}

fn check_regions(keep: usize, delay_ms: u64) {
    check_regions_with(keep, delay_ms, None);
}

/// `lazy`: lazy write of proto-chunks imposed (otherwise `RUSTCRAFT_OPT_PROTO_LAZY`).
fn check_regions_with(keep: usize, delay_ms: u64, lazy: Option<bool>) {
    let what = format!("all regions (map {keep}, writer +{delay_ms} ms, lazy protos {lazy:?})");
    let t = std::time::Instant::now();
    let Some(got) = std::thread::spawn(move || run_regions(keep, delay_ms, lazy)).join().expect("simulation") else {
        return;
    };
    let sum = total(&got);
    eprintln!("{what}: {sum} ({} chunks) in {:.1} s", got.len(), t.elapsed().as_secs_f64());
    let (want, n) = EXPECTED_REGIONS;
    assert!(!want.is_empty(), "{what}: no expected fingerprint; enter (\"{sum}\", {})", got.len());
    assert!(
        (sum.as_str(), got.len()) == (want, n),
        "{what}: regions {sum} ({} chunks) instead of {want} ({n}): closing did not write everything, \
         or the result depends on evictions or on write latency",
        got.len()
    );
}

#[test]
fn all_regions() {
    check_regions(900, 0);
    check_regions(900, 0); // two identical closings (determinism)
}

/// Generator map reduced to 16: the chunks evicted before their write is on
/// disk go through the generator's waiting table (like the game serves reads from
/// the pending writes); the result must be that of the 900 map.
#[test]
fn all_regions_reduced_map() {
    check_regions(16, 0);
}

#[test]
fn all_regions_slow_writer() {
    check_regions(900, 20);
}

#[test]
fn all_regions_reduced_map_slow_writer() {
    check_regions(16, 20);
}

/// The two paths of the proto-chunks (written at every batch, or on eviction and at save)
/// leave the same regions, full map and reduced map with a slow writer.
#[test]
fn all_regions_protos_per_batch_and_lazy() {
    for lazy in [false, true] {
        check_regions_with(900, 0, Some(lazy));
        check_regions_with(16, 20, Some(lazy));
    }
}

/// Save with the generator running: the proto-chunks it keeps in memory are on
/// disk as soon as the save ends (flush at the start of `save`), identical to those of a
/// full closing; without a flush, the protos not yet evicted would be missing.
#[test]
fn save_writes_the_generator_protos() {
    save_writes_protos(false, None);
}

/// By default (`RUSTCRAFT_OPT_PROTO_FLUSH_ASYNC` not set), the automatic save does not make
/// the server thread wait: no generator output picked up during `save`, the
/// proto-chunks arrive at the next pickup and end up on disk.
#[test]
fn save_writes_the_generator_protos_without_waiting() {
    // Fixed here: the "everything at 0" suite cuts the default through the environment.
    save_writes_protos(true, Some(true));
}

/// `RUSTCRAFT_OPT_PROTO_FLUSH_ASYNC=0`: the automatic save waits for the flush (outputs
/// picked up during `save`), same result on disk.
#[test]
fn save_writes_the_generator_protos_awaited() {
    save_writes_protos(true, Some(false));
}

fn save_writes_protos(autosave: bool, no_wait: Option<bool>) {
    let Some(got) = std::thread::spawn(move || {
        // Save flush = lazy path (`RUSTCRAFT_OPT_PROTO_LAZY`), exercised here
        // even when the environment cuts it ("everything at 0" suite: write at every batch, nothing
        // to flush).
        crate::generation::TEST_PROTO_LAZY.with(|k| k.set(Some(true)));
        crate::generation::TEST_PROTO_FLUSH_ASYNC.with(|k| k.set(no_wait));
        let data = Arc::new(need(rc_testdata::version_data(), "26.3 data cache")?);
        need(crate::registries::client_jar(&data), "jar client 26.3")?;
        let dir = std::env::temp_dir().join(format!("rc-sim-optsave-{}-{autosave}-{no_wait:?}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _guard = Scratch(dir.clone());
        let mut w = crate::newworld::NewWorld::new("opt");
        w.seed = SEED;
        crate::newworld::create_world(&dir, &data, &w).expect("world creation");
        let mut s = crate::portal_tests::open_world(&dir, data);
        setup(&mut s);
        go(&mut s, crate::dims::OVERWORLD, 8.5, None, 8.5, false);
        if autosave {
            // Automatic save: flush awaited (`Some(false)`) or only requested (default).
            let seen = s.generator.as_ref().expect("generator started").seen;
            s.save(false);
            let after_save = s.generator.as_ref().expect("generator started").seen;
            if no_wait == Some(false) {
                assert!(after_save > seen, "flush awaited: flush batch not picked up during the save");
            } else {
                assert_eq!(after_save, seen, "default: the automatic save waited for the generator");
            }
            let t = std::time::Instant::now();
            while s.generator.as_ref().is_some_and(|g| g.seen == seen) && t.elapsed().as_secs() < 30 {
                std::thread::sleep(std::time::Duration::from_millis(2));
                s.poll_gen();
            }
            if let Some(w) = s.writer.take() {
                w.finish();
            }
        } else {
            s.save(true);
        }
        let during = regions_digest(&dir);
        s.stop_gen();
        s.stop_parked();
        s.save(true);
        let after = regions_digest(&dir);
        drop(s);
        Some((during, after))
    })
    .join()
    .expect("simulation") else {
        return;
    };
    let (during, after) = got;
    let protos = after.len() - during.len().min(after.len());
    eprintln!("running save: {} chunks, closing: {} ({protos} more)", during.len(), after.len());
    assert!(during.len() > 100, "too few chunks written: {}", during.len());
    assert!(during == after, "the running save did not write the generator's proto-chunks ({protos} missing)");
}

/// Bench model world folder: `RUSTCRAFT_OPT_WORLD`, otherwise `target/perf/model`.
fn model_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RUSTCRAFT_OPT_WORLD") {
        return Some(PathBuf::from(p));
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/perf/model");
    p.join("level.dat").is_file().then_some(p)
}

/// Fingerprint of the files of a world (sorted relative paths + contents), lock excluded.
fn input_digest(root: &Path) -> String {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
        for e in std::fs::read_dir(dir).expect("unreadable model world").flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else if p.file_name().is_some_and(|n| n != "session.lock") {
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.push((rel, p));
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    for (rel, p) in &files {
        ctx.update(rel.as_bytes());
        ctx.update(&std::fs::read(p).expect("model file"));
    }
    hex(ctx.finish().as_ref())
}

/// Bench model world (already generated areas): plains and crowd site, then Nether.
fn run_model(model: &Path) -> Option<Vec<(String, String)>> {
    let data = Arc::new(need(rc_testdata::version_data(), "26.3 data cache")?);
    need(crate::registries::client_jar(&data), "jar client 26.3")?;
    let dir = crate::tests::fresh_copy(model, "opt");
    let mut s = crate::portal_tests::open_world(&dir, data);
    setup(&mut s);
    let mut out = Vec::new();
    go(&mut s, crate::dims::OVERWORLD, 1600.5, Some(76.0), -447.5, false);
    ticks(&mut s, 400);
    go(&mut s, crate::dims::OVERWORLD, 2240.5, Some(77.0), -469.5, false);
    ticks(&mut s, 200);
    digest(&mut s, &mut out);
    go(&mut s, crate::dims::NETHER, -7.5, Some(34.0), -23.5, false);
    ticks(&mut s, 300);
    digest(&mut s, &mut out);
    s.stop_gen();
    s.stop_parked();
    Some(out)
}

/// Two simulations, each in a fresh thread (the "clock" random sources follow the same
/// sequence there, `rng::test_seed` being per-thread): they must be identical to each other,
/// then to the expected fingerprint.
fn check(
    what: &str,
    run: impl Fn() -> Option<Vec<(String, String)>> + Send + Sync + Clone + 'static,
    expected: Option<(&str, usize)>,
) {
    let fresh = || {
        let r = run.clone();
        std::thread::spawn(r).join().expect("simulation")
    };
    let Some(a) = fresh() else { return };
    let Some(b) = fresh() else { return };
    let diff: Vec<String> = a
        .iter()
        .filter(|x| !b.contains(x))
        .map(|x| x.0.clone())
        .chain(b.iter().filter(|x| !a.iter().any(|y| y.0 == x.0)).map(|x| format!("{} (2nd only)", x.0)))
        .collect();
    assert!(diff.is_empty(), "{what}: non-deterministic simulation: {diff:?}");
    let got = total(&a);
    eprintln!("{what}: simulation fingerprint {got} ({} files)", a.len());
    match expected {
        Some((want, n)) if !want.is_empty() => assert!(
            (got.as_str(), a.len()) == (want, n),
            "{what}: fingerprint {got} ({} files) instead of {want} ({n}). An optimization must \
             never change it; if it is an INTENDED behavior fix, replace the \
             constant in tests_opt.rs with (\"{got}\", {}).",
            a.len(),
            a.len()
        ),
        _ => panic!("{what}: no expected fingerprint recorded; enter (\"{got}\", {}) in tests_opt.rs", a.len()),
    }
}

#[test]
fn fingerprint_new_world() {
    check("new world", run_new, Some(EXPECTED_NEW));
}

#[test]
fn fingerprint_mechanisms() {
    check("mechanisms", run_mech, Some(EXPECTED_MECH));
}

#[test]
fn fingerprint_bench_world() {
    let Some(model) = need(model_dir(), "bench model world (RUSTCRAFT_OPT_WORLD or target/perf/model)") else {
        return;
    };
    let input = input_digest(&model);
    let known = EXPECTED_MODEL.iter().find(|e| e.0 == input).map(|e| (e.1, e.2));
    if known.is_none() {
        assert!(!required(), "unknown model world (files {input}): enter its fingerprint in EXPECTED_MODEL");
        eprintln!("unknown model world (files {input}): determinism only checked");
    }
    let m = model.clone();
    let run = move || run_model(&m);
    match known {
        Some(k) => check("bench world", run, Some(k)),
        None => {
            // Model without reference: only the equality of two simulations is checked.
            let a = std::thread::spawn(run.clone()).join().expect("simulation");
            let b = std::thread::spawn(run).join().expect("simulation");
            if let (Some(a), Some(b)) = (a, b) {
                assert_eq!(total(&a), total(&b), "bench world: non-deterministic simulation");
                eprintln!("bench world: fingerprint {} ({} files, input files {input})", total(&a), a.len());
            }
        }
    }
}

/// Crowd bench (C6-M07, measurement): N persistent animals (cows, sheep, pigs, chickens, a
/// third of them babies) placed at the density of the "crowd" scene (100 in 24 × 24) on the plains of the
/// model world; 50 warm-up ticks then 200 measured. N = 100, 400, 1,600. `RUSTCRAFT_OPT_*` switches
/// read once: one process per variant.
#[test]
#[ignore = "measurement bench"]
fn bench_crowd() {
    use crate::mobs::species::Kind;
    let Some(model) = need(model_dir(), "bench model world") else { return };
    let Some(data) = need(rc_testdata::version_data(), "26.3 data cache") else { return };
    crate::mobs::index::set_verify(false);
    let data = Arc::new(data);
    if need(crate::registries::client_jar(&data), "jar client 26.3").is_none() {
        return;
    }
    for n in [100usize, 400, 1600] {
        let dir = crate::tests::fresh_copy(&model, &format!("optcrowd{n}"));
        let mut s = crate::portal_tests::open_world(&dir, data.clone());
        setup(&mut s);
        crate::nbtio::set(&mut s.rules, "minecraft:spawn_mobs", rc_proto::nbt::Nbt::Byte(0));
        go(&mut s, crate::dims::OVERWORLD, 1600.5, Some(76.0), -447.5, false);
        let side = 24.0 * (n as f64 / 100.0).sqrt();
        let kinds = [Kind::Cow, Kind::Sheep, Kind::Pig, Kind::Chicken];
        let mut r: u64 = 0x5EED;
        for i in 0..n {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            let (fx, fz) = ((r & 0xFFFF) as f64 / 65536.0, ((r >> 16) & 0xFFFF) as f64 / 65536.0);
            let (x, z) = (1600.5 - side / 2.0 + fx * side, -447.5 - side / 2.0 + fz * side);
            let y = f64::from(s.surface_y(x.floor() as i32, z.floor() as i32, 4));
            let mut m = s.new_mob(kinds[i % 4], [x, y, z], 0.0);
            m.persistent = true;
            if i % 3 == 0 {
                m.age = -24000;
            }
            s.add_mob(m);
        }
        ticks(&mut s, 50);
        let lookups = || {
            use crate::mobs::index::stats;
            use std::sync::atomic::Ordering::Relaxed;
            stats::HIT.load(Relaxed) + stats::FOUND.load(Relaxed) + stats::ABSENT.load(Relaxed)
        };
        let l0 = lookups();
        let mut d: Vec<f64> = (0..200)
            .map(|_| {
                let t = std::time::Instant::now();
                s.game_tick();
                s.tick += 1;
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        let sum: f64 = d.iter().sum();
        d.sort_by(f64::total_cmp);
        eprintln!(
            "bench_crowd N={n} ({} mobs): median {:.3} ms, p99 {:.3} ms, mean {:.3} ms",
            s.mobs.list.len(),
            d[d.len() / 2],
            d[d.len() * 99 / 100],
            sum / d.len() as f64
        );
        // Gain bound of a "one lookup instead of 3-4" view: reads by id
        // per tick × measured cost of a read in this world (the gain cannot exceed that).
        let per_tick = (lookups() - l0) as f64 / d.len() as f64;
        let ids: Vec<i32> = s.mobs.list.iter().flatten().map(|m| m.id).collect();
        let reps = 1_000_000 / ids.len().max(1) + 1;
        let t = std::time::Instant::now();
        let mut acc = 0.0;
        for _ in 0..reps {
            for &id in &ids {
                acc += std::hint::black_box(&s).mob_ref(std::hint::black_box(id)).map_or(0.0, |m| m.pos[0]);
            }
        }
        let ns = t.elapsed().as_secs_f64() * 1e9 / (reps * ids.len()) as f64;
        std::hint::black_box(acc);
        eprintln!(
            "bench_crowd N={n}: {per_tick:.0} reads by id per tick, {ns:.1} ns per read, \
             single-view gain bound {:.2} µs per tick",
            per_tick * ns * 1e-3
        );
        s.stop_gen();
        s.stop_parked();
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Bench of the integrated server's tick on the model world (not a proof: a measurement). Five sites
/// of the scene bench, chunks at 8 around, simulation at 6, spawns, time and weather
/// running; 100 warm-up ticks then 400 measured per site. Run alone, quiet machine:
/// `cargo test --release -p rc-sim bench_ticks -- --ignored --nocapture`; `RAYON_NUM_THREADS`
/// sets the light threads (1 = one core).
#[test]
#[ignore = "measurement bench"]
fn bench_ticks() {
    let Some(model) = need(model_dir(), "bench model world") else { return };
    let Some(data) = need(rc_testdata::version_data(), "26.3 data cache") else { return };
    // Measurement: no comparison with the sweep at every lookup (proof of the other tests).
    crate::mobs::index::set_verify(false);
    let data = Arc::new(data);
    if need(crate::registries::client_jar(&data), "jar client 26.3").is_none() {
        return;
    }
    let dir = crate::tests::fresh_copy(&model, "optbench");
    let mut s = crate::portal_tests::open_world(&dir, data);
    setup(&mut s);
    s.sim_dist = 6;
    s.sim_req.store(6, std::sync::atomic::Ordering::Relaxed);
    // "redstone" site: platform of the model world (`tools/bench/scenes.py`, 128 observer
    // clocks placed by the `perf:redstone` function of the model's pack).
    /// Name, dimension, position, rig command.
    type Site<'a> = (&'a str, &'a str, f64, f64, f64, Option<&'a str>);
    let sites: [Site; 6] = [
        ("plains", crate::dims::OVERWORLD, 1600.5, 76.0, -447.5, None),
        ("forest", crate::dims::OVERWORLD, 64.5, 75.0, 4128.5, None),
        ("village", crate::dims::OVERWORLD, -1407.5, 76.0, 348.5, None),
        ("ocean", crate::dims::OVERWORLD, -479.5, 72.0, -703.5, None),
        ("nether", crate::dims::NETHER, -7.5, 34.0, -23.5, None),
        ("redstone", crate::dims::OVERWORLD, 2240.5, 73.0, 166.5, Some("function perf:redstone")),
    ];
    let mut all = 0.0;
    // Light of the measured ticks only (warm-up excluded): figure without the scene bench's
    // dispersion, relights after changes and lit chunks.
    let mut light_all = rc_light::stats::Totals::default();
    let light_line = |name: &str, l: &rc_light::stats::Totals| {
        eprintln!(
            "bench_ticks {name} light: relights {} ({} changes) {:.3} ms; sky solves {} {:.3} ms, \
             block {} {:.3} ms; chunks {} {:.3} ms",
            l.relights,
            l.changes,
            l.relight_ns as f64 / 1e6,
            l.sky_solves,
            l.sky_ns as f64 / 1e6,
            l.block_solves,
            l.block_ns as f64 / 1e6,
            l.chunks,
            l.chunk_ns as f64 / 1e6
        );
    };
    for (name, dim, x, y, z, setup_cmd) in sites {
        s.enter_world(dim);
        s.pos = [x, y, z];
        let list = around(&s, 8);
        s.load_now(&list);
        s.loaded = true;
        s.teleport = None;
        if let Some(c) = setup_cmd {
            cmd(&mut s, c);
        }
        ticks(&mut s, 100);
        let blocks0 = crate::server::BLOCKS_NS.with(std::cell::Cell::get);
        let light0 = rc_light::stats::totals();
        let mut d: Vec<f64> = (0..400)
            .map(|_| {
                let t = std::time::Instant::now();
                s.game_tick();
                s.tick += 1;
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        let sum: f64 = d.iter().sum();
        all += sum;
        d.sort_by(f64::total_cmp);
        let blocks = (crate::server::BLOCKS_NS.with(std::cell::Cell::get) - blocks0) as f64 / 1e6;
        eprintln!(
            "bench_ticks {name}: median {:.3} ms, p99 {:.3} ms, max {:.3} ms, mean {:.3} ms (of which blocks and fluids {:.3} ms)",
            d[d.len() / 2],
            d[d.len() * 99 / 100],
            d[d.len() - 1],
            sum / d.len() as f64,
            blocks / d.len() as f64
        );
        let l = rc_light::stats::totals().since(&light0);
        light_line(name, &l);
        light_all = light_all.plus(&l);
    }
    eprintln!("bench_ticks total {all:.1} ms (2400 ticks), rayon threads {}", rayon::current_num_threads());
    light_line("total", &light_all);
    {
        use crate::mobs::index::stats;
        use std::sync::atomic::Ordering::Relaxed;
        eprintln!(
            "mob index: served {} , found by sweep {}, absent {}",
            stats::HIT.load(Relaxed),
            stats::FOUND.load(Relaxed),
            stats::ABSENT.load(Relaxed)
        );
    }
    s.stop_gen();
    s.stop_parked();
}

/// Sending of the saved chunks (`loader.rs`, I-02): sliced (`CHUNKSTREAM`) and read on
/// a separate thread (`ASYNC_LOAD`), compared to the old path on the bench's model world. Same
/// chunks sent and same content of each packet (SHA-256), same entities announced (UUID),
/// same positions adopted in the same order, same saveable state (`digest`); sites: plains,
/// teleport into the explored zone, then Nether (dimension change).
mod loading {
    use super::*;
    use crate::loader::{ADOPTED, SABOTAGE, TEST_LATE, TEST_NEAR, TEST_OPT, TEST_SPLIT};
    use hashbrown::HashSet;
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    /// What a client and a save would see of a variant.
    #[derive(PartialEq, Debug)]
    struct Seen {
        /// Per site: chunk sent → SHA-256 of the packet.
        chunks: Vec<BTreeMap<(i32, i32), String>>,
        /// Per site: UUID of the announced entities.
        uuids: Vec<Vec<Announced>>,
        adopted: Vec<(i32, i32)>,
        state: String,
        /// UUID of the mobs adopted twice (must stay empty).
        twice: Vec<u128>,
    }

    /// Announced entity: UUID, type, position (x, z) rounded.
    type Announced = (u128, i32, i32, i32);
    type Site = (&'static str, f64, f64, f64);
    const SITES: [Site; 3] = [
        (crate::dims::OVERWORLD, 1600.5, 76.0, -447.5),
        (crate::dims::OVERWORLD, 1920.5, 76.0, -447.5),
        (crate::dims::NETHER, -7.5, 34.0, -23.5),
    ];

    fn open(dir: &Path, data: Arc<rc_data::VersionData>) -> (Server, rc_client::Session) {
        crate::tests::solo_slot();
        let uuid = rc_client::offline_uuid("AgentI02");
        let (session, peer) = rc_client::memory::session(data.clone(), uuid).expect("session");
        let opts = crate::Options {
            world: dir.to_owned(),
            name: "AgentI02".into(),
            uuid,
            info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
            sim_distance: 12,
        };
        match Server::open(opts, uuid, data, peer, Arc::new(std::sync::atomic::AtomicBool::new(false))) {
            Ok(s) => (s, session),
            Err((_, e)) => panic!("ouverture : {e}"),
        }
    }

    /// "Chunks" part of a server tick (`run`), repeated every `tick` until
    /// the queue is empty, or `limit` calls are made. `between` runs after each call.
    /// Returns the duration of each call (ms).
    fn pump(s: &mut Server, tick: Duration, limit: usize, mut between: impl FnMut(&mut Server)) -> Vec<f64> {
        let mut d = Vec::new();
        let t0 = Instant::now();
        loop {
            let at = Instant::now();
            s.poll_loader();
            s.update_tracking();
            s.poll_gen();
            if crate::loader::late_poll() {
                s.poll_loader();
            }
            s.send_chunks();
            d.push(at.elapsed().as_secs_f64() * 1e3);
            between(s);
            let waiting = s.loader.as_ref().is_some_and(|l| !l.pending.is_empty());
            if (s.queue.is_empty() && !waiting) || d.len() >= limit {
                return d;
            }
            assert!(t0.elapsed() < Duration::from_secs(120), "chunk queue never emptied ({} remaining)", s.queue.len());
            let next = at + tick;
            std::thread::sleep(next.saturating_duration_since(Instant::now()).max(Duration::from_micros(200)));
        }
    }

    fn place(s: &mut Server, (dim, x, y, z): Site) {
        if s.dim.name != dim {
            s.enter_world(dim);
            s.reset_view();
        }
        s.pos = [x, y, z];
        s.loaded = true;
        s.teleport = None;
    }

    fn collect(session: &rc_client::Session) -> (BTreeMap<(i32, i32), String>, Vec<Announced>) {
        let (mut chunks, mut uuids) = (BTreeMap::new(), Vec::new());
        for e in session.events.try_iter() {
            match e {
                rc_client::Event::Chunk(c) => {
                    let sha = hex(ring::digest::digest(&ring::digest::SHA256, format!("{c:?}").as_bytes()).as_ref());
                    assert!(chunks.insert((c.x, c.z), sha).is_none(), "chunk ({}, {}) sent twice", c.x, c.z);
                }
                rc_client::Event::Entity(rc_client::EntityEvent::Add(a)) => {
                    uuids.push((a.uuid, a.kind, a.pos[0] as i32, a.pos[2] as i32))
                }
                _ => {}
            }
        }
        uuids.sort_unstable();
        (chunks, uuids)
    }

    /// A variant (sliced sending, separate read) with a sabotage (0 none); `race`:
    /// synchronous read of part of the queue while the thread reads it.
    /// Each variant in a fresh thread: the thread's own random sources (`rng::test_seed`, UUID of
    /// the items placed at opening) follow the same sequence there.
    fn run(model: &Path, data: &Arc<rc_data::VersionData>, opt: (bool, bool), sabotage: u8, race: bool) -> Seen {
        run_with(model, data, opt, (true, true, true), sabotage, race)
    }

    /// `knobs`: (read slicing, second thread pickup, spawn square read right away).
    fn run_with(
        model: &Path,
        data: &Arc<rc_data::VersionData>,
        opt: (bool, bool),
        knobs: (bool, bool, bool),
        sabotage: u8,
        race: bool,
    ) -> Seen {
        let (model, data) = (model.to_owned(), data.clone());
        std::thread::spawn(move || {
            TEST_SPLIT.with(|c| c.set(Some(knobs.0)));
            TEST_LATE.with(|c| c.set(Some(knobs.1)));
            TEST_NEAR.with(|c| c.set(Some(knobs.2)));
            run_here(&model, &data, opt, sabotage, race)
        })
        .join()
        .expect("variant")
    }

    fn run_here(model: &Path, data: &Arc<rc_data::VersionData>, opt: (bool, bool), sabotage: u8, race: bool) -> Seen {
        TEST_OPT.with(|c| c.set(Some(opt)));
        SABOTAGE.with(|c| c.set(sabotage));
        ADOPTED.with(|a| a.borrow_mut().clear());
        let tag = TEST_SPLIT.with(std::cell::Cell::get) == Some(false);
        let dir = crate::tests::fresh_copy(
            model,
            &format!("i02-{}{}{}-{sabotage}{}", opt.0 as u8, opt.1 as u8, tag as u8, race as u8),
        );
        let (mut s, session) = open(&dir, data.clone());
        setup(&mut s);
        s.view = 10; // beyond the spawn square (17 × 17): following slices exercised
        let mut seen = Seen {
            chunks: Vec::new(),
            uuids: Vec::new(),
            adopted: Vec::new(),
            state: String::new(),
            twice: Vec::new(),
        };
        for site in SITES {
            place(&mut s, site);
            let _ = session.events.try_iter().count();
            // Other caller (move, command): synchronous read of 40 fixed chunks of
            // the view. Separate read: after the request, during the read; old path: before
            // the send (same adoption order in both cases).
            let mut early: Vec<(i32, i32)> = around(&s, 6).into_iter().step_by(4).take(40).collect();
            if race && !opt.1 {
                s.ensure_loaded(&std::mem::take(&mut early));
            }
            let mut raced = !race || early.is_empty();
            pump(&mut s, Duration::from_millis(5), usize::MAX, |s| {
                if !raced {
                    raced = true;
                    let reading =
                        s.loader.as_ref().map_or(0, |l| early.iter().filter(|p| l.pending.contains(*p)).count());
                    assert!(reading > 0, "nothing being read separately");
                    s.ensure_loaded(&early);
                }
            });
            assert!(
                s.generator.as_ref().is_none_or(|g| g.pending.is_empty() && g.seen == 0),
                "the model world must cover the view (no generation): site {site:?}, {:?}",
                s.generator.as_ref().map(|g| g.pending.iter().take(8).copied().collect::<Vec<_>>())
            );
            let (c, u) = collect(&session);
            seen.chunks.push(c);
            seen.uuids.push(u);
        }
        seen.adopted = ADOPTED.with(|a| a.borrow().clone());
        let mut out = Vec::new();
        for dim in [crate::dims::OVERWORLD, crate::dims::NETHER] {
            s.enter_world(dim);
            digest(&mut s, &mut out);
        }
        seen.state = total(&out);
        let mut uu = HashSet::new();
        for w in std::iter::once(&s.mobs.list).chain(s.dims.parked.iter().map(|w| &w.mobs.list)) {
            seen.twice.extend(w.iter().flatten().map(|m| m.uuid).filter(|u| !uu.insert(*u)));
        }
        s.stop_gen();
        s.stop_parked();
        drop(s);
        TEST_OPT.with(|c| c.set(None));
        SABOTAGE.with(|c| c.set(0));
        seen
    }

    fn inputs() -> Option<(PathBuf, Arc<rc_data::VersionData>)> {
        let model = need(model_dir(), "bench model world (RUSTCRAFT_OPT_WORLD or target/perf/model)")?;
        let data = Arc::new(need(rc_testdata::version_data(), "26.3 data cache")?);
        need(crate::registries::client_jar(&data), "jar client 26.3")?;
        Some((model, data))
    }

    fn differs(a: &Seen, b: &Seen) -> Vec<&'static str> {
        let mut d = Vec::new();
        for (i, (x, y)) in a.chunks.iter().zip(&b.chunks).enumerate() {
            let only_a: Vec<_> = x.keys().filter(|k| !y.contains_key(k)).take(6).collect();
            let only_b: Vec<_> = y.keys().filter(|k| !x.contains_key(k)).take(6).collect();
            let other: Vec<_> =
                x.iter().filter(|(k, v)| y.get(k).is_some_and(|w| w != *v)).map(|e| e.0).take(6).collect();
            if !only_a.is_empty() || !only_b.is_empty() || !other.is_empty() {
                eprintln!(
                    "site {i}: {} / {} chunks; only ref. {only_a:?}; only variant {only_b:?}; content {other:?}",
                    x.len(),
                    y.len()
                );
            }
        }
        if a.chunks != b.chunks {
            d.push("chunks sent");
        }
        for (x, y) in a.uuids.iter().zip(&b.uuids) {
            let only_a: Vec<_> = x.iter().filter(|e| !y.contains(e)).collect();
            let only_b: Vec<_> = y.iter().filter(|e| !x.contains(e)).collect();
            if !only_a.is_empty() || !only_b.is_empty() {
                eprintln!("entities: only ref. {only_a:?}; only variant {only_b:?}");
            }
        }
        if a.uuids != b.uuids {
            d.push("entities announced");
        }
        if a.adopted != b.adopted {
            d.push("adoption order");
        }
        if a.state != b.state {
            d.push("saveable state");
        }
        if !b.twice.is_empty() {
            d.push("mobs twice");
        }
        d
    }

    /// The three new variants = the old path; each sabotage is seen.
    #[test]
    fn sliced_send_and_separate_read() {
        let Some((model, data)) = inputs() else { return };
        let reference = run(&model, &data, (false, false), 0, false);
        assert!(reference.chunks.iter().all(|c| c.len() > 200), "too few chunks sent");
        assert!(reference.twice.is_empty(), "mobs twice in the reference");
        for opt in [(true, false), (false, true), (true, true)] {
            // (read slicing, second pickup): defaults, then both cut.
            for knobs in [(true, true, true), (false, false, false), (true, true, false)] {
                let got = run_with(&model, &data, opt, knobs, 0, false);
                assert!(differs(&reference, &got).is_empty(), "{opt:?} {knobs:?} : {:?}", differs(&reference, &got));
            }
        }
        // Sabotages: chunk sent before its loading, chunk forgotten, batches adopted in
        // reverse order.
        for (opt, sab) in
            [((true, true), 1), ((false, true), 1), ((true, false), 2), ((true, true), 2), ((true, true), 3)]
        {
            // Spawn square read right away cut: at 8, it covers the whole view.
            let got = run_with(&model, &data, opt, (true, true, false), sab, false);
            assert!(!differs(&reference, &got).is_empty(), "sabotage {sab} ({opt:?}) not detected");
            eprintln!("sabotage {sab} ({opt:?}) detected: {:?}", differs(&reference, &got));
        }
    }

    /// Synchronous read by another caller during the separate read: the late arrival is
    /// thrown away (neither doubled entities nor changed state); without this filter (sabotage 4), it shows.
    #[test]
    fn sync_read_during_separate_read() {
        let Some((model, data)) = inputs() else { return };
        // Reference: old path, the same 40 chunks read in advance.
        let reference = run_with(&model, &data, (false, false), (true, true, false), 0, true);
        for opt in [(false, true), (true, true)] {
            let got = run_with(&model, &data, opt, (true, true, false), 0, true);
            assert!(differs(&reference, &got).is_empty(), "{opt:?} : {:?}", differs(&reference, &got));
            let bad = run_with(&model, &data, opt, (true, true, false), 4, true);
            assert!(!differs(&reference, &bad).is_empty(), "sabotage 4 ({opt:?}) not detected");
            eprintln!("sabotage 4 ({opt:?}) detected: {:?}", differs(&reference, &bad));
        }
    }

    /// Chunk that left the view during its separate read: never adopted on arrival.
    #[test]
    fn chunk_leaves_during_its_read() {
        let Some((model, data)) = inputs() else { return };
        quit_case(&model, &data, 0);
        // Sabotage 5 (wait kept outside the view): the left chunk is adopted.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| quit_case(&model, &data, 5)));
        SABOTAGE.with(|c| c.set(0));
        TEST_OPT.with(|c| c.set(None));
        assert!(r.is_err(), "sabotage 5 not detected");
    }

    fn quit_case(model: &Path, data: &Arc<rc_data::VersionData>, sabotage: u8) {
        TEST_OPT.with(|c| c.set(Some((false, true))));
        SABOTAGE.with(|c| c.set(sabotage));
        let dir = crate::tests::fresh_copy(model, &format!("i02-quit{sabotage}"));
        let (mut s, _session) = open(&dir, data.clone());
        s.view = 8;
        place(&mut s, SITES[0]);
        s.update_tracking();
        s.send_chunks();
        let asked: Vec<(i32, i32)> = s.loader.as_ref().expect("read thread").pending.iter().copied().collect();
        assert!(!asked.is_empty());
        // Immediate departure at 2,000 blocks: the whole read in progress leaves the view.
        s.pos[0] += 2000.0;
        s.update_tracking();
        // One batch only requested (without slices): expected finished before the pickup.
        let t = Instant::now();
        while s.loader.as_ref().unwrap().ready_len() == 0 {
            assert!(t.elapsed() < Duration::from_secs(60), "batch never read");
            std::thread::sleep(Duration::from_millis(2));
        }
        s.poll_loader();
        assert!(asked.iter().all(|p| !s.chunks.contains_key(p)), "chunk left adopted");
        TEST_OPT.with(|c| c.set(None));
        s.stop_gen();
        s.stop_parked();
    }

    /// Bench (measurement, not a proof): plains32 entry then teleport into the explored zone,
    /// ticks of 50 ms; per variant: first chunk sent, empty queue, longest call and
    /// p99 of the "chunks" part of the tick. `RAYON_NUM_THREADS=1`: one core.
    #[test]
    #[ignore = "measurement bench"]
    fn bench_chunk_sending() {
        let Some((model, data)) = inputs() else { return };
        let variants = [
            ((false, false), true, true),
            ((true, false), false, true),
            ((true, false), true, true),
            ((false, true), true, true),
            ((true, true), true, false),
            ((true, true), true, true),
            ((true, true), false, true),
        ];
        for (opt, split, near) in variants {
            TEST_NEAR.with(|c| c.set(Some(near)));
            TEST_SPLIT.with(|c| c.set(Some(split)));
            TEST_OPT.with(|c| c.set(Some(opt)));
            let dir = crate::tests::fresh_copy(
                &model,
                &format!("i02-bench-{}{}{}{}", opt.0 as u8, opt.1 as u8, split as u8, near as u8),
            );
            let (mut s, session) = open(&dir, data.clone());
            // Plains entry at 32, then teleports to already generated sites of the model
            // (forest, village: on disk, not yet loaded) at 12.
            let stages: [(&str, i32, Site); 3] = [
                ("entry plains32", 32, SITES[0]),
                ("teleport forest", 12, (crate::dims::OVERWORLD, 64.5, 75.0, 4128.5)),
                ("teleport village", 12, (crate::dims::OVERWORLD, -1407.5, 76.0, 348.5)),
            ];
            for (name, view, site) in stages {
                s.view = view;
                place(&mut s, site);
                let _ = session.events.try_iter().count();
                let t0 = Instant::now();
                let mut first = None;
                let mut n = 0usize;
                let mut d = pump(&mut s, Duration::from_millis(50), usize::MAX, |_| {
                    for e in session.events.try_iter() {
                        if matches!(e, rc_client::Event::Chunk(_)) {
                            first.get_or_insert_with(|| t0.elapsed());
                            n += 1;
                        }
                    }
                });
                let all = t0.elapsed();
                d.sort_by(f64::total_cmp);
                eprintln!(
                    "bench_send {opt:?} slicing {split} near {near} {name}: {n} chunks, first at {:.1} ms, all at {:.1} ms, {} ticks, \
                     max call {:.1} ms, p99 {:.1} ms, median {:.2} ms (rayon threads {})",
                    first.map_or(f64::NAN, |f| f.as_secs_f64() * 1e3),
                    all.as_secs_f64() * 1e3,
                    d.len(),
                    d[d.len() - 1],
                    d[(d.len() * 99 / 100).min(d.len() - 1)],
                    d[d.len() / 2],
                    rayon::current_num_threads()
                );
            }
            s.stop_gen();
            s.stop_parked();
            drop(s);
            let _ = std::fs::remove_dir_all(&dir);
        }
        TEST_OPT.with(|c| c.set(None));
        TEST_SPLIT.with(|c| c.set(None));
        TEST_NEAR.with(|c| c.set(None));
    }
}
