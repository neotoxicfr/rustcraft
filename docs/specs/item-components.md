# Item components: network encoding (26.3, protocol 777)

Written in a clean room from the decompiled game, in three parts (order of the ids of the
`minecraft:data_component_type` registry, checked against the official report: 122 types). A network
stack has no length prefix: each component is read according to its encoding.
Decoder: `crates/rc-proto-v777/src/item.rs`.

Glossary: `varint`, `bool`, `f32`… ; `id(registry)` = raw varint; `ref(registry)` = varint,
0 = direct value inline, otherwise id + 1; `set(registry)` = varint n, 0 = tag
(identifier), otherwise n − 1 ids; `list(T)` = varint count then elements;
`option(T)` = bool then T; `text` = text component in network NBT; `stack` / `template`:
see part 3.

## Part 1: `custom_data` to `block_transformer`

Server → client network encoding (codec: the network codec). No length prefix.
Reminders checked in the byte codecs:
- `option(T)` = bool then T; `list(T)` = varint count (bound checked at decoding) then T…
- `map(K,V)` = varint count then (key, value) pairs one after another.
- `either(L,R)` = bool: true → L, false → R.
- `id(registry)` via "the registry codec" = raw varint id (no offset, no direct form).
- `ref(registry)` = varint n, 0 → direct inline, otherwise id n−1.
- `set(registry)` = varint n, 0 → tag `identifier` (string), otherwise n−1 × `id(registry)`.
- Enumerations "the by-id codec": varint of the value's numeric id.

### Components

#### `minecraft:custom_data`
- Not synchronized over the network (persistent only).

#### `minecraft:max_stack_size`
- Encoding: `value: varint` (1..99 on the data side).

#### `minecraft:max_damage`
- Encoding: `value: varint` (> 0).

#### `minecraft:damage`
- Encoding: `value: varint` (≥ 0).

#### `minecraft:unbreakable`
- Encoding: no bytes (unit).

#### `minecraft:use_effects`
- Encoding:
  - `can_sprint: bool`
  - `interact_vibrations: bool`
  - `speed_multiplier: f32`

#### `minecraft:custom_name`
- Encoding: `name: text`.

#### `minecraft:minimum_attack_charge`
- Encoding: `value: f32` (0..1).

#### `minecraft:damage_type`
- Encoding: `type: id(minecraft:damage_type)` (raw varint, synchronized registry; no direct form).

#### `minecraft:item_name`
- Encoding: `name: text`.

#### `minecraft:item_model`
- Encoding: `model: identifier`.

#### `minecraft:lore`
- Encoding: `lines: list(text)`, max 256.

#### `minecraft:rarity`
- Encoding: `rarity: varint enum`: 0 common, 1 uncommon, 2 rare, 3 epic (out of range → common).

#### `minecraft:enchantments`
- Encoding: `enchantments: map(id(minecraft:enchantment) → varint level)` (varint count, then pairs).

#### `minecraft:can_place_on`
- Encoding: `predicates: list(Block predicate)` (no bound). See "Block predicate".
- (No more "show in tooltip" boolean: the list is the only field.)

#### `minecraft:can_break`
- Encoding: same as `can_place_on`.

#### `minecraft:attribute_modifiers`
- Encoding: `entries: list(Attribute entry)` (no bound), each entry:
  - `attribute: id(minecraft:attribute)` (raw varint)
  - `modifier_id: identifier`
  - `amount: f64`
  - `operation: varint enum`: 0 add_value, 1 add_multiplied_base, 2 add_multiplied_total (out of range → 0)
  - `slot: varint enum` (slot group): 0 any, 1 mainhand, 2 offhand, 3 hand, 4 feet, 5 legs, 6 chest, 7 head, 8 armor, 9 body, 10 saddle (out of range → any)
  - `display: varint type` then payload depending on the type:
    - 0 default → nothing
    - 1 hidden → nothing
    - 2 override → `text`
    (out of range → default)

#### `minecraft:custom_model_data`
- Encoding:
  - `floats: list(f32)`
  - `flags: list(bool)`
  - `strings: list(string)`
  - `colors: list(i32)` (big-endian i32, not varint)

#### `minecraft:tooltip_display`
- Encoding:
  - `hide_tooltip: bool`
  - `hidden_components: list(id(minecraft:data_component_type))` (ordered set, no bound)

#### `minecraft:repair_cost`
- Encoding: `value: varint`.

#### `minecraft:creative_slot_lock`
- Encoding: no bytes (unit). Synchronized over the network, not persistent.

#### `minecraft:enchantment_glint_override`
- Encoding: `value: bool`.

#### `minecraft:intangible_projectile`
- Not synchronized over the network (persistent only).

#### `minecraft:food`
- Encoding:
  - `nutrition: varint`
  - `saturation: f32`
  - `can_always_eat: bool`

#### `minecraft:consumable`
- Encoding:
  - `consume_seconds: f32`
  - `animation: varint enum` ("Use animation", see Shared types)
  - `sound: Sound event` (see Shared types)
  - `has_consume_particles: bool`
  - `on_consume_effects: list(Consume effect)` (no bound)

#### `minecraft:use_remainder`
- Encoding: `convert_into: Stack template` (see Shared types; WARNING different order from `stack`).

#### `minecraft:use_cooldown`
- Encoding:
  - `seconds: f32`
  - `cooldown_group: option(identifier)`

#### `minecraft:damage_resistant`
- Encoding: `types: set(minecraft:damage_type)` (tag or direct list; not just a tag).

#### `minecraft:tool`
- Encoding:
  - `rules: list(Rule)` (no bound), each rule:
    - `blocks: set(minecraft:block)`
    - `speed: option(f32)`
    - `correct_for_drops: option(bool)`
  - `default_mining_speed: f32`
  - `damage_per_block: varint`
  - `can_destroy_blocks_in_creative: bool`

#### `minecraft:weapon`
- Encoding:
  - `item_damage_per_attack: varint`
  - `disable_blocking_for_seconds: f32`

#### `minecraft:attack_range`
- Encoding (6 × f32): `min_reach`, `max_reach`, `min_creative_reach`, `max_creative_reach`,
  `hitbox_margin`, `mob_factor`.

#### `minecraft:enchantable`
- Encoding: `value: varint` (> 0).

#### `minecraft:equippable`
- Encoding:
  - `slot: varint enum` ("Equipment slot", see Shared types)
  - `equip_sound: Sound event`
  - `asset_id: option(identifier)` (key of the `minecraft:equipment_asset` registry, sent as an identifier)
  - `camera_overlay: option(identifier)`
  - `allowed_entities: option(set(minecraft:entity_type))`
  - `dispensable: bool`
  - `swappable: bool`
  - `damage_on_hurt: bool`
  - `equip_on_interact: bool`
  - `can_be_sheared: bool`
  - `shearing_sound: Sound event`

#### `minecraft:repairable`
- Encoding: `items: set(minecraft:item)`.

#### `minecraft:glider`
- Encoding: no bytes (unit).

#### `minecraft:tooltip_style`
- Encoding: `style: identifier`.

#### `minecraft:death_protection`
- Encoding: `death_effects: list(Consume effect)` (no bound).

#### `minecraft:blocks_attacks`
- Encoding:
  - `block_delay_seconds: f32`
  - `disable_cooldown_scale: f32`
  - `damage_reductions: list(Reduction)` (no bound), each reduction:
    - `horizontal_blocking_angle: f32`
    - `type: option(set(minecraft:damage_type))`
    - `base: f32`
    - `factor: f32`
  - `item_damage` (always present): `threshold: f32`, `base: f32`, `factor: f32`
  - `bypassed_by: option(set(minecraft:damage_type))`
  - `block_sound: option(Sound event)`
  - `disable_sound: option(Sound event)`

#### `minecraft:piercing_weapon`
- Encoding:
  - `deals_knockback: bool`
  - `dismounts: bool`
  - `sound: option(Sound event)`
  - `hit_sound: option(Sound event)`

#### `minecraft:kinetic_weapon`
- Encoding:
  - `contact_cooldown_ticks: varint`
  - `delay_ticks: varint`
  - `dismount_conditions: option(Condition)`
  - `knockback_conditions: option(Condition)`
  - `damage_conditions: option(Condition)`
  - `forward_movement: f32`
  - `damage_multiplier: f32`
  - `sound: option(Sound event)`
  - `hit_sound: option(Sound event)`
  - where Condition = `max_duration_ticks: varint`, `min_speed: f32`, `min_relative_speed: f32`.

#### `minecraft:attack_animation`
- Encoding: "Swing animation" (see Shared types).

#### `minecraft:interact_animation`
- Encoding: "Swing animation".

#### `minecraft:additional_trade_cost`
- Encoding: `value: varint`. Synchronized over the network, not persistent.

#### `minecraft:block_transformer` (declaration starting at l. 229, included as a precaution)
- Encoding: `transformer: id(minecraft:block_transformer)` (raw varint, no direct form).

<!-- END COMPONENTS -->

### Shared types

### Block predicate (used by can_place_on / can_break)
Fields in order:
1. `blocks: option(set(minecraft:block))`
2. `properties: option(list(Property match))` (list with no bound), each element:
   - `name: string`
   - `value: either` → `bool`:
     - true → exact: `value: string`
     - false → range: `min: option(string)`, `max: option(string)`
3. `nbt: option(nbt)` — network NBT of a compound (type 0x0A mandatory; type 0 refused at decoding).
4. `components: Component matches` (always present, not an option):
   a. `exact: list(Typed component)` (no bound), each element = `id(minecraft:data_component_type)` then
      the value encoded with that component's network codec (recursive: same table as the stack).
   b. `partial: list(Partial predicate)`, max 64, each element:
      - `type: either` → `bool`:
        - true → `id(minecraft:data_component_predicate_type)` (static registry: damage, enchantments,
          stored_enchantments, potion_contents, custom_data, container, bundle_contents,
          firework_explosion, fireworks, writable_book_content, written_book_content,
          attribute_modifiers, trim, jukebox_playable, villager/variant — registration order to
          check in the registry report)
        - false → `id(minecraft:data_component_type)`: "any value" predicate of this component
      - `value: nbt` (network NBT, non-empty): the predicate serialized via its data codec (with
        registry context). For the "any value" form, this is the serialization of a
        unit (expected: empty compound `0A 00`). A client may just read a whole NBT.

### Use animation
`varint enum` (out of range → none): 0 none, 1 eat, 2 drink, 3 block, 4 bow, 5 trident, 6 crossbow,
7 spyglass, 8 toot_horn, 9 brush, 10 bundle, 11 spear.

### Sound event
`ref(minecraft:sound_event)`: varint n; n > 0 → id n−1; n = 0 → direct:
- `location: identifier`
- `fixed_range: option(f32)`

### Stack template (used by use_remainder)
Order: `item: id(minecraft:item)` (raw varint), then `quantity: varint`, then component patch
(varint count added, varint count removed, added = `id(data_component_type)` + value, removed = id only).
No leading quantity and no "empty" case: different from `stack`.

### Status effect
- `effect: id(minecraft:mob_effect)` (raw varint)
- then "Details":
  - `amplifier: varint`
  - `duration: varint`
  - `ambient: bool`
  - `show_particles: bool`
  - `show_icon: bool`
  - `hidden_effect: option(Details)` (recursive: same 5 fields + option, without effect id)

### Consume effect
Selector: `type: id(minecraft:consume_effect_type)` (raw varint, static registry; registration
order: 0 apply_effects, 1 remove_effects, 2 clear_all_effects, 3 teleport_randomly,
4 play_sound — order of the source file, to confirm with the registry report), then:
- apply_effects: `effects: list(Status effect)`, `probability: f32`
- remove_effects: `effects: set(minecraft:mob_effect)`
- clear_all_effects: nothing
- teleport_randomly: `diameter: f32`, `directional_particles: bool`
- play_sound: `sound: Sound event`

### Equipment slot
`varint enum` by own id (NOT the ordinal; out of range → mainhand):
0 mainhand, 1 feet, 2 legs, 3 chest, 4 head, 5 offhand, 6 body, 7 saddle.

### Swing animation
- `type: varint enum` (out of range → none): 0 none, 1 whack, 2 stab
- `duration: varint`

<!-- END TYPES -->

### New primitives

- `either(L, R)`: `bool`; true → L, false → R (used by the property and component predicates).
- `map(K → V)`: varint number of pairs, then K V K V…

### Uncertain points

1. "Any value" partial predicate (Block predicate, partial, `false` branch): the NBT payload
   comes from a DFU unit codec, a library not decompiled here. Expected:
   empty compound `0A 00`. No risk for decoding if the client reads a whole NBT whatever it is.
   File: the corresponding official file.
2. Concrete partial predicates: payload = NBT produced by each predicate's data codec
   (with registries); not detailed field by field, a client simply reads an NBT. Files:
   sources of the game's component predicates.
3. Numeric orders of the static registries `minecraft:consume_effect_type` and
   `minecraft:data_component_predicate_type` deduced from the registration order in the
   official files for consume effects and component predicates; to confirm with the registry
   report of the official server (--reports).
4. `exact` of the Block predicate: each value reuses the network codec of the named component
   (recursion into the whole table); if the type has no network codec, behavior not followed
   (the corresponding official file).

## Part 2: `villager_food` to `block_state`

Source read: decompiled game 26.3 (clean room, notes in our own words). Server → client direction.
Synchronized registries (network list of the registry data loader) useful here: `minecraft:enchantment`,
`minecraft:trim_pattern`, `minecraft:trim_material`, `minecraft:banner_pattern`,
`minecraft:jukebox_song`, `minecraft:instrument`, `minecraft:block_transformer`.

### Components

#### `minecraft:block_transformer`
- Encoding: `transformer: id(minecraft:block_transformer)` (reference only, NO direct
  form: varint = network id as is, without −1 offset).

#### `minecraft:villager_food`
- Encoding: `nutrition: varint`.

#### `minecraft:stored_enchantments`
- Encoding: see Shared types "Enchantments" (same encoding as `minecraft:enchantments`).

#### `minecraft:dye`
- Encoding: `color: enum(Dye color)` (see Shared types).

#### `minecraft:dyed_color`
- Encoding: `rgb: i32`.

#### `minecraft:map_id`
- Encoding: `id: varint`.

#### `minecraft:map_decorations`
- Not synchronized over the network (persistent only): never appears in a network stack.

#### `minecraft:map_post_processing`
- Encoding: `operation: varint` (0 = lock `lock`, 1 = enlarge `scale`; value out of
  range → 0).

#### `minecraft:charged_projectiles`
- Encoding: `projectiles: list(Stack template)` (max 1024; see Shared types — WARNING,
  this is NOT `stack`, the field order differs).

#### `minecraft:bundle_contents`
- Encoding: `contents: list(Stack template)` (no network bound).

#### `minecraft:potion_contents`
- Encoding, in order:
  - `potion: option(id(minecraft:potion))` (reference only, no direct form)
  - `custom_color: option(i32)` (ARGB)
  - `custom_effects: list(Status effect)` (see Shared types)
  - `custom_name: option(string)` (max 32767)

#### `minecraft:potion_duration_scale`
- Encoding: `scale: f32`.

#### `minecraft:suspicious_stew_effects`
- Encoding: `list(entry)`, each entry:
  - `effect: id(minecraft:mob_effect)`
  - `duration: varint` (ticks)

#### `minecraft:writable_book_content`
- Encoding: `pages: list(Filterable(string(1024)))` (max 100 pages; see Shared types
  "Filterable"). String max = 1024 CHARACTERS (UTF-16), i.e. up to 3 × 1024 bytes.

#### `minecraft:written_book_content`
- Encoding, in order:
  - `title: Filterable(string(32))`
  - `author: string` (max 32767)
  - `generation: varint`
  - `pages: list(Filterable(text))` (no network bound)
  - `resolved: bool`

#### `minecraft:trim`
- Encoding, in order:
  - `material: ref(minecraft:trim_material)`; direct form = "Direct trim material"
  - `pattern: ref(minecraft:trim_pattern)`; direct form = "Direct trim pattern"
  (see Shared types).

#### `minecraft:debug_stick_state`
- Not synchronized over the network.

#### `minecraft:entity_data`
- Encoding, in order:
  - `type: id(minecraft:entity_type)` (static registry, raw varint, no direct form)
  - `data: nbt` (MANDATORY network NBT compound: a type 0/end causes an error;
    default NBT quota). The `id` field is removed from the compound on the sender side.

#### `minecraft:bucket_entity_data`
- Encoding: `data: nbt` (mandatory compound, default quota).

#### `minecraft:block_entity_data`
- Encoding, in order:
  - `type: id(minecraft:block_entity_type)` (raw varint)
  - `data: nbt` (mandatory compound, `id` removed).

#### `minecraft:instrument`
- Encoding: `instrument: ref(minecraft:instrument)`; direct form = "Direct instrument"
  (see Shared types).

#### `minecraft:provides_trim_material`
- Encoding: `material: ref(minecraft:trim_material)`; direct form = "Direct trim
  material".

#### `minecraft:ominous_bottle_amplifier`
- Encoding: `amplifier: varint`.

#### `minecraft:jukebox_playable`
- Encoding: `song: ref(minecraft:jukebox_song)`; direct form = "Direct jukebox
  song" (see Shared types).

#### `minecraft:provides_banner_patterns`
- Encoding: `patterns: set(minecraft:banner_pattern)` (elements = raw `id`, no direct
  form).

#### `minecraft:recipes`
- Not synchronized over the network.

#### `minecraft:lodestone_tracker`
- Encoding, in order:
  - `target: option(Global position)` (see Shared types)
  - `tracked: bool`

#### `minecraft:firework_explosion`
- Encoding: "Firework explosion" (see Shared types).

#### `minecraft:fireworks`
- Encoding, in order:
  - `flight_duration: varint`
  - `explosions: list(Firework explosion)` (max 256)

#### `minecraft:profile`
- Encoding, in order:
  - `contents`: `bool` selector;
    - `true` → FULL profile: `uuid: uuid`, `name: string(16)`, `properties: Profile
      properties`
    - `false` → PARTIAL profile: `name: option(string(16))`, `uuid: option(uuid)`,
      `properties: Profile properties`
  - `appearance`: "Appearance override" (see Shared types)
  Decoder note: a partial one without properties with exactly one of the two (name XOR uuid) becomes
  a "dynamic" profile to resolve; otherwise a static profile. No effect on the bytes.

#### `minecraft:note_block_sound`
- Encoding: `sound: identifier`.

#### `minecraft:banner_patterns`
- Encoding: `layers: list(layer)` (no network bound), each layer:
  - `pattern: ref(minecraft:banner_pattern)`; direct form = "Direct banner pattern"
  - `color: enum(Dye color)`

#### `minecraft:base_color`
- Encoding: `color: enum(Dye color)`.

#### `minecraft:pot_decorations`
- Encoding, in order (4 fixed fields):
  - `back: option(Stack template)`
  - `left: option(Stack template)`
  - `right: option(Stack template)`
  - `front: option(Stack template)`

#### `minecraft:container`
- Encoding: `slots: list(option(Stack template))` (max 256; a `false` = empty
  slot; the index in the list = slot number).

#### `minecraft:block_state`
- Encoding: `properties: list(pair)` (varint count, then for each pair `key: string`,
  `value: string`, max 32767 each; any order, hash table).

### Shared types

### Stack template
Always non-empty stack, order DIFFERENT from `stack`:
- `item: id(minecraft:item)` (raw varint)
- `quantity: varint` (no network bound; the disk format imposes 1-99)
- `components`: full component patch, identical to that of `stack`: varint count
  added, varint count removed, then added = `id(data_component_type)` + value (NON-delimited
  network codec, recursive), then removed = `id(data_component_type)` only.

### Enchantments
- `list(entry)`, each entry: `enchantment: id(minecraft:enchantment)` (raw varint,
  synchronized registry, no direct form), `level: varint`.

### Status effect
- `effect: id(minecraft:mob_effect)` (raw varint)
- then "Effect details":
  - `amplifier: varint`
  - `duration: varint` (ticks; −1 = infinite)
  - `ambient: bool`
  - `visible_particles: bool`
  - `visible_icon: bool`
  - `hidden_effect: option(Effect details)` — RECURSIVE: same structure (without effect id),
    nested as many times as needed.

### Filterable(T)
- `raw: T`
- `filtered: option(T)`

### Direct trim material (`ref` form n = 0)
- `palette: identifier`
- `description: text`

### Direct trim pattern (`ref` form n = 0)
- `resource: identifier`
- `description: text`
- `decal: bool`

### Sound event
- `ref(minecraft:sound_event)`; direct form: `name: identifier`, `fixed_range: option(f32)`.

### Direct instrument (`ref` form n = 0)
- `sound: Sound event`
- `use_duration: f32` (seconds)
- `range: f32`
- `durability_damage: varint`
- `description: text`

### Direct jukebox song (`ref` form n = 0)
- `sound: Sound event`
- `description: text`
- `duration_seconds: f32`
- `comparator_signal: varint`

### Direct banner pattern (`ref` form n = 0)
- `resource: identifier`
- `translation_key: string` (max 32767)

### Global position
- `dimension: identifier` (key of the `minecraft:dimension` registry, as a string)
- `position: i64` packed: x on the 26 high bits (shift 38), z on 26 bits
  (shift 12), y on the 12 low bits; each field signed (sign extension).

### Firework explosion
- `shape: varint` (0 small_ball, 1 large_ball, 2 star, 3 creeper, 4 burst; out of range → 0)
- `colors: list(i32)`
- `fade_colors: list(i32)`
- `trail: bool`
- `twinkle: bool`

### Profile properties
- varint count (max 16), then each property:
  - `name: string(64)`
  - `value: string` (32767)
  - `signature: option(string(1024))`

### Appearance override
- `body: option(identifier)`
- `cape: option(identifier)`
- `elytra: option(identifier)`
- `model: option(bool)` (true = thin "slim", false = large "wide")

### Dye color (enum)
- `varint` of id (out of range → 0): 0 white, 1 orange, 2 magenta, 3 light_blue, 4 yellow,
  5 lime, 6 pink, 7 gray, 8 light_gray, 9 cyan, 10 purple, 11 blue, 12 brown, 13 green,
  14 red, 15 black.

### New primitives
- `string(max)`: the max is in CHARACTERS (UTF-16 units); the decoder refuses a length in
  bytes > 3 × max.
- "raw" `id(...)`: varint = direct network id (registry codec / the registry codec),
  without the +1 offset of `ref`.

### Uncertain points
- (lifted) `nbt` of entity / bucket data: verified, non-compound tag or type 0 → error.
- `text`: taken as is from the glossary (serialization of text components = network NBT via
  codec with registries), not descended into again.
- `minecraft:block_transformer`: only the id is transmitted; the structure of the registry itself
  (the corresponding official file, state-provider field in NBT) belongs to registry sync,
  out of scope.


## Part 3: `bees` to the end, stack frame, inventory packets

Source: the corresponding official file lines 335–457 (+ codec classes followed down to the primitives).
Reminder: `id(registry)` = varint WITHOUT offset (codec "the registry codec" / "registry");
`ref(registry)` = varint with 0 = direct (codec "holder" with direct codec).

(Sections "Stack frame" and "Inventory packets": at the end of the file.)

### Components

#### `minecraft:bees`
- Encoding: `bees: list(Occupant)` (no max bound).
- Occupant (in order):
  - `entity_type: id(minecraft:entity_type)`
  - `data: nbt` (network NBT compound, default quota; never absent)
  - `ticks_in_hive: varint`
  - `min_ticks_in_hive: varint`

#### `minecraft:sulfur_cube_content`
- Encoding: a "Stack template" (see Shared types):
  - `item: id(minecraft:item)`
  - `quantity: varint`
  - `patch`: item component diff (same encoding as in a stack, see Stack frame)
- NB: no leading quantity varint like `stack`; here the item comes BEFORE the quantity.

#### `minecraft:lock`
- Not synchronized over the network.

#### `minecraft:container_loot`
- Not synchronized over the network.

#### `minecraft:break_sound`
- Encoding: `sound: ref(minecraft:sound_event)`; direct = "Sound event" (Shared types).

#### `minecraft:compostable`
- Encoding: `layers: Resolvable integer` (Shared types).

#### `minecraft:cooking_fuel`
- Encoding:
  - `burn_duration: Resolvable integer`
  - `speed_multiplier: Resolvable float`

#### `minecraft:brewing_fuel`
- Encoding:
  - `uses: Resolvable integer`
  - `speed_multiplier: Resolvable float`

#### `minecraft:mob_visibility`
- Encoding:
  - `targeting_types: set(minecraft:entity_type)`
  - `visibility: f32`

#### `minecraft:villager/variant`
- Encoding: `id(minecraft:villager_type)` (reference only, no direct).

#### `minecraft:wolf/variant`
- Encoding: `id(minecraft:wolf_variant)` (synchronized registry, reference only).

#### `minecraft:wolf/sound_variant`
- Encoding: `id(minecraft:wolf_sound_variant)`.

#### `minecraft:wolf/collar`
- Encoding: `enum(Dye color)` (Shared types).

#### `minecraft:fox/variant`
- Encoding: `varint` id: 0 `red`, 1 `snow` (out of range → 0).

#### `minecraft:salmon/size`
- Encoding: `varint` id: 0 `small`, 1 `medium`, 2 `large` (out of range → clamped).

#### `minecraft:parrot/variant`
- Encoding: `varint` id: 0 `red_blue`, 1 `blue`, 2 `green`, 3 `yellow_blue`, 4 `gray` (clamped).

#### `minecraft:tropical_fish/pattern`
- Encoding: `varint` = packed id = `base | (index << 8)`, base 0 = small, 1 = large.
  - Base 0: `kob` 0, `sunstreak` 256, `snooper` 512, `dasher` 768, `brinely` 1024, `spotty` 1280.
  - Base 1: `flopper` 1, `stripey` 257, `glitter` 513, `blockfish` 769, `betty` 1025, `clayfish` 1281.
  - Unknown id → `kob`.
  - Warning: multi-byte varint possible (256 → 2 bytes).

#### `minecraft:tropical_fish/base_color`
- Encoding: `enum(Dye color)`.

#### `minecraft:tropical_fish/pattern_color`
- Encoding: `enum(Dye color)`.

#### `minecraft:mooshroom/variant`
- Encoding: `varint` id: 0 `red`, 1 `brown` (clamped).

#### `minecraft:rabbit/variant`
- Encoding: `varint` id (SPARSE): 0 `brown`, 1 `white`, 2 `black`, 3 `white_splotched`,
  4 `gold`, 5 `salt`, 99 `evil` (unknown → `brown`).

#### `minecraft:pig/variant`
- Encoding: `id(minecraft:pig_variant)`.

#### `minecraft:pig/sound_variant`
- Encoding: `id(minecraft:pig_sound_variant)`.

#### `minecraft:cow/variant`
- Encoding: `id(minecraft:cow_variant)`.

#### `minecraft:cow/sound_variant`
- Encoding: `id(minecraft:cow_sound_variant)`.

#### `minecraft:chicken/variant`
- Encoding: `id(minecraft:chicken_variant)`.

#### `minecraft:chicken/sound_variant`
- Encoding: `id(minecraft:chicken_sound_variant)`.

#### `minecraft:zombie_nautilus/variant`
- Encoding: `id(minecraft:zombie_nautilus_variant)`.

#### `minecraft:frog/variant`
- Encoding: `id(minecraft:frog_variant)`.

#### `minecraft:horse/variant`
- Encoding: `varint` id: 0 `white`, 1 `creamy`, 2 `chestnut`, 3 `brown`, 4 `black`, 5 `gray`,
  6 `dark_brown` (out of range → modulo).

#### `minecraft:painting/variant`
- Encoding: `ref(minecraft:painting_variant)` (ONLY entity variant with a direct form).
  Direct (n = 0), in order:
  - `width: varint`
  - `height: varint`
  - `asset: identifier`
  - `title: option(text)`
  - `author: option(text)`

#### `minecraft:llama/variant`
- Encoding: `varint` id: 0 `creamy`, 1 `white`, 2 `brown`, 3 `gray` (clamped).

#### `minecraft:axolotl/variant`
- Encoding: `varint` id: 0 `lucy`, 1 `wild`, 2 `gold`, 3 `cyan`, 4 `blue` (out of range → 0).

#### `minecraft:cat/variant`
- Encoding: `id(minecraft:cat_variant)`.

#### `minecraft:cat/sound_variant`
- Encoding: `id(minecraft:cat_sound_variant)`.

#### `minecraft:cat/collar`
- Encoding: `enum(Dye color)`.

#### `minecraft:sheep/color`
- Encoding: `enum(Dye color)`.

#### `minecraft:shulker/color`
- Encoding: `enum(Dye color)`.

#### `minecraft:provides_pottery_pattern`
- Encoding: `id(minecraft:decorated_pot_pattern)` (reference only).

#### `minecraft:sign_text_front`
- Encoding: "Sign text" (Shared types).

#### `minecraft:sign_text_back`
- Encoding: "Sign text".

#### `minecraft:waxed`
- Encoding: nothing (0 bytes; unit value).

#### `minecraft:cushion/color`
- Encoding: `enum(Dye color)`.

### Shared types

### Dye color
- `varint` id: 0 white, 1 orange, 2 magenta, 3 light_blue, 4 yellow, 5 lime, 6 pink, 7 gray,
  8 light_gray, 9 cyan, 10 purple, 11 blue, 12 brown, 13 green, 14 red, 15 black
  (out of range → 0 white).

### Resolvable integer
- `bool` selector:
  - true → constant: `i32` (big-endian, NOT varint);
  - false → reference: `identifier` (key in the `minecraft:context_int_provider` registry,
    sent by its NAME, not by numeric id).

### Resolvable float
- `bool` selector:
  - true → constant: `f32`;
  - false → reference: `identifier` (key in `minecraft:context_float_provider`).

### Sound event (direct form)
- `name: identifier`
- `fixed_range: option(f32)`

### Sign text
- `lines: 4 × text` (exactly 4, no counter)
- `filtered_lines: option(4 × text)` (present only if different from the raw lines)
- `color: Dye color` (varint)
- `glowing: bool`

### Stack template
- `item: id(minecraft:item)`, `quantity: varint`, `patch`: item component diff.

### Stack frame

### Optional stack (`stack`, optional variant of the item stack codec)
1. `quantity: varint`. If `quantity <= 0` → empty stack, NOTHING else is read (negative is
   accepted and treated as empty). On writing, an empty stack (air, quantity ≤ 0) is written as `0`.
2. `item: id(minecraft:item)` (varint, no offset).
3. `patch`: item component diff:
   - `n_added: varint`, then `n_removed: varint` (no max bound checked; preallocation
     capped at 65536, no effect on the format).
   - If both are 0 → empty patch, end.
   - `n_added` × { `type: id(minecraft:data_component_type)`; `value` according to the network
     codec of the type } — no length prefix (the DELIMITED variant, with length, is only used
     by the "untrusted" codec, i.e. client → server).
   - `n_removed` × { `type: id(minecraft:data_component_type)` }.
   - Server-side order: all the added ones first, then all the removed ones (internal order of a
     table, meaningless; a reader must not assume any order between types).
- `stack!` (non-optional variant of the codec): same bytes, but quantity ≤ 0 = error.

### NON-synchronized component present in a patch
- It is sent anyway: a type without an explicit network codec gets a DEFAULT network
  codec = its persistent form (save Codec) converted to NBT (with registry
  context), written as network `nbt` (type byte then payload; End tag forbidden).
- So `custom_data`, `lock`, `container_loot`, and any other component without
  one: value = `nbt` (the exact form is that of the save JSON/NBT).
  No filtering of these components before sending was found (search on `minecraft:lock`,
  `minecraft:container_loot`: uses in block logic / tooltip only).
- A type with no codec at all ("transient") cannot exist on the construction side
  (the builder requires a persistent codec if no network codec is supplied) → all
  types have a network encoding.

### Order of the `minecraft:data_component_type` registry
- Static registry (not synchronized), ids = registration order = order of the declarations
  in the corresponding official file (class initialization triggered by the registry bootstrap;
  no other registration found elsewhere). 122 types, 0 to 121:

0 custom_data, 1 max_stack_size, 2 max_damage, 3 damage, 4 unbreakable, 5 use_effects,
6 custom_name, 7 minimum_attack_charge, 8 damage_type, 9 item_name, 10 item_model, 11 lore,
12 rarity, 13 enchantments, 14 can_place_on, 15 can_break, 16 attribute_modifiers,
17 custom_model_data, 18 tooltip_display, 19 repair_cost, 20 creative_slot_lock,
21 enchantment_glint_override, 22 intangible_projectile, 23 food, 24 consumable,
25 use_remainder, 26 use_cooldown, 27 damage_resistant, 28 tool, 29 weapon, 30 attack_range,
31 enchantable, 32 equippable, 33 repairable, 34 glider, 35 tooltip_style, 36 death_protection,
37 blocks_attacks, 38 piercing_weapon, 39 kinetic_weapon, 40 attack_animation,
41 interact_animation, 42 additional_trade_cost, 43 block_transformer, 44 villager_food,
45 stored_enchantments, 46 dye, 47 dyed_color, 48 map_id, 49 map_decorations,
50 map_post_processing, 51 charged_projectiles, 52 bundle_contents, 53 potion_contents,
54 potion_duration_scale, 55 suspicious_stew_effects, 56 writable_book_content,
57 written_book_content, 58 trim, 59 debug_stick_state, 60 entity_data, 61 bucket_entity_data,
62 block_entity_data, 63 instrument, 64 provides_trim_material, 65 ominous_bottle_amplifier,
66 jukebox_playable, 67 provides_banner_patterns, 68 recipes, 69 lodestone_tracker,
70 firework_explosion, 71 fireworks, 72 profile, 73 note_block_sound, 74 banner_patterns,
75 base_color, 76 pot_decorations, 77 container, 78 block_state, 79 bees,
80 sulfur_cube_content, 81 lock, 82 container_loot, 83 break_sound, 84 compostable,
85 cooking_fuel, 86 brewing_fuel, 87 mob_visibility, 88 villager/variant, 89 wolf/variant,
90 wolf/sound_variant, 91 wolf/collar, 92 fox/variant, 93 salmon/size, 94 parrot/variant,
95 tropical_fish/pattern, 96 tropical_fish/base_color, 97 tropical_fish/pattern_color,
98 mooshroom/variant, 99 rabbit/variant, 100 pig/variant, 101 pig/sound_variant,
102 cow/variant, 103 cow/sound_variant, 104 chicken/variant, 105 chicken/sound_variant,
106 zombie_nautilus/variant, 107 frog/variant, 108 horse/variant, 109 painting/variant,
110 llama/variant, 111 axolotl/variant, 112 cat/variant, 113 cat/sound_variant, 114 cat/collar,
115 sheep/color, 116 shulker/color, 117 provides_pottery_pattern, 118 sign_text_front,
119 sign_text_back, 120 waxed, 121 cushion/color.
(Automatic extraction: 122 declarations = 122 names; to cross-check with the registry report
of the official server if available.)

### Inventory packets (server → client, "play" state)

#### `container_set_content`
1. `container_id: varint`
2. `state_id: varint`
3. `items: list(stack)` (varint count, no max bound)
4. `carried_item (cursor): stack`

#### `container_set_slot`
1. `container_id: varint`
2. `state_id: varint`
3. `slot: i16`
4. `item: stack`

#### `set_player_inventory`
1. `slot: varint`
2. `item: stack`

#### `set_cursor_item`
1. `item: stack`

### Uncertain points
- `bees`: the occupant's NBT compound is read with a default quota; free content (no
  need to interpret it to move forward in the stream). Nothing uncertain about the format.
- Animal enumerations: out-of-range behavior noted (ZERO / CLAMP / WRAP / sparse) from
  the id tables; a client may simply reject or fall back to the default.
- Exact NBT form of the non-synchronized components (`custom_data`, `lock`, `container_loot`…):
  not detailed here (save codecs); to decode the stream it is enough to read an `nbt`.
- No other point not followed down to the primitives within scope C.
