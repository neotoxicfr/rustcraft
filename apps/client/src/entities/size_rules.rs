//! Size, age and render-distance rules of an entity type, worked out once from the type name when
//! the entity spawns (`KindFacts::size`). The code that runs for every entity on every frame
//! (current size, baby size, scale bound, shadow factor, render distance) then matches these small
//! enums instead of comparing the type name with dozens of strings at every call.
//!
//! The rules are a few bytes with no float inside (the baby sizes and shadow factors are indices
//! into constant tables): stored in every entity, they pack among its other small fields
//! (`size_rules_stay_small`).
//!
//! Every rule is defined by the same names as the string code it replaces, which stays behind
//! `RUSTCRAFT_OPT_KIND_SIZE_RULES=0` (`sizes_by_name.rs`); `tests_size_rules.rs` checks that both
//! give the same bits for every type of the registry.

use super::Size;

/// `RUSTCRAFT_OPT_KIND_SIZE_RULES=0`: sizes, scale bounds, shadow factors and render distances
/// found by comparing the type name (old path) instead of reading the rules stored at spawn.
pub(crate) fn kind_size_rules_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_KIND_SIZE_RULES")
}

/// Special case of a type's current size (`Entity::base_size`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SizeClass {
    /// The type's size (babies and scale aside).
    #[default]
    Plain,
    /// Player or mannequin: size by pose.
    Avatar,
    /// Armor stand: a marker has no size, a small one is half size.
    ArmorStand,
    /// Interaction: width and height from its data.
    Interaction,
    /// Area effect cloud: width from its radius.
    EffectCloud,
    /// Slime, magma cube, sulfur cube: type size × synchronised size.
    Cube,
    /// Phantom: × (1 + 0.15 × size).
    Phantom,
    /// Pufferfish: × inflation.
    Pufferfish,
    /// Salmon: × size variant.
    Salmon,
    /// Shulker: height follows the opening.
    Shulker,
    /// Polar bear: height follows the standing animation.
    PolarBear,
    /// Goat: smaller while ramming.
    Goat,
    /// Camel or camel husk: lower while sitting.
    Camel,
    /// Sniffer: lower while digging.
    Sniffer,
    /// Warden: one block high while emerging or digging.
    Warden,
}

/// Size of a baby of the type (pose aside).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BabySize {
    /// Half the type's size (most types).
    Half,
    /// Its own width, height and eye height.
    Own { w: f32, h: f32, eye: f32 },
    /// Its own size while not sitting (pose 10), half the type's size while sitting (camels).
    OwnUnlessSitting { w: f32, h: f32, eye: f32 },
    /// The type's size scaled by `k`, with its own eye height.
    Scaled { k: f32, eye: f32 },
    /// The type's size scaled by `k`, its own eye height, name tag at the top.
    ScaledNameOnTop { k: f32, eye: f32 },
    /// The type's size scaled by `k`, name tag at the top.
    RebuiltNameOnTop { k: f32 },
}

impl BabySize {
    /// The baby's size for adult size `ty` in pose `pose`.
    pub fn size(self, ty: Size, pose: i32) -> Size {
        let name_on_top = |s: Size| Size { tag: s.h, ..s };
        match self {
            BabySize::Half => ty.scale(0.5),
            BabySize::Own { w, h, eye } => Size::scalable(w, h).eye(eye),
            BabySize::OwnUnlessSitting { w, h, eye } if pose != 10 => Size::scalable(w, h).eye(eye),
            BabySize::OwnUnlessSitting { .. } => ty.scale(0.5),
            BabySize::Scaled { k, eye } => ty.scale(k).eye(eye),
            BabySize::ScaledNameOnTop { k, eye } => name_on_top(ty.scale(k).eye(eye)),
            BabySize::RebuiltNameOnTop { k } => name_on_top(ty.scale(k)),
        }
    }
}

const fn own(w: f32, h: f32, eye: f32) -> BabySize {
    BabySize::Own { w, h, eye }
}

/// Baby sizes of the types that have their own; index 0 (no name): half the type's size.
const BABY_SIZES: [(&[&str], BabySize); 29] = [
    (&[], BabySize::Half),
    (&["armadillo"], BabySize::Scaled { k: 0.6, eye: 0.218_75 }),
    (&["axolotl"], own(0.375, 0.21, 0.093_75)),
    (&["camel", "camel_husk"], BabySize::OwnUnlessSitting { w: 0.95, h: 1.4, eye: 1.38 }),
    (&["chicken"], own(0.3, 0.4, 0.281_25)),
    (&["cow", "mooshroom"], own(0.45, 0.7, 0.69)),
    (&["dolphin"], BabySize::ScaledNameOnTop { k: 0.65, eye: 0.093_75 }),
    (&["donkey", "mule", "llama", "trader_llama"], BabySize::RebuiltNameOnTop { k: 0.5 }),
    (&["horse", "skeleton_horse", "zombie_horse"], BabySize::RebuiltNameOnTop { k: 0.7 }),
    (&["cat", "ocelot"], own(0.3, 0.35, 0.343_75)),
    (&["fox"], BabySize::ScaledNameOnTop { k: 0.6, eye: 0.343_75 }),
    (&["goat"], own(0.45, 0.65, 0.593_75)),
    (&["happy_ghast"], BabySize::Scaled { k: 0.2375, eye: 0.468_75 }),
    (&["nautilus"], BabySize::RebuiltNameOnTop { k: 0.5 }),
    (&["panda"], BabySize::ScaledNameOnTop { k: 0.5, eye: 0.281_25 }),
    (&["pig"], own(0.45, 0.45, 0.406_25)),
    (&["polar_bear"], own(0.7, 0.7, 0.343_75)),
    (&["rabbit"], own(0.24, 0.4, 0.39)),
    (&["sheep"], own(0.45, 0.65, 0.656_25)),
    (&["squid", "glow_squid"], own(0.5, 0.5, 0.37)),
    (&["turtle"], BabySize::RebuiltNameOnTop { k: 0.3 }),
    (&["wolf"], own(0.3, 0.425, 0.343_75)),
    (&["strider"], own(0.45, 0.85, 0.4375)),
    (&["hoglin", "zoglin"], own(0.75, 0.85, 0.625)),
    (&["piglin", "zombified_piglin"], own(0.49, 0.98, 0.78)),
    (&["zombie", "drowned"], own(0.49, 0.98, 0.775)),
    (&["husk"], own(0.49, 0.98, 0.825)),
    (&["zombie_villager"], own(0.49, 0.98, 0.67)),
    (&["villager"], own(0.49, 0.98, 0.63)),
];

/// Shadow radius factors of a baby (age scale, then the extra reduction of its renderer) of the
/// types that have their own; index 0 (no name): half, no reduction.
const BABY_SHADOWS: [(&[&str], (f32, f32)); 7] = [
    (&[], (0.5, 1.0)),
    (&["armadillo", "camel", "camel_husk"], (0.6, 1.0)),
    (&["dolphin"], (0.65, 1.0)),
    (&["goat"], (0.55, 1.0)),
    (&["happy_ghast"], (0.2375, 1.0)),
    (&["turtle"], (0.3, 0.83)),
    (&["villager", "strider"], (0.5, 0.5)),
];

/// Index of the entry of `table` naming `name`, 0 if none does.
fn index_of<T>(table: &[(&[&str], T)], name: &str) -> u8 {
    table.iter().position(|(names, _)| names.contains(&name)).map_or(0, |i| i as u8)
}

/// Bound of the scale attribute as the type accepts it (`Entity::attribute_scale`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScaleBound {
    #[default]
    Free,
    /// Happy ghast.
    AtMostOne,
    /// Shulker.
    AtMostThree,
    /// Ender dragon: never scaled.
    AlwaysOne,
}

/// How far from the camera an entity of the type is drawn (`within_render_distance`). `size` below
/// is the mean side of its current box, `scale` the view-distance scale of the options.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DistanceRule {
    /// size × 64 × scale (most types).
    #[default]
    BySize,
    /// Always drawn (phantom, trident).
    Always,
    /// size × 10 × 64 × scale (players, arrows).
    TenTimesSize,
    /// (size × 4, or 4 for a marker) × 64, whatever the scale (armor stand).
    ArmorStand,
    /// 16 × 64 × scale (item frames).
    ItemFrame,
    /// Within a fixed number of blocks (leash knot 32, firework rocket and bobber 64, shulker
    /// bullet 128).
    WithinBlocks(u8),
    /// Its own view range (data 17) × 64 × scale (displays).
    Display,
    /// 64 × scale (lightning).
    Lightning,
    /// size × 4 × 64, hidden while younger than 2 ticks within 3.5 blocks (fireballs, wind
    /// charges, thrown items).
    ProjectileHiddenWhenYoung,
    /// size × 4 × 64 (dragon fireball, wither skull).
    Projectile,
    /// Drawn while its beam is shown, otherwise size × 64 × scale (End crystal).
    EndCrystal,
}

impl DistanceRule {
    /// Whether the rule may read the entity's size (an End crystal reads it when it has no beam).
    pub fn reads_size(self) -> bool {
        matches!(
            self,
            DistanceRule::BySize
                | DistanceRule::TenTimesSize
                | DistanceRule::ArmorStand
                | DistanceRule::ProjectileHiddenWhenYoung
                | DistanceRule::Projectile
                | DistanceRule::EndCrystal
        )
    }
}

/// Every size rule of one type, stored in each entity at spawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizeRules {
    pub class: SizeClass,
    /// Data index of the baby flag (17 for the piglin, 16 otherwise).
    pub baby_flag_index: u8,
    /// Entry of the baby-size table.
    baby_size: u8,
    pub scale_bound: ScaleBound,
    pub distance: DistanceRule,
    /// Entry of the baby-shadow table.
    baby_shadow: u8,
}

impl Default for SizeRules {
    /// The rules of a type with no special case (the baby flag at its usual index).
    fn default() -> SizeRules {
        SizeRules::of("")
    }
}

impl SizeRules {
    /// Rules of the type named `name` (without namespace).
    pub fn of(name: &str) -> SizeRules {
        SizeRules {
            class: size_class(name),
            baby_flag_index: if name == "piglin" { 17 } else { 16 },
            baby_size: index_of(&BABY_SIZES, name),
            scale_bound: match name {
                "happy_ghast" => ScaleBound::AtMostOne,
                "shulker" => ScaleBound::AtMostThree,
                "ender_dragon" => ScaleBound::AlwaysOne,
                _ => ScaleBound::Free,
            },
            distance: distance_rule(name),
            baby_shadow: index_of(&BABY_SHADOWS, name),
        }
    }

    /// Size of a baby of the type.
    pub fn baby_size(&self) -> BabySize {
        BABY_SIZES[usize::from(self.baby_size)].1
    }

    /// Shadow radius factors of a baby of the type: age scale, then the extra reduction of its
    /// renderer.
    pub fn baby_shadow(&self) -> (f32, f32) {
        BABY_SHADOWS[usize::from(self.baby_shadow)].1
    }

    /// The same rules with another baby shadow (a test's sabotage).
    #[cfg(test)]
    pub fn with_baby_shadow_of(self, name: &str) -> SizeRules {
        SizeRules { baby_shadow: index_of(&BABY_SHADOWS, name), ..self }
    }
}

fn size_class(name: &str) -> SizeClass {
    match name {
        "player" | "mannequin" => SizeClass::Avatar,
        "armor_stand" => SizeClass::ArmorStand,
        "interaction" => SizeClass::Interaction,
        "area_effect_cloud" => SizeClass::EffectCloud,
        "slime" | "magma_cube" | "sulfur_cube" => SizeClass::Cube,
        "phantom" => SizeClass::Phantom,
        "pufferfish" => SizeClass::Pufferfish,
        "salmon" => SizeClass::Salmon,
        "shulker" => SizeClass::Shulker,
        "polar_bear" => SizeClass::PolarBear,
        "goat" => SizeClass::Goat,
        "camel" | "camel_husk" => SizeClass::Camel,
        "sniffer" => SizeClass::Sniffer,
        "warden" => SizeClass::Warden,
        _ => SizeClass::Plain,
    }
}

fn distance_rule(name: &str) -> DistanceRule {
    match name {
        "phantom" | "trident" => DistanceRule::Always,
        "player" | "arrow" | "spectral_arrow" => DistanceRule::TenTimesSize,
        "armor_stand" => DistanceRule::ArmorStand,
        "item_frame" | "glow_item_frame" => DistanceRule::ItemFrame,
        "leash_knot" => DistanceRule::WithinBlocks(32),
        "block_display" | "item_display" | "text_display" => DistanceRule::Display,
        "lightning_bolt" => DistanceRule::Lightning,
        "firework_rocket" | "fishing_bobber" => DistanceRule::WithinBlocks(64),
        "shulker_bullet" => DistanceRule::WithinBlocks(128),
        "fireball" | "small_fireball" | "wind_charge" | "breeze_wind_charge" | "snowball" | "egg"
        | "ender_pearl" | "splash_potion" | "lingering_potion" | "experience_bottle" | "eye_of_ender" => {
            DistanceRule::ProjectileHiddenWhenYoung
        }
        "dragon_fireball" | "wither_skull" => DistanceRule::Projectile,
        "end_crystal" => DistanceRule::EndCrystal,
        _ => DistanceRule::BySize,
    }
}
