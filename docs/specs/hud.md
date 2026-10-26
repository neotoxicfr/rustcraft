# In-game interface (HUD) outside the chat (26.3)

Clean-room spec, written from a reading of the decompiled 26.3 client (kept outside the repository). The chat, the font and text rendering (drop shadow, glyph width) belong to `chat.md`; the 3D rendering of item models belongs to `models.md`. Here: the interface frame, layers, sprites, crosshair, hotbar, health/food/armor/air/experience/mount bars, held item name, action bar, vignette, effects, death screen, and the packets that feed all of this.

Conventions:
- "GUI" units: virtual interface pixels (see § 1.1). `L` = GUI width, `H` = GUI height, `cx = L / 2` (integer division). Origin at the **top left**, y downward.
- `rect(x, y, w, h)`: full sprite quad, top-left corner (x, y), size w×h.
- Divisions `/` on integers: truncated integer division (toward zero), unless stated otherwise.
- "tick" = client tick (20 per second). `t` = HUD tick counter (incremented once per tick outside pause).
- Colours written `0xAARRGGBB`.
- Blends (src/dst):

| Name here | Colour | Alpha |
|---|---|---|
| **translucent** (GUI default) | `src·αs + dst·(1−αs)` | `αs + αd·(1−αs)` |
| **premultiplied** | `src + dst·(1−αs)` | `αs + αd·(1−αs)` |
| **inversion** | `src·(1−dst) + dst·(1−src)` | `αs` (dst ignored) |
| **vignette** | `dst·(1−src)` | `αd` (unchanged) |
| **additive** | `src + dst` | `αs + αd` |

---

## 1. Frame, layers, sprites

### 1.1 GUI scale (automatic choice)
- Option "interface scale": 0 = auto (default), otherwise a fixed value capped by the auto maximum.
- Computation (window in physical pixels `Wf × Hf`, `max` = option value, 0 = unlimited):
  `s = 1`; while `s ≠ max` **and** `s < Wf` **and** `s < Hf` **and** `Wf / (s+1) ≥ 320` **and** `Hf / (s+1) ≥ 240`: `s += 1`.
  If the Unicode font is forced and `s` is odd: `s += 1`.
- `L = ceil(Wf / s)`, `H = ceil(Hf / s)` (rounded **up**).
- Examples: 1920×1080 → 4 (480×270); 1280×720 → 3 (427×240); 854×480 → 2 (427×240).

### 1.2 Projection
- Orthographic, left 0, right `Wf / s` (float), top 0, bottom `Hf / s`, y flipped (top of the screen = 0), near plane 1000 and far plane 11000; a translation z = −11000 is applied to the whole interface (all the 2D lives at z ≈ 0 in this frame). No useful depth: the order is handled by the layers (§ 1.3).
- Vertex coordinates = GUI integers, transformed by a 3×2 2D matrix (pose stack); one GUI pixel covers `s×s` physical pixels.

### 1.3 Drawing order: strata and layers
The interface is not drawn in raw call order. Model:
- A GUI frame = list of **strata**. A stratum = chain of stacked **layers** (layer 0, then "above", etc.). The strata are drawn in order, and within a stratum the layers from bottom to top.
- Automatic placement of an element (sprite, rectangle, item, text) according to its bounding rectangle (after pose and scissor):
  1. if the rectangle of the **previous** element entirely contains the new one → the new one goes into the layer just above the current layer;
  2. otherwise start from the highest layer of the current stratum and go down until a layer containing an element that **intersects** the new one is found; the element goes into the layer just above. If there is no intersection: layer 0.
- Within a layer: first the "solid" elements (sprites, rectangles, items already converted to quads), **sorted** by (scissor, pipeline, texture) to group calls, then the text glyphs (text always goes above the quads of the same layer).
- Consequence to reproduce: inside a layer, the relative order of sprites that do not overlap is irrelevant; sprites that overlap go up one layer, so the call order of § 2 to 5 is enough to get the right result.
- `new stratum` (below) forces a new start above everything that precedes.

Call order of an in-game frame (no world loading screen; "HUD hidden" = F1 key):
1. Initial stratum, if HUD not hidden: **camera overlays** (vignette § 5.3, then in first person pumpkin/spyglass, powder snow, portal or nausea).
2. If HUD not hidden: new stratum → **crosshair** (§ 2).
3. If HUD not hidden: new stratum → **hotbar** (§ 3) → **health, armor, food, air** (§ 4) → **mount health** → **contextual bar** (background) → **experience level** (text) → contextual bar (foreground) → **held item name** (§ 5.1) → **effects** (§ 5.4) → boss bars.
4. Sleeping (even with HUD hidden): new stratum, full-screen veil colour `0x101010` with alpha `220·a`, `a = sleep/100`; if `a > 1`, `a = 1 − (sleep − 100)/10`.
5. If HUD not hidden, each in a new stratum if it is drawn: demo, sidebar scoreboard, **action bar** (§ 5.2), titles, chat, player list, subtitles (deferred).
6. Then the open screen if any, the save indicator, the notifications (toasts), the debug screen.

### 1.4 Interface sprites (`gui` atlas)
- Atlas `textures/atlas/gui.png` built from **all** of `assets/minecraft/textures/gui/sprites/` (id = relative path without extension, e.g. `hud/hotbar`) and from `assets/minecraft/textures/mob_effect/` with the prefix `mob_effect/` (e.g. `mob_effect/speed`). No mipmaps.
- Filtering: nearest (no smoothing), including for the item atlas (§ 3.4). Exception: a texture outside the atlas whose `.mcmeta` carries `"texture": {"blur": true}` is sampled bilinearly (case of `misc/vignette.png`).
- Animated sprites: the standard `animation` `.mcmeta` (frames stacked vertically, `time` in ticks) also applies to GUI sprites (e.g. arrows of the locator bar).
- Scaling (`gui.scaling` section of the `.mcmeta`, default **stretch**):
  - **stretch**: the whole sprite is stretched over the requested rectangle.
  - **tile** `{width, height}`: the sprite is repeated in tiles of `width×height` GUI pixels from the top-left corner; the last tile is cropped **[not verified: proportional cropping of the UVs]**.
  - **nine_slice** `{width, height, border, stretch_inner?}`: `border` = an integer (4 sides) or `{left, top, right, bottom}`; borders clamped to half the requested size. Requested size = nominal size → direct drawing. Otherwise: corners drawn at fixed size, edges and centre filled with **tiles** (or stretched if `stretch_inner = true`). If only the height (resp. width) equals the nominal, split in 3 only.
- Drawing a **portion** of a sprite (progress bars): for a stretch sprite, sub-rectangle `(u, v, w, h)` of a sprite of nominal size `(Sw, Sh)` → UVs interpolated between the sprite's edges in the atlas at fractions `u/Sw … (u+w)/Sw`, `v/Sh … (v+h)/Sh`. For a tile/nine-slice sprite: whole drawing under scissor.
- Sprite shader: `colour = texture × vertex_colour`; **fragment discarded if resulting alpha = 0**; then × colour modulator (1 by default). **Translucent** blend unless stated otherwise.
- Tint: most sprites have opaque white colour; an alpha `a ∈ [0,1]` translates to the colour `(255·a, 255, 255, 255)` in ARGB.

---

## 2. Crosshair

- Conditions (all): HUD not hidden; **first-person** view; mode ≠ spectator, or spectator aiming at an entity offering a menu or a "spectator-interactive" block (chests, etc.); the 3D debug crosshair is not active.
- Sprite `hud/crosshair` (15×15), `rect((L − 15)/2, (H − 15)/2, 15, 15)`, **inversion** blend: each white pixel of the sprite takes the inverse colour of what it covers; alpha-0 pixels discarded.
- Attack indicator (option "attack indicator", default **crosshair**): same inversion blend, at `x = L/2 − 8`, `y = H/2 − 7 + 16`:
  - `strength = attack cooldown progress` (0..1, `models`/combat out of scope).
  - "full" if a living entity is aimed at, `strength ≥ 1`, held item cooldown delay `> 5` ticks, living entity, and target within the item's range if the item defines a range → `rect(x, y, 16, 16)` sprite `hud/crosshair_attack_indicator_full`.
  - otherwise if `strength < 1`: background `hud/crosshair_attack_indicator_background` `rect(x, y, 16, 4)` then progress `hud/crosshair_attack_indicator_progress` portion `(0, 0, p, 4)` drawn at `rect(x, y, p, 4)`, `p = floor(strength·17)`.
- Option "attack indicator" = hotbar: see § 3.3. Disabled: nothing.

---

## 3. Hotbar

Spectator mode: spectator-specific bar (teleport menu), **out of scope**. Otherwise (survival, adventure, creative):

### 3.1 Sprites and positions
| Element | Sprite (size) | Rectangle |
|---|---|---|
| Background | `hud/hotbar` (182×22) | `(cx − 91, H − 22, 182, 22)` |
| Selection | `hud/hotbar_selection` (24×23) | `(cx − 92 + 20·sel, H − 23, 24, 23)` |
| Offhand slot (if offhand non-empty), main hand **right** | `hud/hotbar_offhand_left` (29×24) | `(cx − 120, H − 23, 29, 24)` |
| same, main hand **left** | `hud/hotbar_offhand_right` (29×24) | `(cx + 91, H − 23, 29, 24)` |
| Item of slot i (0..8) | 16×16 item | `(cx − 88 + 20·i, H − 19)` |
| Offhand item (left) | 16×16 item | `(cx − 117, H − 19)` |
| Offhand item (right) | 16×16 item | `(cx + 101, H − 19)` |

- Order: background, selection, offhand slot, then the 9 items (variant seed 1..9), then the offhand item (seed 10), then the attack indicator if any. Each item is followed by its decorations (§ 3.5).
- The offhand is placed on the side **opposite** to the main hand (option "main hand", default right → slot to the left of the bar).

### 3.2 Pickup "pop"
Each stack carries a counter `pop` (ticks, decremented by 1 per inventory tick). Set to 5 when an item arrives in a hotbar slot (§ 6.3). Render: if `f = pop − tick_fraction > 0`, `k = 1 + f/5`, the item (not the decorations) is scaled by `(1/k, (k + 1)/2)` around the point `(x + 8, y + 12)`.

### 3.3 Attack indicator on the hotbar
If the option is "hotbar" and `strength < 1`: `y = H − 20`, `x = cx + 97` (or `cx − 113` if the main hand is **left**); background `hud/hotbar_attack_indicator_background` `rect(x, y, 18, 18)`, then bottom portion of `hud/hotbar_attack_indicator_progress`: `p = floor(strength·19)`, source `(0, 18 − p, 18, p)` at `rect(x, y + 18 − p, 18, p)` (filling from bottom to top).

### 3.4 Item in a slot (what has to be implemented)
- The item model is resolved in the **GUI** display context (`display.gui` transform of the model, `models.md`), with the player and the world as owners (dependent properties: clock, compass, etc.).
- It is rendered in real 3D into an **item atlas texture**: one cell of `16·s` physical pixels per distinct model (cache by model identity; animated models are redrawn), orthographic projection with depth −1000..1000, pose = translation to the centre of the cell then scale `(16s, −16s, 16s)` (the model fits in a centred unit cube). Transparent background (0,0,0,0).
- Lighting: if the model uses block lighting ("side_lit", case of 3D blocks) → "3D items" lighting, otherwise "flat items". Two directional lights, base directions `n(0.2, 1, −0.7)` and `n(−0.2, 1, 0.7)`:
  - flat: directions transformed by `Ry(−π/8)·Rx(3π/4)`;
  - 3D: by `diag(1,−1,1)·Ryxz(1.0821041, 3.2375858, 0)·Ryxz(−π/8, 3π/4, 0)` (YXZ rotations composed in this order) **[not verified: diffuse formula, see `lighting.md` for the entities' ambient term]**.
- The atlas cell is then pasted at `rect(x, y, 16, 16)` with the **premultiplied** blend, nearest filtering.
- Enchantment glint: rendered in the same 3D pass **[not verified]**.
- "Oversized" items (model overflowing 16×16): rendered into a dedicated texture — out of scope.

### 3.5 Decorations of a stack
In order, current pose:
1. **Durability bar** if the item is damaged (damage > 0): opaque black rectangle `(x + 2, y + 13) → (x + 15, y + 15)` (13×2); then opaque colour rectangle `(x + 2, y + 13)` of width `w`, height 1.
   `w = clamp(round(13 − damage·13/max), 0, 13)`; colour = HSV → RGB with hue `max(0, (max − damage)/max) / 3`, saturation 1, value 1 (green → red). (The bag has its own bar — out of scope.)
2. **Cooldown** (item cooldown, fraction `c ∈ ]0,1]`): rectangle `0x7FFFFFFF` (half-transparent white) from `(x, y + floor(16·(1 − c)))` over `ceil(16·c)` high, 16 wide.
3. **Counter** if quantity ≠ 1: decimal text white (`0xFFFFFFFF`) **with shadow**, at `x + 17 − width(text)`, `y + 9` (right-aligned on the slot's edge).

### 3.6 Selection: keys and wheel
- Keys 1..9 (`hotbar.1..9`): slot 0..8 (spectator: spectator menu; creative + hotbar save/load key held: save/load, out of scope).
- Wheel, with no screen open: accumulated scroll (discrete scrolling option → sign only; × sensitivity); accumulate, reset the accumulator if the sign changes, remove the integer part. `notches = notchY ≠ 0 ? notchY : −notchX`. New slot = `sel − sign(notches)`, brought into 0..8 by wrapping (wheel up → previous slot). Spectator: flight speed `clamp(v + 0.005·notchY, 0, 0.2)`.
- Sent to the server: see § 6.2 (not immediate).

---

## 4. Survival bars

Displayed only if the mode is **survival or adventure** (health, armor, food, air). In creative and spectator: none of these bars; experience neither (§ 4.6); only the held item name goes down by 14 (§ 5.1).

Landmarks: `xL = cx − 91` (left edge), `xR = cx + 91` (right edge), `yBase = H − 39`. Hearts, food, armor, bubbles: 9×9 sprites spaced by **8**.

### 4.1 Health: computations
- `health = ceil(hp)`, `abs = ceil(absorption)`, `effective_max_health = max(max health attribute, displayedHealth, health)`.
- Blink tracking (every frame):
  - if `health < lastHealth` and the player has a hurt delay > 0: `lastTime = now(ms)`, `blinkEnd = t + 20`;
  - if `health > lastHealth` and delay > 0: same with `blinkEnd = t + 10`;
  - if `now − lastTime > 1000 ms`: `displayedHealth = health`, `lastTime = now`;
  - `lastHealth = health`.
  - `blinking = blinkEnd > t and ((blinkEnd − t)/3) mod 2 = 1`.
- `rows = ceil((effective_max_health + abs) / 2 / 10)`, `rowHeight = max(10 − (rows − 2), 3)`.
- Regeneration active: `regenIdx = t mod ceil(effective_max_health + 5)`, otherwise −1.
- Random draw: generator reseeded every frame with `t·312871` (the game's LCG generator, `models`/random **[not verified: same sequence as the game]**).

### 4.2 Armor (drawn before the hearts)
If `armor = floor(armor attribute) > 0`: `y = yBase − (rows − 1)·rowHeight − 10`; for i = 0..9 at `x = xL + 8i`: `hud/armor_full` if `2i + 1 < armor`, `hud/armor_half` if `= armor`, `hud/armor_empty` otherwise.

### 4.3 Hearts
- Type: poison → `poisoned`, otherwise wither → `withered`, otherwise fully frozen → `frozen`, otherwise normal. Hardcore mode: `hardcore` variants.
- Sprites `hud/heart/<name>`: container `container`, `container_blinking`, `container_hardcore`, `container_hardcore_blinking`; fill `full`, `half`, `hardcore_full`, `hardcore_half` and for each type `T ∈ {poisoned, withered, absorbing, frozen}`: `T_full`, `T_half`, `T_hardcore_full`, `T_hardcore_half`; each with a `_blinking` variant.
- `nV = ceil(effective_max_health / 2)`, `nA = ceil(abs / 2)`. Loop over `c` from `nV + nA − 1` **down to 0 (descending order)**:
  - `x = xL + 8·(c mod 10)`, `y = yBase − (c / 10)·rowHeight`;
  - if `health + abs ≤ 4`: `y += random(0..1)` (shaking, one draw per heart in this order);
  - if `c < nV` and `c = regenIdx`: `y −= 2`;
  - container (blinking variant if `blinking`);
  - `h = 2c`; if `c ≥ nV` (absorption heart) and `h − 2nV < abs`: `absorbing` heart (or `withered` if the type is wither), half if `h − 2nV + 1 = abs`, never blinking;
  - if `blinking` and `h < displayedHealth`: fill of the type in **blinking** variant, half if `h + 1 = displayedHealth` (shows the old health in white behind);
  - if `h < health`: normal fill of the type, half if `h + 1 = health`.

### 4.4 Food
Drawn if the player is not riding a living entity that displays its health (otherwise the mount's health takes its place, § 4.7).
- Sprites: `hud/food_empty|half|full`, or `hud/food_*_hunger` under the Hunger effect.
- For i = 0..9: `x = xR − 8i − 9`, `y = yBase`; if saturation ≤ 0 and `t mod (food·3 + 1) = 0`: `y += random(0..2) − 1` (draw per icon). Empty always, then full if `2i + 1 < food`, half if `= food`.

### 4.5 Air (bubbles)
- `max = 300` (player), `air = clamp(currentAir, 0, max)`, `underwater` = eyes in water. Displayed if `underwater` or `air < max`.
- `y = yBase − 10`, then another `−10` if food is displayed; then `−10·(mountRows − 1)` if a mount displays hearts (`mountRows = ceil(mountHearts/10)`).
- `B(off) = ceil((air + off)·10 / max)`. `full = B(−2)`, `burstPos = B(0)`, `empty = 10 − B(underwater and air ≠ 0 ? 1 : 0)`.
- For b = 1..10, `x = xR − 8(b − 1) − 9`:
  - `b ≤ full` → `hud/air`;
  - otherwise if `full ≠ burstPos` and `b = burstPos` and `underwater` → `hud/air_bursting` + "bubble pops" sound if `b` ≠ last sounded bubble (volume `0.5 + 0.1·max(0, empty − 2)`, pitch `1 + 0.1·max(0, empty − 4)`);
  - otherwise if `b > 10 − empty` → `hud/air_empty`, offset by `random(0..1)` in y if `empty = 10` and `t` even.
  - Out of the water: the memory of the last sounded bubble is reset to 0.
- Source of `currentAir`: entity data 1 of the local player, maintained by the **server alone** (the client does not evolve it) and sent on each change; default value 300. The server never sends a data value that went back to its default on a new player: on respawn, the client creates a player with default data and only takes over the old ones if the respawn packet carries the "keep entity data" flag (2). The integrated server publishes this data the same way (`rc-sim` `vitals.rs`).

### 4.6 Contextual bar and experience
A single 182×5 bar at `(cx' , H − 29)` with `cx' = (L − 182)/2`. Choice:
- if waypoints exist (locator bar, § 4.8): jump bar if jumping mount **and** (jump charge > 0 or jump cooldown); otherwise experience if survival/adventure mode **and** XP progress changed less than 100 ticks ago; otherwise locator;
- otherwise: jump if jumping mount; otherwise experience if survival/adventure; otherwise nothing.

**Experience**: `hud/experience_bar_background` full, then `hud/experience_bar_progress` portion `(0, 0, p, 5)`, `p = floor(progress·183)` (if `p > 0`). Nothing if the XP required for the next level ≤ 0.

**Level** (survival/adventure and level > 0, whatever the bar type): text = the number alone, `x = (L − width)/2`, `y = H − 35`; drawn **5 times without shadow**: at `(x+1, y)`, `(x−1, y)`, `(x, y+1)`, `(x, y−1)` colour `0xFF000000`, then at `(x, y)` colour `0xFF80FF20` (green).

**Mount jump**: `hud/jump_bar_background`; if cooldown > 0: `hud/jump_bar_cooldown` full; otherwise `hud/jump_bar_progress` portion of width `q = (charge > 0) ? floor(charge·181) + 1 : 0`.

### 4.7 Mount health
If the player is riding a living entity that displays its health: `hearts = min(30, floor(maxHealth + 0.5)/2)`, `mountHealth = ceil(health)`. Rows of 10 starting at `y = H − 39`, each following row 10 higher; `x = xR − 8i − 9`; `hud/heart/vehicle_container`, then `vehicle_full` if `2i + 1 + 20·row < mountHealth`, `vehicle_half` if `=`. Drawn even in creative (not subject to the survival condition).

### 4.8 Locator bar (overview)
Background `hud/locator_bar_background` (nine-slice 12×5, borders 5/1). Waypoints whose yaw angle to the camera is in ]−60°, 60°]: 9×9 dot at `x = ceil((L − 9)/2) + floor(angle·173/120)`, `y = H − 31`, colour = the waypoint's colour or colour derived from the hash of its UUID/name (brightness 0.9); 7×5 up/down arrow if the waypoint is above/below (`y ∓ 6`). Dot styles (`hud/locator_bar_dot/...`) depending on distance **[not verified: thresholds]**. The camera player itself is ignored. Fed by the waypoint tracking packet — out of scope of this spec.

---

## 5. Other elements

### 5.1 Held item name
- Each tick: if the selected slot is empty → timer 0; otherwise if the item changes (other item, or other displayed name) → timer `= floor(40 × toast display time)` (option, default 1.0 → **40 ticks**); otherwise timer −1 if it is > 0.
- Render (outside spectator) if timer > 0: displayed name of the item, colour of its rarity (common white, uncommon yellow, rare cyan, epic light purple), in **italic** if it has a custom name. `x = (L − width)/2`, `y = H − 59` (`+14` outside survival/adventure). Alpha `min(255, floor(timer·256/10))` (fade over the last 10 ticks, no partial-tick interpolation); text **with shadow**, colour white × alpha.
- Text background: only if the "text background" option is "everywhere" (default: chat only → no background): rectangle `(x − 2, y − 2) → (x + width + 2, y + 11)` colour black with "background opacity" (default 0.5) multiplied by the text colour. Same rule for the action bar and the titles.

### 5.2 Action bar (overlay)
- Received through the action bar packet (or a system message marked "overlay", `chat.md`): duration **60 ticks**; record played → text "Now Playing: …" with animated colour.
- Render if duration > 0: `f = duration − tick_fraction`, alpha `min(255, floor(f·255/20))` (fade over 20 ticks); if alpha > 0: new stratum, centred text: `x = cx − width/2`, `y = H − 68 − 4`, with shadow (and background per § 5.1). Colour: white, or animated = HSV `(f/50, 0.7, 0.6)` with this alpha.

### 5.3 Vignette
- Option "vignette" (default enabled). Drawn first (under the whole HUD), full screen `(0, 0, L, H)` with the texture `textures/misc/vignette.png` (256×256, bilinear filtering), UV 0..1, **vignette** blend (darkens: `dst·(1 − tex·colour)`).
- Smoothed brightness `v` (start 1.0), updated every tick: `target = clamp(1 − brightness(max raw light at the camera's eye block), 0, 1)` (the dimension's brightness function, `lighting.md`); `v += (target − v)·0.01`.
- Colour: `b = clamp(v, 0, 1)` → `(b, b, b)`. Near the world border (distance `d` < threshold `S`, `S = max(warning blocks, min(speed·warning time, |target − size|))`): `w = 1 − d/S` → colour `(b·(1 − w), b + (1 − b)·w, b + (1 − b)·w)` (red tint).
- Effect: very slight darkness in full daylight (target ≈ 0 → no darkening), edges darkened in the dark.

### 5.4 Status effects (top right)
- Drawn if the player has effects and no screen that itself displays the effects is open. Only the effects with "show icon".
- Sorting: **reversed** natural order of the instances (natural order: non-ambient before ambient, non-infinite before infinite, then increasing duration, then colour; if both durations exceed 32147 or both are ambient: ambient then colour only).
- Placement: beneficial effects in row y = 1, harmful in y = 27 (demo: +15). The n-th of its category at `x = L − 25·n`.
- 24×24 background: `hud/effect_background_ambient` (ambient, e.g. beacon) or `hud/effect_background`. Icon `mob_effect/<name>` 18×18 at `(x + 3, y + 3)` with alpha `a`:
  - `a = 1` except non-ambient effect ending within ≤ 200 ticks: `d` = remaining duration, `u = 10 − d/20`;
    `a = clamp(d/10/5·0.5, 0, 0.5) + cos(d·π/5)·clamp(u/10·0.25, 0, 0.25)`, clamped to [0, 1] (accelerating blinking).

### 5.5 Camera overlays (brief reminder)
- Carved pumpkin / equipped items with a camera overlay: opaque white full-screen texture (first person, outside spyglass).
- Spyglass: `textures/misc/spyglass_scope.png` square of side `min(L, H)·k` centred (k smoothed toward 1.125, start 0.5), black bands around.
- Powder snow: `textures/misc/powder_snow_outline.png` full screen, alpha = freeze fraction.
- Portal: Nether portal particle sprite full screen, alpha `a` (if `a < 1`: `a⁴·0.8 + 0.2`). Otherwise nausea (if the screen effects scale is < 1): `textures/misc/nausea.png` additive blend, colour `(0.2, 0.4, 0.2)·strength`, scaled by `lerp(strength, 2, 1)` around the centre, `strength = intensity·(1 − scale)`.

### 5.6 Death screen (summary)
- Opened on receiving the local player's kill packet (if the death screen is enabled by the game rule; otherwise the client requests respawn immediately).
- Background: vertical gradient `0x60500000` (top) → `0xA0803030` (bottom) over the whole screen (the HUD stays underneath, not hidden).
- Title "You Died!" ("Game Over!" in hardcore) at ×2 scale, centred at `((L/2)/2, 30)` in the ×2 frame (i.e. screen centre, y = 60 GUI); cause of death centred at y = 85; "Score: <n>" (n in yellow) at y = 100.
- 200×20 buttons at `(L/2 − 100, H/4 + 72)` "Respawn" ("Spectate World" in hardcore) and `(L/2 − 100, H/4 + 96)` "Title Screen"; inactive during the first 20 ticks. Escape does not close.
- Respawn → client command packet action 0 (respawn), § 6.
- On the respawn packet, the local player is new: full health and fresh food (20, saturation 5) until the health packet, which the server only sends on the next tick; health, absorption and score (entity data) are only taken over from the old player with flag 2. A client that kept health at 0 in this interval would reopen the death screen (opening "dead player without screen").

---

## 6. Network packets (play phase)

Fields in write order. The numeric ids follow the project's protocol 777 table.

### 6.1 Server → client
| Packet | Fields | Client-side effect |
|---|---|---|
| Health update (`set_health`) | health `f32`, food VarInt, saturation `f32` | health set (if decrease and not the first: hurt delay 20, hit animation; if increase: delay 10; the very first packet triggers nothing); food and saturation set |
| Experience (`set_experience`) | progress `f32` (0..1), **level** VarInt, **total** VarInt | if the progress changes: start of the priority XP display (100 ticks, § 4.6) |
| Selected slot (`set_held_slot`) | slot VarInt | applied to the selection if 0..8, without touching the "last sent slot" (§ 6.2): on the next tick the client therefore sends `set_carried_item` back with this same slot (acknowledgement in effect) |
| Container contents (`container_set_content`) | container id VarInt, state VarInt, list of stacks (VarInt n + n optional stacks), stack carried by the cursor | container 0 = player inventory (indices § 6.3) |
| Container slot (`container_set_slot`) | container id VarInt, state VarInt, slot `i16`, optional stack | container 0: if hotbar slot (36..44), non-empty stack and (old empty or greater quantity) → `pop = 5` |
| Player inventory slot (`set_player_inventory`) | slot VarInt (inventory indices § 6.3), optional stack | direct set |
| Cursor item (`set_cursor_item`) | optional stack | — |
| Player death (`player_combat_kill`) | entity id VarInt, message (text component) | death screen § 5.6 |

Optional stack: quantity VarInt; if > 0: item id VarInt then the component patch (number added VarInt, number removed VarInt, then entries) — component details out of scope.

Other sources of the bars (outside the packets above): max health and armor = **attributes** (attribute update packet); air, absorption = synchronised **entity data**; effects = effect add/remove packets; game mode = login/respawn and game event; item cooldowns = cooldown packet.

### 6.2 Client → server
- **Carried item** (`set_carried_item`): slot `i16`. The client keeps the "last sent slot"; before each action that depends on it (break/continue breaking, use on a block, use an item, attack, interact with an entity, release use, piercing attack, drop) **and on every tick** of the game mode, if the selected slot differs, it sends this packet. Changing slot by key or wheel therefore sends nothing immediately: the send goes out on the next tick.
- **Client command**: VarInt action (0 = respawn, 1 = request statistics, 2 = request game rules).

### 6.3 Slot indices
- Inventory (packet `set_player_inventory`, selection): 0..8 hotbar, 9..35 main storage, 36..39 feet/legs/chest/head, 40 offhand, 41 body armor, 42 saddle.
- Container 0 (inventory menu): 0 crafting result, 1..4 2×2 grid, 5..8 armor (head → feet), 9..35 main storage, 36..44 hotbar, 45 offhand.

---

## 7. Resources to load
- `textures/gui/sprites/hud/**` (bar, selection, offhand slots, attack indicators, armor, food, air, XP/jump/locator bars, effect backgrounds, `heart/*`: 47 files), `textures/mob_effect/*.png`, their `.mcmeta`.
- `textures/misc/vignette.png` (+ `.mcmeta` blur), `nausea.png`, `spyglass_scope.png`, `powder_snow_outline.png`, `pumpkinblur.png` (overlay of the carved pumpkin's equippable component, path `textures/<id>.png`).
- Language: `gui.experience.level` (= "%s"), `deathScreen.*`, `record.nowPlaying`.
- Sounds: `ui.hud.bubble_pop` (bubble pops).
