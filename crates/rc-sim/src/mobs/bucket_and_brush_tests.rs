//! Mobs taken into a bucket and an armadillo brushed, through the player's interaction on the
//! platform of the test world (`RUSTCRAFT_SOLO_WORLD`).

use super::species::Kind;
use super::tests_spawn_paths::open;
use crate::blocks::probe::Probe;
use crate::player::Stack;

/// Where the mob stands, and the player two blocks from it.
const MOB_AT: [f64; 3] = [216.5, -59.0, 216.5];
const PLAYER_AT: [f64; 3] = [216.5, -59.0, 214.5];

/// `id` alone in the player's main hand (first hotbar slot).
fn hold(p: &mut Probe, id: &str) {
    p.s.held = 0;
    p.s.slots[36] = Some(Stack { id: id.into(), count: 1, components: None });
}

fn held_id(p: &Probe) -> Option<&str> {
    p.s.slots[36].as_ref().map(|s| s.id.as_str())
}

/// A new mob of `kind` beside the player, used with the held item; true if the use took place.
fn use_held_item_on_new(p: &mut Probe, kind: Kind) -> bool {
    let mob = p.s.new_mob(kind, MOB_AT, 0.0);
    let id = mob.id;
    p.s.add_mob(mob);
    p.s.player_interact(id, 0)
}

fn advancement_done(p: &Probe, id: &str) -> bool {
    let index = *p.s.prog.tree.index.get(id).unwrap_or_else(|| panic!("advancement {id} missing"));
    p.s.prog.done(index)
}

/// A fish, an axolotl and a tadpole taken into a water bucket fire `filled_bucket` with the
/// bucket obtained, like a bucket of fluid: each one's advancement is granted.
#[test]
fn bucketing_a_mob_fires_the_filled_bucket_trigger() {
    let Some((mut p, _dir)) = open("bucket-trigger") else { return };
    p.s.pos = PLAYER_AT;
    for (kind, bucket, advancement) in [
        (Kind::Cod, "minecraft:cod_bucket", "minecraft:husbandry/tactical_fishing"),
        (Kind::Axolotl, "minecraft:axolotl_bucket", "minecraft:husbandry/axolotl_in_a_bucket"),
        (Kind::Tadpole, "minecraft:tadpole_bucket", "minecraft:husbandry/tadpole_in_a_bucket"),
    ] {
        hold(&mut p, "minecraft:water_bucket");
        assert!(!advancement_done(&p, advancement), "{advancement} granted before");
        assert!(use_held_item_on_new(&mut p, kind), "{kind:?} not taken");
        assert_eq!(held_id(&p), Some(bucket), "{kind:?}");
        assert!(advancement_done(&p, advancement), "{advancement} not granted");
    }
}

/// A sulfur cube taken with an empty bucket goes through the same pickup: the filled bucket
/// in hand, the cube gone.
#[test]
fn sulfur_cube_bucketed_like_the_others() {
    let Some((mut p, _dir)) = open("sulfur-cube-bucket") else { return };
    p.s.pos = PLAYER_AT;
    hold(&mut p, "minecraft:bucket");
    assert!(use_held_item_on_new(&mut p, Kind::SulfurCube), "sulfur cube not taken");
    assert_eq!(held_id(&p), Some("minecraft:sulfur_cube_bucket"));
    assert!(!p.s.mobs.list.iter().any(|m| m.kind == Kind::SulfurCube && !m.removed), "cube still in the world");
}

/// An adult armadillo brushed drops one scute (table `brush/armadillo`), the brush worn by 16.
#[test]
fn brushing_an_armadillo_drops_a_scute() {
    let Some((mut p, _dir)) = open("armadillo-brush") else { return };
    p.s.pos = PLAYER_AT;
    hold(&mut p, "minecraft:brush");
    let scutes_before = scutes_on_the_ground(&p);
    assert!(use_held_item_on_new(&mut p, Kind::Armadillo), "brush refused");
    assert_eq!(scutes_on_the_ground(&p), scutes_before + 1, "one scute");
    let wear = p.s.slots[36].as_ref().and_then(|s| crate::items::component(s, "minecraft:damage")).and_then(|d| d.as_i64());
    assert_eq!(wear, Some(16), "brush worn");
}

fn scutes_on_the_ground(p: &Probe) -> i32 {
    p.s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == "minecraft:armadillo_scute").map(|d| d.stack.count).sum()
}
