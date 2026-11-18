# Items in an interface slot (26.3)

Written in a clean room from the decompiled client, in three parts: definitions
`items/*.json` and their evaluation (A), geometry and rendering in the item atlas (B), special
models, default components and display name (C). Complements `hud.md` § 3.4-3.5.
Default components: report of the official generator, read when preparing the data
(`rc-data`, table `ItemInfo`), never committed.

## Mission A — Item definitions (`assets/minecraft/items/*.json`) and evaluation in a GUI context

Target version: Minecraft Java 26.3 (protocol 777). Source: decompiled client, read in a clean room;
everything is reworded, no line of code copied. Identifiers without a namespace mean `minecraft:`.

#### 1. Finding an item's definition

1. Each item stack carries the component `minecraft:item_model` (an identifier). By default, when
   the item is registered, this component is **the item's registry id**
   (e.g. `minecraft:diamond_sword`). There is no other default in 26.3: the default value is
   fixed to the item's id. The server can replace it stack by stack (component sent over
   the network), which redirects to another definition.
2. If the component is **absent** from the stack (explicitly removed by the server), the item produces
   **no layer**: nothing is drawn (but the slot decorations — count, bar — are still
   handled elsewhere). `oversized_in_gui` is then untouched (stays false).
3. The identifier `ns:path` designates the file `assets/<ns>/items/<path>.json`.
4. If no definition is loaded for this identifier: use the **missing model**
   (geometry of the missing block model, purple/black checkerboard texture, see § 2.9), and the default
   properties of § 1.1.
5. An empty stack produces nothing (the rendering is cleared then we stop).

#### 1.1 Top-level fields of an `items/*.json` file

| Field | Type | Default | Meaning |
|---|---|---|---|
| `model` | item model object (§ 2) | required | model selection tree |
| `hand_animation_on_swap` | boolean | `true` | play the lower/raise animation of the hand when the stack in hand changes. If the `item_model` component is absent: `true`. No effect in GUI. |
| `oversized_in_gui` | boolean | `false` | allows the model to overflow the 16×16 slot in GUI (§ 7). No vanilla item sets it to true. |
| `swap_animation_scale` | float | `1.0` | factor applied to the speed/amplitude of the in-hand swap animation (vanilla: `1.95` for the 7 `*_spear` spears). If `item_model` is absent: `1.0`. No effect in GUI. |

Note: `oversized_in_gui` is read from the definition of the id **of the evaluated stack**; during a
`bundle/selected_item` (§ 2.8), the inner stack overwrites this flag with its own value
(last write wins).

#### 1.2 Input data of the evaluation in GUI (hotbar and inventory)

The evaluation of a tree receives: the stack, the display context, the world (or nothing), the holder
(or nothing), an integer seed.

| Case | context | world | holder | seed |
|---|---|---|---|---|
| Hotbar, slots 0..8 | `gui` | current client world | the player displayed by the hotbar (local player, or the observed player in spectator mode) | 1, 2, …, 9 (slot i → i+1) |
| Hotbar, offhand | `gui` | same | same | 10 |
| Slot of a container screen (inventory…) | `gui` | current client world | local player (`null` if none) | `slot.x + slot.y × screenImageWidth` (slot coordinates relative to the screen) |
| "Fake" slot of a screen (the fake item) | `gui` | client world | **absent** | same formula |
| Stack held at the cursor in a screen | `gui` | client world | local player | 0 |
| Creative tab icon | `gui` | client world | local player | 0 |

The seed is only used for pseudo-random properties (compass without a target, clock outside the Overworld,
§ 5). The world may be absent in the menus outside a game.

#### 2. Item model types (field `type`)

All the model objects are dispatched on `type`. Exhaustive list registered in 26.3:
`empty`, `model`, `range_dispatch`, `special`, `composite`, `bundle/selected_item`, `select`,
`condition`. Nothing else.

#### 2.0 Common field `transformation` (new, optional)

Accepted by `model`, `composite`, `condition`, `select`, `range_dispatch`, `special`
(not by `empty` nor `bundle/selected_item`). Two accepted forms:

- **decomposed form** (object, the 4 fields mandatory):
  `translation` [x,y,z], `left_rotation`, `scale` [x,y,z], `right_rotation`.
  A rotation is either a list of 4 floats `[x,y,z,w]` (quaternion, **normalized** on
  load), or an object `{ "angle": radians, "axis": [x,y,z] }` (axis-angle).
  Matrix = T(translation) · R(left_rotation) · S(scale) · R(right_rotation)
  (right-multiplied in this order: a point first undergoes `right_rotation`, then the scale,
  then `left_rotation`, then the translation).
- **matrix form**: list of 16 floats, read **row by row** (element i → row i÷4,
  column i mod 4), so translation at indices 3, 7, 11.

Composition along the tree, at load time (baking): each node receives the matrix of its
parent P (identity at the root); if the node has a `transformation` M, the matrix passed to
its children (and used by itself if it is a leaf) is **P · M**; otherwise P unchanged.
The final matrix of a leaf becomes the "local transformation" of its layer.

When rendering a layer (model frame, block units, 1 = 16 model pixels): first apply
the model's `display.gui` display transformation (mission B), **then** right-multiply
by the local transformation: pose = Display · Local. A vertex therefore undergoes
first the local transformation, then the display one.

Vanilla uses: beds (foot translated by [0,0,1]), banners, chests, Shulker boxes,
conduit (translation [0.5,0.5,0.5]), etc. — essentially to place special models
or composite ones; flat `item/generated` models do not have any.

#### 2.1 `model` — simple model

| Field | Type | Default |
|---|---|---|
| `model` | model id (`assets/<ns>/models/<path>.json`) | required |
| `transformation` | § 2.0 | none |
| `tints` | list of tint sources (§ 4) | empty list |

Evaluation: produces **one layer**.
1. If the stack glints ("foil") — rule: if the component `enchantment_glint_override` is
   present, its value decides; otherwise true iff the component `enchantments` is non-empty; special
   case for `minecraft:compass` (the only compass-type item): also true if the component
   `lodestone_tracker` is present. Have `enchantment_glint_override = true` by default:
   enchanted golden apple, bottle o' enchanting, written book, Nether star, enchanted book,
   End crystal, debug stick — the layer gets the `special` glint
   if the item is in the tag `#minecraft:compasses` or is `minecraft:clock`,
   otherwise `STANDARD`; and the rendering is marked "animated".
2. For each tint source, in list order: the color is computed
   (§ 4) and appended to the layer's tint list; the position in the list **is**
   the tint index (`tintindex` of the model faces).
3. Geometry: the referenced model is resolved (`parent` inheritance, textures) then baked without
   rotation; properties taken from the resolved model: `gui_light` ("side" = lit like a
   block, "front" = flat front lighting), `display` (per-context transformations), particle
   texture.
4. The model is marked "animated" if one of its textures is animated.
5. Load constraint: all faces must come from the **same atlas**, and this atlas
   must be the items one or the blocks one; otherwise load error.

#### 2.2 `composite`

| Field | Type | Default |
|---|---|---|
| `models` | list of item models | required |
| `transformation` | § 2.0 | none |

Evaluation: evaluates each sub-model in order and **stacks all their layers** (list order
= draw order). Simplifications at baking: empty list → `empty`; a single
element → that element directly (with the composed transformation).

#### 2.3 `condition`

| Field | Type | Default |
|---|---|---|
| `property` | boolean property id (§ 3) + its own fields, at the same level | required |
| `on_true` | item model | required |
| `on_false` | item model | required |
| `transformation` | § 2.0 | none |

Evaluation: computes the property (stack, world, living holder or nothing, seed, context) and
evaluates `on_true` or `on_false`.

#### 2.4 `select`

| Field | Type | Default |
|---|---|---|
| `property` | selection property id (§ 3.2) + its own fields at the same level | required |
| `cases` | list of `{ "when": value or non-empty list of values, "model": model }` | required |
| `fallback` | item model | **missing model** (§ 2.9) |
| `transformation` | § 2.0 | none |

Evaluation: computes the value; looks for a case whose `when` contains exactly this value;
otherwise `fallback`. An **absent** (null) value always falls to `fallback`. Validation at
load: `cases` non-empty, and the same value must not appear twice (in the same
`when` or in two cases) — otherwise the file is rejected (load error → the item
takes the missing model [consequence of the rejection: not verified in detail]). The format of the
`when` values depends on the property (§ 3.2).
Values tied to the data registries (trim material, dimension…): they are
reinterpreted in the registries of the current world when a world is present; without a world,
direct comparison. [detail of the "registry swapper": not verified, no effect in vanilla]

#### 2.5 `range_dispatch`

| Field | Type | Default |
|---|---|---|
| `property` | numeric property id (§ 3.3) + own fields | required |
| `scale` | float | `1.0` |
| `entries` | list of `{ "threshold": float, "model": model }` | required |
| `fallback` | item model | **missing model** |
| `transformation` | § 2.0 | none |

Evaluation:
1. v = property × `scale` (32-bit float).
2. The entries are sorted by increasing threshold on load (stable sort: at equal threshold, the
   file order is kept).
3. If v is NaN → `fallback`.
4. Otherwise take the entry of **highest index** whose threshold ≤ v (linear search:
   first entry whose threshold > v, minus one; beyond 16 entries, binary search —
   same result except for equal thresholds, where the binary search may return any of the
   equal ones [edge case, not verified in practice]).
5. If no entry has a threshold ≤ v (v < smallest threshold) → `fallback`.

#### 2.6 `special`

| Field | Type | Default |
|---|---|---|
| `base` | model id (used only for `gui_light`, `display` and particle) | required |
| `model` | object `{ "type": <special renderer>, ... }` | required |
| `transformation` | § 2.0 | none |

Evaluation: one layer; enchantment glint **always `STANDARD`** if the stack glints
(never `special`); no tints; the special renderer's argument is extracted from the stack;
display properties taken from `base`. If the special renderer cannot be built,
fall back to the missing model (with the composed transformation).
Special renderer types registered in 26.3: `bell`, `banner`, `book`, `conduit`, `chest`,
`copper_golem_statue`, `head`, `player_head`, `shulker_box`, `shield`, `trident`,
`decorated_pot`, `end_cube` (detail: outside mission A). Beds are **not** special:
`composite` of two `model` (head + foot translated by [0,0,1]).

#### 2.7 `empty`

No fields. Produces no layer (nothing is drawn).

#### 2.8 `bundle/selected_item`

No fields (a possible `transformation` is ignored: not read). Evaluation: if the bundle has
a selected item, **the whole resolution of § 1 is re-evaluated** for that inner stack (its
own `item_model`, its own definition, its own `oversized_in_gui`) with the same
context, world, holder and seed, and its layers are **appended** after the layers already
produced. Otherwise nothing. Used by the 17 bundles, combined in a `composite` with
`bundle/has_selected_item` (§ 3.1).

#### 2.9 Missing model

Used: identifier without a definition, `fallback` absent from `select`/`range_dispatch`, special
renderer impossible. One layer, geometry and properties of the "missing" block model (cube with
a checkerboard texture), without tint or glint, with the composed transformation of the node it
substitutes for. [exact form of the geometry: see mission B, not verified here]

#### 3. Properties

Conventions: "holder" = living entity passed to the evaluation (in GUI: the player, or nothing).
"Same stack" = **identity** of the stack object (the inventory slot and the stack in use/at the
cursor are the same object), not content equality. Column "hotbar" = value in a slot
of the local player's hotbar (context `gui`, holder = local player, world present).

The fields specific to a property are placed **at the same level** as `property` in the object
`condition`/`select`/`range_dispatch`.

#### 3.1 Boolean properties (`condition`)

Exhaustive list registered in 26.3:

| `property` | Fields (default) | Value | Hotbar / inventory |
|---|---|---|---|
| `custom_model_data` | `index` integer ≥ 0 (0) | true iff the component `custom_model_data` exists and its list `flags` has an element at `index` equal to true; absent/out of bounds → false | same |
| `using_item` | — | holder present AND it is using an item AND the stack in use is this stack | true on the slot of the hand that draws the bow/eats… (the bow is seen drawn in the hotbar); holder absent → false |
| `broken` | — | damageable stack AND damage ≥ max − 1 | same |
| `damaged` | — | damageable stack AND damage > 0 | same |
| `fishing_rod/cast` | — | holder is a player, has a fishing bobber cast, and the stack held in the fishing arm is this stack | true on the slot of the rod that is fishing |
| `has_component` | `component` (component type identifier, required), `ignore_default` (false) | `ignore_default` false: the component is present on the stack (including the item's default); true: present **and different** from the item's default value (added/modified) | same |
| `bundle/has_selected_item` | — | the bundle contents have a selected item (index ≠ −1) | true only when the player scrolls the selection in the bundle tooltip; the index is not transmitted over the network (decoded as −1) |
| `selected` | — | holder is the **local** player and the selected stack of its hotbar is this stack | true on the active slot of the hotbar; false for an observed player (spectator) |
| `carried` | — | holder is the local player and the stack held at the cursor in its menu is this stack | true only for the stack drawn under the cursor |
| `extended_view` | — | context = `gui` AND a Shift key is pressed | true in GUI if Shift is held |
| `keybind_down` | `keybind` (key name, e.g. `key.use`, required; unknown name = load error) | the key is currently pressed | same |
| `view_entity` | — | if a camera entity exists: holder == camera; otherwise holder == local player | true in the hotbar (except spectator on another entity); holder absent → false (if a camera exists) |
| `component` | `predicate` (component predicate type) + `value` (predicate argument) | the matching component predicate accepts the stack | same |

Predicate types for `component` (registered on the world side): `damage`, `potion_contents`,
`custom_data`, `container`, `bundle_contents`, `fireworks`, `writable_book_content`,
`written_book_content`, `trim`, `jukebox_playable`, `villager/variant`, plus possible generic
predicates [exhaustive list and semantics of each predicate: not verified here;
no vanilla item uses `component` as a condition].

Vanilla uses (exhaustive survey of the files): `using_item` (bow, crossbow, goat horn,
shield, trident), `has_component` (compass `lodestone_tracker`, wolf armor `dyed_color`),
`bundle/has_selected_item` (17 bundles), `fishing_rod/cast` (fishing rod), `broken` (elytra).
`damaged`, `selected`, `carried`, `extended_view`, `keybind_down`, `view_entity`,
`custom_model_data`, `component`: no vanilla use (resource packs only).

#### 3.2 Selection properties (`select`)

Exhaustive list:

| `property` | Fields | Computed value | Format of `when` | Hotbar / inventory |
|---|---|---|---|---|
| `custom_model_data` | `index` (0) | `strings[index]` of the component `custom_model_data`; absent → null | string | same |
| `main_hand` | — | main arm of the holder; holder absent → null | `left` / `right` | the player's arm; fake slot → null → `fallback` |
| `charge_type` | — | component `charged_projectiles` absent or empty → `none`; contains a firework rocket → `rocket`; otherwise `arrow` | `none` / `arrow` / `rocket` | same |
| `trim_material` | — | key of the material of the component `trim`; absent (or material without a key) → null | trim material identifier (`minecraft:iron`…) | same |
| `block_state` | `block_state_property` (string, required) | value (string) of that property in the component `block_state`; component or property absent → null | string (e.g. `"0"`, `"true"`) | same |
| `display_context` | — | display context | `none`, `thirdperson_lefthand`, `thirdperson_righthand`, `firstperson_lefthand`, `firstperson_righthand`, `head`, `gui`, `ground`, `fixed`, `on_shelf` | **`gui`** |
| `local_time` | `pattern` (required), `locale` (`""` = root locale), `time_zone` (optional; default = system time zone) | **real** time of the machine formatted according to `pattern` (date pattern syntax in the manner of Java / ICU date patterns), recomputed at most once per second (cached 1000 ms, state shared by all the stacks that use this node) | string | same (e.g. chest: `pattern` `MM-dd`, cases `12-24`,`12-25`,`12-26` → Christmas texture) |
| `context_entity_type` | — | entity type of the holder; absent → null | entity identifier | `minecraft:player`; fake slot → null |
| `context_dimension` | — | dimension of the world; no world → null | dimension identifier | current dimension |
| `component` | `component` (component type, required, non-transient) | value of the component on the stack (null if absent) | value encoded according to the component's codec | same |

Vanilla uses: `display_context` (26 items: 7 spears, trident, spyglass, 17 bundles), `trim_material`
(29 armors), `block_state` (light `level`, beehive/nest `honey_level`, 8 copper golem statues
`copper_golem_pose`, test block `mode`),
`charge_type` (crossbow), `context_dimension` (clock), `local_time` (chest, trapped chest).

#### 3.3 Numeric properties (`range_dispatch`)

Common signature: (stack, world or nothing, holder or nothing, seed) — **not** the display
context. Exhaustive list:

| `property` | Fields (default) | Formula | Hotbar / inventory |
|---|---|---|---|
| `custom_model_data` | `index` (0) | `floats[index]` of the component; absent → 0 | same |
| `bundle/fullness` | — | weight of the bundle contents (exact fraction converted to float): Σ (count × unit weight); unit weight = 1/(max stack size); nested bundle = weight of its contents + 1/16; non-empty stack with bees = 1; weight in error (overflow) → 1; without the component → 0 | same |
| `damage` | `normalize` (true) | d = damage clamped to [0, max] (0 if component absent), m = `max_damage` (0 if absent); normalized: clamp(d/m, 0, 1), otherwise clamp(d, 0, m). Beware: m = 0 with `normalize` → 0/0 = NaN → the clamp lets NaN through → `fallback` | same |
| `cooldown` | — | holder is a player: remaining cooldown fraction of the stack's cooldown group = clamp((end − currentTick)/(end − start), 0, 1) (no tick fraction); otherwise 0 | same |
| `time` | `wobble` (true), `source` (required: `daytime`, `moon_phase`, `random`) | § 5.1 | — |
| `compass` | `wobble` (true), `target` (required: `none`, `lodestone`, `spawn`, `recovery`) | § 5.2 | — |
| `crossbow/pull` | — | holder absent → 0; crossbow charged (`charged_projectiles` non-empty) → 0; otherwise (use duration of the stack − remaining ticks of the holder) / charge duration, where charge duration = ⌊1.25 s modified by the Quick Charge enchantment × 20⌋ ticks. **Does not check** that the stack is the one in use: if the holder is using something else, the value is computed anyway [to be reproduced as is] | the crossbow only calls this property under `using_item` |
| `use_cycle` | `period` (> 0, default 1) | holder present and stack in use = this stack: (remaining use ticks) mod `period`; otherwise 0 | same |
| `use_duration` | `remaining` (false) | holder present and stack in use = this stack: `remaining` true → remaining ticks; false → total use duration of the stack − remaining ticks (elapsed ticks); otherwise 0 | drawn bow visible in the hotbar (`scale` 0.05: thresholds 0.65 and 0.9) |
| `count` | `normalize` (true) | n = count, M = max stack size; normalized: clamp(n/M, 0, 1), otherwise clamp(n, 0, M) | same |

The values are 32-bit floats; then multiplied by the node's `scale`.

#### 4. Tint sources (`tints` of a `model` model)

#### 4.1 Application

- The order of the `tints` list gives the index: element k ↔ faces whose `tintindex` = k.
- The colors are computed on **every evaluation** (every frame) and stored in the layer.
- When drawing each face: if the face has no `tintindex` (−1) → vertex color
  0xFFFFFFFF (opaque white). Otherwise, if 0 ≤ `tintindex` < number of tints → the computed
  ARGB color, **as is** (alpha included); if the index exceeds the list → opaque white.
- This color is the vertex color; it multiplies the texture in the shader
  (component by component, alpha included) [multiplication in the shader: mission B/C].
  No multiplication by another base color on this path.
- Tints only apply to `model` models; `special`, missing model, `empty` have
  none.

#### 4.2 Color encoding in the JSON

Color fields (`value`, `default`): either a signed 32-bit integer interpreted as ARGB
(e.g. `-6265536` = 0xFFA06540), or a list of 3 floats [r,g,b] in [0,1], converted to
channel = ⌊f × 255⌋ with alpha 255. A positive integer without an alpha byte (e.g. `10511680`) has an
alpha of **zero**: only some sources force it opaque (table below).

#### 4.3 Exhaustive list of sources (26.3)

| `type` | Fields (default) | Color | Alpha forced to FF? |
|---|---|---|---|
| `constant` | `value` (required) | `value` | yes (on load) |
| `dye` | `default` (required) | component `dyed_color` present → its `rgb`; otherwise `default` | yes for the component; **no** for `default` (taken as is) |
| `grass` | `temperature` ∈ [0,1], `downfall` ∈ [0,1] (both required in the JSON) | color map `textures/colormap/grass.png` (256×256): rain' = downfall × temperature; x = ⌊(1 − temperature) × 255⌋; y = ⌊(1 − rain') × 255⌋; pixel (x, y) as ARGB; if y·256 + x is outside the image → 0xFFFF00FF | no (the map's pixel is already opaque) |
| `firework` | `default` (required) | component `firework_explosion`: 0 colors or absent → `default`; 1 color → it; n colors → truncated integer average of each channel (Σ/n), alpha FF | 1 color and average: yes; `default`: **no** |
| `potion` | `default` (required) | component `potion_contents` absent → `default`. Present: `custom_color` if it exists; otherwise weighted average of the **visible** effects (effects of the base potion then custom effects) with weight = amplifier + 1: channel = ⌊Σ(weight × effect_channel) / Σ weight⌋; no visible effect → `default` | yes (always) |
| `team` | `default` (required) | holder present, in a team that has a color → RGB of that team color; otherwise `default` | yes (always) |
| `custom_model_data` | `index` (0), `default` (required) | `colors[index]` of the component `custom_model_data` if it exists; otherwise `default` | yes (always) |

**There is no `map_color` source in 26.3** (the `map_color` component was removed;
`filled_map.json` is a `model` without a tint: the layer is drawn as is).

Useful vanilla values:
- `dye` default **−6265536 = 0xFFA06540** (leather items: 49 occurrences); dyed
  wolf armor (`wolf_armor_dyed`): `tints` = [constant −1 (white), dye `default` 0] — the default 0
  (transparent) is never used because this model is only chosen if `dyed_color` is present.
- `grass` (0.5; 1.0) → x = 127, y = ⌊(1 − 0.5) × 255⌋ = 127 → pixel measured (bench, PNG reading)
  **0x7CBD6B**, i.e. 0xFF7CBD6B. Items: grass block, short/tall grasses, ferns, bush,
  … (6 `grass` occurrences).
- `constant` −12012264 = 0xFF48B518 (oak, acacia, jungle, dark oak leaves, and vine),
  −8345771 = 0xFF80A755 (birch), −10380959 = 0xFF619961 (spruce), −9321636 = 0xFF71C35C (lily pad),
  −7158200 = 0xFF92C648 (mangrove); `grass` items: grass block, short grass, tall grass,
  fern, large fern, bush,
  −1 = opaque white (layer 0 of the firework star and of the dyed wolf armor).
- `potion` default −13083194 = 0xFF385DC6 (potion, splash potion, lingering potion, tipped arrow).
- `firework` default −7697782 = 0xFF8A8A8A (firework star, layer 1).

#### 5. Needle properties: `time` and `compass`

#### 5.1 Common rules (needle)

1. Holder absent → 0 (no animation). World absent but holder present → take the holder's world;
   still absent → 0.
2. A "wobbler" (oscillator) stores the state. **The state belongs to the baked definition node**,
   not to the stack: all the stacks that use the same definition (all clocks, e.g.,
   in the hotbar, in hand, in item frames) share the same state. It is only updated
   **once per game tick** (guard on the world's `game_time`) — except the version without
   oscillation, updated on every call.
3. Wobbler with `wobble` = true, of factor f, initial state (rot, delta) (0, 0):
   δ = positiveMod(target − rot + 0.5; 1) − 0.5; delta = (delta + 0.1·δ) × f;
   rot = positiveMod(rot + delta; 1). Result = rot.
   With `wobble` = false: result = target of the last call.
   positiveMod(a, b) = ((a mod b) + b) mod b in floats (remainder truncated C-style).

#### 5.2 `time`

f = 0.9. Target depending on `source`:
- `daytime`: value of the environment attribute `sun_angle` (degrees) at the holder's
  position, ÷ 360 (same attribute as the sun angle of the sky, already specified on the environment side).
- `moon_phase`: moon phase index (attribute `moon_phase` at the holder's position) ÷ 8.
- `random`: uniform draw [0,1) from a generator specific to the node, random seed (non-
  deterministic; every tick when the update happens).
Result = rotation of the wobbler. The slot seed plays no part.

Vanilla clock: `select` on `context_dimension`; case `minecraft:overworld` →
`range_dispatch` `time` source `daytime`, `scale` 64; fallback → the same with source `random`; 64
entries `clock_00`..`clock_63` at thresholds 0; 0.5; 1.5; … ; 62.5 then `clock_00` at 63.5
(back to 0 after the last half-step). In GUI without world/holder (the fake item) → 0 → `clock_00`.

#### 5.3 `compass`

f = 0.8 (two independent wobblers: "toward the target" and "no target").
Target (global position) depending on `target`:
- `none`: none; `lodestone`: target of the component `lodestone_tracker` (absent if the
  component or its target is missing); `spawn`: global respawn point of the client world
  (spawn point packet); `recovery`: last death location of the holder if it is a player.

Valid target ⇔ present AND same dimension as the holder's world AND distance² between the center
of the target block and the holder's position ≥ 1e-5.

- **Invalid target** (spins at random): every tick, the "no target" wobbler receives
  a uniform random target [0,1); result = positiveMod(rot + h(seed) / 2.1474836e9; 1),
  where h(seed) = seed × 1327217883 as a **signed 32-bit integer with overflow**, then
  converted to float. The slot seed (§ 1.2) therefore offsets each slot: this is the only
  vanilla property that depends on it.
- **Valid target**: a = atan2(zc − zp, xc − xp) / (2π) with (xc,zc) = center of the target block,
  (xp,zp) = holder's position (computed in double then converted to float);
  r = positiveMod(visualRotationY / 360; 1), where visualRotationY = **body rotation**
  (`body_yaw`) for a living entity.
  - Holder = local player and normal tick rate (no `/tick freeze`):
    "toward the target" wobbler updated with the target 0.5 − (r − 0.25); result =
    positiveMod(a + rot; 1).
  - Otherwise: positiveMod(0.5 − (r − 0.25 − a); 1) (without oscillation).

Vanilla compass: `condition` `has_component` `lodestone_tracker`; true → `range_dispatch`
`compass` target `lodestone`, false → `range_dispatch` `compass` target `spawn` (outside the
spawn point's dimension, the target is invalid → random rotation). `scale` 32;
33 entries: `compass_16` at threshold 0, then `compass_17`…`compass_31`, `compass_00`…`compass_15`
at thresholds 0.5; 1.5; … ; 30.5, and again `compass_16` at threshold 31.5 (value 0 = image 16,
needle toward the "top" of the icon). The `special` glint (§ 2.1) applies to compasses and the clock.

#### 6. Result of the evaluation: layers

The evaluation produces an **ordered list of layers** (cleared at the start). Each layer carries:
model quads (or special renderer + argument), tint list, glint type
(`none`/`standard`/`special`), block-light lighting (= `gui_light` "side"), display
transformation of the context (`display.gui` of the referenced model, or of `base`), local transformation
(§ 2.0), extent (set of distinct vertices, for the bounding box). Layers are
drawn in list order. The rendering is marked "animated" if a layer has a glint or
an animated texture (used by the GUI item cache, see hud.md § 3.4).

A "model identity" is accumulated during the evaluation (each node traversed, each tint
color, each glint type, each special argument): two stacks of the same
identity give the same image (cache key of the GUI item atlas).

#### 7. `oversized_in_gui` in detail

If true for the stack: compute the model's bounding box = envelope of all the
layer extents, each vertex transformed by (`gui` display · local) of its layer
(units: block). If ⌈widthX × 16⌉ ≤ 16 AND ⌈heightY × 16⌉ ≤ 16 → treated as normal.
Otherwise screen rectangle: x = slotX + ⌊min_x × 16⌋ + 8, y = slotY − ⌊max_y × 16⌋ + 8,
width = ⌈sizeX × 16⌉, height = ⌈sizeY × 16⌉; the item is then rendered separately (texture
dedicated per model identity) instead of the 16×16 atlas [render path: not followed, no vanilla
item concerned].

#### 8. Complement to hud.md § "enchantment glint"

When drawing a layer, each face picks its render type according to the layer's glint:
`none` → normal item render of the material; `standard` → "with glint" variant; `special` →
"special glint" variant, where the matrix used for the glint coordinates is a copy of the
pose of which **all coefficients** are multiplied by 0.5 in `gui` context (0.75 in first
person, unchanged elsewhere). The glint is therefore 2× finer on the clock and the compasses
in GUI. [Glint shader, scroll speed, color: mission B/C, not followed here.]

#### 9. Uncertain points

1. Rejection of an invalid `items/*.json` file (duplicate `when`, unknown property…):
   assumed → definition absent → missing model; not followed up to the loader.
2. Exact geometry of the missing model in item context (checkerboard cube vs. plate): not verified.
3. Binary search of `range_dispatch` (> 16 entries) with equal thresholds: the index
   returned among equal ones is not guaranteed; no vanilla case (the clock has 65 entries but
   distinct thresholds).
4. `local_time`: exact syntax of the pattern (ICU date patterns, like Java's), behaviour of an empty `locale`
   and of the default time zone: reproduced "Java-style", not tested.
5. `daytime`: the `sun_angle` value is read at the holder's position without a tick fraction
   (no intra-tick interpolation) — read this way in the call, not verified against a capture.
6. `component` predicates (condition): list and semantics not detailed.
7. Reinterpretation of `select` values in the world's registries ("registry swapper"):
   zero effect assumed for vanilla content.
8. Vertex color × texture multiplication (alpha included) in the item shader: assumed,
   to be confirmed by the rendering mission.
9. `crossbow/pull` does not test that the stack is the one in use: reproduced as read, zero
   visible effect in vanilla (guarded by `using_item`).




## Items in a GUI slot — geometry and rendering (mission B)

Source: decompiled 26.3 client, read in a clean room. Complements `docs/specs/hud.md` § 3.4.
Units: "model px" = 1/16 of a block; the JSON models are in model px (0..16), the baked quads are in blocks (0..1).

#### 1. Item model: parent chain and resolution

#### 1.1 Fields of a `models/**.json` file
| field | type | default if absent | role |
|---|---|---|---|
| `parent` | identifier | none (root) | parent model. `builtin/generated` = flat item generator (§ 2). |
| `elements` | list | absent (inherited) | cuboid geometry (§ 3). |
| `textures` | object `slot → value` | empty | value = `"#other_slot"` (reference) or sprite identifier, or object `{sprite, force_translucent (default false)}`. |
| `display` | object `context → transformation` | absent | per-context transformations (§ 4). |
| `gui_light` | `"front"` \| `"side"` | absent (inherited) | GUI lighting (§ 6). Any other string = load error. |
| `ambientocclusion` | boolean | absent (inherited), final default `true` | no effect in the GUI (no ambient occlusion for items). |

#### 1.2 Resolution along the chain (child → parents)
- **Geometry**: the first model of the chain (starting from the child) that has a geometry (`elements`, or the generator `builtin/generated`) provides it; otherwise empty geometry. The `elements` are not merged: the closest one replaces everything.
- **`gui_light`**: the first non-null value encountered; if none: **`side`** (global default). `item/generated` declares `front`; the generator `builtin/generated` also declares `front`; `block/block` declares `side`.
- **`ambientocclusion`**: first non-null value, default `true`.
- **`display`**: resolved **context by context**: for the GUI context take the `gui` transformation of the first model of the chain that has a `display` block containing the key `gui`; otherwise "no transformation" (§ 4.3). Two contexts of the same model can therefore come from different parents.
- **Textures**: the slot tables are stacked from the root toward the child; a slot defined closer to the child overwrites (a value overwrites a reference and vice versa). Then the `#x` references are resolved iteratively (repeated passes as long as a reference finds a resolved value; chains of references are therefore followed). A reference left without a target → warning, slot absent (missing texture at baking).
- `builtin/generated` adds the reference `particle → #layer0`.

#### 1.3 Reference models of the game
- `item/generated`: `parent: builtin/generated`, `gui_light: front`, `display` without a `gui` key (ground, head, thirdperson_righthand, firstperson_righthand, fixed only). **So in the GUI: no transformation** (§ 4.3).
- `item/handheld`: parent `item/generated`, only redefines the hands → GUI identical to `item/generated`.
- `block/block`: `gui_light: side`, `display.gui = {rotation [30, 225, 0], translation [0,0,0], scale [0.625, 0.625, 0.625]}`.
- No other `builtin/...` exists in this version: the "special" models (chests, banners, heads…) go through the `items/*.json` definitions of special type, not through a `builtin` parent **[outside mission B, see mission A]**.

#### 2. `builtin/generated`: extrusion of the layers

#### 2.1 Layers
- Slots read in the order `layer0`, `layer1`, `layer2`, `layer3`, `layer4` (**5 at most**). Stop at the **first absent slot** (a `layer2` without `layer1` is ignored).
- Each layer is baked independently (with its sprite and its number) then the quads are concatenated in layer order: all the quads of layer 0, then those of layer 1, etc.
- **Tint index of each quad = layer number** (0 for `layer0`, 1 for `layer1`…). No `shade_direction_override`, light emission 0.
- Transparency of the layer's material = transparency of the **whole** sprite (opaque / cutout / translucent, computed on the entire image at load), or translucent if `force_translucent`. It selects the render type (§ 5.4).

#### 2.2 Front and back faces
Box `from = (0, 0, 7.5)`, `to = (16, 16, 8.5)` (model px), i.e. a thickness of 1 px centered on z = 8.
- **South** face (+Z, z = 8.5/16 = 0.53125): UV `(0, 0) → (16, 16)` (whole image, top-left at top-left seen from +Z).
- **North** face (−Z, z = 7.5/16 = 0.46875): UV `(16, 0) → (0, 16)` (image flipped horizontally, so seen from behind it reads correctly as a consistent mirror).
- Vertices (after /16) in the order of the standard face template (see § 3.2): south = `(0,1,z1) (0,0,z1) (1,0,z1) (1,1,z1)` with UV `(0,0) (0,1) (1,1) (1,0)`; north = `(1,1,z0) (1,0,z0) (0,0,z0) (0,1,z0)` with UV `(1,0) (1,1) (0,1) (0,0)` (UV as a fraction of the sprite, converted to atlas by `u_atlas = u0 + (u1 − u0)·u`).
- These two faces are always emitted, whatever the sprite content (transparent pixels included: the alpha cutout eliminates them at render).

#### 2.3 Edge faces (extrusion)
Notation: sprite of `w × h` pixels (size of **one** image, not of the animated sheet), `sx = 16/w`, `sy = 16/h`. Pixel `(x, y)` with y toward the **bottom** of the image. A pixel is "transparent" if its alpha is **exactly 0**; any pixel outside the image is transparent.

1. Set of images to examine: non-animated sprite → the only image; animated sprite → **each distinct image** referenced by the animation (unique indices of `frames`, in order of first appearance). The edges are the **union** over all these images (an edge present in a single image is emitted for all; it is simply cut by the alpha in the others).
2. For each image and each **non-transparent** pixel `(x, y)`, four tests; each adds an edge `(type, x, y)` to a **set** (without duplicates):
   - **TOP** if the pixel `(x, y − 1)` is transparent;
   - **BOTTOM** if `(x, y + 1)` is transparent;
   - **LEFT** if `(x − 1, y)` is transparent;
   - **RIGHT** if `(x + 1, y)` is transparent.
3. For each edge (emission order: iteration of a hashed set, **unspecified**; no visible effect, the edges do not overlap), in model px:
   - `X0 = x·sx`, `X1 = (x + 1)·sx`, `Yh = 16 − y·sy` (top edge of the pixel), `Yb = 16 − (y + 1)·sy` (bottom edge).
   - TOP: horizontal plane `y = Yh`, from x `X0..X1`, z `7.5..8.5`, final normal **+Y**.
   - BOTTOM: horizontal plane `y = Yb`, x `X0..X1`, z `7.5..8.5`, final normal **−Y**.
   - LEFT: vertical plane `x = X0`, y `Yb..Yh`, z `7.5..8.5`, final normal **−X** (outward).
   - RIGHT: vertical plane `x = X1`, y `Yb..Yh`, z `7.5..8.5`, final normal **+X**.
   (Baking detail: LEFT is built with the "east" template and RIGHT with the "west" template, but with `from.y > to.y`; the vertex order is reversed, the recomputed normal points outward and the final template is "west" for LEFT, "east" for RIGHT. The result to reproduce is the one above: faces oriented outward.)
   - **UV** (in sprite px then ×s to go to 0..16, then /16): `u ∈ [x + 0.1, x + 0.9]·sx`; for TOP/BOTTOM `v` from `(y + 0.1)·sy` to `(y + 0.9)·sy`, for LEFT/RIGHT from `(y + 0.9)·sy` to `(y + 0.1)·sy`. In other words each edge samples **the inside of its own pixel** (0.1 px margin), so with nearest filtering it takes the solid color of that pixel (of the current animation image).
   - Tint index, material, render type: those of the layer.
4. All generated faces are "non-cullable" (no `cullface`).

#### 2.4 Useful cases
- 16×16 sprite: `sx = sy = 1`, edges of 1 model px. 32×32 sprite: edges of ½ model px (the outline follows the pixels of the high-resolution sprite).
- Non-square sprite (rare): `sx ≠ sy`, the geometry stays within 0..16 on both axes.
- In the GUI, for a flat item without rotation (`item/generated`), the edges are seen **edge-on** (orthographic front projection): they cover no pixel, only the south face is visible. They count for rotated transformations (hand, ground) and for the models that would rotate a flat item in the GUI via `display.gui`.

#### 3. Element models (blocks used as items)

The baking of the `elements` is that of the block models (`docs/specs/models.md`), with the **identity** model state (no variant rotation, no UV transformation). Useful reminders and differences specific to items:
- `from`/`to` clamped to [−16, 32] per component (error otherwise). Vertices = chosen corner /16 → coordinates 0..1.
- Face: `uv` absent → default UVs deduced from the box (south: `(from.x, 16−to.y, to.x, 16−from.y)`, north: `(16−to.x, 16−to.y, 16−from.x, 16−from.y)`, east: `(16−to.z, 16−to.y, 16−from.z, 16−from.y)`, west: `(from.z, 16−to.y, to.z, 16−from.y)`, up: `(from.x, from.z, to.x, to.z)`, down: `(from.x, 16−to.z, to.x, 16−from.z)`). Face `rotation` (0/90/180/270) = circular shift of the UV corner index; `tintindex` default **−1** (no tint).
- Template of the 4 vertices per face (min corner = `from`, max = `to`), with corner UV: vertex 0 → `(uMin, vMin)`, 1 → `(uMin, vMax)`, 2 → `(uMax, vMax)`, 3 → `(uMax, vMin)`:
  - down: (min,min,max) (min,min,min) (max,min,min) (max,min,max); up: (min,max,min) (min,max,max) (max,max,max) (max,max,min)
  - north: (max,max,min) (max,min,min) (min,min,min) (min,max,min); south: (min,max,max) (min,min,max) (max,min,max) (max,max,max)
  - west: (min,max,min) (min,min,min) (min,min,max) (min,max,max); east: (max,max,max) (max,min,max) (max,min,min) (max,max,min)
- Element rotation: `{origin (px, /16), axis+angle}` or form `{x, y, z}` (degrees, matrix `Rz(z)·Ry(y)·Rx(x)` — X applied first); `rescale` (default false) multiplies by `1/max(|components|)` of the image of each unit axis. Applied around the origin, before any other transformation.
- **Quad direction** (serves as the lighting normal, § 6): axis closest to the geometric normal `(p1−p0)×(p2−p0)` (dot product ≥ 0 and maximal); unrotated quads: reordering of the vertices according to the template of this direction. An element rotated by 22.5° therefore keeps an **axial** lighting normal.
- Item / block differences in the GUI: **all** faces are drawn (`cullface` is ignored, "cullable" faces included), no ambient occlusion, **no per-face directional shading** (the fixed factors 0.5/0.6/0.8 of blocks do not apply); `shade_direction_override` is ignored by the item rendering; `light_emission` (0..15) has no effect in the GUI (white lightmap, § 6.4).
- An item model can only reference **one** atlas: the items one or the blocks one (load error otherwise).
- Transparency of the material of each face: computed on the face's **UV rectangle** in the sprite (union over the images of an animated sprite), except `force_translucent` → translucent. It selects the render type (§ 5.4).

#### 4. `display.gui` transformation

#### 4.1 Fields (per display context)
| field | default | processing on load |
|---|---|---|
| `rotation` [x, y, z] | [0, 0, 0] | degrees, unbounded |
| `translation` [x, y, z] | [0, 0, 0] | **×1/16** then clamped to [−5, 5] blocks per component (i.e. ±80 in the JSON) |
| `scale` [x, y, z] | [1, 1, 1] | clamped to [−4, 4] per component |
Array of size ≠ 3 → error. Context names: `thirdperson_righthand`, `thirdperson_lefthand`, `firstperson_righthand`, `firstperson_lefthand`, `head`, `gui`, `ground`, `fixed`, `on_shelf`. (Left hand absent → copy of the right hand, at load of each file.)

#### 4.2 Application (GUI context: no left-hand mirror)
Model frame: unit block 0..1, Y up. Right composition of the current pose (the last matrix written applies first to the vertices):
`M_display = T(tx, ty, tz) · R · S(sx, sy, sz) · T(−0.5, −0.5, −0.5)`
with `R = Rx(rx) · Ry(ry) · Rz(rz)` (angles in radians; applied to a vector: Z first, then Y, then X). So for a vertex `v`: centering (−0.5), scale, rotation, translation.

#### 4.3 Absence of `gui`
If no model of the chain defines `display.gui`: `M_display = T(−0.5, −0.5, −0.5)` alone (centering). This is the case of `item/generated` and `item/handheld`.

#### 4.4 Local transformation of the item definition
The `items/*.json` definition of model type can carry `transformation` (mission A); it is composed **after**: `M_layer = M_display · M_local` (M_local applies first to the vertices, in the 0..1 frame of the model). Each rendering layer (composite model) has its own `M_display · M_local`.

#### 4.5 Reference values
- Flat item (`item/generated`): `M = T(−0.5)`; the front face is at z = +0.03125, the image fills exactly [−0.5, 0.5]².
- Block (`block/block`): `M = Rx(30°)·Ry(225°)·S(0.625)·T(−0.5)`.

#### 5. Rendering in the GUI item atlas

#### 5.1 Atlas (allocation, cache, re-render)
- Slot size `S = 16 · guiScale` physical pixels (current integer guiScale). GUI scale change → atlas destroyed and recreated.
- Atlas size (square): `N` = number of distinct model identities in the frame; `side = ceil(sqrt(N + floor(N/2)))` slots; size = power of two ≥ `side · S`, clamped to [512, max texture size]. Format RGBA8, plus a 32-bit float depth buffer of the same size. On creation: color (0,0,0,0), depth **0**.
- If the existing atlas has enough room for the union (occupied slots ∪ identities of the frame) → reused; otherwise free the slots occupied by identities absent from the frame (as many as needed); if impossible → new atlas of recomputed size (if the computed size is identical to the current one: warning "too many items", some are not drawn).
- **Cache key** = list of the identity elements accumulated while resolving the model: each model node traversed (selectors, conditions, composites, leaf model), then for a leaf model: the glint type if any, then **each computed tint color**. Two stacks that lead to the same model, same glint and same tints share the slot (the count and the durability bar are not in the slot).
- Allocation: first free slot in row-by-row order (x then y). A reassigned slot (already used before) is cleared (color 0, depth 0) before drawing; a new slot is not (already blank).
- **Animated**: a model is "animated" if one of its quads uses an animated sprite, **or if it has a glint** (§ 7) (element models as well as special models). Its slot is released at the end of the frame → redrawn every frame. The others stay cached as long as they are used.
- A slot already ready is not redrawn.

#### 5.2 Pose and projection (checks hud.md § 3.4)
Target frame: atlas pixels, x to the right, **y down**, z toward the viewer.
- Orthographic projection over the whole texture: x ∈ [0, size] → left/right, y ∈ [0, size] → top/bottom (y inverted), depth: z = +1000 → depth 1, z = −1000 → depth 0 (**reversed Z**: depth = z/2000 + 0.5 in [0,1] convention).
- Depth test: **greater or equal** passes, depth write enabled (all item types, translucent included). Buffer cleared to 0 → the fragment of greatest z (closest to the viewer) wins; on a tie, the **last drawn** wins.
- Back-face culling: enabled (default of the item pipelines).
- Scissor limited to the slot. "Model-view" matrix = identity.
- Complete pose of a vertex `v` (block coordinates 0..1 of the baked quad) for the slot (cx, cy):
  `p = T(cx·S + S/2, cy·S + S/2, 0) · Scale(S, −S, S) · M_display · M_local · v`
  (the slot is centered; `−S` on Y puts the top of the model back at the top of the image; the unit model fits in [−0.5, 0.5]³ before scaling).
- Quad normal: `n = N · d`, `d` = unit vector of the quad's direction (§ 3), `N` = accumulated normal matrix. For a uniform scale in absolute value (case of `Scale(S, −S, S)` and uniform `display.gui`), N receives only the **signs**: `diag(1, −1, 1)`; the rotations apply to it as is; a non-uniform scale multiplies N by `diag(1/sx, 1/sy, 1/sz)` and the normal is then renormalized. The normals are therefore expressed in the "pixels, y down" frame.

#### 5.3 Vertices
For each quad: 4 vertices, position `p`, atlas UV of the sprite (§ 2.2), color = tint (§ 6.3), normal `n` common to the 4 vertices, "full daylight" light coordinate (15, 15), no overlay (neutral overlay pixel, alpha 1).

#### 5.4 Render types and draw order
For each quad, the type depends on the transparency of its material and on the glint:
| transparency | without glint | standard glint | special glint |
|---|---|---|---|
| opaque / cutout | `item_cutout` | `item_cutout_glint` | `item_cutout_glint_special` |
| translucent | `item_translucent` | `item_translucent_glint` | `item_translucent_glint_special` |
(one variant per atlas: items or blocks; same shader.)
- All six types cull **back faces**, everywhere (interface, hand, dropped item, item frame, carried,
  display), in classic rendering as in improved transparency. Blocks carried by an entity
  (enderman, minecart, block display) go through the same types; the frame model through an opaque type shifted forward, the falling block and the piston through the terrain block types,
  all culled too.
- Cutout: **no blending**, raw RGBA write; fragment rejected if texture alpha < 0.1.
- Translucent: same rejection < 0.1, then blend `rgb = src.rgb·src.a + dst.rgb·(1 − src.a)`, `a = src.a + dst.a·(1 − src.a)` (on a (0,0,0,0) background the result is **premultiplied**).
- The quads of a rendering layer are split in two lists (unblended / blended), keeping the original order within each list. All the "solid" lists of the slot are drawn first (in the layers' submission order), then the translucent lists.
- Translucent: quads sorted by z of the **centroid** (average of the 4 vertices after pose), from smallest (farthest) to largest (stable sort).
- Quad order of a model: faces without `cullface` in the order elements × JSON faces, then faces with `cullface` grouped by direction in the order north, south, east, west, up, down. For a flat item: layer 0 (south, north, edges), layer 1 (…), etc. — since the south faces of the layers are at the **same z**, the upper layer covers the lower one everywhere it is not rejected (depth tie → the last wins).

#### 6. Lighting and vertex color

#### 6.1 Lights (checks hud.md § 3.4)
Two fixed directions `L0 = n(0.2, 1, −0.7)`, `L1 = n(−0.2, 1, 0.7)`, transformed once and for all:
- `gui_light: front` (flat items): `M = Ry(−π/8) · Rx(3π/4)` → **L0 = (−0.222519, −0.171499, 0.959726), L1 = (−0.215012, −0.971825, 0.096568)**.
- `gui_light: side` (blocks): `M = diag(1, −1, 1) · Ry(1.0821041) · Rx(3.2375858) · Ry(−π/8) · Rx(3π/4)` (each `Ryxz(a, b, 0)` = `Ry(a)·Rx(b)`) → **L0 = (−0.933439, −0.262695, −0.244300), L1 = (−0.103571, −0.976607, 0.188446)**.
(JOML's `R`: standard direct rotation, `M·v` applied to the column vector; bench computation by script, 6 decimals.)
Choice: from the **first layer** of the rendering stack (resolved `gui_light` of that layer's model: `side` → 3D, `front` → flat).

#### 6.2 Diffusion formula (item shader, per vertex)
`k = min(1, 0.6 · (max(0, L0·n) + max(0, L1·n)) + 0.4)`; `vertex_color.rgb = tint.rgb · k`, alpha = tint alpha. `n` = normal of § 5.2 (not renormalized in the shader). No other shading (no per-face factors, no AO). Resulting values (bench, script):
- Flat item, front face (n = (0,0,1)): `k = min(1, 0.6·(0.959726 + 0.096568) + 0.4) = 1` → **full brightness**; back face culled; edges invisible (edge-on).
- Block `block/block` (rotation 30/225): visible faces up `k = 1.0`, east (**left** side on screen) `k = 0.650537` (≈ 165.9/255), north (**right** side) `k = 0.4` (ambient only, 102/255). [to be compared with a capture of the official client]

#### 6.3 Vertex color (tint)
- Quad with tint index `t ≥ 0`: color = `tints[t]` if `t <` number of tints computed for this layer, otherwise opaque white. `t = −1` → opaque white. The tints (ARGB, alpha included) are supplied by the item definition (mission A).
- Fragment: `c = texel · lit_vertex_color` (the alpha rejection < 0.1 applies to the **texel alone**, before multiplication); neutral overlay; lightmap = 1×1 white texture in the GUI (factor 1); no fog in the GUI; color modulator = 1.

#### 6.4 Sampling
Item atlas and block atlas: **nearest, no mipmap, clamped edges** (for GUI items, even when the block is reduced to 0.625: no mip, aliasing possible at guiScale 1).

#### 7. Enchantment glint

#### 7.1 When
- The stack has a glint if the component `enchantment_glint_override` is present: its boolean value decides (true forces, false forbids). Otherwise: glint if the component `enchantments` is non-empty; for compasses (`compass`), also if `lodestone_tracker` is present. Several items have `enchantment_glint_override: true` by default in their prototype (enchanted golden apple, bottle o' enchanting, written book, enchanted book, Nether star, End crystal, debug stick — values transmitted by the server with the components).
- Type: **special** if the item is in the tag `#compasses` or is `clock`; **standard** otherwise. The type is part of the cache key and makes the slot animated (redrawn every frame).
- Special models (chests, heads…): glint always of standard type (slot animated too), only a "glint" boolean is passed to their rendering engine, which handles its own render type [out of scope].

#### 7.2 Where
**In the same pass as the item, in the same shader** (no more second pass): each quad is drawn with the matching "glint" type, which adds a second sampler `textures/misc/enchanted_glint_item.png` (128×128, `.mcmeta` `blur: true` → **linear filtering, repeat**).

#### 7.3 Glint coordinates
- Texture matrix (recomputed on every render-type preparation, therefore every frame): `ms = floor(t_ms · speed · 8)` with `t_ms` = monotonic clock in milliseconds, `speed` = option "glint speed" (default **0.5**, 0..1); `o0 = (ms mod 110000)/110000`, `o1 = (ms mod 30000)/30000`;
  `TM = T(−o0, o1, 0) · Rz(π/18) · Scale(8)` (applied to `(u, v, 0, 1)`: scale, then 10° rotation, then translation).
- Standard: `glint_uv = TM · (u_atlas, v_atlas)` — these are the quad's **atlas UVs** (0..1 over the whole item/block atlas), so the pattern depends on the sprite's place in the atlas. With the default speed: scrolling in u with a period of 27.5 s, in v of 7.5 s.
- Special (compasses, clock): UV3 computed per vertex from the local position: `q = 2 · v_local` in the GUI (×0.75 in first-person view, ×1 elsewhere), then `q ← Rdir · Rx(−π/2) · Ry(π) · q` (Ry applied first) where `Rdir` depends on the approximate direction of the local normal (up: identity; down: Rx(π); south: Rx(π/2); north: Rx(π/2)·Ry(0)·Rz(π); west: Rx(π/2)·Rz(π/2); east: Rx(π/2)·Rz(−π/2)), and `UV3 = (−q.x, −q.y) · 0.0078125`. For the front face of a compass in the GUI this gives `UV3 = (x/64, −y/64)` (x, y ∈ [0,1] of the model). Then `glint_uv = TM · UV3`. Here `v_local` = position of the vertex in the 0..1 frame of the baked quad (before `M_display`) [not verified: no homogeneous correction — the pose matrix is multiplied component by component by 0.5 including w, then inverted and applied without division by w, which indeed gives 2·v_local].

#### 7.4 Color (fragment)
After `c = texel · vertex_color` (and alpha rejection < 0.1):
1. `c.a = max(c.a, A)` with `A` = option "glint strength" (default **0.75**, 0..1);
2. `g = A · glint_texture(glint_uv)` (all 4 components);
3. `c.rgb += g.rgb²` (component by component; equivalent of the old additive blend `src·src + dst`), written in RGBA8 so clamped to 1.
The glint is **not** lit (added after the lighting) and only touches the non-rejected fragments of the item.

#### 8. Copy of the slot to the screen
- GUI rectangle `(x, y) → (x + 16, y + 16)` transformed by the current 2D pose (including the pickup animation, hud.md § 3.2), possible screen scissor.
- UV = the slot: `u ∈ [cx·S, (cx+1)·S]/size`, top of the slot at the top of the rectangle (in the original's OpenGL convention, `v0 = 1 − cy·S/size`, `v1 = v0 − S/size`; with a top-left origin: `v ∈ [cy·S, (cy+1)·S]/size`).
- **Nearest** sampling, repeat; white vertex color; **premultiplied** blend: `rgb = src.rgb + dst.rgb·(1 − src.a)`, `a = src.a + dst.a·(1 − src.a)`.
- At integer GUI scale without 2D pose, the copy is texel for pixel.
- Consistency remark: the "cutout" quads write a **non-premultiplied** color; this is harmless because a sprite classified "cutout" only has alphas of 0 or 255 (any pixel of intermediate alpha classifies the sprite translucent). Exception: glint on a translucent item, where the alpha is raised to ≥ A and the color blended normally.
- Oversized items (`oversized_in_gui` of the definition and bounding box > 16 px after `M_display`): dedicated texture, out of scope.

#### 9. Order of operations (recap for a stack in a slot)
1. Resolve the item definition in the GUI context → layers (baked quads, `gui_light`, `M_display` gui, `M_local`, tints, glint type) and identity key.
2. Get/allocate the atlas slot; if new, reassigned or animated: clear if reassigned, choose the lights from the first layer, submit each layer with `T(center)·Scale(S,−S,S)·M_display·M_local`.
3. Draw: solids (no blending, depth ≥, back faces culled), then translucents sorted by increasing z; per-vertex lighting § 6.2, tint § 6.3, glint § 7.4.
4. Copy the slot into the 16×16 GUI rectangle (premultiplied, nearest), then the decorations (hud.md § 3.5).

### Uncertain points
- Brightness of the block faces (§ 6.2: 1.0 / 0.6505 / 0.4) computed from the constants; not yet compared with a capture of the official client (to be measured on a plain gray block).
- Exact order of the edges in the extrusion (hashed set): assumed with no visible effect.
- UV3 of the special glint: the reduction "pose ×0.5 component by component then inverted = 2·local position" relies on the assumption that the position transformation ignores w (behaviour of the matrix library) [not verified in the library].
- Maximum texture size of the device and "too many items" case: fallback behaviour only skimmed.
- Interpolation of animated sprites (`interpolate`) in the item atlas: handled on the texture atlas side (outside this document); the slot is simply redrawn every frame.
- The choice of lights is made on the first layer only: a composite model mixing `front` and `side` uses that of the first [verified in the code, effect not observed].



## Items in an interface slot — mission C: special models, default components, display name

Source: official 26.3 client and server (protocol 777), read in a clean room on 2026-10-12.
Everything is described in our own words; the JSON field and identifier names are those of the
public resource format.

#### 1. `minecraft:special` models

#### 1.1 The `special` node in an item definition

A node of type `minecraft:special` in `assets/<ns>/items/<item>.json` carries three fields:

| Field | Required | Default | Role |
|---|---|---|---|
| `base` | yes | — | Identifier of a classic block/item model (`models/…json`). **No** geometry of it is drawn; only the following is taken from it: display transformations (`display.gui`, etc., resolved by walking up `parent`), `gui_light` lighting mode (`front` = flat lighting, otherwise `side` = 3D block lighting), `particle` texture. |
| `transformation` | no | identity | Added local transformation (see 1.2). |
| `model` | yes | — | Object `{ "type": "<special type>", …parameters }`; the type chooses the drawer (1.4). |

Note: **every** item model node (`select`, `condition`, `composite`, `model`…) can also
carry a `transformation` field; they accumulate down the tree (1.2). Vanilla example:
`shield.json` and the `fallback` branch of `trident.json` carry `scale [1,-1,-1]` on the
`condition` node, not on the `special` node.

If the special type cannot be built (e.g. a head with an unknown `kind`), the node is replaced
by the "missing" model (purple/black checkerboard) with the same transformation.

#### 1.2 Transformations: JSON form and composition order

JSON form of a `transformation` (two accepted forms):
- **Decomposed**: object `{translation:[x,y,z], left_rotation:Q, scale:[x,y,z], right_rotation:Q}`.
  The four fields are required in this form. A quaternion `Q` is written either
  `[x, y, z, w]` or `{ "angle": <radians>, "axis": [x,y,z] }`.
- **Matrix**: list of 16 floats, read **row by row** (element i → row i/4, column i%4).

Matrix of a decomposed form (column frame, vector on the right):
`M = T(translation) · R(left_rotation) · S(scale) · R(right_rotation)`
(the point first undergoes `right_rotation`, then the scale, then `left_rotation`, then the translation).

Composition along the tree: each node receives the matrix `P` of its parent (identity at the
root) and computes `P' = P · M_node` if the node has a `transformation`, otherwise `P' = P`. The
final matrix of a `special` node is therefore `M_local = M_root · … · M_condition · M_special`.

Order when drawing a special layer (pose stack, item model frame, units = blocks):
1. current pose of the slot (GUI placement, mission A);
2. `display.<context>` display transformation of the `base` model (for the slot: `gui`), applied
   as for an ordinary item model (mission A);
3. right multiplication by `M_local`;
4. the special drawer draws its entity model in this frame (coordinates of the entity-model
   parts in 1/16 block, Y axis pointing down — hence the negative `scale`s or rotations of
   π around X in the definitions).

The extents of a special layer = vertices of the entity model's boxes at rest,
passed through 2. then 3.; it feeds the `oversized_in_gui` rule (mission A) [exact use
not followed here].

#### 1.3 Enchantment glint on special models

If the stack "glints" (rule 2.5), the layer receives the **standard** glint type and the slot is
marked animated. But the glint is **drawn by only two types**: `shield` and `trident`. All
the other types ignore the flag: an enchanted chest, head, banner (or one with
`enchantment_glint_override: true`) is shown **without a glint** in a slot.
- `trident`: if glint, the same model drawn with the render type "solid entity + glint".
- `shield`: see 1.4.10.

#### 1.4 The 13 registered types

Exhaustive list of the client registry (`minecraft:` implicit): `bell`, `banner`, `book`, `conduit`,
`chest`, `copper_golem_statue`, `head`, `player_head`, `shulker_box`, `shield`, `trident`,
`decorated_pot`, `end_cube`.

**No special type for** beds (`composite` definition of two block models
`block/<color>_bed_head` and `block/<color>_bed_foot`, the foot shifted by `translation [0,0,1]`),
signs and hanging signs (ordinary models `item/<wood>_sign` / `item/<wood>_hanging_sign`), nor the bundle (`minecraft:bundle/selected_item` is a distinct item model type, not
a special one).

Notation: "layer" = entity model location `minecraft:<name>#main` unless indicated;
"atlas X / sprite Y" = sprite `Y` of the atlas `textures/atlas/X.png` (source file
`textures/Y.png`). Tint `-1` = white (no tint).

Table of dye colors (texture diffuse color, alpha forced to 255), used by
banners and shields:

| color | RGB | color | RGB |
|---|---|---|---|
| white | F9FFFE | gray | 474F52 |
| orange | F9801D | light_gray | 9D9D97 |
| magenta | C74EBD | cyan | 169C9C |
| light_blue | 3AB3DA | purple | 8932B8 |
| yellow | FED83D | blue | 3C44AA |
| lime | 80C71F | brown | 835432 |
| pink | F38BAA | green | 5E7C16 |
| red | B02E26 | black | 1D1D21 |

#### 1.4.1 `bell`
- Fields: none.
- Draws: layer `bell`, "at rest" state (swing 0). Texture: block atlas, sprite
  `entity/bell/bell_body`. "Solid entity" render.
- Vanilla items: **none** (the bell in hand uses a classic model). Serves the blocks
  (the built-in block models).

#### 1.4.2 `banner`
- Fields: `color` (required, dye name); `attachment` (`ground` by default, or `wall`).
- Argument taken from the stack: component `banner_patterns` (empty list if absent).
- Draws (local frame, no internal transformation):
  1. layer `standing_banner` (or `wall_banner`) = pole + bar, then layer `standing_banner#flag`
     (or `wall_banner#flag`) at rest (phase 0), both with atlas `banner_patterns` / sprite
     `entity/banner/banner_base`, solid render, no tint;
  2. on the flag only: pattern layer `entity/banner/base` tinted by `color` (order 1);
  3. for each layer `i` of `banner_patterns` (at most 16, in order): sprite
     `entity/banner/<pattern asset_id>` (atlas `banner_patterns`) tinted by the color of the layer,
     order `i+2`, "banner pattern" render (translucent).
- Extents: standing pole + flag at rest.
- Vanilla items: the 16 `<color>_banner`, `base: item/template_banner`, transformation
  `translation [0.5,0,0.5]`, `scale [0.6666667,-0.6666667,-0.6666667]`, identity rotations.
- `item/template_banner`: `gui` = rotation [30,20,0], translation [0,-3.25,0], scale 0.5325;
  `gui_light: front`.

#### 1.4.3 `book`
- Fields (all required): `open_angle` (degrees, converted to radians), `page1`, `page2`
  (page positions, floats).
- Draws: layer `book` in this state. Texture: block atlas, sprite
  `entity/enchantment/enchanting_table_book`. Solid render.
- Vanilla items: **none**.

#### 1.4.4 `chest`
- Fields: `texture` (required, short identifier, e.g. `minecraft:normal`); `openness`
  (float, default 0 = closed, 1 = open); `chest_type` (`single` by default, `left`, `right`).
- Draws: layer `chest` (resp. `double_chest_left`, `double_chest_right`), lid open according to
  `openness`. Texture: atlas `chest`, sprite `entity/chest/<texture>` (e.g. `entity/chest/normal`).
  "Cutout entity with back-face culling" render.
- Vanilla items (all `single`, `openness` 0, no transformation):

| item(s) | `base` | `texture` |
|---|---|---|
| `chest` | `item/chest` | `normal`; `christmas` if the local date is 24, 25 or 26 December (`select` on `minecraft:local_time`, pattern `MM-dd`) |
| `trapped_chest` | `item/trapped_chest` | `trapped`; `christmas` on the same dates |
| `ender_chest` | `item/ender_chest` | `ender` |
| `copper_chest`, `waxed_copper_chest` | `item/copper_chest` | `copper` |
| `exposed_…`, `waxed_exposed_…` | `item/exposed_copper_chest` | `copper_exposed` |
| `weathered_…`, `waxed_weathered_…` | `item/weathered_copper_chest` | `copper_weathered` |
| `oxidized_…`, `waxed_oxidized_…` | `item/oxidized_copper_chest` | `copper_oxidized` |

- Common base `item/template_chest`: `gui` = rotation [30,45,0], translation 0, scale 0.625;
  `gui_light` absent (→ `side`). Particle depends on the item (oak planks, obsidian, copper block…).

#### 1.4.5 `conduit`
- Fields: none.
- Draws: layer `conduit#shell` alone (the shell, not the cage nor the eye). Texture: block atlas,
  sprite `entity/conduit/base`. Solid render.
- Vanilla item: `conduit`, `base: item/conduit`, transformation `translation [0.5,0.5,0.5]`.
- `item/conduit`: `gui` = rotation [30,45,0], scale 1; `gui_light` → `side`.

#### 1.4.6 `copper_golem_statue`
- Fields (required): `texture` (full file path, e.g.
  `minecraft:textures/entity/copper_golem/copper_golem.png`, **not** an atlas sprite); `pose`
  (`standing`, `sitting`, `running`, `star`).
- Draws: layer depending on the pose: `copper_golem`, `copper_golem_sitting`, `copper_golem_running`,
  `copper_golem_star`. Before drawing, the model root is set to `y = 0` and rotated by π around
  Z (flip). "Cutout entity" render.
- Vanilla items: `copper_golem_statue`, `exposed_…`, `weathered_…`, `oxidized_…` and their
  `waxed_…` versions (8 items). Each is a `select` on the block-state property `copper_golem_pose`
  read from the `block_state` component of the stack (`sitting`, `running`, `star`; fallback `standing`).
  Texture: `copper_golem.png`, `copper_golem_exposed.png`, `copper_golem_weathered.png`,
  `copper_golem_oxidized.png`; a waxed version shares the texture of its unwaxed version.
- `base: item/template_copper_golem_statue`: `gui` = rotation [30,45,180], translation [0,2,0],
  scale 0.55; `gui_light` → `side`. No transformation of its own.

#### 1.4.7 `decorated_pot`
- Fields: none.
- Argument taken from the stack: component `pot_decorations` (4 empty faces if absent).
- Draws: layer `decorated_pot_base` (parts neck, top, bottom) with atlas `decorated_pot` /
  sprite `entity/decorated_pot/decorated_pot_base`; then layer `decorated_pot_sides` (4 faces
  front, back, left, right). For each face: if the decoration item of that face carries
  the component `provides_pottery_pattern`, sprite `entity/decorated_pot/<pattern asset_id>`
  (atlas `decorated_pot`); otherwise (brick or empty face) sprite `entity/decorated_pot/decorated_pot_side`.
  All in solid render. No internal transformation (the rotation according to direction and
  the wobble only concern the placed block).
- Extents: neck, top and bottom only.
- Vanilla item: `decorated_pot`, `base: item/decorated_pot` (`gui` = rotation [30,45,0],
  scale 0.6; `gui_light: front`), no transformation of its own.

#### 1.4.8 `end_cube`
- Required field: `effect` = `portal` or `gateway`.
- Draws the cube with the End portal effect (render `end_portal` or `end_gateway`).
- Vanilla items: **none** (serves the portal and gateway blocks).

#### 1.4.9 `head`
- Fields: `kind` (required: `skeleton`, `wither_skeleton`, `player`, `zombie`, `creeper`,
  `dragon`, `piglin`); `texture` (optional: short identifier `ns:path` transformed into the file
  `ns:textures/entity/<path>.png`); `animation` (float, default 0; animation position —
  dragon jaw, piglin ears).
- Layers and default textures ("cutout entity with Z offset" render, direct file texture,
  outside any atlas):

| `kind` | layer | default texture |
|---|---|---|
| skeleton | `skeleton_skull` | `textures/entity/skeleton/skeleton.png` |
| wither_skeleton | `wither_skeleton_skull` | `textures/entity/skeleton/wither_skeleton.png` |
| zombie | `zombie_head` | `textures/entity/zombie/zombie.png` |
| creeper | `creeper_head` | `textures/entity/creeper/creeper.png` |
| dragon | `dragon_skull` (dragon head model) | `textures/entity/enderdragon/dragon.png` |
| piglin | `piglin_head` (piglin head model) | `textures/entity/piglin/piglin.png` |
| player | `player_head` | default skin (slim/steve, see 1.4.11) |

- A single draw pass (no overlaid layer); the presence of the "hat" in the `player_head` layer
  is a matter of geometry [not verified here, separate spec].
- Vanilla items (all with transformation `translation [0.5,0,0.5]`, `left_rotation [1,0,0,0]`
  = rotation of π around X, scale 1): `skeleton_skull`, `wither_skeleton_skull`, `zombie_head`,
  `creeper_head`, `piglin_head` (`base: item/template_skull`), `dragon_head`
  (`base: item/dragon_head`).
- `item/template_skull`: `gui` = rotation [30,45,0], translation [0,3,0], scale 1; `gui_light` →
  `side`. `item/dragon_head`: `gui` = rotation [30,45,0], translation [-2,2,0], scale 0.6.

#### 1.4.10 `shield`
- Fields: none.
- Argument taken from the stack: its components (`banner_patterns`, `base_color`).
- Draws the layer `shield` (solid render):
  - "has patterns" ⇔ `banner_patterns` non-empty **or** `base_color` present;
  - base sprite: atlas `shield_patterns`, `entity/shield/shield_base` if patterns, otherwise
    `entity/shield/shield_base_nopattern`;
  - without patterns: drawing of the model with this sprite; if glint, the same drawing is done with the render
    type "solid entity + glint" (a single pass);
  - with patterns: normal drawing of the base sprite, then pattern layers like the banner (1.4.2, 2.–3.)
    applied to the **whole** shield model, with sprites `entity/shield/base` then
    `entity/shield/<asset_id>` (atlas `shield_patterns`), base color = `base_color` or white
    by default; then if glint, a "patterned shield glint" pass at order
    `layer_count + 2`.
- Vanilla item: `shield` — `condition` on `minecraft:using_item` (blocking):
  `base: item/shield_blocking` if true, `item/shield` otherwise; transformation `scale [1,-1,-1]` on
  the `condition` node. In an inventory slot, `using_item` is false → `item/shield`.
- `item/shield`: `gui` = rotation [15,-25,-5], translation [2,3,0], scale 0.65; `gui_light: front`.

#### 1.4.11 `player_head`
- Fields: none.
- Argument taken from the stack: component `profile`. Absent → default render.
- Draws: layer `player_head`, animation 0, "**translucent** entity" render with the profile's
  skin texture:
  - no profile: the game's default skin = `textures/entity/player/slim/steve.png` (index 6 of the
    list alex, ari, efe, kai, makena, noor, steve, sunny, zuri as slim, then the same as wide);
  - profile present but skin not loaded yet: default skin **determined by the UUID** of the
    profile: index `positive_mod(h, 18)` in the list of 18 skins (9 slim then 9 wide), with
    `x = hi XOR lo` (hi/lo = 64 high/low bits of the UUID) and
    `h = (int32)(x >> 32) XOR (int32)x` (standard hash of a Java UUID, signed integer); the
    profile's possible `skin_patch` is passed along [exact effect not followed];
  - skin loaded: downloaded texture (network/profiles spec separate).
- Vanilla item: `player_head`, `base: item/template_skull`, same transformation as `head`.

#### 1.4.12 `shulker_box`
- Fields: `texture` (required, short identifier, e.g. `minecraft:shulker_red`); `openness`
  (default 0).
- Draws: layer `shulker_box`, lid according to `openness`. Texture: atlas `shulker_boxes`,
  sprite `entity/shulker/<texture>`. "Cutout entity" render.
- Vanilla items: `shulker_box` (`texture: shulker`) and the 16 `<color>_shulker_box`
  (`texture: shulker_<color>`), `base: item/<item>` (parent `item/template_shulker_box`).
  Transformation: `translation [0.5,1.4995,0.5]`, `left_rotation [1,0,0,0]` (π around X),
  `scale [0.9995,0.9995,0.9995]`.
- `item/template_shulker_box`: `gui` = rotation [30,45,0], scale 0.625; `gui_light` → `side`.

#### 1.4.13 `trident`
- Fields: none.
- Draws: layer `trident`, file texture `textures/entity/trident/trident.png`, solid render
  (or solid + glint if the stack glints).
- Vanilla item: `trident`, but **not in an interface slot**: `trident.json` is a `select` on
  `minecraft:display_context` whose case `gui`, `ground`, `fixed`, `on_shelf` gives the ordinary flat model
  `item/trident` (`item/generated`, layer `item/trident`). The special one is only used in hand
  (`base: item/trident_in_hand` or `item/trident_throwing` depending on `using_item`, `scale [1,-1,-1]`
  on the `condition` node).

#### 1.5 Recap of the vanilla items with a special model in a slot

91 `special` nodes spread over 63 item files (script count): 16 banners, 13 chest nodes
(9 items), conduit, 32 statue nodes (8 items), decorated pot, 6 heads, player head,
2 shield nodes, 17 Shulker boxes, 2 trident nodes (outside a slot). Types `bell`, `book`,
`end_cube`: no item.

#### 2. Default components of items

#### 2.1 Where the client gets them

- The item registry is **built in** (hard-coded in the client, not synchronized over the
  network). Each item receives there at initialization a component "prototype": those set by
  its constructor/properties, then systematically `item_name` = translatable text of its
  description key (`item.minecraft.<id>` or `block.minecraft.<id>`, 3.3) and `item_model` = its
  identifier (unless explicitly overridden), plus components common to all (`max_stack_size` 64,
  `rarity` common, `enchantments` empty, `lore` empty, `repair_cost` 0, `attribute_modifiers`,
  `tooltip_display`, `break_sound`, `use_effects`, `attack_animation`, `interact_animation`…).
- On the network, a stack is encoded `count (varint)`; if count > 0: `item id (varint,
  protocol id of the registry minecraft:item)` then a component **patch** relative to the
  prototype (additions/replacements, and explicit removals). Effective components of a stack =
  the item's prototype, then the patch applied (a removal removes the component even if it is in
  the prototype). The client must therefore know the prototype of each of the 1658 items.

#### 2.2 Extraction outside the game: "Default Components" report of the official server

The server's data generator produces one file per item containing **exactly** the
prototype (same initialization code as the client).

Command (the server jar is a "bundler"; the system property chooses the entry point):

```
java -DbundlerMainClass=<entry point of the data generator, see crates/rc-data/src/prepare.rs> -jar server.jar --reports --output <folder>
```

(`--all` also includes the reports; `--output` defaults to `generated`.)

**Output, outside the repository**: `%LOCALAPPDATA%\rustcraft-re\26.3\reports\reports\`, which
notably contains:
- `minecraft/components/item/<id>.json`: 1658 files = number of entries of the registry
  `minecraft:item` in `registries.json` (from `air`, id 0, to the end);
- `registries.json`: protocol id of each item (`entries.<id>.protocol_id`);
- `blocks.json`, `packets.json`, `commands.json`, `datapack.json`.

Format of a components file: `{ "components": { "minecraft:<component>": <JSON value>, … } }`,
values in the public JSON format of components (same encoding as `/give` in JSON). Only the
components present in the prototype appear; an item without components would have no file
(no case in 26.3).

Example (in our words): `diamond_sword` → `max_damage` 1561, `damage` 0, `max_stack_size` 1,
`rarity` common, `item_name` = `{translate: item.minecraft.diamond_sword}`, `item_model`
`minecraft:diamond_sword`, `enchantments` empty, `enchantable` 10, tool, attribute modifiers…

26.3 statistics (bench, counting over the reports):
- `max_stack_size`: 1342 items at 64, 68 at 16, 248 at 1; present on all 1658.
- `max_damage` (and `damage` 0): 84 items.
- `rarity`: 1543 common, 78 uncommon, 18 rare, 19 epic.
- `enchantment_glint_override: true` by default: `debug_stick`, `enchanted_book`,
  `enchanted_golden_apple`, `end_crystal`, `experience_bottle`, `nether_star`, `written_book`.
- `item_name` and `item_model`: present on all 1658.

Proposal for Rustcraft: extraction script (outside the repository for the raw data, or a generated
table committed if the license allows — to be decided by the user) that reads these 1658 JSONs and
produces a compact table per protocol id: `max_stack_size`, `max_damage`, `rarity`,
`item_name` (translation key), `item_model`, `enchantment_glint_override`, `enchantments` non-
empty (never the case by default), plus the components useful for slot rendering
(`dyed_color`, `potion_contents`, `banner_patterns`, `base_color`, `pot_decorations`, `block_state`,
`profile`, `charged_projectiles`, `bundle_contents`…) — the rest can be kept as raw JSON.

#### 2.3 What the report does not say (behaviour tied to the item's Java class)

The prototype only contains data; some rules depend on the item's class and
must be hard-coded on the client side:

| item(s) | rule |
|---|---|
| `compass` | glint if `lodestone_tracker` present (in addition to enchantments); special name (3.2) |
| `potion`, `splash_potion`, `lingering_potion`, `tipped_arrow` | name taken from `potion_contents` (3.2) |
| `shield` | name according to `base_color` (3.2) |
| `player_head` | name according to `profile.name` (3.2) |
| `air` | name read from the prototype, never from the stack [no practical effect] |

No other item class overrides the name or the glint (exhaustive search of the overrides
in all of the game's code).

#### 2.4 Effective rarity of a stack

`base = rarity` of the stack (component, default `common`). If the stack is **enchanted** (component
`enchantments` non-empty; `stored_enchantments` does not count): common → rare, uncommon → rare,
rare → epic, epic stays epic. Otherwise `base`.

#### 2.5 Is a glint ("foil") needed?

1. If the stack carries `enchantment_glint_override` (patch or prototype): its boolean value decides,
   in both directions (`false` turns off the glint of an enchanted item).
2. Otherwise: glint ⇔ `enchantments` non-empty; for `compass`: or `lodestone_tracker` present.

For special models, see 1.3 (only shield and trident draw it).

#### 3. Display name of a stack

#### 3.1 Priority order

1. `custom_name` present → that text.
2. Otherwise, `written_book_content` present and **raw** title (`title.raw`, not the filtered
   version) non-empty after trimming whitespace → literal text of the title.
3. Otherwise, the class's own name (3.2) or, failing that, the `item_name` component of the stack (patch then
   prototype); if missing, empty text.

Note: for the classes of 3.2, when their condition applies, the `item_name` component
is **not** consulted (an `item_name` set by the server on a potion is ignored in favor of the
potion name).

#### 3.2 Special cases (step 3)

- **Compass** (`compass`) with `lodestone_tracker`: translation `item.minecraft.lodestone_compass`.
- **Potions** (`potion`, `splash_potion`, `lingering_potion`) and **tipped arrow**
  (`tipped_arrow`), if `potion_contents` present (always true by default): key
  `<item description key>.effect.<suffix>`, e.g. `item.minecraft.potion.effect.swiftness`,
  with `suffix` = field `custom_name` of `potion_contents` (string) if it exists, otherwise base name
  of the referenced potion, otherwise `empty` ("Uncraftable Potion"). Base name of a potion =
  identifier without the `long_` or `strong_` prefix (verified on the 46 vanilla potions: `water`,
  `mundane`, `thick`, `awkward`, `turtle_master`, `swiftness`…). E.g. `splash_potion` + `water` →
  `item.minecraft.splash_potion.effect.water` = "Splash Water Bottle".
- **Shield** with `base_color`: key `item.minecraft.shield.<color>` (e.g. "Red Shield").
- **Player head** with `profile` whose `name` field is present: key
  `block.minecraft.player_head.named` with an argument = that name (en_us: "%s's Head"). Profile without a
  name → ordinary name ("Player Head").

#### 3.3 "block" or "item" items

The prefix of the description key (`block.minecraft.<id>` for most items that place
a block, `item.minecraft.<id>` otherwise, with a few exceptions and imposed keys) is frozen in
`item_name` of the prototype: **read `item_name` in the report**, do not deduce the prefix. The keys
`.effect.`, `.named`, `.<color>` of 3.2 rely on the same description key, which is the key
of the prototype's `item_name` (the report's `translate`) for these items.

#### 3.4 Style: rarity color and italics

Displayed text (name of the held item above the hotbar, name in bundle tooltips) = an empty parent
text, to which the name (3.1) is added as a child, with on the parent:
- color = color of the effective rarity (2.4): common white `FFFFFF`, uncommon yellow `FFFF55`,
  rare cyan `55FFFF`, epic magenta `FF55FF`;
- italic **if and only if** the stack carries `custom_name` (not for a written book title).

The child's style only inherits the attributes it does not set itself: a `custom_name` that has
its own `color` or `italic: false` wins over the rarity / the italics.

Hotbar reminder (already in hud.md § 5.1, verified identical): centered, `y = gui_height − 59`
(+14 if the player cannot be hurt), alpha `min(255, ⌊timer·256/10⌋)`, timer re-armed to
`⌊40 × notification duration⌋` when the held item changes or its name (3.1, unstyled) changes.

### Uncertain points

- Exact use of the extents of special layers in the `oversized_in_gui` rule: read on the
  producer side only (1.2), consumption not followed.
- Default value of an absent `gui_light` = `side`: not re-verified here (mission A).
- `skin_patch` of the player head profile: passed to the default skin, exact effect not followed.
- Geometry of the entity layers (including the `player_head` hat): out of mission.
- Extraction proposal 2.2: choice "committed table or local generation" to be decided
  (license of the game data).
- The imposed description keys have not been inventoried: no impact
  as long as `item_name` of the report is read.

