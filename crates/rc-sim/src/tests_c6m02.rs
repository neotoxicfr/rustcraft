//! Proofs of mission C6-M02 (per-state tables on the block and mob side) on a server opened
//! on a new world: each new path compared with the old one on ALL states, blocks or
//! tags, before and after a tag reload that modifies them. Micro-benches ignored
//! by default (`--ignored`).

use crate::server::Server;
use crate::tag_bits::TagId;

/// Server on a new world, generator stopped and folder erased at the end.
struct World {
    s: Server,
    _dir: crate::tests_opt::Scratch,
}

impl Drop for World {
    fn drop(&mut self) {
        self.s.stop_gen();
        self.s.stop_parked();
    }
}

fn world(tag: &str) -> Option<World> {
    let (s, dir) = crate::portal_tests::open_new_world(tag, 0xC6_02)?;
    Some(World { s, _dir: crate::tests_opt::Scratch(dir) })
}

/// Block tags modified as by a reloaded data packet: `tag` loses its first two
/// blocks and gains `add`.
fn reload_with(s: &mut Server, edits: &[(&str, &str)]) {
    let mut tags = s.regs.tags.clone();
    let blocks = &mut tags.iter_mut().find(|(r, _)| r == "minecraft:block").expect("block tags").1;
    for &(tag, add) in edits {
        let id = s.data.blocks.iter().position(|b| b.name == add).expect("added block") as i32;
        let e = blocks.iter_mut().find(|(n, _)| n == tag).expect("modified tag");
        e.1.drain(..2.min(e.1.len()));
        e.1.push(id);
    }
    s.set_registry_tags(tags);
}

/// All blocks, plus indices outside the table (block unknown to callers).
fn all_blocks(s: &Server) -> impl Iterator<Item = u32> {
    (0..s.data.blocks.len() as u32).chain([s.data.blocks.len() as u32, u32::from(u16::MAX), u32::MAX])
}

fn check_tags(s: &Server) -> usize {
    let mut names: Vec<&str> = s.block_tags.keys().map(String::as_str).collect();
    names.push("minecraft:absent_from_jar");
    let mut hits = 0;
    for tag in names {
        for b in all_blocks(s) {
            let old = s.block_tags.get(tag).is_some_and(|v| v.binary_search(&b).is_ok());
            assert_eq!(s.tag_u32(tag, b, 1), old, "bit set, {tag} block {b}");
            assert_eq!(s.tag_u32(tag, b, 2), old, "{tag} block {b}");
            hits += usize::from(old);
        }
    }
    let mut present = 0;
    for &t in TagId::ALL {
        present += usize::from(s.block_tags.contains_key(t.name()));
        for b in all_blocks(s) {
            let old = s.block_tags.get(t.name()).is_some_and(|v| v.binary_search(&b).is_ok());
            for mode in 0..3 {
                assert_eq!(s.tag_id_u32(t, b, mode), old, "{t:?} block {b}, level {mode}");
            }
        }
    }
    assert!(present >= TagId::ALL.len() - 2, "named tags present in the jar: {present}");
    hits
}

#[test]
fn tag_bits_identical_before_and_after_reload() {
    let Some(mut w) = world("c6m02-tags") else { return };
    assert!(check_tags(&w.s) > 1000, "tag coverage");
    reload_with(&mut w.s, &[("minecraft:fences", "minecraft:stone"), ("minecraft:walls", "minecraft:dirt")]);
    let stone = w.s.data.blocks.iter().position(|b| b.name == "minecraft:stone").expect("stone") as u16;
    assert!(w.s.tag_id(TagId::Fences, stone), "reloaded tag taken into account");
    check_tags(&w.s);
}

#[test]
fn mob_per_state_rules_identical() {
    let Some(mut w) = world("c6m02-mob") else { return };
    let check = |s: &Server| {
        let n = s.data.states.len() as u32;
        let (mut land, mut water, mut suff) = (0, 0, 0);
        for st in (0..n).chain([n, n + 7, u32::MAX]) {
            let l = s.land_ok(st, false);
            assert_eq!(s.land_ok(st, true), l, "on foot, state {st}");
            let wa = s.water_ok(st, false);
            assert_eq!(s.water_ok(st, true), wa, "swimming, state {st}");
            let su = s.suffocating_with(st, false);
            assert_eq!(s.suffocating_with(st, true), su, "suffocation, state {st}");
            (land, water, suff) = (land + usize::from(l), water + usize::from(wa), suff + usize::from(su));
        }
        assert!(land > 1000 && water > 50 && suff > 500, "coverage: {land} on foot, {water} swimming, {suff} suffocating");
    };
    check(&w.s);
    // Reload that removes stone (among others) from suffocation and modifies fence gates and
    // fences: the per-state bits do not depend on it, suffocation follows the tag.
    reload_with(
        &mut w.s,
        &[
            ("minecraft:causes_suffocation", "minecraft:glass"),
            ("minecraft:fence_gates", "minecraft:stone"),
            ("minecraft:fences", "minecraft:stone"),
        ],
    );
    check(&w.s);
}

#[test]
fn block_name_resolved_by_identical_table() {
    let Some(w) = world("c6m02-ids") else { return };
    let s = &w.s;
    let id = &s.bs.t.id;
    for (n, v) in [
        ("nether_portal", id.nether_portal),
        ("end_portal", id.end_portal),
        ("end_gateway", id.end_gateway),
        ("bedrock", id.bedrock),
        ("frogspawn", id.frogspawn),
        ("sniffer_egg", id.sniffer_egg),
        ("creaking_heart", id.creaking_heart),
        ("sculk_shrieker", id.sculk_shrieker),
        ("dried_ghast", id.dried_ghast),
        ("command_block", id.command_block),
        ("lightning_rod", id.lightning_rod),
        ("carved_pumpkin", id.carved_pumpkin),
        ("tnt", id.tnt),
        ("lava", id.lava),
        ("water", id.water),
        ("farmland", id.farmland),
    ] {
        assert_ne!(v, u16::MAX, "identifier resolved: {n}");
        assert_eq!(s.data.blocks[v as usize].name, format!("minecraft:{n}"));
    }
    let tags: Vec<&str> = TagId::ALL.iter().map(|t| t.name()).take(24).chain(["minecraft:absent"]).collect();
    let mut names: Vec<String> = s.data.blocks.iter().map(|b| b.name.clone()).collect();
    names.extend(["stone", "minecraft:", "", "minecraft:minecraft:stone", "minecraft:nope", "Minecraft:stone"].map(String::from));
    let mut hits = 0;
    for tag in &tags {
        for n in &names {
            let old = s.block_tag_by_name_with(tag, n, false);
            assert_eq!(s.block_tag_by_name_with(tag, n, true), old, "{tag} / {n}");
            assert_eq!(s.tag_bits.name_in_tag(tag, n), old, "closure: {tag} / {n}");
            hits += usize::from(old);
        }
    }
    assert!(hits > 100, "coverage: {hits}");
}

#[test]
fn sky_darkening_cache_identical() {
    let Some(mut w) = world("c6m02-sky") else { return };
    let s = &mut w.s;
    let mut checked = 0;
    for t in (0..24_000i64).step_by(7).chain([23_999, 24_000, 133, 11_867, 13_670, 22_330]) {
        if let Some(c) = s.clocks.iter_mut().find(|(n, _)| n == "minecraft:overworld") {
            c.1 = t;
        }
        s.time = t;
        // Rain and thunder rising and falling in steps of 0.01 (like the game) over the day.
        let phase = (t / 7) as f32;
        s.life.weather.rain_level = ((phase * 0.01).sin().abs() * 100.0).round() / 100.0;
        s.life.weather.thunder_level = ((phase * 0.013).cos().abs() * 100.0).round() / 100.0;
        for _ in 0..2 {
            assert_eq!(s.mob_sky_darken_with(true), s.mob_sky_darken_with(false), "time {t}");
        }
        // Weather changed without changing the time: the kept value must not be reused.
        s.life.weather.rain_level = 1.0 - s.life.weather.rain_level;
        assert_eq!(s.mob_sky_darken_with(true), s.mob_sky_darken_with(false), "rain inverted, time {t}");
        s.life.weather.thunder_level = 1.0 - s.life.weather.thunder_level;
        assert_eq!(s.mob_sky_darken_with(true), s.mob_sky_darken_with(false), "thunder inverted, time {t}");
        checked += 1;
    }
    assert!(checked > 3000);
}

/// Mean time (ns) of `f` over `n` calls.
fn per_call(n: usize, mut f: impl FnMut(usize) -> bool) -> f64 {
    let t0 = std::time::Instant::now();
    let mut k = 0usize;
    for i in 0..n {
        k += usize::from(f(i));
    }
    std::hint::black_box(k);
    t0.elapsed().as_nanos() as f64 / n as f64
}

#[test]
#[ignore = "micro-bench"]
fn bench_tables_c6m02() {
    let Some(mut w) = world("c6m02-bench") else { return };
    let s = &mut w.s;
    let n = 1_000_000;
    let blocks = s.data.blocks.len();
    let states = s.data.states.len() as u32;
    let names: Vec<&str> = s.block_tags.keys().map(String::as_str).collect();
    for mode in 0..3u8 {
        let by_name = per_call(n, |i| s.tag_u32(names[i % names.len()], (i * 7919 % blocks) as u32, mode));
        let by_id = per_call(n, |i| s.tag_id_u32(TagId::ALL[i % TagId::ALL.len()], (i * 7919 % blocks) as u32, mode));
        println!("tags, level {mode}: by name {by_name:.1} ns, named {by_id:.1} ns");
    }
    for table in [false, true] {
        let land = per_call(n, |i| s.land_ok((i as u32 * 7919) % states, table));
        let water = per_call(n, |i| s.water_ok((i as u32 * 7919) % states, table));
        let suff = per_call(n, |i| s.suffocating_with((i as u32 * 7919) % states, table));
        println!("mobs, table {table}: on foot {land:.1} ns, swimming {water:.1} ns, suffocation {suff:.1} ns");
    }
    let bnames: Vec<String> = s.data.blocks.iter().map(|b| b.name.clone()).collect();
    for fast in [false, true] {
        let t = per_call(100_000, |i| s.block_tag_by_name_with("minecraft:logs", &bnames[i % bnames.len()], fast));
        println!("tag by block name, table {fast}: {t:.1} ns");
    }
    for cache in [false, true] {
        let t = per_call(n, |_| s.mob_sky_darken_with(cache) > 3);
        println!("sky darkening, kept {cache}: {t:.1} ns");
    }
    // Old and new top of shape through the mob view (random cells around the player).
    let p0 = s.pos.map(|v| v.floor() as i32);
    let pos = |i: usize| [p0[0] + (i % 32) as i32 - 16, p0[1] + ((i / 32) % 16) as i32 - 8, p0[2] + ((i / 512) % 32) as i32 - 16];
    let t = per_call(n, |i| s.collision_top(pos(i)).is_some());
    println!("collision_top (current switch): {t:.1} ns");
}
