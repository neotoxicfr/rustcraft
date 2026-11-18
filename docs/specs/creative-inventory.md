# Creative inventory and recipe book (26.3, protocol 777)

Source: reading of the decompiled client (kept outside the repository), reworded. Prerequisites: `inventories.md`
(container screens, slot numbering, click logic § 2, common rendering § 4, tooltip
§ 4.7), `hud.md` (layers, sprites, item in a slot), `items.md` and `item-components.md`
(stacks, components), `text.md`, `option-menus.md` (options, keys), `sounds.md`.

Conventions: those of `inventories.md` (interface coordinates, `(gx, gy)` = top-left corner
of the screen's image, ARGB colours). Mouse buttons are given in protocol numbering
(0 left, 1 right, 2 middle).

---

## 1. Creative inventory: opening and life cycle

- Opens in place of the player's inventory screen when the player has **infinite
  materials** (the "instant build" ability of `player_abilities`, i.e. creative): inventory
  key → inventory screen (local, no packet, `inventories.md` § 3) → if infinite materials are
  present, immediately replaced by the creative screen. The open survival inventory screen also
  checks every tick and switches to the creative screen if the player goes into creative.
- Conversely, on every tick of the creative screen: if the player no longer has infinite
  materials → replaced by the survival inventory screen.
- The creative screen uses its own menu (window identifier 0) which shares the carried item
  with the player's menu 0. Closing: like any container screen (`container_close` with
  window 0).
- The selected tab is **remembered for the session** (global variable): it reopens on the last
  tab. Initial value: "Building Blocks". On opening, if the remembered tab can no longer be
  displayed (§ 2.3), fall back to the default tab.
- **No `container_click` is sent** in the creative screen: every modification is applied
  locally then transmitted slot by slot through `set_creative_mode_slot` (§ 7).
- Does not pause the game.

---

## 2. Tabs

### 2.1 List (order of the `creative_mode_tab` registry, 26.3)

14 tabs, fixed positions (row, column). There is **no pagination** in the base game:
7 slots on top, 7 on the bottom, all occupied.

| # | identifier | row, column | title (key) | icon | type | notes |
|---|---|---|---|---|---|---|
| 0 | `building_blocks` | top, 0 | `itemGroup.buildingBlocks` | bricks | category | default tab |
| 1 | `colored_blocks` | top, 1 | `itemGroup.coloredBlocks` | cyan wool | category | |
| 2 | `natural_blocks` | top, 2 | `itemGroup.natural` | grass block | category | |
| 3 | `functional_blocks` | top, 3 | `itemGroup.functional` | oak sign | category | |
| 4 | `redstone_blocks` | top, 4 | `itemGroup.redstone` | redstone dust | category | |
| 5 | `hotbar` | top, 5 | `itemGroup.hotbar` | bookshelf | saved hotbars | right-aligned |
| 6 | `search` | top, 6 | `itemGroup.search` | compass | search | right-aligned, background `tab_item_search` |
| 7 | `tools_and_utilities` | bottom, 0 | `itemGroup.tools` | diamond pickaxe | category | |
| 8 | `combat` | bottom, 1 | `itemGroup.combat` | netherite sword | category | |
| 9 | `food_and_drinks` | bottom, 2 | `itemGroup.foodAndDrink` | golden apple | category | |
| 10 | `ingredients` | bottom, 3 | `itemGroup.ingredients` | iron ingot | category | |
| 11 | `spawn_eggs` | bottom, 4 | `itemGroup.spawnEggs` | creeper spawn egg | category | |
| 12 | `op_blocks` | bottom, 5 | `itemGroup.op` | command block | category | right-aligned, conditional content (§ 3.3) |
| 13 | `inventory` | bottom, 6 | `itemGroup.inventory` | chest | survival inventory | right-aligned, background `tab_inventory`, no title, no scrollbar |

Default background: `textures/gui/container/creative_inventory/tab_items.png` (256 × 256, of
which 195 × 136 is used). All icons are plain stacks (quantity 1, no added component).

### 2.2 Types

- **category**: built list of items (§ 3);
- **saved hotbars**: content = the 9 hotbars of `hotbar.nbt` (§ 8);
- **search**: input field + results (§ 4);
- **survival inventory**: the player's real slots (§ 5).

### 2.3 Visibility

A tab is shown unless it is of category type **and** its list is empty. In practice only
`op_blocks` can disappear (without permission, § 3.3); its place stays empty (positions do not
close up). Hidden tabs are neither drawn nor clickable.

---

## 3. Content of the "category" tabs

### 3.1 Principle

Each tab has a generator that pushes stacks (quantity 1) in order, with a visibility:
*tab and search* (default), *tab only*, *search only*. Two sets are kept per tab: the tab's
list and the "for search" list. Both are insertion-ordered sets, deduplicated on (item,
components): a stack already present is not added twice.

A stack is added **only if its item is enabled**: the set of features required by the item must
be included in the set received through `update_enabled_features` (in base 26.3, no item asks
for anything other than `minecraft:vanilla`, but the rule must be generic). The generated potions
are additionally filtered by their own required features.

Construction parameters: (enabled features, operator permission § 3.3, dynamic registries
received during configuration + tags). The lists are rebuilt **only if one of the three
changes** (comparison with the last set of parameters), when the screen opens and on every tick
of the open screen. Build order: first all category tabs, then the others (search aggregates the
categories, § 4.1). After a rebuild, if the selected tab is a category that became empty →
default tab; otherwise its content is reloaded keeping the scroll row (§ 6.3).

### 3.2 Fixed lists and generated parts

The bulk of each tab is a list hard-coded in the client (≈ 1,500 lines in total, game order).
**Recommendation**: obtain it through the extractor (`tools/extractor`) by running the jar's
generators with the base registries and outputting, per tab, the ordered sequence (item
identifier + added components + visibility), flagging the sequences that depend on dynamic
registries (list below) so that the client regenerates them from what the server sent. None of
this list is to be copied by hand into the repository.

Parts depending on data (to be generated on the client):

| tab | position | generated content |
|---|---|---|
| `colored_blocks`, `functional_blocks` | everywhere | coloured families in the "game" order of the 16 colours: white, light gray, gray, black, brown, red, orange, yellow, lime, green, cyan, light blue, blue, purple, magenta, pink (not the dye identifier order) — fixed, extracted |
| `functional_blocks` | right after the plain painting | one painting per painting variant **carrying the tag** `#minecraft:placeable` (registry `painting_variant`), sorted by area (width × height) ascending then width; stack = `painting` + component `painting/variant` |
| `tools_and_utilities` | after the elytra | firework rockets of flight duration 1, 2, 3 (component `fireworks`, no explosion) |
| `tools_and_utilities` | after the minecarts | one goat horn per instrument of the tag `#minecraft:goat_horns` (registry `instrument`), in the tag's order; component `instrument` |
| `combat` | after the crossbow | rockets of duration 1, 2, 3 (the same stacks as in tools: deduplicated in search) |
| `combat` | at the end | one tipped arrow per potion of the `potion` registry, registry order, filtered by features; component `potion_contents` |
| `food_and_drinks` | after the rabbit stew | suspicious stews: one per effect source (flowers…), deduplicated — fixed, extracted |
| `food_and_drinks` | after the honey bottle | ominous bottles of amplifier 0 to 4 |
| `food_and_drinks` | at the end | potions, then splash potions, then lingering potions, one per potion of the registry |
| `ingredients` | at the end | enchanted books: in the tab, one per enchantment at **max level** (tab only); in search only, one per enchantment **and per level** from min to max; order of the received `enchantment` registry |
| `op_blocks` | all | see § 3.3 |

The other families (wood, copper in 4 oxidation states then waxed, lanterns, etc.) are fixed and
come from the extraction.

### 3.3 "Operator" tab

Content only if the **permission** is true, otherwise empty list → tab hidden.
Permission = option `operatorItemsTab` (`options.operatorItemsTab`, default false, set in the
commands screen) **and** player in instant build **and** operator level ≥ 2 (entity event 26 to
28 on the local player). A change in any of these terms triggers a rebuild (§ 3.1).

Order: command block, chain command block, repeating command block, command block minecart,
jigsaw block, structure block, structure void, barrier, debug stick, test instance block; one
test block per mode (mode order, mode carried by the stack's block state); sixteen light
blocks from level 15 to level 0 (`level` state carried by the stack); then the painting variants
**without** the tag `#minecraft:placeable`, same sort as in § 3.2.

### 3.4 "Saved hotbars" tab

9 rows of 9 (one per saved hotbar, order 1 to 9). Empty hotbar (the 9 stacks empty): empty row
except, in the column = number of the hotbar, a **paper** marked "creative slot lock" (component
`creative_slot_lock`) and named `inventory.hotbarInfo` with as arguments (name of the "save
hotbar" key, name of the key of the corresponding hotbar slot). Non-empty hotbar: its 9 decoded
stacks (§ 8). These stacks are picked up like those of a category (§ 7.2); the locked paper cannot
be picked up.

---

## 4. Search tab

### 4.1 Search base

Search list = union, in the order of the registry's tabs, of the "for search" lists of all
tabs except search itself, deduplicated on (item, components). Empty field → this entire list is
shown.

Two indexes are built on this list (in the background in the game; free here):

- **by name**: for each stack, all lines of its tooltip in "normal + creative" mode (not the
  advanced mode), plain text without formatting codes, trimmed, empty lines ignored, in
  lowercase; plus the item identifier (namespace and path, lowercase);
- **by tag**: the identifiers of the item tags carried by each stack.

Rebuilt when the lists are rebuilt (§ 3.1), on every `update_tags` (tag index) and on language
change (all indexes).

### 4.2 Query

Field text (50 characters max), converted to lowercase.

- Starts with `#`: the `#` is removed and the **by-tag** index is searched:
  - without `:`: stacks for which a tag path **contains** the text;
  - with `:` (split at the first `:`, two trimmed pieces): namespace contains the left part
    **and** path contains the right part.
  - In addition, the list of "visible tags" is recomputed: all tags of the item registry
    satisfying the same criterion (without `:`: path contains; with `:`: namespace and path
    contain). They are used for the tooltip (§ 6.5).
- Otherwise, **by-name** index:
  - without `:`: stacks for which **a tooltip line contains** the text (the identifier is not
    consulted);
  - with `:`: namespace of the identifier contains the left part **and** (path of the identifier
    contains the right part **or** a tooltip line contains it).

"Contains" = exact substring, character-sensitive (the data is already lowercase). An empty part
is contained everywhere. **Sort**: results are returned in the order of the search list (§ 4.1),
without duplicates. Any modification of the text resets the scroll to the top.

### 4.3 Field and keyboard

- Field at `(gx + 82, gy + 6)`, 80 × 9, **borderless**, white text, non-inverted selection,
  visible only in the search tab. In this tab it always has focus and cannot lose it; switching
  to the tab → text kept if we were already in it, otherwise cleared.
- Outside the search tab: the "chat" key (T) switches to the search tab, and the character of
  this keystroke is ignored (it does not reach the field).
- In the search tab: if a number key is pressed while the hovered slot is not a creative grid
  slot or contains an item, the swap with the hotbar is attempted first (§ 7.2, swap mode,
  requires a hovered slot); if it happens, the keystroke does not reach the field. Otherwise the
  field receives the key; the field captures the keyboard (the inventory key is not a close),
  only Escape closes.
- A click on a grid slot places the field's cursor at the end of the text and removes the
  selection.

---

## 5. "Survival inventory" tab

Background `textures/gui/container/creative_inventory/tab_inventory.png`. No title, no
scrollbar. The slots displayed are those of the player's menu 0 (`inventories.md` § 5.1),
repositioned:

| menu 0 slots | position (relative to gx, gy) |
|---|---|
| 0 to 4 (result and 2 × 2 grid) | off-screen (not displayed) |
| 5 to 8 (head, chest, legs, feet) | `p = slot − 5`: `(54 + 54·⌊p/2⌋, 6 + 27·(p mod 2))` → head (54, 6), chest (54, 33), legs (108, 6), feet (108, 33) |
| 45 (off-hand) | (35, 20) |
| 9 to 35 (inventory) | `(9 + 18c, 54 + 18r)`, c = (slot − 9) mod 9, r = (slot − 9) div 9 |
| 36 to 44 (hotbar) | `(9 + 18c, 112)` |
| bin (slot specific to the screen) | (173, 112) |

- The slots keep their rules (armour, empty-slot sprites `container/slot/*`, etc.).
- Bin: hover → tooltip `inventory.binSlot`.
- Player model: same rendering as the survival inventory (`inventories.md` § 5.1) in the frame
  `(gx + 73, gy + 6)`–`(gx + 105, gy + 49)`, size 20, vertical offset 0.0625.
- Active status effects: displayed next to the image as in the survival inventory (same
  uncertain point).

---

## 6. Geometry and rendering

### 6.1 Image and grid

- Image 195 × 136; `gx = (W − 195) / 2`, `gy = (H − 136) / 2` (no recipe book).
- Category, hotbar and search tabs: grid of 45 slots (5 rows × 9) at
  `(9 + 18x, 18 + 18y)`; player hotbar at `(9 + 18c, 112)` (slots 36 to 44 of menu 0).
- Tab title (except the inventory tab): `(8, 6)`, `0xFF404040`, no shadow. No "Inventory"
  line.

### 6.2 Tabs

- Size 26 × 32. Relative abscissa: `27·column`; if the tab is right-aligned:
  `195 − 27·(7 − column) + 1` (column 5 → 142, column 6 → 169).
- Drawing: ordinate `gy − 28` (top row) or `gy + 136 − 4` (bottom row).
  Sprites `container/creative_inventory/tab_<top|bottom>_<selected|unselected>_<column + 1>`
  (26 × 32). Icon: 16 × 16 item at `(x + 5, y + 8 + 1)` on top, `(x + 5, y + 8 − 1)` on the bottom.
- Click area: reference ordinate `gy − 32` (top) or `gy + 136` (bottom); click if
  `x ≤ mx ≤ x + 26` and `y ≤ my ≤ y + 32` (bounds included) in relative coordinates.
- Hover for the tooltip (tab name): rectangle `(x + 3, y + 3, 21, 27)` relative to the same
  reference; the first hovered tab (registry order) wins.
- "Hand" cursor when hovering an unselected tab.
- Background drawing order: unselected tabs, then the background image of the current tab
  (195 × 136 from (0, 0) of the 256 × 256 texture), the search field, the scrollbar, then the
  selected tab **over** the image, then (inventory tab) the player model.
- Selection: the left press on a tab is consumed (no slot click); it is the left **release** on a
  tab that selects it. A click outside the image but on the current tab does not count as
  "outside the screen".

### 6.3 Scrolling

- `n` = number of stacks in the list; excess rows `R = ⌈n/9⌉ − 5`. Scrollable if the tab allows
  a scrollbar and `n > 45`.
- State `s ∈ [0, 1]`. Start row `k = max(0, ⌊s·R + 0.5⌋)`; the grid slot (x, y) displays the
  stack `x + 9·(y + k)` (empty beyond).
- Wheel (if scrollable): `s ← clamp(s − Δ/R, 0, 1)` (Δ = notches, positive upwards).
- Scrollbar: area `[gx + 175, gx + 189) × [gy + 18, gy + 130)` (only if the tab has a
  scrollbar). Left press inside → drag if scrollable; during the drag
  `s = clamp((my − (gy + 18) − 7.5) / (112 − 15), 0, 1)`. Release → end.
- Handle: sprite `container/creative_inventory/scroller` (or `scroller_disabled` if not
  scrollable), 12 × 15, at `(gx + 175, gy + 18 + ⌊(112 − 17)·s⌋)`; drawn for any tab with a
  scrollbar. Cursor: vertical resize during the drag, hand on hover if scrollable, "forbidden"
  otherwise.
- Tab change → `s = 0`. Reload of the same tab's list (§ 3.1) or window resize → we keep the row
  `k` and recompute `s = clamp(k/R, 0, 1)`.

### 6.4 Item tooltips

Base: `inventories.md` § 4.7. Differences:

- Hovering a creative grid slot: "creative" tooltip mode — the "hidden tooltip" component is
  ignored; a painting without a variant shows `painting.random` (grey); a suspicious stew lists
  its effects. The "advanced tooltips" option is respected.
- Category tab + grid slot: tooltip as is.
- Otherwise (search, hotbars, inventory, or player hotbar): we insert from line 1 (under the
  name):
  1. (search tab + grid slot only) for each visible tag (§ 4.2) that the item carries: line
     `#<namespace>:<path>` dark purple, each new one inserted at line 1;
  2. then, for each displayed tab except search, in order, if the stack appears in its "for
     search" list (same item and components): its name in blue, inserted at lines 1, 2, 3…
     (thus above the tags).

### 6.5 Carried item and dragging

As `inventories.md` § 4.5–4.6. Dragging (mode 5) cannot drop into creative grid slots;
double-click (mode 6) picks nothing up there.

---

## 7. Creative clicks and `set_creative_mode_slot`

### 7.1 `set_creative_mode_slot` packet (server-bound)

`short` slot (signed, 16 bits) then an optional stack ("untrusted" codec: components delimited
by length, like `container_click` — `inventories.md` § 1.6).

Sent **only if** the player has infinite materials **and** the stack's item is enabled
(features). Two uses:

- **Set a slot**: slot = number in **menu 0** (1 to 45; 5..8 armour, 9..35 inventory, 36..44
  hotbar, 45 off-hand). The server accepts it if 1 ≤ slot ≤ 45 and quantity ≤ max stack size,
  sets it as is and sends back corrections if needed. Slot 0 (result) is refused.
- **Drop**: slot = −1, non-empty stack. The server drops the stack if its drop limiter allows
  it (+20 per drop, refusal beyond 1,480, −1 per tick).
  On the client side, each drop sent increments the local limiter (+20, −1 per player tick);
  "can drop" = counter < 1,280. No drop sent if a container screen other than the creative one
  is open.

**Sending mechanism**: while the creative screen is open, an observer is attached to the
player's menu 0; on each "broadcast of changes" of menu 0 (comparison of each slot with its last
known value), each modified slot goes out as `set_creative_mode_slot` (slot, new stack). The
carried item is never transmitted: the server does not know it.

### 7.2 Category, saved hotbars, search tabs

Inputs are translated into (slot, button, mode) as in `inventories.md` § 2.9; then:

- A left press outside the image (slot −999, pickup mode) becomes the **drop** mode.
- If the mode is drop and the client "cannot drop" → nothing.
- **Outside the image** (no slot, not a drag), carried item non-empty, click really outside the
  image (§ 6.2): button 0 → drops the whole carried stack (packet −1) and empties the carried
  item; button 1 → removes 1 from the carried item and drops it (packet −1).
- Slot refusing pickup (locked paper, non-enabled item) → nothing.
- **Creative grid slot** (outside a drag) — local prediction, no packet except drops:
  - swap (keys 1–9, F): if the slot is not empty, the inventory slot `button` (0..8 hotbar, 40
    off-hand) receives a copy at **max size**; broadcast → packet for the corresponding slot of
    menu 0 (36 + n, or 45);
  - clone (middle click): if carried is empty and the slot not empty → carried = copy at max
    size;
  - drop (Q / Ctrl+Q on hover): copy of 1 (Q) or of the max size (Ctrl+Q) dropped (packet −1);
  - pickup, carried and slot of the same stack (item + components): button 0 → carried +1
    (capped at the max size), or set to the max size if Shift; button 1 → carried −1;
  - pickup, carried empty, slot not empty: carried = copy with quantity = that of the slot (1),
    or the max size if Shift;
  - pickup, other cases: button 0 → carried emptied (the item disappears); button 1 → carried
    −1.
- **Player hotbar slot** (at the bottom of the image): normal click logic (`inventories.md` § 2)
  applied to the screen's menu, with these differences: Shift+click **clears** the slot (no
  move); drop with a non-empty carried item: removes from the slot 1 (button 0) or the whole
  stack (button 1) and drops it (packet −1). Then broadcast of menu 0 → packets for the changed
  slots 36..44.
- **End of drag** (mode 5, end phase): the 9 hotbar slots are sent explicitly (36 + i, stack of
  the slot), changed or not.

### 7.3 Survival inventory tab

- **Bin + Shift** (click with Shift on the bin): empties **all** slots of menu 0 (0 to 45) and
  sends one packet per slot (empty stack), including slot 0 (which the server ignores).
- Bin without Shift: the carried item is emptied (destroyed), no packet.
- Drop on hovering a non-empty slot: removes 1 (button 0) or the max size (button 1), drops
  (packet −1) then sends the slot's new content (menu 0 slot packet).
- Drop outside the image (slot −999) with carried non-empty: drops all of the carried item
  (packet −1), whatever the button, and empties the carried item.
- Everything else: click applied **to the player's menu 0** (same logic as survival, infinite
  materials mode for cloning), slot = menu 0 number, then broadcast → packets.

### 7.4 What is predicted, what the server corrects

Everything is predicted locally (slot contents and carried item). The server only sends
corrections (`container_set_slot` of menu 0) if a set is refused (non-enabled item, quantity too
large, invalid slot). The carried item exists only on the client: if it is still carried on
closing, the server knows nothing about it (see uncertain points).

---

## 8. Saved hotbars (`hotbar.nbt`)

### 8.1 Keys

Keys `key.saveToolbarActivator` (C) and `key.loadToolbarActivator` (X), category
"Creative" (already in `options.rs`). **In game, with no screen open, player in infinite
materials**, on pressing a hotbar slot key 1–9 (index i):

- if X is held (priority): **load** — for slot_i = 0..8, the hotbar slot receives the decoded
  stack of hotbar i and a `set_creative_mode_slot` (36 + slot_i, stack) is sent for each,
  empty ones included;
- otherwise if C is held: **save** — hotbar i receives the 9 current hotbar slots; action
  message (action bar, no rainbow animation) `inventory.hotbarSaved` with arguments (name of the
  X key, name of the key of slot i); the file is rewritten.
- otherwise: normal slot selection (`hud.md`).

Outside creative, or if a screen is open, or as a spectator: normal key behaviour.

### 8.2 File

`<game folder>/hotbar.nbt`, **uncompressed** NBT (compound root with an empty name), read lazily
on first access and rewritten entirely on each save:

```
{
  DataVersion: 5023,          // 26.3 data version
  "0": [ stack, stack, … ×9 ],  // hotbar 1
  …
  "8": [ … ]                  // hotbar 9
}
```

- Each stack: compound `{ id: "minecraft:…", count: <int>, components: { … } }`; `count`
  is always written; `components` omitted if there is no difference from the default components
  (persistent form of components, `item-components.md`). Empty stack: **empty** compound `{}`.
- The list must be exactly 9 elements, otherwise this hotbar is ignored (empty hotbar).
- An unreadable stack (unknown item, invalid component) becomes empty, the others are kept.
- File from an earlier version (`DataVersion` absent = 1343): the game updates it through its
  converters. Rustcraft: read as is whatever decodes, the rest empty (uncertain point).
- A hotbar is "empty" if its 9 elements are exactly `{}`.

---

## 9. Recipe book: packets

All in the play phase. Base codecs (registry identifier as `varint`, set of references
— `varint` n: 0 = tag name follows, otherwise n − 1 identifiers —, "direct or by identifier"
reference — `varint` 0 = inline value, otherwise identifier + 1): `protocol-supplements.md`
and `item-components.md`.

### 9.1 Recipe display identifier

`varint` (index assigned by the server, stable for the session).

### 9.2 Book entry

1. identifier (9.1);
2. recipe display (9.3);
3. group: compact optional `varint` (0 = none, otherwise group + 1);
4. category: identifier of the `recipe_book_category` registry (order: `crafting_building_blocks`,
   `crafting_redstone`, `crafting_equipment`, `crafting_misc`, `furnace_food`, `furnace_blocks`,
   `furnace_misc`, `blast_furnace_blocks`, `blast_furnace_misc`, `smoker_food`, `stonecutter`,
   `smithing`, `campfire`);
5. crafting requirements: boolean present, then list (`varint` length) of ingredients,
   each ingredient = set of item references. Absent → never "craftable".

### 9.3 Recipe display (registry `recipe_display`)

`varint` type then body:

| id | type | body |
|---|---|---|
| 0 | `crafting_shapeless` | list of slot displays (ingredients), result, station |
| 1 | `crafting_shaped` | width `varint`, height `varint`, list of displays (width × height, row by row), result, station |
| 2 | `furnace` | ingredient, fuel, result, station, duration `varint` (ticks), experience `float` |
| 3 | `stonecutter` | input, result, station |
| 4 | `smithing` | template, base, addition, result, station |

"Station" = slot display of the crafting table / furnace… (serves as the category icon for the
toast, § 13).

### 9.4 Slot display (registry `slot_display`)

`varint` type then body; a display is "resolved" into an ordered list of stacks:

| id | type | body | resolution |
|---|---|---|---|
| 0 | `empty` | — | nothing |
| 1 | `any_fuel` | — | all items having the "cooking fuel" component (item registry order) |
| 2 | `with_any_potion` | display | for each potion of the registry, each resolved stack with this `potion_contents` |
| 3 | `only_with_component` | display, component type (`varint`) | resolved stacks that have this component |
| 4 | `item` | item (`varint`) | the stack of quantity 1 |
| 5 | `item_stack` | item `varint`, quantity `varint`, component differences (network codec of components) | the stack |
| 6 | `tag` | set of item references | one stack per item of the set/tag |
| 7 | `dyed` | dye, target | demonstrations: product target × dye (at most 16 non-empty stacks, the index running over the targets first), target dyed with the dye's colour |
| 8 | `smithing_trim` | base, material, pattern (direct or by-id reference) | demonstrations drawn at random (at most 16 out of 256 attempts) — display only |
| 9 | `with_remainder` | input, remainder | the stacks of the input (remainder ignored for display) |
| 10 | `composite` | list of displays | concatenation |

### 9.5 Client-bound packets

- `recipe_book_add`: list (`varint`) of entries `(entry 9.2, flags byte)` — bit 0
  "notification" (toast), bit 1 "highlight" (new) — then boolean "replace".
  Replace → the book is emptied (recipes and highlights) before adding. Each entry is
  added/replaced by identifier; highlight flag → identifier added to the "new" ones;
  notification flag → toast (§ 13).
- `recipe_book_remove`: list of identifiers; removed from the recipes and from the "new" ones.
- `recipe_book_settings`: 4 × (open `bool`, filter `bool`) in the order crafting table
  (inventory and crafting table share), furnace, blast furnace, smoker.
- `place_ghost_recipe`: window identifier (`varint`), recipe display (9.3). Ignored if
  the window is not the current menu or if the screen has no book; otherwise ghost recipe
  (§ 11.4).

After each of the first three: grouping into collections (§ 10.1), rebuild of the book's search
index, and refresh of the book if a screen with a book is open.

### 9.6 Server-bound packets

- `recipe_book_change_settings`: book type (`varint`: 0 crafting table, 1 furnace, 2 blast
  furnace, 3 smoker), open `bool`, filter `bool`. Sent on every opening/closing of the book
  and on every toggle of the filter.
- `recipe_book_seen_recipe`: identifier. Sent when a highlighted recipe becomes visible on a
  page (§ 11.3); the identifier leaves the "new" ones locally.
- `place_recipe`: window identifier (`varint`), recipe identifier (`varint`),
  "maximum" `bool` (Shift held). The server fills the grid (normal slot corrections) or, if
  ingredients are missing, answers `place_ghost_recipe`.

---

## 10. Recipe book: client-side model

### 10.1 Collections

A **collection** = a button of the book. Grouping of all known recipes, in the iteration order
of the recipe table (uncertain point § 14): by category; a recipe without a group forms its own
collection; recipes of the same (category, group) form a single collection, created on first
encounter, in encounter order.

"Search" tabs (pseudo-categories) = concatenation, in this order, of the collections of:
- crafting table: equipment, building, misc, redstone;
- furnace: food, blocks, misc;
- blast furnace: blocks, misc;
- smoker: food.

### 10.2 Selection and "craftable"

Available content = the player's main inventory (36 slots, no armour or off-hand), counting
only stacks that are **undamaged, unenchanted, without a custom name**, quantity capped at the
max size; plus, for the crafting table and the inventory, the content of the crafting grid (same
criteria). Furnaces: nothing more on the client side.

For each recipe of a collection:
- **selected** if it is displayed on this screen: crafting table/inventory — fixed shape fitting
  in the grid (width and height ≤ those of the grid, 2 × 2 or 3 × 3), or shapeless with
  number of ingredients ≤ grid slots; other types refused; furnaces — type `furnace` only (the
  tab's category sorts by furnace type);
- **craftable** if selected and the requirements (§ 9.2) can be satisfied by the available
  content: each ingredient must receive **one** unit of an item of its set, without reusing the
  same unit (assignment problem); absent requirements → not craftable.

Recomputed when the book opens, on every inventory change (modification counter checked every
tick), on every click on a crafting slot (which also clears the ghost recipe and the "last
recipe placed"), and on every book update.

### 10.3 Book search

Index per collection: tooltip lines (normal mode) of the result stacks of all its recipes +
identifiers of these items; same rules as § 4.2 (without the `#`). Query in lowercase. Easter
egg: the exact query `excitedze` switches the language to `en_pt` if it exists — optional.

---

## 11. Recipe book: display

### 11.1 Placement

Screens with a book: survival inventory (crafting table type, 2 × 2 grid), crafting table
(3 × 3), furnace, blast furnace, smoker. Book button: `inventories.md` § 5.1, 5.5, 5.6 (20 × 18,
sprites `recipe_book/button` / `button_highlighted`); a click toggles the book and repositions
the screen (`gx`, § 4.1 of `inventories.md`) and the button. Inventory: the release following
this click is swallowed.

- "Narrow" if `W < 379`. Offset `d = 0` if narrow, otherwise 86.
- Book origin: `bx = (W − 147)/2 − d`, `by = (H − 166)/2`. Background `textures/gui/recipe_book.png`,
  portion **(1, 1)** of 147 × 166 (256 × 256 texture).
- Narrow and book visible: the container screen is drawn for its background only (no slots or
  items), the book on top; no slot can be hovered; any click outside the book is swallowed.
- Order: (screen content), next layer: book, next layer: carried item,
  screen tooltips, book tooltips (hovered button then hovered ghost slot).
- Open/filter state per book type: given by `recipe_book_settings`; checked every tick
  (the book opens/closes if the server changes the state).
- "Outside the screen" for container clicks: outside the image **and** outside the book
  rectangle `(gx − 147, gx) × (gy, gy + height)` (bounds excluded) **and** not on the selected
  tab.

### 11.2 Elements

- **Search**: field at `(bx + 25, by + 13)`, 81 × 14, 50 characters, white text, hint
  `gui.recipebook.search_hint` (hint style of search fields). Click on the magnifier (area
  `(bx + 8, by + 13)` of width 17, field height) or on the field → focus. The "chat" key gives
  focus to the field (keystroke ignored). Enter (or select key) without focus: redoes the last
  recipe placed (with its interface click). Escape in narrow mode: closes the book.
- **Filter**: toggle button 26 × 16 at `(bx + 110, by + 12)`, sprites
  `recipe_book/filter_<enabled|disabled>[_highlighted]` (crafting table/inventory) or
  `recipe_book/furnace_filter_…` (furnaces). Tooltip: filter active →
  `gui.recipebook.toggleRecipes.craftable` (crafting table), `…smeltable` (furnace), `…blastable`
  (blast furnace), `…smokable` (smoker); inactive → `gui.recipebook.toggleRecipes.all`.
- **Tabs** (35 × 27, sprites `recipe_book/tab` / `tab_selected`) to the left of the book,
  `x = bx − 30`, `y = by + 3 + 27·i` (i = rank among the visible tabs). The selected tab is
  drawn 2 px further left (icons too). Icons ("dummy" stacks, no decoration) at `y + 5`: one icon
  at `x + 9`, two icons at `x + 3` and `x + 14`.
  - crafting table: search (compass); equipment (iron axe + gold sword); building
    (bricks); misc (lava bucket + apple); redstone (redstone dust);
  - furnace: search; food (porkchop); blocks (stone); misc (lava bucket + emerald);
  - blast furnace: search; blocks (redstone ore); misc (iron shovel + gold leggings);
  - smoker: search; food (porkchop).
  The search tab is always visible; the others only if at least one collection has a selected
  recipe. Initial tab: the first; kept on refresh.
  "New" animation: if a displayable recipe (filter included) of the tab is highlighted, the tab
  is stretched vertically by `1 + 0.1·sin(π·t/15)` around `(x + 8, y + 12)` for 15 ticks.
- **Page**: 20 buttons 25 × 25, at `(bx + 11 + 25·(i mod 5), by + 31 + 25·⌊i/5⌋)`.
  Collections listed = those of the tab having at least one selected recipe, filtered by the
  search (if non-empty), then, filter active, by "at least one craftable".
  Pages: `⌈n/20⌉`. Arrows 12 × 17: next `(bx + 93, by + 137)` sprites
  `recipe_book/page_forward[_highlighted]`, previous `(bx + 38, by + 137)`
  `recipe_book/page_backward[_highlighted]`, visible if there is more than one page and a page
  in that direction; tooltips `gui.recipebook.next_page` / `previous_page`. If more than one
  page: text `gui.recipebook.page` (page, total) white, centred at `x = bx + 73`, `y = by + 141`.
  Changing tab → page 1; otherwise the page is kept as long as it exists.

### 11.3 Recipe button

- Recipes shown = selected (filter inactive) or craftable (filter active).
- Background: `recipe_book/slot_craftable` / `slot_uncraftable` depending on whether the
  collection has at least one craftable recipe (independently of the filter), variant
  `slot_many_*` if more than one recipe is shown.
- Item shown: with a cycle index `c = ⌊T/30⌋` (T = elapsed time in ticks, which does not advance
  while Ctrl is held): recipe `c mod m`, then result stack
  `⌊c/m⌋ mod (number of stacks of the result)`. Dummy stack at `(x + 4, y + 4)`; if more than one
  recipe and all results are the same stack: a second stack (normal rendering) at
  `(x + 5, y + 5)` drawn first, and the dummy stack moves to `(x + 3, y + 3)`.
- When a page is displayed, each highlighted recipe shown → `recipe_book_seen_recipe`
  and scale animation `1 + 0.1·sin(π·t/15)` (x and y) around `(x + 8, y + 12)` over 15 ticks.
- Tooltip (if no variants menu is open): tooltip of the shown stack, plus
  `gui.recipebook.moreRecipes` if there are several recipes.
- Left click → place the current recipe (§ 11.4). Right click on a button with several
  recipes → variants menu.

### 11.4 Placement and ghost recipe

- Place: if the recipe is not craftable **and** it is already the last one placed → nothing.
  Otherwise: last placed = it, ghost cleared, `place_recipe` (current window, id, Shift).
  In narrow mode, the book then closes.
- **Ghost recipe** (on `place_ghost_recipe`): for each concerned slot, list of resolved stacks
  (§ 9.4) — result on the result slot, ingredients on the grid slots:
  - fixed shape: placement in the grid left to right and top to bottom; if the recipe is
    shorter than half the grid, it is centred vertically (`⌊H/2 − h/2⌋` rows skipped); likewise
    horizontally if narrower than half;
  - shapeless: the first n ingredients in the first n slots;
  - furnace: ingredient in slot 0, fuel in slot 1 only if it is empty.
  Drawing, after the screen's slots: background `0x30FF0000` over 16 × 16 (24 × 24 offset by −4
  for the result of the crafting table and furnaces, 16 × 16 for the inventory), dummy stack
  `list[c mod size]`, veil `0x30FFFFFF` 16 × 16, decorations (quantity) for the result
  only. Hovering a ghost slot → tooltip of the shown stack.

### 11.5 Variants menu

- Recipes: craftable first, then (filter inactive) selected non-craftable.
  `t` = total; columns `k = 4` if `t ≤ 16`, otherwise 5; rows `⌈t/k⌉`.
- Position: starts from the button's corner `(x, y)`, reference centre
  `(cx, cy) = (bx + 73, by + 13 + 83)`:
  - if `x + min(t, k)·25 > cx + 50` → `x −= 25·⌊(right − (cx + 50))/25⌋`;
  - if `y + rows·25 > cy + 50` → `y −= 25·⌈(bottom − (cy + 50))/25⌉`;
  - if `y < cy − 100` → `y −= 25·⌈(y − (cy − 100))/25⌉` (negative value: the menu moves down).
- Background: sprite `recipe_book/overlay_recipe` as nine-patch (32 × 32, border 4),
  `(min(t, k)·25 + 8) × (rows·25 + 8)`, on a layer above the page.
- Buttons 24 × 24 at `(x + 4 + 25·(i mod k), y + 5 + 25·⌊i/k⌋)`, sprites
  `recipe_book/crafting_overlay[_disabled][_highlighted]` (or `furnace_overlay…`), "disabled"
  for a non-craftable recipe. Ingredients drawn in miniature (scale 0.375, centred at
  `(x + 2 + 3 + 7·col, y + 2 + 3 + 7·row)` of a 3 × 3 grid, with the same placement as in
  § 11.4; furnace: the ingredient in the centre (1, 1)), stack `c mod n`.
- Left click on a button → place that recipe; any other click → menu closed (click consumed).
  Closing the book → menu closed.

---

## 12. Book: interactions with the container screen

- Click on a crafting slot (inventory: result and grid; crafting table: same; furnace:
  slots 0, 1, 2) → clears ghost and "last placed", recomputes (§ 10.2).
- The book receives clicks, keys and characters **before** the screen; its field captures the
  keyboard when it has focus (the inventory key no longer closes the screen, `inventories.md`
  § 3).
- Spectator: the book reacts to nothing.

---

## 13. "Recipe unlocked" notification

### 13.1 Notification manager (toasts)

- 5 slots of height 32, stacked from the top of the screen on the right. A notification
  occupies `⌈height/32⌉` consecutive slots (first free block); otherwise it waits in a queue.
- Updated every frame (time in ms). Entry: slide of 600 ms, progress
  `p = clamp(Δt/600, 0, 1)²`; position `x = W − width·p` (exit: `p` decreasing likewise),
  `y = slot·32`. A visibility change during an animation restarts from the currently visible
  portion. End of rendering: hidden for more than 600 ms → slot freed.
- Interface sounds at volume 1, pitch 1: `ui.toast.in` / `ui.toast.out` on visibility change
  (at most one per update).
- Drawn after the rest of the interface (new layer), unless the interface is hidden (F1).

### 13.2 Recipe notification

- Only one at a time: a new unlock (notification flag of `recipe_book_add`) adds an entry to
  the existing notification (visible or queued) or creates one.
- Entry = (station stack — first resolved stack of the "station" display —, first stack of the
  result).
- Size 160 × 32, background `toast/recipe`. Title `recipe.toast.title` at (30, 7) colour
  `0xFF500050`; description `recipe.toast.description` at (30, 18) black; no shadow.
  Station: dummy stack at scale 0.6 at (3, 3) (scaled frame); result: dummy stack at (8, 8).
- Stays visible while the time since the last addition is < `5000 × notification display
  duration` (option, default 1.0) ms; entry shown:
  `⌊t / max(1, 5000·mult / entryCount)⌋ mod entryCount` (t = fully visible time, ms).

---

## 14. Uncertain points

1. **Order of the book's collections**: the game iterates a hash table indexed by the display
   identifier (integer). As long as the identifiers stay < the table's capacity (power of 2
   ≥ 16, doubled when the number of known recipes exceeds 3/4 of the capacity), the order is
   ascending; beyond that, the order becomes "identifier modulo capacity, then insertion order".
   Reproducing this rule gives the same order in the vast majority of cases; buckets of more than
   8 elements (tree structure) are not modelled. To be validated by capture.
2. **Fixed lists of the tabs**: they can only come from the jar (§ 3.2); the extractor's output
   format and the flagging of dynamic sequences remain to be designed.
3. **Carried item on closing the creative screen**: the server does not know it; on the client
   side it stays in the local menu 0. Recommendation: empty it on closing (visible behaviour
   identical: the item disappears).
4. **Conversion of old `hotbar.nbt` files** (`DataVersion` < 5023): no converters;
   best-effort reading.
5. **"Craftable" assignment problem**: the game's algorithm was not examined in detail
   (augmenting path search); any exact matching algorithm gives the same yes/no.
6. **Status effects** next to the creative inventory: layout not examined (same gap as
   `inventories.md` § 6.7).
7. **Trim demonstrations** (`smithing_trim`): random draw; the exact order cannot be
   reproduced, no gameplay consequence.
8. **Narration** (screen reader) of the buttons and tabs: out of scope.
9. `update_recipes` (property sets, stonecutter recipes): out of this spec; needed for
   "smeltable / fuel" (`inventories.md` § 6.2), not for the book.
