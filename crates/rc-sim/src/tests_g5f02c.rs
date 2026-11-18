//! G5-F02c: fall bounded after a wind charge jump (impulse context), per-block fall rules
//! (honey, stalagmite tip, powder snow), fall damage multiplier, block fall sound and landing
//! dust. Fresh flat world: the absence of the 26.3 cache or of the client jar makes the test FAIL.

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto_v777::play::SoundRef;

use crate::blocks::probe::Probe;
use crate::mobs::species::Kind;

struct W {
    p: Probe,
    dir: PathBuf,
}

impl Drop for W {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// First air layer of the flat world (grass at y = −61).
const Y: i32 = -60;

fn world(tag: &str) -> W {
    let data = rc_testdata::version_data().expect("G5-F02c: 26.3 data cache missing");
    let data = Arc::new(data);
    assert!(crate::registries::client_jar(&data).is_some(), "G5-F02c: 26.3 client jar missing");
    let dir = std::env::temp_dir().join(format!("rc-g5f02c-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 7;
    w.preset = crate::newworld::Preset::Flat;
    w.allow_commands = true;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    let want: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    p.s.ensure_loaded(&want);
    let t = std::time::Instant::now();
    while !want.iter().all(|c| p.s.chunks.get(c).is_some_and(Option::is_some)) {
        assert!(t.elapsed().as_secs() < 300, "G5-F02c: anti-hang guard: flat world generation stuck for 300 s");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    p.run(&format!("fill 0 {Y} 0 15 {} 15 minecraft:air\n", Y + 3));
    p.s.pos = [8.5, f64::from(Y), 8.5];
    p.s.on_ground = true;
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    let _ = p.take_events();
    W { p, dir }
}

/// Player landing at (8.5, Y, 8.5): `pre` blocks already fallen, then a movement of `h`.
fn land(w: &mut W, pre: f64, h: f64) {
    let s = &mut w.p.s;
    s.life.vit.fall = pre;
    s.life.vit.cooldown = 0;
    s.life.vit.health = 20.0;
    let before = [8.5, f64::from(Y) + h, 8.5];
    s.pos = [8.5, f64::from(Y), 8.5];
    s.on_ground = true;
    s.after_move(before, false);
}

/// Burst of a charge (`cause`) at point `at`, the player being there.
fn burst(w: &mut W, cause: &'static str, at: [f64; 3]) {
    let s = &mut w.p.s;
    s.pos = at;
    s.on_ground = false;
    s.prog.expl_cause_next = Some(cause);
    s.wind_burst(None, at, 1.2, 1.22, "minecraft:entity.wind_charge.wind_burst");
    s.prog.expl_cause_next = None;
}

fn sounds(w: &W, evs: &[Event], name: &str) -> Vec<(f32, f32)> {
    let Some(id) = w.p.s.data.registry("minecraft:sound_event").and_then(|r| r.entries.iter().position(|e| e == name))
    else {
        panic!("sound {name} missing from the registry");
    };
    evs.iter()
        .filter_map(|e| match e {
            Event::Sound(s) if s.sound == SoundRef::Id(id as i32) => Some((s.volume, s.pitch)),
            _ => None,
        })
        .collect()
}

/// `block` particles: (state, count, position, speed).
fn dust(w: &W, evs: &[Event]) -> Vec<(i32, i32, [f64; 3], [f32; 3])> {
    let kind = w
        .p
        .s
        .data
        .registry("minecraft:particle_type")
        .and_then(|r| r.entries.iter().position(|e| e == "minecraft:block"))
        .expect("block particle missing") as i32;
    evs.iter()
        .filter_map(|e| match e {
            Event::Particles(p) if p.particle.kind == kind => Some((p.particle.state, p.count, p.pos, p.speed)),
            _ => None,
        })
        .collect()
}

// ---------- Wind charge ----------

/// Wind charge jump from 6 blocks up, 10 blocks of fall: damage for 6 blocks (3) and not for 10
/// (7); context forgotten after damage. Breeze charge: the whole fall.
#[test]
fn wind_charge_bounds_the_fall() {
    let mut w = world("wind");
    let at = [8.5, f64::from(Y) + 6.0, 8.5];
    burst(&mut w, "minecraft:wind_charge", at);
    assert_eq!(w.p.s.life.vit.impulse_impact, Some(at), "impact point not noted");
    assert_eq!(w.p.s.life.vit.impulse_grace, 40);
    land(&mut w, 9.0, 1.0);
    assert_eq!(w.p.s.life.vit.health, 17.0, "fall not bounded to the impact point");
    assert_eq!(w.p.s.life.vit.impulse_impact, None, "context kept after damage");
    // Breeze charge: no bound.
    burst(&mut w, "minecraft:breeze_wind_charge", at);
    assert_eq!(w.p.s.life.vit.impulse_impact, None);
    land(&mut w, 9.0, 1.0);
    assert_eq!(w.p.s.life.vit.health, 13.0, "breeze charge treated like the player's");
}

/// Wind charge jump from the ground and return to the ground: no damage, context forgotten even
/// during the grace (landed at the height of the impact).
#[test]
fn wind_charge_back_on_ground_no_damage() {
    let mut w = world("wind-ground");
    burst(&mut w, "minecraft:wind_charge", [8.5, f64::from(Y), 8.5]);
    land(&mut w, 9.0, 1.0);
    assert_eq!(w.p.s.life.vit.health, 20.0, "damage from a wind charge jump");
    assert_eq!(w.p.s.life.vit.impulse_impact, None);
}

/// Landing without damage below the impact point during the grace: context kept; the grace
/// decreases by one per tick; at zero, the next movement on the ground forgets it. Another
/// explosion cancels the grace without erasing the point.
#[test]
fn wind_charge_grace() {
    let mut w = world("wind-grace");
    let at = [8.5, f64::from(Y) + 6.0, 8.5];
    burst(&mut w, "minecraft:wind_charge", at);
    land(&mut w, 1.0, 1.0);
    assert_eq!(w.p.s.life.vit.impulse_impact, Some(at), "context forgotten during the grace");
    for _ in 0..40 {
        w.p.s.vitals_tick();
    }
    assert_eq!(w.p.s.life.vit.impulse_grace, 0, "grace not counted down");
    let pos = w.p.s.pos;
    w.p.s.after_move(pos, true);
    assert_eq!(w.p.s.life.vit.impulse_impact, None, "context kept on the ground after the grace");
    // Ordinary explosion: grace cancelled, point kept.
    let v = &mut w.p.s.life.vit;
    v.impulse_from_explosion(true, at);
    v.impulse_from_explosion(false, [0.0; 3]);
    assert_eq!((v.impulse_grace, v.impulse_impact), (0, Some(at)));
}

/// Context read back from the player file (game fields); without a point, the key disappears.
#[test]
fn impulse_context_saved() {
    let mut v = crate::vitals::Vitals::read(&Nbt::Compound(vec![]));
    v.impulse_impact = Some([1.5, 70.0, -3.25]);
    v.impulse_grace = 12;
    let mut root = Nbt::Compound(vec![]);
    v.write(&mut root);
    let r = crate::vitals::Vitals::read(&root);
    assert_eq!((r.impulse_impact, r.impulse_grace), (Some([1.5, 70.0, -3.25]), 12));
    v.impulse_reset();
    v.write(&mut root);
    assert!(root.get("current_explosion_impact_pos").is_none(), "erased point still written");
    assert!(matches!(root.get("current_impulse_context_reset_grace_time"), Some(Nbt::Int(0))));
}

// ---------- Per-block fall rules ----------

/// Block rule: honey ×0.2 (the game's float), upward stalagmite tip (+2.5)×2 stalagmite type,
/// downward tip and frustum ordinary, powder snow without damage.
#[test]
fn fall_rules_per_block() {
    let mut w = world("rules");
    let st = |w: &mut W, b: &str| {
        w.p.run(&format!("setblock 3 {Y} 3 {b}\n"));
        w.p.s.st([3, Y, 3])
    };
    let honey = st(&mut w, "minecraft:honey_block");
    assert_eq!(w.p.s.fall_rule(honey, 10.0, false), Some((10.0, f64::from(0.2f32), "minecraft:fall")));
    let tip = st(&mut w, "minecraft:pointed_dripstone[vertical_direction=up,thickness=tip]");
    assert_eq!(w.p.s.fall_rule(tip, 10.0, false), Some((12.5, 2.0, "minecraft:stalagmite")));
    let down = st(&mut w, "minecraft:pointed_dripstone[vertical_direction=down,thickness=tip]");
    assert_eq!(w.p.s.fall_rule(down, 10.0, false), Some((10.0, 1.0, "minecraft:fall")));
    let frustum = st(&mut w, "minecraft:pointed_dripstone[vertical_direction=up,thickness=frustum]");
    assert_eq!(w.p.s.fall_rule(frustum, 10.0, false), Some((10.0, 1.0, "minecraft:fall")));
    let snow = st(&mut w, "minecraft:powder_snow");
    assert_eq!(w.p.s.fall_rule(snow, 10.0, false), None);
}

/// Player: honey (10 blocks → 1), stalagmite tip (5 blocks → 9, stalagmite type), downward tip
/// (5 blocks → 2).
#[test]
fn player_honey_and_stalagmite() {
    let mut w = world("player-blocks");
    w.p.run(&format!("setblock 8 {} 8 minecraft:honey_block\n", Y - 1));
    land(&mut w, 9.0, 1.0);
    assert_eq!(w.p.s.life.vit.health, 19.0, "fall on honey not reduced");
    w.p.run(&format!("setblock 8 {} 8 minecraft:grass_block\n", Y - 1));
    w.p.run(&format!("setblock 8 {Y} 8 minecraft:pointed_dripstone[vertical_direction=up,thickness=tip]\n"));
    let land_on_tip = |w: &mut W| {
        let s = &mut w.p.s;
        s.life.vit.fall = 4.0;
        s.life.vit.cooldown = 0;
        s.life.vit.health = 20.0;
        s.pos = [8.5, f64::from(Y) + 0.5, 8.5];
        s.on_ground = true;
        s.after_move([8.5, f64::from(Y) + 1.5, 8.5], false);
    };
    land_on_tip(&mut w);
    assert_eq!(w.p.s.life.vit.health, 11.0, "stalagmite tip: doubled damage missing");
    assert_eq!(w.p.s.life.vit.last_damage.as_ref().map(|d| d.0.as_str()), Some("minecraft:stalagmite"));
    w.p.run(&format!("setblock 8 {Y} 8 minecraft:pointed_dripstone[vertical_direction=down,thickness=tip]\n"));
    land_on_tip(&mut w);
    assert_eq!(w.p.s.life.vit.health, 18.0, "downward tip treated like a stalagmite");
}

/// Mobs: honey ×0.2, tip ×2 +2.5, powder snow without damage (zombie, 10 blocks).
#[test]
fn mob_honey_stalagmite_snow() {
    let mut w = world("mob-blocks");
    let hit = |w: &mut W, b: &str| {
        w.p.run(&format!("setblock 3 {Y} 3 {b}\n"));
        let on = w.p.s.st([3, Y, 3]);
        let mut m = w.p.s.new_mob(Kind::Zombie, [3.5, f64::from(Y) + 1.0, 3.5], 0.0);
        w.p.s.fall_on(&mut m, on, 10.0);
        m.health
    };
    assert_eq!(hit(&mut w, "minecraft:honey_block"), 19.0, "honey");
    assert_eq!(hit(&mut w, "minecraft:pointed_dripstone[vertical_direction=up,thickness=tip]"), 1.0, "tip");
    assert_eq!(hit(&mut w, "minecraft:powder_snow"), 20.0, "powder snow");
    assert_eq!(hit(&mut w, "minecraft:stone"), 13.0, "ordinary block");
}

// ---------- Multiplier, block sound, dust ----------

/// `fall_damage_multiplier` attribute at 2: 5 blocks → 4 instead of 2.
#[test]
fn player_fall_multiplier() {
    let mut w = world("mult");
    let attr = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:fall_damage_multiplier".into())),
        ("base".into(), Nbt::Double(2.0)),
    ]);
    crate::nbtio::set(&mut w.p.s.pdata.root, "attributes", Nbt::List(vec![attr]));
    assert_eq!(w.p.s.player_attribute("minecraft:fall_damage_multiplier"), 2.0, "attribute not set");
    land(&mut w, 0.0, 5.0);
    assert_eq!(w.p.s.life.vit.health, 16.0, "multiplier ignored");
}

/// Damaging landing on grass: block fall sound (½, ¾) after the player's sound; grass dust,
/// 150 × (0.2 + 1/15) = 40 shards at speed 0.15 (fall from before the movement: 4 blocks); fall
/// in a single movement: no dust.
#[test]
fn player_block_sound_and_dust() {
    let mut w = world("sound-dust");
    let grass = w.p.s.st([8, Y - 1, 8]);
    land(&mut w, 4.0, 1.0);
    let evs = w.p.take_events();
    assert_eq!(w.p.s.life.vit.health, 18.0);
    assert_eq!(sounds(&w, &evs, "minecraft:block.grass.fall"), vec![(0.5, 0.75)], "block sound missing");
    let d = dust(&w, &evs);
    assert_eq!(d, vec![(grass as i32, 40, [8.5, f64::from(Y), 8.5], [0.15; 3])], "dust missing or wrong");
    land(&mut w, 0.0, 5.0);
    let evs = w.p.take_events();
    assert!(dust(&w, &evs).is_empty(), "dust on the fall of the last movement");
}

/// Dust brought back to the edge of the effect block when the entity is not above it: player at
/// (9.1, 8.5) on block (8, 8) → x = 9.0.
#[test]
fn dust_at_block_edge() {
    let w = world("edge");
    let _ = w.p.take_events();
    w.p.s.fall_dust([9.1, f64::from(Y), 8.5], [8, Y - 1, 8], 4.0, 3.0);
    let evs = w.p.take_events();
    let d = dust(&w, &evs);
    assert_eq!(d.len(), 1);
    assert!((d[0].2[0] - 9.0).abs() < 1e-12 && (d[0].2[2] - 8.5).abs() < 1e-12, "{:?}", d[0].2);
    // On air: nothing.
    w.p.s.fall_dust([9.1, f64::from(Y), 8.5], [8, Y, 8], 4.0, 3.0);
    assert!(dust(&w, &w.p.take_events()).is_empty());
}

/// Mob landing after 5 blocks: 150 × (0.2 + 2/15) = 50 shards.
#[test]
fn mob_dust() {
    let mut w = world("mob-dust");
    let grass = w.p.s.st([8, Y - 1, 8]);
    let mut m = w.p.s.new_mob(Kind::Zombie, [8.5, f64::from(Y) + 0.5, 8.5], 0.0);
    m.fall_distance = 5.0;
    let _ = w.p.take_events();
    w.p.s.mob_move(&mut m, [0.0, -1.0, 0.0]);
    assert!(m.on_ground);
    let evs = w.p.take_events();
    let d = dust(&w, &evs);
    assert_eq!(d.len(), 1, "mob dust missing");
    assert_eq!((d[0].0, d[0].1, d[0].3), (grass as i32, 50, [0.15; 3]));
}
