# Special screens (26.3, protocol 777)

Completes `inventories.md`, which is authoritative for everything common: packets `open_screen`,
`container_close`, `container_set_data`, `container_click`, `container_button_click`,
`rename_item`, `container_slot_state_changed` (§ 1), clicks and prediction (§ 2), opening and
closing (§ 3), common rendering of a container screen (§ 4: position `gx, gy`, drawing
order, titles, slots, carried item, tooltip). Already specified there and only completed
here: enchanting table (§ 5.8), anvil (§ 5.9), furnaces (§ 5.6), chests (§ 5.2).

Conventions carried over:

- Slot coordinates relative to the corner `(gx, gy)` of the image; the slot occupies 16 × 16 from
  `(x, y)`. "Standard inventory (x0, y0)" = 27 slots `(x0 + 18c, y0 + 18r)` then 9 hotbar
  slots `(x0 + 18c, y0 + 58)`. Unless stated otherwise `x0 = 8`, `y0 = 84`.
- Background textures `textures/gui/container/<name>.png` (256 × 256 unless stated), image 176 × 166
  by default, title at `(8, 6)` and "Inventory" at `(8, height − 94)`, colour `0xFF404040`
  without shadow.
- Sprites = interface atlas (`textures/gui/sprites/<id>.png`).
- "Interface sound" = sound played locally, interface category, volume 0.25, pitch 1,
  no attenuation. "Click" = `ui.button.click` played that way (every standard button plays it).
- "Shift-click": describes the local prediction of mode 1 (`inventories.md` § 2.3). The server
  always corrects; the rules are given so that the prediction is right.
- Unless stated, the result sounds (taking a traded, ground, woven item…) come from the
  server (sound or level event): the client has nothing to play.

Container properties (`container_set_data`) added by this document:

| menu | indices |
|---|---|
| beacon | 0 pyramid levels (0..4); 1 primary effect; 2 secondary effect (identifier of the `mob_effect` registry **+ 1**, 0 = none) |
| loom | 0 index of the chosen pattern (−1 = none) |
| stonecutter | 0 index of the chosen recipe (−1 = none) |
| smithing table | 0 recipe error (1 = the three inputs are filled but no result) |
| lectern | 0 displayed page |
| crafter | 0..8 slot disabled (1) or active (0); 9 powered (1) |
| enchanting, anvil, furnaces, brewing stand | see `inventories.md` § 1.3 |

Grindstone, cartography table, merchant, mounts, shulker box: no properties.

---

## 1. Trading (merchant, `merchant`, type 19)

### 1.1 `merchant_offers` packet (client-bound)

| field | encoding |
|---|---|
| window | `varint` |
| offers | `varint` n, then n × offer |
| merchant level | `varint` (1..5; 0 = wandering trader) |
| merchant experience | `varint` |
| show progress | `bool` |
| can restock | `bool` |

One offer:

| field | encoding |
|---|---|
| price A | cost (below) |
| result | stack (usual stack encoding) |
| price B | `bool` present, then cost if present |
| out of stock | `bool` |
| uses | `i32` |
| max uses | `i32` |
| experience gained | `i32` |
| special price difference | `i32` |
| price multiplier | `f32` |
| demand | `i32` |

Cost = item (`varint`, registry `item`), quantity (`varint`), then list (`varint` n) of
**required** components: n × { type (`varint`, registry `data_component_type`), value in the
network format of the component }. The cost is drawn as a stack of this item, of this quantity, carrying
these components.

Handling: ignored if the window is not the current menu or if it is not a merchant menu.
Otherwise replaces the offer list, the experience, the level, the two booleans. "Out of stock" true
forces uses = max. The packet arrives right after `open_screen`, then after each trade.

Derived quantities:

- **Out of stock**: uses ≥ max uses.
- **Adjusted price A**: `b = base quantity`; `d = max(0, ⌊b × demand × multiplier⌋)` (computed
  in 32-bit float); adjusted quantity = `clamp(b + d + special difference, 1, max stack size of
  the item)`. Price B is never adjusted.
- An offer is **satisfied** by (A, B) if A is the right item with the required components and
  quantity ≥ adjusted price A, and if: without price B → B empty; with price B → B of the right item, required
  components, quantity ≥ price B.

### 1.2 Menu (slots)

| slot | role | position |
|---|---|---|
| 0 | payment 1 | `(136, 37)` |
| 1 | payment 2 | `(162, 37)` |
| 2 | result (receives nothing) | `(220, 37)` |
| 3..38 | standard inventory | `(108, 84)` |

The **client predicts the result**: on each change of slots 0/1 (and on choosing an offer), if
slot 0 is empty we take `(A, B) = (slot 1, empty)`, otherwise `(slot 0, slot 1)`. If A is empty →
empty result. Otherwise we look for the offer (chosen offer index `h`: if `0 < h < n`, only offer
h is tested, otherwise the first satisfied offer) with (A, B); if none or out of stock, we retry
with (B, A). Offer found and not out of stock → result = copy of the offer's result, "future
experience" = its experience; otherwise empty result and future experience 0.

Shift-click: result → 3..38 reverse direction; 0/1 → 3..38; 3..29 → hotbar 30..38; hotbar → 3..29.
The gathering double-click never takes from a slot of the merchant menu.

### 1.3 Screen

- Texture `villager.png` **512 × 256**, image **276 × 166**, background `rect(gx, gy, 276, 166)`
  source `(0, 0)`.
- Title: if `1 ≤ level ≤ 5` and progress displayed, text `merchant.title` with
  (received title, `merchant.level.<level>`); otherwise the title alone. Centred on `x = 187` (i.e.
  `187 − width/2`), y 6.
- Label `merchant.trades` centred on x 53, y 6. "Inventory" at `(107, 72)`. Same colour.
- **Offer buttons**: 7 buttons i = 0..6 at `(gx + 5, gy + 18 + 20i)`, 88 × 20, standard button rendering
  (`widget/button`, `button_highlighted` on hover/focus) without a label. Button visible
  if `i < number of offers`. Click: `chosen offer = i + scroll` then:
  1. the menu's selection index = chosen offer (recomputation of the result);
  2. "fill the payment" prediction: if slot 0 is non-empty, put it away in 3..38 (reverse
     direction, usual merge rules); if it does not fit entirely, stop there. Same for
     slot 1. If 0 and 1 are then empty: for price A, walk 3..38 and move into
     slot 0 every item that satisfies the cost (same item and same components as what is already
     in slot 0), up to the stack's max size; same for price B into slot 1;
  3. send **`select_trade`** (server-bound, `varint` offer index).
  The click plays the button sound. The server does the same and sends the slots back.
- **List** (if offers non-empty): for each visible offer (all if n ≤ 7, otherwise the
  indices `scroll .. scroll + 6`), row k = 0..6, `y = gy + 19 + 20k`:
  - price A at `(gx + 10, y)`: item (stack of the adjusted price); if adjusted quantity = base
    quantity, normal decorations; otherwise decorations of the base stack at `(gx + 10, y)` (the
    number is displayed even if it is 1), decorations of the adjusted stack at `(gx + 24, y)`
    (likewise), and sprite `container/villager/discount_strikethrough` 9 × 2 at `(gx + 17, y + 12)`;
  - price B if it exists: item and decorations at `(gx + 40, y)`;
  - arrow `container/villager/trade_arrow` (or `trade_arrow_out_of_stock` if out of stock), 10 × 9,
    at `(gx + 60, y + 3)`;
  - result: item and decorations at `(gx + 73, y)`.
  These items are drawn **after** the buttons (on top).
- **Scrollbar**: `steps = n − 6`. If `steps > 1`: `rest = 139 − (27 + (steps − 1) ×
  139 / steps)`; `step = 1 + rest / steps + 139 / steps` (integer divisions);
  `dy = min(113, scroll × step)`, forced to 113 if `scroll = steps − 1`; sprite
  `container/villager/scroller` 6 × 27 at `(gx + 94, gy + 18 + dy)`. Otherwise
  `scroller_disabled` at `(gx + 94, gy + 18)`. Hand cursor on hover (vertical resize
  during a drag).
  - Wheel (n > 7): `scroll = clamp(scroll − Δ, 0, n − 7)`.
  - Click in `x ∈ ]gx + 94, gx + 100[`, `y ∈ ]gy + 18, gy + 158]` (n > 7) → drag:
    `s = (y − (gy + 18) − 13.5) / 112`; `scroll = clamp(⌊s × (n − 7) + 0.5⌋, 0, n − 7)`.
- **Chosen offer out of stock**: sprite `container/villager/out_of_stock` 28 × 21 at
  `(gx + 182, gy + 35)` (background layer, under the background arrow). Tooltip `merchant.deprecated`
  on hovering `rect(186, 35, 22, 21)` if the merchant can restock.
- **Experience bar** (if progress displayed, level < 5): background
  `container/villager/experience_bar_background` 102 × 5 at `(gx + 136, gy + 16)`. Thresholds
  `S = [0, 10, 70, 150, 250]`; for `1 ≤ level ≤ 4`: `min = S[level − 1]`, `max = S[level]`,
  `m = 102 / (max − min)`; if `xp ≥ min`: current part `w = min(⌊m × (xp − min)⌋, 102)`
  (sprite `experience_bar_current`, source `(0, 0, w, 5)`, at `(gx + 136, gy + 16)`); if
  the future experience > 0: `wf = min(⌊xf × m⌋, 102 − w)` (sprite `experience_bar_result`,
  source `(w, 0, wf, 5)`, at `(gx + 136 + w, gy + 16)`).
- **Button tooltips** (hovering a button whose offer exists): `x < bx + 20` → stack of the
  adjusted price A; `bx + 30 < x < bx + 50` → price B if it exists; `x > bx + 65` → result.
- Sounds: only the button click; the villager's "yes / no" and the trade sound
  come from the server.

---

## 2. Mount inventories

### 2.1 Opening

- Inventory key in game: if the player is a passenger of a vehicle that has an inventory of its own
  (horses and the like, nautiluses; also chest minecarts and boats) → `player_command`
  action "open inventory"; nothing is opened locally. Otherwise normal inventory.
- **`mount_screen_open`** (client-bound): window (`varint`), columns of the mount's
  inventory (`varint`), entity identifier (`i32`). The client creates a menu of `3 × columns`
  mount slots, makes it the current menu (identifier = window) and opens the screen **if**
  the entity is known and is an equine (horse, donkey, mule, llama, trader llama, skeleton/zombie
  horses, camel, camel husk) or a nautilus (nautilus, zombie nautilus); otherwise
  nothing. Title = display name of the entity.
- The content arrives through `container_set_content`; the rest follows `inventories.md`.

### 2.2 Menu

| slot | role | position | accepts |
|---|---|---|---|
| 0 | saddle | `(8, 18)` | item equippable as a saddle by this entity; size 1 |
| 1 | body (armour, carpet) | `(8, 36)` | item equippable on the body by this entity; size 1 |
| 2 .. 2 + 3C − 1 | chest (C columns) | `(80 + 18x, 18 + 18y)`, y = 0..2, x = 0..C−1 | any |
| following | standard inventory | `(8, 84)` | |

Active slot (drawn, clickable):

| entity | saddle | body | columns C |
|---|---|---|---|
| horse, skeleton horse, zombie horse | yes | yes (horse armour) | 0 |
| donkey, mule | adult and tamed | no | 5 if chest, otherwise 0 |
| llama, trader llama | no | yes (carpet) | strength if chest, otherwise 0 |
| camel, camel husk | adult and tamed | no | 0 |
| nautilus, zombie nautilus | adult and tamed | adult and tamed | 0 |

(An equine's saddle also requires membership in the entity tag `can_equip_saddle`; the body
requires `can_wear_horse_armor` or being a llama. The received value C is authoritative for the number of
slots.)

Empty-slot sprites: saddle `container/slot/saddle`; body `container/slot/horse_armor`,
`container/slot/llama_armor` (llamas) or `container/slot/nautilus_armor_inventory` (nautiluses).

Shift-click: from a mount slot → inventory (reverse direction). From the inventory: if
the body slot accepts the item and is empty → body; otherwise if the saddle accepts it and is empty →
saddle; otherwise if C > 0 → chest; otherwise (or if the chest is full) inventory ↔ hotbar.

### 2.3 Screen

- Texture `horse.png` (equines) or `nautilus.png` (nautiluses), 176 × 166.
- Chest (C > 0, equines only): sprite `container/horse/chest_slots` (90 × 54), source part
  `(0, 0, 18C, 54)` at `(gx + 79, gy + 17)`.
- Slot frame `container/slot` 18 × 18 at `(gx + 7, gy + 17)` if the saddle is active, and at
  `(gx + 7, gy + 35)` if the body is active.
- Entity model in the area `(gx + 26, gy + 18)`–`(gx + 78, gy + 70)`, size 17, vertical
  offset 0.25, follows the mouse like the inventory player (`inventories.md` § 5.1).
- No sound on the client side.

---

## 3. Workstation blocks

### 3.1 Anvil — see `inventories.md` § 5.9

Complement: slots 0, 1, 2 then inventory 3..38; property 0 = cost. The name field sends
`rename_item`. "Too expensive": `container.repair.expensive`. Sounds (anvil used, broken):
server (level events).

### 3.2 Grindstone (`grindstone`, type 15)

- `grindstone.png`. Slots 0 `(49, 19)`, 1 `(49, 40)` (accept a damageable or enchanted item),
  2 result `(129, 34)`, inventory 3..38.
- Error: if 0 or 1 non-empty and 2 empty → `container/grindstone/error` 28 × 21 at
  `(gx + 92, gy + 31)`.
- The official client recomputes the result locally (same menu); the server sends it anyway.
  Recommendation: do not predict, display the received slot 2.
- Shift-click: 2 → 3..38 reverse; 0/1 → 3..38; inventory → 0..1 if one of the two is empty,
  otherwise inventory ↔ hotbar.

### 3.3 Stonecutter (`stonecutter`, type 24)

- `stonecutter.png`, title at `(8, 5)`. Slots 0 input `(20, 33)`, 1 result `(143, 33)`,
  inventory 2..37. Property 0 = chosen recipe.
- **Recipe list**: built on the client side from the "stonecutter" section of
  `update_recipes`: ordered list of entries { ingredient, result display }. Visible
  recipes = entries whose ingredient accepts the item of slot 0, in the received order. The
  list is recomputed (and the selection reset to −1, the scroll to 0) when the **item** of
  slot 0 changes (not the quantity).
- 4 × 3 grid from `(gx + 52, gy + 14)`: element j (= index − start) at
  `x = gx + 52 + 16(j mod 4)`, `y = gy + 16 + 18⌊j/4⌋`; 16 × 18 background drawn at `(x, y − 1)`:
  `container/stonecutter/recipe_selected` if it is the chosen recipe, `recipe_highlighted` on
  hovering `[x, x+16) × [y, y+18)`, otherwise `recipe`; then the item (first item of the
  result display) at `(x, y)`. Tooltip: this item.
- Click in `[gx + 52 + 16(j mod 4), +16) × [gy + 14 + 18⌊j/4⌋, +18)` (list displayed,
  i.e. slot 0 non-empty): if the index ≠ chosen recipe and is valid → interface
  sound `ui.stonecutter.select_recipe`, then `container_button_click` (window, index).
  A click on the already chosen recipe sends nothing. The result comes from the server.
- Scrolling (if > 12 recipes): `hidden rows = ⌈n/4⌉ − 3`; handle
  `container/stonecutter/scroller` (or `scroller_disabled`) 12 × 15 at
  `(gx + 119, gy + 15 + ⌊41 × s⌋)`, s ∈ [0, 1]. Wheel: `s = clamp(s − Δ/rows, 0, 1)`;
  drag (click in `[gx+119, +12) × [gy+9, +54)`): `s = clamp((y − gy − 14 − 7.5) / 39, 0,
  1)`. Start = `⌊s × rows + 0.5⌋ × 4`.
- Shift-click: 1 → 2..37 reverse; 0 → 2..37; inventory → 0 if a stonecutter entry accepts
  the item, otherwise inventory ↔ hotbar.

### 3.4 Loom (`loom`, type 18)

- `loom.png`, title at `(8, 4)`. Slots 0 banner `(13, 26)`, 1 dye `(33, 26)`, 2 pattern
  `(23, 45)`, 3 result `(143, 57)`, inventory 4..39. Property 0 = chosen pattern.
- Empty sprites (drawn if the slot is empty): `container/slot/banner`, `container/slot/dye`,
  `container/slot/banner_pattern`.
- **Patterns offered** (client computation, if banner and dye present): empty pattern slot → the
  patterns of the `banner_pattern` tag `no_item_required` (tag order received through `update_tags`);
  otherwise the list of the `provides_banner_patterns` component of the pattern item. A single
  proposal → chosen automatically.
- Display of the grid if banner and dye present, banner with fewer than 6 layers and
  non-empty list (re-evaluated when one of the three inputs changes).
- 4 × 4 grid, 14 × 14 buttons at `(gx + 60 + 14c, gy + 13 + 14r)`: sprite
  `container/loom/pattern_selected` (chosen), `pattern_highlighted` (hover; tooltip
  `<pattern key>.<dye colour>`, white by default), otherwise `pattern`; then
  thumbnail: grey rectangle (grey dye colour) 5 × 10 at `(+4, +2)` and, on top, the
  pattern (sprite of the banners atlas, source part u ∈ [0, 21/64], v ∈ [1/64, 41/64]).
- Click on a button of valid index → interface sound `ui.loom.select_pattern`, then
  `container_button_click` (index). (Clicking the chosen pattern again still sends the packet.)
- Preview: if the result exists and the banner has < 6 layers → banner (standing banner flag
  model, base colour of the result, layers of the result) in `(gx + 141, gy + 8)`–
  `(gx + 161, gy + 48)`. Banner with 6 layers → `container/loom/error` 26 × 26 at
  `(gx + 138, gy + 52)`.
- Scrolling (> 16 patterns): `hidden rows = ⌈n/4⌉ − 4`; handle `container/loom/scroller`
  (or `scroller_disabled`) 12 × 15 at `(gx + 119, gy + 13 + ⌊41 s⌋)`; wheel and drag
  like the stonecutter (area `[gx+119, +12) × [gy+9, +56)`, `s = (y − gy − 13 − 7.5) / 41`);
  first row = `⌊s × rows + 0.5⌋`.
- A click outside the image (strict rectangle 176 × 166) counts as "outside the screen".
- Shift-click: 3 → 4..39 reverse; 0..2 → 4..39; inventory → banner / dye (tag
  `loom_dyes` + component `dye`) / pattern (tag `loom_patterns` + `provides_banner_patterns`)
  depending on the item, otherwise inventory ↔ hotbar.

### 3.5 Cartography table (`cartography_table`, type 23)

- `cartography_table.png`, title at `(8, 4)`. Slots 0 map `(15, 15)` (item with `map_id`),
  1 addition `(15, 52)` (paper, empty map, glass pane), 2 result `(145, 39)`, inventory 3..38.
- Map data: that known locally through `map_item_data` (locked, scale).
- Error `container/cartography_table/error` 28 × 21 at `(gx + 35, gy + 31)` if: map
  locked and addition paper or glass pane; or addition paper and (map outside tag `extendable_maps` or
  scale ≥ 4). In these cases "blocked".
- Preview (128 × 128 map rendering scaled by s, origin at the top left):
  - paper and not blocked: `container/cartography_table/scaled_map` 66 × 66 at `(gx+67, gy+13)`,
    map at `(gx + 85, gy + 31)` s = 0.226;
  - empty map: `duplicated_map` 50 × 66 at `(gx + 83, gy + 13)`, map at `(gx + 86, gy + 16)`
    s = 0.34; new layer; `duplicated_map` at `(gx + 67, gy + 29)`, map at
    `(gx + 70, gy + 32)` s = 0.34;
  - glass pane: `map` 66 × 66 at `(gx + 67, gy + 13)`, map at `(gx + 71, gy + 17)` s = 0.45, then
    `locked` 10 × 14 at `(gx + 118, gy + 60)`;
  - otherwise: `map` and map as above without the padlock.
- Shift-click: 2 → 3..38 reverse; 0/1 → 3..38; inventory: map → 0; paper/empty map/glass pane
  → 1; otherwise inventory ↔ hotbar.

### 3.6 Smithing table (`smithing`, type 21)

- `smithing.png`, title at `(44, 15)`. Slots 0 template `(8, 48)`, 1 base `(26, 48)`, 2 addition
  `(44, 48)`, 3 result `(98, 48)`, inventory 4..39. Property 0 = recipe error.
- Acceptance of slots 0..2: item sets `smithing_template`, `smithing_base`,
  `smithing_addition` received in `update_recipes`.
- Error (property 0 = 1): `container/smithing/error` 28 × 21 at `(gx + 65, gy + 46)`;
  tooltip `container.upgrade.error_tooltip` on hover.
- **Cycling empty-slot icons** (drawn if the slot is empty): each slot keeps a
  list, a tick counter and an index; on each screen tick, if the list changed →
  index 0; if non-empty, `counter += 1` and every 30 ticks the index advances. Drawing: if the
  list has more than one icon and `counter ≥ 30`, fade `α = min(counter mod 30 + partial, 4) / 4`:
  old icon at `1 − α` then new at `α`; otherwise new icon opaque. 16 × 16 sprites.
  - slot 0: `container/slot/smithing_template_armor_trim`,
    `container/slot/smithing_template_netherite_upgrade`;
  - armor trim template in slot 0: base = `helmet, chestplate, leggings, boots`; addition =
    `ingot, redstone_dust, lapis_lazuli, quartz, diamond, emerald, amethyst_shard`;
  - netherite upgrade template: base = `helmet, sword, chestplate, pickaxe, leggings,
    axe, boots, hoe, shovel, nautilus_armor, spear`; addition = `ingot`;
  - other item or slot 0 empty: empty lists (prefix `container/slot/`).
- Help tooltips (wrap width 115): slot 0 empty and hovered →
  `container.upgrade.missing_template_tooltip`; template present and slot 1 / 2 empty and hovered →
  `item.minecraft.smithing_template.<armor_trim|netherite_upgrade>.base_slot_description` /
  `.additions_slot_description`.
- **Preview on a dummy**: armor stand (without base plate, with arms) drawn in
  `(gx + 121, gy + 20)`–`(gx + 161, gy + 80)`, scale 25, translation (0, 1, 0), rotation
  X 25° (0.4363 rad) and Z 180°, body oriented at 210°. Updated when slot 3 changes: item
  equippable on the head (rendered as armour if it is one, otherwise item worn on the head), chest, legs,
  feet → corresponding equipment; any other item → left hand.
- Shift-click (common with the anvil): result → inventory reverse; inputs → inventory;
  inventory → inputs if a slot accepts it, otherwise inventory ↔ hotbar.

### 3.7 Enchanting table — see `inventories.md` § 5.8

Complements (animated book, uncertain point 6 of that document):

- State: `open`, `page`, `target page`, `speed`, and their previous values. On
  each screen tick: if the stack of slot 0 changed, draw `target page +=
  (rand(4) − rand(4))` until `|page − target| > 1`. `open ± 0.2` depending on whether one of the
  three costs is non-zero, clamped [0, 1]. `d = clamp((target − page) × 0.4, −0.2, 0.2)`;
  `speed += (d − speed) × 0.9`; `page += speed`.
- Drawing (values interpolated at the partial tick): book model (texture
  `textures/entity/enchantment/enchanting_table_book.png`), scale 40, in
  `(gx + 14, gy + 14)`–`(gx + 52, gy + 45)`; rotations Y 180°, X 25°; translation
  `((1−o) × 0.2, (1−o) × 0.1, (1−o) × 0.25)`; Y `−(1−o) × 90° − 90°`; X 180°; pages
  `p1 = clamp(frac(f + 0.25) × 1.6 − 0.3, 0, 1)`, `p2 = clamp(frac(f + 0.75) × 1.6 − 0.3, 0, 1)`;
  model opening `1.25 × o`; full light.
- Diffuse lighting of all the interface 3D models (book, entities, dummy): two lights
  specific to the interface, (0.2, −1, 1) and (−0.2, −1, 0) normalised, expressed in the screen's
  pose frame (y downwards, scale (s, s, −s)) — never the world's lights. Front face
  of an inventory player: 0.82 (0.74 with the world's).
- Properties 0..9 and `container_button_click` 0..2: unchanged. Enchanting sound: server.

### 3.8 Beacon (`beacon`, type 9)

- `beacon.png`, image **230 × 219**. **No** title or "Inventory". Instead:
  `block.minecraft.beacon.primary` centred on `(62, 10)` and `block.minecraft.beacon.secondary`
  centred on `(169, 10)`, colour `0xFFE0E0E0` **with** shadow.
- Slots: 0 payment `(136, 110)` (item tag `beacon_payment_items`, size 1); standard
  inventory `(36, 137)` = slots 1..36.
- Decorative items (background): netherite ingot `(20, 109)`, emerald `(41, 109)`, diamond
  `(63, 109)`, gold ingot `(86, 109)`, iron ingot `(108, 109)`.
- Effects per tier: 0 = speed, haste; 1 = resistance, jump boost; 2 = strength;
  3 = regeneration.
- 22 × 22 buttons: background `container/beacon/button_disabled` (inactive), `button_selected`,
  `button_highlighted` (hover/focus), otherwise `button`; 18 × 18 icon at `(+2, +2)`.
  - Primaries, tier t = 0..2: `k` effects, `L = 22k + 2(k − 1)`; button c at
    `(gx + 76 + 24c − L/2, gy + 22 + 25t)`. Icon `mob_effect/<effect>`, tooltip name of
    the effect. Active if `t < levels`; selected if its effect = primary.
  - Secondaries (L = 46): regeneration at `(gx + 144, gy + 47)` (active if levels > 3,
    selected if = secondary); "level II" button at `(gx + 168, gy + 47)`, visible
    only if a primary is chosen: icon and name of the primary followed by ` II`, active if
    levels > 3, selected if secondary = primary.
  - Confirm `(gx + 164, gy + 107)`, icon `container/beacon/confirm`, tooltip
    `gui.done`; active if the payment slot is non-empty and a primary is chosen.
  - Cancel `(gx + 190, gy + 107)`, icon `container/beacon/cancel`, tooltip `gui.cancel`.
- Choice (local): click on a non-selected effect button; primary → primary = effect, and
  secondary cleared if it differs from the new primary; secondary → secondary = effect. On
  every property reception, the local primary and secondary are **overwritten** by
  properties 1 and 2. Button states re-evaluated on every tick and after each choice.
- Confirm → **`set_beacon`** (server-bound): primary (`bool` present + `varint` `mob_effect`
  identifier, without +1), secondary (same), then closing the container (`container_close`
  like Escape). Cancel → closing only. Button click sound on every press.
- Shift-click: 0 → 1..36 reverse; from the inventory, if slot 0 is empty, the item is a
  payment and the stack is **exactly 1** → slot 0; otherwise 1..27 → hotbar 28..36, hotbar →
  1..27.

### 3.9 Crafter (`crafter_3x3`, type 7)

- `crafter.png`, centred title. Slots 0..8 grid `(26 + 18x, 17 + 18y)`; standard inventory
  9..44; **45** result `(134, 35)` (non-interactive: neither pickup nor drop). Properties 0..9.
- Disabled grid slot: instead of the content, sprite `container/crafter/disabled_slot`
  18 × 18 at `(x − 1, y − 1)`; accepts nothing.
- Click (non-spectator) on an **empty** grid slot:
  - pickup mode: disabled → enable it; active and carried item empty → disable it;
  - swap mode: disabled and source inventory slot non-empty → enable it;
  - toggling = set the property locally, send **`container_slot_state_changed`**
    (slot, window, new `bool` active state), play `ui.button.click` at the player's
    position, players category, volume 0.4, pitch 1 (enabling) or 0.75 (disabling);
  - then the normal click follows (`container_click`).
- Tooltip `gui.togglable_slot` on an empty active grid slot, carried empty.
- Redstone indicator `container/crafter/powered_redstone` (property 9 = 1) or
  `unpowered_redstone`, 16 × 16 at `(W/2 + 9, H/2 − 48)` (screen coordinates).
- Result computed by the server alone. Shift-click: 0..8 → 9..44 reverse; inventory → 0..8.

### 3.10 Brick-built furnaces

Smoker and blast furnace: identical to the furnace (`inventories.md` § 5.6), with their own
textures and sprites; only the recipe book filter differs.

### 3.11 Ender chest, shulker box

- Ender chest: menu `generic_9x3`, see `inventories.md` § 5.2.
- Shulker box (`shulker_box`, type 20): `shulker_box.png` 176 × 166; slots 0..26
  `(8 + 18c, 18 + 18r)`; standard inventory 27..62. The box's slots refuse items
  that cannot go into a container (shulker boxes). Shift-click like a chest.

---

## 4. Books

Common background: "in-game" screen (translucent gradient of `option-menus.md` § 1.12, no blur),
texture `textures/gui/book.png`, part 192 × 192 at `(L, 2)` with `L = (W − 192)/2`.

### 4.1 Reading (written book)

- **`open_book`** (client-bound): hand (`varint` 0 main, 1 off-hand). The client reads the
  stack held in that hand: component `written_book_content` (pages = text components,
  already resolved by the server; filtered version if filtering is active) or, failing that,
  `writable_book_content` (pages in plain text). Neither → nothing.
- Page text: forced black style `0xFF000000`, no shadow; wrapped at width 114; at most
  14 lines (128 / 9) at `(L + 36, 32 + 9i)`.
- Indicator `book.pageIndicator` (page, max(number of pages, 1)), right-aligned at
  `x = L + 148`, `y = 18`.
- Page buttons 23 × 13: next at `(L + 116, 159)` (`widget/page_forward`,
  `page_forward_highlighted`), previous at `(L + 43, 159)` (`widget/page_backward[_highlighted]`)
  ; visible only if there is a page after / before. Press: interface sound
  `item.book.page_turn` (instead of the click). Page Up / Page Down keys = same buttons.
- "Done" button (`gui.done`) 200 wide at `((W − 200)/2, 196)`.
- **Links**: hovering text with an event → tooltip / cursor; left click on a clickable style:
  `change_page` → go to page N − 1 (clamped); `run_command` → close the container on the server
  side (lectern only) then run the command like a chat click; other events (URL, copy,
  suggestion…) → common handling of text clicks (`chat.md`).

### 4.2 Book and quill (editing)

- **Local** opening: using a book and quill (`use_item` predicted client-side) opens the
  editor on the pages of the `writable_book_content` component (one empty page if none).
- Multiline input area without decoration or background: `x = (W − 114)/2 − 8`, `y = 28`, 122 × 134;
  black text and cursor, no shadow; 1,024 characters and 14 lines max per page. Initial focus
  on the area.
- Page indicator as in reading (real number of pages). Page buttons (with
  `page_turn` sound): previous visible if page > 0; next always visible — on the last page it
  adds an empty page (if < 100) and goes there.
- 98 × 20 buttons at y 196: "Sign" (`book.signButton`) at `(W/2 − 100)` → signing
  screen; "Done" at `(W/2 + 2)` → close then save.
- Save: remove the trailing empty pages, update the local stack, send
  **`edit_book`**: slot (`varint`: chosen hotbar slot, or 40 for the off-hand),
  pages (`varint` n ≤ 100, strings ≤ 1,024), title (`bool` present + string ≤ 32) absent.
  Escape closes **without** sending.
- Signing: black texts without shadow centred in the width 114 from `L + 36`:
  `book.editTitle` at y 34, `book.byAuthor` (username, dark grey) at y 60, `book.finalizeWarning`
  wrapped at 114 at `(L + 36, 82)`. Borderless field centred, black without shadow, 15 characters, at
  `((W − 114)/2 − 3, 50)` 114 × 20, initial focus. 98 × 20 buttons at y 196:
  "Sign and Close" (`book.finalizeButton`, active if the title is not blank) at `W/2 − 100`,
  "Cancel" at `W/2 + 2` (back to the editor keeping the title). Enter in a non-empty
  field = sign. Signing → `edit_book` with the pages (without removing trailing empty pages) and the
  title (edge spaces removed), then closing.

### 4.3 Lectern (`lectern`, type 17)

- Menu: **a single slot** (the book, slot 0, not displayed), no inventory slot.
  Property 0 = displayed page. Screen = reading § 4.1, book taken from slot 0 (refreshed on
  every slot change), page = property 0.
- Page buttons **without** local sound; they do not advance locally: previous →
  `container_button_click` 1, next → 2; `change_page` link to N → button `100 + (N − 1)`
  if different from the current page. The server sends property 0 back.
- If the player can build: "Done" 98 wide at `(W/2 − 100, 196)` and "Take Book"
  (`lectern.take_book`) at `(W/2 + 2, 196)` → button 3. Otherwise "Done" alone (200).
- Closing (Escape, Done): `container_close` then closing. The game is not paused.

---

## 5. Sign editor

- **`open_sign_editor`** (client-bound): position (packed `position`), side (`varint`:
  0 = back, 1 = front). If the block entity at that position is a sign → editor
  (hanging if hanging sign); otherwise warning, nothing.
- Initial state: the 4 lines of the side, reduced to their plain text (filtered version if
  filtering is active). Text colour: if glowing text → text colour of the dye;
  otherwise "dark" colour = dye colour × 0.4 per component (glowing black:
  `0xFFF0EBCC`, case of in-world rendering).
- "In-game" screen (gradient, no pause). Title `sign.edit` (hanging: `hanging_sign.edit`)
  centred at `(W/2, 40)`, white with shadow. "Done" button `(W/2 − 100, H/4 + 144)` 200 × 20.
- Drawing, origin `(W/2, Y)`:
  - standing or wall sign: Y = 90; background `textures/gui/signs/<wood>.png` (24 × 26) translated
    by (0, 27) and scaled 3.9, source part `(0, 0, 24, h)` at `(−12, −13)` with h = 12
    for a wall sign, 26 otherwise; text at scale 0.9765628; line height 10;
    max line width 90;
  - hanging: Y = 125; background `textures/gui/hanging_signs/<wood>.png` (16 × 16) translated by
    (0, −13), scale 4.5, at `(−8, −8)`; text scale 1; line 9; max width 60.
  - Line i at `(−width/2, 10i − 20)` (resp. `9i − 18`), no shadow, colour above.
  - Cursor (standard blinking) on the active line: at the end of the line, "append" cursor
    (underscore); in the middle, opaque vertical bar of the height of a line; selection
    highlighted.
- Input: Up → previous line (modulo 4), Down or Enter → next, cursor at the end of
  the line. Standard text editing (clipboard included) refusing any line whose font width
  would exceed the max width. Each modification **immediately** updates the text of the
  local block entity (the world's sign shows the input live).
- Tick: if the player is absent, the sign removed or the player farther than the block
  interaction range + 4 → close.
- On closing (Escape, Done, forced close): **`sign_update`** (server-bound):
  position, 4 strings (≤ 384 each, plain text of the lines), side (`varint` 0/1 — **after** the
  lines).

---

## 6. Operator screens (brief)

Opened **locally** when a creative player with the required permission level uses the
block (or the minecart); the content comes from `block_entity_data` (command block: a packet
received while the screen is open refreshes the fields). "In-game" screens.

| screen | packet sent | fields |
|---|---|---|
| command block | `set_command_block` | position; command (string ≤ 32,767); mode (`varint`: 0 sequence, 1 repeating, 2 impulse); options byte: 1 track output, 2 conditional, 4 always active |
| command minecart | `set_command_minecart` | entity (`varint`); command; track output (`bool`) |
| structure block | `set_structure_block` | position; action (`varint`: 0 update, 1 save, 2 load, 3 detect size); mode (`varint`: 0 save, 1 load, 2 corner, 3 data); name; offset x, y, z (3 × `i8`, −48..48); size (3 × `i8`, 0..48); mirror (`varint`: 0 none, 1 left-right, 2 front-back); rotation (`varint` 0..3); data (string ≤ 128); integrity (`f32`, 0..1); seed (`varlong`); options byte: 1 ignore entities, 2 show air, 4 show box, 8 strict |
| jigsaw block | `set_jigsaw_block` | position; name, target, pool (3 identifiers); final state (string); joint (string `rollable` / `aligned`); selection priority, placement priority (2 × `varint`) |
| jigsaw block, "Generate" | `jigsaw_generate` | position; levels (`varint`); keep jigsaws (`bool`) |
| test block | `set_test_block` | position; mode (`varint`: 0 start, 1 log, 2 fail, 3 accept); message (string) |
| test instance | `test_instance_block_action` | position; action (`varint`: 0 init, 1 query, 2 set, 3 reset, 4 save, 5 export, 6 run); data: test (`bool` + identifier as string), size (3 × `varint`), rotation (`varint`), ignore entities (`bool`), status (`varint`, always 0 "cleared"), error message (`bool` + text component, always absent) |

Command block, main layout: title `advMode.setCommand` centred y 20; label
`advMode.command` at `(W/2 − 149, 40)` grey `0xFFA0A0A0`; command field `(W/2 − 150, 50)`
300 × 20 (32,500 characters, command suggestions through `command_suggestion`); buttons for
mode / condition / trigger 100 × 20 at y 165 (`W/2 − 154`, `W/2 − 50`, `W/2 + 54`);
"Done" `(W/2 − 154, H/4 + 132)` and "Cancel" `(W/2 + 4, H/4 + 132)`, 150 × 20; previous
output and O/X tracking button. Implementation: `op_screens.rs` (screens), `tech_blocks.rs`
(data, boxes, F3+I).

### 6.1 Command block and minecart

- On opening, empty command field, default modes (impulse, unconditional, needs
  signal); "Done", O/X and the three modes **inactive** until the block's `block_entity_data`
  (command, output, modes, active). O/X read from the client's block entity (true by default).
- Output field: the output if tracked, otherwise "-"; the label `advMode.previousOutput`
  is only drawn if the field is not empty, at `(W/2 − 149, Youtput − 10)`.
- Enter always validates (even with "Done" inactive). Escape closes without sending anything.
- Minecart (`command_block_minecart`, entity interaction): same screen, output at y 150, without
  the three mode buttons; command and output read from the entity data (indices 13
  string, 14 component); sends `set_command_minecart`. Non-operator player: the interaction
  "passes".
- Closed if the block is no longer a command block or if the minecart disappears.
- Suggestions: same rules as the chat (`commands.md`) but input always read as a
  command, leading "/" optional; request to the server with the field's text as is;
  list **under** the field (top y 71, seven lines at most, background `0x80000000`), usage lines
  from y 72; Tab without a displayed list moves to the next component; Escape with a list
  only hides it. Argument colours and ghost text in the field.

### 6.2 Structure block

Title y 10. Fields: name `(W/2 − 152, 40)` 300 (128 chars, identifier characters only, `:`
once before any `/`), offset x/y/z at y 80 (`W/2 − 152`, `− 72`, `+ 8`, 80 wide, 15
chars), size x/y/z at y 120, integrity `(W/2 − 152, 120)` and seed `(W/2 − 72, 120)`, data
`(W/2 − 152, 120)` 240. Buttons: mode `(W/2 − 154, 185)` 50 (save → load → corner;
"data" only with Alt), rotations 0/90/180/270 at y 185 (`W/2 − 102`, `− 61`, `+ 21`,
`+ 62`, 40; the current one inactive), mirror `(W/2 − 20, 185)` 40 (`mirror.*`), action
save/load `(W/2 + 104, 185)` 50, detect `(W/2 + 104, 120)`, strict `(W/2 + 104, 120)`,
entities `(W/2 + 104, 160)`, air / box `(W/2 + 104, 80)`. Visible: save = name, offset,
size, save, detect, entities, air; load = name, offset, integrity, seed, load,
entities, strict, mirror, rotations, box; corner = name; data = data. Grey labels (on the
right aligned to `W/2 + 154`) and line `structure_block.mode_info.*` at `(W/2 − 153, 174)`.
Mode, mirror, rotation, entities, strict, air, box apply immediately to the client's
block entity (the in-game box follows); Cancel / Escape puts them back. Unreadable values: 0
(coordinates), 1 (integrity), 0 (seed). Integrity displayed with 1 to 4 decimals.

In-game box (creative operator or spectator, block within 96 blocks): in save mode, or
in load mode with "box"; corner and size computed from offset, mirror and rotation;
edges `0xFFE5E5E5`, the three edges of the origin corner tinted (x red, y green, z blue);
zero size on an axis: nothing. Save with "air": small cubes centred on air (light
blue, 0.1 + 0.1 side), structure void (pink), barriers (red), light
(yellow).

### 6.3 Jigsaw block, test block, test instance

- Jigsaw (no title): pool, name, target (y 20, 55, 90; 300 wide, 128 chars), final state
  (y 125, 256 chars), selection and placement priorities (y 160, 98 wide, 3 chars, with
  tooltips); joint `(W/2 + 54, 160)` only if the block faces up or down
  (default "aligned" when horizontal, "rollable" when vertical); levels slider
  0..20, "Keep Jigsaws: ON" and "Generate" at y 185. "Done" and "Generate"
  inactive as long as one of the three identifiers is invalid; Generate = Done then
  `jigsaw_generate`.
- Test block: message field `(W/2 − 152, 80)` 240 hidden in start mode; mode
  `(W/2 − 154, 185)` 50; line `test_block.mode_info.*` at y 174.
- Test instance: identifier `(W/2 − 158, 40)` 316; description area 316 × 72 at y 70;
  size ×3, rotation, entities at y 160 (five columns); "Reset" and "Save" at
  y 185 (Save active if rotation 0 and identifier valid); "Run", "Done",
  "Cancel" at y 210. On opening: action "init", then "query" on each modification
  of the identifier; area empty until the answer `test_instance_block_status` (component +
  optional size), which replaces all its text with "failed: <error>" (error carried by
  the block AT THE TIME of the answer, red by default, its own colours kept) then an empty
  line, then the status; invalid identifier: all the text replaced by
  `test_instance.description.invalid_id` in red. Size read clamped 1..48 (1 otherwise).
- Description area: frame `widget/text_field`, `text_field_highlighted` when it has
  focus (click on it without sound, or tabbing between the identifier and size X); text wrapped
  at 308, margin 4, cut at the frame minus one pixel; content = lines × 9 + 8; beyond 72,
  6 px scrollbar ON THE RIGHT outside the frame (x + 316), handle
  `clamp(72 × 72 / content, 32, 64)`; wheel over the area (bar included) and up and
  down arrows with focus: 9 px per notch; an arrow that no longer scrolls moves the focus.
  The scroll is not readjusted when the text changes (like the game).
- Test instance in game (below): box offset by (margin, 1 + margin, 1 + margin), size
  rotated; status beam; error markers.

#### Test instance in the world, `/test pos`

- Synchronised registry `test_instance` (configuration): per test, `required` (true by default)
  and `padding` (0 by default, 0..128). Content omitted (base pack known): only `always_pass`,
  optional and without padding. Absent or unknown test: required, without padding.
- Block data: `data.status` (`cleared`, `running`, `finished`), `data.error_message`,
  list `errors` (`pos` as an array of 3 integers, `text` component).
- Beacon beam from the block, a single section 2048 high, same animation and same
  widening in the distance as the beacon (normal radius through a spyglass): nothing if cleared, grey
  808080 while running, green 00FF00 finished without error, red FF0000 error of a required test,
  orange FF8000 error of an optional test. Visible to all players.
- Range: the block is rendered (box, beam, markers) if its centre is within render
  distance × 16 flat OR within 96 blocks in 3D (the box stays reserved to creative
  operators and spectators; the structure block keeps only the 96 limit).
- Error marker: solid red cube alpha ⌊0.375 × 255⌋ inflated by 0.02 around the block, centred
  white plain text, scale 0.16, at (0.5, 1.2, 0.5) of the block, always in front.
- `game_test_highlight_pos` (two packed positions: absolute, relative to the test): solid cube
  `0x6000FF00` inflated by 0.02 on the absolute position and relative "x, y, z" text
  (same style and place) for 10 s; a new mark on the same block replaces the old one;
  everything is cleared on world change (connection, respawn).

### 6.4 F3+I (recreate copy)

Operator (level ≥ 2) without Shift: request to the server (`block_entity_tag_query`: transaction,
position; `entity_tag_query`: transaction, entity; answer `tag_query`: transaction, optional
NBT; only the last transaction is served). Block: `/setblock x y z state{…}` with the
NBT in compact SNBT (sorted keys, no spaces, quotes only when necessary). Entity:
`/summon id x y z {…}` (positions with two decimals, "round half up") with the NBT
without `UUID` or `Pos` in "pretty" SNBT (`key: value`, `, ` between elements, keys in the iteration
order of the game's hash table). With Shift, or without permission: client data
(without permission: no data). Message `debug.inspect.server.*` / `client.*`.

---

## 7. Uncertain points

1. **Result prediction**: merchant (predicted by the official client, § 1.2) to be done;
   grindstone and loom (recomputed locally by the official client) left to the server
   here — a result may appear one frame later than in the official game.
2. **Recipe sets**: stonecutter (`update_recipes`, stonecutter section) and smithing (`smithing_*`
   sets); without them the stonecutter's list is empty and the smithing table's shift-click
   prediction wrong. Format of a result display ("slot display"): refer to the recipe
   book spec when it exists; in the meantime, only handle the "stack" display.
3. **3D models in the interface**: mount entity, smithing dummy, loom banner,
   enchanting book — same camera and lighting questions as the inventory
   player (`inventories.md` § 6, point 6). The existing entity engine should serve.
4. **Map rendering** in the cartography table: depends on the maps spec (colours,
   decorations); the scale and position above suffice for the frame.
5. **Multiline text of the book and quill**: automatic line wrapping, line-by-line
   cursor movement and the 14-line limit — reuse the chat field; the exact
   behaviour of the official multiline field is not noted here.
6. **Sign lines**: the width limit (90 / 60) applies in the game's font; the official
   screen applies no additional visual cut or truncation.
7. **Merchant offer buttons**: the "standard button without label" rendering is deduced; an
   official capture would confirm the absence of text and the disabled state.
8. The mouse button clicks of the decompiled client follow a numbering 1 = left;
   brought back here to "left click".
