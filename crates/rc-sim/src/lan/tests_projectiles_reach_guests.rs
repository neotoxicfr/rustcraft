//! Projectiles and the guests of the local network: a guest's projectile is the guest's (kill
//! credit, pearl teleport, the arrow's hit sound sent to it), and a projectile hits a guest like
//! the host (a mob's arrow, a splash potion).

use super::tests_g3f05::{
    FLOOR, G, T, body, drain, feet, guest_packets, guest_ready, host_ready, id_of, ids, join, open, publish,
    server_tick,
};
use crate::mobs::species::Kind;
use crate::player::Stack;
use crate::projectiles::{PKind, Proj};

fn host_and_guest(tag: &str, host: [f64; 3], guest_at: [f64; 3]) -> (T, G) {
    let mut t = open(tag);
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    host_ready(&mut t, host);
    guest_ready(&mut t, guest_at);
    (t, g)
}

fn guest(t: &T) -> &super::ctx::PlayerCtx {
    t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap()
}

fn stack(id: &str) -> Stack {
    Stack { id: id.into(), count: 1, components: None }
}

/// An arrow of `owner` at `pos` flying at `vel`, out of its owner's reach already.
fn arrow(t: &mut T, owner: u128, pos: [f64; 3], vel: [f64; 3]) -> i32 {
    let id = t.s.proj_new_id();
    let mut a = Proj::new(PKind::Arrow, id, pos, stack("minecraft:arrow"));
    a.owner = Some(owner);
    a.damage = 2.0;
    a.vel = vel;
    a.left_owner = true;
    t.s.proj_add(a);
    id
}

/// Ticks the server until `done` holds (at most `limit` ticks).
fn ticks_until(t: &mut T, limit: u32, mut done: impl FnMut(&T) -> bool) -> bool {
    for _ in 0..limit {
        server_tick(t);
        if done(t) {
            return true;
        }
    }
    false
}

/// The arrow-hit game event (6) among a guest's packets.
fn got_arrow_hit_sound(t: &T, g: &G) -> bool {
    let game_event = ids(t).game_event;
    guest_packets(g).iter().any(|p| id_of(p) == game_event && body(p).first() == Some(&6))
}

/// The guest's arrow hits the host: the host is hurt, and the guest's client gets the arrow's
/// hit sound (the game sends it to the shooting player). Fails on the code before (the arrow had
/// no owner it could find: no sound); sabotage verified (failure): the guest's projectiles ticked
/// with the host installed.
#[test]
fn a_guest_arrow_on_the_host_rings_for_the_guest() {
    let (mut t, g) = host_and_guest("guest-arrow-host", feet([-3, FLOOR + 1, 0]), feet([3, FLOOR + 1, 0]));
    let shooter = guest(&t).uuid;
    arrow(&mut t, shooter, [2.0, f64::from(FLOOR) + 2.0, 0.5], [-2.0, 0.0, 0.0]);
    let _ = guest_packets(&g);
    assert!(ticks_until(&mut t, 10, |t| t.s.life.vit.health < 20.0), "the guest's arrow never hurt the host");
    assert!(got_arrow_hit_sound(&t, &g), "the shooting guest heard no arrow hit sound");
}

/// The `pvp` rule off, the guest's arrow flies through the host without hurting them. Fails
/// with the host kept among the arrow's targets (the arrow stops at the host and drops);
/// sabotage verified (failure): the player the shooter may not hurt kept among the targets.
#[test]
fn a_guest_arrow_passes_through_the_host_when_pvp_is_off() {
    let (mut t, _g) = host_and_guest("guest-arrow-no-pvp", feet([-3, FLOOR + 1, 0]), feet([3, FLOOR + 1, 0]));
    crate::nbtio::set(&mut t.s.rules, "minecraft:pvp", rc_proto::nbt::Nbt::Byte(0));
    let shooter = guest(&t).uuid;
    let id = arrow(&mut t, shooter, [2.0, f64::from(FLOOR) + 2.0, 0.5], [-2.0, 0.0, 0.0]);
    let host_x = t.s.pos[0];
    let past_the_host = |t: &T| t.s.proj.list.iter().any(|p| p.id == id && p.pos[0] < host_x - 1.0);
    assert!(ticks_until(&mut t, 10, past_the_host), "the arrow did not fly through the host");
    assert_eq!(t.s.life.vit.health, 20.0, "the host was not hurt");
}

/// The `pvp` rule off, the guest's splash potion of harming breaks at the host's feet: the host
/// is spared. Fails with the victim's refusal removed (the host takes the harm); sabotage
/// verified (failure): the refusal of another player's damage removed.
#[test]
fn a_guest_splash_potion_spares_the_host_when_pvp_is_off() {
    let (mut t, _g) = host_and_guest("guest-splash-no-pvp", feet([3, FLOOR + 1, 3]), feet([-5, FLOOR + 1, -5]));
    crate::nbtio::set(&mut t.s.rules, "minecraft:pvp", rc_proto::nbt::Nbt::Byte(0));
    let owner = guest(&t).uuid;
    splash_harming(&mut t, owner, [3.5, f64::from(FLOOR) + 2.5, 3.5]);
    let _ = ticks_until(&mut t, 20, |_| false);
    assert_eq!(t.s.life.vit.health, 20.0, "the guest's splash hurt the host despite the pvp rule");
}

/// The guest's arrow kills the host: the death message names the guest as the killer. Fails
/// with the author left out (a death by an arrow from nobody); sabotage verified (failure): the
/// shooting player not given to the death message.
#[test]
fn a_host_killed_by_a_guest_arrow_dies_by_the_guest() {
    let (mut t, _g) = host_and_guest("guest-arrow-death", feet([-3, FLOOR + 1, 0]), feet([3, FLOOR + 1, 0]));
    t.s.life.vit.health = 1.0;
    let shooter = guest(&t).uuid;
    arrow(&mut t, shooter, [2.0, f64::from(FLOOR) + 2.0, 0.5], [-2.0, 0.0, 0.0]);
    let _ = drain(&t);
    assert!(ticks_until(&mut t, 10, |t| t.s.life.vit.dead), "the guest's arrow did not kill the host");
    let message = drain(&t).into_iter().find_map(|e| match e {
        rc_client::Event::Died { message, .. } => message,
        _ => None,
    });
    let message = format!("{:?}", message.expect("the host's death message"));
    assert!(message.contains("death.attack.arrow"), "a death by an arrow: {message}");
    assert!(message.contains("Guest"), "the guest is named as the killer: {message}");
}

/// The guest's arrow kills a zombie: the kill is the guest's (its statistic), not the host's.
/// Fails on the code before (no owner found: nobody credited); sabotage verified (failure): the
/// guest's projectiles ticked with the host installed.
#[test]
fn a_guest_arrow_kill_is_credited_to_the_guest() {
    let (mut t, _g) = host_and_guest("guest-arrow-kill", feet([-5, FLOOR + 1, -5]), feet([-3, FLOOR + 1, 0]));
    let mut z = t.s.new_mob(Kind::Zombie, feet([3, FLOOR + 1, 0]), 0.0);
    z.health = 1.0;
    let zid = z.id;
    t.s.add_mob(z);
    let shooter = guest(&t).uuid;
    arrow(&mut t, shooter, [-2.0, f64::from(FLOOR) + 2.0, 0.5], [2.0, 0.0, 0.0]);
    assert!(ticks_until(&mut t, 10, |t| t.s.mob_ref(zid).is_none_or(|m| !m.alive())), "the zombie survived");
    let by_guest = t.s.as_guest(0, |sv| sv.stat_value("minecraft:killed", "minecraft:zombie"));
    assert_eq!(by_guest, 1, "the guest is credited with the zombie");
    assert_eq!(t.s.stat_value("minecraft:killed", "minecraft:zombie"), 0, "the host killed nothing");
}

/// A mob's arrow flying at the guest hurts it. Fails on the code before (the guests were no
/// targets of a projectile: the arrow flew through); sabotage verified (failure): the other
/// players left out of the targets.
#[test]
fn a_mob_arrow_hurts_the_guest() {
    let (mut t, _g) = host_and_guest("mob-arrow-guest", feet([-5, FLOOR + 1, -5]), feet([3, FLOOR + 1, 0]));
    let skeleton = t.s.new_mob(Kind::Skeleton, feet([-4, FLOOR + 1, 4]), 0.0);
    let owner = skeleton.uuid;
    t.s.add_mob(skeleton);
    arrow(&mut t, owner, [-1.0, f64::from(FLOOR) + 2.0, 0.5], [2.0, 0.0, 0.0]);
    assert!(ticks_until(&mut t, 10, |t| guest(t).vit.health < 20.0), "the mob's arrow never hurt the guest");
}

/// The guest throws an ender pearl onto the floor: it is teleported where it lands. Fails on the
/// code before (a pearl teleported its owner only when it was the host: the guest's pearl broke
/// with no effect); sabotage verified (failure): the guest's projectiles ticked with the host
/// installed.
#[test]
fn a_guest_pearl_teleports_the_guest() {
    let (mut t, _g) = host_and_guest("guest-pearl-teleport", feet([-5, FLOOR + 1, -5]), feet([-3, FLOOR + 1, 0]));
    let id = t.s.proj_new_id();
    let mut pearl =
        Proj::new(PKind::EnderPearl, id, [3.5, f64::from(FLOOR) + 3.0, 3.5], stack("minecraft:ender_pearl"));
    pearl.owner = Some(guest(&t).uuid);
    pearl.vel = [0.0, -0.5, 0.0];
    t.s.proj_add(pearl);
    let landed =
        ticks_until(&mut t, 20, |t| (guest(t).pos[0] - 3.5).abs() < 0.01 && (guest(t).pos[2] - 3.5).abs() < 0.01);
    assert!(landed, "the guest was not teleported to its pearl (at {:?})", guest(&t).pos);
    assert!(guest(&t).vit.health < 20.0, "the pearl's landing hurts the guest");
    assert!(t.s.life.vit.health == 20.0 && t.s.pos[0] < 0.0, "the host stayed where it was, unhurt");
}

/// A splash potion of harming the host breaks at the guest's feet: the guest is hurt. Fails on
/// the code before (a splash only reached the mobs and the host); sabotage verified (failure):
/// the other players left out of the splash.
#[test]
fn a_splash_potion_reaches_the_guest() {
    let (mut t, _g) = host_and_guest("splash-guest", feet([-5, FLOOR + 1, -5]), feet([3, FLOOR + 1, 3]));
    let owner = t.s.uuid;
    splash_harming(&mut t, owner, [3.5, f64::from(FLOOR) + 2.5, 3.5]);
    assert!(ticks_until(&mut t, 20, |t| guest(t).vit.health < 20.0), "the splash never reached the guest");
}

/// A splash potion of harming of `owner` falling from `pos`.
fn splash_harming(t: &mut T, owner: u128, pos: [f64; 3]) {
    let id = t.s.proj_new_id();
    let mut item = stack("minecraft:splash_potion");
    crate::items::set_component(
        &mut item,
        "minecraft:potion_contents",
        rc_proto::nbt::Nbt::Compound(vec![("potion".into(), rc_proto::nbt::Nbt::String("minecraft:harming".into()))]),
    );
    let mut potion = Proj::new(PKind::SplashPotion, id, pos, item);
    potion.owner = Some(owner);
    potion.vel = [0.0, -0.5, 0.0];
    t.s.proj_add(potion);
}
