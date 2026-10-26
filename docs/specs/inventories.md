# Inventories and containers (26.3, protocol 777)

Source: reading of the decompiled client (kept outside the repository), reworded. Prerequisites: `hud.md` (layers,
sprites, item in a slot, decorations § 3.5), `items.md` and `item-components.md` (stacks,
components), `text.md` (font, shadow).

Conventions: coordinates in interface units; `(gx, gy)` = top-left corner of the screen image
(§ 4.1); a slot is designated by the top-left corner of its 16 × 16 item area,
relative to `(gx, gy)`. ARGB colors. `rect(x, y, w, h)`. "Partial sprite" = portion
`(u, v, w, h)` (in sprite pixels) of a sprite of size `(W, H)` pasted at `rect(x, y, w, h)`
without scaling.

---

## 1. Packets

All in the play phase. "Window id" = `varint` (0 = player inventory, always
implicitly open; the server numbers the other windows 1, 2, …).

### 1.1 `open_screen` (client-bound)

| field | encoding |
|---|---|
| window | `varint` |
| menu type | `varint`, id in the `menu` registry (table below) |
| title | text component (NBT) |

`menu` registry (fixed order):

| id | name | screen | id | name | screen |
|---|---|---|---|---|---|
| 0 | `generic_9x1` | 1-row chest | 13 | `enchantment` | enchanting table |
| 1 | `generic_9x2` | 2-row chest | 14 | `furnace` | furnace |
| 2 | `generic_9x3` | chest (single, barrel…) | 15 | `grindstone` | grindstone |
| 3 | `generic_9x4` | 4 rows | 16 | `hopper` | hopper |
| 4 | `generic_9x5` | 5 rows | 17 | `lectern` | lectern |
| 5 | `generic_9x6` | double chest | 18 | `loom` | loom |
| 6 | `generic_3x3` | dispenser / dropper | 19 | `merchant` | merchant |
| 7 | `crafter_3x3` | crafter | 20 | `shulker_box` | shulker box |
| 8 | `anvil` | anvil | 21 | `smithing` | smithing table |
| 9 | `beacon` | beacon | 22 | `smoker` | smoker |
| 10 | `blast_furnace` | blast furnace | 23 | `cartography_table` | cartography table |
| 11 | `brewing_stand` | brewing stand | 24 | `stonecutter` | stonecutter |
| 12 | `crafting` | crafting table | | | |

Client handling: creates the menu of the type (empty slots and properties, state 0, empty carried item)
with the received id, makes it the player's **current menu**, then opens the associated screen (which
closes the previous screen without sending anything). The content then arrives through
`container_set_content`. Type with no known screen: warning, nothing is opened. The screen
of a type not handled here may, in a first version, fall back to a generic screen (uncertain
point § 6).

### 1.2 `container_close`

- Client-bound: `varint` window. The client closes **without sending back** a packet: the current menu
  becomes the inventory again (window 0) and the screen is closed. The received value is not compared with
  the open window.
- Server-bound: `varint` window = id of the current menu (0 if it is the player's
  inventory). Sent on any client-side closing (Escape, inventory key, death), then the same
  local closing. The carried item stays displayed until the server correction: it is the server
  that puts it back in the inventory (or drops it if there is no room left) and sends the slots back.

### 1.3 `container_set_data` (client-bound)

| field | encoding |
|---|---|
| window | `varint` |
| property | `i16` |
| value | `i16` (signed: values > 32767 come back truncated to 16 bits) |

Ignored if the window is not the current menu. Otherwise the property at the given index takes the
value. Meaning per menu:

| menu | indices |
|---|---|
| furnace, blast furnace, smoker | 0 remaining burn time (ticks); 1 total duration of the fuel in use; 2 cooking elapsed; 3 total cooking time |
| brewing stand | 0 remaining brewing time; 1 remaining fuel (0..20); 2 total brewing duration; 3 total fuel |
| enchantment | 0..2 level cost of the 3 options; 3 seed of the names (16 bits); 4..6 offered enchantment index (id in the `enchantment` registry, −1 = none); 7..9 level of this enchantment (−1 = none) |
| anvil | 0 repair cost in levels |

### 1.4 `set_cursor_item` (client-bound)

Optional stack (same encoding as the stacks of `container_set_slot`). Replaces the carried item
of the **current menu**, unless the open screen is the creative inventory (ignored).

### 1.5 Reminder: state number

`container_set_content` and `container_set_slot` carry a state number (`varint`) that the
client remembers in the menu concerned (window 0 → inventory menu, even if another menu is
open; other window → only if it is the current menu). The client never increments it:
it sends back unchanged the last one received in each `container_click`. A stale number triggers
a full resend of the content on the server side.

### 1.6 `container_click` (server-bound)

| field | encoding |
|---|---|
| window | `varint` |
| state number | `varint` |
| slot | `i16` (−999 = outside the screen; −1 = no slot) |
| button | signed `i8` |
| mode | `varint`: 0 pickup, 1 quick move, 2 swap, 3 clone, 4 throw, 5 drag, 6 pickup all |
| modified slots | `varint` n ≤ 128, then n × { slot `i16`, hashed stack } |
| carried item | hashed stack |

Hashed stack:

- `bool` present; if false: empty stack.
- Otherwise: item (`varint`, id in the `item` registry), quantity (`varint`), then the
  **hashed component patch**:
  - `varint` a ≤ 256, then a × { component type (`varint`, `data_component_type` registry),
    fingerprint big-endian `i32` } — components **added or redefined** relative to the
    item's prototype;
  - `varint` r ≤ 256, then r × component type (`varint`) — components **removed**.
  - The order of the entries is free (the server compares sets).

Client-side construction (§ 2): copy all the menu's slots, apply the click's prediction,
then list each slot whose stack differs from the copy (item, quantity or components)
with its new hashed stack; "carried item" = carried item after prediction.

What the server does with it: it replays the click, then **remembers** the hashed stacks as "what
the client believes it has"; any slot whose real state does not match (different fingerprint, quantity
or item) is sent back to it via `container_set_slot` / `set_cursor_item`. A wrong
fingerprint therefore rejects nothing: it only costs a slot resend. An out-of-bounds slot or
a window that is not the one open on the server side: packet ignored.

### 1.7 Component fingerprint

Fingerprint = CRC32C (Castagnoli, initial value `0xFFFFFFFF`, reflected, final xor
`0xFFFFFFFF`) of a byte sequence obtained by walking the **persistent form** of the component's
value (that of the data files and commands, not the network form), with
registry references written as text identifiers (`"minecraft:sharpness"`). The
`i32` value sent is the CRC itself.

Each node of the persistent form has its own fingerprint H (4 bytes). The bytes of a node
(all integers **little-endian**):

| node | bytes |
|---|---|
| empty | `01` |
| byte | `06`, the byte |
| 16-bit integer | `07`, 2 bytes |
| 32-bit integer | `08`, 4 bytes |
| 64-bit integer | `09`, 8 bytes |
| float 32 | `0A`, IEEE bits (4 bytes) |
| float 64 | `0B`, IEEE bits (8 bytes) |
| string | `0C`, length in UTF-16 units (`i32`), then each UTF-16 unit (2 bytes) |
| boolean | `0D`, `00` or `01` |
| list | `04`, H of each element in order (4 bytes each, little-endian), `05` |
| map (object, record) | `02`, then for each entry H(key) ‖ H(value), entries **sorted** by H(key) as an increasing unsigned integer, then by H(value), `03` |
| byte array | `0E`, the bytes, `0F` |
| `i32` array | `10`, each integer (4 bytes), `11` |
| `i64` array | `12`, each integer (8 bytes), `13` |

H(node) is itself the CRC32C of these bytes, written as 4 little-endian bytes when it goes into
a parent. Record keys are strings. Examples: `damage` → 32-bit integer; "unit"
component (no value, e.g. `unbreakable` in its field-less form) → empty map
(`02 03`); `custom_name` → string if the text is a literal with no style or children, otherwise the
map of the text component; `enchantments` → map { id → 32-bit integer level }. An optional
field that is absent or equal to its omitted default value does not appear.

Recommendation: compute the common components exactly (`damage`, `custom_name`,
`enchantments`, `repair_cost`, `lore`…); for the others, an approximate fingerprint
only causes a slot resend by the server (see § 6).

### 1.8 `container_button_click` (server-bound)

`varint` window, `varint` button. Used by the enchanting table (button 0..2), and by
other screens out of scope (stonecutter, loom, lectern…). No local prediction
of the result.

### 1.9 `rename_item` (server-bound, anvil)

String (≤ 32767; the field limits it to 50 characters). Sent at each change of the name (§ 5.9).

### 1.10 `container_slot_state_changed` (server-bound, crafter — memo)

`varint` slot, `varint` window, `bool` new state. Out of scope here.

---

## 2. Clicks: what the client predicts and sends

Every click follows the same path: if the screen's window id ≠ current menu →
ignored (warning). Otherwise: copy of the slots, **local application** of the click logic
below (the same as the server's), computation of the modified slots, sending of
`container_click` (§ 1.6). The server then corrects whatever diverges.

### 2.1 Definitions

- Empty stack = quantity 0 or air item. "Same item" = same id; "same stack" =
  same item **and** equal components.
- Max size of a stack = component `max_stack_size` (1 to 99). Max size of a slot = min(the slot's
  max size — 64 by default, 1 for armor/potion/item to enchant, see § 5 — , the stack's
  max size).
- Each slot has the rules "can receive P" (default yes) and "can be taken" (default yes).
  Result slot (crafting table, inventory, furnace, anvil): can receive nothing.
- Safe insertion of P (at most n items) into a slot: if P is non-empty and the slot accepts it:
  `t = min(n, |P|, max_slot(P) − slot_quantity)`; if t ≤ 0 nothing; empty slot → it receives t
  items taken from P; slot containing the same stack → +t, P −t. Returns the rest of P.
- Removal (a items requested, cap m): refused if the slot cannot be taken; refused if the
  slot is not modifiable (cannot be taken **or** cannot receive its own content, case
  of result slots) and `m < quantity`; otherwise removes `min(a, m)`.

### 2.2 Mode 0 "pickup": left click (button 0) / right click (button 1)

Slot −999 (click outside the screen image): carried item non-empty → button 0: everything is thrown,
carried empty; button 1: a single item thrown. (Client side, "throw" = only swinging the arm,
no entity created.)

Slot ≥ 0, C = the slot's stack, P = carried item:

1. Some items intercept the click first (bundle: insertion/extraction in the bundle) —
   out of scope, see § 6.
2. C empty: if P non-empty → safe insertion of P (button 0: all, button 1: 1 item).
3. C non-empty and slot can be taken:
   - P empty: remove `|C|` (button 0) or `⌈|C|/2⌉` (button 1) → becomes P.
   - P non-empty and the slot accepts P: same stack → safe insertion (all or 1); otherwise, if
     `|P| ≤ max_slot(P)` → **swap** C ↔ P.
   - P non-empty, the slot refuses P (result slot) but same stack: remove `|C|` with cap
     `max(P) − |P|` (refused as a whole if the result does not fit entirely); P grows accordingly.

Taking from a crafting result slot (inventory or crafting table): the client also removes 1
item from each non-empty slot of the grid (recipe remainders — buckets… — are left to the
server). The result itself is **never** recomputed on the client side: it arrives through
`container_set_slot`.

### 2.3 Mode 1 "quick move" (shift + left or right click, button 0/1)

Slot ≥ 0 and can be taken. Repeat: move the slot's stack according to the menu's rule (§ 5), as long as
something moved and the slot still contains the same item. Slot −999 with shift:
treated as a normal click outside the screen (mode 0).

Move to a range [start, end) (forward or reverse direction):
1. If the stack is stackable (max > 1): walk the range; for each slot containing the same
   stack, top up to `max_slot`.
2. If items remain: the first **empty** slot of the range (in the same direction) that accepts the
   stack receives `min(rest, max_slot)`. Only one empty slot is filled.

The move fails (stops) if nothing moved or if the quantity is unchanged.

### 2.4 Mode 2 "swap" (keys 1-9, F)

Hovering a slot with an empty carried item; button = 0..8 (hotbar key: player inventory
slot 0..8) or 40 (offhand, F key). Also triggered by a mouse button assigned to these keys.
S = inventory stack, C = the slot's stack:
- S and C empty: nothing.
- S empty: if the slot can be taken, C goes into the inventory, the slot becomes empty.
- C empty: if the slot accepts S: if `|S| > max_slot(S)`, the slot receives `max_slot` items taken
  from S; otherwise all of S goes into the slot.
- Both non-empty, slot can be taken and accepts S: if `|S| > max_slot`, the slot receives
  `max_slot` of S and C is stored elsewhere in the inventory (or dropped); otherwise simple swap.

### 2.5 Mode 3 "clone" (middle click, button 2; "pick block" key on hover, button 0)

Only if the player has infinite materials (creative), carried item empty, slot ≥ 0 non-empty:
carried item = copy of the stack with quantity = its max size. Outside creative: the packet goes out
anyway but nothing is predicted.

### 2.6 Mode 4 "throw" (Q key on hover: button 0; Ctrl+Q: button 1)

Carried item empty, slot ≥ 0. Button 0: removes 1 item; button 1: the whole stack, then repeats
as long as the slot still contains the same item. On the client side a limiter blocks throw spam
(counter +20 per throw, −1 per tick, blocked above 1280). A left click outside the screen with
empty carried item **and** without shift also sends mode 4 (slot −999, button 0): no effect.

### 2.7 Mode 5 "drag" (quick craft), 3 phases

Button mask = `phase | (type << 2)`, phase 0 start, 1 adding a slot, 2 end; type 0 =
spread (left click), 1 = one per slot (right click), 2 = full stack (middle click, creative
only).

Screen side: left/right/middle-creative press **with a non-empty carried item** on a slot (or
outside the screen) → drag started, nothing sent. While dragging, each hovered slot is added
if: carried non-empty, `|P| > number of slots already kept` (or type 2), slot empty or same stack
not full, slot that accepts P. On release of the same button:
- no slot kept → ordinary click (mode 0, or clone if it is the middle button) on the
  slot under the cursor;
- otherwise sending of **three kinds** of packets in order: phase 0 (slot −999), one phase 1 per
  kept slot (set order), phase 2 (slot −999). Each packet carries its own
  prediction (only phase 2 modifies slots).
- Release of a **different** button during the drag: drag cancelled, nothing sent.

Menu logic (prediction and server): phase out of sequence (not 0 → 1 → 1… → 2) or carried
empty → everything reset. Phase 0: invalid type (2 without creative) → reset. Phase 2:
- a single slot kept → equivalent to a mode 0 on this slot with button = type;
- otherwise, `rest = |P|`; for each kept slot still valid (add criterion, with `≥` instead of `>`
  for the number of slots): `before` = current quantity (0 if empty),
  `n = min(share + before, min(max(P), max_slot(P)))` with share = `⌊|P| / number of slots⌋`
  (type 0), 1 (type 1), `max(P)` (type 2); the slot receives P with quantity n; `rest −= n − before`.
  Then carried = P with quantity `rest` (in type 2 the starting quantity is not consumed
  differently: the same formula applies).

Rendering during the drag: § 4.6.

Any other click while a drag is in progress on the menu side resets the drag.

### 2.8 Mode 6 "pickup all" (left double-click)

Triggered on **release** of the 2nd left click on the same slot, less than 250 ms after the
1st (same screen, same button). Without shift: send mode 6, button 0, hovered slot. Logic: carried
non-empty and (slot empty or cannot be taken) — in practice the 1st click already picked up the stack —: two
passes over all the slots (button 0: from first to last; 1: in reverse), as long as
`|P| < max(P)`: each slot containing the same stack, that can be taken, gives
`min(its quantity, max(P) − |P|)`; the 1st pass ignores already-full stacks.
The anvil/smithing menu etc. excludes nothing; crafting menus exclude their result slot.

With shift (double shift-click): for each menu slot that can be taken, non-empty, of the **same container**
as the clicked slot and able to receive the last stack moved by shift-click → one mode 1 per
slot (one packet each).

### 2.9 Summary of input → packet

| input | conditions | mode, button, slot |
|---|---|---|
| left/right press, carried empty | on a slot | 0 (or 1 if shift), 0/1, slot |
| left/right press, carried empty | outside the screen | 4, 0/1, −999 |
| middle press, creative, carried empty | on a slot | 3, 2, slot |
| left/right release, carried non-empty, no drag | on a slot / outside the screen | 0 (1 if shift on a slot), 0/1, slot / −999 |
| middle release, carried non-empty | — | 3, 2, slot |
| keys 1-9 / F (or assigned mouse button) | hover, carried empty | 2, 0..8 / 40, slot |
| Q / Ctrl+Q | hover on a non-empty slot | 4, 0 / 1, slot |
| "pick block" key | hover on a non-empty slot | 3, 0, slot |
| left double-click (slot that accepts pickup all: neither crafting result, stonecutter, smithing, cartography; never at the merchant) | release | 6, 0, slot |
| Shift + left double-click (same condition) | release | 1, 0, t for each slot t non-empty, takeable, of the same part (container / inventory / result) and carrying the same stack as the last one moved by Shift-click |
| drag | release | 5 ×(2 + n) |

"Outside the screen" = cursor outside `[gx, gx + width) × [gy, gy + height)` (with the recipe book
open, also outside the book). A press that lands neither on a slot nor outside the screen
(image background, between two slots) does nothing. After a handled press, the next release
is ignored (no double sending).

---

## 3. Opening and closing

- Inventory key (E by default), in game with no screen: if the player is riding a mount whose
  inventory is managed by the server → `player_command` packet "open inventory"
  (out of scope); otherwise **local** opening of the inventory screen (no packet), on
  menu 0. Player with infinite materials (creative): creative inventory screen instead (out of
  scope, § 6).
- Closing: Escape (all container screens) or inventory key (except when an input field
  captures the keyboard: anvil, book search) → server-bound `container_close` (§ 1.2)
  then local closing. Anvil: Escape closes even if the field has focus.
- At each screen tick: if the player is dead or removed → closing as above.
- A container screen does not pause the game.

---

## 4. Common rendering of container screens

### 4.1 Position

- Screen image: `width × height` (default 176 × 166). `gx = (W − width) / 2`,
  `gy = (H − height) / 2` (integer divisions, W, H = interface size).
- With the recipe book visible and `W ≥ 379`: `gx = 177 + (W − width − 200) / 2`.

### 4.2 Drawing order

1. Layer 1: background — full-screen vertical gradient from `0xC0101010` (top) to `0xD0101010` (bottom)
   (no blur: "in game" screen). Then the screen's background texture (§ 5) at
   `rect(gx, gy, width, height)`, source `(0, 0)` of a 256 × 256 texture, and the
   screen's own background elements (arrows, flames, player…).
2. Layer 2: widgets (book button, anvil field), then, translated by `(gx, gy)`:
   - titles (§ 4.3);
   - back highlight of the hovered slot: sprite `container/slot_highlight_back`
     (24 × 24, nine-slice border 4) at `rect(x − 4, y − 4, 24, 24)`;
   - all the active slots, in menu order (§ 4.4);
   - front highlight: `container/slot_highlight_front`, same rectangle.
3. New layer: carried item (§ 4.5).
4. Tooltip (deferred, drawn last, § 4.6).

Hovered slot: first active slot whose `[x − 1, x + 17) × [y − 1, y + 17)` contains the
cursor (relative to `gx, gy`). Recipe book open on a narrow screen: no hovered slot.

### 4.3 Titles

- Title: received text (or fixed for the inventory), at `(titleX, titleY)`, default `(8, 6)`.
- "Inventory" (`container.inventory`) at `(8, height − 94)`.
- Color `0xFF404040`, **no** shadow.

### 4.4 A slot

- Empty stack and "empty slot" sprite defined (§ 5): 16 × 16 sprite at `rect(x, y, 16, 16)`.
- Otherwise: item at `rect(x, y, 16, 16)` (hud.md § 3.4, variant seed `x + y × width`),
  then its decorations (quantity, durability bar, cooldown: hud.md § 3.5). Crafting result
  slots are drawn the same way.

### 4.5 Carried item

Carried non-empty: item and decorations at `(sx − 8, sy − 8)` (`sx, sy` = cursor). During a
drag over more than one slot, the displayed quantity is the planned rest (§ 4.6); rest 0 → the
number "0" in yellow (`§e`) replaces the quantity, the stack itself is not drawn.

### 4.6 Drag preview

For each kept slot (if more than one) that remains valid: veil `0x80FFFFFF` at
`rect(x, y, 16, 16)` under the item, then the stack P with the planned quantity
`min(share + before, max)`; if capped, the quantity is written in yellow. A slot that has become
invalid is removed from the set and the rest recomputed. With **a single** kept slot, this
slot is not drawn at all. Planned rest = `|P| − Σ(n − before)` (type 2: max size of P).

### 4.7 Item tooltip

Conditions: hovered slot non-empty and carried item empty (except items with an image tooltip that
allow it with an item in hand, e.g. bundle).

Lines (in order, each line = text component):
1. Name: the stack's hover name (items.md, § "displayed text") colored by rarity —
   common white, uncommon yellow, rare aqua, epic light purple; an enchanted stack goes up
   one step (common/uncommon → rare, rare → epic); italic if `custom_name`.
2. Lines specific to the item then to the components, in this order, each only if the
   component is present and not hidden by `tooltip_display`: tropical fish, instrument,
   map, bees, container loot, container contents, banner patterns, pot
   decorations, written book, charged projectiles, fireworks, explosion, potion, disc, trim,
   stored enchantments, enchantments, dye color, profile, **lore**, sulfur cube
   contents.
   - Enchantment: translated name, gray (red if curse), followed by ` ` + Roman numeral
     (`enchantment.level.N`) unless level 1 and max level 1. Order: first that of the
     tag `#minecraft:tooltip_order`, then the others.
   - Lore: each line in dark purple italic (style merged under the line's own).
3. Attributes: for each slot group (order: any, mainhand, offhand, hand, feet,
   legs, chest, head, armor, body, saddle), if at least one visible modifier: empty line, then
   `item.modifiers.<group>` in gray, then one line per modifier:
   - displayed amount = amount × 100 for multiplicative operations, × 10 for knockback,
     otherwise as is, format "#.##" (decimal point);
   - base attack modifiers (damage, speed): the player's base is added (1 and 4),
     text ` ` + `attribute.modifier.equals.<op>` in dark green;
   - otherwise > 0: `attribute.modifier.plus.<op>` (blue if attribute is "positive", red otherwise);
     < 0: `attribute.modifier.take.<op>` with the absolute value (colors swapped).
4. "Intangible", "Unbreakable" (if present), sign texts, ominous bottle amplifier, suspicious stew
   effects, block state, entity data; `can_break` /
   `can_place_on` (empty line, header, list).
5. Advanced tooltips (F3+H): `item.durability` (if damaged), the item's id in
   dark gray, `item.components` (number of components) in dark gray.
6. Item disabled by feature options: warning line.

`tooltip_display.hide_tooltip` (outside creative): no tooltip.

Box:
- Width `w` = max width of the lines; height `h` = 10 per line if there are several, 8 if
  there is only one (the 2 px gap under the name is in the drawing, not in `h`). Text tooltips
  of the screens (enchantment hints, smithing, patterns, merchant, recipe book): no
  gap under the 1st line and `h = 10·n − 2`. Button tooltips (beacon): component
  positioning (§ 1.8 of `option-menus.md`), wrapped at 170 px.
- Carried item: only a container's tooltip (bundle) stays displayed when hovering a slot.
- Position: `(sx + 12, sy − 12)`; if `x + w > W`: `x = max(x − 24 − w, 4)`; if
  `y + h + 3 > H`: `y = H − h − 3`.
- Background: sprite `tooltip/background` (nine-slice 100 × 100, border 9) then `tooltip/frame`
  (border 10, stretched center) at `rect(x − 12, y − 12, w + 24, h + 24)`. Component `tooltip_style`
  = identifier `ns:path` → sprites `ns:tooltip/<path>_background` and `_frame`.
- Text: lines at `(x, y)`, `y += 10` (+ 2 after the 1st), base color white, **with**
  shadow. Tooltip images (bundle, map…) are drawn after the texts.

---

## 5. Screens

Slot order = the protocol's slot number. "Standard inventory (top T)" = 27 slots
`(8 + 18c, T + 18r)` (r = 0..2, c = 0..8; inventory slots 9..35), then 9 hotbar slots
`(8 + 18c, T + 58)` (inventory slots 0..8). All textures are
`textures/gui/container/<name>.png`, 256 × 256.

### 5.1 Player inventory (window 0)

- Texture `inventory.png`, 176 × 166. Title `container.crafting` at `(97, 6)`; **no**
  "Inventory" line.
- Slots: 0 result `(154, 28)`; 1..4 2 × 2 grid `(98 + 18c, 18 + 18r)`; 5..8 armor head,
  chest, legs, feet `(8, 8 + 18i)` (max size 1, accepts only what equips in this
  slot, cannot be taken if "binding" enchantment outside creative; slot inactive if the
  player cannot use the slot); 9..44 standard inventory (top 84); 45 offhand
  `(77, 62)`.
- "Empty slot" sprites: `container/slot/helmet`, `chestplate`, `leggings`, `boots`,
  `container/slot/shield` (offhand).
- Shift-click: result → 9..44 in reverse; grid or armor → 9..44; armor piece (outside
  slots 5..8) → its armor slot if empty; offhand item → 45 if empty; inventory
  9..35 → hotbar 36..44; hotbar → 9..35.
- Player: rendering of the local player model in the area `(gx + 26, gy + 8)`–`(gx + 75, gy + 78)`,
  size 30, clipped to this area. Center `(cx, cy)` of the area; `ax = atan((cx − sx)/40)`,
  `ay = atan((cy − sy)/40)`; body orientation `180° + 20·ax`, head `20·ax` in yaw and
  `−20·ay` in pitch (0 when gliding), plus an overall tilt of `20·ay` degrees around
  X; no shadow nor outline, scale forced to 1, anchored at mid-height of the box + 0.0625.
  3D rendering into a texture: uncertain implementation point.
- Recipe book button `(gx + 104, H/2 − 22)`, 20 × 18, sprites `recipe_book/button`
  / `button_highlighted`: book in uncertain points (§ 6).
- Active effects: drawn next to the image (sprites `container/inventory/effect_background*`)
  — uncertain point.

### 5.2 Chests (`generic_9xN`, N = 1..6; double chest = 6)

- Texture `generic_54.png`. Height `114 + 18N`, width 176.
- Background in two pieces: `rect(gx, gy, 176, 18N + 17)` source `(0, 0)`; then
  `rect(gx, gy + 18N + 17, 176, 96)` source `(0, 126)`.
- Title `(8, 6)`; "Inventory" at `(8, height − 94)` = `(8, 20 + 18N)`.
- Slots: 0..9N−1 grid `(8 + 18c, 18 + 18r)`; then standard inventory top `31 + 18N`.
- Shift-click: chest slot → inventory (all slots after the chest) in reverse;
  inventory → chest (forward).
- Shulker box (`shulker_box`): same 3-row layout, different texture — out of
  detailed scope.

### 5.3 Dispenser / dropper (`generic_3x3`)

- `dispenser.png`, 176 × 166. **Centered** title: `x = (176 − text_width) / 2`, y 6.
- Slots 0..8 `(62 + 18c, 17 + 18r)`; standard inventory top 84.
- Shift-click: 0..8 → 9..44 reverse; inventory → 0..8.

### 5.4 Hopper (`hopper`)

- `hopper.png`, 176 × 133. Title `(8, 6)`, "Inventory" `(8, 39)`.
- Slots 0..4 `(44 + 18i, 20)`; standard inventory top 51.
- Shift-click: like the dispenser (0..4 ↔ rest).

### 5.5 Crafting table (`crafting`)

- `crafting_table.png`, 176 × 166. Title at `(29, 6)`.
- Slots 0 result `(124, 35)`; 1..9 grid `(30 + 18c, 17 + 18r)`; inventory top 84.
- Shift-click: result → 10..45 reverse (repeated as long as there is a result); inventory → grid
  1..9, failing that inventory ↔ hotbar; grid → 10..45.
- Book button `(gx + 5, H/2 − 49)`.

### 5.6 Furnace, blast furnace, smoker

- Textures `furnace.png`, `blast_furnace.png`, `smoker.png`, 176 × 166. Centered title.
- Slots 0 ingredient `(56, 17)`; 1 fuel `(56, 53)` (accepts only fuel or bucket;
  bucket max 1); 2 result `(116, 35)` (receives nothing); inventory top 84.
- Flame (if property 0 > 0): `p = clamp(prop0 / d, 0, 1)` with `d = prop1` (200 if 0);
  `k = ⌈13p⌉ + 1`; partial sprite `container/<furnace>/lit_progress` (14 × 14), source
  `(0, 14 − k, 14, k)` at `rect(gx + 56, gy + 36 + 14 − k, 14, k)` (empties from the top).
- Arrow: `q = prop2 / prop3` clamped to [0, 1] (0 if either is 0); `l = ⌈24q⌉`; partial sprite
  `container/<furnace>/burn_progress` (24 × 16), source `(0, 0, l, 16)` at `rect(gx + 79, gy + 34, l, 16)`.
- `<furnace>` = `furnace`, `blast_furnace`, `smoker`.
- Shift-click: result → 3..38 reverse; inventory → 0 if the item smelts, otherwise → 1 if it is a
  fuel, otherwise inventory ↔ hotbar; slots 0/1 → 3..38. "Smelts" / "fuel"
  depend on the recipe property sets sent by the server (uncertain point § 6).
- Book button `(gx + 20, H/2 − 49)`.

### 5.7 Brewing stand (`brewing_stand`)

- `brewing_stand.png`, 176 × 166. Centered title.
- Slots 0..2 bottles `(56, 51)`, `(79, 58)`, `(102, 51)` (max size 1, empty sprite
  `container/slot/potion`); 3 ingredient `(79, 17)`; 4 fuel `(17, 17)` (items with the
  `brewing_fuel` component, empty sprite `container/slot/brewing_fuel`); inventory top 84.
- Fuel: if `total = prop3 > 0`: `l = clamp(⌈18·prop1 / total⌉, 0, 18)`; if `l > 0` partial
  sprite `container/brewing_stand/fuel_length` (18 × 4), source `(0, 0, l, 4)` at
  `rect(gx + 60, gy + 44, l, 4)`.
- If `t = prop0 > 0` and `T = prop2 > 0`:
  - arrow: `l = ⌊28·(1 − t/T)⌋`; if `l > 0`, `container/brewing_stand/brew_progress` (9 × 28),
    source `(0, 0, 9, l)` at `rect(gx + 97, gy + 16, 9, l)`;
  - bubbles: `l = [29, 24, 20, 16, 11, 6, 0][(t / 2) mod 7]`; if `l > 0`,
    `container/brewing_stand/bubbles` (12 × 29), source `(0, 29 − l, 12, l)` at
    `rect(gx + 63, gy + 14 + 29 − l, 12, l)`.
- Shift-click from the inventory: fuel → 4 (or 3 if it also serves as an ingredient and 4 is
  full); ingredient → 3; bottle → 0..2; otherwise inventory ↔ hotbar. From 0..4 → 5..40 reverse.

### 5.8 Enchanting table (`enchantment`, simplified)

- `enchanting_table.png`, 176 × 166. Title `(8, 6)`.
- Slots 0 item `(15, 47)` (max size 1); 1 lapis `(35, 47)` (lapis lazuli only, empty sprite
  `container/slot/lapis_lazuli`); inventory top 84.
- Three options i = 0..2, area `rect(gx + 60, gy + 14 + 19i, 108, 19)`:
  - cost `prop_i = 0`: sprite `container/enchanting_table/enchantment_slot_disabled` alone;
  - unavailable (lapis < i + 1 or level < cost, outside creative): disabled background, icon
    `level_<i+1>_disabled` (16 × 16) at `(gx + 61, gy + 15 + 19i)`, name text in
    `0xFF342F25`, cost in `0xFF407F10`;
  - available: background `enchantment_slot_highlighted` on hover (hand cursor; name in
    `0xFFFFFF80`) otherwise `enchantment_slot` (name in `0xFF685E4A`), icon `level_<i+1>`, cost in
    `0xFF80FF20`.
  - Name: Java linear congruential generator reseeded at each frame with prop 3; for each
    option with non-zero cost, in order, `draw(2) + 3` words picked by `draw(62)` from a fixed
    list of 62 words (`GALACTIC_WORDS` in `screens_work.rs`), joined by spaces, font
    `minecraft:alt`, cut at the character at the width `86 − width(cost)` (one line), at
    `(gx + 80, gy + 16 + 19i)`, no shadow. Cost (with shadow) right-aligned: `x = gx + 80 + 86 − width`, `y = gy + 23 + 19i`.
  - Tooltip on hover `rect(60, 14 + 19i, 108, 17)` if cost > 0 and index ≥ 0:
    `container.enchant.clue` (white) with the full name of the offered enchantment (gray, red
    for a curse; level omitted for a single-level enchantment); outside creative: empty
    line then, if level insufficient, `container.enchant.level.requirement` in red, otherwise
    `container.enchant.lapis.one/many` (gray, red if lapis insufficient) and
    `container.enchant.level.one/many` (gray).
  - Click on an available option (lapis ≥ i + 1, level ≥ i + 1 and ≥ cost, item present,
    cost > 0; creative: only cost and item count) → `container_button_click` button i, without
    a slot click.
- Animated book at `rect(gx + 14, gy + 14, 38, 31)`: uncertain point (texture
  `textures/entity/enchantment/enchanting_table_book.png`).
- Shift-click: 0/1 → 2..37 reverse; lapis → 1; other → a single copy in 0 if empty
  and acceptable.

### 5.9 Anvil (`anvil`)

- `anvil.png`, 176 × 166. Title at `(60, 6)`.
- Slots 0 `(27, 47)`, 1 `(76, 47)`, 2 result `(134, 47)`; inventory top 84. The result
  can only be taken if cost > 0 and (creative or level ≥ cost) — decided by the menu.
- Field background: `container/anvil/text_field` (slot 0 non-empty) or `text_field_disabled`,
  `rect(gx + 59, gy + 20, 110, 16)`.
- Error: if slot 0 or 1 non-empty and result empty: `container/anvil/error` (28 × 21) at
  `(gx + 99, gy + 45)`.
- Cost (prop 0 > 0), in the titles: text `container.repair.expensive` (if cost ≥ 40 outside
  creative, `0xFFFF6060`) or, if the result exists, `container.repair.cost` (green `0xFF80FF20`,
  red `0xFFFF6060` if the result cannot be taken). `tx = 176 − 8 − width − 2`; veil
  `0x4F000000` at `rect(tx − 2, 67, 168 − (tx − 2), 12)`; text at `(tx, 69)` with shadow.
- Name field at `(gx + 62, gy + 24)`, 103 × 12, no border, white text, 50 characters max,
  permanent focus; takes the item's name when slot 0 changes, editable only if it
  is non-empty. At each change: if the text equals the default name of an item without
  `custom_name` → empty string; if different from the remembered name → `rename_item`. Uncertain point:
  text editing (cursor, selection) — reuse the chat input field.
- Shift-click: result → inventory reverse; 0/1 → inventory; inventory → 0..1 if the item
  can go there, otherwise inventory ↔ hotbar.

---

## 6. Uncertain points

1. **Fingerprints**: the exact persistent form of each of the ~100 components (fields omitted by
   default, compact forms: string vs map for text, list vs single object) was not
   collected component by component. Proposed measurement: capture of an official client clicking various
   items, compare the fingerprints. An error only costs a slot resend.
2. **Recipe-dependent prediction**: "smelts", "fuel", brewing stand ingredient,
   potion input come from the recipe property sets (`update_recipes`). Without them,
   the shift-click prediction in furnace/brewing stand may be wrong (the server corrects).
3. **Recipe book** (inventory, crafting table, furnaces): not described; only plan for the
   `gx` offset (§ 4.1) and the button.
4. **Creative inventory** (tabs, `set_creative_mode_slot`): out of scope.
5. **Bundles**: click interception, selection wheel, image tooltip: out of
   scope; in the meantime, treat the bundle as an ordinary item (the server will correct).
6. 3D rendering of the player in the frame and of the enchanting book: camera and
   light parameters not collected in detail.
7. Status effects displayed next to the inventory: layout not collected.
8. Screens out of scope (beacon, grindstone, loom, merchant, smithing, stonecutter, cartography,
   lectern, crafter, mounts): nothing is opened; recommendation: generic "slots only"
   screen from `container_set_content` so as not to block the player.
9. The mouse button numbers of the decompiled client follow a numbering 1 = left,
   2 = middle, 3 = right; brought back here to the protocol buttons 0 / 2 / 1.
10. Tooltip: exact content of the lines per component (potion, fireworks, map…) not
    detailed; only name, enchantments, lore and attributes are.
