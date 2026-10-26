//! Client keyframe animations (definitions extracted at preparation time,
//! `VersionData::entity_animations`): interpolation and application to poses.
//!
//! The vectors are offsets added to the part's pose, already in its units: pixels
//! (y inverted at extraction), radians, scale − 1.

use std::sync::OnceLock;

use rc_data::{AnimTarget, KeyframeAnimation};

use super::*;

/// Version data, for the families that have no access to the engine (`animate`).
static DEFS: OnceLock<Arc<VersionData>> = OnceLock::new();

/// Keeps the definitions (called when the engine is created; one version per run).
pub(super) fn install(data: &Arc<VersionData>) {
    let _ = DEFS.set(data.clone());
}

/// Definition by name (`camel.walk`), without going through the engine.
pub(super) fn get(name: &str) -> Option<&'static KeyframeAnimation> {
    DEFS.get()?.entity_animation(name)
}

impl EntityRenderer {
    /// Animation definition by name (`camel.walk`).
    #[allow(dead_code)]
    pub(super) fn animation(&self, name: &str) -> Option<&KeyframeAnimation> {
        self.source.entity_animation(name)
    }
}

/// Adds to the parts the animation values at `elapsed_seconds` since its start, multiplied
/// by `scale` (1 for an animation state, bounded speed for walking). Parts absent from the
/// layer are ignored.
pub(super) fn apply(p: &mut Pose<'_>, anim: &KeyframeAnimation, elapsed_seconds: f32, scale: f32) {
    let t = if anim.looping && anim.length > 0.0 { elapsed_seconds % anim.length } else { elapsed_seconds };
    for c in &anim.channels {
        let Some(i) = p.index(&c.part) else { continue };
        let v = sample(&c.keyframes, t, scale);
        let at = match c.target {
            AnimTarget::Position => 0,
            AnimTarget::Rotation => 3,
            AnimTarget::Scale => 6,
        };
        let part = &mut p.parts[i];
        for k in 0..3 {
            part[at + k] += v[k];
        }
    }
}

/// Walk animation: time = walk position × 50 ms × `speed_factor` (whole
/// milliseconds), amplitude = min(speed × `scale_factor`, 1).
pub(super) fn apply_walk(
    p: &mut Pose<'_>,
    anim: &KeyframeAnimation,
    s: &AnimState,
    speed_factor: f32,
    scale_factor: f32,
) {
    let ms = (s.walk_pos * 50.0 * speed_factor) as i64;
    apply(p, anim, ms as f32 / 1000.0, (s.walk_speed * scale_factor).min(1.0));
}

/// Animation started at tick `start` (entity age), read at age `age` (ticks + fraction):
/// whole elapsed milliseconds × `speed_factor`.
pub(super) fn apply_since(p: &mut Pose<'_>, anim: &KeyframeAnimation, start: f32, age: f32, speed_factor: f32) {
    let ms = ((age - start) * 50.0) as i64;
    let ms = (ms as f32 * speed_factor) as i64;
    apply(p, anim, ms as f32 / 1000.0, 1.0);
}

/// Walk at a given position and speed (nautilus: position offset by age).
pub(super) fn apply_walk_at(
    p: &mut Pose<'_>,
    anim: &KeyframeAnimation,
    pos: f32,
    speed: f32,
    speed_factor: f32,
    scale_factor: f32,
) {
    let ms = (pos * 50.0 * speed_factor) as i64;
    apply(p, anim, ms as f32 / 1000.0, (speed * scale_factor).min(1.0));
}

/// Plays state `st` (encoded by [`State`]) by name if it is started.
pub(super) fn play(p: &mut Pose<'_>, name: &str, st: f32, age: f32) {
    if let Some(start) = State::start_of(st)
        && let Some(a) = get(name)
    {
        apply_since(p, a, start, age, 1.0);
    }
}

/// Client animation state (start at the entity's tick, or stopped) stored in an `f32` of
/// `Entity::extra`: 0 = stopped, otherwise start tick + 0.5 (exact integer up to 2²³).
pub(super) struct State;

impl State {
    pub(super) fn start_of(v: f32) -> Option<f32> {
        (v != 0.0).then_some(v - 0.5)
    }

    pub(super) fn started(v: f32) -> bool {
        v != 0.0
    }

    pub(super) fn start(v: &mut f32, tick: f32) {
        *v = tick + 0.5;
    }

    pub(super) fn start_if_stopped(v: &mut f32, tick: f32) {
        if *v == 0.0 {
            Self::start(v, tick);
        }
    }

    pub(super) fn stop(v: &mut f32) {
        *v = 0.0;
    }

    pub(super) fn animate_when(v: &mut f32, on: bool, tick: f32) {
        if on {
            Self::start_if_stopped(v, tick);
        } else {
            Self::stop(v);
        }
    }

    /// Moves the start back by `ticks` (started state only).
    pub(super) fn fast_forward(v: &mut f32, ticks: f32) {
        if *v != 0.0 {
            *v -= ticks;
        }
    }
}

/// Value of a channel at time `t`: previous keyframe = last one with time < t (at least the
/// first), interpolation chosen by the next keyframe.
fn sample(k: &[rc_data::Keyframe], t: f32, scale: f32) -> [f32; 3] {
    let first_ge = k.partition_point(|f| t > f.time);
    let prev = first_ge.saturating_sub(1);
    let next = (prev + 1).min(k.len() - 1);
    let (a, b) = (&k[prev], &k[next]);
    let alpha = if next != prev { ((t - a.time) / (b.time - a.time)).clamp(0.0, 1.0) } else { 0.0 };
    if b.catmull {
        let p0 = k[prev.saturating_sub(1)].post;
        let p3 = k[(next + 1).min(k.len() - 1)].post;
        std::array::from_fn(|i| catmull_rom(alpha, p0[i], a.post[i], b.post[i], p3[i]) * scale)
    } else {
        std::array::from_fn(|i| (a.post[i] + (b.pre[i] - a.post[i]) * alpha) * scale)
    }
}

fn catmull_rom(t: f32, p0: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    0.5 * (2.0 * p1
        + (p2 - p0) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
        + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t * t * t)
}

/// Family test tools: spawned entity, synthetic layer, animation state.
#[cfg(test)]
pub(super) mod testkit {
    use super::*;
    use crate::entities::Entities;
    use rc_client::EntityEvent;
    use rc_proto_v777::entity::AddEntity;

    /// An entity of kind `kind` (network id 7) in an empty client world.
    pub fn spawn(kind: &str) -> Entities {
        let mut all = Entities { type_names: vec![format!("minecraft:{kind}")], ..Default::default() };
        let add = AddEntity {
            id: 7,
            uuid: 1,
            kind: 0,
            pos: [0.5, 64.0, 0.5],
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        all.apply(EntityEvent::Add(Box::new(add)));
        all
    }

    pub fn fire(all: &mut Entities, event: i8) {
        all.apply(EntityEvent::Event { id: 7, event });
    }

    /// Layer of named parts (parent by index, `None` = root), zero poses.
    pub fn test_layer(parts: &[(&str, Option<usize>)]) -> BakedLayer {
        let mut known = [u16::MAX; KNOWN];
        for (i, (n, _)) in parts.iter().enumerate() {
            if let Some(k) = known_id(n) {
                known[k] = i as u16;
            }
        }
        BakedLayer {
            parts: parts
                .iter()
                .map(|&(_, parent)| BakedPart {
                    parent: parent.map_or(u32::MAX, |p| p as u32),
                    pose: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
                    quads: Vec::new(),
                })
                .collect(),
            known,
            names: parts.iter().map(|(n, _)| (*n).to_owned()).collect(),
        }
    }

    pub fn state(e: &Entity, pt: f32) -> AnimState {
        let (walk_pos, walk_speed) = e.walk_at(pt);
        AnimState {
            walk_pos,
            walk_speed,
            age: e.age as f32 + pt,
            yaw: 0.0,
            pitch: 0.0,
            swing: 0.0,
            swing_right: true,
            swing_on: false,
            swing_kind: 1,
            crouching: false,
            baby: e.baby(),
            riding: false,
            arm_pose: [0, 0],
            arm_first_right: true,
            crossbow_held: false,
            swim: e.pose_anim.swim(pt),
            fall_flying: false,
            speed: 1.0,
            using: false,
            use_ticks: 0.0,
            crossbow_charge: 25.0,
            spear: [None; 2],
            use_right: true,
            in_water: e.in_water,
            piglin: 0,
        }
    }

    /// Pose animated by the full engine (families then common rules).
    pub fn posed<'a>(l: &'a BakedLayer, e: &Entity, pt: f32) -> Pose<'a> {
        let mut p = Pose::reset(l, Vec::new(), Vec::new());
        super::super::animate(&mut p, e, &state(e, pt), pt, true);
        p
    }

    pub fn part<'a>(p: &'a Pose<'_>, name: &str) -> &'a [f32; 9] {
        &p.parts[p.index(name).unwrap()]
    }

    /// Animation definitions of the local cache (26.3); false if absent (test skipped).
    pub fn install_cached() -> bool {
        if DEFS.get().is_some() {
            return true;
        }
        match rc_testdata::version_data() {
            Some(v) => {
                install(&Arc::new(v));
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every animation played by the families exists in the extracted definitions (names taken
    /// from the game's classes by the extractor: a misspelt name would leave the model frozen).
    #[test]
    fn played_animation_names_exist() {
        if !testkit::install_cached() {
            return;
        }
        let names = [
            "armadillo.walk",
            "armadillo.roll_up",
            "armadillo.roll_out",
            "armadillo.peek",
            "baby_armadillo.armadillo_baby_walk",
            "baby_armadillo.armadillo_baby_roll_up",
            "baby_armadillo.armadillo_baby_roll_out",
            "baby_armadillo.armadillo_baby_peek",
            "baby_axolotl.axolotl_walk_floor",
            "baby_axolotl.swim",
            "baby_axolotl.walk_floor_underwater",
            "baby_axolotl.idle_floor",
            "baby_axolotl.idle_underwater",
            "baby_axolotl.idle_floor_underwater",
            "baby_axolotl.play_dead",
            "bat.flying",
            "bat.resting",
            "breeze.idle",
            "breeze.shoot",
            "breeze.slide",
            "breeze.slide_back",
            "breeze.inhale",
            "breeze.jump",
            "copper_golem.walk",
            "copper_golem.walk_item",
            "copper_golem.idle",
            "copper_golem.chest_interaction_noitem_get",
            "copper_golem.chest_interaction_noitem_noget",
            "copper_golem.chest_interaction_item_drop",
            "copper_golem.chest_interaction_item_nodrop",
            "creaking.walk",
            "creaking.attack",
            "creaking.invulnerable",
            "creaking.death",
            "fox_baby.walk",
            "frog.jump",
            "frog.croak",
            "frog.tongue",
            "frog.swim",
            "frog.walk",
            "frog.idle_water",
            "nautilus.swimming",
            "sniffer.walk",
            "sniffer.sniff_search",
            "sniffer.dig",
            "sniffer.longsniff",
            "sniffer.stand_up",
            "sniffer.happy",
            "sniffer.sniffsniff",
            "warden.attack",
            "warden.sonic_boom",
            "warden.dig",
            "warden.emerge",
            "warden.roar",
            "warden.sniff",
        ];
        let missing: Vec<_> = names.iter().filter(|n| get(n).is_none()).collect();
        assert!(missing.is_empty(), "missing: {missing:?}");
    }

    fn kf(time: f32, v: f32, catmull: bool) -> rc_data::Keyframe {
        rc_data::Keyframe { time, pre: [v; 3], post: [v; 3], catmull }
    }

    #[test]
    fn linear_and_bounds() {
        let k = [kf(0.0, 0.0, false), kf(1.0, 2.0, false)];
        assert_eq!(sample(&k, 0.5, 1.0), [1.0; 3]);
        assert_eq!(sample(&k, 0.5, 0.5), [0.5; 3]);
        // Before the first and after the last keyframe: values held.
        assert_eq!(sample(&k, -1.0, 1.0), [0.0; 3]);
        assert_eq!(sample(&k, 3.0, 1.0), [2.0; 3]);
        // Exactly on a keyframe: it is the next one (alpha = 1 towards it).
        assert_eq!(sample(&k, 1.0, 1.0), [2.0; 3]);
    }

    #[test]
    fn catmull_rom_passes_through_keys() {
        let k = [kf(0.0, 0.0, true), kf(1.0, 1.0, true), kf(2.0, 0.0, true)];
        assert!((sample(&k, 1.0, 1.0)[0] - 1.0).abs() < 1e-6);
        // Symmetric around the peak.
        let (l, r) = (sample(&k, 0.75, 1.0)[0], sample(&k, 1.25, 1.0)[0]);
        assert!((l - r).abs() < 1e-6 && l < 1.0 && l > 0.5);
    }
}
