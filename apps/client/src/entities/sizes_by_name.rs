//! Old path of the entity sizes (`RUSTCRAFT_OPT_KIND_SIZE_RULES=0`): every call compares the type
//! name with the strings of each special case. Kept unchanged until the quiet-machine bench
//! settles the switch; the rules stored at spawn (`size_rules.rs`) must give the same bits
//! (`tests_size_rules.rs`).

use rc_proto_v777::entity::Meta;

use super::{Entity, Kind, Size};

/// Dimensions of a baby: specific to the type (fixed size, or type scaled with given
/// eyes), otherwise type × 1/2. `ty`: adult dimensions; `pose`: current pose.
pub(crate) fn baby_size(kind: &str, ty: Size, pose: i32) -> Size {
    let own = |w: f32, h: f32, eye: f32| Size::scalable(w, h).eye(eye);
    // Scaled type whose attachments are rebuilt: name at the top.
    let rebuilt = |k: f32| {
        let s = ty.scale(k);
        Size { tag: s.h, ..s }
    };
    match kind {
        "armadillo" => ty.scale(0.6).eye(0.218_75),
        "axolotl" => own(0.375, 0.21, 0.093_75),
        "camel" | "camel_husk" if pose != 10 => own(0.95, 1.4, 1.38),
        "chicken" => own(0.3, 0.4, 0.281_25),
        "cow" | "mooshroom" => own(0.45, 0.7, 0.69),
        "dolphin" => {
            let s = ty.scale(0.65).eye(0.093_75);
            Size { tag: s.h, ..s }
        }
        "donkey" | "mule" | "llama" | "trader_llama" => rebuilt(0.5),
        "horse" | "skeleton_horse" | "zombie_horse" => rebuilt(0.7),
        "cat" | "ocelot" => own(0.3, 0.35, 0.343_75),
        "fox" => {
            let s = ty.scale(0.6).eye(0.343_75);
            Size { tag: s.h, ..s }
        }
        "goat" => own(0.45, 0.65, 0.593_75),
        "happy_ghast" => ty.scale(0.2375).eye(0.468_75),
        "nautilus" => rebuilt(0.5),
        "panda" => {
            let s = ty.scale(0.5).eye(0.281_25);
            Size { tag: s.h, ..s }
        }
        "pig" => own(0.45, 0.45, 0.406_25),
        "polar_bear" => own(0.7, 0.7, 0.343_75),
        "rabbit" => own(0.24, 0.4, 0.39),
        "sheep" => own(0.45, 0.65, 0.656_25),
        "squid" | "glow_squid" => own(0.5, 0.5, 0.37),
        "turtle" => rebuilt(0.3),
        "wolf" => own(0.3, 0.425, 0.343_75),
        "strider" => own(0.45, 0.85, 0.4375),
        "hoglin" | "zoglin" => own(0.75, 0.85, 0.625),
        "piglin" | "zombified_piglin" => own(0.49, 0.98, 0.78),
        "zombie" | "drowned" => own(0.49, 0.98, 0.775),
        "husk" => own(0.49, 0.98, 0.825),
        "zombie_villager" => own(0.49, 0.98, 0.67),
        "villager" => own(0.49, 0.98, 0.63),
        _ => ty.scale(0.5),
    }
}

/// `Entity::type_size` as the old path had it: inlined where it is used.
fn type_size(e: &Entity, table: &[[f32; 4]]) -> Size {
    let d = usize::try_from(e.kind_id).ok().and_then(|i| table.get(i)).copied().unwrap_or([0.6, 1.8, 1.62, 1.8]);
    Size { w: d[0], h: d[1], eye: d[2], tag: d[3], fixed: false }
}

/// `Entity::size_factor` by name.
pub(crate) fn size_factor(e: &Entity) -> Option<f32> {
    match e.kind.as_str() {
        "slime" | "magma_cube" | "sulfur_cube" => Some(e.meta_int(18).unwrap_or(1).clamp(1, 127) as f32),
        "phantom" => Some(1.0 + 0.15 * e.meta_int(16).unwrap_or(0).clamp(0, 64) as f32),
        _ => None,
    }
}

/// `Entity::base_size` by name.
pub(crate) fn base_size(e: &Entity, table: &[[f32; 4]]) -> Size {
    let living = e.class != Kind::Other;
    let pose = e.pose();
    // Sleeping: 0.2 × 0.2 fixed, for any living entity.
    if living && pose == 2 {
        return Size::fixed(0.2, 0.2).eye(0.2);
    }
    let ty = type_size(e, table);
    let float = |i: u8, d: f32| match e.meta.get(&i) {
        Some(Meta::Float(v)) => *v,
        _ => d,
    };
    match e.kind.as_str() {
        "player" | "mannequin" => {
            return match pose {
                1 | 3 | 4 => Size::scalable(0.6, 0.6).eye(0.4),
                5 => Size::scalable(0.6, 1.5).eye(1.27),
                7 => Size::fixed(0.2, 0.2).eye(1.62),
                _ => Size::scalable(0.6, 1.8).eye(1.62),
            };
        }
        // Armor stand: marker without size, small = baby.
        "armor_stand" => {
            let flags = e.meta_byte(15);
            return if flags & 0x10 != 0 {
                Size::fixed(0.0, 0.0)
            } else if flags & 0x01 != 0 {
                ty.scale(0.5).eye(0.9875)
            } else {
                ty
            };
        }
        "interaction" => return Size::scalable(float(8, 1.0), float(9, 1.0)),
        "area_effect_cloud" => return Size::scalable(float(8, 3.0) * 2.0, 0.5),
        // Cubes: type × size, without age scale.
        "slime" | "magma_cube" | "sulfur_cube" => return ty.scale(size_factor(e).unwrap_or(1.0)),
        _ => {}
    }
    if !living {
        return ty;
    }
    let baby = baby(e);
    let base = if baby { baby_size(&e.kind, ty, pose) } else { ty };
    match e.kind.as_str() {
        "phantom" => base.scale(size_factor(e).unwrap_or(1.0)),
        "pufferfish" => base.scale(match e.meta_int(17).unwrap_or(0) {
            0 => 0.5,
            1 => 0.7,
            _ => 1.0,
        }),
        "salmon" => base.scale(match e.meta_int(17).unwrap_or(1) {
            0 => 0.5,
            2 => 1.5,
            _ => 1.0,
        }),
        // Open, resting on the ground: the height follows the opening.
        "shulker" if e.peek > 0.0 && matches!(e.meta.get(&16), Some(Meta::Direction(0)) | None) => {
            base.scale2(1.0, 1.0 + e.peek)
        }
        // Standing bear: client-specific animation (0 to 6).
        "polar_bear" if e.extra[0] > 0.0 => base.scale2(1.0, 1.0 + e.extra[0] / 6.0),
        "goat" if pose == 6 => base.scale(0.7),
        "camel" | "camel_husk" if pose == 10 => {
            if baby {
                Size::scalable(0.95, 0.425).eye(0.41)
            } else {
                Size::scalable(ty.w, ty.h - 1.43).eye(0.845)
            }
        }
        // Digging (state 5 of entity data 18; 17 is the age lock): 0.4 lower.
        "sniffer" if e.meta_int(18) == Some(5) => {
            Size::scalable(ty.w, ty.h - 0.4).eye(0.81).scale(if baby { 0.5 } else { 1.0 })
        }
        "warden" if matches!(pose, 13 | 14) => Size::fixed(base.w, 1.0),
        _ => base,
    }
}

/// `Entity::standing_eye` by name.
pub(crate) fn standing_eye(e: &Entity, table: &[[f32; 4]]) -> f32 {
    let ty = type_size(e, table);
    let s = match e.kind.as_str() {
        "player" | "mannequin" => Size::scalable(0.6, 1.8).eye(1.62),
        _ if baby(e) => baby_size(&e.kind, ty, 0),
        _ => ty,
    };
    s.scale(attribute_scale(e)).eye
}

/// `Entity::attribute_scale` by name.
pub(crate) fn attribute_scale(e: &Entity) -> f32 {
    match e.kind.as_str() {
        "happy_ghast" => e.scale.min(1.0),
        "shulker" => e.scale.min(3.0),
        "ender_dragon" => 1.0,
        _ => e.scale,
    }
}

/// `Entity::baby` by name.
pub(crate) fn baby(e: &Entity) -> bool {
    e.class == Kind::Mob && e.meta_bool(if e.kind == "piglin" { 17 } else { 16 })
}
