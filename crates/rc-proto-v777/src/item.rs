//! Network item stacks (`docs/specs/item-components.md`): quantity, item, component
//! patch. No length is prefixed: each component is read according to its encoding. The
//! bytes of each added component are kept as is; interpretation happens on
//! demand (name, damage, glint…).

use rc_proto::varint::read_varint;
use rc_proto::{Decode, Error, Reader, Result, read_str};

/// Stack received (non-empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStack {
    pub item: i32,
    pub count: i32,
    /// Added components: type (`minecraft:data_component_type`), bytes of the value.
    pub added: Vec<(u16, Box<[u8]>)>,
    /// Components removed relative to the item's default components.
    pub removed: Vec<u16>,
}

impl ItemStack {
    /// Bytes of an added component.
    pub fn component(&self, ty: u16) -> Option<&[u8]> {
        self.added.iter().find(|c| c.0 == ty).map(|c| &c.1[..])
    }
}

/// Shape of a component's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Unit,
    Nbt,
    VarInt,
    F32,
    Bool,
    I32,
    Text,
    Ident,
    Lore,
    BlockPredicates,
    AttributeModifiers,
    CustomModelData,
    TooltipDisplay,
    Food,
    Consumable,
    Template,
    UseCooldown,
    HolderSet,
    Tool,
    Weapon,
    AttackRange,
    Equippable,
    DeathProtection,
    BlocksAttacks,
    PiercingWeapon,
    KineticWeapon,
    Swing,
    UseEffects,
    TemplateList,
    PotionContents,
    VarIntPairs,
    WritableBook,
    WrittenBook,
    Trim,
    TypedNbt,
    Instrument,
    TrimMaterial,
    JukeboxPlayable,
    LodestoneTracker,
    FireworkExplosion,
    Fireworks,
    Profile,
    BannerPatterns,
    PotDecorations,
    Container,
    BlockState,
    Bees,
    Sound,
    ResolvableInt,
    Fuel,
    MobVisibility,
    Painting,
    SignText,
}

fn kind_of(name: &str) -> Option<Kind> {
    use Kind::*;
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    Some(match n {
        "unbreakable" | "creative_slot_lock" | "glider" | "waxed" => Unit,
        // Not synced: save form as network NBT.
        "custom_data"
        | "intangible_projectile"
        | "map_decorations"
        | "debug_stick_state"
        | "recipes"
        | "lock"
        | "container_loot"
        | "bucket_entity_data" => Nbt,
        "max_stack_size"
        | "max_damage"
        | "damage"
        | "repair_cost"
        | "enchantable"
        | "additional_trade_cost"
        | "block_transformer"
        | "villager_food"
        | "dye"
        | "map_id"
        | "map_post_processing"
        | "ominous_bottle_amplifier"
        | "rarity"
        | "damage_type"
        | "base_color"
        | "provides_pottery_pattern" => VarInt,
        "minimum_attack_charge" | "potion_duration_scale" => F32,
        "enchantment_glint_override" => Bool,
        "dyed_color" => I32,
        "custom_name" | "item_name" => Text,
        "item_model" | "tooltip_style" | "note_block_sound" => Ident,
        "lore" => Lore,
        "enchantments" | "stored_enchantments" | "suspicious_stew_effects" => VarIntPairs,
        "can_place_on" | "can_break" => BlockPredicates,
        "attribute_modifiers" => AttributeModifiers,
        "custom_model_data" => CustomModelData,
        "tooltip_display" => TooltipDisplay,
        "food" => Food,
        "consumable" => Consumable,
        "use_remainder" | "sulfur_cube_content" => Template,
        "use_cooldown" => UseCooldown,
        "damage_resistant" | "repairable" | "provides_banner_patterns" => HolderSet,
        "tool" => Tool,
        "weapon" => Weapon,
        "attack_range" => AttackRange,
        "equippable" => Equippable,
        "death_protection" => DeathProtection,
        "blocks_attacks" => BlocksAttacks,
        "piercing_weapon" => PiercingWeapon,
        "kinetic_weapon" => KineticWeapon,
        "attack_animation" | "interact_animation" => Swing,
        "use_effects" => UseEffects,
        "charged_projectiles" | "bundle_contents" => TemplateList,
        "potion_contents" => PotionContents,
        "writable_book_content" => WritableBook,
        "written_book_content" => WrittenBook,
        "trim" => Trim,
        "entity_data" | "block_entity_data" => TypedNbt,
        "instrument" => Instrument,
        "provides_trim_material" => TrimMaterial,
        "jukebox_playable" => JukeboxPlayable,
        "lodestone_tracker" => LodestoneTracker,
        "firework_explosion" => FireworkExplosion,
        "fireworks" => Fireworks,
        "profile" => Profile,
        "banner_patterns" => BannerPatterns,
        "pot_decorations" => PotDecorations,
        "container" => Container,
        "block_state" => BlockState,
        "bees" => Bees,
        "break_sound" => Sound,
        "compostable" => ResolvableInt,
        "cooking_fuel" | "brewing_fuel" => Fuel,
        "mob_visibility" => MobVisibility,
        "painting/variant" => Painting,
        "sign_text_front" | "sign_text_back" => SignText,
        // Animal variants and colours: a varint (registry or enumeration identifier).
        _ if n.contains('/') => VarInt,
        _ => return None,
    })
}

/// Consume effects, by identifier of the `minecraft:consume_effect_type` registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsumeEffect {
    ApplyEffects,
    RemoveEffects,
    ClearAllEffects,
    TeleportRandomly,
    PlaySound,
}

/// Maximum nesting (stacks in containers, hidden effects…).
const MAX_DEPTH: u32 = 16;

/// Stack decoder, built from the version's registries.
#[derive(Debug, Clone, Default)]
pub struct ItemCodec {
    kinds: Vec<Option<Kind>>,
    consume: Vec<Option<ConsumeEffect>>,
    names: Vec<String>,
}

fn bad(what: &'static str, value: i64) -> Error {
    Error::InvalidValue { what, value }
}

fn vi(r: &mut Reader<'_>) -> Result<i32> {
    read_varint(r)
}

fn boolean(r: &mut Reader<'_>) -> Result<bool> {
    bool::decode(r)
}

fn skip_n(r: &mut Reader<'_>, n: usize) -> Result<()> {
    r.take(n).map(|_| ())
}

fn ident(r: &mut Reader<'_>) -> Result<()> {
    read_str(r, 32767).map(|_| ())
}

fn string(r: &mut Reader<'_>, max: usize) -> Result<()> {
    read_str(r, max).map(|_| ())
}

fn nbt(r: &mut Reader<'_>) -> Result<()> {
    rc_proto::nbt::skip(r)
}

/// Mandatory compound NBT (entity data, bees…).
fn compound(r: &mut Reader<'_>) -> Result<()> {
    let ty = *r.rest().first().ok_or(Error::UnexpectedEof { needed: 1 })?;
    if ty != 10 {
        return Err(bad("compound NBT expected", i64::from(ty)));
    }
    nbt(r)
}

fn option(r: &mut Reader<'_>, f: impl FnOnce(&mut Reader<'_>) -> Result<()>) -> Result<()> {
    if boolean(r)? { f(r) } else { Ok(()) }
}

/// List; `min` = minimum size of an element (bound against hostile counters).
fn list(r: &mut Reader<'_>, min: usize, mut f: impl FnMut(&mut Reader<'_>) -> Result<()>) -> Result<()> {
    let n = rc_proto::read_count(r, min)?;
    for _ in 0..n {
        f(r)?;
    }
    Ok(())
}

fn holder_set(r: &mut Reader<'_>) -> Result<()> {
    match vi(r)? {
        0 => ident(r),
        n if n > 0 => {
            for _ in 1..n {
                vi(r)?;
            }
            Ok(())
        }
        n => Err(bad("reference set", i64::from(n))),
    }
}

/// `ref(registry)`: 0 = direct value read by `direct`, otherwise identifier + 1.
fn reference(r: &mut Reader<'_>, direct: impl FnOnce(&mut Reader<'_>) -> Result<()>) -> Result<()> {
    if vi(r)? == 0 { direct(r) } else { Ok(()) }
}

fn sound(r: &mut Reader<'_>) -> Result<()> {
    reference(r, |r| {
        ident(r)?;
        option(r, |r| skip_n(r, 4))
    })
}

fn effect_details(r: &mut Reader<'_>, depth: u32) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(bad("hidden effect nested too deeply", i64::from(depth)));
    }
    vi(r)?;
    vi(r)?;
    skip_n(r, 3)?;
    if boolean(r)? {
        effect_details(r, depth + 1)?;
    }
    Ok(())
}

fn effect_instance(r: &mut Reader<'_>) -> Result<()> {
    vi(r)?;
    effect_details(r, 0)
}

fn firework_explosion(r: &mut Reader<'_>) -> Result<()> {
    vi(r)?;
    list(r, 4, |r| skip_n(r, 4))?;
    list(r, 4, |r| skip_n(r, 4))?;
    skip_n(r, 2)
}

fn profile_properties(r: &mut Reader<'_>) -> Result<()> {
    let n = rc_proto::read_len(r, 16)?;
    for _ in 0..n {
        string(r, 64)?;
        string(r, 32767)?;
        option(r, |r| string(r, 1024))?;
    }
    Ok(())
}

fn template_stack_check(count: i32) -> Result<()> {
    if count <= 0 {
        return Err(bad("stack template quantity", i64::from(count)));
    }
    Ok(())
}

impl ItemCodec {
    /// `components`: names of the `minecraft:data_component_type` registry in identifier
    /// order; `consume_effects`: those of `minecraft:consume_effect_type`.
    pub fn new(components: &[String], consume_effects: &[String]) -> Self {
        let consume = consume_effects
            .iter()
            .map(|n| match n.strip_prefix("minecraft:").unwrap_or(n) {
                "apply_effects" => Some(ConsumeEffect::ApplyEffects),
                "remove_effects" => Some(ConsumeEffect::RemoveEffects),
                "clear_all_effects" => Some(ConsumeEffect::ClearAllEffects),
                "teleport_randomly" => Some(ConsumeEffect::TeleportRandomly),
                "play_sound" => Some(ConsumeEffect::PlaySound),
                _ => None,
            })
            .collect();
        ItemCodec { kinds: components.iter().map(|n| kind_of(n)).collect(), consume, names: components.to_vec() }
    }

    /// Network identifier of a component type (`minecraft:damage`…).
    pub fn component_id(&self, name: &str) -> Option<u16> {
        self.names.iter().position(|n| n == name).map(|i| i as u16)
    }

    /// Optional stack (`None` if empty).
    pub fn read_stack(&self, r: &mut Reader<'_>) -> Result<Option<ItemStack>> {
        self.stack(r, 0)
    }

    /// Resolvable profile (`profile` component, metadata serializer 41).
    pub fn skip_profile(&self, r: &mut Reader<'_>) -> Result<()> {
        if boolean(r)? {
            skip_n(r, 16)?;
            string(r, 16)?;
        } else {
            option(r, |r| string(r, 16))?;
            option(r, |r| skip_n(r, 16))?;
        }
        profile_properties(r)?;
        option(r, ident)?;
        option(r, ident)?;
        option(r, ident)?;
        option(r, |r| boolean(r).map(|_| ()))
    }

    /// Stack template (item, quantity, patch; never empty), e.g. `item` particle.
    pub fn read_template(&self, r: &mut Reader<'_>) -> Result<ItemStack> {
        let item = vi(r)?;
        let count = vi(r)?;
        template_stack_check(count)?;
        let (added, removed) = self.patch(r, 1)?;
        Ok(ItemStack { item, count, added, removed })
    }

    /// Cost of a merchant offer (`special-screens.md` § 1.1): item, quantity, required
    /// components (type + value), returned as a stack carrying those components.
    pub fn read_cost(&self, r: &mut Reader<'_>) -> Result<ItemStack> {
        let item = vi(r)?;
        let count = vi(r)?;
        let n = rc_proto::read_count(r, 1)?;
        let mut added = Vec::with_capacity(n);
        for _ in 0..n {
            let ty = self.component_type(r)?;
            let start = r.rest();
            let before = r.remaining();
            self.value(r, ty, 1)?;
            added.push((ty, start[..before - r.remaining()].into()));
        }
        Ok(ItemStack { item, count, added, removed: Vec::new() })
    }

    fn stack(&self, r: &mut Reader<'_>, depth: u32) -> Result<Option<ItemStack>> {
        let count = vi(r)?;
        if count <= 0 {
            return Ok(None);
        }
        let item = vi(r)?;
        let (added, removed) = self.patch(r, depth)?;
        Ok(Some(ItemStack { item, count, added, removed }))
    }

    /// Patch: added (type + value) then removed (type only).
    #[allow(clippy::type_complexity)]
    fn patch(&self, r: &mut Reader<'_>, depth: u32) -> Result<(Vec<(u16, Box<[u8]>)>, Vec<u16>)> {
        if depth > MAX_DEPTH {
            return Err(bad("stacks nested too deeply", i64::from(depth)));
        }
        let n_add = rc_proto::read_count(r, 1)?;
        let n_rem = rc_proto::read_count(r, 1)?;
        let mut added = Vec::with_capacity(n_add);
        for _ in 0..n_add {
            let ty = self.component_type(r)?;
            let start = r.rest();
            let before = r.remaining();
            self.value(r, ty, depth)?;
            let used = before - r.remaining();
            added.push((ty, start[..used].into()));
        }
        let mut removed = Vec::with_capacity(n_rem);
        for _ in 0..n_rem {
            removed.push(self.component_type(r)?);
        }
        Ok((added, removed))
    }

    fn component_type(&self, r: &mut Reader<'_>) -> Result<u16> {
        let ty = vi(r)?;
        match usize::try_from(ty).ok().and_then(|i| self.kinds.get(i)) {
            Some(Some(_)) => Ok(ty as u16),
            _ => Err(bad("item component type", i64::from(ty))),
        }
    }

    /// Patch without keeping the bytes (nested stacks).
    fn skip_patch(&self, r: &mut Reader<'_>, depth: u32) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(bad("stacks nested too deeply", i64::from(depth)));
        }
        let n_add = rc_proto::read_count(r, 1)?;
        let n_rem = rc_proto::read_count(r, 1)?;
        for _ in 0..n_add {
            let ty = self.component_type(r)?;
            self.value(r, ty, depth)?;
        }
        for _ in 0..n_rem {
            self.component_type(r)?;
        }
        Ok(())
    }

    /// Stack template: item, quantity, patch (never empty).
    fn template(&self, r: &mut Reader<'_>, depth: u32) -> Result<()> {
        vi(r)?;
        template_stack_check(vi(r)?)?;
        self.skip_patch(r, depth + 1)
    }

    fn consume_effect(&self, r: &mut Reader<'_>) -> Result<()> {
        let id = vi(r)?;
        match usize::try_from(id).ok().and_then(|i| self.consume.get(i).copied().flatten()) {
            Some(ConsumeEffect::ApplyEffects) => {
                list(r, 7, effect_instance)?;
                skip_n(r, 4)
            }
            Some(ConsumeEffect::RemoveEffects) => holder_set(r),
            Some(ConsumeEffect::ClearAllEffects) => Ok(()),
            Some(ConsumeEffect::TeleportRandomly) => skip_n(r, 5),
            Some(ConsumeEffect::PlaySound) => sound(r),
            None => Err(bad("consume effect", i64::from(id))),
        }
    }

    fn block_predicate(&self, r: &mut Reader<'_>, depth: u32) -> Result<()> {
        option(r, holder_set)?;
        option(r, |r| {
            list(r, 2, |r| {
                string(r, 32767)?;
                if boolean(r)? {
                    string(r, 32767)
                } else {
                    option(r, |r| string(r, 32767))?;
                    option(r, |r| string(r, 32767))
                }
            })
        })?;
        option(r, compound)?;
        // Exact components (value according to the type), then partial predicates (at most 64).
        list(r, 1, |r| {
            let ty = self.component_type(r)?;
            self.value(r, ty, depth + 1)
        })?;
        let n = rc_proto::read_len(r, 64)?;
        for _ in 0..n {
            boolean(r)?;
            vi(r)?;
            nbt(r)?;
        }
        Ok(())
    }

    fn value(&self, r: &mut Reader<'_>, ty: u16, depth: u32) -> Result<()> {
        use Kind::*;
        let kind = self.kinds[ty as usize].expect("type checked");
        match kind {
            Unit => Ok(()),
            Nbt => nbt(r),
            VarInt => vi(r).map(|_| ()),
            F32 | I32 => skip_n(r, 4),
            Bool => boolean(r).map(|_| ()),
            Text => nbt(r),
            Ident => ident(r),
            Lore => list(r, 1, nbt),
            VarIntPairs => list(r, 2, |r| vi(r).and_then(|_| vi(r)).map(|_| ())),
            BlockPredicates => list(r, 4, |r| self.block_predicate(r, depth)),
            AttributeModifiers => list(r, 1, |r| {
                vi(r)?;
                ident(r)?;
                skip_n(r, 8)?;
                vi(r)?;
                vi(r)?;
                match vi(r)? {
                    2 => nbt(r),
                    _ => Ok(()),
                }
            }),
            CustomModelData => {
                list(r, 4, |r| skip_n(r, 4))?;
                list(r, 1, |r| boolean(r).map(|_| ()))?;
                list(r, 1, |r| string(r, 32767))?;
                list(r, 4, |r| skip_n(r, 4))
            }
            TooltipDisplay => {
                boolean(r)?;
                list(r, 1, |r| vi(r).map(|_| ()))
            }
            Food => {
                vi(r)?;
                skip_n(r, 4)?;
                boolean(r).map(|_| ())
            }
            Consumable => {
                skip_n(r, 4)?;
                vi(r)?;
                sound(r)?;
                boolean(r)?;
                list(r, 1, |r| self.consume_effect(r))
            }
            Template => self.template(r, depth),
            UseCooldown => {
                skip_n(r, 4)?;
                option(r, ident)
            }
            HolderSet => holder_set(r),
            Tool => {
                list(r, 3, |r| {
                    holder_set(r)?;
                    option(r, |r| skip_n(r, 4))?;
                    option(r, |r| boolean(r).map(|_| ()))
                })?;
                skip_n(r, 4)?;
                vi(r)?;
                boolean(r).map(|_| ())
            }
            Weapon => {
                vi(r)?;
                skip_n(r, 4)
            }
            AttackRange => skip_n(r, 24),
            Equippable => {
                vi(r)?;
                sound(r)?;
                option(r, ident)?;
                option(r, ident)?;
                option(r, holder_set)?;
                skip_n(r, 5)?;
                sound(r)
            }
            DeathProtection => list(r, 1, |r| self.consume_effect(r)),
            BlocksAttacks => {
                skip_n(r, 8)?;
                list(r, 10, |r| {
                    skip_n(r, 4)?;
                    option(r, holder_set)?;
                    skip_n(r, 8)
                })?;
                skip_n(r, 12)?;
                option(r, holder_set)?;
                option(r, sound)?;
                option(r, sound)
            }
            PiercingWeapon => {
                skip_n(r, 2)?;
                option(r, sound)?;
                option(r, sound)
            }
            KineticWeapon => {
                vi(r)?;
                vi(r)?;
                for _ in 0..3 {
                    option(r, |r| {
                        vi(r)?;
                        skip_n(r, 8)
                    })?;
                }
                skip_n(r, 8)?;
                option(r, sound)?;
                option(r, sound)
            }
            Swing => vi(r).and_then(|_| vi(r)).map(|_| ()),
            UseEffects => skip_n(r, 6),
            TemplateList => list(r, 3, |r| self.template(r, depth)),
            PotionContents => {
                option(r, |r| vi(r).map(|_| ()))?;
                option(r, |r| skip_n(r, 4))?;
                list(r, 7, effect_instance)?;
                option(r, |r| string(r, 32767))
            }
            WritableBook => list(r, 2, |r| {
                string(r, 1024)?;
                option(r, |r| string(r, 1024))
            }),
            WrittenBook => {
                string(r, 32)?;
                option(r, |r| string(r, 32))?;
                string(r, 32767)?;
                vi(r)?;
                list(r, 2, |r| {
                    nbt(r)?;
                    option(r, nbt)
                })?;
                boolean(r).map(|_| ())
            }
            Trim => {
                reference(r, |r| ident(r).and_then(|_| nbt(r)))?;
                reference(r, |r| {
                    ident(r)?;
                    nbt(r)?;
                    boolean(r).map(|_| ())
                })
            }
            TypedNbt => {
                vi(r)?;
                compound(r)
            }
            Instrument => reference(r, |r| {
                sound(r)?;
                skip_n(r, 8)?;
                vi(r)?;
                nbt(r)
            }),
            TrimMaterial => reference(r, |r| ident(r).and_then(|_| nbt(r))),
            JukeboxPlayable => reference(r, |r| {
                sound(r)?;
                nbt(r)?;
                skip_n(r, 4)?;
                vi(r).map(|_| ())
            }),
            LodestoneTracker => {
                option(r, |r| {
                    ident(r)?;
                    skip_n(r, 8)
                })?;
                boolean(r).map(|_| ())
            }
            FireworkExplosion => firework_explosion(r),
            Fireworks => {
                vi(r)?;
                let n = rc_proto::read_len(r, 256)?;
                for _ in 0..n {
                    firework_explosion(r)?;
                }
                Ok(())
            }
            Profile => {
                if boolean(r)? {
                    skip_n(r, 16)?;
                    string(r, 16)?;
                } else {
                    option(r, |r| string(r, 16))?;
                    option(r, |r| skip_n(r, 16))?;
                }
                profile_properties(r)?;
                option(r, ident)?;
                option(r, ident)?;
                option(r, ident)?;
                option(r, |r| boolean(r).map(|_| ()))
            }
            BannerPatterns => list(r, 2, |r| {
                reference(r, |r| ident(r).and_then(|_| string(r, 32767)))?;
                vi(r).map(|_| ())
            }),
            PotDecorations => {
                for _ in 0..4 {
                    option(r, |r| self.template(r, depth))?;
                }
                Ok(())
            }
            Container => {
                let n = rc_proto::read_len(r, 256)?;
                for _ in 0..n {
                    option(r, |r| self.template(r, depth))?;
                }
                Ok(())
            }
            BlockState => list(r, 2, |r| string(r, 32767).and_then(|_| string(r, 32767))),
            Bees => list(r, 4, |r| {
                vi(r)?;
                compound(r)?;
                vi(r)?;
                vi(r).map(|_| ())
            }),
            Sound => sound(r),
            ResolvableInt => resolvable(r),
            Fuel => resolvable(r).and_then(|_| resolvable(r)),
            MobVisibility => {
                holder_set(r)?;
                skip_n(r, 4)
            }
            Painting => reference(r, |r| {
                vi(r)?;
                vi(r)?;
                ident(r)?;
                option(r, nbt)?;
                option(r, nbt)
            }),
            SignText => {
                for _ in 0..4 {
                    nbt(r)?;
                }
                option(r, |r| {
                    for _ in 0..4 {
                        nbt(r)?;
                    }
                    Ok(())
                })?;
                vi(r)?;
                boolean(r).map(|_| ())
            }
        }
    }
}

/// Integer or float that can be resolved: 4-byte constant, or provider identifier.
fn resolvable(r: &mut Reader<'_>) -> Result<()> {
    if boolean(r)? { skip_n(r, 4) } else { ident(r) }
}

/// Inventory packets (`hud.md` § 6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inventory {
    /// Full contents of a container: identifier, state, slots, cursor stack.
    Content {
        container: i32,
        state: i32,
        slots: Vec<Option<ItemStack>>,
        carried: Option<ItemStack>,
    },
    Slot {
        container: i32,
        state: i32,
        slot: i16,
        stack: Option<ItemStack>,
    },
    /// Slot of the player inventory (inventory indices).
    PlayerSlot {
        slot: i32,
        stack: Option<ItemStack>,
    },
    Cursor(Option<ItemStack>),
}

impl ItemCodec {
    pub fn read_content(&self, r: &mut Reader<'_>) -> Result<Inventory> {
        let container = vi(r)?;
        let state = vi(r)?;
        let n = rc_proto::read_count(r, 1)?;
        let mut slots = Vec::with_capacity(n);
        for _ in 0..n {
            slots.push(self.read_stack(r)?);
        }
        let carried = self.read_stack(r)?;
        Ok(Inventory::Content { container, state, slots, carried })
    }

    pub fn read_slot(&self, r: &mut Reader<'_>) -> Result<Inventory> {
        let container = vi(r)?;
        let state = vi(r)?;
        let slot = i16::decode(r)?;
        Ok(Inventory::Slot { container, state, slot, stack: self.read_stack(r)? })
    }

    pub fn read_player_slot(&self, r: &mut Reader<'_>) -> Result<Inventory> {
        let slot = vi(r)?;
        Ok(Inventory::PlayerSlot { slot, stack: self.read_stack(r)? })
    }

    pub fn read_cursor(&self, r: &mut Reader<'_>) -> Result<Inventory> {
        Ok(Inventory::Cursor(self.read_stack(r)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec() -> ItemCodec {
        let names: Vec<String> =
            ["custom_data", "max_stack_size", "max_damage", "damage", "unbreakable", "custom_name"]
                .iter()
                .map(|n| format!("minecraft:{n}"))
                .collect();
        ItemCodec::new(&names, &[])
    }

    #[test]
    fn empty_and_simple_stacks() {
        let c = codec();
        assert_eq!(c.read_stack(&mut Reader::new(&[0])).unwrap(), None);
        // 3 × item 7, damage = 5 added, unbreakable removed.
        let bytes = [3, 7, 1, 1, 3, 5, 4];
        let mut r = Reader::new(&bytes);
        let s = c.read_stack(&mut r).unwrap().unwrap();
        assert!(r.is_empty());
        assert_eq!((s.item, s.count, s.removed.clone()), (7, 3, vec![4]));
        assert_eq!(s.component(3), Some(&[5][..]));
    }

    #[test]
    fn unknown_component_type_is_an_error() {
        assert!(codec().read_stack(&mut Reader::new(&[1, 7, 1, 0, 99])).is_err());
    }
}
