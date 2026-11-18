//! Old path of the render distance and of the shadow factor (`RUSTCRAFT_OPT_KIND_SIZE_RULES=0`):
//! every call compares the type name with the strings of each special case. Kept unchanged
//! until the quiet-machine bench settles the switch; the rules stored at spawn must give the same
//! results (`tests_size_rules.rs`).

use super::{Entity, Kind, Size, objects};

/// `within_render_distance_of` by name.
pub(super) fn within_render_distance_of(e: &Entity, d2: f64, view_scale: f64, full: impl Fn() -> Size) -> bool {
    // Mean side of the current box (size of cubes, babies, scale included).
    let size = || {
        let s = full();
        (f64::from(s.w) + f64::from(s.h) + f64::from(s.w)) / 3.0
    };
    let within = |s: f64| d2 < s * s;
    let young = e.age < 2 && d2 < 12.25;
    match e.kind.as_str() {
        "phantom" | "trident" => true,
        "player" | "arrow" | "spectral_arrow" => within(size() * 10.0 * 64.0 * view_scale),
        "armor_stand" => {
            let s = size() * 4.0;
            within(if s == 0.0 { 4.0 } else { s } * 64.0)
        }
        "item_frame" | "glow_item_frame" => within(16.0 * 64.0 * view_scale),
        "leash_knot" => d2 < 1024.0,
        "block_display" | "item_display" | "text_display" => {
            let range = match e.meta.get(&17) {
                Some(rc_proto_v777::entity::Meta::Float(v)) => f64::from(*v),
                _ => 1.0,
            };
            within(range * 64.0 * view_scale)
        }
        "lightning_bolt" => within(64.0 * view_scale),
        "firework_rocket" | "fishing_bobber" => d2 < 4096.0,
        "shulker_bullet" => d2 < 16384.0,
        "fireball" | "small_fireball" | "wind_charge" | "breeze_wind_charge" if young => false,
        "snowball" | "egg" | "ender_pearl" | "splash_potion" | "lingering_potion" | "experience_bottle"
        | "eye_of_ender"
            if young =>
        {
            false
        }
        "fireball" | "small_fireball" | "dragon_fireball" | "wither_skull" | "wind_charge" | "breeze_wind_charge"
        | "snowball" | "egg" | "ender_pearl" | "splash_potion" | "lingering_potion" | "experience_bottle"
        | "eye_of_ender" => within(size() * 4.0 * 64.0),
        "end_crystal" => objects::beam_target(e).is_some() || within(size() * 64.0 * view_scale),
        _ => within(size() * 64.0 * view_scale),
    }
}

/// `shadow_scale` by name.
pub(super) fn shadow_scale(e: &Entity) -> f32 {
    match e.kind.as_str() {
        "slime" | "magma_cube" | "sulfur_cube" => return e.meta_int(18).unwrap_or(1).clamp(1, 127) as f32,
        "pufferfish" => return 1.0 + e.meta_int(17).unwrap_or(0).clamp(0, 2) as f32,
        _ => {}
    }
    let attr = if e.class != Kind::Other { living_scale(e) } else { 1.0 };
    if matches!(e.kind.as_str(), "player" | "mannequin" | "armor_stand") || !crate::entities::sizes_by_name::baby(e) {
        return attr;
    }
    let (age, extra) = match e.kind.as_str() {
        "armadillo" | "camel" | "camel_husk" => (0.6, 1.0),
        "dolphin" => (0.65, 1.0),
        "goat" => (0.55, 1.0),
        "happy_ghast" => (0.2375, 1.0),
        "turtle" => (0.3, 0.83),
        "villager" | "strider" => (0.5, 0.5),
        _ => (0.5, 1.0),
    };
    age * extra * attr
}

/// `living_scale` by name.
pub(super) fn living_scale(e: &Entity) -> f32 {
    match e.kind.as_str() {
        "ender_dragon" => 1.0,
        "shulker" => e.scale.min(3.0),
        "happy_ghast" => e.scale.min(1.0),
        _ => e.scale,
    }
}

/// `crouch_offset` by name.
pub(super) fn crouch_offset(e: &Entity) -> f64 {
    if matches!(e.kind.as_str(), "player" | "mannequin") && e.pose() == 5 {
        -f64::from(living_scale(e)) * 2.0 / 16.0
    } else {
        0.0
    }
}
