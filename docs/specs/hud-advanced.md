# Advanced HUD: boss bars, status effects, titles (26.3)

Source: reading of the decompiled client (kept outside the repository), reworded. Frame, strata and sprites: `hud.md` § 1.
Units: interface units; `L`, `H` = interface width and height; `div` = integer division.
Packet ids: official 26.3 report (play phase, server → client).

---

## 1. Boss bars

### 1.1 Packet `boss_event` (0x09)

| field | type |
|---|---|
| bar id | UUID (2 × i64) |
| operation | varint (enumeration, below) |
| data | depends on the operation |

| operation | id | data (in order) |
|---|---|---|
| add | 0 | name: NBT text component; progress: `f32`; colour: varint; style: varint; flags: u8 |
| remove | 1 | — |
| progress | 2 | progress: `f32` |
| name | 3 | name: NBT text component |
| style | 4 | colour: varint; style: varint |
| flags | 5 | flags: u8 |

- Colour (id): 0 pink, 1 blue, 2 red, 3 green, 4 yellow, 5 purple, 6 white.
- Style (id): 0 progress (full bar), 1 six notches, 2 ten notches, 3 twelve notches, 4 twenty notches.
- Flags (bits): 1 darken the sky, 2 boss music, 4 world fog. The other bits are ignored.
- Progress: normally in [0, 1], not clamped by the client (see § 1.3 and Uncertain points).

### 1.2 Client-side processing

- The client keeps an **insertion-ordered table** (id → bar). The display order is the order of first insertion.
- Add: creates (or replaces, keeping its original place in the order if the id already existed) the bar with all its fields; the displayed progress is immediately the received value (no animation).
- Remove: deletes the entry.
- Progress: animation start = **currently displayed value** (interpolated, § 1.3), target = received value, timestamp = now (wall clock in ms).
- Name, style, flags: replace the corresponding fields.
- Operation 2 to 5 on an unknown id: the official client crashes (access to a missing entry). For us: ignore the packet.
- The table is only emptied on **disconnection** (not on dimension change nor on respawn: the server sends the removals).

### 1.3 Progress animation

- `a = clamp((now_ms − timestamp) / 100, 0, 1)`; displayed progress `p = start + a·(target − start)`. Linear fade over **100 ms of real time** (independent of ticks and of pause).
- Filled width: `w = floor(p·181) + (p > 0 ? 1 : 0)` (out of 182). So any progress > 0 shows at least 1 column; `p = 1` → 182.

### 1.4 Rendering

Conditions: HUD not hidden, at least one bar. Position in the HUD order: right after the status effects (`hud.md` § 1.3, point 3), in a **new stratum**.

For each bar, in table order, with `y` initialised to **12**:
1. `x = L div 2 − 91`.
2. Background: sprite `boss_bar/<colour>_background` (182×5) drawn at `(x, y)`, size 182×5.
3. If style ≠ progress: notch sprite `boss_bar/notched_<n>_background` (182×5) on top, same place.
4. If `w > 0`: sprite `boss_bar/<colour>_progress` **cropped** to its `w` leftmost columns (texture coordinates u from 0 to w/182, v from 0 to 1, no stretching), drawn at `(x, y)`, size `w`×5; then, if style ≠ progress, `boss_bar/notched_<n>_progress` cropped the same way.
5. Name: `nameX = L div 2 − (name width) div 2`, `nameY = y − 9`; white colour `0xFFFFFFFF`, **with shadow**, no background.
6. `y += 19` (10 + line height 9). If `y ≥ H div 3`: stop (the following bars are not drawn).

Sprite colour names: `pink`, `blue`, `red`, `green`, `yellow`, `purple`, `white`. Notches: `notched_6`, `notched_10`, `notched_12`, `notched_20`. All in the `gui` atlas (`textures/gui/sprites/boss_bar/*.png`, 182×5).

Maximum number on screen: bar `i` (starting at 0) is at `y = 12 + 19·i`; the first is always drawn; the next is drawn as long as `12 + 19·(i+1) < H div 3`. Examples: `H = 240` → 4 bars; `H = 270` → 5 bars.

### 1.5 Effects of the flags

Each flag is an **OR over all the bars** of the table (including those that do not fit on screen).

- **Darken the sky**: counter `k` (0..1) updated every render tick while the world runs normally: +0.05 (max 1) if a flag is active, otherwise −0.0125 while > 0; interpolated between ticks. Used by the fog colour (`environment.md` § 6, point 3) and by the lightmap (`lighting.md` § 4.2/4.3, "bossDarkening").
- **World fog**: atmospheric fog distances shortened (`environment.md` § 5.4: start ≤ 10, end ≤ 96, sky and clouds = end); sky pass removed (`environment.md` § 8.1, `sky.md`); in the End, the End flash only adds a third of its intensity to the lightmap's sky factor (`lighting.md` § 4.3).
- **Boss music**: only if the player is in the dimension `minecraft:the_end`. The situational music becomes the sound event `music.dragon`, min/max delays 0, and replaces the current music. Takes priority over ambient music, but not over the own music of an open screen.

---

## 2. Status effects

### 2.1 Network

- `update_mob_effect` (0x87) and `remove_mob_effect` (0x4F): formats in `entities.md` part 1 § 1.18 and § 1.19.
- Processing (any living entity, in particular the local player):
  - Add/update: the new instance (effect, amplifier, duration, ambient, particles, icon) **replaces** any instance of the same effect. If an instance existed, its fade state (§ 2.3) is copied into the new one. If bit 8 (fade) is **absent**, the fade is immediately set to its final value (1 if the effect is "active" in the sense of § 2.3, otherwise 0).
  - Remove: the instance is deleted with no other processing (its fade factor disappears with it: immediate return to 0 for the visual effects).
- No chain of hidden effects on the client side: the packet only carries the visible instance; when the strongest effect expires on the server side, the server sends an update.

### 2.2 Client tick of an instance

On each client tick, for each active effect of the entity:
1. If infinite duration (−1) or duration > 0: duration −1 (infinite stays infinite).
2. Fade update (§ 2.3).

The client **never** removes an effect that has reached 0: it stays displayed (duration 0) until the remove packet.

### 2.3 Fade factor (progressive visual effects)

Each effect has three parameters (ticks): fade-in duration `fe`, fade-out duration `fs`, fade-out lead `as`.
- Nausea: `fe = 150`, `fs = 20`, `as = 60`.
- Darkness: `fe = fs = as = 22`.
- All others: 0, 0, 0.

"Active" = infinite duration, or remaining duration > `as`. Each tick: `previous = factor`; target = 1 if active, otherwise 0; if `factor ≠ target`: duration = `fe` if active otherwise `fs`; if duration = 0 → `factor = target`, otherwise `factor += clamp(target − factor, −1/duration, +1/duration)`.
Value read at render time: `lerp(tick_fraction, previous, factor)` (if the entity is removed from the world: `previous = factor` first). Effect absent → 0.

### 2.4 HUD display (top right)

Rules already given in `hud.md` § 5.4 (conditions, sorting, placement, 24×24 backgrounds, 18×18 icon, blinking of the last 200 ticks). Additions:
- Top row (`y = 1`): **beneficial** category only. Bottom row (`y = 27`): **harmful and neutral**. Separate counters per row; the n-th (starting at 1) is at `x = L − 25·n`.
- Drawing in the hotbar stratum (no new stratum), after the held item's name, before the boss bars.
- Icon: sprite `mob_effect/<effect name>` of the `gui` atlas, loaded from `textures/mob_effect/<name>.png` (18×18; the `gui` atlas adds this folder with the prefix `mob_effect/`). Effect without a registry key: missing sprite.
- Icon colour: white with alpha `a` (`hud.md` § 5.4); background always opaque.
- Ambient (bit 1): background `hud/effect_background_ambient`, **never** blinking. Non-ambient: background `hud/effect_background`.
- Effect without bit 4 (icon): nothing drawn and it does not count in its row's offset.
- Duration 0 (expired effect, remove not yet received): `d = 0` → `a = 0 + cos(0)·0.25 = 0.25`.
- Do not draw while a screen that itself displays the effect list is open (inventories with an effects panel; panel out of scope here).

Categories (order of the built-in `minecraft:mob_effect` registry, network id = rank starting at 0):

| id | effect | category | id | effect | category |
|---|---|---|---|---|---|
| 0 | speed | beneficial | 20 | health_boost | beneficial |
| 1 | slowness | harmful | 21 | absorption | beneficial |
| 2 | haste | beneficial | 22 | saturation | beneficial |
| 3 | mining_fatigue | harmful | 23 | glowing | neutral |
| 4 | strength | beneficial | 24 | levitation | harmful |
| 5 | instant_health | beneficial | 25 | luck | beneficial |
| 6 | instant_damage | harmful | 26 | unluck | harmful |
| 7 | jump_boost | beneficial | 27 | slow_falling | beneficial |
| 8 | nausea | harmful | 28 | conduit_power | beneficial |
| 9 | regeneration | beneficial | 29 | dolphins_grace | beneficial |
| 10 | resistance | beneficial | 30 | bad_omen | neutral |
| 11 | fire_resistance | beneficial | 31 | hero_of_the_village | beneficial |
| 12 | water_breathing | beneficial | 32 | darkness | harmful |
| 13 | invisibility | beneficial | 33 | trial_omen | neutral |
| 14 | blindness | harmful | 34 | raid_omen | neutral |
| 15 | night_vision | beneficial | 35 | wind_charged | harmful |
| 16 | hunger | harmful | 36 | weaving | harmful |
| 17 | weakness | harmful | 37 | oozing | harmful |
| 18 | poison | harmful | 38 | infested | harmful |
| 19 | wither | harmful | 39 | breath_of_the_nautilus | beneficial |

Effect colour (used by the sorting, `hud.md` § 5.4): RGB value specific to each effect, to be extracted from the registry (e.g. nausea `0x551D4A`, blindness `0x1F1F23`, night vision `0xC2FF66`, darkness `0x292721`).

### 2.5 Visual effects tied to an effect (rules only)

All these rules concern the camera entity (in practice the local player).

| effect | rule | reference |
|---|---|---|
| Night vision | intensity `n` = 1 if more than 200 ticks remain (or infinite), otherwise `0.7 + 0.3·sin((duration − tick_fraction)·π·0.2)` (final flicker); brightens the lightmap (per-channel max with the colour `night_vision_color` × n) and the fog colour (except under Darkness). Conduit power underwater: same channel, intensity = underwater vision. | `lighting.md` § 4.3, `environment.md` § 6 point 4 |
| Blindness | very short fog (radius 5, or interpolated toward 5 over the last second), darkened fog colour, no sky pass. | `environment.md` § 5.2, 5.3, 6, 8.1 |
| Darkness | fog radius ≈ 15 depending on the fade factor (§ 2.3), lightmap pulsation `cos(age·π·0.025)`, lowering of the configured brightness, no sky pass, no brightening by night vision; attenuated by the "darkness effect" option (0..1). | `environment.md` § 5.3, 6; `lighting.md` § 4.3 |
| Nausea | distortion of the world projection (below); and, if the "screen distortion" option < 1, `textures/misc/nausea.png` overlay. | `hud.md` § 5.5 |
| Regeneration, hunger, poison, wither | only change the heart/food sprites. | `hud.md` § 4 |
| Mining Fatigue, Haste, Strength, etc. | no visual effect on the rendering side. | — |

**Distortion (nausea and portal)**, world projection only (not the hand nor the HUD):
- Portal intensity `p` (interpolated) and nausea intensity `n` = nausea fade factor (§ 2.3); "screen distortion" option `e` (0..1, default 1).
- Accumulated angle: on each local player tick, if `p > 0` or `n > 0` (`n` read without interpolation): `speed = (20·p + 7·n)/(p + n)` degrees/tick, `time += speed`; otherwise `speed = 0`. Angle at render time: `time + tick_fraction·speed` (degrees).
- `i = max(p, n)·e²`. If `i > 0`: `s = (5/(i² + 5) − 0.04·i)²`; the projection matrix is multiplied by: rotation of angle `angle` around the axis `(0, √2/2, √2/2)`, then scale `(1/s, 1, 1)`, then rotation of `−angle` around the same axis. Applied after the view bob.

---

## 3. Titles

### 3.1 Packets

| packet | id | fields |
|---|---|---|
| `set_title_text` | 0x74 | text: NBT text component |
| `set_subtitle_text` | 0x72 | text: NBT text component |
| `set_titles_animation` | 0x75 | fade in: `i32`, stay: `i32`, fade out: `i32` (ticks, 32-bit big-endian integers, not varint) |
| `clear_titles` | 0x0E | reset the durations: bool |

### 3.2 State and processing

State: `title` (optional), `subtitle` (optional), timer `t` (ticks), durations `A` (fade in), `M` (stay), `D` (fade out); defaults **A = 10, M = 70, D = 20**.

- `set_title_text`: `title = text`; `t = A + M + D`. The subtitle in memory is kept.
- `set_subtitle_text`: `subtitle = text` only; neither starts nor restarts the timer (it is displayed with the next title, or right away if a title is in progress).
- `set_titles_animation`: each value ≥ 0 replaces the corresponding duration (a value < 0 leaves the current duration); then, if `t > 0`, `t = A + M + D` (the title in progress **restarts** from the beginning of the fade in).
- `clear_titles`: `title = subtitle = absent`, `t = 0`; if the boolean is true, durations reset to 10 / 70 / 20.
- Tick (outside pause): if `t > 0`: `t −= 1`; if it reaches 0: title and subtitle cleared.
- Disconnection: clearing and default durations.

### 3.3 Alpha

With `f = t − tick_fraction`:
- by default `α = 255`;
- if `t > D + M` (fade-in phase): `α = trunc((A + M + D − f)·255 / A)`;
- if `t ≤ D` (fade-out phase): `α = trunc(f·255 / D)`;
- `α = clamp(α, 0, 255)`. Nothing is drawn if `α = 0`.
Truncation toward zero of a float (a division by zero with `A = 0` gives +∞ → 255, or 0/0 → 0 on the very first frame; see Uncertain points).

### 3.4 Rendering

Conditions: HUD not hidden, title present, `t > 0`, `α > 0`. Position in the HUD order: after the action bar, before the chat (`hud.md` § 1.3, point 5), in a **new stratum**.

- Origin moved to `(L div 2, H div 2)`.
- Title, at **×4** scale: title width `l` (font units); text at `(−l div 2, −10)` of the enlarged frame. That is, on screen, top-left corner `(L div 2 − 4·(l div 2), H div 2 − 40)`.
- Subtitle (if it exists), at **×2** scale: width `l'`; text at `(−l' div 2, 5)`. That is on screen `(L div 2 − 2·(l' div 2), H div 2 + 10)`.
- Text colour: white with alpha `α` (`α << 24 | 0xFFFFFF`), **with shadow**. The component's own colours apply, multiplied by this alpha.
- Background ("text with background"): only if the "text background" option is "everywhere" (default "chat only" → no background). Rectangle in the enlarged frame `(x − 2, y − 2) → (x + l + 2, y + 9 + 2)`, colour = black with the "text background opacity" (default 0.5) **multiplied component by component** by the text colour (so the background's alpha follows the fade). Drawn before the text. Same rule as `hud.md` § 5.1.

Existing summary reminder: `chat.md` (paragraph "Titles"); the present section is authoritative.

---

## 4. Resources

- `textures/gui/sprites/boss_bar/{pink,blue,red,green,yellow,purple,white}_{background,progress}.png` and `notched_{6,10,12,20}_{background,progress}.png` (182×5).
- `textures/gui/sprites/hud/effect_background.png`, `effect_background_ambient.png` (24×24).
- `textures/mob_effect/<effect>.png` (40 files, 18×18), sprites `mob_effect/<effect>` of the `gui` atlas.
- `textures/misc/nausea.png` (nausea overlay, `hud.md` § 5.5).
- Sound event `music.dragon`.

---

## 5. Uncertain points

- **Progress outside [0, 1]**: not clamped by the client; `p > 1` gives `w > 182` and a crop that reads beyond the sprite in the atlas (visual overflow). Recommendation: clamp `w` to [0, 182] on our side; difference visible only with a server that sends aberrant values.
- **Add on an existing id**: the official table (ordered insertion) keeps the initial position on a replacement; not tested in game.
- **A = 0** in `set_titles_animation`: the fade-in formula divides by zero; first frame possibly invisible (0/0), then 255. Reproduce as is or force 255: a one-frame difference.
- **D = 0**: the fade-out phase is never reached (the render requires `t > 0`), hence a hard cut; consistent.
- **Screens displaying effects**: the exact condition (which screens, available room) has not been detailed here.
- **Boss music**: the choice and sequencing of tracks (music manager) are not described; only the selection rule is.
- **Effect colours**: only four values noted; the full table is to be extracted from the registry if the exact sorting by colour matters (it only separates equal durations).
