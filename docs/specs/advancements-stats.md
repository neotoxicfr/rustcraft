# Advancements and statistics (26.3)

Scope: receiving and storing advancements, the "advancement made" toast, the advancements screen,
the statistics request and the statistics screen. Packet ids are resolved by name
(`rc-client/src/ids.rs`). Cross-references: toast manager → `creative-inventory.md`
§ 13.1; stack template (icons) → `item-components.md` "Stack template"; text
components and wrapping → `text.md` § 3‑4; lists, menu tabs, header/footer → `option-menus.md`
§ 1.9, 1.10, 1.12; chat message → `chat.md`.

Colours are ARGB. `9` = font line height. Integer divisions truncate toward zero
(like the game) unless stated otherwise.

---

## 1. Network

### 1.1 `update_advancements` (to the client)

In order:

1. boolean **reset**;
2. list (varint n) of **additions**, each:
   - advancement id;
   - parent: present boolean + id;
   - display: present boolean + **display** block (below);
   - requirements: varint n groups, each group = varint m strings (criterion names);
   - "telemetry event" boolean (ignored);
   - `x` f32, `y` f32 (position in the tree, in cells);
3. set (varint n) of **removed** ids;
4. table (varint n) of **progress**: id → table (varint m) criterion name → present boolean
   + i64 (date obtained, milliseconds since the Unix epoch);
5. boolean **show** (allows toasts).

Display block: title (text component, trusted network format), description
(component), icon (stack template), frame varint (0 task, 1 challenge, 2 goal; out of range → 0),
flags i32 (bit 0 background present, bit 1 toast, bit 2 hidden), then if bit 0: background
id. Background texture = `<ns>:textures/<path>.png` (e.g. `minecraft:gui/advancements/backgrounds/stone`
→ `textures/gui/advancements/backgrounds/stone.png`). "Announce in chat" is not
transmitted (the server handles it, § 3.3).

Requirements: AND of ORs. An advancement is **done** if the group list is non-empty and
every group contains at least one obtained criterion. Satisfied groups = number of groups with
at least one obtained criterion; percentage = satisfied / number of groups (0 if the criteria
table is empty).

### 1.2 Client-side processing

Strict order:

1. If reset: clear the tree and the progress; notify the open screen (tabs cleared,
   selected tab null).
2. **Removals**: each unknown id is ignored; otherwise the node **and all its
   descendants** are forgotten (stored progress is not purged, no visible effect).
3. **Additions**: successive passes over the remaining list; within a pass, each entry with no
   parent or whose parent is already known is inserted in order; if a pass inserts nothing, the
   rest is abandoned (error log). The tree keeps **insertion order** (it fixes the order
   of the tabs and the nodes). Then the position `(x, y)` is assigned to each added node.
4. **Progress**: for each entry whose advancement is known to the tree (otherwise warning,
   ignored): criteria absent from the requirements are removed, missing ones are added as not
   obtained; the progress **replaces** the old one. If **not** a reset, advancement done,
   "show" true, display present with the toast flag → **toast** (§ 3).
5. Notify the open screen (rebuild, § 4.2).

### 1.3 `select_advancements_tab` (to the client)

Optional id (boolean + id). Sets the selected tab **without** sending a packet back; an unknown
id means "none". If the screen is open, it switches to that tab (none
if the tab does not exist).

### 1.4 `seen_advancements` (to the server)

Varint action: 0 "tab opened" followed by the root id; 1 "screen closed" (nothing
else). Sent:

- "tab opened" on every tab choice by the client (screen opening, click on a
  tab, rebuild that falls back to the first tab) — even if it is already the selected tab.
  When the screen opens it is sent **twice** for the first tab (§ 4.2).
- "screen closed" when the advancements screen is left (closing, return to the pause menu, another
  screen).

The server remembers the tab (root with a display) and sends back `select_advancements_tab` if it
changed.

### 1.5 Visibility (server rule, for reference)

The server only sends visible advancements: a node is visible if it is done or has a
done descendant; otherwise we walk up at most 3 levels (itself, parent, grandparent): the first
"done" makes it visible, the first "hidden and not done" or "no display" hides it, otherwise
hidden. The client has a single rule of its own: a node with the **hidden** flag is drawn and
hoverable only if it is done.

---

## 2. Statistics — network

### 2.1 `client_command` (to the server)

Varint action: 0 respawn, **1 statistics request**, 2 game rule values request. Sent with 1 each
time the Statistics screen is opened.

### 2.2 `award_stats` (to the client)

Table (varint n): statistic → varint value. Statistic = varint **type** then varint
**value** in the type's registry:

| type | name | value registry |
|---|---|---|
| 0 | `mined` | blocks |
| 1 | `crafted` | items |
| 2 | `used` | items |
| 3 | `broken` | items |
| 4 | `picked_up` | items |
| 5 | `dropped` | items |
| 6 | `killed` | entity types |
| 7 | `killed_by` | entity types |
| 8 | `custom` | custom statistics (§ 2.3) |

Each value **replaces** the stored value (default 0). The counter starts empty at connection
(`login`) and survives respawns. If the Statistics screen is open, it is notified (§ 5.1).

### 2.3 Custom statistics (network id, format)

Translation key `stat.minecraft.<name>`. Formats: E = integer, T = time, D = distance,
X = tenths (§ 2.4).

0 leave_game E · 1 play_time T · 2 total_world_time T · 3 time_since_death T ·
4 time_since_rest T · 5 sneak_time T · 6 walk_one_cm D · 7 crouch_one_cm D · 8 sprint_one_cm D ·
9 walk_on_water_one_cm D · 10 fall_one_cm D · 11 climb_one_cm D · 12 fly_one_cm D ·
13 walk_under_water_one_cm D · 14 minecart_one_cm D · 15 boat_one_cm D · 16 pig_one_cm D ·
17 happy_ghast_one_cm D · 18 horse_one_cm D · 19 aviate_one_cm D · 20 swim_one_cm D ·
21 strider_one_cm D · 22 nautilus_one_cm D · 23 jump E · 24 drop E · 25 damage_dealt X ·
26 damage_dealt_absorbed X · 27 damage_dealt_resisted X · 28 damage_taken X ·
29 damage_blocked_by_shield X · 30 damage_absorbed X · 31 damage_resisted X · 32 deaths E ·
33 mob_kills E · 34 animals_bred E · 35 player_kills E · 36 fish_caught E ·
37 talked_to_villager E · 38 traded_with_villager E · 39 eat_cake_slice E · 40 fill_cauldron E ·
41 use_cauldron E · 42 clean_armor E · 43 clean_banner E · 44 clean_shulker_box E ·
45 interact_with_brewingstand E · 46 interact_with_beacon E · 47 inspect_dropper E ·
48 inspect_hopper E · 49 inspect_dispenser E · 50 play_noteblock E · 51 tune_noteblock E ·
52 pot_flower E · 53 trigger_trapped_chest E · 54 open_enderchest E · 55 enchant_item E ·
56 play_record E · 57 interact_with_furnace E · 58 interact_with_crafting_table E ·
59 open_chest E · 60 sleep_in_bed E · 61 sleep_in_straw_bed E · 62 open_shulker_box E ·
63 open_barrel E · 64 interact_with_blast_furnace E · 65 interact_with_smoker E ·
66 interact_with_lectern E · 67 interact_with_campfire E · 68 interact_with_cartography_table E ·
69 interact_with_loom E · 70 interact_with_stonecutter E · 71 bell_ring E · 72 raid_trigger E ·
73 raid_win E · 74 interact_with_anvil E · 75 interact_with_grindstone E · 76 target_hit E ·
77 interact_with_smithing_table E.

All block / item / entity statistics are formatted as E.

### 2.4 Value formats

"2 decimals" template: at least one digit before the decimal point, exactly 2 after, decimal
point, no thousands separator, rounded to nearest with ties to even (on the double
value).

- **E**: integer with thousands separator "," (`1,234,567`).
- **X**: `v × 0.1` in the 2-decimals template (`125` → `12.50`).
- **D** (v in cm): `m = v/100`, `km = m/1000`; if `km > 0.5` → `km` in the template + `" km"`;
  otherwise if `m > 0.5` → `m` in the template + `" m"`; otherwise `v + " cm"` (raw integer).
- **T** (v in ticks): `s = v/20`, `min = s/60`, `h = min/60`, `d = h/24`, `y = d/365`; first
  threshold strictly > 0.5 among y, d, h, min → value in the template + `" y"`, `" d"`, `" h"`,
  `" min"`; otherwise `s` written as a Java-style double (shortest decimal form, with at
  least one decimal: `0.0 s`, `1.0 s`, `12.35 s`) + `" s"`.

---

## 3. "Advancement made" toast

### 3.1 Content

- Size 160 × 32, background sprite `toast/advancement` over the full size.
- Title wrapped to a width of 125 (`text.md` § 4). Frame label: `advancements.toast.task`,
  `advancements.toast.goal`, `advancements.toast.challenge`; colour `0xFFFF88FF` (challenge) or
  `0xFFFFFF00` (task, goal). Texts **without shadow**.
- Title on **one** line: label at (30, 7) in the frame colour; title at (30, 18) white.
- Title on **several** lines (t = time fully visible, ms):
  - `t < 1500`: label alone at (30, 11), alpha `⌊clamp((1500 − t)/300, 0, 1) × 255⌋`;
  - otherwise: lines vertically centred, first `y = 16 − (n × 9)/2`, step 9, x 30, white
    with alpha `⌊clamp((t − 1500)/300, 0, 1) × 252⌋`.
- Icon (stack created from the template) at (8, 8), without decorations.

### 3.2 Duration and sound

- Visible while `t < 5000 × toast display time` (option, default 1.0); several
  advancement toasts coexist (one per advancement, manager queue).
- Challenge only: sound `ui.toast.challenge_complete` (interface category, volume 1, pitch 1)
  played when the toast **takes a slot** (not when it enters the queue), at most once
  per sound and per manager update. The manager's enter/exit sounds
  are added (`creative-inventory.md` § 13.1).

### 3.3 Chat message

Sent by the server (system message): key `chat.type.advancement.<task|goal|challenge>` with
(player name, `[title]`); the bracketed title has the frame colour (green `0xFF55FF55` for
task and goal, dark purple `0xFFAA00AA` for challenge) and a hover showing coloured title + line
break + description. Nothing to do on the client beyond the chat (`chat.md`).

---

## 4. Advancements screen

### 4.1 Opening, closing, frame

- Opening: `gui.advancements` button of the pause menu (parent screen = pause) or the **L** key
  (`key.advancements`) in game (no parent).
- L key in the screen: back to the parent if there is one, otherwise close (mouse capture).
  Escape / "Done": back to the parent (or the game). Any exit sends "screen closed" (§ 1.4).
- Header / footer layout (`option-menus.md` § 1.12): title `gui.advancements` in the header,
  "Done" button 200 wide in the footer, usual screen background.
- Window: `window.png` (`textures/gui/advancements/window.png`, 252 × 140 read at (0, 0) in
  a 256 × 256 texture) at `G = (W − 252)/2`, `Hh = (H − 140)/2`. Interior at `(G + 9, Hh + 18)`,
  234 × 113. Window title at `(G + 8, Hh + 6)`, `0xFF404040`, no shadow: title of the selected
  tab, otherwise `gui.advancements`.
- No tab: interior filled with opaque black; `advancements.empty` centred at
  `x = G + 126`, `y = Hh + 18 + 56 − 4`; `advancements.sad_label` (":(") centred at
  `y = Hh + 18 + 113 − 9`; white with shadow.

Drawing order: background + header + footer → stratum: interior (tab, clipped to the interior) →
stratum: window, then tab backgrounds, then tab icons, then title → stratum: dim overlay and
tooltip of the hovered node, tab tooltip.

### 4.2 Building the tabs (on opening and on every update)

- Tabs are rebuilt on every `update_advancements` received while the screen is open;
  the scroll state (and the centring already done) of a tab with the same root is kept.
- For each **root** (node without a parent) in tree order: if it has a display
  **and** a background → new tab with index = number of tabs already created; otherwise ignored (its
  descendants are shown nowhere).
- Placement by index: 8 above (indices 0‑7), 8 below (8‑15), 5 on the left (16‑20),
  5 on the right (21‑25). **No paging**: beyond 26, roots are ignored.
- For each non-root node (tree order) with a display: node added to the tab of its
  root. Its drawn parent = first ancestor with a display, looked up in the tab
  (attachment redone on every addition, so arrival order does not matter); children in
  attachment order.
- Stored progress is assigned to the nodes.
- Selected tab: the one with the same root as before; failing that the **first**, and "tab opened"
  is sent to the server. When the screen opens, the selected tab is always the first (the
  remembered tab is overwritten) and "tab opened" is sent twice.
- `select_advancements_tab` received or click → corresponding tab (none if it does not exist:
  empty screen).

### 4.3 Tabs

Coordinates relative to `(G, Hh)`, size, sprites (prefix `advancements/`):

| side | position of tab i (local) | size | icon (+dx, +dy) | sprites first / middle / last |
|---|---|---|---|---|
| above | `(32 i, −28)` | 28 × 32 | +6, +9 | `tab_above_left`, `tab_above_middle`, `tab_above_right` |
| below | `(32 i, 136)` | 28 × 32 | +6, +6 | `tab_below_left`, `…_middle`, `…_right` |
| left | `(−28, 28 i)` | 32 × 28 | +10, +5 | `tab_left_top`, `tab_left_middle`, `tab_left_bottom` |
| right | `(248, 28 i)` | 32 × 28 | +6, +5 | `tab_right_top`, `tab_right_middle`, `tab_right_bottom` |

- Local index 0 → "first", local index = max − 1 (7 or 4) → "last", otherwise "middle";
  suffix `_selected` for the selected tab.
- Tabs drawn **only if there are at least 2**. Icon = the root's icon (dummy stack,
  without decorations).
- Strict hover (`x < mx < x + w`, same for y): tooltip = the root's title; "hand" cursor on a
  non-selected tab. Left click → selected tab + "tab opened".

### 4.4 Content of a tab

- Node: `x = ⌊xr × 28⌋`, `y = ⌊yr × 27⌋` (xr, yr from the packet). Bounding box of the content:
  `min/max` over `[x, x + 28] × [y, y + 27]` of all the nodes of the tab.
- **Centring**, on the first draw of the tab: `scrollX = 117 − (x_max + x_min)/2`,
  `scrollY = 56 − (y_max + y_min)/2` (integer division).
- Drawing, clipped to the interior, origin = interior corner, `sx = ⌊scrollX⌋`, `sy = ⌊scrollY⌋`:
  1. **Tiled background**: the root's background texture (16 × 16, whole); `ox = sx mod 16`,
     `oy = sy mod 16` (remainder with the sign of the dividend); tiles at `(ox + 16 i, oy + 16 j)` for
     `i ∈ [−1, 15]`, `j ∈ [−1, 8]`.
  2. **Links**, two passes over the tree from the root (parent before children): first all
     black links (`0xFF000000`, thick), then all white ones (`0xFFFFFFFF`, thin).
  3. **Nodes**, from the root, parent before children. A hidden node that is not done is not
     drawn (its children are). Frame `<type>_frame_obtained` if percentage ≥ 1, otherwise
     `<type>_frame_unobtained` (`task`, `goal`, `challenge`), 26 × 26 at `(sx + x + 3, sy + y)`;
     icon at `(sx + x + 8, sy + y + 5)`.
- A node not connected to the root (displayed ancestor missing from the tab) is not drawn but stays
  hoverable (§ 4.6).

**Links** between a node (cx, cy) and its drawn parent (px, py), scrolled coordinates.
Notation: `dX = px + 13`, `sX = px + 30`, `dY = py + 13`, `mX = cx + 13`, `mY = cy + 13`.
"H(a, b, y)" = horizontal line from `min(a,b)` to `max(a,b)` **inclusive**; "V(x, a, b)" =
vertical line strictly **between** a and b (lines `min+1` to `max−1`).

- Black: H(sX, dX, dY − 1), H(sX + 1, dX, dY), H(sX, dX, dY + 1), H(mX, sX − 1, mY − 1),
  H(mX, sX − 1, mY), H(mX, sX − 1, mY + 1), V(sX − 1, mY, dY), V(sX + 1, mY, dY).
- White: H(sX, dX, dY), H(mX, sX, mY), V(sX, mY, dY).

The links of a hidden node are drawn like the others.

### 4.5 Scrolling (no zoom)

- Horizontally scrollable if `x_max − x_min > 234`, vertically if `y_max − y_min > 113`.
- Move `(dx, dy)`: on each scrollable axis, `scrollX = clamp(scrollX + dx, −(x_max − 234), 0)`
  (same for y with 113); non-scrollable axis unchanged.
- **Left-click** drag: the first drag event only arms the scrolling, the following ones
  apply the mouse's `(dx, dy)` (interface units, fractional). Releasing or dragging with
  another button disarms it. During the drag, 4-way "resize" cursor,
  horizontal or vertical depending on the scrollable axes. Dragging works anywhere on the
  screen.
- Wheel: `(dx, dy) = (notchX × 16, notchY × 16)`.

### 4.6 Hover, dim overlay and tooltip

Every tick (20 Hz): mouse relative to the interior `(rx, ry)`; if `0 < rx < 234` and
`0 < ry < 113`, first node of the tab (insertion order) that is visible (not hidden, or done) such that
`sx + x ≤ rx ≤ sx + x + 26` and `sy + y ≤ ry ≤ sy + y + 26` → hovered. Dim overlay:
hover → `v = clamp(v + 0.06, 0, 0.3)`; otherwise `v = clamp(v − 0.12, 0, 1)` and no longer hovered.
Drawing (tooltip stratum, interior origin, not clipped): black rectangle with alpha `⌊v × 255⌋`
over `(0, 0, 234, 113)`, then the hovered node's tooltip.

**Tooltip width** (computed when the node is created):
- title lines = title wrapped to 163; `lT = max(widest line, 80)`;
- `lP = width(advancements.progress with (g, g)) + 8` if g = number of groups > 1, otherwise 0;
- `L = 29 + lT + lP`; coloured description (frame colour applied where it has none:
  green `0xFF55FF55` task/goal, dark purple `0xFFAA00AA` challenge) wrapped as well as possible: for
  `δ ∈ [0, 10, −10, 25, −25]`, wrap at `L − δ`, difference = |widest line − L|; the first
  difference ≤ 10 wins, otherwise the smallest difference (first one on ties);
- `L = max(L, widest description line)`; **width `w = L + 8`**.

**Drawing** (n title lines, d description lines):
- `hT = 9 n + 17`; `top = sy + y + (26 − hT)/2`; `botT = top + hT`; `hD = 9 d + 6`.
- To the left if `G + sx + x + w + 26 ≥ W`; above if `botT + hD ≥ 113`.
- `gT = sx + x − w + 32` (left) or `sx + x`.
- If d > 0: sprite `advancements/title_box` (nine-slice 200 × 26, border 10) of `w × (hT + hD)`
  at `(gT, botT − (hT + hD))` (above) or `(gT, top)`.
- Title bar, `p` = percentage, `a = ⌊p × w⌋`:
  - `p ≥ 1`: all "obtained", obtained frame;
  - `a < 2`: all "not obtained", not-obtained frame;
  - `a > w − 2`: all "obtained", **not**-obtained frame;
  - otherwise: left half "obtained" over `a`, right "not obtained" over `w − a`, not-obtained frame.
  Uniform: sprite `advancements/box_obtained` or `box_unobtained` (nine-slice 200 × 26,
  border 10) of `w × hT` at `(gT, top)`. Split: the sprite is drawn at **200 × hT** and
  only a window of it is shown: left = sprite placed at `(gT, top)`, clipped to
  `(gT, top, a, hT)`; right = sprite placed at `(gT + w − 200, top)`, clipped to
  `(gT + a, top, w − a, hT)`.
- Node frame (26 × 26) at `(sx + x + 3, sy + y)`.
- Title (white, shadow): lines with step 9 starting at `y = top + 9`, at `x = gT + 5` (left) or
  `sx + x + 32`. Progress `advancements.progress` ("x/y" = satisfied groups / groups, absent if
  g ≤ 1 or no known criterion) white at `y = top + 9`, `x = sx + x − lp` (left) or
  `sx + x + w − lp − 5` (lp = text width).
- Description (component colour, shadow) at `x = gT + 5`, starting at `y = top − 9 d + 1`
  (above) or `botT`.
- Icon at `(sx + x + 8, sy + y + 5)`.

---

## 5. Statistics screen

### 5.1 Structure

- Opened from the pause menu (button `gui.stats`, parent = pause); sends `client_command` 1.
- Menu tab bar (`option-menus.md` § 1.10) at the top: `stat.generalButton`,
  `stat.itemsButton`, `stat.mobsButton`. Header height = bottom of the bar (24). Header background:
  `textures/gui/tab_header_background.png` repeated (16 × 16) over `W × 24`; menu background below;
  footer of 33 with "Done" 200 wide; separator `footer_separator` (2 high) at
  `y = H − 33`. Escape / Done → parent.
- **Loading** (until the first `award_stats` received, screen open): 3 "waiting" tabs,
  only the first active. Content centred in the tab area, with a 30 bottom margin: block of
  height 28 = `multiplayer.downloadingStats` white (shadow) centred at the top + 2, then animated grey
  dots `0xFF808080` centred at `bottom − 11`: `O o o`, `o O o`, `o o O`, `o O o`, frame =
  `⌊ms / 300⌋ mod 4`.
- On the first `award_stats`: tabs replaced by the 3 lists, first one selected. Items /
  Mobs tabs active only if their list is non-empty, otherwise inactive with tooltip
  `gui.stats.none_found`. Later `award_stats` rebuild nothing (the General list reads
  the values live, the other two are frozen).
- Lists: content area `y = 24`, `h = H − 24 − 33`, **row width 280**, standard scrolling and
  selection (§ 1.9). List background not drawn (all three); separators not drawn
  for General and Mobs, drawn for Items.

### 5.2 General

- Rows of 14. All custom statistics (§ 2.3), sorted by **translated name**
  (raw string comparison, UTF‑16 units).
- Colour: even-index row `0xFFFFFFFF`, odd `0xFFBABABA`. Name at `(xc + 2, ym − 4)`,
  formatted value (§ 2.4) right-aligned at `xd − 4` (xc / xd = left / right of the usable
  area, ym = vertical middle), with shadow.

### 5.3 Items

- Rows of 22. Content: every item with a value > 0 in `broken`, `crafted`, `used`,
  `picked_up` or `dropped`, plus the item of every block with `mined` > 0; air excluded. If empty:
  no rows. Otherwise a header row then one row per item.
- Columns c = 0‑5: `mined` (block), `broken`, `crafted`, `used`, `picked_up`, `dropped`;
  `X(c) = 75 + 40 c` from the usable left `xc`.
- **Header**: 6 buttons 18 × 18 at `(xc + X(c) − 18, top + 1)`: background `statistics/header`
  (hover / focus: `container/slot`), then icon `statistics/block_mined`, `item_broken`,
  `item_crafted`, `item_used`, `item_picked_up`, `item_dropped`; tooltip = the type's name
  (`stat_type.minecraft.<name>`). Sorted column: arrow `statistics/sort_down` (descending order)
  or `sort_up` 18 × 18 at `(xc + X(c) − 36, top + 1)`.
- **Item row**: `container/slot` 18 × 18 at `(xc, top)`, item at `(xc + 1, top + 1)` without
  decorations; white outline if focused; hover → full item tooltip anchored at
  `(xc + 18, top + 18)`. Values right-aligned at `xc + X(c)`, `y = ym − 4`; column 0
  for an item that is not a block → `stats.none` ("-"). Colour by index in the
  list **header included** (first item row = index 1 → `0xFFBABABA`): even white, odd
  `0xFFBABABA`.
- **Sort** (click on a button): new column → descending; same column: descending →
  ascending → none. Key = the column's value (column 0: −1 for a non-block); tie →
  item id, in the **same direction**. "None" keeps the current order (stable sort with a
  null key).
- Initial order: not defined in the game; Rustcraft: item id order.

### 5.4 Mobs

- Rows of 36. Every entity type (registry order) with `killed` > 0 or `killed_by` > 0.
- Type name white at `(xc + 2, top + 1)`; line 2 at `(xc + 12, top + 10)`:
  `stat_type.minecraft.killed` (number, name) or, if 0, `stat_type.minecraft.killed.none` (name);
  line 3 at `(xc + 12, top + 19)`: `stat_type.minecraft.killed_by` (name, number) or
  `stat_type.minecraft.killed_by.none` (name). Colour `0xFFBABABA` if number > 0, otherwise
  `0xFF808080`. Shadow.

---

## 6. Resources

Sprites: `toast/advancement`, `advancements/{box,task_frame,goal_frame,challenge_frame}_{obtained,unobtained}`,
`advancements/title_box`, `advancements/tab_*` (24), `statistics/{header,sort_up,sort_down,block_mined,item_broken,item_crafted,item_used,item_picked_up,item_dropped}`,
`container/slot`. Textures: `textures/gui/advancements/window.png`, backgrounds
`textures/gui/advancements/backgrounds/{stone,adventure,husbandry,nether,end}.png` (read on
demand depending on the packet: to be declared in the resource cache), `textures/gui/tab_header_background.png`.
Sound: `ui.toast.challenge_complete`. Key: `key.advancements` (L).

## 7. Advancements on the integrated server (`rc-sim/src/progress/`)

- Tree: files `data/<ns>/advancement/**.json` from the jar then from the world's data packs (a redefined id replaces the original). Parents resolved, roots = those without a parent; layout computed on the server side (Walker tree, `layout.rs`); children sorted by id (the game sorts them in a hash-table order that varies from one launch to the next).
- Requirements: AND of ORs over criterion names (absent: one group per criterion). Done = every group has at least one obtained criterion.
- Visibility (game rule): an advancement is displayed if it is done, or if a descendant up to two levels below is, or if its parent / grandparent is done, except `hidden` while it is not done; roots without a display are never sent.
- Sending: first packet `reset=true` with all the visible ones and their progress; then, on every tick where something changed, additions / removals / progress of only the changed ones (`reset=false`). Selected tab: `select_advancements_tab` in response to `seen_advancements` (action 0).
- Triggers: those of the jar (`trig.rs`) evaluated at the matching game points (breaking, placing, picking up, crafting, hits, deaths, dimension change, sleeping, breeding, taming, interactions, loot chests, enchanting, effects, position every second, falling, inventory on every change…). Full predicates: entities, locations, items, damage sources, loot conditions (`pred.rs`).
- Rewards on completion: experience, loot tables given to the player, unlocked recipes, executed function (level 2). Chat message `chat.type.advancement.<task|goal|challenge>` if `announce_to_chat` and the `show_advancement_messages` rule; toast only if `show_toast`.
- Save `players/advancements/<uuid>.json`: object `{"<id>": {"criteria": {"<name>": "yyyy-MM-dd HH:mm:ss +zzzz"}, "done": bool}, …, "DataVersion": N}` sorted by id, without the advancements that have no obtained criterion.

## 8. Statistics on the integrated server (`rc-sim/src/stats.rs`)

- Counters: `mined` (suitable tool only, never in creative), `crafted` (number actually taken, table and workstations), `used` (blocks placed, items consumed, tools), `broken`, `picked_up` (quantity of the original stack even if only part of it is picked up — game behaviour reproduced), `dropped`, `killed`, `killed_by`, `custom` (78 statistics: play time, distances by movement mode in cm, jumps, damage, deaths, block interactions…).
- Distances: counted on every movement according to the state (swimming, underwater, on water, ladder, crouching, sprinting, walking, flying, falling, vehicle) with the game's rules (rounding to the centimetre, thresholds).
- Scoreboard criteria `minecraft.<type>:<id>` updated with the statistic; `deathCount`, `totalKillCount`, `teamkill.*` too (`playerKillCount`, `killedByTeam.*`: not yet, fights between players are not simulated).
- "Time since rest": this counter also feeds the phantom spawner (`mobs/phantom.rs`), no separate file any more.
- `award_stats`: sent on `client_command` action 1, with only the statistics changed since the last send (all of them on joining the game). Save `players/stats/<uuid>.json`: `{"stats": {"minecraft:<type>": {"minecraft:<id>": n}}, "DataVersion": N}`.

---

## Uncertain points

1. Network ids of the statistic types and of the custom statistics: deduced from the registration
   order; to be confirmed with the server's `registries.json` report (registries
   `stat_type`, `custom_stat`) or the extractor.
2. Initial order of the Items list: the game uses a set with no stable order; Rustcraft's choice
   = item id order (difference invisible unless compared row by row).
3. Scroll bounds `[−(max − 234), 0]`: if `min < 0` (nodes with negative coordinates), part of
   the content stays unreachable; and if `max < 234` on a scrollable axis, the lower bound
   exceeds the upper one (the game's clamping behaviour, to be reproduced as is: `v < low → low`, otherwise
   `min(v, high)`). Pathological case, vanilla servers not affected.
4. Rendering of very low-alpha text (multi-line toast, start of the fade): the text
   engine may skip near-zero alphas (`text.md`); not verified here.
5. Reopening after a window resize: the advancements screen does not redo its
   construction (only the placement); not verified against the exact screen cycle of 26.3.
6. Arrival of an addition whose id already exists: the new node replaces the old one at the same
   order position, but the old one remains a child of its parent; visual effect not studied (the vanilla
   server removes before re-adding).
