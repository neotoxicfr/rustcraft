//! Family F05: unit tests (gossip, schedule, prices, offer draw, save)
//! and end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`): jobless villager
//! who takes a placed composter, trading (opening, offer choice, purchase),
//! wandering trader that disappears at the delay. Absent: tests skipped.

use std::time::{Duration, Instant};

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;

use super::super::jrand::JRand;
use super::super::mob::Mob;
use super::super::species::Kind;
use super::super::tests::{inject, kind_id, mob_nbt, move_player, prep};
use super::super::trades::{Cost, Offer, recipe_for};
use super::brain::{Act, DimKey, GPos, schedule};
use super::{G_MAJOR_NEG, G_MINOR_NEG, G_TRADING, Gossips};
use crate::player::Stack;
use crate::tests::{start_test, test_world};

#[test]
fn gossip_rules() {
    let mut g = Gossips::default();
    g.add(7, G_TRADING, 2);
    g.add(7, G_TRADING, 30);
    // Sum beyond the maximum (25): maximum (the old value is lower).
    assert_eq!(g.0[0].1[G_TRADING], 25);
    g.add(7, G_MINOR_NEG, 25);
    assert_eq!(g.reputation(7), 25 - 25);
    g.add(7, G_MAJOR_NEG, 1);
    assert_eq!(g.0[0].1[G_MAJOR_NEG], 0, "value < 2 removed");
    g.decay();
    // Trade −2, minor negative −20 → 5.
    assert_eq!(g.0[0].1[G_TRADING], 23);
    assert_eq!(g.0[0].1[G_MINOR_NEG], 5);
    let mut other = Gossips::default();
    let mut rng = JRand::new(5);
    let n = other.transfer_from(&g, &mut rng, 10);
    assert!((1..=2).contains(&n));
    // Loss on transfer: trade −20 → 3; minor negative −20 → nothing.
    assert_eq!(other.0.first().map(|e| e.1[G_TRADING]), Some(3));
    assert_eq!(other.0.first().map(|e| e.1[G_MINOR_NEG]), Some(0));
    let back = Gossips::from_nbt(Some(&g.to_nbt()));
    assert_eq!(back, g);
}

#[test]
fn schedule_keyframes() {
    assert_eq!(schedule(false, 0), Act::Rest);
    assert_eq!(schedule(false, 10), Act::Idle);
    assert_eq!(schedule(false, 2000), Act::Work);
    assert_eq!(schedule(false, 8999), Act::Work);
    assert_eq!(schedule(false, 9000), Act::Meet);
    assert_eq!(schedule(false, 11000), Act::Idle);
    assert_eq!(schedule(false, 23999), Act::Rest);
    assert_eq!(schedule(true, 3000), Act::Play);
    assert_eq!(schedule(true, 6000), Act::Idle);
    assert_eq!(schedule(true, 24_000 + 10_500), Act::Play);
}

fn offer(a: &str, n: i32, out: &str) -> Offer {
    Offer {
        cost_a: Cost { id: a.into(), count: n, components: None },
        cost_b: None,
        result: Stack { id: out.into(), count: 1, components: None },
        uses: 0,
        max_uses: 16,
        reward_exp: true,
        special: 0,
        demand: 0,
        mult: 0.05,
        xp: 2,
    }
}

#[test]
fn prices_and_selection_hint() {
    let Some(data) = rc_testdata::version_data() else { return };
    let items = crate::items::Items::new(std::sync::Arc::new(data), &Vec::new());
    let mut o = offer("minecraft:wheat", 20, "minecraft:emerald");
    assert_eq!(o.cost_a_count(&items), 20);
    o.demand = 10;
    // 20 + ⌊20 × 10 × 0.05⌋ = 30.
    assert_eq!(o.cost_a_count(&items), 30);
    o.special = -40;
    assert_eq!(o.cost_a_count(&items), 1, "clamped to 1");
    o.special = 60;
    assert_eq!(o.cost_a_count(&items), 64, "clamped to the stack");
    let offers =
        vec![offer("minecraft:wheat", 20, "minecraft:emerald"), offer("minecraft:wheat", 10, "minecraft:bread")];
    let w = Stack { id: "minecraft:wheat".into(), count: 20, components: None };
    // Index 0: search for the first satisfied offer (game quirk, index 0 is never
    // imposed); index 1: only offer 1.
    assert_eq!(recipe_for(&items, &offers, Some(&w), None, 0), Some(0));
    assert_eq!(recipe_for(&items, &offers, Some(&w), None, 1), Some(1));
    let few = Stack { count: 12, ..w };
    assert_eq!(recipe_for(&items, &offers, Some(&few), None, 0), Some(1));
}

#[test]
fn villager_nbt_roundtrip() {
    let mut m = Mob::new(Kind::Villager, 1, 9, [0.5, 0.0, 0.5], 0.0, JRand::new(1));
    m.define_data();
    super::load(&mut m, &Nbt::Compound(Vec::new()));
    {
        let n = m.npc.as_mut().unwrap();
        n.offers = Some(vec![offer("minecraft:wheat", 20, "minecraft:emerald")]);
        n.food = 7;
        n.gossips.add(42, G_TRADING, 4);
        n.restocks = 1;
        n.inv[0] = Some(Stack { id: "minecraft:bread".into(), count: 3, components: None });
        let b = n.brain.as_mut().unwrap();
        b.mem.home = Some(GPos { dim: DimKey::of("minecraft:the_nether"), pos: [1, 2, 3] });
        b.mem.job_site = Some(GPos { dim: DimKey::of("demo:moon"), pos: [4, 5, 6] });
        b.mem.golem = Some(100);
    }
    m.x.vdata_final = true;
    let e = m.save();
    let mut r = Mob::new(Kind::Villager, 2, 0, [0.0; 3], 0.0, JRand::new(2));
    r.load(&e);
    let n = r.npc.as_ref().unwrap();
    assert_eq!(n.offers.as_ref().map(Vec::len), Some(1));
    assert_eq!(n.food, 7);
    assert_eq!(n.gossips.reputation(42), 4);
    assert_eq!(n.restocks, 1);
    assert_eq!(n.inv[0].as_ref().map(|s| s.count), Some(3));
    let b = n.brain.as_ref().unwrap();
    // Dimension of locations kept as is (Nether, unknown dimension).
    assert_eq!(b.mem.home, Some(GPos { dim: DimKey::of("minecraft:the_nether"), pos: [1, 2, 3] }));
    assert_eq!(b.mem.job_site.map(|g| (g.dim.name(), g.pos)), Some(("demo:moon".to_owned(), [4, 5, 6])));
    assert_eq!(b.mem.golem, Some(100));
    let mems = e.get("Brain").and_then(|b| b.get("memories")).expect("memories");
    let dim = mems.get("minecraft:home").and_then(|h| h.get("value")).and_then(|v| v.get("dimension"));
    assert_eq!(dim.and_then(Nbt::as_str), Some("minecraft:the_nether"));
}

#[test]
fn schedule_applies_in_every_dimension() {
    // No more "outside the Overworld" cases: the timeline alone decides (work at noon).
    let mut b = super::brain::Brain {
        mem: super::brain::Mem::default(),
        active: Act::Idle,
        default: Act::Idle,
        available: vec![Act::Core, Act::Work, Act::Meet, Act::Rest, Act::Idle],
        baby: false,
        last_schedule: -9999,
        sensors: [0; 9],
        bed_cache: Vec::new(),
        bed_tried: 0,
        bed_last: 0,
        refresh: false,
        nodes: Vec::new(),
    };
    b.mem.job_site = Some(GPos { dim: DimKey::of("minecraft:the_nether"), pos: [0, 0, 0] });
    b.update_schedule(100, 6000);
    assert_eq!(b.active, Act::Work);
}

#[test]
fn wandering_trader_home_roundtrip() {
    let mut m = Mob::new(Kind::WanderingTrader, 1, 9, [0.5, 0.0, 0.5], 0.0, JRand::new(1));
    m.define_data();
    super::load(&mut m, &Nbt::Compound(Vec::new()));
    assert_eq!(m.npc.as_ref().unwrap().home, None);
    {
        let n = m.npc.as_mut().unwrap();
        n.home = Some(([10, 64, -3], 16));
        n.wander = None;
    }
    let e = m.save();
    assert_eq!(e.get("home_radius").and_then(Nbt::as_i64), Some(16));
    let mut r = Mob::new(Kind::WanderingTrader, 2, 0, [0.0; 3], 0.0, JRand::new(2));
    r.load(&e);
    // Destination reached (forgotten): the residence stays.
    assert_eq!(r.npc.as_ref().unwrap().home, Some(([10, 64, -3], 16)));
}

fn wait_for(s: &rc_client::Session, secs: u64, mut f: impl FnMut(&Event) -> bool) -> bool {
    crate::tests::wait_game(s, secs, &mut f)
}

fn scenario(tag: &str, mode: i32, items: &[(&str, i32)], ents: &[Nbt]) -> std::path::PathBuf {
    let world = test_world().expect("world");
    let dir = prep(&world, tag, mode, "normal", items);
    move_player(&dir, [210.5, -59.0, 210.5]);
    inject(&dir, ents);
    dir
}

/// Jobless villager: a composter placed by the player becomes its job site (POI acquired,
/// walk, farmer profession taken and sent).
#[test]
fn villager_takes_composter() {
    let (Some(_), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let ents = [mob_nbt("villager", [214.5, -59.0, 210.5], vec![])];
    let dir = scenario("f05job", 1, &[("composter", 1)], &ents);
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    s.link.send_carried_item(0).unwrap();
    // Composter placed on the grass, 2 blocks from the villager.
    s.link.send_use_item_on([216, -60, 212], 1, [0.5, 1.0, 0.5], false, 1).unwrap();
    let farmer = super::super::variants::PROFESSIONS.iter().position(|p| *p == "farmer").unwrap() as i32;
    let mut got = false;
    wait_for(&s, 40, |e| {
        if let Event::Entity(EntityEvent::Data { values, .. }) = e {
            got |= values.iter().any(|(i, v)| {
                *i == 19 && matches!(v, rc_proto_v777::entity::Meta::Villager([_, p, _]) if *p == farmer)
            });
        }
        got
    });
    s.link.disconnect();
    handle.wait();
    // The POI taken is written to `poi/` on save.
    let poi = dir.join("dimensions").join("minecraft").join("overworld").join("poi");
    let poi_written = std::fs::read_dir(&poi).map(|d| d.count() > 0).unwrap_or(false);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(got, "no farmer profession");
    assert!(poi_written, "no poi/ file");
}

/// Trading: opening (menu 19, offers), offer choice (wheat moved, emerald offered),
/// purchase by quick move (experience and uses sent back).
#[test]
fn trade_with_farmer() {
    let (Some(_), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let vd = Nbt::Compound(vec![
        ("level".into(), Nbt::Int(1)),
        ("profession".into(), Nbt::String("minecraft:farmer".into())),
        ("type".into(), Nbt::String("minecraft:plains".into())),
    ]);
    let offers = super::super::trades::offers_to_nbt(&[offer("minecraft:wheat", 20, "minecraft:emerald")]);
    let ents = [mob_nbt(
        "villager",
        [211.5, -59.0, 210.5],
        vec![("VillagerData".into(), vd), ("Offers".into(), offers), ("Xp".into(), Nbt::Int(1))],
    )];
    let dir = scenario("f05trade", 0, &[("wheat", 20)], &ents);
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    let vk = kind_id(&data, "minecraft:villager");
    let mut vid = None;
    wait_for(&s, 10, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == vk
        {
            vid = Some(a.id);
        }
        vid.is_some()
    });
    let vid = vid.expect("villager");
    let (mut window, mut got_offers) = (None, None);
    let t = Instant::now();
    let mut last = Duration::ZERO;
    let mut pos = None;
    while t.elapsed() < Duration::from_secs(15) && !(window.is_some() && got_offers.is_some()) {
        // The villager (with AI) may move away: repeated click, like a player.
        if t.elapsed() >= last + Duration::from_millis(700) {
            last = t.elapsed();
            s.link.send_interact(vid, 0, [0.0, 1.0, 0.0], false).unwrap();
        }
        let Ok(e) = s.events.recv_timeout(Duration::from_millis(20)) else { continue };
        match e {
            Event::OpenScreen { window: w, kind: 19, .. } => window = Some(w),
            Event::MerchantOffers(o) => got_offers = Some(o.offers.len()),
            Event::Entity(EntityEvent::Move(mv)) if mv.id == vid && mv.delta.is_some() => pos = Some(t.elapsed()),
            _ => {}
        }
    }
    eprintln!("last villager movement: {pos:?}");
    let window = window.expect("trading screen");
    assert_eq!(got_offers, Some(1));
    s.link.send_raw(s.link.ids.select_trade, |w| rc_proto_v777::special::write_select_trade(w, 0)).unwrap();
    let emerald = data
        .registry("minecraft:item")
        .and_then(|r| r.entries.iter().position(|e| e == "minecraft:emerald"))
        .map(|i| i as i32);
    let mut result_shown = false;
    wait_for(&s, 5, |e| {
        if let Event::Inventory(rc_proto_v777::item::Inventory::Slot { slot: 2, stack: Some(st), .. }) = e {
            result_shown |= Some(st.item) == emerald;
        }
        if let Event::Inventory(rc_proto_v777::item::Inventory::Content { slots, .. }) = e {
            result_shown |= slots.get(2).and_then(|s| s.as_ref()).is_some_and(|st| Some(st.item) == emerald);
        }
        result_shown
    });
    // Purchase: quick move of the result.
    s.link.send_container_click(window, 0, 2, 0, 1, &[], None).unwrap();
    let mut traded = None;
    wait_for(&s, 5, |e| {
        if let Event::MerchantOffers(o) = e {
            traded = Some((o.xp, o.offers.first().map_or(0, |x| x.uses)));
        }
        traded.is_some()
    });
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(result_shown, "emerald not offered");
    assert_eq!(traded, Some((3, 1)), "trade not made");
}

/// Wandering trader: disappears at the end of its delay (outside a trade).
#[test]
fn wandering_trader_despawns() {
    let (Some(_), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let ents = [mob_nbt("wandering_trader", [214.5, -59.0, 210.5], vec![("DespawnDelay".into(), Nbt::Int(60))])];
    let dir = scenario("f05trader", 1, &[], &ents);
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    let tk = kind_id(&data, "minecraft:wandering_trader");
    let (mut id, mut gone) = (None, false);
    wait_for(&s, 15, |e| {
        match e {
            Event::Entity(EntityEvent::Add(a)) if a.kind == tk => id = Some(a.id),
            Event::Entity(EntityEvent::Remove(l)) if id.is_some_and(|i| l.contains(&i)) => gone = true,
            _ => {}
        }
        gone
    });
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(id.is_some(), "trader did not appear");
    assert!(gone, "trader still there");
}

/// Demo world (`RUSTCRAFT_F05_PREP=<folder>`, `RUSTCRAFT_F05_SCENE=trade|village`):
/// "trade": level 1 farmer without AI in front of the player (same world for the official server of
/// comparison); "village": composter, bed and bell placed by the player, two jobless villagers,
/// a baby and a wandering trader with AI.
#[test]
#[ignore]
fn prep_f05_world() {
    let Ok(out) = std::env::var("RUSTCRAFT_F05_PREP") else { return };
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let scene = std::env::var("RUSTCRAFT_F05_SCENE").unwrap_or_else(|_| "trade".into());
    let out = std::path::PathBuf::from(out);
    let (x, z) = (230.5, 230.5);
    let items: &[(&str, i32)] = if scene == "village" { &[("composter", 1), ("red_bed", 1), ("bell", 1)] } else { &[] };
    let made = prep(&world, "f05prep", 1, "normal", items);
    for e in std::fs::read_dir(made.join("players").join("data")).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("dat") {
            continue;
        }
        let mut pl = crate::nbtio::read_dat(&p).unwrap();
        crate::nbtio::set(&mut pl, "Pos", Nbt::List([x, -59.0, z].iter().map(|&v| Nbt::Double(v)).collect()));
        crate::nbtio::set(&mut pl, "Rotation", Nbt::List(vec![Nbt::Float(0.0), Nbt::Float(10.0)]));
        std::fs::write(&p, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
    }
    let face = |yaw: f32| ("Rotation".to_owned(), Nbt::List(vec![Nbt::Float(yaw), Nbt::Float(0.0)]));
    if scene == "village" {
        let vd = |prof: &str| {
            Nbt::Compound(vec![
                ("level".into(), Nbt::Int(1)),
                ("profession".into(), Nbt::String(format!("minecraft:{prof}"))),
                ("type".into(), Nbt::String("minecraft:plains".into())),
            ])
        };
        inject(
            &made,
            &[
                mob_nbt("villager", [x - 2.0, -59.0, z + 6.0], vec![face(180.0), ("VillagerData".into(), vd("none"))]),
                mob_nbt("villager", [x + 2.0, -59.0, z + 7.0], vec![face(180.0), ("VillagerData".into(), vd("none"))]),
                mob_nbt("villager", [x, -59.0, z + 5.0], vec![face(180.0), ("Age".into(), Nbt::Int(-20000))]),
                mob_nbt(
                    "wandering_trader",
                    [x + 4.0, -59.0, z + 9.0],
                    vec![face(180.0), ("DespawnDelay".into(), Nbt::Int(48000))],
                ),
            ],
        );
        // Blocks placed by the player (creative): composter, bed, bell on the grass.
        let data = std::sync::Arc::new(data);
        let (s, handle, _) = start_test(&made, data);
        std::thread::sleep(Duration::from_millis(3000));
        let gx = x.floor() as i32;
        let gz = z.floor() as i32;
        for (slot, pos, seq) in
            [(0i16, [gx - 2, -60, gz + 3], 1), (1, [gx + 3, -60, gz + 3], 2), (2, [gx, -60, gz + 4], 3)]
        {
            s.link.send_carried_item(slot).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            s.link.send_use_item_on(pos, 1, [0.5, 1.0, 0.5], false, seq).unwrap();
            std::thread::sleep(Duration::from_millis(800));
        }
        std::thread::sleep(Duration::from_millis(500));
        s.link.disconnect();
        handle.wait();
    } else {
        // `RUSTCRAFT_F05_WHO`: profession (farmer by default) or `wandering_trader`.
        let who = std::env::var("RUSTCRAFT_F05_WHO").unwrap_or_else(|_| "farmer".into());
        let e = if who == "wandering_trader" {
            mob_nbt("wandering_trader", [x, -59.0, z + 2.5], vec![face(180.0), ("NoAI".into(), Nbt::Byte(1))])
        } else {
            let vd = Nbt::Compound(vec![
                ("level".into(), Nbt::Int(1)),
                ("profession".into(), Nbt::String(format!("minecraft:{who}"))),
                ("type".into(), Nbt::String("minecraft:plains".into())),
            ]);
            mob_nbt(
                "villager",
                [x, -59.0, z + 2.5],
                vec![
                    face(180.0),
                    ("NoAI".into(), Nbt::Byte(1)),
                    ("VillagerData".into(), vd),
                    ("Xp".into(), Nbt::Int(1)),
                ],
            )
        };
        inject(&made, &[e]);
    }
    let _ = std::fs::remove_dir_all(&out);
    std::fs::rename(&made, &out).unwrap();
    eprintln!("world prepared: {}", out.display());
}
