//! Shulker (`server-mobs.md`, rendering): shell that opens while rotating, head that aims,
//! orientation depending on the attach face, colour, visual slide after a teleport.
//!
//! Entry points (called by `entity_render.rs` before its own rules):
//! - `layers` / `skin`: texture by colour (16 = shulker without colour);
//! - `tick`: client-side opening (follows value 17 by 0.05 per tick), slide countdown;
//! - `teleported`: new block received, start of the 6-tick slide;
//! - `render_offset`: slide offset (drawing root);
//! - `rotations` / `attach`: body always at 0° client-side, then rotation of the attach face;
//! - `animate`: lid (height, rotation), head.
//!
//! Client state: `extra[0]` = previous opening, `extra[1]` = current opening,
//! `extra[2]` = remaining slide ticks; `history[0]` = old attach block.
//! Metadata: 16 = attach face, 17 = raw opening (0..100), 18 = colour.

use super::*;

const KIND: &str = "shulker";

/// Textures per dye colour (game index), then without colour.
const TEXTURES: [&str; 17] = [
    "entity/shulker/shulker_white",
    "entity/shulker/shulker_orange",
    "entity/shulker/shulker_magenta",
    "entity/shulker/shulker_light_blue",
    "entity/shulker/shulker_yellow",
    "entity/shulker/shulker_lime",
    "entity/shulker/shulker_pink",
    "entity/shulker/shulker_gray",
    "entity/shulker/shulker_light_gray",
    "entity/shulker/shulker_cyan",
    "entity/shulker/shulker_purple",
    "entity/shulker/shulker_blue",
    "entity/shulker/shulker_brown",
    "entity/shulker/shulker_green",
    "entity/shulker/shulker_red",
    "entity/shulker/shulker_black",
    "entity/shulker/shulker",
];

/// No shadow (radius 0 in the game).
pub(super) fn shadow(k: &str) -> Option<(f32, f32)> {
    (k == KIND).then_some((0.0, 0.0))
}

pub(super) fn layers(
    _r: &EntityRenderer,
    k: &str,
    _baby: bool,
    skin: u32,
) -> Option<Vec<(String, String, Pass, Wool)>> {
    (k == KIND).then(|| {
        let tex = TEXTURES[(skin as usize).min(16)];
        vec![("minecraft:shulker#main".to_owned(), tex.to_owned(), Pass::Cutout, Wool::None)]
    })
}

/// Colour: value 18 is 16 (no colour) by default; outside 0..15, no colour.
pub(super) fn skin(_r: &EntityRenderer, e: &Entity) -> Option<u32> {
    if e.kind != KIND {
        return None;
    }
    let c = match e.meta.get(&18) {
        Some(rc_proto_v777::entity::Meta::Byte(b)) => i32::from(*b),
        _ => 16,
    };
    Some(if (0..16).contains(&c) { c as u32 } else { 16 })
}

/// One client tick: the opening approaches the target (value 17 × 0.01) by at most 0.05;
/// the slide counts down, then the old block is forgotten.
pub(super) fn tick(e: &mut Entity) {
    if e.kind != KIND {
        return;
    }
    let cur = e.extra[1];
    e.extra[0] = cur;
    let target = f32::from(e.meta_byte(17)) * 0.01;
    if cur > target {
        e.extra[1] = (cur - 0.05).clamp(target, 1.0);
    } else if cur < target {
        e.extra[1] = (cur + 0.05).clamp(0.0, target);
    }
    if e.extra[2] > 0.0 {
        e.extra[2] -= 1.0;
    } else {
        e.history.clear();
    }
}

fn block_of(p: [f64; 3]) -> [f64; 3] {
    [p[0].floor(), p[1].floor(), p[2].floor()]
}

/// Position set while not mounted: re-centred on the block (x and z at the middle, y rounded to the
/// nearest block), as the game does on every placement.
fn snapped(p: [f64; 3]) -> [f64; 3] {
    [p[0].floor() + 0.5, (p[1] + 0.5).floor(), p[2].floor() + 0.5]
}

/// Received position (the game never interpolates it): re-centred when not mounted; if the block changes
/// after the first tick, the target opening falls back to 0 locally and, when not mounted, the drawing
/// slides from the old block for 6 ticks.
pub fn teleported(e: &mut Entity, old: [f64; 3]) {
    if e.kind != KIND {
        return;
    }
    if e.vehicle.is_none() {
        e.pos = snapped(e.pos);
    }
    let (from, to) = (block_of(old), block_of(e.pos));
    if e.age == 0 || from == to {
        return;
    }
    e.meta.insert(17, rc_proto_v777::entity::Meta::Byte(0));
    if e.vehicle.is_some() || e.history.first() == Some(&to) {
        return;
    }
    e.history.clear();
    e.history.push(from);
    e.extra[2] = 6.0;
    // No position interpolation on top of the slide.
    e.pos_o = e.pos;
}

/// Mounting: slide cancelled, attachment set on the ground locally. Dismounting:
/// the old block becomes the current block (no new slide) and the body returns to 0°.
pub fn riding_changed(e: &mut Entity, mounted: bool) {
    if e.kind != KIND {
        return;
    }
    e.history.clear();
    if mounted {
        e.extra[2] = 0.0;
        e.meta.insert(16, rc_proto_v777::entity::Meta::Direction(0));
    } else {
        e.history.push(block_of(e.pos));
        (e.body, e.body_o) = (0.0, 0.0);
    }
}

/// Render offset of the slide: `-(block − old block) × ((remaining − pt) / 6)² × scale`
/// (the entity scale, capped at 3, multiplies the offset, which is not scaled
/// by the matrix).
pub(super) fn render_offset(e: &Entity, pt: f32) -> Option<[f64; 3]> {
    if e.kind != KIND || e.extra[2] <= 0.0 {
        return None;
    }
    let old = e.history.first()?;
    let s = (f64::from(e.extra[2]) - f64::from(pt)) / 6.0;
    let s = s * s * f64::from(living_scale(e));
    let b = block_of(e.pos);
    Some([-(b[0] - old[0]) * s, -(b[1] - old[1]) * s, -(b[2] - old[2]) * s])
}

/// Body rotation offset by 180° by the game: `180 − (body + 180)` = −body. The body
/// stays at 0° client-side (empty rotation control), unless set by a mount.
pub(super) fn rotations(e: &Entity, pt: f32, m: &mut Mat4) -> bool {
    if e.kind != KIND {
        return false;
    }
    *m *= Mat4::from_rotation_y(-rot_lerp(pt, e.body_o, e.body) * (PI / 180.0));
    true
}

/// Rotation of the face opposite the attach face around the point (0, 0.5, 0), after the
/// death tilt.
pub(super) fn attach(e: &Entity, m: &mut Mat4) {
    if e.kind != KIND {
        return;
    }
    let face = match e.meta.get(&16) {
        Some(rc_proto_v777::entity::Meta::Direction(d)) => *d,
        _ => 0,
    };
    // Opposite: down ↔ up, north ↔ south, west ↔ east (the game's 3D ids).
    let opposite = face ^ 1;
    let h = PI / 2.0;
    let r = match opposite {
        0 => Mat4::from_rotation_x(PI),
        2 => Mat4::from_rotation_x(h) * Mat4::from_rotation_z(PI),
        3 => Mat4::from_rotation_x(h),
        4 => Mat4::from_rotation_x(h) * Mat4::from_rotation_z(h),
        5 => Mat4::from_rotation_x(h) * Mat4::from_rotation_z(-h),
        _ => return,
    };
    let pivot = Vec3::new(0.0, 0.5, 0.0);
    *m *= Mat4::from_translation(pivot) * r * Mat4::from_translation(-pivot);
}

/// Displayed opening (interpolated between two ticks).
fn peek(e: &Entity, pt: f32) -> f32 {
    e.extra[0] + (e.extra[1] - e.extra[0]) * pt
}

pub(super) fn animate(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) -> bool {
    if e.kind != KIND {
        return false;
    }
    let bs = (0.5 + peek(e, pt)) * PI;
    let q = -1.0 + bs.sin();
    let extra = if bs > PI { (s.age * 0.1).sin() * 0.7 } else { 0.0 };
    let peek = peek(e, pt);
    p.set("lid", |l| {
        l[0] = 0.0;
        l[1] = 16.0 + bs.sin() * 8.0 + extra;
        l[2] = 0.0;
        l[4] = if peek > 0.3 { q * q * q * q * PI * 0.125 } else { 0.0 };
    });
    // Head: interpolated pitch, raw head yaws minus 180° minus the body.
    let pitch = e.pitch_o + (e.pitch - e.pitch_o) * pt;
    p.set("head", |h| {
        h[3] = pitch * (PI / 180.0);
        h[4] = (e.head - 180.0 - e.body) * (PI / 180.0);
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::entities::Entities;
    use rc_client::EntityEvent;
    use rc_proto_v777::entity::{AddEntity, Meta};

    /// A shulker spawned at (x, y, z), head yaw 40°, id 7.
    fn spawn(pos: [f64; 3]) -> Entities {
        let mut all =
            Entities { type_names: vec!["minecraft:shulker".into(), "minecraft:boat".into()], ..Default::default() };
        let add = AddEntity {
            id: 7,
            uuid: 1,
            kind: 0,
            pos,
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 40.0,
            data: 0,
        };
        all.apply(EntityEvent::Add(Box::new(add)));
        all
    }

    #[test]
    fn spawn_snaps_and_zeroes_body() {
        let all = spawn([3.2, 64.6, -1.9]);
        let e = &all.map[&7];
        assert_eq!(e.pos, [3.5, 65.0, -1.5]);
        assert_eq!(e.pos_o, e.pos);
        assert_eq!((e.body, e.body_o, e.head), (0.0, 0.0, 40.0));
    }

    #[test]
    fn new_block_resets_peek_and_glides() {
        let mut all = spawn([0.5, 64.0, 0.5]);
        all.tick();
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(17, Meta::Byte(100));
        // Same block: nothing changes.
        e.snap_to([0.7, 64.2, 0.4], 0.0, 0.0);
        assert_eq!(e.pos, [0.5, 64.0, 0.5]);
        assert_eq!(e.meta_byte(17), 100);
        assert!(render_offset(e, 0.0).is_none());
        // Three blocks further: target opening at 0, 6-tick slide from the old block.
        e.snap_to([3.5, 64.0, 0.5], 0.0, 0.0);
        assert_eq!(e.meta_byte(17), 0);
        assert_eq!(e.pos_o, e.pos);
        let o = render_offset(e, 0.0).unwrap();
        assert!((o[0] + 3.0).abs() < 1e-9 && o[1] == 0.0 && o[2] == 0.0);
        let o = render_offset(e, 0.5).unwrap();
        assert!((o[0] + 3.0 * (5.5f64 / 6.0).powi(2)).abs() < 1e-9);
    }

    #[test]
    fn body_control_is_inert() {
        let mut all = spawn([0.5, 64.0, 0.5]);
        all.tick();
        let e = all.map.get_mut(&7).unwrap();
        e.head = 170.0;
        e.snap_to([5.5, 64.0, 0.5], 90.0, 0.0);
        all.tick();
        let e = &all.map[&7];
        // The mob control would have aligned the body with the yaw and brought the head back to 75°.
        assert_eq!((e.body, e.head), (0.0, 170.0));
    }

    #[test]
    fn riding_cancels_glide_and_attaches_down() {
        let mut all = spawn([0.5, 64.0, 0.5]);
        all.tick();
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(16, Meta::Direction(1));
        e.snap_to([2.5, 64.0, 0.5], 0.0, 0.0);
        assert!(render_offset(e, 0.0).is_some());
        all.apply(EntityEvent::Passengers { vehicle: 7, passengers: vec![] });
        let add = AddEntity {
            id: 8,
            uuid: 2,
            kind: 1,
            pos: [2.5, 64.0, 0.5],
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        all.apply(EntityEvent::Add(Box::new(add)));
        all.apply(EntityEvent::Passengers { vehicle: 8, passengers: vec![7] });
        let e = all.map.get_mut(&7).unwrap();
        assert!(render_offset(e, 0.0).is_none());
        assert!(matches!(e.meta.get(&16), Some(Meta::Direction(0))));
        // Mounted: no re-centring; the body follows the mount.
        e.snap_to([2.3, 64.2, 0.5], 0.0, 0.0);
        assert_eq!(e.pos, [2.3, 64.2, 0.5]);
        e.body = 30.0;
        all.apply(EntityEvent::Passengers { vehicle: 8, passengers: vec![] });
        let e = &all.map[&7];
        assert_eq!((e.body, e.body_o), (0.0, 0.0));
        assert!(render_offset(e, 0.0).is_none());
    }

    #[test]
    fn removed_vehicle_dismounts_shulker() {
        let mut all = spawn([0.5, 64.0, 0.5]);
        all.tick();
        let add = AddEntity {
            id: 8,
            uuid: 2,
            kind: 1,
            pos: [2.5, 64.0, 0.5],
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        all.apply(EntityEvent::Add(Box::new(add)));
        all.apply(EntityEvent::Passengers { vehicle: 8, passengers: vec![7] });
        let e = all.map.get_mut(&7).unwrap();
        e.snap_to([2.3, 64.2, 0.5], 0.0, 0.0);
        e.body = 30.0;
        assert_eq!(e.vehicle, Some(8));
        all.apply(EntityEvent::Remove(vec![8]));
        let e = &all.map[&7];
        assert_eq!(e.vehicle, None);
        assert_eq!((e.body, e.body_o), (0.0, 0.0));
        assert_eq!(e.history, vec![block_of(e.pos)]);
        // Free: a move to another block re-centres and triggers the slide.
        let e = all.map.get_mut(&7).unwrap();
        e.snap_to([5.3, 64.2, 0.5], 0.0, 0.0);
        assert_eq!(e.pos, [5.5, 64.0, 0.5]);
        assert!(render_offset(e, 0.0).is_some());
    }

    #[test]
    fn glide_offset_scales_with_entity() {
        let mut all = spawn([0.5, 64.0, 0.5]);
        all.tick();
        let e = all.map.get_mut(&7).unwrap();
        e.snap_to([2.5, 64.0, 0.5], 0.0, 0.0);
        let x = |e: &Entity| render_offset(e, 0.0).expect("slide")[0];
        assert!((x(e) + 2.0).abs() < 1e-9);
        e.scale = 2.0;
        assert!((x(e) + 4.0).abs() < 1e-9);
        // Shulker scale capped at 3.
        e.scale = 10.0;
        assert!((x(e) + 6.0).abs() < 1e-9);
    }

    #[test]
    fn living_scale_bounds() {
        let mut all = spawn([0.5, 64.0, 0.5]);
        let e = all.map.get_mut(&7).unwrap();
        e.scale = 0.25;
        assert_eq!(living_scale(e), 0.25);
        e.scale = 5.0;
        assert_eq!(living_scale(e), 3.0);
        e.kind = "happy_ghast".to_owned();
        assert_eq!(living_scale(e), 1.0);
        e.kind = "ender_dragon".to_owned();
        assert_eq!(living_scale(e), 1.0);
        e.kind = "zombie".to_owned();
        assert_eq!(living_scale(e), 5.0);
    }

    #[test]
    fn peek_moves_by_a_twentieth_toward_data() {
        let mut all = spawn([0.5, 64.0, 0.5]);
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(17, Meta::Byte(30));
        for want in [0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.3] {
            tick(e);
            assert!((e.extra[1] - want).abs() < 1e-6, "{} vs {want}", e.extra[1]);
        }
        assert!((peek(e, 0.5) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn no_shadow() {
        assert_eq!(shadow_of("shulker"), (0.0, 0.0));
    }

    #[test]
    fn opposite_faces() {
        // down 0 ↔ up 1, north 2 ↔ south 3, west 4 ↔ east 5.
        assert_eq!([0, 1, 2, 3, 4, 5].map(|f: i32| f ^ 1), [1, 0, 3, 2, 5, 4]);
    }

    #[test]
    fn up_attach_turns_upside_down() {
        // Attached to the ceiling (up face): opposite = down, half-turn around X at 0.5.
        let r = Mat4::from_translation(Vec3::new(0.0, 0.5, 0.0))
            * Mat4::from_rotation_x(PI)
            * Mat4::from_translation(Vec3::new(0.0, -0.5, 0.0));
        let p = r.transform_point3(Vec3::ZERO);
        assert!((p - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5);
    }
}
