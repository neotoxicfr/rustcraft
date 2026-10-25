//! G4-F07a: simulated lead (tie, untie, spring pull, snap, fence knot, transfer, shears, saving)
//! and trader llamas held by their trader.
//! Fresh flat world: the 26.3 cache and the client jar are required (the test FAILS without them).

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::Event;
use rc_proto_v777::play::SoundRef;

use super::leash::{self, Anchor, Holder};
use super::mob::Mob;
use super::species::Kind;
use crate::blocks::probe::Probe;
use crate::player::Stack;

struct W {
    p: Probe,
    dir: PathBuf,
}

impl Drop for W {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const Y: i32 = -60;

fn world(tag: &str) -> W {
    let data = rc_testdata::version_data().expect("G4-F07a: 26.3 data cache missing");
    let data = Arc::new(data);
    assert!(crate::registries::client_jar(&data).is_some(), "G4-F07a: 26.3 client jar missing");
    let dir = std::env::temp_dir().join(format!("rc-g4f07a-{tag}-{}", std::process::id()));
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
        assert!(t.elapsed().as_secs() < 120, "G4-F07a: flat world generation took too long");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    p.run(&format!("fill -8 {Y} -8 15 {} 15 minecraft:air\n", Y + 3));
    p.s.pos = [0.5, f64::from(Y), 0.5];
    p.s.yaw = 0.0;
    p.s.on_ground = true;
    p.s.game_mode = 0;
    p.s.sneaking = false;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    p.s.proj_player.known_move = [0.0; 3];
    for i in 0..p.s.slots.len() {
        p.s.slots[i] = None;
    }
    p.s.life.drops.clear();
    let _ = p.take_events();
    W { p, dir }
}

impl W {
    fn spawn(&mut self, k: Kind, pos: [f64; 3]) -> i32 {
        let mut m = self.p.s.new_mob(k, pos, 0.0);
        m.track.seen = true;
        let id = m.id;
        self.p.s.add_mob(m);
        id
    }

    fn mob(&self, id: i32) -> &Mob {
        self.p.s.mobs.get(id).expect("mob")
    }

    /// One lead tick of mob `id` (taken out of the list for the duration of the tick).
    fn leash_tick(&mut self, id: i32) {
        let i = self.p.s.mob_index(id).expect("mob");
        let mut m = self.p.s.mobs.list[i].take().expect("mob");
        self.p.s.leash_tick(&mut m);
        self.p.s.mobs.list[i] = Some(m);
    }

    fn hold(&mut self, id: &str, n: i32) {
        let h = 36 + self.p.s.held as usize;
        self.p.s.slots[h] = Some(Stack { id: id.to_owned(), count: n, components: None });
    }

    fn held_count(&self) -> i32 {
        self.p.s.slots[36 + self.p.s.held as usize].as_ref().map_or(0, |s| s.count)
    }

    fn leads_dropped(&self) -> i32 {
        self.p.s.life.drops.iter().filter(|d| d.stack.id == leash::LEAD && !d.is_removed()).map(|d| d.stack.count).sum()
    }

    fn sounds(&self, evs: &[Event], name: &str) -> usize {
        let id = self.p.s.sound_id(name).expect("known sound");
        evs.iter().filter(|e| matches!(e, Event::Sound(s) if matches!(s.sound, SoundRef::Id(i) if i == id))).count()
    }

    fn links(evs: &[Event]) -> Vec<(i32, i32)> {
        evs.iter()
            .filter_map(|e| match e {
                Event::Misc(rc_client::misc::Misc::Link { held, holder }) => Some((*held, *holder)),
                _ => None,
            })
            .collect()
    }
}

/// Lead in hand on a cow: tied to the player (item consumed, sound, network link); the second
/// click unties it (item given back on the ground, null link); a zombie refuses it.
#[test]
fn tie_and_untie_with_lead() {
    let mut w = world("tie");
    let cow = w.spawn(Kind::Cow, [2.5, f64::from(Y), 0.5]);
    w.hold(leash::LEAD, 2);
    assert!(w.p.s.player_interact(cow, 0), "click with the lead refused");
    let evs = w.p.take_events();
    assert_eq!(leash::holder(w.mob(cow)), Some(Holder::Player));
    assert_eq!(w.held_count(), 1, "lead not consumed");
    assert_eq!(w.sounds(&evs, "minecraft:item.lead.tied"), 1);
    assert_eq!(W::links(&evs), vec![(cow, w.p.s.player_id)]);
    assert!(w.mob(cow).requires_custom_persistence(), "held animal not persistent");
    // Untie: the lead falls on the ground.
    assert!(w.p.s.player_interact(cow, 0));
    let evs = w.p.take_events();
    assert!(leash::holder(w.mob(cow)).is_none());
    assert_eq!(w.leads_dropped(), 1, "detached lead not given back");
    assert_eq!(w.sounds(&evs, "minecraft:item.lead.untied"), 1);
    assert_eq!(W::links(&evs), vec![(cow, 0)]);
    // Hostile: refused, the item stays.
    let z = w.spawn(Kind::Zombie, [0.5, f64::from(Y), 2.5]);
    w.p.s.player_interact(z, 0);
    assert!(leash::holder(w.mob(z)).is_none(), "zombie held on a lead");
    assert_eq!(w.held_count(), 1);
}

/// Beyond 12 blocks (centre to centre): snap, sound at the holder, lead on the ground. Between 6
/// and 12: damped spring (game values: 0.8/0.2/0.8, ×0.3 along the movement, torque ×10,
/// damping 0.91 in the air).
#[test]
fn pull_then_snap() {
    let mut w = world("pull");
    let x = 8.5;
    let cow = w.spawn(Kind::Cow, [x, f64::from(Y), 0.5]);
    let u = w.p.s.uuid;
    {
        let i = w.p.s.mob_index(cow).unwrap();
        let mut m = w.p.s.mobs.list[i].take().unwrap();
        m.on_ground = false;
        w.p.s.leash_set(&mut m, Holder::Player, Anchor::Uuid(u), false);
        w.p.s.mobs.list[i] = Some(m);
    }
    w.leash_tick(cow);
    // Point of the cow (0; 0.5 h; 0.5 w) and of the player (0; 0.5 h; 0), null yaws.
    let (cw, ch) = (f64::from(0.9f32), f64::from(1.4f32));
    let ep = [x, f64::from(Y) + 0.5 * ch, 0.5 + 0.5 * cw];
    let hp = [0.5, f64::from(Y) + 0.5 * f64::from(1.8f32), 0.5];
    let d = [hp[0] - ep[0], hp[1] - ep[1], hp[2] - ep[2]];
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let disp = d.map(|c| c / dist * (dist - 6.0) * f64::from(0.3f32));
    let want = [disp[0] * 0.8, disp[1] * 0.2, disp[2] * 0.8];
    let m = w.mob(cow);
    for (a, (got, exp)) in m.vel.iter().zip(want).enumerate() {
        assert!((got - exp).abs() < 1e-12, "velocity {a}: {got} instead of {exp}");
    }
    // Torque taken on the displacement before the ×0.3 reduction (game order).
    let torque = 0.5 * cw * (d[0] / dist * (dist - 6.0));
    let yaw = (0.0 - 10.0 * torque) as f32;
    assert_eq!(m.yaw, yaw, "yaw turned by the torque");
    let ang = m.leash.as_ref().unwrap().angular;
    assert!((ang - 10.0 * torque * f64::from(0.91f32)).abs() < 1e-12, "momentum not damped by 0.91");
    assert_eq!(leash::home(m), Some(([0, Y, 0], 5)), "home around the holder");
    // Snap.
    let i = w.p.s.mob_index(cow).unwrap();
    w.p.s.mobs.list[i].as_mut().unwrap().pos = [13.0, f64::from(Y), 0.5];
    let _ = w.p.take_events();
    w.leash_tick(cow);
    let evs = w.p.take_events();
    assert!(leash::holder(w.mob(cow)).is_none(), "lead not snapped at 12.5 blocks");
    assert_eq!(w.sounds(&evs, "minecraft:item.lead.break"), 1);
    assert_eq!(w.leads_dropped(), 1);
    assert_eq!(w.mob(cow).goals.disabled & super::goals::MOVE, super::goals::MOVE, "movement not cut");
    assert!(leash::home(w.mob(cow)).is_none());
}

/// Click on a fence: knot created, the animal moves onto it; click on the knot: it goes back to
/// the player and the knot disappears; shears: lead cut. The knot is never saved and the
/// reloaded animal recreates it.
#[test]
fn fence_knot_transfer_and_shears() {
    let mut w = world("knot");
    let fence = [3, Y, 3];
    w.p.run(&format!("setblock {} {} {} minecraft:oak_fence\n", fence[0], fence[1], fence[2]));
    let cow = w.spawn(Kind::Cow, [2.5, f64::from(Y), 0.5]);
    w.hold(leash::LEAD, 1);
    assert!(w.p.s.player_interact(cow, 0));
    let _ = w.p.take_events();
    // Empty hand on the fence.
    w.p.s.slots[36 + w.p.s.held as usize] = None;
    let st = w.p.s.get(fence);
    assert!(w.p.s.block_use_without_item(fence, st, crate::blocks::Dir::Up, [0.5, 1.0, 0.5]));
    let evs = w.p.take_events();
    let knot = w.p.s.knot_at(fence).expect("knot missing");
    assert_eq!(leash::holder(w.mob(cow)), Some(Holder::Knot(knot)));
    assert_eq!(w.sounds(&evs, "minecraft:item.lead.tied"), 1);
    assert_eq!(W::links(&evs), vec![(cow, knot)]);
    let saved = w.mob(cow).save();
    assert_eq!(saved.get("leash"), Some(&rc_proto::nbt::Nbt::IntArray(fence.to_vec())), "block attachment badly written");
    // Reloaded: the knot is recreated on its block.
    let mut again = w.p.s.new_mob(Kind::Cow, [2.5, f64::from(Y), 1.5], 0.0);
    again.load(&saved);
    assert!(leash::holder(&again).is_none() && again.leash.is_some(), "reloaded attachment not deferred");
    let id2 = again.id;
    w.p.s.add_mob(again);
    w.p.s.deco_remove(knot);
    w.leash_tick(id2);
    let k2 = w.p.s.knot_at(fence).expect("knot not recreated on reload");
    assert_eq!(leash::holder(w.mob(id2)), Some(Holder::Knot(k2)));
    // Click on the knot: both animals go back to the player, the knot disappears.
    let _ = w.p.take_events();
    {
        let i = w.p.s.mob_index(cow).unwrap();
        let mut m = w.p.s.mobs.list[i].take().unwrap();
        w.p.s.leash_set(&mut m, Holder::Knot(k2), Anchor::Pos(fence), false);
        w.p.s.mobs.list[i] = Some(m);
    }
    assert!(w.p.s.knot_interact(k2, 36 + w.p.s.held as usize));
    assert_eq!(leash::holder(w.mob(cow)), Some(Holder::Player));
    assert_eq!(leash::holder(w.mob(id2)), Some(Holder::Player));
    assert!(w.p.s.knot_at(fence).is_none(), "empty knot not removed");
    // Shears: the lead falls, shears sound.
    w.hold("minecraft:shears", 1);
    let _ = w.p.take_events();
    assert!(w.p.s.player_interact(cow, 0));
    let evs = w.p.take_events();
    assert!(leash::holder(w.mob(cow)).is_none(), "shears had no effect");
    assert_eq!(w.sounds(&evs, "minecraft:item.shears.snip"), 1);
}

/// Sneaking, click on another animal: those the player holds move onto it; once the holder is
/// dead, the lead falls on the next tick.
#[test]
fn transfer_to_an_animal_and_dead_holder() {
    let mut w = world("transfer");
    let cow = w.spawn(Kind::Cow, [2.5, f64::from(Y), 0.5]);
    let horse = w.spawn(Kind::Horse, [0.5, f64::from(Y), 3.5]);
    w.hold(leash::LEAD, 1);
    assert!(w.p.s.player_interact(cow, 0));
    w.p.s.sneaking = true;
    w.p.s.slots[36 + w.p.s.held as usize] = None;
    assert!(w.p.s.player_interact(horse, 0), "sneaking transfer refused");
    assert_eq!(leash::holder(w.mob(cow)), Some(Holder::Mob(horse)));
    let hu = w.mob(horse).uuid;
    assert_eq!(w.mob(cow).leash.as_ref().unwrap().anchor, Anchor::Uuid(hu));
    w.p.s.mobs.get_mut(horse).unwrap().health = 0.0;
    w.leash_tick(cow);
    assert!(leash::holder(w.mob(cow)).is_none(), "lead kept by a dead holder");
    assert_eq!(w.leads_dropped(), 1);
}

/// Trader llama held by its trader: it leaves one tick before him, refuses the rider and
/// targets the trader's attacker.
#[test]
fn held_trader_llamas() {
    let mut w = world("trader");
    let trader = w.spawn(Kind::WanderingTrader, [0.5, f64::from(Y), 4.5]);
    let llama = w.spawn(Kind::TraderLlama, [2.5, f64::from(Y), 4.5]);
    let u = w.mob(trader).uuid;
    {
        let i = w.p.s.mob_index(llama).unwrap();
        let mut m = w.p.s.mobs.list[i].take().unwrap();
        w.p.s.leash_set(&mut m, Holder::Mob(trader), Anchor::Uuid(u), false);
        w.p.s.mobs.list[i] = Some(m);
    }
    if let Some(n) = w.p.s.mobs.get_mut(trader).unwrap().npc.as_mut() {
        n.despawn_delay = 300;
    }
    {
        let i = w.p.s.mob_index(llama).unwrap();
        let mut m = w.p.s.mobs.list[i].take().unwrap();
        super::mounts::llama::maybe_despawn(&mut w.p.s, &mut m);
        assert_eq!(m.mount.despawn_delay, 299, "delay not modelled on the trader's");
        // Mounting refused.
        w.p.s.slots[36 + w.p.s.held as usize] = None;
        let before = w.p.s.mobs.mounts.riding.vehicle;
        w.p.s.mobs.list[i] = Some(m);
        w.p.s.player_interact(llama, 0);
        assert_eq!(w.p.s.mobs.mounts.riding.vehicle, before, "trader llama mounted");
    }
    // Defense: a zombie hits the trader.
    let z = w.spawn(Kind::Zombie, [0.5, f64::from(Y), 7.5]);
    w.p.s.mobs.get_mut(trader).unwrap().last_hurt_by_mob = Some((super::mob::Ent::Mob(z), 40));
    let i = w.p.s.mob_index(llama).unwrap();
    let mut m = w.p.s.mobs.list[i].take().unwrap();
    let mut g = super::goals::mount_goals::MGoal::DefendTrader {
        base: super::goals::TBase::new(false, false),
        stamp: 0,
        by: None,
    };
    assert!(w.p.s.mount_can_use(&mut g, &mut m), "trader defense inactive");
    w.p.s.mount_start(&mut g, &mut m);
    assert_eq!(m.target, Some(super::mob::Ent::Mob(z)));
    assert!(!w.p.s.mount_can_use(&mut g, &mut m), "same hit targeted twice");
    // Delay elapsed: removed with its lead.
    if let Some(n) = w.p.s.mobs.get_mut(trader).unwrap().npc.as_mut() {
        n.despawn_delay = 1;
    }
    m.track.seen = true;
    let _ = w.p.take_events();
    super::mounts::llama::maybe_despawn(&mut w.p.s, &mut m);
    assert!(m.removed && m.leash.is_none());
    assert_eq!(W::links(&w.p.take_events()), vec![(llama, 0)]);
    w.p.s.mobs.list[i] = Some(m);
}

impl W {
    /// Ties `id` to holder `h` then plays one lead tick (home zone set).
    fn tie(&mut self, id: i32, h: Holder, anchor: Anchor) {
        let i = self.p.s.mob_index(id).expect("mob");
        let mut m = self.p.s.mobs.list[i].take().expect("mob");
        self.p.s.leash_set(&mut m, h, anchor, false);
        self.p.s.mobs.list[i] = Some(m);
        self.leash_tick(id);
    }
}

/// The lead's zone (radius 5 around the holder's block) counts as home for the goals: the
/// trader defense refuses an attacker whose block is at d² = 25 from the trader and accepts a
/// close attacker; a held bee does not go back to its flower.
#[test]
fn lead_home_for_goals() {
    let mut w = world("home");
    let trader = w.spawn(Kind::WanderingTrader, [0.5, f64::from(Y), 4.5]);
    let llama = w.spawn(Kind::TraderLlama, [2.5, f64::from(Y), 4.5]);
    let u = w.mob(trader).uuid;
    w.tie(llama, Holder::Mob(trader), Anchor::Uuid(u));
    assert_eq!(leash::home(w.mob(llama)), Some(([0, Y, 4], 5)));
    let far = w.spawn(Kind::Zombie, [4.5, f64::from(Y), 7.5]);
    let near = w.spawn(Kind::Zombie, [0.5, f64::from(Y), 6.5]);
    let i = w.p.s.mob_index(llama).unwrap();
    let mut m = w.p.s.mobs.list[i].take().unwrap();
    let mut g = super::goals::mount_goals::MGoal::DefendTrader {
        base: super::goals::TBase::new(false, false),
        stamp: 0,
        by: None,
    };
    w.p.s.mobs.get_mut(trader).unwrap().last_hurt_by_mob = Some((super::mob::Ent::Mob(far), 40));
    assert!(!w.p.s.mount_can_use(&mut g, &mut m), "attacker outside the lead's zone targeted");
    w.p.s.mobs.get_mut(trader).unwrap().last_hurt_by_mob = Some((super::mob::Ent::Mob(near), 41));
    assert!(w.p.s.mount_can_use(&mut g, &mut m), "close attacker refused");
    w.p.s.mobs.list[i] = Some(m);
    // Held bee: the return to the known flower is cut.
    let bee = w.spawn(Kind::Bee, [2.5, f64::from(Y) + 1.0, 0.5]);
    {
        let b = w.p.s.mobs.get_mut(bee).unwrap();
        b.wild.flower = Some([8, Y, 8]);
        b.wild.no_nectar_ticks = 700;
    }
    let mut fg = super::wild::goals::WGoal::GoToFlower { travel: 0 };
    let i = w.p.s.mob_index(bee).unwrap();
    let mut m = w.p.s.mobs.list[i].take().unwrap();
    assert!(w.p.s.wild_can_use(&mut fg, &mut m), "free bee without return to the flower");
    w.p.s.mobs.list[i] = Some(m);
    let pu = w.p.s.uuid;
    w.tie(bee, Holder::Player, Anchor::Uuid(pu));
    let mut m = w.p.s.mobs.list[i].take().unwrap();
    assert!(!w.p.s.wild_can_use(&mut fg, &mut m), "held bee set off again towards its flower");
    w.p.s.mobs.list[i] = Some(m);
}

/// Held happy ghast at 7 blocks: every float point kept is within 9 blocks of the centre of
/// the player's block (the lead's zone), not in its former zone.
#[test]
fn held_ghast_floats_in_the_lead_zone() {
    let mut w = world("ghast");
    let g = w.spawn(Kind::HappyGhast, [7.5, f64::from(Y) + 1.0, 0.5]);
    let pu = w.p.s.uuid;
    w.tie(g, Holder::Player, Anchor::Uuid(pu));
    assert_eq!(leash::home(w.mob(g)), Some(([0, Y, 0], 9)));
    let i = w.p.s.mob_index(g).unwrap();
    let mut m = w.p.s.mobs.list[i].take().unwrap();
    m.rec.restriction = Some(([7, Y + 1, 0], 64));
    let c = [0.5, f64::from(Y) + 0.5, 0.5];
    for n in 0..20 {
        let t = w.p.s.ghast_fly_to(&mut m, 0);
        let d = (0..3).map(|a| (t[a] - c[a]).powi(2)).sum::<f64>();
        assert!(d < 81.0, "point {n} outside the lead's zone: {t:?}");
    }
    w.p.s.mobs.list[i] = Some(m);
}

/// Dispenser shears: a knot's lead falls (knot removed, shears sound in the neutral category);
/// a held sheep loses its lead without being sheared; a player in the cell drops his animals.
#[test]
fn dispenser_shears_cut_leads() {
    let mut w = world("dispenser");
    let shears = || Stack { id: "minecraft:shears".into(), count: 1, components: None };
    let fence = [3, Y, 3];
    w.p.run(&format!("setblock {} {} {} minecraft:oak_fence\n", fence[0], fence[1], fence[2]));
    let k = w.p.s.knot_create(fence);
    let cow = w.spawn(Kind::Cow, [2.5, f64::from(Y), 1.5]);
    w.tie(cow, Holder::Knot(k), Anchor::Pos(fence));
    let _ = w.p.take_events();
    w.p.s.dispense_item([2, Y, 3], crate::blocks::Dir::East, 0, shears());
    let evs = w.p.take_events();
    assert!(leash::holder(w.mob(cow)).is_none(), "knot's lead not cut");
    assert!(w.p.s.knot_at(fence).is_none(), "empty knot left behind");
    assert_eq!(w.leads_dropped(), 1);
    let snip = w.p.s.sound_id("minecraft:item.shears.snip").unwrap();
    assert!(
        evs.iter().any(|e| matches!(e, Event::Sound(s) if matches!(s.sound, SoundRef::Id(i) if i == snip) && s.category == 6)),
        "shears sound missing or outside the neutral category"
    );
    // Sheep held by the player, in the targeted cell: lead cut, wool kept.
    let pu = w.p.s.uuid;
    let sheep = w.spawn(Kind::Sheep, [6.5, f64::from(Y), 6.5]);
    w.tie(sheep, Holder::Player, Anchor::Uuid(pu));
    w.p.s.dispense_item([5, Y, 6], crate::blocks::Dir::East, 0, shears());
    assert!(leash::holder(w.mob(sheep)).is_none(), "sheep's lead not cut");
    assert!(!w.mob(sheep).sheared, "held sheep sheared");
    // Player in the cell: the cow he holds is dropped.
    w.tie(cow, Holder::Player, Anchor::Uuid(pu));
    w.p.s.dispense_item([-1, Y, 0], crate::blocks::Dir::East, 0, shears());
    assert!(leash::holder(w.mob(cow)).is_none(), "player's leads not cut");
}

/// Sulfur cube whose fuse runs out: the lead falls (item, null link) before the explosion.
#[test]
fn sulfur_cube_drops_its_lead_before_exploding() {
    let mut w = world("cube");
    crate::nbtio::set(&mut w.p.s.rules, "minecraft:tnt_explodes", rc_proto::nbt::Nbt::Byte(0));
    let c = w.spawn(Kind::SulfurCube, [3.5, f64::from(Y), 0.5]);
    let pu = w.p.s.uuid;
    w.tie(c, Holder::Player, Anchor::Uuid(pu));
    let _ = w.p.take_events();
    let i = w.p.s.mob_index(c).unwrap();
    let mut m = w.p.s.mobs.list[i].take().unwrap();
    m.cube.explosion = Some((1, false, 1));
    m.cube.fuse = 1;
    assert!(w.p.s.cube_pre_tick(&mut m), "cube did not explode");
    let evs = w.p.take_events();
    assert!(m.leash.is_none());
    assert_eq!(W::links(&evs), vec![(c, 0)], "null link not sent");
    w.p.s.mobs.list[i] = Some(m);
    assert_eq!(w.leads_dropped(), 1, "lead not dropped on the ground");
}

/// Lead home saved (`home_pos`, `home_radius`); reloaded with a missing holder, the animal
/// drops the lead after 100 ticks and keeps the zone (like the game); a normally detached
/// lead clears everything.
#[test]
fn lead_home_saved_and_reloaded() {
    use rc_proto::nbt::Nbt;
    let mut w = world("save");
    let cow = w.spawn(Kind::Cow, [2.5, f64::from(Y), 0.5]);
    let pu = w.p.s.uuid;
    w.tie(cow, Holder::Player, Anchor::Uuid(pu));
    let mut saved = w.mob(cow).save();
    assert_eq!(saved.get("home_radius"), Some(&Nbt::Int(5)));
    assert_eq!(saved.get("home_pos"), Some(&Nbt::IntArray(vec![0, Y, 0])));
    // Missing holder.
    crate::nbtio::set(&mut saved, "leash", Nbt::Compound(vec![("UUID".into(), crate::nbtio::uuid_nbt(0x1234))]));
    let mut again = w.p.s.new_mob(Kind::Cow, [2.5, f64::from(Y), 1.5], 0.0);
    again.load(&saved);
    again.tick_count = 101;
    let id2 = again.id;
    w.p.s.add_mob(again);
    w.leash_tick(id2);
    let m = w.mob(id2);
    assert!(m.leash.is_none(), "missing attachment not abandoned");
    assert_eq!(super::goals::rpos_home(m), Some(([0, Y, 0], 5)), "zone lost on abandon");
    assert_eq!(m.save().get("home_radius"), Some(&Nbt::Int(5)));
    // Normal detach: zone cleared, nothing written any more.
    w.p.s.slots[36 + w.p.s.held as usize] = None;
    assert!(w.p.s.player_interact(cow, 0));
    let m = w.mob(cow);
    assert!(super::goals::rpos_home(m).is_none());
    assert!(m.save().get("home_radius").is_none() && m.save().get("home_pos").is_none());
}

/// Held tamed nautilus: its zone check does not rewrite the home.
#[test]
fn held_nautilus_keeps_the_lead_zone() {
    let mut w = world("nautilus");
    let n = w.spawn(Kind::Nautilus, [3.5, f64::from(Y), 0.5]);
    w.p.s.mobs.get_mut(n).unwrap().tame = true;
    let pu = w.p.s.uuid;
    w.tie(n, Holder::Player, Anchor::Uuid(pu));
    let i = w.p.s.mob_index(n).unwrap();
    let mut m = w.p.s.mobs.list[i].take().unwrap();
    w.p.s.nautilus_brain_tick(&mut m);
    assert!(m.sea.home.is_none(), "nautilus zone set during the lead: {:?}", m.sea.home);
    assert_eq!(super::goals::rpos_home(&m), Some(([0, Y, 0], 5)));
    w.p.s.mobs.list[i] = Some(m);
}

/// Bench (ignored by default): cost of a taut (spring) and slack (close follow, path included)
/// lead tick, best of 5 series.
#[test]
#[ignore]
fn bench_leash_tick() {
    let mut w = world("bench");
    let u = w.p.s.uuid;
    for (x, label) in [(8.5, "taut"), (3.5, "close")] {
        let cow = w.spawn(Kind::Cow, [x, f64::from(Y), 0.5]);
        let i = w.p.s.mob_index(cow).unwrap();
        let mut m = w.p.s.mobs.list[i].take().unwrap();
        w.p.s.leash_set(&mut m, Holder::Player, Anchor::Uuid(u), false);
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            for _ in 0..2000 {
                m.pos = [x, f64::from(Y), 0.5];
                m.vel = [0.0; 3];
                w.p.s.leash_tick(&mut m);
            }
            best = best.min(t.elapsed().as_nanos() as f64 / 2000.0);
        }
        w.p.s.mobs.list[i] = Some(m);
        println!("lead {label}: {best:.0} ns per tick");
    }
}
