//! G4-F07b: saddled pig steered with a carrot on a stick (saddle, mount, steering, boost, wear,
//! dismount onto a neighbouring cell); llama spit (shared projectile margin, leaving the
//! llama through the stretched and inflated box, smoothed rotation).

use rc_client::Event;
use rc_proto_v777::entity::Meta;
use rc_proto_v777::play::SoundRef;

use super::jrand::JRand;
use super::mob::{Ent, Mob};
use super::mounts::SADDLE;
use super::mounts::llama::Spit;
use super::species::Kind;
use crate::blocks::probe::{Probe, state_string};
use crate::player::Stack;
use crate::server::Server;

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
    assert_eq!(state_string(&p.s, p.s.get([216, -60, 216])), "minecraft:stone", "zone 200..231 missing from the copy");
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    Some((p, dir))
}

fn cleanup(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

fn bare(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let m = p.s.new_mob(k, pos, 0.0);
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn mob(p: &Probe, id: i32) -> &Mob {
    p.s.mob_ref(id).expect("mob")
}

fn tick_one(p: &mut Probe, id: i32) {
    let i = p.s.mob_index(id).expect("mob");
    let mut m = p.s.mobs.list.take(i).expect("mob");
    p.s.tick_mob(&mut m);
    p.s.mobs.list.put(i, m);
}

fn sounds(p: &Probe, events: &[Event]) -> Vec<String> {
    let reg = p.s.data.registry("minecraft:sound_event").expect("sound registry");
    events
        .iter()
        .filter_map(|e| match e {
            Event::Sound(s) => Some(match &s.sound {
                SoundRef::Id(i) => reg.entries.get(*i as usize).cloned().unwrap_or_default(),
                SoundRef::Named(n) => n.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn stack(id: &str, count: i32) -> Option<Stack> {
    Some(Stack { id: id.into(), count, components: None })
}

/// Saddle fitted (pig sound), mount, carrot in hand: the player steers; boost of 140 to 980
/// ticks (index 18), stick worn by 7; second use during the boost: no boost, "used"
/// statistic. Without the carrot, the pig is no longer steered.
#[test]
fn saddled_pig_steered_with_carrot() {
    let Some((mut p, dir)) = probe("g4f07b-pig") else { return };
    let pig = bare(&mut p, Kind::Pig, [216.5, -59.0, 206.5]);
    let slot = 36 + p.s.held as usize;
    p.s.slots[slot] = stack("minecraft:saddle", 2);
    let _ = p.take_events();
    assert!(p.s.player_interact(pig, 0), "saddle refused");
    assert!(mob(&p, pig).equipment[SADDLE].is_some(), "saddle not fitted");
    assert_eq!(p.s.slots[slot].as_ref().map(|s| s.count), Some(1), "one saddle removed");
    let evs = p.take_events();
    assert!(sounds(&p, &evs).iter().any(|n| n == "minecraft:entity.pig.saddle"), "pig saddle sound");

    p.s.slots[slot] = stack("minecraft:carrot_on_a_stick", 1);
    assert!(p.s.player_interact(pig, 0), "mount refused");
    assert_eq!(p.s.mobs.mounts.riding.vehicle, Some(pig), "player not mounted");
    tick_one(&mut p, pig);
    assert!(Server::player_controls(mob(&p, pig)), "pig not steered with the carrot");

    p.s.on_use_item(0);
    let total = mob(&p, pig).neth.boost_total;
    assert!((140..=980).contains(&total), "no boost: {total}");
    let st = p.s.slots[slot].clone().expect("stick");
    assert_eq!(p.s.life.items.damage(&st), 7, "carrot on a stick wear");
    let mut m = p.s.mob_ref(pig).cloned().expect("pig");
    m.sync_data();
    assert_eq!(m.get(18), Some(&Meta::Int(total)), "boost duration synced");
    assert_eq!(p.s.prog.stats.get("minecraft:used", "minecraft:carrot_on_a_stick"), 0);
    p.s.on_use_item(0);
    assert_eq!(mob(&p, pig).neth.boost_total, total, "boost restarted during the boost");
    assert_eq!(p.s.prog.stats.get("minecraft:used", "minecraft:carrot_on_a_stick"), 1, "\"used\" statistic");

    p.s.slots[slot] = None;
    tick_one(&mut p, pig);
    assert!(!Server::player_controls(mob(&p, pig)), "steered without the carrot");
    cleanup(p, dir);
}

/// The stick breaks on the last use: a new fishing rod.
#[test]
fn carrot_on_a_stick_breaks() {
    let Some((mut p, dir)) = probe("g4f07b-break") else { return };
    let pig = bare(&mut p, Kind::Pig, [216.5, -59.0, 206.5]);
    let i = p.s.mob_index(pig).unwrap();
    p.s.mobs.list.get_mut(i).unwrap().equipment[SADDLE] = stack("minecraft:saddle", 1);
    let slot = 36 + p.s.held as usize;
    let mut st = Stack { id: "minecraft:carrot_on_a_stick".into(), count: 1, components: None };
    crate::items::set_component(&mut st, "minecraft:damage", rc_proto::nbt::Nbt::Int(20));
    p.s.slots[slot] = Some(st);
    assert!(p.s.player_interact(pig, 0), "mount refused");
    tick_one(&mut p, pig);
    p.s.on_use_item(0);
    let st = p.s.slots[slot].clone().expect("fishing rod");
    assert_eq!(st.id, "minecraft:fishing_rod");
    assert_eq!(p.s.life.items.damage(&st), 0);
    cleanup(p, dir);
}

/// Dismounting a pig facing south: right cell (west) at the centre, otherwise the left one;
/// same rule for the chicken and the nautiluses (animals without a rule of their own). A spider
/// or a ravager (not animals): top of the box.
#[test]
fn pig_dismounts_onto_neighbouring_cell() {
    let Some((mut p, dir)) = probe("g4f07b-dismount") else { return };
    let poses = [(0.6, 1.8), (0.6, 1.5), (0.6, 0.6)];
    for k in [Kind::Spider, Kind::Ravager] {
        let id = bare(&mut p, k, [216.5, -59.0, 206.5]);
        let v = p.s.mob_ref(id).cloned().unwrap();
        let top = v.bbox().max[1];
        assert_eq!(p.s.dismount_location(&v, 0.6, &poses), [216.5, top, 206.5], "{k:?}: top of the box");
        let i = p.s.mob_index(id).unwrap();
        p.s.mobs.list.take(i);
    }
    for k in [Kind::Pig, Kind::Chicken, Kind::Nautilus, Kind::ZombieNautilus] {
        let id = bare(&mut p, k, [216.5, -59.0, 206.5]);
        let v = p.s.mob_ref(id).cloned().unwrap();
        assert_eq!(p.s.dismount_location(&v, 0.6, &poses), [215.5, -59.0, 206.5], "{k:?}: west cell");
        p.run("setblock 215 -59 206 minecraft:stone\nsetblock 215 -58 206 minecraft:stone\n");
        assert_eq!(p.s.dismount_location(&v, 0.6, &poses), [217.5, -59.0, 206.5], "{k:?}: east cell");
        p.run("setblock 215 -59 206 minecraft:air\nsetblock 215 -58 206 minecraft:air\n");
        let i = p.s.mob_index(id).unwrap();
        p.s.mobs.list.take(i);
    }
    cleanup(p, dir);
}

fn spit(owner: i32, pos: [f64; 3], vel: [f64; 3], tick: i32) -> Spit {
    Spit {
        id: 990_000 + tick,
        owner,
        pos,
        vel,
        yaw: 0.0,
        pitch: 0.0,
        rng: JRand::new(7),
        removed: false,
        left_owner: true,
        tick,
        rank: 0,
    }
}

/// Shared margin: on the first tick (margin 0), a spit grazing a cow at 0.2 does not hit it;
/// on the 10th tick (margin capped at 0.3), it does.
#[test]
fn spit_shared_margin() {
    let Some((mut p, dir)) = probe("g4f07b-margin") else { return };
    let llama = bare(&mut p, Kind::Llama, [205.5, -59.0, 205.5]);
    let cow = bare(&mut p, Kind::Cow, [216.5, -59.0, 210.5]);
    let max = mob(&p, cow).health;
    let half = f64::from(mob(&p, cow).width()) / 2.0;
    let x = 216.5 + half + 0.2;
    p.s.mobs.mounts.spits.push(spit(llama, [x, -58.5, 208.0], [0.0, 0.0, 3.0], 0));
    p.s.spits_tick();
    assert_eq!(mob(&p, cow).health, max, "hit without margin");
    p.s.mobs.mounts.spits.clear();
    p.s.mobs.mounts.spits.push(spit(llama, [x, -58.5, 208.0], [0.0, 0.0, 3.0], 9));
    p.s.spits_tick();
    assert!(mob(&p, cow).health < max, "not hit with the 0.3 margin");
    cleanup(p, dir);
}

/// Leaving the llama: a spit 0.5 from the llama's box has not left it yet (box stretched and
/// inflated by 1); rotation smoothed towards the velocity (a fifth of the difference per tick).
#[test]
fn spit_exit_and_rotation() {
    let Some((mut p, dir)) = probe("g4f07b-exit") else { return };
    let llama = bare(&mut p, Kind::Llama, [210.5, -59.0, 210.5]);
    let half = f64::from(mob(&p, llama).width()) / 2.0;
    let mut s = spit(llama, [210.5 + half + 0.5 + 0.125, -58.0, 210.5], [0.5, 0.0, 0.0], 0);
    s.left_owner = false;
    p.s.mobs.mounts.spits.push(s);
    p.s.spits_tick();
    let s = p.s.mobs.mounts.spits.first().expect("spit removed");
    assert!(!s.left_owner, "left the llama too early");
    assert!((s.yaw - 18.0).abs() < 1.0e-3, "rotation not smoothed: {}", s.yaw);
    assert_eq!(s.pitch, 0.0);
    cleanup(p, dir);
}

/// Until the spit has left, neither the llama nor its passenger (a chicken sitting on it) is
/// hit, even when its path crosses the passenger.
#[test]
fn spit_spares_the_llama_passenger() {
    let Some((mut p, dir)) = probe("g4f07b-passenger") else { return };
    let llama = bare(&mut p, Kind::Llama, [210.5, -59.0, 210.5]);
    let hen = bare(&mut p, Kind::Chicken, [212.5, -59.0, 210.5]);
    let (il, ih) = (p.s.mob_index(llama).unwrap(), p.s.mob_index(hen).unwrap());
    let mut l = p.s.mobs.list.take(il).unwrap();
    let mut h = p.s.mobs.list.take(ih).unwrap();
    assert!(p.s.mob_start_riding(&mut h, &mut l, true), "chicken not mounted");
    p.s.mobs.list.put(il, l);
    p.s.mobs.list.put(ih, h);
    let seat = mob(&p, hen).pos;
    let (lmax, hmax) = (mob(&p, llama).health, mob(&p, hen).health);
    let y = seat[1] + f64::from(mob(&p, hen).height()) / 2.0;
    let mut s = spit(llama, [seat[0] - 1.0, y, seat[2]], [0.8, 0.0, 0.0], 5);
    s.left_owner = false;
    p.s.mobs.mounts.spits.push(s);
    p.s.spits_tick();
    assert!(!p.s.mobs.mounts.spits.first().expect("spit").left_owner, "left too early");
    assert_eq!(mob(&p, hen).health, hmax, "llama passenger hit before the spit left");
    assert_eq!(mob(&p, llama).health, lmax, "llama hit before the spit left");
    cleanup(p, dir);
}

/// The target kept is the nearest by its entry point: a wide golem whose face is nearer than a
/// chicken whose centre is nearer.
#[test]
fn spit_targets_nearest_at_entry() {
    let Some((mut p, dir)) = probe("g4f07b-entry") else { return };
    let llama = bare(&mut p, Kind::Llama, [205.5, -59.0, 220.5]);
    let hen = bare(&mut p, Kind::Chicken, [212.0, -59.0, 210.5]);
    let golem = bare(&mut p, Kind::IronGolem, [212.2, -59.0, 211.0]);
    let (hmax, gmax) = (mob(&p, hen).health, mob(&p, golem).health);
    p.s.mobs.mounts.spits.push(spit(llama, [210.0, -58.7, 210.5], [3.0, 0.0, 0.0], 0));
    p.s.spits_tick();
    assert!(mob(&p, golem).health < gmax, "golem (nearest entry) spared");
    assert_eq!(mob(&p, hen).health, hmax, "chicken (nearest centre) hit");
    cleanup(p, dir);
}

// ---------- Birth during the entity walk (complete server ticks) ----------

/// Complete server tick, like the server loop (events drained).
fn full_tick(p: &mut Probe) {
    p.s.game_tick();
    p.s.tick += 1;
    let _ = p.take_events();
}

/// Complete server ticks until `seen` answers, at most `max`; returns its answer.
fn until<T>(p: &mut Probe, max: usize, what: &str, mut seen: impl FnMut(&Server) -> Option<T>) -> T {
    for _ in 0..max {
        full_tick(p);
        if let Some(v) = seen(&p.s) {
            return v;
        }
    }
    panic!("{what}: nothing in {max} server ticks");
}

/// Minecart placed at `pos`, the mob seated in it.
fn seat_in_cart(p: &mut Probe, id: i32, pos: [f64; 3]) {
    p.s.next_entity += 1;
    let mut c = crate::minecart::Cart::new(crate::minecart::CKind::Rideable, p.s.next_entity, pos);
    let i = p.s.mob_index(id).expect("mob");
    let mut m = p.s.mobs.list.take(i).expect("mob");
    assert!(p.s.cart_mob_start_riding(&mut c, &mut m), "mob not mounted in the minecart");
    p.s.mobs.list.put(i, m);
    p.s.cart_add(c);
}

fn set_target(p: &mut Probe, id: i32, t: Option<Ent>) {
    let i = p.s.mob_index(id).expect("mob");
    p.s.mobs.list.get_mut(i).expect("mob").target = t;
}

fn proj_ticks(p: &Probe, id: i32) -> Option<i32> {
    p.s.proj.list.iter().find(|q| q.id == id).map(|q| q.tick_count)
}

fn cloud_age(p: &Probe, id: i32) -> Option<i32> {
    p.s.proj.clouds.iter().find(|c| c.id == id).map(|c| c.age)
}

/// A spit born during the entity walk makes its first tick on the next server tick, whether the
/// llama is free (ticked before the spits) or a passenger of a minecart (ticked after them,
/// with the minecarts): never on the tick of its birth, never two ticks later.
#[test]
fn spit_first_tick_on_next_server_tick() {
    let Some((mut p, dir)) = probe("g4f07b-spit-tick") else { return };
    let cow = bare(&mut p, Kind::Cow, [216.5, -59.0, 222.5]);
    let free = bare(&mut p, Kind::Llama, [210.5, -59.0, 210.5]);
    let carried = bare(&mut p, Kind::Llama, [222.5, -59.0, 210.5]);
    seat_in_cart(&mut p, carried, [222.5, -59.0, 210.5]);
    for (who, id) in [("free llama", free), ("llama in a minecart", carried)] {
        set_target(&mut p, id, Some(Ent::Mob(cow)));
        let sid = until(&mut p, 200, who, |s| s.mobs.mounts.spits.iter().find(|x| x.owner == id).map(|x| x.id));
        let tick = |p: &Probe| p.s.mobs.mounts.spits.iter().find(|x| x.id == sid).map(|x| x.tick);
        assert_eq!(tick(&p), Some(0), "{who}: spit advanced on the tick of its birth");
        full_tick(&mut p);
        assert_eq!(tick(&p), Some(1), "{who}: spit still on the next tick");
        set_target(&mut p, id, None);
    }
    cleanup(p, dir);
}

/// Next permitted use of an evoker spell (`i32::MAX`: never).
fn spell_next(p: &mut Probe, id: i32, spell: super::illagers::Spell, at: i32) {
    let i = p.s.mob_index(id).expect("evoker");
    let m = p.s.mobs.list.get_mut(i).expect("evoker");
    for w in &mut m.goals.goals {
        if let super::goals::Goal::Ill(super::illagers::IGoal::UseSpell { spell: s, next_tick, .. }) = &mut w.goal
            && *s == spell
        {
            *next_tick = at;
        }
    }
}

/// Evoker fangs, same rule: first tick on the next server tick, evoker free or passenger of a
/// minecart (vexes disabled to keep only the fangs, one evoker at a time; player in creative,
/// target fixed).
#[test]
fn fangs_first_tick_on_next_server_tick() {
    use super::illagers::Spell;
    let Some((mut p, dir)) = probe("g4f07b-fangs-tick") else { return };
    p.s.game_mode = 1;
    let free = bare(&mut p, Kind::Evoker, [216.5, -59.0, 212.5]);
    let carried = bare(&mut p, Kind::Evoker, [226.5, -59.0, 212.5]);
    seat_in_cart(&mut p, carried, [226.5, -59.0, 212.5]);
    for id in [free, carried] {
        spell_next(&mut p, id, Spell::SummonVex, i32::MAX);
        spell_next(&mut p, id, Spell::Fangs, i32::MAX);
        set_target(&mut p, id, Some(Ent::Player));
    }
    for (who, id) in [("free evoker", free), ("evoker in a minecart", carried)] {
        spell_next(&mut p, id, Spell::Fangs, 0);
        let fid = until(&mut p, 300, who, |s| s.mobs.ill.fangs.iter().find(|f| f.owner == Some(id)).map(|f| f.id));
        let warm = |p: &Probe| p.s.mobs.ill.fangs.iter().find(|f| f.id == fid).map(|f| f.warmup);
        assert_eq!(warm(&p), Some(0), "{who}: fangs advanced on the tick of their birth");
        full_tick(&mut p);
        assert_eq!(warm(&p), Some(-1), "{who}: fangs still on the next tick");
        spell_next(&mut p, id, Spell::Fangs, i32::MAX);
    }
    cleanup(p, dir);
}

/// A creeper under an effect that explodes during its tick: its cloud has not aged at the end of
/// the explosion tick (age 0), it is 1 on the next; wait, effects and end therefore fall on
/// their own tick.
#[test]
fn creeper_cloud_ages_on_next_tick() {
    let Some((mut p, dir)) = probe("g4f07b-creeper-cloud") else { return };
    p.s.game_mode = 1;
    let mut m = p.s.new_mob(Kind::Creeper, [216.5, -59.0, 218.5], 0.0);
    assert!(p.s.give_mob_effect(&mut m, "minecraft:speed", 600, 0), "effect refused");
    m.ignited = true;
    p.s.add_mob(m);
    let id = until(&mut p, 100, "explosion", |s| s.proj.clouds.first().map(|c| c.id));
    assert_eq!(cloud_age(&p, id), Some(0), "creeper cloud aged on the tick of its birth");
    full_tick(&mut p);
    assert_eq!(cloud_age(&p, id), Some(1));
    cleanup(p, dir);
}

/// A lingering potion that hits the ground during the projectile tick: its cloud does not age
/// there either (age 0 at the end of the impact tick).
#[test]
fn potion_cloud_ages_on_next_tick() {
    let Some((mut p, dir)) = probe("g4f07b-potion-cloud") else { return };
    let mut item = Stack { id: "minecraft:lingering_potion".into(), count: 1, components: None };
    let contents =
        rc_proto::nbt::Nbt::Compound(vec![("potion".into(), rc_proto::nbt::Nbt::String("minecraft:poison".into()))]);
    crate::items::set_component(&mut item, "minecraft:potion_contents", contents);
    let pid = p.s.proj_new_id();
    let mut q =
        crate::projectiles::Proj::new(crate::projectiles::PKind::LingeringPotion, pid, [216.5, -57.5, 214.5], item);
    q.vel = [0.0, -0.6, 0.0];
    p.s.proj_add(q);
    let id = until(&mut p, 40, "impact", |s| s.proj.clouds.first().map(|c| c.id));
    assert_eq!(cloud_age(&p, id), Some(0), "potion cloud aged on the tick of its birth");
    full_tick(&mut p);
    assert_eq!(cloud_age(&p, id), Some(1));
    cleanup(p, dir);
}

/// Arrow summoned by a command block minecart on a powered activator rail (command run during
/// the minecart's tick, hence during the walk): first tick on the next tick.
#[test]
fn command_minecart_arrow_on_next_tick() {
    let Some((mut p, dir)) = probe("g4f07b-command") else { return };
    p.run("setblock 216 -60 214 minecraft:redstone_block\nsetblock 216 -59 214 minecraft:activator_rail[powered=true,shape=north_south]\n");
    let src = p.s.server_source();
    let cmd = "summon minecraft:command_block_minecart 216.5 -59 214.5 \
               {Command:\"summon minecraft:arrow ~ ~3 ~ {Motion:[0.0d,0.0d,0.3d]}\"}";
    let (ok, _) = p.s.perform(src, cmd);
    assert!(ok, "command block minecart not summoned");
    let kind = crate::projectiles::PKind::Arrow;
    let aid = until(&mut p, 20, "minecart arrow", |s| s.proj.list.iter().find(|q| q.kind == kind).map(|q| q.id));
    assert_eq!(proj_ticks(&p, aid), Some(0), "minecart arrow advanced on the tick of its birth");
    full_tick(&mut p);
    assert_eq!(proj_ticks(&p, aid), Some(1));
    cleanup(p, dir);
}

/// Guard: what is born before the entity walk ticks within the same server tick, as in the game.
/// Arrow added between two ticks (player, typed command) and dispenser arrow (block tick):
/// first tick right away.
#[test]
fn outside_the_walk_first_tick_right_away() {
    let Some((mut p, dir)) = probe("g4f07b-outside-walk") else { return };
    let ammo = Stack { id: "minecraft:arrow".into(), count: 1, components: None };
    let mut a = p.s.new_arrow([210.5, -57.0, 215.5], &ammo, None);
    a.vel = [0.3, 0.0, 0.0];
    let aid = a.id;
    p.s.proj_add(a);
    full_tick(&mut p);
    assert_eq!(proj_ticks(&p, aid), Some(1), "arrow added before the tick kept waiting");
    p.run(
        "setblock 220 -59 214 minecraft:dispenser[facing=up]\n\
         data merge block 220 -59 214 {Items:[{Slot:0b,id:\"minecraft:arrow\",count:1}]}\n\
         setblock 221 -59 214 minecraft:redstone_block\n",
    );
    let kind = crate::projectiles::PKind::Arrow;
    let did = until(&mut p, 20, "dispenser shot", |s| {
        s.proj.list.iter().find(|q| q.id != aid && q.kind == kind).map(|q| q.id)
    });
    assert_eq!(proj_ticks(&p, did), Some(1), "dispenser arrow kept waiting");
    cleanup(p, dir);
}

/// Player's pearl read back from its file (as on entry, before any walk): it ticks on the very
/// tick it is put back, although the put-back falls during the entity walk.
#[test]
fn reloaded_pearl_ticks_right_away() {
    let Some((mut p, dir)) = probe("g4f07b-pearl") else { return };
    let item = Stack { id: "minecraft:ender_pearl".into(), count: 1, components: None };
    let id = p.s.proj_new_id();
    let kind = crate::projectiles::PKind::EnderPearl;
    let mut pearl = crate::projectiles::Proj::new(kind, id, [210.5, -50.0, 215.5], item);
    pearl.owner = Some(p.s.uuid);
    pearl.vel = [0.2, 0.0, 0.0];
    let nbt = p.s.proj_to_nbt(&pearl);
    p.s.proj_player.pearls_read = true;
    p.s.proj_player.pending_pearls = vec![nbt];
    full_tick(&mut p);
    let back = p.s.proj.list.iter().find(|q| q.kind == kind).expect("pearl not put back");
    assert_eq!(back.tick_count, 1, "reloaded pearl kept waiting for a tick");
    cleanup(p, dir);
}

/// Frozen game, the player mounted on `id` (the only mob that still ticks): the entity that
/// `born` finds is born during a frozen tick, does not tick while frozen (`ticks` at 0), then
/// makes its first tick on the first thawed tick, not one more.
fn frozen_birth(
    p: &mut Probe,
    who: &str,
    id: i32,
    born: impl Fn(&Server) -> Option<i32>,
    ticks: impl Fn(&Server, i32) -> Option<i32>,
) {
    let i = p.s.mob_index(id).expect("mount");
    let mut v = p.s.mobs.list.take(i).expect("mount");
    assert!(p.s.player_start_riding(&mut v), "{who}: player not mounted");
    p.s.mobs.list.put(i, v);
    p.s.player_command("tick freeze");
    assert!(p.s.cmd.ticking.frozen(), "game not frozen");
    let e = until(p, 200, who, &born);
    for _ in 0..3 {
        full_tick(p);
    }
    assert_eq!(ticks(&p.s, e), Some(0), "{who}: ticked while frozen");
    p.s.player_command("tick unfreeze");
    full_tick(p);
    assert_eq!(ticks(&p.s, e), Some(1), "{who}: first tick missed on thaw");
    if p.s.mobs.mounts.riding.vehicle.is_some() {
        p.s.player_stop_riding();
    }
}

/// Frozen game: the player's mount still ticks. A spit (mounted llama), a snowball (mounted snow
/// golem) and the cloud of a creeper under an effect that explodes (mounted with `/ride`) born
/// during a frozen tick make their first tick on the first thawed tick.
#[test]
fn freeze_then_thaw_first_tick_on_thaw() {
    let Some((mut p, dir)) = probe("g4f07b-freeze") else { return };
    p.s.cmd.op = 4;
    p.s.game_mode = 1;
    let cow = bare(&mut p, Kind::Cow, [216.5, -59.0, 224.5]);
    let llama = bare(&mut p, Kind::Llama, [212.5, -59.0, 210.5]);
    let golem = bare(&mut p, Kind::SnowGolem, [220.5, -59.0, 210.5]);
    let i = p.s.mob_index(llama).unwrap();
    p.s.mobs.list.get_mut(i).unwrap().mount.set_flag(super::mounts::FLAG_TAME, true);
    set_target(&mut p, llama, Some(Ent::Mob(cow)));
    let spit = |s: &Server, e: i32| s.mobs.mounts.spits.iter().find(|x| x.id == e).map(|x| x.tick);
    frozen_birth(&mut p, "spit", llama, |s| s.mobs.mounts.spits.first().map(|x| x.id), spit);
    set_target(&mut p, llama, None);
    set_target(&mut p, golem, Some(Ent::Mob(cow)));
    let snow = crate::projectiles::PKind::Snowball;
    let ball = |s: &Server| s.proj.list.iter().find(|q| q.kind == snow).map(|q| q.id);
    frozen_birth(&mut p, "snowball", golem, ball, |s, e| {
        s.proj.list.iter().find(|q| q.id == e).map(|q| q.tick_count)
    });
    set_target(&mut p, golem, None);
    let mut c = p.s.new_mob(Kind::Creeper, [216.5, -59.0, 214.5], 0.0);
    assert!(p.s.give_mob_effect(&mut c, "minecraft:speed", 600, 0), "effect refused");
    c.ignited = true;
    let creeper = c.id;
    p.s.add_mob(c);
    let age = |s: &Server, e: i32| s.proj.clouds.iter().find(|c| c.id == e).map(|c| c.age);
    frozen_birth(&mut p, "cloud", creeper, |s| s.proj.clouds.first().map(|c| c.id), age);
    cleanup(p, dir);
}
