//! G5-F02f: sound played when a food item is finished; ravager trampling crops.
//! Fresh flat world from `g6f01_tests`: the 26.3 cache and the client jar are required (the test
//! fails without them).

use rc_client::Event;
use rc_proto_v777::play::{SoundPacket, SoundRef};

use super::g6f01_tests::{W, Y, stack, world};
use crate::mobs::species::Kind;

/// Sounds of the event `name`, in sending order, with their index in `evs`.
fn sounds<'a>(w: &W, evs: &'a [Event], name: &str) -> Vec<(usize, &'a SoundPacket)> {
    let id = w.p.s.sound_id(name).expect("known sound");
    evs.iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            Event::Sound(s) if s.sound == SoundRef::Id(id) => Some((i, s)),
            _ => None,
        })
        .collect()
}

/// Eats (or drinks) the held item to the end; events of the finish.
fn consume(w: &mut W, id: &str) -> Vec<Event> {
    // "Ready" player: its tick (use included) runs.
    w.p.s.loaded = true;
    w.p.s.held = 0;
    w.hold(stack(id, 2));
    w.p.s.life.vit.food = 10;
    let _ = w.events();
    w.p.s.on_use_item(0);
    assert!(w.p.s.life.vit.using.is_some(), "{id}: use started");
    let mut evs = Vec::new();
    for _ in 0..60 {
        w.p.s.vitals_tick();
        evs.extend(w.events());
        if w.p.s.life.vit.using.is_none() {
            break;
        }
    }
    assert_eq!(w.held().map(|s| s.count), Some(1), "{id}: one item consumed");
    evs
}

/// Finishing a food item: the consumable's sound (neutral category, volume 1, pitch 1 ± 0.4) just
/// before the burp; the component's own sound replaces the default one (honey bottle).
#[test]
fn food_finish_plays_consumable_sound_before_burp() {
    let mut w = world("g5f02f-eat");
    for (item, sound) in [
        ("minecraft:bread", "minecraft:entity.generic.eat"),
        ("minecraft:honey_bottle", "minecraft:item.honey_bottle.drink"),
    ] {
        let evs = consume(&mut w, item);
        let eat = sounds(&w, &evs, sound);
        let burp = sounds(&w, &evs, "minecraft:entity.player.burp");
        assert_eq!(eat.len(), 1, "{item}: one finish sound");
        assert_eq!(burp.len(), 1, "{item}: one burp");
        let (i, s) = eat[0];
        assert!(i < burp[0].0, "{item}: sound before the burp");
        assert_eq!((s.category, s.volume), (6, 1.0), "{item}: neutral, volume 1");
        assert!((0.6..=1.4).contains(&s.pitch), "{item}: pitch {}", s.pitch);
        assert_eq!(s.pos, Some(w.p.s.pos), "{item}: at the player's position");
    }
}

fn drops(w: &W, id: &str) -> i32 {
    w.p.s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == id).map(|d| d.stack.count).sum()
}

/// Ravager standing on crops: all those in its box are broken with their drops, stems and grass
/// are kept, those outside the box are kept; nothing without `mob_griefing`.
#[test]
fn ravager_tramples_crops() {
    let mut w = world("g5f02f-rav");
    let crops = [
        ([4, Y, 4], "minecraft:wheat[age=7]"),
        ([5, Y, 4], "minecraft:carrots[age=7]"),
        ([4, Y, 5], "minecraft:potatoes[age=7]"),
        ([5, Y, 5], "minecraft:beetroots[age=3]"),
        ([3, Y, 3], "minecraft:torchflower_crop[age=1]"),
        ([3, Y, 5], "minecraft:pitcher_crop[age=1,half=lower]"),
    ];
    let kept = [([5, Y, 3], "minecraft:melon_stem[age=7]"), ([3, Y, 4], "minecraft:short_grass")];
    for x in 2..=6 {
        for z in 2..=6 {
            w.set([x, Y - 1, z], "minecraft:farmland[moisture=7]");
        }
    }
    for (p, s) in crops.iter().chain(&kept) {
        w.set(*p, s);
    }
    // Outside the box (width 1.95 centered on 4.5: blocks 3..5).
    w.set([6, Y, 4], "minecraft:wheat[age=7]");

    let mut m = w.p.s.new_mob(Kind::Ravager, [4.5, f64::from(Y), 4.5], 0.0);
    w.cmd("gamerule mob_griefing false");
    w.p.s.ravager_trample_crops(&m);
    for (p, s) in &crops {
        assert_eq!(&w.name(*p), s, "without mob_griefing: kept");
    }
    w.cmd("gamerule mob_griefing true");
    w.p.s.ravager_ai_step(&mut m);
    let _ = w.events();
    for (p, s) in &crops {
        assert_eq!(w.name(*p), "minecraft:air", "{s} trampled");
    }
    for (p, s) in &kept {
        assert!(w.name(*p).starts_with(s.split('[').next().unwrap()), "{s} kept: {}", w.name(*p));
    }
    assert_eq!(w.name([6, Y, 4]), "minecraft:wheat[age=7]", "outside the box");
    assert!(drops(&w, "minecraft:wheat") >= 1, "wheat drops");
    assert!(drops(&w, "minecraft:carrot") >= 1, "carrot drops");
    assert!(drops(&w, "minecraft:pitcher_pod") >= 1, "pitcher plant drops");
}

/// Micro-bench (loaded machine, `--ignored`): a ravager's trampling pass over an empty floor
/// (common case) and over a full field.
#[test]
#[ignore = "micro-bench"]
fn bench_ravager_trample_pass() {
    let mut w = world("g5f02f-bench");
    let m = w.p.s.new_mob(Kind::Ravager, [4.5, f64::from(Y), 4.5], 0.0);
    let n = 1_000_000;
    let t = std::time::Instant::now();
    for _ in 0..n {
        w.p.s.ravager_trample_crops(std::hint::black_box(&m));
    }
    let empty = t.elapsed().as_secs_f64() * 1e9 / f64::from(n);
    // Full field: the crop is replaced at every round (placing is included in the figure).
    for x in 3..=5 {
        for z in 3..=5 {
            w.set([x, Y - 1, z], "minecraft:farmland[moisture=7]");
        }
    }
    let wheat = w.p.s.default_state("minecraft:wheat");
    let k = 2_000;
    let t = std::time::Instant::now();
    for _ in 0..k {
        for x in 3..=5 {
            for z in 3..=5 {
                w.p.s.set_block([x, Y, z], wheat, 3);
            }
        }
        w.p.s.ravager_trample_crops(&m);
    }
    let full = t.elapsed().as_secs_f64() * 1e9 / f64::from(k);
    let _ = w.events();
    eprintln!("trampling: empty box {empty:.1} ns, 9 crops (place + break) {full:.0} ns");
}
