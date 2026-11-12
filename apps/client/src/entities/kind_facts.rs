//! Facts about an entity's type that the per-frame and per-tick walks ask for (crosshair targets,
//! simulated projectiles, neighbours that push the player, building checks), worked out once from
//! the type name when the entity spawns. The walks then read a few bytes stored in the entity
//! instead of comparing its type name, which lives on the heap: one pointer chase and several
//! string comparisons less per entity and per walk.
//!
//! Every fact is defined by the same string rule as the code it replaces (`old_rules` in the
//! tests checks them over the whole type registry).

use super::{Kind, SizeRules, classify};

#[cfg(test)]
#[path = "kind_facts_tests.rs"]
mod tests;

/// `RUSTCRAFT_OPT_KIND_FLAGS=0`: crosshair targeting tests the type name of every entity in reach
/// on every frame (old path) instead of the facts stored at spawn.
pub(crate) fn kind_flags_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_KIND_FLAGS")
}

/// `RUSTCRAFT_OPT_KIND_ID_FILTERS=0`: the per-tick walks (simulated projectiles, pushing
/// neighbours, players near block entities) and the building checks compare type names (old
/// path) instead of reading the facts stored at spawn.
pub(crate) fn kind_id_filters_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_KIND_ID_FILTERS")
}

/// Whether the crosshair may aim at an entity of this type (`targeting.md` § 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PickRule {
    /// Never aimed at (non-living types outside the game's list, arrows and tridents).
    #[default]
    Never,
    /// Always aimed at (living types, hanging entities, vehicles, a few objects).
    Always,
    /// A player: unless the player list shows them in spectator mode.
    UnlessSpectator,
    /// An armor stand: unless it is a marker (flag 0x10 of data 15).
    UnlessMarker,
}

/// Movement rule of a projectile simulated on the client between two server resyncs
/// (`projectile_sim.rs` turns it into its parameters).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MotionFamily {
    /// Not a simulated projectile.
    #[default]
    None,
    /// Arrow or spectral arrow.
    Arrow,
    Trident,
    /// Snowball, egg, ender pearl (light gravity).
    ThrownLight,
    /// Splash and lingering potions.
    ThrownPotion,
    ExperienceBottle,
    /// Fireballs and wither skulls (accelerated, slowed in liquids).
    Fireball,
    /// Wind charges (accelerated, no drag).
    WindCharge,
    LlamaSpit,
    ShulkerBullet,
    EyeOfEnder,
    FireworkRocket,
    FishingBobber,
}

/// Facts of one entity type, stored in each entity at spawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct KindFacts {
    pub pick: PickRule,
    /// Aimed-at box inflated by 1 (fireballs, wind charges).
    pub pick_radius_one: bool,
    /// The `player` type (remote players).
    pub player: bool,
    /// Player or mannequin (same standing width).
    pub avatar: bool,
    /// Type name ending in `boat` or `raft`: pushes the player as a boat.
    pub boat_or_raft: bool,
    /// Boat, raft or minecart.
    pub vehicle_like: bool,
    /// Never pushes the player although living (bat, armor stand, parrot, vex).
    pub never_pushes: bool,
    /// Keeps a block from being placed where its box stands (`App::entity_blocks`).
    pub blocks_building: bool,
    pub trident: bool,
    pub fishing_bobber: bool,
    /// Item frame or glow item frame (its box depends on a held map).
    pub item_frame: bool,
    /// Its box may be kept from one frame to the next while its position, data and scale stay
    /// the same. Not for paintings (size from the variant registry), shulkers (opening moves
    /// every tick) and polar bears (standing animation).
    pub box_cacheable: bool,
    pub motion: MotionFamily,
    /// Size, age and render-distance rules (`size_rules.rs`).
    pub size: SizeRules,
}

impl KindFacts {
    /// Facts of the type named `name` (without namespace).
    pub fn of(name: &str) -> KindFacts {
        let living = classify(name).1 != Kind::Other;
        let vehicle_like = name.ends_with("boat") || name.ends_with("raft") || name.ends_with("minecart");
        KindFacts {
            pick: pick_rule(name, living, vehicle_like),
            pick_radius_one: matches!(name, "fireball" | "wind_charge" | "breeze_wind_charge"),
            player: name == "player",
            avatar: matches!(name, "player" | "mannequin"),
            boat_or_raft: name.ends_with("boat") || name.ends_with("raft"),
            vehicle_like,
            never_pushes: matches!(name, "bat" | "armor_stand" | "parrot" | "vex"),
            blocks_building: living || vehicle_like || matches!(name, "falling_block" | "tnt" | "end_crystal"),
            trident: name == "trident",
            fishing_bobber: name == "fishing_bobber",
            item_frame: matches!(name, "item_frame" | "glow_item_frame"),
            box_cacheable: !matches!(name, "painting" | "shulker" | "polar_bear"),
            motion: motion_family(name),
            size: SizeRules::of(name),
        }
    }
}

fn pick_rule(name: &str, living: bool, vehicle_like: bool) -> PickRule {
    match name {
        "player" => PickRule::UnlessSpectator,
        "armor_stand" => PickRule::UnlessMarker,
        // Outside the `redirectable_projectile` tag: never aimed at, in flight or stuck.
        "arrow" | "spectral_arrow" | "trident" => PickRule::Never,
        // Hanging entities and the few objects the game lets the crosshair hit.
        "painting" | "item_frame" | "glow_item_frame" | "leash_knot" | "end_crystal" | "interaction"
        | "falling_block" | "tnt" | "shulker_bullet" | "fireball" | "wind_charge" | "breeze_wind_charge" => {
            PickRule::Always
        }
        _ if vehicle_like || living => PickRule::Always,
        _ => PickRule::Never,
    }
}

fn motion_family(name: &str) -> MotionFamily {
    match name {
        "arrow" | "spectral_arrow" => MotionFamily::Arrow,
        "trident" => MotionFamily::Trident,
        "snowball" | "egg" | "ender_pearl" => MotionFamily::ThrownLight,
        "splash_potion" | "lingering_potion" => MotionFamily::ThrownPotion,
        "experience_bottle" => MotionFamily::ExperienceBottle,
        "fireball" | "small_fireball" | "dragon_fireball" | "wither_skull" => MotionFamily::Fireball,
        "wind_charge" | "breeze_wind_charge" => MotionFamily::WindCharge,
        "llama_spit" => MotionFamily::LlamaSpit,
        "shulker_bullet" => MotionFamily::ShulkerBullet,
        "eye_of_ender" => MotionFamily::EyeOfEnder,
        "firework_rocket" => MotionFamily::FireworkRocket,
        "fishing_bobber" => MotionFamily::FishingBobber,
        _ => MotionFamily::None,
    }
}
