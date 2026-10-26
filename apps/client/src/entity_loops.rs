//! Sounds sustained by the client for entities it does not drive (`sounds.md` § 3.7):
//! minecart rolling, bee buzzing (normal or aggressive); and entry sound
//! of the local player into a bubble column.
//!
//! Like the game: an entity's sound is created only once, when it appears (`add_entity`),
//! started at zero volume; it stops for good if the entity disappears or becomes silent.
//! Volumes follow the synchronized velocity (`set_entity_motion`) on every tick.

use rc_proto_v777::entity::Meta;

use crate::entities::Entity;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Minecart,
    Bee { angry: bool },
}

struct Loop {
    handle: rc_audio::SoundHandle,
    entity: i32,
    kind: Kind,
}

/// State of the sustained sounds (kept in `Riding`).
pub(crate) struct Loops {
    list: Vec<Loop>,
    /// Bees to start at the next engine tick (queue of the game's sustained sounds).
    queued: Vec<(i32, bool)>,
    /// Bubble column: player already inside at the previous tick; first tick (no sound).
    in_bubbles: bool,
    first_tick: bool,
}

impl Default for Loops {
    fn default() -> Self {
        Loops { list: Vec::new(), queued: Vec::new(), in_bubbles: false, first_tick: true }
    }
}

/// Horizontal component of a mob's velocity on the client: components under 0.003 set to
/// zero (step of the remote mob), then length, in single float like the game.
fn mob_speed(v: [f64; 3]) -> f32 {
    let x = if v[0].abs() < 0.003 { 0.0 } else { v[0] };
    let z = if v[2].abs() < 0.003 { 0.0 } else { v[2] };
    x.hypot(z) as f32
}

/// Volume of a minecart's rolling: zero under 0.01 block/tick or frozen world, otherwise
/// `0.7 × min(v, 0.5)` (cap 0.35, as is in the game). The pitch stays 1: the game computes
/// a pitch rise in a field that is never read.
pub(crate) fn minecart_volume(speed: f32, runs: bool) -> f32 {
    if speed >= 0.01 && runs { speed.clamp(0.0, 0.5) * 0.7 } else { 0.0 }
}

/// Volume and pitch of the buzzing: `1.2 × min(v, 0.5)` (cap 0.6) and pitch
/// `min + clamp(v, min, max) × (max − min)` (adult 0.7–1.1, baby 1.1–1.5); zero when stopped.
pub(crate) fn bee_params(speed: f32, baby: bool) -> (f32, f32) {
    if speed < 0.01 {
        return (0.0, 0.0);
    }
    let (lo, hi) = if baby { (1.1, 1.5) } else { (0.7, 1.1) };
    (speed.clamp(0.0, 0.5) * 1.2, lo + speed.clamp(lo, hi) * (hi - lo))
}

/// Angry bee: synchronized anger end (data 19) later than the world time.
fn bee_angry(e: &Entity, game_time: i64) -> bool {
    matches!(e.meta.get(&19), Some(Meta::Long(end)) if *end > 0 && *end - game_time > 0)
}

/// Sound position: entity coordinates in single float (like the game).
fn sound_pos(e: &Entity) -> [f64; 3] {
    e.pos.map(|v| f64::from(v as f32))
}

fn bee_event(angry: bool) -> &'static str {
    if angry { "minecraft:entity.bee.loop_aggressive" } else { "minecraft:entity.bee.loop" }
}

impl crate::App {
    /// Entity appeared (`add_entity`): minecart → rolling started right away; bee →
    /// buzzing (aggressive if already angry) queued for the next tick.
    pub(crate) fn loop_sound_added(&mut self, id: i32) {
        let Some(e) = self.entities.map.get(&id) else { return };
        if e.kind == "bee" {
            let angry = bee_angry(e, self.env.env.clocks.game_time);
            self.riding.loops.queued.push((id, angry));
        } else if e.kind.ends_with("minecart") && !e.meta_bool(4) {
            let pos = sound_pos(e);
            let Some(a) = self.audio.get() else { return };
            let req = rc_audio::SoundRequest::new("minecraft:entity.minecart.riding", rc_audio::Category::Neutral)
                .at(pos)
                .volume(0.0)
                .looping(0)
                .start_silent();
            if let Some(handle) = a.play_request(req) {
                self.riding.loops.list.push(Loop { handle, entity: id, kind: Kind::Minecart });
            }
        }
    }

    /// Tick of the sustained sounds: bee queue, then volume, pitch and position of each
    /// sound; permanent stop if the entity disappeared, became silent or if the sound was
    /// cut (`stop_sound`, world change).
    pub(crate) fn tick_loop_sounds(&mut self) {
        self.tick_bubble_column_sound();
        let Some(a) = self.audio.get() else { return };
        let game_time = self.env.env.clocks.game_time;
        let runs = self.exact.rate.runs;
        let loops = &mut self.riding.loops;
        for (id, angry) in std::mem::take(&mut loops.queued) {
            let Some(e) = self.entities.map.get(&id) else { continue };
            if e.meta_bool(4) {
                continue;
            }
            let req = rc_audio::SoundRequest::new(bee_event(angry), rc_audio::Category::Neutral)
                .at(sound_pos(e))
                .volume(0.0)
                .looping(0)
                .start_silent();
            if let Some(handle) = a.play_request(req) {
                loops.list.push(Loop { handle, entity: id, kind: Kind::Bee { angry } });
            }
        }
        let mut switched = Vec::new();
        loops.list.retain(|l| {
            let e = self.entities.map.get(&l.entity);
            let Some(e) = e.filter(|e| !e.meta_bool(4) && a.is_playing(l.handle)) else {
                a.stop_handle(l.handle);
                return false;
            };
            let (volume, pitch) = match l.kind {
                Kind::Minecart => {
                    let v = e.velocity;
                    (minecart_volume(v[0].hypot(v[2]) as f32, runs), 1.0)
                }
                Kind::Bee { angry } => {
                    // Anger gained or lost: the other loop takes over at the next tick.
                    if bee_angry(e, game_time) != angry {
                        switched.push((l.entity, !angry));
                        a.stop_handle(l.handle);
                        return false;
                    }
                    bee_params(mob_speed(e.velocity), e.meta_bool(16))
                }
            };
            a.set_sound_params(l.handle, volume, pitch);
            a.set_position(l.handle, sound_pos(e));
            true
        });
        loops.queued.extend(switched);
    }

    /// Entering a bubble column (player box reduced by 0.4 at top and bottom):
    /// whirlpool sound (downward) or rising bubbles sound, players category, except on the
    /// first tick and in spectator.
    fn tick_bubble_column_sound(&mut self) {
        let b = self.player.bounding_box();
        let lo = [b.min[0] + 1e-6, b.min[1] + 0.4 + 1e-6, b.min[2] + 1e-6];
        let hi = [b.max[0] - 1e-6, b.max[1] - 0.4 - 1e-6, b.max[2] - 1e-6];
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let mut found = None;
        'scan: for x in lo[0].floor() as i32..=hi[0].floor() as i32 {
            for y in lo[1].floor() as i32..=hi[1].floor() as i32 {
                for z in lo[2].floor() as i32..=hi[2].floor() as i32 {
                    let s = view.state(x, y, z);
                    if self.data.block_of(s).is_some_and(|i| i.name == "minecraft:bubble_column") {
                        found = Some(self.data.state_properties(s).any(|p| p == ("drag", "true")));
                        break 'scan;
                    }
                }
            }
        }
        let loops = &mut self.riding.loops;
        if let Some(drag) = found {
            if !loops.in_bubbles && !loops.first_tick && !self.player.spectator {
                let event = if drag {
                    "minecraft:block.bubble_column.whirlpool_inside"
                } else {
                    "minecraft:block.bubble_column.upwards_inside"
                };
                if let Some(a) = self.audio.get() {
                    a.play(event, rc_audio::Category::Player, Some(self.player.pos), 1.0, 1.0, None);
                }
            }
            loops.in_bubbles = true;
        } else {
            loops.in_bubbles = false;
        }
        loops.first_tick = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minecart_volume_matches_game() {
        assert_eq!(minecart_volume(0.009, true), 0.0);
        assert_eq!(minecart_volume(0.2, true), 0.2 * 0.7);
        // Cap at 0.5 block/tick: 0.35 even at full speed.
        assert_eq!(minecart_volume(1.2, true), 0.35);
        assert_eq!(minecart_volume(0.4, false), 0.0, "frozen world");
    }

    #[test]
    fn bee_params_match_game() {
        assert_eq!(bee_params(0.0, false), (0.0, 0.0));
        let (v, p) = bee_params(0.3, false);
        assert!((v - 0.36).abs() < 1e-6 && (p - (0.7 + 0.7 * 0.4)).abs() < 1e-6, "{v} {p}");
        // Baby: pitch 1.1 + 1.1 × 0.4 at low speed (speed clamped to 1.1).
        let (_, p) = bee_params(0.1, true);
        assert!((p - (1.1 + 1.1 * 0.4)).abs() < 1e-6, "{p}");
        assert_eq!(bee_params(2.0, false).0, 0.6);
    }

    #[test]
    fn mob_speed_drops_tiny_components() {
        assert_eq!(mob_speed([0.002, 0.5, 0.002]), 0.0);
        assert_eq!(mob_speed([0.3, 0.0, 0.4]), 0.5);
    }
}
