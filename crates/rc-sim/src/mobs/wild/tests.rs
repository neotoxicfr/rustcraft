//! Tests of family F11: info sheets, panda genes, Manhattan traversal, bamboo search
//! order, anger, construction (goal draws), metadata and NBT.

use rc_proto_v777::entity::Meta;

use super::super::jrand::JRand;
use super::super::mob::Mob;
use super::super::species::{Category, Kind};
use super::{bee, fox, panda};

fn mob(kind: Kind) -> Mob {
    let mut m = Mob::new(kind, 7, 1, [0.5, 64.0, 0.5], 0.0, JRand::new(42));
    m.define_data();
    m
}

#[test]
fn info_sheets() {
    for (k, n, w, h, hp) in [
        (Kind::Bee, "minecraft:bee", 0.55, 0.5, 10.0),
        (Kind::Fox, "minecraft:fox", 0.6, 0.7, 10.0),
        (Kind::Panda, "minecraft:panda", 1.3, 1.25, 20.0),
    ] {
        let i = k.info();
        assert_eq!(i.name, n);
        assert_eq!((i.width, i.height, i.health), (w, h, hp));
        assert_eq!(i.category, Category::Creature);
        assert_eq!(Kind::from_name(n), Some(k));
        assert!(k.is_animal() && k.ageable());
    }
    assert_eq!(Kind::Fox.info().follow, 32.0);
    assert_eq!(Kind::Panda.info().attack, 6.0);
    assert_eq!(Kind::Bee.info().volume, 0.4);
    assert_eq!(Kind::Panda.food_tag(), Some("minecraft:panda_food"));
    // Babies: bee ½, fox 0.6, panda ½ (own eyes).
    assert_eq!(super::super::species::dims(Kind::Fox, true), (0.6 * 0.6, 0.7 * 0.6, 0.343_75));
    assert_eq!(super::super::species::dims(Kind::Panda, true).2, 0.281_25);
}

#[test]
fn panda_genes() {
    // Draw out of 16: 0 lazy, 1 worried, 2 playful, 3 and 5..8 weak, 4 aggressive, 9..10 brown.
    let mut seen = [0; 7];
    let mut r = JRand::new(1);
    for _ in 0..16_000 {
        seen[usize::from(panda::random_gene(&mut r))] += 1;
    }
    assert!(seen.iter().all(|&n| n > 0));
    assert!(seen[usize::from(panda::WEAK)] > seen[usize::from(panda::NORMAL)]);
    // Recessives expressed only when double.
    assert_eq!(panda::variant(panda::BROWN, panda::NORMAL), panda::NORMAL);
    assert_eq!(panda::variant(panda::BROWN, panda::BROWN), panda::BROWN);
    assert_eq!(panda::variant(panda::LAZY, panda::BROWN), panda::LAZY);
    let mut m = mob(Kind::Panda);
    m.wild.main_gene = panda::WEAK;
    m.wild.hidden_gene = panda::WEAK;
    panda::set_attributes(&mut m);
    assert_eq!(m.max_health(), 10.0);
    m.wild.main_gene = panda::LAZY;
    panda::set_attributes(&mut m);
    assert_eq!(m.movement_speed(), f64::from(0.07f32));
}

#[test]
fn panda_heredity() {
    let mut a = mob(Kind::Panda);
    a.wild.main_gene = panda::AGGRESSIVE;
    a.wild.hidden_gene = panda::AGGRESSIVE;
    let mut b = mob(Kind::Panda);
    b.wild.main_gene = panda::PLAYFUL;
    b.wild.hidden_gene = panda::PLAYFUL;
    let mut agg = 0;
    for seed in 0..200 {
        let mut baby = Mob::new(Kind::Panda, 9, 2, [0.0; 3], 0.0, JRand::new(seed));
        panda::genes_from_parents(&mut baby, &mut a, Some(&b));
        let g = [baby.wild.main_gene, baby.wild.hidden_gene];
        // One gene from each parent, except mutation (1 in 32).
        if g.contains(&panda::AGGRESSIVE) && g.contains(&panda::PLAYFUL) {
            agg += 1;
        }
    }
    assert!(agg > 150, "heredity: {agg}");
}

#[test]
fn manhattan_traversal() {
    let l = bee::manhattan_box([0, 0, 0], 1, 1, 1);
    assert_eq!(l[0], [0, 0, 0]);
    // Depth 1: x = −1, then y = −1 (z 0), y = 0 (z ±1), y = 1, then x = 0…
    assert_eq!(&l[1..4], &[[-1, 0, 0], [0, -1, 0], [0, 0, 1]]);
    assert_eq!(l[4], [0, 0, -1]);
    assert_eq!(l.len(), 27);
}

#[test]
fn construction_and_metadata() {
    let mut b = mob(Kind::Bee);
    super::register_goals(&mut b);
    assert!((20..=60).contains(&b.wild.flower_cd));
    assert_eq!(b.goals.goals.len(), 14);
    assert_eq!(b.targets.goals.len(), 3);
    assert_eq!(b.get(19), Some(&Meta::Long(-1)));
    b.wild.set_flag(bee::NECTAR, true);
    b.sync_data();
    assert_eq!(b.get(18), Some(&Meta::Byte(8)));
    let mut f = mob(Kind::Fox);
    super::register_goals(&mut f);
    assert_eq!(f.goals.goals.len(), 21);
    assert_eq!(f.targets.goals.len(), 1);
    fox::set_target_goals(&mut f);
    assert_eq!(f.targets.goals.len(), 4);
    f.variant = 1;
    fox::add_trusted(&mut f, 5);
    fox::add_trusted(&mut f, 6);
    fox::add_trusted(&mut f, 7);
    assert_eq!(f.wild.trusted, [Some(5), Some(7)]);
    f.sync_data();
    assert_eq!(f.get(18), Some(&Meta::Int(1)));
    assert_eq!(f.get(20), Some(&Meta::Entity(Some(5))));
    let mut p = mob(Kind::Panda);
    p.wild.main_gene = panda::BROWN;
    p.sync_data();
    assert_eq!(p.get(21), Some(&Meta::Byte(4)));
    assert!(p.can_pickup && f.can_pickup);
}

#[test]
fn save_round_trip() {
    let mut b = mob(Kind::Bee);
    b.wild.hive = Some([1, 2, 3]);
    b.wild.set_flag(bee::STUNG, true);
    b.anger_end = 1234;
    b.wild.angry_at = Some(99);
    let e = b.save();
    let mut c = mob(Kind::Bee);
    c.load(&e);
    assert_eq!(c.wild.hive, Some([1, 2, 3]));
    assert!(bee::has_stung(&c));
    assert_eq!((c.anger_end, c.wild.angry_at), (1234, Some(99)));
    let mut f = mob(Kind::Fox);
    f.variant = 1;
    fox::add_trusted(&mut f, 42);
    f.wild.set_flag(fox::SLEEPING, true);
    let e = f.save();
    let mut g = mob(Kind::Fox);
    g.load(&e);
    assert_eq!((g.variant, g.wild.trusted[0]), (1, Some(42)));
    assert!(fox::flag(&g, fox::SLEEPING));
    // Read back: targeting goals set according to the variant.
    assert_eq!(g.targets.goals.len(), 3);
    let mut p = mob(Kind::Panda);
    p.wild.main_gene = panda::AGGRESSIVE;
    p.wild.hidden_gene = panda::WEAK;
    let e = p.save();
    assert_eq!(e.get("MainGene").and_then(rc_proto::nbt::Nbt::as_str), Some("aggressive"));
    let mut q = mob(Kind::Panda);
    q.load(&e);
    assert_eq!((q.wild.main_gene, q.wild.hidden_gene), (panda::AGGRESSIVE, panda::WEAK));
}

/// Diagnostics of the world test prerequisites (displayed, never failing).
#[test]
fn world_test_prerequisites() {
    eprintln!("F11: test world {:?}, data {}", crate::tests::test_world(), rc_testdata::version_data().is_some());
}

#[test]
fn fox_targeting_goals_only_once() {
    // /summon: read-back of the `{id}` compound (red variant) then finalization (snow variant).
    let mut f = mob(Kind::Fox);
    super::register_goals(&mut f);
    let before = f.targets.goals.len();
    super::summon_loaded(&mut f);
    f.variant = 1;
    fox::set_target_goals(&mut f);
    assert_eq!(f.targets.goals.len(), before + 3);
}

#[test]
fn weak_panda_clamps_health_to_maximum() {
    let mut p = mob(Kind::Panda);
    p.wild.main_gene = panda::WEAK;
    p.wild.hidden_gene = panda::WEAK;
    p.health = 20.0;
    panda::set_attributes(&mut p);
    assert_eq!(p.max_health(), 10.0);
    assert_eq!(p.health, 10.0);
}

#[test]
fn effective_thunder_level() {
    let mut w = crate::weather::Weather::read(std::path::Path::new("."));
    w.thunder_level = 1.0;
    w.rain_level = 0.0;
    assert_eq!(w.thunder_eff(), 0.0);
    w.rain_level = 1.0;
    assert_eq!(w.thunder_eff(), 1.0);
}
