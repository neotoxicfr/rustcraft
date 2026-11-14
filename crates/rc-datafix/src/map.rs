//! Map of nested data: for each kind, where its value holds data of other kinds, per version.
//!
//! The game describes its saved formats with one schema per version; a conversion rule reads
//! the data with the schema in force just before its own version, so a rule on item stacks
//! reaches the stacks in a chest inside a minecart inside a chunk. This map is that description
//! for the versions the chain covers ([`crate::OLDEST_CONVERTIBLE`] onwards); each older version
//! a rule needs adds its differences here. Fields not listed hold nothing the conversion visits
//! and are kept as they are.

use crate::kind::Kind;
use crate::SavedData;

/// How a value is laid out, as far as the conversion is concerned.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Node {
    /// Data of another kind.
    Of(Kind),
    /// A list whose every element is the inner node.
    List(&'static Node),
    /// A compound: each listed field, when present, is the given node. A field whose value does
    /// not read as its node is left out of the conversion (kept as it is), the rest still read.
    Fields(&'static [(&'static str, Node)]),
    /// Fields that must all be present and read: otherwise the value does not read at all.
    Required(&'static [(&'static str, Node)]),
    /// A compound whose every value is the inner node (keys are free text).
    Values(&'static Node),
    /// A compound whose keys are identifiers of a kind, with plain values (statistics).
    KeysOf(Kind),
    /// A compound whose keys are identifiers of no particular kind, values the inner node.
    NamespacedKeys(&'static Node),
    /// An identifier of no particular kind.
    Namespaced,
    /// The first alternative whose form fits the value (a compound, a list or a string).
    Either(&'static [Node]),
    /// Several descriptions of the same compound.
    All(&'static [Node]),
    /// A compound whose `key` field picks which fields it has among `cases`; `plain` lists the
    /// keys known to hold nothing nested. With `namespaced`, the key is an identifier (completed
    /// with `minecraft:` when it has no namespace, and listed here without it); otherwise it is
    /// compared as it is written.
    Choice {
        key: &'static str,
        namespaced: bool,
        cases: &'static [(&'static str, Node)],
        plain: &'static [&'static str],
    },
    /// A scoreboard objective's criterion, packed as `type:id` text in `CriteriaName` (each part
    /// a dotted identifier); its id is a block, item or entity identifier according to its type.
    PackedCriterion,
    /// Plain text.
    Text,
    /// Holds nothing the conversion visits.
    Plain,
}

/// The schema in force for some data: a version key (`version * 10 + step`), like the game's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Schema(pub(crate) i32);

impl Schema {
    /// The schema data saved at data version `version` are read with.
    pub(crate) fn of_version(version: i32) -> Schema {
        Schema(version.saturating_mul(10))
    }

    /// The schema a rule of version key `rule_key` reads its input with (the one before it).
    pub(crate) fn before(rule_key: i32) -> Schema {
        Schema(rule_key - 1)
    }

    fn at_least(self, version: i32, step: i32) -> bool {
        self.0 >= version * 10 + step
    }
}

/// Data of kind `target` can lie somewhere inside data of kind `holder` (itself excluded), in
/// `schema`: what lets a rule skip the files where its kind never occurs.
pub(crate) fn can_hold(holder: Kind, target: Kind, schema: Schema) -> bool {
    let mut seen: Vec<Kind> = Vec::new();
    let mut todo: Vec<Kind> = Vec::new();
    kinds_in(shape(holder, schema), &mut todo);
    while let Some(kind) = todo.pop() {
        if kind == target {
            return true;
        }
        if !seen.contains(&kind) {
            seen.push(kind);
            kinds_in(shape(kind, schema), &mut todo);
        }
    }
    false
}

/// Every kind `node` names directly (not through those kinds' own shapes).
fn kinds_in(node: Node, out: &mut Vec<Kind>) {
    match node {
        Node::Of(kind) | Node::KeysOf(kind) => out.push(kind),
        Node::List(inner) | Node::Values(inner) | Node::NamespacedKeys(inner) => kinds_in(*inner, out),
        Node::Fields(fields) | Node::Required(fields) => fields.iter().for_each(|(_, n)| kinds_in(*n, out)),
        Node::Either(nodes) | Node::All(nodes) => nodes.iter().for_each(|n| kinds_in(*n, out)),
        Node::Choice { cases, .. } => cases.iter().for_each(|(_, n)| kinds_in(*n, out)),
        Node::PackedCriterion => out.extend([Kind::BlockName, Kind::ItemName, Kind::EntityName]),
        Node::Namespaced | Node::Text | Node::Plain => {}
    }
}

const ITEM: Node = Node::Of(Kind::ItemStack);
const ITEMS: Node = Node::List(&ITEM);
const TEXT: Node = Node::Of(Kind::TextComponent);
const TEXTS: Node = Node::List(&TEXT);
const STATE: Node = Node::Of(Kind::BlockState);
const ENTITY_TREE: Node = Node::Of(Kind::EntityTree);
const ENTITY_TREES: Node = Node::List(&ENTITY_TREE);
const BLOCK_ENTITY: Node = Node::Of(Kind::BlockEntity);
const ITEM_NAME: Node = Node::Of(Kind::ItemName);
const BLOCK_NAME: Node = Node::Of(Kind::BlockName);
const ENTITY_NAME: Node = Node::Of(Kind::EntityName);
const BIOME: Node = Node::Of(Kind::Biome);
const RECIPE: Node = Node::Of(Kind::Recipe);

/// Where `kind` holds other data, in `schema`.
pub(crate) fn shape(kind: Kind, schema: Schema) -> Node {
    match kind {
        Kind::Level => Node::Of(Kind::LevelSummary),
        Kind::Player => PLAYER,
        Kind::Chunk => CHUNK,
        Kind::EntityChunk => Node::Fields(&[("Entities", ENTITY_TREES)]),
        Kind::Hotbar => Node::Values(&ITEMS),
        Kind::Structure => STRUCTURE,
        Kind::Stats => STATS,
        Kind::Advancements => ADVANCEMENTS,
        Kind::SavedData(data) => saved_data(data),
        Kind::EntityTree => Node::All(&[Node::Fields(&[("Passengers", ENTITY_TREES)]), Node::Of(Kind::Entity)]),
        Kind::Entity => entity(schema),
        Kind::EntityEquipment => EQUIPMENT,
        Kind::BlockEntity => block_entity(schema),
        // Stack written as its bare identifier: read as one item (`count` 1) by the walk.
        Kind::ItemStack => Node::Fields(&[("id", ITEM_NAME), ("components", Node::Of(Kind::DataComponents))]),
        Kind::DataComponents => data_components(schema),
        Kind::VillagerTrade => Node::Fields(&[("buy", ITEM), ("buyB", ITEM), ("sell", ITEM)]),
        Kind::Particle => Node::Fields(&[("item", ITEM), ("block_state", STATE)]),
        Kind::TextComponent => TEXT_COMPONENT,
        Kind::UntaggedSpawner => SPAWNER,
        Kind::Objective => OBJECTIVE,
        Kind::Team => Node::Fields(&[("MemberNamePrefix", TEXT), ("MemberNameSuffix", TEXT), ("DisplayName", TEXT)]),
        Kind::WorldGenSettings => WORLD_GEN_SETTINGS,
        Kind::LevelSummary
        | Kind::PoiChunk
        | Kind::Options
        | Kind::DebugProfile
        | Kind::BlockState
        | Kind::FlatBlockState
        | Kind::StructureFeature => Node::Plain,
        Kind::BlockName
        | Kind::ItemName
        | Kind::EntityName
        | Kind::GameEventName
        | Kind::Biome
        | Kind::Recipe
        | Kind::MultiNoiseParameterList => Node::Namespaced,
    }
}

const PLAYER: Node = Node::All(&[
    Node::Of(Kind::EntityEquipment),
    Node::Fields(&[
        ("RootVehicle", Node::Fields(&[("Entity", ENTITY_TREE)])),
        ("ender_pearls", ENTITY_TREES),
        ("Inventory", ITEMS),
        ("EnderItems", ITEMS),
        ("ShoulderEntityLeft", ENTITY_TREE),
        ("ShoulderEntityRight", ENTITY_TREE),
        ("recipeBook", Node::Fields(&[("recipes", Node::List(&RECIPE)), ("toBeDisplayed", Node::List(&RECIPE))])),
    ]),
]);

const CHUNK: Node = Node::Fields(&[
    ("entities", ENTITY_TREES),
    // A block entity the game does not know is kept as it is.
    ("block_entities", Node::List(&Node::Either(&[BLOCK_ENTITY, Node::Plain]))),
    ("block_ticks", Node::List(&Node::Required(&[("i", BLOCK_NAME)]))),
    (
        "sections",
        Node::List(&Node::Fields(&[
            ("biomes", Node::Fields(&[("palette", Node::List(&BIOME))])),
            ("block_states", Node::Fields(&[("palette", Node::List(&STATE))])),
        ])),
    ),
    ("structures", Node::Fields(&[("starts", Node::Values(&Node::Of(Kind::StructureFeature)))])),
]);

const STRUCTURE: Node = Node::Fields(&[
    ("entities", Node::List(&Node::Fields(&[("nbt", ENTITY_TREE)]))),
    ("blocks", Node::List(&Node::Fields(&[("nbt", BLOCK_ENTITY)]))),
    ("palette", Node::List(&STATE)),
    ("palettes", Node::List(&Node::List(&STATE))),
]);

const STATS: Node = Node::Fields(&[(
    "stats",
    Node::Fields(&[
        ("minecraft:mined", Node::KeysOf(Kind::BlockName)),
        ("minecraft:crafted", Node::KeysOf(Kind::ItemName)),
        ("minecraft:used", Node::KeysOf(Kind::ItemName)),
        ("minecraft:broken", Node::KeysOf(Kind::ItemName)),
        ("minecraft:picked_up", Node::KeysOf(Kind::ItemName)),
        ("minecraft:dropped", Node::KeysOf(Kind::ItemName)),
        ("minecraft:killed", Node::KeysOf(Kind::EntityName)),
        ("minecraft:killed_by", Node::KeysOf(Kind::EntityName)),
        ("minecraft:custom", Node::NamespacedKeys(&Node::Plain)),
    ]),
)]);

const ADVANCEMENTS: Node = Node::Fields(&[
    ("minecraft:adventure/adventuring_time", Node::Fields(&[("criteria", Node::KeysOf(Kind::Biome))])),
    ("minecraft:adventure/kill_a_mob", Node::Fields(&[("criteria", Node::KeysOf(Kind::EntityName))])),
    ("minecraft:adventure/kill_all_mobs", Node::Fields(&[("criteria", Node::KeysOf(Kind::EntityName))])),
    ("minecraft:husbandry/bred_all_animals", Node::Fields(&[("criteria", Node::KeysOf(Kind::EntityName))])),
]);

fn saved_data(data: SavedData) -> Node {
    match data {
        SavedData::CustomBossEvents => Node::Fields(&[("data", Node::Values(&Node::Fields(&[("Name", TEXT)])))]),
        SavedData::MapData => {
            Node::Fields(&[("data", Node::Fields(&[("banners", Node::List(&Node::Fields(&[("name", TEXT)])))]))])
        }
        SavedData::Scoreboard => Node::Fields(&[(
            "data",
            Node::Fields(&[
                ("Objectives", Node::List(&Node::Of(Kind::Objective))),
                ("Teams", Node::List(&Node::Of(Kind::Team))),
                ("PlayerScores", Node::List(&Node::Fields(&[("display", TEXT)]))),
            ]),
        )]),
        SavedData::StructureFeatureIndices => Node::Fields(&[(
            "data",
            Node::Fields(&[("Features", Node::Values(&Node::Of(Kind::StructureFeature)))]),
        )]),
        SavedData::WorldGenSettings => Node::Required(&[("data", Node::Of(Kind::WorldGenSettings))]),
        SavedData::CommandStorage
        | SavedData::EnderDragonFight
        | SavedData::GameRules
        | SavedData::Tickets
        | SavedData::MapIndex
        | SavedData::Raids
        | SavedData::RandomSequences
        | SavedData::ScheduledEvents
        | SavedData::Stopwatches
        | SavedData::WanderingTrader
        | SavedData::Weather
        | SavedData::WorldBorder
        | SavedData::WorldClocks => Node::Plain,
    }
}

const EQUIPMENT: Node = Node::Fields(&[(
    "equipment",
    Node::Fields(&[
        ("mainhand", ITEM),
        ("offhand", ITEM),
        ("feet", ITEM),
        ("legs", ITEM),
        ("chest", ITEM),
        ("head", ITEM),
        ("body", ITEM),
        ("saddle", ITEM),
    ]),
)]);

/// A game event listener (sculk sensors, wardens, allays).
const LISTENER: Node =
    Node::Fields(&[("listener", Node::Fields(&[("event", Node::Fields(&[("game_event", Node::Of(Kind::GameEventName))]))]))]);

const SPAWNER: Node = Node::Fields(&[
    ("SpawnPotentials", Node::List(&Node::Required(&[("data", Node::Required(&[("entity", ENTITY_TREE)]))]))),
    ("SpawnData", Node::Required(&[("entity", ENTITY_TREE)])),
]);

const TRADES: Node = Node::Fields(&[("Recipes", Node::List(&Node::Of(Kind::VillagerTrade)))]);

/// An entity: its equipment, its name, then the fields its identifier gives it. Entities added
/// by a schema are unknown to the earlier ones.
fn entity(schema: Schema) -> Node {
    if schema.at_least(5000, 0) {
        ENTITY_SINCE_5000
    } else if schema.at_least(4997, 0) {
        ENTITY_SINCE_4997
    } else {
        ENTITY_BEFORE_4997
    }
}

const ENTITY_SINCE_5000: Node = Node::All(&[
    Node::Of(Kind::EntityEquipment),
    Node::Fields(&[("CustomName", TEXT)]),
    Node::Choice { key: "id", namespaced: true, cases: ENTITIES_SINCE_4997, plain: PLAIN_ENTITIES_SINCE_5000 },
]);
const ENTITY_SINCE_4997: Node = Node::All(&[
    Node::Of(Kind::EntityEquipment),
    Node::Fields(&[("CustomName", TEXT)]),
    Node::Choice { key: "id", namespaced: true, cases: ENTITIES_SINCE_4997, plain: PLAIN_ENTITIES_SINCE_4997 },
]);
const ENTITY_BEFORE_4997: Node = Node::All(&[
    Node::Of(Kind::EntityEquipment),
    Node::Fields(&[("CustomName", TEXT)]),
    Node::Choice { key: "id", namespaced: true, cases: ENTITIES_BEFORE_4997, plain: PLAIN_ENTITIES_BEFORE_4997 },
]);

/// Entities whose identifier gives them nested data (the minecart and boat families, item
/// holders, traders...). The last one was added at 4997.
const ENTITIES_SINCE_4997: &[(&str, Node)] = &[
    ("acacia_chest_boat", CHEST_ITEMS),
    ("allay", Node::All(&[Node::Fields(&[("Inventory", ITEMS)]), LISTENER])),
    ("area_effect_cloud", Node::Fields(&[("custom_particle", Node::Of(Kind::Particle))])),
    ("arrow", ARROW),
    ("bamboo_chest_raft", CHEST_ITEMS),
    ("birch_chest_boat", CHEST_ITEMS),
    ("block_display", Node::Fields(&[("block_state", STATE)])),
    ("cherry_chest_boat", CHEST_ITEMS),
    ("chest_minecart", Node::Fields(&[("DisplayState", STATE), ("Items", ITEMS)])),
    ("command_block_minecart", Node::Fields(&[("DisplayState", STATE), ("LastOutput", TEXT)])),
    ("dark_oak_chest_boat", CHEST_ITEMS),
    ("donkey", CHEST_ITEMS),
    ("enderman", Node::Fields(&[("carriedBlockState", STATE)])),
    ("falling_block", Node::Fields(&[("BlockState", STATE), ("TileEntityData", BLOCK_ENTITY)])),
    ("firework_rocket", Node::Fields(&[("FireworksItem", ITEM)])),
    ("furnace_minecart", MINECART),
    ("glow_item_frame", HELD_ITEM),
    ("hopper_minecart", Node::Fields(&[("DisplayState", STATE), ("Items", ITEMS)])),
    ("item", HELD_ITEM),
    ("item_display", Node::Fields(&[("item", ITEM)])),
    ("item_frame", HELD_ITEM),
    ("jungle_chest_boat", CHEST_ITEMS),
    ("lingering_potion", HELD_ITEM),
    ("llama", CHEST_ITEMS),
    ("mangrove_chest_boat", CHEST_ITEMS),
    ("minecart", MINECART),
    ("mule", CHEST_ITEMS),
    ("oak_chest_boat", CHEST_ITEMS),
    ("ominous_item_spawner", Node::Fields(&[("item", ITEM)])),
    ("pale_oak_chest_boat", CHEST_ITEMS),
    ("piglin", Node::Fields(&[("Inventory", ITEMS)])),
    ("pillager", Node::Fields(&[("Inventory", ITEMS)])),
    ("spawner_minecart", Node::All(&[MINECART, SPAWNER])),
    ("spectral_arrow", ARROW),
    ("splash_potion", HELD_ITEM),
    ("spruce_chest_boat", CHEST_ITEMS),
    ("text_display", Node::Fields(&[("text", TEXT)])),
    ("tnt", Node::Fields(&[("block_state", STATE)])),
    ("tnt_minecart", MINECART),
    ("trader_llama", CHEST_ITEMS),
    ("trident", Node::Fields(&[("inBlockState", STATE), ("item", ITEM)])),
    ("villager", TRADER),
    ("wandering_trader", TRADER),
    ("warden", LISTENER),
    ("zombie_villager", Node::Fields(&[("Offers", TRADES)])),
    ("poplar_chest_boat", CHEST_ITEMS),
];
const ENTITIES_BEFORE_4997: &[(&str, Node)] = ENTITIES_SINCE_4997.split_at(ENTITIES_SINCE_4997.len() - 1).0;

const CHEST_ITEMS: Node = Node::Fields(&[("Items", ITEMS)]);
const HELD_ITEM: Node = Node::Fields(&[("Item", ITEM)]);
const MINECART: Node = Node::Fields(&[("DisplayState", STATE)]);
const ARROW: Node = Node::Fields(&[("inBlockState", STATE), ("item", ITEM), ("weapon", ITEM)]);
const TRADER: Node = Node::Fields(&[("Inventory", ITEMS), ("Offers", TRADES)]);

/// Entities known to hold nothing nested beyond their equipment and name. The last two were
/// added at 4997 (`poplar_boat`) and 5000 (`cushion`).
const PLAIN_ENTITIES_SINCE_5000: &[&str] = &[
    "acacia_boat", "armadillo", "armor_stand", "axolotl", "bamboo_raft", "bat", "bee", "bee_stinger",
    "birch_boat", "blaze", "bogged", "breeze", "breeze_wind_charge", "camel", "camel_husk", "cat",
    "cave_spider", "cherry_boat", "chicken", "cod", "copper_golem", "cow", "creaking",
    "creaking_transient", "creeper", "dark_oak_boat", "dolphin", "dragon_fireball", "drowned", "egg",
    "elder_guardian", "end_crystal", "ender_dragon", "ender_pearl", "endermite", "evoker", "evoker_fangs",
    "experience_bottle", "experience_orb", "eye_of_ender", "fireball", "fox", "frog", "ghast", "giant",
    "glow_squid", "goat", "guardian", "happy_ghast", "hoglin", "horse", "husk", "illusioner",
    "interaction", "iron_golem", "jungle_boat", "leash_knot", "llama_spit", "magma_cube", "mangrove_boat",
    "mannequin", "marker", "mooshroom", "nautilus", "oak_boat", "ocelot", "painting", "pale_oak_boat",
    "panda", "parched", "parrot", "phantom", "pig", "piglin_brute", "polar_bear", "pufferfish", "rabbit",
    "ravager", "salmon", "sheep", "shulker", "shulker_bullet", "silverfish", "skeleton", "skeleton_horse",
    "slime", "small_fireball", "sniffer", "snow_golem", "snowball", "spider", "spruce_boat", "squid",
    "stray", "strider", "sulfur_cube", "tadpole", "tropical_fish", "turtle", "vex", "vindicator",
    "wind_charge", "witch", "wither", "wither_skeleton", "wither_skull", "wolf", "zoglin", "zombie",
    "zombie_horse", "zombie_nautilus", "zombified_piglin", "poplar_boat", "cushion",
];
const PLAIN_ENTITIES_SINCE_4997: &[&str] = PLAIN_ENTITIES_SINCE_5000.split_at(PLAIN_ENTITIES_SINCE_5000.len() - 1).0;
const PLAIN_ENTITIES_BEFORE_4997: &[&str] = PLAIN_ENTITIES_SINCE_4997.split_at(PLAIN_ENTITIES_SINCE_4997.len() - 1).0;

/// A block entity: its components, then the fields its identifier gives it. The decorated pot's
/// sherds became four item stacks at 4996.1 (a list of item identifiers before).
fn block_entity(schema: Schema) -> Node {
    if schema.at_least(4996, 1) { BLOCK_ENTITY_SINCE_4996_1 } else { BLOCK_ENTITY_BEFORE_4996_1 }
}

const BLOCK_ENTITY_SINCE_4996_1: Node = Node::All(&[
    Node::Fields(&[("components", Node::Of(Kind::DataComponents))]),
    Node::Choice { key: "id", namespaced: true, cases: BLOCK_ENTITIES_SINCE_4996_1, plain: PLAIN_BLOCK_ENTITIES },
]);
const BLOCK_ENTITY_BEFORE_4996_1: Node = Node::All(&[
    Node::Fields(&[("components", Node::Of(Kind::DataComponents))]),
    Node::Choice { key: "id", namespaced: true, cases: BLOCK_ENTITIES_BEFORE_4996_1, plain: PLAIN_BLOCK_ENTITIES },
]);

/// Four item stacks, one per side (pot decorations since 4996, sherds since 4996.1).
const POT_FACES: Node = Node::Fields(&[("back", ITEM), ("left", ITEM), ("right", ITEM), ("front", ITEM)]);

/// Block entities whose identifier gives them nested data; the decorated pot comes last, the
/// only one whose layout changed in the versions covered.
const BLOCK_ENTITIES_SINCE_4996_1: &[(&str, Node)] = &[
    ("banner", NAMED),
    ("barrel", NAMED_CONTAINER),
    ("beacon", NAMED),
    ("beehive", Node::Fields(&[("bees", Node::List(&Node::Fields(&[("entity_data", ENTITY_TREE)])))])),
    ("blast_furnace", COOKER),
    ("brewing_stand", NAMED_CONTAINER),
    ("brushable_block", Node::Fields(&[("item", ITEM)])),
    ("campfire", CONTAINER),
    ("chest", NAMED_CONTAINER),
    ("chiseled_bookshelf", CONTAINER),
    ("command_block", Node::Fields(&[("CustomName", TEXT), ("LastOutput", TEXT)])),
    ("crafter", NAMED_CONTAINER),
    ("dispenser", NAMED_CONTAINER),
    ("dropper", NAMED_CONTAINER),
    ("enchanting_table", NAMED),
    ("furnace", COOKER),
    ("hanging_sign", SIGN),
    ("hopper", NAMED_CONTAINER),
    ("jigsaw", Node::Fields(&[("final_state", Node::Of(Kind::FlatBlockState))])),
    ("jukebox", Node::Fields(&[("RecordItem", ITEM)])),
    ("lectern", Node::Fields(&[("Book", ITEM)])),
    ("mob_spawner", SPAWNER),
    ("piston", Node::Fields(&[("blockState", STATE)])),
    ("sculk_sensor", LISTENER),
    ("sculk_shrieker", LISTENER),
    ("shelf", CONTAINER),
    ("shulker_box", NAMED_CONTAINER),
    ("sign", SIGN),
    ("skull", Node::Fields(&[("custom_name", TEXT)])),
    ("smoker", COOKER),
    (
        "test_instance_block",
        Node::Fields(&[
            ("data", Node::Fields(&[("error_message", TEXT)])),
            ("errors", Node::List(&Node::Fields(&[("text", TEXT)]))),
        ]),
    ),
    ("trapped_chest", NAMED_CONTAINER),
    (
        "trial_spawner",
        Node::Fields(&[
            ("spawn_potentials", Node::List(&Node::Required(&[("data", Node::Required(&[("entity", ENTITY_TREE)]))]))),
            ("spawn_data", Node::Required(&[("entity", ENTITY_TREE)])),
        ]),
    ),
    (
        "vault",
        Node::Fields(&[
            ("config", Node::Fields(&[("key_item", ITEM)])),
            ("server_data", Node::Fields(&[("items_to_eject", ITEMS)])),
            ("shared_data", Node::Fields(&[("display_item", ITEM)])),
        ]),
    ),
    ("decorated_pot", Node::Fields(&[("sherds", POT_FACES), ("item", ITEM)])),
];
/// Before 4996.1: the same, the pot's sherds being a list of item identifiers.
const BLOCK_ENTITIES_BEFORE_4996_1: &[(&str, Node)] = &{
    let mut cases = [("", Node::Plain); BLOCK_ENTITIES_SINCE_4996_1.len()];
    let last = cases.len() - 1;
    let mut i = 0;
    while i < last {
        cases[i] = BLOCK_ENTITIES_SINCE_4996_1[i];
        i += 1;
    }
    cases[last] = ("decorated_pot", Node::Fields(&[("sherds", Node::List(&ITEM_NAME)), ("item", ITEM)]));
    cases
};

const NAMED: Node = Node::Fields(&[("CustomName", TEXT)]);
const CONTAINER: Node = Node::Fields(&[("Items", ITEMS)]);
const NAMED_CONTAINER: Node = Node::Fields(&[("Items", ITEMS), ("CustomName", TEXT)]);
const COOKER: Node =
    Node::Fields(&[("Items", ITEMS), ("CustomName", TEXT), ("RecipesUsed", Node::KeysOf(Kind::Recipe))]);
const SIGN_SIDE: Node = Node::Fields(&[("messages", TEXTS), ("filtered_messages", TEXTS)]);
const SIGN: Node = Node::Fields(&[("front_text", SIGN_SIDE), ("back_text", SIGN_SIDE)]);

const PLAIN_BLOCK_ENTITIES: &[&str] = &[
    "DUMMY", "bell", "calibrated_sculk_sensor", "comparator", "conduit", "copper_golem_statue",
    "creaking_heart", "daylight_detector", "end_gateway", "end_portal", "ender_chest", "potent_sulfur",
    "sculk_catalyst", "structure_block", "test_block",
];

/// The components of an item stack or a block entity. From 4996 the game's map of them starts
/// again from the one of 4059: `can_break` and `can_place_on` take back that older layout.
fn data_components(schema: Schema) -> Node {
    if schema.at_least(4996, 0) { COMPONENTS_SINCE_4996 } else { COMPONENTS_BEFORE_4996 }
}

/// The blocks an adventure mode predicate names: one identifier or a list.
const PREDICATE_BLOCKS: Node = Node::Fields(&[("blocks", Node::Either(&[BLOCK_NAME, Node::List(&BLOCK_NAME)]))]);
/// Can-break / can-place-on before 4996: one predicate or a list of them.
const ADVENTURE_PREDICATES_BEFORE_4996: Node = Node::Either(&[PREDICATE_BLOCKS, Node::List(&PREDICATE_BLOCKS)]);
/// Can-break / can-place-on since 4996: the predicates under `predicates`.
const ADVENTURE_PREDICATES_SINCE_4996: Node = Node::Fields(&[("predicates", Node::List(&PREDICATE_BLOCKS))]);

/// The components, with the two layouts that differ between the epochs given.
macro_rules! components {
    ($adventure:expr, $pot_decorations:expr) => {
        Node::Fields(&[
            ("minecraft:bees", Node::List(&Node::Fields(&[("entity_data", ENTITY_TREE)]))),
            ("minecraft:block_entity_data", BLOCK_ENTITY),
            ("minecraft:bundle_contents", ITEMS),
            ("minecraft:can_break", $adventure),
            ("minecraft:can_place_on", $adventure),
            ("minecraft:charged_projectiles", ITEMS),
            ("minecraft:container", Node::List(&Node::Fields(&[("item", ITEM)]))),
            ("minecraft:entity_data", ENTITY_TREE),
            ("minecraft:pot_decorations", $pot_decorations),
            ("minecraft:custom_name", TEXT),
            ("minecraft:item_name", TEXT),
            ("minecraft:lore", TEXTS),
            (
                "minecraft:written_book_content",
                Node::Fields(&[(
                    "pages",
                    Node::List(&Node::Either(&[Node::Fields(&[("raw", TEXT), ("filtered", TEXT)]), TEXT])),
                )]),
            ),
            ("minecraft:use_remainder", ITEM),
            (
                "minecraft:equippable",
                Node::Fields(&[("allowed_entities", Node::Either(&[ENTITY_NAME, Node::List(&ENTITY_NAME)]))]),
            ),
            ("minecraft:sulfur_cube_content", ITEM),
        ])
    };
}

const COMPONENTS_BEFORE_4996: Node = components!(ADVENTURE_PREDICATES_BEFORE_4996, Node::List(&ITEM_NAME));
const COMPONENTS_SINCE_4996: Node = components!(ADVENTURE_PREDICATES_SINCE_4996, POT_FACES);

/// A text: plain text, a list of texts, or a compound with children and a hover event.
const TEXT_COMPONENT: Node = Node::Either(&[
    Node::Text,
    TEXTS,
    Node::Fields(&[
        ("extra", TEXTS),
        ("separator", TEXT),
        (
            "hover_event",
            Node::Choice {
                key: "action",
                namespaced: false,
                cases: &[
                    // The hover event compound is the shown item stack itself.
                    ("show_item", ITEM),
                    ("show_entity", Node::Fields(&[("id", ENTITY_NAME), ("name", TEXT)])),
                    ("show_text", Node::Fields(&[("value", TEXT)])),
                ],
                plain: &[],
            },
        ),
    ]),
]);

const OBJECTIVE: Node = Node::All(&[Node::PackedCriterion, Node::Fields(&[("DisplayName", TEXT)])]);

const WORLD_GEN_SETTINGS: Node = Node::Required(&[(
    "dimensions",
    Node::NamespacedKeys(&Node::Required(&[(
        "generator",
        Node::Choice {
            key: "type",
            namespaced: false,
            cases: &[
                (
                    "minecraft:flat",
                    Node::Fields(&[(
                        "settings",
                        Node::Fields(&[("biome", BIOME), ("layers", Node::List(&Node::Fields(&[("block", BLOCK_NAME)])))]),
                    )]),
                ),
                (
                    "minecraft:noise",
                    Node::Fields(&[
                        ("biome_source", BIOME_SOURCE),
                        (
                            "settings",
                            Node::Either(&[
                                Node::Text,
                                Node::Fields(&[("default_block", BLOCK_NAME), ("default_fluid", BLOCK_NAME)]),
                            ]),
                        ),
                    ]),
                ),
            ],
            plain: &["minecraft:debug"],
        },
    )])),
)]);

const BIOME_SOURCE: Node = Node::Choice {
    key: "type",
    namespaced: false,
    cases: &[
        ("minecraft:checkerboard", Node::Required(&[("biomes", Node::List(&BIOME))])),
        ("minecraft:fixed", Node::Required(&[("biome", BIOME)])),
        (
            "minecraft:multi_noise",
            Node::Either(&[
                Node::Required(&[("preset", Node::Of(Kind::MultiNoiseParameterList))]),
                Node::List(&Node::Required(&[("biome", BIOME)])),
            ]),
        ),
    ],
    plain: &["minecraft:the_end"],
};
