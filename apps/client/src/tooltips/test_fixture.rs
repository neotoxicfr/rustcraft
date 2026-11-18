//! Fixture of the tooltip proofs and benches: the game's version data, two languages, and stacks
//! covering the tooltip sections (enchantments, potion, book, fireworks, banner, map, lore, dye,
//! damage, spawn egg, painting, stew, style, hidden components).

use rc_proto::nbt::Nbt;
use rc_proto::varint::write_varint;
use rc_proto_v777::item::ItemStack;

use crate::hud::HotbarItems;
use crate::inventory::PlayerInventory;
use crate::stack::{ComponentIds, ComponentLookup, HudEnv};

/// Map id of the map stack: far from the ids other tests give to the shared map table.
pub(crate) const MAP_ID: i32 = 918_273;

/// Block tag of the "can break" stack: a name no other test looks up in the shared tag table.
pub(crate) const BREAKABLE_TAG: &str = "minecraft:breakable_in_tooltip_proofs";

/// The tooltip context, tags, registries and map table are one per process: a proof that writes
/// them, or that compares two builds of a tooltip, holds this lock.
pub(crate) fn world_inputs_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Glyph widths without fonts: 6 px per character, `space` px for a space.
pub(crate) struct FixedWidths {
    pub space: f32,
}

impl rc_text::Metrics for FixedWidths {
    fn advance(&self, _font: &str, c: char) -> (f32, f32) {
        (if c == ' ' { self.space } else { 6.0 }, 1.0)
    }
}

pub(crate) struct TooltipWorld {
    pub data: rc_data::VersionData,
    pub ids: ComponentIds,
    pub item_names: Vec<String>,
    /// Enchantment registry as a server sends it (ids 0 to 3).
    pub enchantments: Vec<String>,
    pub english: rc_text::Lang,
    pub french: rc_text::Lang,
    inventory: PlayerInventory,
    env: HudEnv,
}

impl TooltipWorld {
    /// Fails when the version data are missing: nothing would be proven without them.
    pub fn new() -> TooltipWorld {
        let data = rc_testdata::version_data().expect("version data missing (extraction)");
        let ids = ComponentIds::new(&data);
        TooltipWorld::with_ids(data, ids)
    }

    /// The same world reading its component ids through `lookup`.
    pub fn with_lookup(lookup: ComponentLookup) -> TooltipWorld {
        let data = rc_testdata::version_data().expect("version data missing (extraction)");
        let ids = ComponentIds::with_lookup(&data, lookup);
        TooltipWorld::with_ids(data, ids)
    }

    fn with_ids(data: rc_data::VersionData, ids: ComponentIds) -> TooltipWorld {
        const ENGLISH: &[u8] = br#"{
            "item.minecraft.diamond_sword": "Diamond Sword",
            "item.minecraft.filled_map": "Map",
            "filled_map.id": "Id #%s",
            "filled_map.unknown": "Unknown Map",
            "item.durability": "Durability: %s / %s",
            "enchantment.minecraft.sharpness": "Sharpness",
            "enchantment.level.5": "V",
            "item.spawn_egg.peaceful": "Disabled in Peaceful",
            "painting.random": "Random variant",
            "potion.withDuration": "%s (%s)"
        }"#;
        let french = r#"{"item.minecraft.diamond_sword": "Épée en diamant", "item.minecraft.filled_map": "Carte"}"#;
        TooltipWorld {
            item_names: data.registry("minecraft:item").expect("item registry").entries.clone(),
            enchantments: ["sharpness", "unbreaking", "mending", "vanishing_curse"]
                .map(|name| format!("minecraft:{name}"))
                .to_vec(),
            english: rc_text::Lang::load(ENGLISH, None, None),
            french: rc_text::Lang::load(ENGLISH, Some(french.as_bytes()), None),
            inventory: PlayerInventory::default(),
            env: HudEnv::empty(),
            ids,
            data,
        }
    }

    pub fn items(&self) -> HotbarItems<'_> {
        HotbarItems {
            inv: &self.inventory,
            ids: &self.ids,
            data: &self.data,
            infos: &self.data.items,
            names: &self.item_names,
            env: &self.env,
            pt: 0.0,
        }
    }

    fn registry_id(&self, registry: &str, name: &str) -> i32 {
        self.data.registry_id(registry, name).unwrap_or_else(|| panic!("{name} missing from {registry}")) as i32
    }

    /// One `item` carrying `components` (name without namespace, network bytes).
    pub fn stack(&self, item: &str, components: Vec<(&str, Vec<u8>)>) -> ItemStack {
        let added = components
            .into_iter()
            .map(|(name, bytes)| {
                let id = self.ids.of(name).unwrap_or_else(|| panic!("component {name} missing from the registry"));
                (id, bytes.into_boxed_slice())
            })
            .collect();
        ItemStack { item: self.registry_id("minecraft:item", item), count: 1, added, removed: Vec::new() }
    }

    /// The stacks of the proofs, with a short label each.
    pub fn stacks(&self) -> Vec<(&'static str, ItemStack)> {
        let potion = self.registry_id("minecraft:potion", "minecraft:swiftness");
        let zombie = self.registry_id("minecraft:entity_type", "minecraft:zombie");
        vec![
            ("plain block", self.stack("minecraft:stone", vec![])),
            ("sword with default attributes", self.stack("minecraft:diamond_sword", vec![])),
            (
                "enchanted sword with name, lore and style",
                self.stack(
                    "minecraft:diamond_sword",
                    vec![
                        ("enchantments", vec![2, 3, 1, 0, 5]),
                        ("custom_name", text_tag("Sting")),
                        ("lore", [vec![2], text_tag("Forged long ago"), text_tag("Glows near foes")].concat()),
                        ("tooltip_style", string("minecraft:fancy")),
                        ("unbreakable", vec![]),
                    ],
                ),
            ),
            (
                "potion with a timed effect",
                self.stack("minecraft:potion", vec![("potion_contents", potion_of(potion))]),
            ),
            (
                "written book",
                self.stack("minecraft:written_book", vec![("written_book_content", book("Notes", "Zoé", 1))]),
            ),
            ("firework rocket", self.stack("minecraft:firework_rocket", vec![("fireworks", two_equal_explosions())])),
            ("banner with a pattern", self.stack("minecraft:white_banner", vec![("banner_patterns", vec![1, 1, 14])])),
            ("filled map", self.stack("minecraft:filled_map", vec![("map_id", varint(MAP_ID))])),
            (
                "dyed and damaged armor",
                self.stack(
                    "minecraft:leather_chestplate",
                    vec![("dyed_color", vec![0, 0x12, 0x34, 0x56]), ("damage", varint(17))],
                ),
            ),
            ("spawn egg of a hostile mob", self.stack("minecraft:zombie_spawn_egg", vec![])),
            (
                "egg carrying operator-only entity data",
                self.stack("minecraft:cow_spawn_egg", vec![("entity_data", typed_data(self.falling_block()))]),
            ),
            (
                "egg retyped to a hostile mob",
                self.stack("minecraft:cow_spawn_egg", vec![("entity_data", typed_data(zombie))]),
            ),
            ("painting without a variant", self.stack("minecraft:painting", vec![])),
            // Registry references are sent as id + 1: the first entry of each registry.
            ("painting of a variant", self.stack("minecraft:painting", vec![("painting/variant", vec![1])])),
            ("goat horn", self.stack("minecraft:goat_horn", vec![("instrument", vec![1])])),
            (
                "pickaxe that can break a block tag",
                self.stack("minecraft:diamond_pickaxe", vec![("can_break", block_tag_predicate(BREAKABLE_TAG))]),
            ),
            (
                "suspicious stew",
                self.stack("minecraft:suspicious_stew", vec![("suspicious_stew_effects", vec![1, 5, 0xa0, 0x01])]),
            ),
            (
                "sword hiding its attributes",
                self.stack("minecraft:diamond_sword", vec![("tooltip_display", self.hidden(&["attribute_modifiers"]))]),
            ),
            (
                "stack hiding its whole tooltip",
                self.stack("minecraft:diamond_sword", vec![("tooltip_display", vec![1, 0])]),
            ),
        ]
    }

    fn falling_block(&self) -> i32 {
        self.registry_id("minecraft:entity_type", "minecraft:falling_block")
    }

    /// `tooltip_display` hiding the named component types (tooltip itself shown).
    fn hidden(&self, components: &[&str]) -> Vec<u8> {
        let mut bytes = vec![0];
        write_varint(&mut bytes, components.len() as i32);
        for name in components {
            write_varint(&mut bytes, i32::from(self.ids.of(name).expect("component type")));
        }
        bytes
    }
}

pub(crate) fn varint(value: i32) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_varint(&mut bytes, value);
    bytes
}

fn string(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    rc_proto::write_str(&mut bytes, text);
    bytes
}

/// A plain text as a network text tag.
fn text_tag(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    rc_proto::nbt::write(&mut bytes, Some(&Nbt::String(text.into())));
    bytes
}

/// Potion contents: base potion, no custom colour, no custom effect, no custom name.
fn potion_of(potion: i32) -> Vec<u8> {
    [vec![1], varint(potion), vec![0, 0, 0]].concat()
}

/// Written book: title, no filtered title, author, generation, no page, not resolved.
fn book(title: &str, author: &str, generation: i32) -> Vec<u8> {
    [string(title), vec![0], string(author), varint(generation), vec![0, 0]].concat()
}

/// Rocket of flight duration 1 with two identical star explosions (one colour, no fade).
fn two_equal_explosions() -> Vec<u8> {
    let explosion = [vec![2, 1], vec![0x00, 0xb3, 0x31, 0x2c], vec![0, 0, 0]].concat();
    [vec![1, 2], explosion.clone(), explosion].concat()
}

/// One block predicate naming a block tag, without property, data or component conditions.
fn block_tag_predicate(tag: &str) -> Vec<u8> {
    [vec![1, 1, 0], string(tag), vec![0, 0, 0, 0]].concat()
}

/// Typed entity or block entity data: type id, then an empty compound.
fn typed_data(type_id: i32) -> Vec<u8> {
    [varint(type_id), vec![10, 0]].concat()
}
