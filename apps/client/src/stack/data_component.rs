//! Component types the client reads by name on repeated paths (tooltip lines, entity rendering):
//! one list, from which [`ComponentIds`](super::ComponentIds) resolves every network id once.

/// Declares [`DataComponent`] and the registry path of each variant from a single list, so that a
/// variant cannot be added without its path.
macro_rules! data_components {
    ($($variant:ident => $path:literal),* $(,)?) => {
        /// A component type whose network id is kept by [`ComponentIds`](super::ComponentIds).
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum DataComponent {
            $($variant),*
        }

        impl DataComponent {
            /// Every variant, in declaration order (the order of the table of resolved ids).
            pub(crate) const ALL: &'static [DataComponent] = &[$(DataComponent::$variant),*];

            /// Registry path of the component type, without the `minecraft:` namespace.
            pub(crate) fn path(self) -> &'static str {
                match self {
                    $(DataComponent::$variant => $path),*
                }
            }
        }
    };
}

data_components! {
    AttributeModifiers => "attribute_modifiers",
    BannerPatterns => "banner_patterns",
    Bees => "bees",
    BlockEntityData => "block_entity_data",
    BlockState => "block_state",
    CanBreak => "can_break",
    CanPlaceOn => "can_place_on",
    ChargedProjectiles => "charged_projectiles",
    Container => "container",
    ContainerLoot => "container_loot",
    CustomName => "custom_name",
    Damage => "damage",
    DyedColor => "dyed_color",
    Enchantments => "enchantments",
    EntityData => "entity_data",
    FireworkExplosion => "firework_explosion",
    Fireworks => "fireworks",
    Instrument => "instrument",
    IntangibleProjectile => "intangible_projectile",
    JukeboxPlayable => "jukebox_playable",
    Lore => "lore",
    MapPostProcessing => "map_post_processing",
    OminousBottleAmplifier => "ominous_bottle_amplifier",
    PaintingVariant => "painting/variant",
    PotDecorations => "pot_decorations",
    PotionContents => "potion_contents",
    PotionDurationScale => "potion_duration_scale",
    Profile => "profile",
    SignTextBack => "sign_text_back",
    SignTextFront => "sign_text_front",
    StoredEnchantments => "stored_enchantments",
    SulfurCubeContent => "sulfur_cube_content",
    SuspiciousStewEffects => "suspicious_stew_effects",
    Tool => "tool",
    TooltipDisplay => "tooltip_display",
    TooltipStyle => "tooltip_style",
    Trim => "trim",
    TropicalFishBaseColor => "tropical_fish/base_color",
    TropicalFishPattern => "tropical_fish/pattern",
    TropicalFishPatternColor => "tropical_fish/pattern_color",
    Unbreakable => "unbreakable",
    WrittenBookContent => "written_book_content",
}
