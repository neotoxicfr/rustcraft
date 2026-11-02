//! G6-F07: bone meal on the added plants (bushes, dry grasses, netherrack, nylium, planted
//! nether fungus, bamboo, climbing plants, lichen, hanging moss, mangrove leaves, rooted dirt,
//! pale moss carpet, pitcher plant, seagrass, pickles, shelf mushroom, small dripleaf) and in
//! water; survival of the red shrub. Fresh flat world from `g6f01_tests`: the 26.3 cache and
//! the client jar are required (the test fails without them).

use super::g6f01_tests::{W, Y, stack, world};
use super::*;

fn bm(w: &mut W, at: P) -> bool {
    let ok = w.p.s.grow_with_bone_meal(at);
    w.p.s.flush_block_changes();
    let _ = w.p.take_events();
    ok
}

fn block(w: &W, at: P) -> String {
    let n = w.name(at);
    n.split('[').next().unwrap_or("").to_owned()
}

fn prop(w: &W, at: P, key: &str) -> String {
    w.p.s.v().get(w.p.s.get(at), key).unwrap_or("?").to_owned()
}

fn count_around(w: &W, at: P, name: &str) -> usize {
    Dir::HORIZONTAL.iter().filter(|&&d| block(w, rel(at, d)) == name).count()
}

/// Bush, red shrub and firefly bush: a free neighbour receives the same plant; with no possible
/// neighbour, the bone meal is refused. Red shrub: drops without ground.
#[test]
fn bushes_spread_and_red_shrub_survives() {
    let mut w = world("bush");
    for (x, name) in [(2, "minecraft:bush"), (7, "minecraft:red_shrub"), (12, "minecraft:firefly_bush")] {
        let at = [x, Y, 4];
        w.set(at, name);
        assert!(bm(&mut w, at), "{name}: bone meal accepted");
        assert_eq!(count_around(&w, at, name), 1, "{name}: a single neighbour");
    }
    // Surrounded: no free neighbour left.
    let at = [2, Y, 10];
    w.set(at, "minecraft:bush");
    for d in Dir::HORIZONTAL {
        w.set(rel(at, d), "minecraft:stone");
    }
    assert!(!bm(&mut w, at), "surrounded bush: bone meal refused");
    // Red shrub deprived of ground: removed on update.
    let at = [7, Y, 12];
    w.set(at, "minecraft:red_shrub");
    w.set(below(at), "minecraft:air");
    assert_eq!(block(&w, at), "minecraft:air", "red shrub without ground");
}

/// Short dry grass → tall; tall → short grass on a neighbour.
#[test]
fn dry_grass() {
    let mut w = world("drygrass");
    w.cmd(&format!("fill 0 {} 0 15 {} 15 minecraft:sand", Y - 1, Y - 1));
    let at = [4, Y, 4];
    w.set(at, "minecraft:short_dry_grass");
    assert!(bm(&mut w, at));
    assert_eq!(block(&w, at), "minecraft:tall_dry_grass");
    assert!(bm(&mut w, at));
    assert_eq!(count_around(&w, at, "minecraft:short_dry_grass"), 1);
}

/// Netherrack touched by nylium: becomes that nylium; alone, refused. Nylium: vegetation
/// above. Fungus on its nylium: planted variant (stem, drops of the broken plants).
#[test]
fn nether_plants() {
    let mut w = world("nether");
    w.set([2, Y, 2], "minecraft:netherrack");
    assert!(!bm(&mut w, [2, Y, 2]), "netherrack without nylium");
    w.set([3, Y, 2], "minecraft:warped_nylium");
    assert!(bm(&mut w, [2, Y, 2]));
    assert_eq!(block(&w, [2, Y, 2]), "minecraft:warped_nylium");

    w.cmd(&format!("fill 6 {} 6 12 {} 12 minecraft:crimson_nylium", Y - 1, Y - 1));
    assert!(bm(&mut w, [9, Y - 1, 9]));
    let grown = (6..=12)
        .flat_map(|x| (6..=12).map(move |z| [x, Y, z]))
        .filter(|&q| block(&w, q) != "minecraft:air")
        .count();
    assert!(grown > 0, "nylium vegetation");

    let at = [2, Y, 12];
    w.set(below(at), "minecraft:crimson_nylium");
    w.set(at, "minecraft:crimson_fungus");
    // Cocoa attached on the side (independent of the block below): only breaking the base drops it.
    w.set([3, Y + 1, 12], "minecraft:jungle_log");
    w.set(above(at), "minecraft:cocoa[age=0,facing=east]");
    for _ in 0..60 {
        if block(&w, at) != "minecraft:crimson_fungus" {
            break;
        }
        assert!(bm(&mut w, at), "fungus on its nylium");
    }
    assert_eq!(block(&w, at), "minecraft:crimson_stem", "planted huge fungus");
    assert_eq!(block(&w, above(at)), "minecraft:crimson_stem");
    let beans: i32 = w.p.s.life.drops.iter()
        .filter(|d| !d.is_removed() && d.stack.id == "minecraft:cocoa_beans")
        .map(|d| d.stack.count)
        .sum();
    assert_eq!(beans, 1, "cocoa broken with drops");
    // Fungus off its nylium: refused.
    w.set([5, Y, 2], "minecraft:warped_fungus");
    assert!(!bm(&mut w, [5, Y, 2]));
}

/// Fungus planted under a hanging weeping vine: every vine cell crossed by the stem is broken
/// in turn (event 2001), even if the break below turned it into a tip.
#[test]
fn planted_fungus_breaks_whole_vine_column() {
    let mut w = world("fungusvines");
    let at = [8, Y, 8];
    w.cmd(&format!("fill 0 {} 0 15 {} 15 minecraft:stone", Y + 40, Y + 40));
    w.cmd(&format!("fill 8 {} 8 8 {} 8 minecraft:weeping_vines_plant", Y + 4, Y + 39));
    w.set([8, Y + 3, 8], "minecraft:weeping_vines[age=0]");
    w.set(below(at), "minecraft:crimson_nylium");
    w.set(at, "minecraft:crimson_fungus");
    let _ = w.events();
    let mut broken = Vec::new();
    for _ in 0..60 {
        if block(&w, at) != "minecraft:crimson_fungus" {
            break;
        }
        assert!(w.p.s.grow_with_bone_meal(at), "fungus on its nylium");
        for e in w.events() {
            if let rc_client::Event::LevelEvent { kind: 2001, pos, .. } = e {
                broken.push(pos);
            }
        }
    }
    assert_eq!(block(&w, at), "minecraft:crimson_stem", "planted huge fungus");
    let stems: Vec<i32> =
        (Y + 3..Y + 40).filter(|&y| block(&w, [8, y, 8]) == "minecraft:crimson_stem").collect();
    assert!(stems.len() >= 2, "the stem crosses at least two vine cells: {stems:?}");
    let top = *stems.last().expect("stem");
    for y in stems {
        assert!(broken.contains(&[8, y, 8]), "vine at y = {y} broken (broken: {broken:?})");
    }
    // The last break turned the body above into a tip; the cap does not replace a vine: the
    // tip stays.
    assert_eq!(block(&w, [8, top + 1, 8]), "minecraft:weeping_vines", "tip above the stem");
}

/// Planted fungus surrounded by tall grass: when the cap starts low enough, the broken half
/// takes the other with it (shape update) and the cap places nothing in the inner cells of its
/// bottom rows. No half is left alone.
#[test]
fn planted_fungus_leaves_no_half_tall_grass() {
    let mut w = world("fungusgrass");
    let at = [8, Y, 8];
    let ring: Vec<P> =
        (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| [8 + dx, Y, 8 + dz])).filter(|&q| q != at).collect();
    let mut cleared = 0;
    for seed in 0..16 {
        // Reset (a fill that changes nothing is refused: no assertion here).
        w.p.run(&format!("fill 3 {Y} 3 13 {} 13 minecraft:air\n", Y + 30));
        w.set(below(at), "minecraft:crimson_nylium");
        w.set(at, "minecraft:crimson_fungus");
        for &q in &ring {
            w.set(q, "minecraft:tall_grass[half=lower]");
            w.set(above(q), "minecraft:tall_grass[half=upper]");
        }
        w.p.s.bs.rng = JavaRandom::new(seed);
        for _ in 0..60 {
            if block(&w, at) != "minecraft:crimson_fungus" {
                break;
            }
            assert!(bm(&mut w, at), "fungus on its nylium");
        }
        assert_eq!(block(&w, at), "minecraft:crimson_stem", "seed {seed}: planted huge fungus");
        for &q in &ring {
            match (block(&w, q).as_str(), block(&w, above(q)).as_str()) {
                ("minecraft:tall_grass", "minecraft:tall_grass") => {
                    assert_eq!(prop(&w, q, "half"), "lower");
                    assert_eq!(prop(&w, above(q), "half"), "upper");
                }
                ("minecraft:air", "minecraft:air") => cleared += 1,
                _ => panic!("seed {seed}, cell {q:?}: {} under {}", w.name(q), w.name(above(q))),
            }
        }
    }
    assert!(cleared > 0, "no cap low enough to touch the grass");
}

fn break_events(evs: Vec<rc_client::Event>) -> Vec<P> {
    evs.into_iter()
        .filter_map(|e| match e {
            rc_client::Event::LevelEvent { kind: 2001, pos, .. } => Some(pos),
            _ => None,
        })
        .collect()
}

/// Fire and soul fire broken: no break effect (event 2001), unlike stone.
#[test]
fn fire_breaks_silently() {
    let mut w = world("firebreak");
    w.set([4, Y - 1, 4], "minecraft:netherrack");
    w.set([4, Y, 4], "minecraft:fire");
    w.set([6, Y - 1, 6], "minecraft:soul_soil");
    w.set([6, Y, 6], "minecraft:soul_fire");
    w.set([8, Y, 8], "minecraft:stone");
    let _ = w.events();
    for p in [[4, Y, 4], [6, Y, 6], [8, Y, 8]] {
        assert!(w.p.s.destroy_block(p, true, 512), "{p:?} broken");
    }
    assert_eq!(break_events(w.events()), vec![[8, Y, 8]], "only the stone has a break effect");
}

/// Fungus planted next to a fire on netherrack: the cap, when it starts at ground level,
/// breaks the fire with no break effect.
#[test]
fn planted_fungus_breaks_fire_silently() {
    let mut w = world("fungusfire");
    let (at, fire) = ([8, Y, 8], [9, Y, 8]);
    let mut gone = 0;
    for seed in 0..16 {
        w.p.run(&format!("fill 3 {Y} 3 13 {} 13 minecraft:air\n", Y + 30));
        w.set(below(at), "minecraft:crimson_nylium");
        w.set(below(fire), "minecraft:netherrack");
        w.set(fire, "minecraft:fire");
        w.set(at, "minecraft:crimson_fungus");
        let _ = w.events();
        w.p.s.bs.rng = JavaRandom::new(seed);
        let mut breaks = Vec::new();
        for _ in 0..60 {
            if block(&w, at) != "minecraft:crimson_fungus" {
                break;
            }
            assert!(w.p.s.grow_with_bone_meal(at), "fungus on its nylium");
            breaks.extend(break_events(w.events()));
        }
        assert_eq!(block(&w, at), "minecraft:crimson_stem", "seed {seed}: planted huge fungus");
        assert!(!breaks.contains(&fire), "seed {seed}: break effect on the fire");
        if block(&w, fire) != "minecraft:fire" {
            gone += 1;
        }
    }
    assert!(gone > 0, "no cap low enough to break the fire");
}

/// Lichen at the top of the world against a wall: the spread that wraps around the edge toward
/// y = 320 (outside the world) fails to place; the game then tries the next direction, and a
/// face is added elsewhere, whatever the drawn order.
#[test]
fn lichen_skips_a_spread_outside_the_world() {
    let mut w = world("lichentop");
    let h = w.p.s.dim.height;
    let top = h.min_y + h.sections as i32 * 16 - 1;
    let lichen = [4, top, 4];
    let faces = |w: &W| {
        let mut n = 0;
        for x in 2..7 {
            for y in top - 2..=top {
                for z in 2..7 {
                    if block(w, [x, y, z]) == "minecraft:glow_lichen" {
                        n += Dir::ALL.iter().filter(|d| prop(w, [x, y, z], d.name()) == "true").count();
                    }
                }
            }
        }
        n
    };
    for seed in 0..32 {
        w.p.run(&format!("fill 2 {} 2 6 {top} 6 minecraft:air\n", top - 2));
        w.set([4, top, 5], "minecraft:stone");
        w.set([4, top - 1, 5], "minecraft:stone");
        w.set(lichen, "minecraft:glow_lichen[south=true]");
        let n0 = faces(&w);
        w.p.s.bs.rng = JavaRandom::new(seed);
        assert!(bm(&mut w, lichen), "seed {seed}: bone meal accepted");
        assert_eq!(faces(&w), n0 + 1, "seed {seed}: one more lichen face");
    }
}

/// Micro-bench (loaded machine, `--ignored`): placing a feature in the live world, 9 chunks.
/// Copy of the world into the region, cell-by-cell reads (the previous code) against unpacking
/// one pass per section; then a complete oak placement (journal replayed since L9-STRUCT: the
/// region / world comparison it replaced is gone).
#[test]
#[ignore = "micro-bench"]
fn bench_g6f07_deco_copy_and_compare() {
    use std::hint::black_box;
    use std::time::Instant;
    let mut w = world("benchdeco");
    // Sections packed as in a real world: layers of varied blocks.
    let mix = ["stone", "andesite", "granite", "diorite", "dirt", "gravel", "deepslate", "tuff"];
    for y in -64i32..40 {
        let b = mix[y.rem_euclid(8) as usize];
        w.p.run(&format!("fill -16 {y} -16 31 {y} 31 minecraft:{b}\n"));
    }
    let (wd, dg) = w.p.s.place_data().expect("generation data");
    let s = &w.p.s;
    let h = s.dim.height;
    let cs: Vec<(i32, i32)> = (-1..=1).flat_map(|dz| (-1..=1).map(move |dx| (dx, dz))).collect();
    // Complete region copy (unpacking and height maps), 9 chunks.
    let deco9 = || {
        let t = Instant::now();
        for &(cx, cz) in &cs {
            black_box(s.deco_chunk(cx, cz, &dg, &wd));
        }
        t.elapsed().as_secs_f64() * 1e3
    };
    // Region copy shared among n threads (placement lead, L9-STRUCT).
    let live: Vec<_> = cs.iter().map(|c| s.chunks.get(c).and_then(Option::as_deref)).collect();
    let (data, states) = (&*s.data, &*s.states);
    let deco9_par = |n: usize| {
        let t = Instant::now();
        std::thread::scope(|sc| {
            for (part, lv) in cs.chunks(cs.len().div_ceil(n)).zip(live.chunks(cs.len().div_ceil(n))) {
                let (dg, wd) = (&dg, &wd);
                sc.spawn(move || {
                    for (&c, l) in part.iter().zip(lv) {
                        black_box(crate::server::Server::copy_chunk(data, states, h, *l, c, dg, wd, true));
                    }
                });
            }
        });
        t.elapsed().as_secs_f64() * 1e3
    };
    let mut par = [f64::MAX; 4];
    for _ in 0..30 {
        for (k, n) in [1usize, 2, 4, 8].into_iter().enumerate() {
            par[k] = par[k].min(deco9_par(n));
        }
    }
    eprintln!("region copy on 1/2/4/8 threads (spawned): {par:.2?} ms");
    // The two variants, each over the 9 chunks; time in ms.
    let copy = |by_section: bool| {
        let t = Instant::now();
        for &(cx, cz) in &cs {
            let c = s.chunks.get(&(cx, cz)).and_then(Option::as_ref).expect("loaded chunk");
            let mut out = vec![0u32; h.sections * 4096];
            for (si, sec) in c.sections.iter().enumerate() {
                if by_section {
                    sec.blocks.unpack_into(&mut out[si * 4096..(si + 1) * 4096]);
                } else {
                    for i in 0..4096 {
                        out[si * 4096 + i] = sec.blocks.get(i);
                    }
                }
            }
            black_box(&out);
        }
        t.elapsed().as_secs_f64() * 1e3
    };
    // Alternated rounds, best time of each variant.
    let mut best = [f64::MAX; 3];
    for _ in 0..30 {
        for (k, v) in [copy(false), copy(true), deco9()].into_iter().enumerate() {
            best[k] = best[k].min(v);
        }
    }
    let spots: Vec<P> = (0..30).map(|i| [-12 + 3 * (i % 8), 40, -12 + 3 * (i / 8)]).collect();
    for &at in &spots {
        w.set(below(at), "minecraft:dirt");
    }
    let t = Instant::now();
    let grown = spots.iter().filter(|&&at| w.p.s.place_world_feature("minecraft:oak", at)).count();
    let tree = t.elapsed().as_secs_f64() * 1e3 / spots.len() as f64;
    assert!(grown > 0, "no oak placed");
    // Bone meal on grass: up to 128 attempts, each placing a grass or flower feature.
    w.p.run("fill -16 39 16 31 39 31 minecraft:grass_block\n");
    let field: Vec<P> = (0..8).map(|i| [-12 + 3 * i, 40, 24]).collect();
    let t = Instant::now();
    for &at in &field {
        w.p.s.grow_with_bone_meal(below(at));
    }
    let grass = t.elapsed().as_secs_f64() * 1e3 / field.len() as f64;
    eprintln!(
        "9 chunks, best of 30: copy per cell {:.2} ms, per section {:.2} ms, region copy {:.2} ms; \
         complete oak placement {tree:.2} ms ({grown} placed); bone meal on grass {grass:.2} ms",
        best[0], best[1], best[2]
    );
}

/// Features placed by name (saplings, bone meal): all present in a plains flat world, whose
/// biomes cite neither the nether features nor most of the trees.
#[test]
fn live_features_loaded_everywhere() {
    let mut w = world("feats");
    let (_, dg) = w.p.s.place_data().expect("generation data");
    let deco = dg.deco.clone().expect("decorator");
    let missing: Vec<&str> =
        rc_worldgen::decor::LIVE_FEATURES.iter().copied().filter(|f| deco.reg.feature_id(f).is_none()).collect();
    assert!(missing.is_empty(), "missing features: {missing:?}");
    assert!(deco.reg.placed_id("minecraft:grass_bonemeal").is_some());
}

/// Bamboo: sapling → stalk; stalk → 1 or 2 stalks at the top.
#[test]
fn bamboo() {
    let mut w = world("bamboo");
    let at = [4, Y, 4];
    w.set(at, "minecraft:bamboo_sapling");
    assert!(bm(&mut w, at));
    assert_eq!(block(&w, above(at)), "minecraft:bamboo");
    assert!(bm(&mut w, at));
    assert_eq!(block(&w, rel_n(at, Dir::Up, 2)), "minecraft:bamboo");
}

/// Climbing plants: head that grows (age + 1), body that defers to the head, cave vine berries,
/// kelp in water.
#[test]
fn growing_plants() {
    let mut w = world("vines");
    // High ceiling: the nether vine gains an unbounded number of cells (0.826 per cell).
    w.cmd(&format!("fill 0 {} 0 15 {} 15 minecraft:stone", Y + 60, Y + 60));
    w.cmd(&format!("fill 6 {} 3 6 {} 3 minecraft:stone", Y + 6, Y + 6));
    let top = [3, Y + 59, 3];
    w.set(top, "minecraft:weeping_vines[age=3]");
    assert!(bm(&mut w, top));
    assert_eq!(block(&w, top), "minecraft:weeping_vines_plant");
    // 1 cell or more; the bottom head has age 3 + number of cells.
    let tip = (Y..top[1]).find(|&y| block(&w, [3, y, 3]) != "minecraft:air").expect("lengthened vine");
    assert_eq!(block(&w, [3, tip, 3]), "minecraft:weeping_vines");
    assert_eq!(prop(&w, [3, tip, 3], "age"), (3 + top[1] - tip).min(25).to_string());
    let before = (Y..=top[1]).filter(|&y| block(&w, [3, y, 3]) != "minecraft:air").count();
    assert!(bm(&mut w, top), "body: defers to the head");
    let after = (Y..=top[1]).filter(|&y| block(&w, [3, y, 3]) != "minecraft:air").count();
    assert!(after > before, "the column lengthens");

    let cave = [6, Y + 5, 3];
    w.set(cave, "minecraft:cave_vines[berries=false]");
    assert!(bm(&mut w, cave));
    assert_eq!(prop(&w, cave, "berries"), "true");
    assert!(!bm(&mut w, cave), "berries already there");

    w.cmd(&format!("fill 9 {Y} 9 9 {} 9 minecraft:water", Y + 3));
    let kelp = [9, Y, 9];
    w.set(kelp, "minecraft:kelp[age=0]");
    assert!(bm(&mut w, kelp));
    assert_eq!(block(&w, above(kelp)), "minecraft:kelp");
    assert_eq!(block(&w, kelp), "minecraft:kelp_plant");
    assert!(bm(&mut w, kelp), "kelp: body");
    assert_eq!(block(&w, rel_n(kelp, Dir::Up, 2)), "minecraft:kelp");
}

/// Glow lichen against a wall: one more face somewhere; hanging moss: lengthened tip; mangrove
/// leaves: hanging propagule; rooted dirt: roots.
#[test]
fn faces_and_hanging() {
    let mut w = world("hanging");
    w.set([5, Y, 5], "minecraft:stone");
    w.set([4, Y, 5], "minecraft:glow_lichen[east=true]");
    let faces = |w: &W| {
        let mut n = 0;
        for x in 2..8 {
            for y in Y..Y + 3 {
                for z in 2..8 {
                    if block(w, [x, y, z]) == "minecraft:glow_lichen" {
                        n += Dir::ALL.iter().filter(|d| prop(w, [x, y, z], d.name()) == "true").count();
                    }
                }
            }
        }
        n
    };
    let n0 = faces(&w);
    assert!(bm(&mut w, [4, Y, 5]));
    assert_eq!(faces(&w), n0 + 1, "one more lichen face");

    w.cmd(&format!("fill 0 {} 0 15 {} 15 minecraft:stone", Y + 6, Y + 6));
    let moss = [9, Y + 5, 9];
    w.set(moss, "minecraft:pale_hanging_moss[tip=true]");
    assert!(bm(&mut w, moss));
    assert_eq!(block(&w, below(moss)), "minecraft:pale_hanging_moss");
    assert_eq!(prop(&w, below(moss), "tip"), "true");
    assert_eq!(prop(&w, moss, "tip"), "false");

    let leaves = [12, Y + 5, 12];
    w.set(leaves, "minecraft:mangrove_leaves[persistent=true]");
    assert!(bm(&mut w, leaves));
    assert_eq!(w.name(below(leaves)).split('[').next(), Some("minecraft:mangrove_propagule"));
    assert_eq!(prop(&w, below(leaves), "hanging"), "true");
    assert_eq!(prop(&w, below(leaves), "age"), "0");
    assert!(!bm(&mut w, leaves), "cell below occupied");

    let dirt = [12, Y + 5, 2];
    w.set(dirt, "minecraft:rooted_dirt");
    assert!(bm(&mut w, dirt));
    assert_eq!(block(&w, below(dirt)), "minecraft:hanging_roots");
}

/// Pale moss carpet against a wall: layer above; pitcher plant: + 1 age, upper half from
/// age 3; shelf mushroom: age 1; small dripleaf: big dripleaf.
#[test]
fn carpet_pitcher_shelf_dripleaf() {
    let mut w = world("misc");
    w.set([3, Y, 3], "minecraft:stone");
    w.set([3, Y + 1, 3], "minecraft:stone");
    w.set([2, Y, 3], "minecraft:pale_moss_carpet");
    assert!(bm(&mut w, [2, Y, 3]));
    assert_eq!(block(&w, [2, Y + 1, 3]), "minecraft:pale_moss_carpet");
    assert_eq!(prop(&w, [2, Y + 1, 3], "bottom"), "false");

    let pit = [6, Y, 6];
    w.set(below(pit), "minecraft:farmland");
    w.set(pit, "minecraft:pitcher_crop[age=2,half=lower]");
    assert!(bm(&mut w, pit));
    assert_eq!(prop(&w, pit, "age"), "3");
    assert_eq!(w.name(above(pit)).split('[').next(), Some("minecraft:pitcher_crop"));
    assert_eq!(prop(&w, above(pit), "half"), "upper");
    assert!(bm(&mut w, above(pit)), "upper half: defers to the lower one");
    assert_eq!(prop(&w, pit, "age"), "4");
    assert!(!bm(&mut w, pit), "ripe");

    w.set([10, Y, 3], "minecraft:stone");
    w.set([9, Y, 3], "minecraft:shelf_mushroom[facing=west,age=0]");
    assert!(bm(&mut w, [9, Y, 3]));
    assert_eq!(prop(&w, [9, Y, 3], "age"), "1");
    assert!(!bm(&mut w, [9, Y, 3]));

    let leaf = [12, Y, 12];
    w.set(below(leaf), "minecraft:clay");
    w.set(leaf, "minecraft:small_dripleaf[facing=north,half=lower]");
    w.set(above(leaf), "minecraft:small_dripleaf[facing=north,half=upper]");
    assert!(bm(&mut w, above(leaf)));
    assert!(block(&w, leaf).starts_with("minecraft:big_dripleaf"), "{}", w.name(leaf));
    assert_eq!(prop(&w, leaf, "facing"), "north");
    let top = (Y..Y + 6).rfind(|&y| block(&w, [12, y, 12]) != "minecraft:air").unwrap_or(Y);
    assert_eq!(block(&w, [12, top, 12]), "minecraft:big_dripleaf", "leaf at the top");
    assert!((Y + 1..=Y + 4).contains(&top), "height 2 to 5");
}

/// Coral biome: side face clicked → wall fan turned toward that face at the first attempt;
/// corals sown around.
#[test]
fn corals_in_warm_ocean() {
    let mut w = world("coral");
    w.cmd(&format!("fillbiome 0 {} 0 15 {} 15 minecraft:warm_ocean", Y - 4, Y + 8));
    w.cmd(&format!("fill 0 {} 0 15 {} 15 minecraft:stone", Y - 1, Y - 1));
    w.cmd(&format!("fill 0 {Y} 0 15 {} 15 minecraft:water", Y + 3));
    let wall = [8, Y + 1, 8];
    w.set(wall, "minecraft:stone");
    w.hold(stack("minecraft:bone_meal", 1));
    w.click(wall, Dir::East, [1.0, 0.5, 0.5]);
    let fan = rel(wall, Dir::East);
    assert!(block(&w, fan).ends_with("_coral_wall_fan"), "wall fan: {}", w.name(fan));
    assert_eq!(prop(&w, fan, "facing"), "east");
}

/// Warm ocean / plains border: the coral biome is read through the fuzzy zoom, not in the
/// block's raw quart. On cells where the two readings differ, the wall fan of the first attempt
/// follows the zoom.
#[test]
fn corals_follow_zoomed_biome() {
    let mut w = world("coralzoom");
    w.cmd(&format!("fillbiome 0 {} 0 7 {} 15 minecraft:warm_ocean", Y - 4, Y + 8));
    w.cmd(&format!("fill 0 {} 0 15 {} 15 minecraft:stone", Y - 1, Y - 1));
    w.cmd(&format!("fill 0 {Y} 0 15 {} 15 minecraft:water", Y + 3));
    let seed = w.p.s.wgs.zoom_seed;
    // Warm quart: quart x 0 or 1 (blocks 0 to 7 of chunk 0).
    let warm = |qx: i32| (0..2).contains(&qx);
    let mut probes: Vec<(P, bool)> = Vec::new();
    for z in 0..16 {
        for x in 5..11 {
            let p = [x, Y, z];
            let (qx, _, _) = rc_worldgen::biome::zoom(seed, p[0], p[1], p[2]);
            let zoomed = warm(qx);
            if zoomed != warm(x >> 2) && !probes.iter().any(|(q, z2)| *z2 == zoomed || (q[2] - z).abs() < 9) {
                probes.push((p, zoomed));
            }
        }
    }
    assert!(!probes.is_empty(), "no cell where the zoom and the raw quart differ");
    for (p, zoomed) in probes {
        let wall = rel(p, Dir::West);
        w.set(wall, "minecraft:stone");
        w.hold(stack("minecraft:bone_meal", 1));
        w.click(wall, Dir::East, [1.0, 0.5, 0.5]);
        let fan = block(&w, p).ends_with("_coral_wall_fan");
        assert_eq!(fan, zoomed, "cell {p:?}: fan {fan}, zoomed biome has corals {zoomed} ({})", w.name(p));
    }
}

/// Seagrass → tall; pickles on coral → 4; bone meal on the face of a solid block in water
/// (player and dispenser): seagrass.
#[test]
fn water_plants() {
    let mut w = world("water");
    w.cmd(&format!("fill 0 {} 0 15 {} 15 minecraft:stone", Y - 1, Y - 1));
    w.cmd(&format!("fill 0 {Y} 0 15 {} 15 minecraft:water", Y + 3));
    let sg = [2, Y, 2];
    w.set(sg, "minecraft:seagrass");
    assert!(bm(&mut w, sg));
    assert_eq!(prop(&w, sg, "half"), "lower");
    assert_eq!(block(&w, above(sg)), "minecraft:tall_seagrass");

    let pk = [5, Y, 5];
    w.set(below(pk), "minecraft:tube_coral_block");
    w.set(pk, "minecraft:sea_pickle[pickles=1,waterlogged=true]");
    assert!(bm(&mut w, pk));
    assert_eq!(prop(&w, pk, "pickles"), "4");
    w.set([7, Y, 7], "minecraft:sea_pickle[pickles=1,waterlogged=true]");
    assert!(!bm(&mut w, [7, Y, 7]), "pickle off coral");

    let count = |w: &W| {
        (0..16).flat_map(|x| (0..16).map(move |z| [x, Y, z])).filter(|&q| block(w, q) == "minecraft:seagrass").count()
    };
    let n0 = count(&w);
    w.hold(stack("minecraft:bone_meal", 4));
    w.click([10, Y - 1, 10], Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(w.held().map(|s| s.count), Some(3), "bone meal consumed");
    let n1 = count(&w);
    assert!(n1 > n0, "seagrass sown ({n0} -> {n1})");
    // No full water in front of the face: nothing.
    w.set([13, Y, 13], "minecraft:air");
    w.click([13, Y - 1, 13], Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(w.held().map(|s| s.count), Some(3), "no water: bone meal kept");

    // Dispenser facing the water: seagrass, one bone meal thrown, particles.
    let disp = [15, Y + 1, 15];
    w.set(below(disp), "minecraft:water");
    w.set(disp, "minecraft:dispenser[facing=down]");
    let mut slots = w.p.s.slots(disp);
    slots[0] = stack("minecraft:bone_meal", 2).map(|s| s.to_nbt(None));
    w.p.s.store_slots(disp, &slots);
    let _ = w.events();
    let st = w.p.s.get(disp);
    w.p.s.dispense_from(st, disp);
    let evs = w.events();
    let left: i32 = w.p.s.be_item_counts(disp).iter().filter(|e| e.0 == "minecraft:bone_meal").map(|e| e.1).sum();
    assert_eq!(left, 1, "dispenser: one bone meal thrown");
    assert!(
        evs.iter().any(|e| matches!(e, rc_client::Event::LevelEvent { kind: 1505, data: 15, .. })),
        "dispenser: particles"
    );
    let front = block(&w, below(disp));
    assert!(front == "minecraft:seagrass" || front == "minecraft:tall_seagrass", "dispenser: {front}");
}
