//! The per-species components an item stack hands to the mob it places or makes born (a spawn
//! egg used on a block, in a liquid, from a dispenser, or on a mob of its species): variant,
//! sound variant, collar, colour, size, fish pattern, villager type, absorbed block. The game
//! reads them first, then the custom name and data, then the stack's entity data.

use rc_proto::nbt::Nbt;

use super::combat::DYES;
use super::mob::Mob;
use super::sealife::fish::{self, PATTERN_NAMES, PATTERNS};
use super::species::Kind;
use super::surface::rabbit;
use super::variants::VILLAGER_TYPES;
use crate::player::Stack;
use crate::server::Server;

/// Serialized names of the variants stored by index, in id order.
const HORSE_COLORS: [&str; 7] = ["white", "creamy", "chestnut", "brown", "black", "gray", "dark_brown"];
const LLAMA_VARIANTS: [&str; 4] = ["creamy", "white", "brown", "gray"];
const PARROT_VARIANTS: [&str; 5] = ["red_blue", "blue", "green", "yellow_blue", "gray"];
const AXOLOTL_VARIANTS: [&str; 5] = ["lucy", "wild", "gold", "cyan", "blue"];
const SALMON_SIZES: [&str; 3] = ["small", "medium", "large"];
/// Mooshroom and fox: the second name is variant 1.
const MOOSHROOM_VARIANTS: [&str; 2] = ["red", "brown"];
const FOX_VARIANTS: [&str; 2] = ["red", "snow"];
/// Rabbit variants: name and id (the killer bunny is 99).
const RABBIT_VARIANTS: [(&str, i32); 7] = [
    ("brown", 0),
    ("white", 1),
    ("black", 2),
    ("white_splotched", 3),
    ("gold", 4),
    ("salt", 5),
    ("evil", rabbit::EVIL),
];

/// "Body" equipment slot: the block a sulfur cube absorbed.
const BODY: usize = 6;

fn index_in(names: &[&str], name: &str) -> Option<i32> {
    names.iter().position(|n| *n == name).map(|i| i as i32)
}

fn dye_index(name: &str) -> Option<i32> {
    index_in(&DYES, name)
}

/// Pattern, base colour and pattern colour of a packed tropical fish variant.
fn tropical_fish_parts(variant: i32) -> [i32; 3] {
    let v = fish::normalize(variant);
    [v & 0xFFFF, (v >> 16) & 0xFF, (v >> 24) & 0xFF]
}

/// A registry identifier written with or without its namespace.
fn full_id(name: &str) -> String {
    if name.contains(':') { name.to_owned() } else { format!("minecraft:{name}") }
}

impl Server {
    /// Reads every per-species component of `stack` that `m`'s species knows (others are left
    /// to their own species); the synced data follow.
    pub(crate) fn apply_species_components(&self, m: &mut Mob, stack: &Stack) {
        let Some(Nbt::Compound(components)) = &stack.components else { return };
        let mut changed = false;
        for (key, value) in components {
            if let Some(component) = key.strip_prefix("minecraft:") {
                changed |= self.apply_species_component(m, component, value).is_some();
            }
        }
        if changed {
            m.sync_data();
        }
    }

    /// One component; `None` when the species does not read it or the value is unknown.
    fn apply_species_component(&self, m: &mut Mob, component: &str, value: &Nbt) -> Option<()> {
        let name = value.as_str();
        match (m.kind, component) {
            (Kind::Pig, "pig/variant")
            | (Kind::Cow, "cow/variant")
            | (Kind::Chicken, "chicken/variant")
            | (Kind::Wolf, "wolf/variant")
            | (Kind::Cat, "cat/variant") => {
                m.variant = self.registered_variant_index(m.kind, false, &full_id(name?))?
            }
            (Kind::Pig, "pig/sound_variant")
            | (Kind::Cow, "cow/sound_variant")
            | (Kind::Chicken, "chicken/sound_variant")
            | (Kind::Wolf, "wolf/sound_variant")
            | (Kind::Cat, "cat/sound_variant") => {
                m.sound_variant = self.registered_variant_index(m.kind, true, &full_id(name?))?;
            }
            (Kind::Wolf, "wolf/collar") | (Kind::Cat, "cat/collar") => m.collar = dye_index(name?)?,
            (Kind::Sheep, "sheep/color") => m.wool = dye_index(name?)? as u8,
            (Kind::Shulker, "shulker/color") => m.end.color = dye_index(name?)? as i8,
            (Kind::Frog, "frog/variant") => {
                let id = full_id(name?);
                m.variant = self.registry_index("minecraft:frog_variant", &id)?;
                m.bm.frog_variant = Some(id);
            }
            (Kind::ZombieNautilus, "zombie_nautilus/variant") => {
                let id = full_id(name?);
                m.variant = self.registry_index("minecraft:zombie_nautilus_variant", &id)?;
                m.sea.variant_name = Some(id);
            }
            (Kind::Mooshroom, "mooshroom/variant") => m.variant = index_in(&MOOSHROOM_VARIANTS, name?)?,
            (Kind::Fox, "fox/variant") => m.variant = index_in(&FOX_VARIANTS, name?)?,
            (Kind::Parrot, "parrot/variant") => m.variant = index_in(&PARROT_VARIANTS, name?)?,
            (Kind::Axolotl, "axolotl/variant") => m.variant = index_in(&AXOLOTL_VARIANTS, name?)?,
            (Kind::Llama | Kind::TraderLlama, "llama/variant") => m.variant = index_in(&LLAMA_VARIANTS, name?)?,
            (Kind::Salmon, "salmon/size") => m.variant = index_in(&SALMON_SIZES, name?)?,
            (Kind::Rabbit, "rabbit/variant") => {
                let id = RABBIT_VARIANTS.iter().find(|(n, _)| Some(*n) == name)?.1;
                rabbit::set_variant(m, id);
            }
            // The horse's colour alone (low byte): its markings and the bits above stay.
            (Kind::Horse, "horse/variant") => m.variant = (m.variant & !0xFF) | index_in(&HORSE_COLORS, name?)?,
            // One part of the fish's look; the other two are read back from the packed variant
            // (an unknown pattern as kob, a colour out of range as white) and packed again.
            (Kind::TropicalFish, "tropical_fish/pattern") => {
                let pattern = PATTERNS[index_in(&PATTERN_NAMES, name?)? as usize];
                let [_, base, color] = tropical_fish_parts(m.variant);
                m.variant = fish::pack(pattern, base, color);
            }
            (Kind::TropicalFish, "tropical_fish/base_color") => {
                let [pattern, _, color] = tropical_fish_parts(m.variant);
                m.variant = fish::pack(pattern, dye_index(name?)?, color);
            }
            (Kind::TropicalFish, "tropical_fish/pattern_color") => {
                let [pattern, base, _] = tropical_fish_parts(m.variant);
                m.variant = fish::pack(pattern, base, dye_index(name?)?);
            }
            // The type changes, profession and level stay; the data are then fixed.
            (Kind::Villager | Kind::ZombieVillager, "villager/variant") => {
                let id = full_id(name?);
                m.villager[0] = index_in(&VILLAGER_TYPES, id.strip_prefix("minecraft:")?)?;
                m.x.vdata_final = true;
            }
            // The absorbed block, dropped for sure when the cube dies.
            (Kind::SulfurCube, "sulfur_cube_content") => {
                m.equipment[BODY] = Some(Stack::from_nbt(value)?);
                m.drop_chance[BODY] = 2.0;
            }
            _ => return None,
        }
        Some(())
    }

    fn registry_index(&self, registry: &str, id: &str) -> Option<i32> {
        self.regs.entries(registry).iter().position(|entry| entry == id).map(|i| i as i32)
    }
}
