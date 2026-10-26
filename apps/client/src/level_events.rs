//! Sounds of world events (`level_event`, `docs/specs/sounds.md` § 1.6) apart from the block
//! sound sets (breaking, mining: to come with the extracted table).

use rc_audio::{AudioEngine, Category, SoundRequest};

/// Pitch of an event: constant or formula drawn from the world's randomness.
#[derive(Clone, Copy)]
enum Pitch {
    Fixed(f32),
    /// 1 + (f − f)·0.2
    H1,
    /// 0.9 + f·0.1
    H2,
    /// 2.6 + (f − f)·0.8
    H3,
}

/// Local event: sound, category, volume, pitch, delay depending on the distance.
fn local(kind: i32, data: i32) -> Option<(&'static str, Category, f32, Pitch, bool)> {
    use Category::{Ambient, Block, Hostile, Neutral};
    use Pitch::{Fixed, H1, H2, H3};
    Some(match kind {
        1000 => ("minecraft:block.dispenser.dispense", Block, 1.0, Fixed(1.0), false),
        1001 => ("minecraft:block.dispenser.fail", Block, 1.0, Fixed(1.2), false),
        1002 => ("minecraft:block.dispenser.launch", Block, 1.0, Fixed(1.2), false),
        1004 => ("minecraft:entity.firework_rocket.shoot", Neutral, 1.0, Fixed(1.2), false),
        1009 if data == 0 => ("minecraft:block.fire.extinguish", Block, 0.5, H3, false),
        1015 => ("minecraft:entity.ghast.warn", Hostile, 10.0, H1, false),
        1016 => ("minecraft:entity.ghast.shoot", Hostile, 10.0, H1, false),
        1017 => ("minecraft:entity.ender_dragon.shoot", Hostile, 10.0, H1, false),
        1018 => ("minecraft:entity.blaze.shoot", Hostile, 2.0, H1, false),
        1019 => ("minecraft:entity.zombie.attack_wooden_door", Hostile, 2.0, H1, false),
        1020 => ("minecraft:entity.zombie.attack_iron_door", Hostile, 2.0, H1, false),
        1021 => ("minecraft:entity.zombie.break_wooden_door", Hostile, 2.0, H1, false),
        1022 => ("minecraft:entity.wither.break_block", Hostile, 2.0, H1, false),
        1024 => ("minecraft:entity.wither.shoot", Hostile, 2.0, H1, false),
        1025 => ("minecraft:entity.bat.takeoff", Neutral, 0.05, H1, false),
        1026 => ("minecraft:entity.zombie.infect", Hostile, 2.0, H1, false),
        1027 => ("minecraft:entity.zombie_villager.converted", Hostile, 2.0, H1, false),
        1029 => ("minecraft:block.anvil.destroy", Block, 1.0, H2, false),
        1030 => ("minecraft:block.anvil.use", Block, 1.0, H2, false),
        1031 => ("minecraft:block.anvil.land", Block, 0.3, H2, false),
        1033 => ("minecraft:block.chorus_flower.grow", Block, 1.0, Fixed(1.0), false),
        1034 => ("minecraft:block.chorus_flower.death", Block, 1.0, Fixed(1.0), false),
        1035 => ("minecraft:block.brewing_stand.brew", Block, 1.0, Fixed(1.0), false),
        1039 => ("minecraft:entity.phantom.bite", Hostile, 0.3, H2, false),
        1040 => ("minecraft:entity.zombie.converted_to_drowned", Hostile, 2.0, H1, false),
        1041 => ("minecraft:entity.husk.converted_to_zombie", Hostile, 2.0, H1, false),
        1042 => ("minecraft:block.grindstone.use", Block, 1.0, H2, false),
        1043 => ("minecraft:item.book.page_turn", Block, 1.0, H2, false),
        1044 => ("minecraft:block.smithing_table.use", Block, 1.0, H2, false),
        1045 => ("minecraft:block.pointed_dripstone.land", Block, 2.0, H2, false),
        1046 => ("minecraft:block.pointed_dripstone.drip_lava_into_cauldron", Block, 2.0, H2, false),
        1047 => ("minecraft:block.pointed_dripstone.drip_water_into_cauldron", Block, 2.0, H2, false),
        1048 => ("minecraft:entity.skeleton.converted_to_stray", Hostile, 2.0, H1, false),
        1049 => ("minecraft:block.crafter.craft", Block, 1.0, Fixed(1.0), false),
        1050 => ("minecraft:block.crafter.fail", Block, 1.0, Fixed(1.0), false),
        1052 => ("minecraft:block.sulfur_spike.land", Block, 2.0, H2, false),
        1053 | 1054 => ("minecraft:entity.splash_potion.break", Neutral, 1.0, H2, false),
        1500 if data > 0 => ("minecraft:block.composter.fill_success", Block, 1.0, Fixed(1.0), false),
        1500 => ("minecraft:block.composter.fill", Block, 1.0, Fixed(1.0), false),
        1501 => ("minecraft:block.lava.extinguish", Block, 0.5, H3, false),
        1502 => ("minecraft:block.redstone_torch.burnout", Block, 0.5, H3, false),
        1503 => ("minecraft:block.end_portal_frame.fill", Block, 1.0, Fixed(1.0), false),
        1505 => ("minecraft:item.bone_meal.use", Block, 1.0, Fixed(1.0), false),
        2006 if data == 1 => ("minecraft:entity.dragon_fireball.explode", Hostile, 1.0, H2, false),
        3012 => ("minecraft:block.trial_spawner.spawn_mob", Block, 1.0, H1, true),
        3013 | 3019 => ("minecraft:block.trial_spawner.detect_player", Block, 1.0, H1, true),
        3014 => ("minecraft:block.trial_spawner.eject_item", Block, 1.0, H1, true),
        3015 => ("minecraft:block.vault.activate", Block, 1.0, H1, true),
        3016 => ("minecraft:block.vault.deactivate", Block, 1.0, H1, true),
        3018 => ("minecraft:block.cobweb.place", Block, 1.0, H1, true),
        3020 => ("minecraft:block.trial_spawner.ominous_activate", Block, if data == 0 { 0.3 } else { 1.0 }, H1, true),
        3021 => ("minecraft:block.trial_spawner.spawn_item", Block, 1.0, H1, true),
        1032 => ("minecraft:block.portal.travel", Ambient, 0.25, Fixed(0.0), false),
        _ => return None,
    })
}

/// Client world randomness (pitch draws): xorshift, no effect on the network.
pub struct WorldRandom(u64);

impl Default for WorldRandom {
    fn default() -> Self {
        WorldRandom(0x9E37_79B9_7F4A_7C15)
    }
}

impl WorldRandom {
    /// Standard Gaussian (Box-Muller).
    pub fn gaussian(&mut self) -> f64 {
        let u = f64::from(self.f()).max(1e-12);
        let v = f64::from(self.f());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }

    pub fn f(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Plays the sound of a world event; `camera` = eye position.
pub fn play(
    audio: &mut AudioEngine,
    rnd: &mut WorldRandom,
    kind: i32,
    pos: [i32; 3],
    data: i32,
    global: bool,
    camera: [f64; 3],
) {
    let center = pos.map(|v| f64::from(v) + 0.5);
    if global {
        let (name, volume) = match kind {
            1023 => ("minecraft:entity.wither.spawn", 1.0),
            1028 => ("minecraft:entity.ender_dragon.death", 5.0),
            1038 => ("minecraft:block.end_portal.spawn", 1.0),
            _ => return,
        };
        // Placed 2 blocks from the camera toward the block: always audible.
        let d = [center[0] - camera[0], center[1] - camera[1], center[2] - camera[2]];
        let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
        let p = [camera[0] + d[0] / n * 2.0, camera[1] + d[1] / n * 2.0, camera[2] + d[2] / n * 2.0];
        audio.play(name, Category::Hostile, Some(p), volume, 1.0, None);
        return;
    }
    // Extinguishing an item (data 1): its own pitch formula.
    if kind == 1009 && data == 1 {
        let pitch = 1.6 + (rnd.f() - rnd.f()) * 0.4;
        audio.play("minecraft:entity.generic.extinguish_fire", Category::Block, Some(center), 0.7, pitch, None);
        return;
    }
    if kind == 1051 {
        let pitch = 0.4 / (0.8 + rnd.f() * 0.4);
        audio.play("minecraft:entity.wind_charge.throw", Category::Block, Some(center), 0.5, pitch, None);
        return;
    }
    let Some((name, category, volume, pitch, delayed)) = local(kind, data) else { return };
    let pitch = match pitch {
        Pitch::Fixed(p) => p,
        Pitch::H1 => 1.0 + (rnd.f() - rnd.f()) * 0.2,
        Pitch::H2 => 0.9 + rnd.f() * 0.1,
        Pitch::H3 => 2.6 + (rnd.f() - rnd.f()) * 0.8,
    };
    if kind == 1032 {
        // Local portal ambience: relative, no attenuation.
        audio.play(name, category, None, volume, 0.8 + rnd.f() * 0.4, None);
        return;
    }
    let mut req = SoundRequest::new(name, category).at(center).volume(volume).pitch(pitch);
    if delayed {
        // Sound at 40 blocks/s beyond 10 blocks.
        let d2: f64 = (0..3).map(|i| (center[i] - camera[i]).powi(2)).sum();
        if d2 > 100.0 {
            req = req.delay((d2.sqrt() / 40.0 * 20.0).floor() as u32);
        }
    }
    audio.play_request(req);
}
