//! Items with their own ray in singleplayer (test world `RUSTCRAFT_SOLO_WORLD`, absent: skipped).

use crate::blocks::Dir;
use crate::blocks::probe::{Probe, state_string};
use crate::player::Stack;

/// Stone slab at y −60, source water basin x 110..114, z 112..116 (stone bottom).
const BASE: &str = "fill 100 -60 100 131 -60 131 minecraft:stone\nfill 100 -59 100 131 -40 131 minecraft:air\n\
fill 110 -60 112 114 -60 116 minecraft:water\n";

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    assert_eq!(name_at(&p, [100, -60, 100]), "minecraft:stone", "test area absent");
    // At the north edge of the basin, facing south.
    p.s.pos = [112.5, -59.0, 110.5];
    p.s.yaw = 0.0;
    p.s.game_mode = 0;
    Some((p, dir))
}

fn done(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

fn name_at(p: &Probe, at: [i32; 3]) -> String {
    state_string(&p.s, p.s.get(at))
}

fn hold(p: &mut Probe, id: &str, count: i32) {
    let slot = 36 + p.s.held as usize;
    p.s.slots[slot] = Some(Stack { id: id.to_owned(), count, components: None });
}

fn held(p: &Probe) -> Option<Stack> {
    p.s.slots[36 + p.s.held as usize].clone()
}

/// Empty bucket aiming at water (bottom out of reticle reach or not): source taken, water bucket.
#[test]
fn bucket_takes_water_source() {
    let Some((mut p, dir)) = probe("fluid-bucket") else { return };
    p.s.pitch = 37.0;
    hold(&mut p, "minecraft:bucket", 1);
    assert!(p.s.ray_item_use(0));
    assert_eq!(name_at(&p, [112, -60, 112]), "minecraft:air");
    assert_eq!(held(&p).map(|s| s.id), Some("minecraft:water_bucket".to_owned()));
    done(p, dir);
}

/// Water bucket aiming at the stone in front: source placed above.
#[test]
fn water_bucket_places_source() {
    let Some((mut p, dir)) = probe("fluid-water") else { return };
    p.s.pitch = 60.0;
    hold(&mut p, "minecraft:water_bucket", 1);
    assert!(p.s.ray_item_use(0));
    assert_eq!(name_at(&p, [112, -59, 111]), "minecraft:water[level=0]");
    assert_eq!(held(&p).map(|s| s.id), Some("minecraft:bucket".to_owned()));
    done(p, dir);
}

/// Empty bottle aiming at water: water bottle.
#[test]
fn bottle_fills_with_water() {
    let Some((mut p, dir)) = probe("fluid-bottle") else { return };
    p.s.pitch = 37.0;
    hold(&mut p, "minecraft:glass_bottle", 1);
    assert!(p.s.ray_item_use(0));
    let s = held(&p).expect("bottle");
    assert_eq!(s.id, "minecraft:potion");
    assert_eq!(name_at(&p, [112, -60, 112]), "minecraft:water[level=0]");
    done(p, dir);
}

/// Lily pad and frogspawn: placed on the water by their ray, never on a targeted block.
#[test]
fn lily_pad_and_frogspawn_on_water_only() {
    let Some((mut p, dir)) = probe("fluid-lily") else { return };
    p.s.pitch = 37.0;
    hold(&mut p, "minecraft:lily_pad", 2);
    assert!(p.s.ray_item_use(0));
    assert_eq!(name_at(&p, [112, -59, 112]), "minecraft:lily_pad");
    assert_eq!(held(&p).map(|s| s.count), Some(1));
    // Click on the stone: nothing (the item can do nothing on a block).
    p.s.use_item_on_block([112, -60, 111], Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(name_at(&p, [112, -59, 111]), "minecraft:air");
    assert_eq!(held(&p).map(|s| s.count), Some(1));
    // Frogspawn one block further.
    p.s.pos = [112.5, -59.0, 111.5];
    hold(&mut p, "minecraft:frogspawn", 1);
    assert!(p.s.ray_item_use(0));
    assert_eq!(name_at(&p, [112, -59, 113]), "minecraft:frogspawn");
    assert_eq!(held(&p), None);
    done(p, dir);
}

/// Squid spawn egg aiming at water: squid placed in the water block (feet at the bottom of the block), egg taken.
#[test]
fn spawn_egg_into_water() {
    use crate::mobs::species::Kind;
    let Some((mut p, dir)) = probe("fluid-egg") else { return };
    p.s.pitch = 37.0;
    hold(&mut p, "minecraft:squid_spawn_egg", 1);
    assert!(p.s.ray_item_use(0));
    let squids: Vec<[f64; 3]> =
        p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Squid).map(|m| m.pos).collect();
    assert_eq!(squids, vec![[112.5, -60.0, 112.5]]);
    assert_eq!(held(&p), None);
    done(p, dir);
}

/// End to end: `use_item` packet (yaw, pitch) with an empty bucket facing a deep
/// basin (the reticle touches nothing) → water bucket sent back to the client, source removed.
#[test]
fn use_item_packet_fills_bucket() {
    use std::time::{Duration, Instant};
    let (Some(_), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else { return };
    let Some(dir) = crate::tests_s8::prep_world("fluid-e2e", 0, [112.5, -59.0, 110.5], &[(0, "minecraft:bucket", 1)])
    else {
        return;
    };
    let data = std::sync::Arc::new(data);
    let (s, handle, _p) = crate::tests::start_test(&dir, data.clone());
    let link = &s.link;
    link.send_carried_item(0).unwrap();
    let full = data.registry("minecraft:item").unwrap().entries.iter().position(|e| e == "minecraft:water_bucket");
    let t = Instant::now();
    let mut got = false;
    let mut seen = Vec::new();
    // Chunks slow to load when other tests are running: basin and use redone every
    // 500 ms until the water bucket.
    let mut next = Instant::now() + Duration::from_millis(400);
    let mut seq = 7;
    while t.elapsed() < Duration::from_secs(40) && !got {
        if Instant::now() >= next {
            link.send_command("fill 110 -62 112 114 -60 116 minecraft:water").unwrap();
            link.send_use_item(0, seq, 0.0, 37.0).unwrap();
            seq += 1;
            next += Duration::from_millis(500);
        }
        let Ok(e) = s.events.recv_timeout(Duration::from_millis(20)) else { continue };
        got = matches!(&e, rc_client::Event::Inventory(rc_proto_v777::item::Inventory::Slot { slot: 36, stack: Some(st), .. })
            if Some(st.item as usize) == full);
        if !matches!(e, rc_client::Event::Chunk { .. }) {
            let n = if matches!(e, rc_client::Event::Chat { .. }) { 1200 } else { 160 };
            seen.push(format!("{e:?}").chars().take(n).collect::<String>());
        }
    }
    assert!(got, "water bucket not received: {seen:#?}");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ray without fluid that touches nothing (toward the sky): water bucket kept, nothing placed.
#[test]
fn water_bucket_miss_keeps_item() {
    let Some((mut p, dir)) = probe("fluid-miss") else { return };
    p.s.pitch = -60.0;
    hold(&mut p, "minecraft:water_bucket", 1);
    assert!(p.s.ray_item_use(0));
    assert_eq!(held(&p).map(|s| s.id), Some("minecraft:water_bucket".to_owned()));
    done(p, dir);
}

/// Empty bucket on a bubble column: column replaced by air, water bucket.
#[test]
fn bucket_takes_bubble_column() {
    let Some((mut p, dir)) = probe("fluid-bubble") else { return };
    p.run("setblock 112 -60 112 minecraft:bubble_column[drag=false]\n");
    p.s.pitch = 37.0;
    hold(&mut p, "minecraft:bucket", 1);
    assert!(p.s.ray_item_use(0));
    assert_eq!(name_at(&p, [112, -60, 112]), "minecraft:air");
    assert_eq!(held(&p).map(|s| s.id), Some("minecraft:water_bucket".to_owned()));
    done(p, dir);
}

/// Water bucket on an already waterlogged block: success with no effect (the bucket empties, nothing changes,
/// no neighbouring source, the block is not broken).
#[test]
fn water_bucket_on_waterlogged_block_is_noop() {
    let Some((mut p, dir)) = probe("fluid-wet-lantern") else { return };
    p.run("setblock 112 -59 111 minecraft:lantern[hanging=false,waterlogged=true]\n");
    p.s.pitch = 60.0;
    hold(&mut p, "minecraft:water_bucket", 1);
    assert!(p.s.ray_item_use(0));
    assert_eq!(name_at(&p, [112, -59, 111]), "minecraft:lantern[hanging=false,waterlogged=true]");
    assert_eq!(name_at(&p, [112, -58, 111]), "minecraft:air");
    assert_eq!(held(&p).map(|s| s.id), Some("minecraft:bucket".to_owned()));
    done(p, dir);
}
