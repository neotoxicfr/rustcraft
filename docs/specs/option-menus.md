# Pause menu, options screens and the `options.txt` file (26.3, protocol 777)

Source: reading of the decompiled client (kept outside the repository), reworded. Units = interface pixels
("GUI units", after the interface scale, see `chat.md` § scale). `W`, `H` = screen width and
height in GUI units. Integer divisions unless stated otherwise. Colors in hexadecimal ARGB.
Cross-references: `title-screen-servers.md` (panorama, title screen), `inventories.md` § 4.7 (tooltip
box), `text.md` (font), `chat.md` § 3.1 (chat settings), `sounds.md`.

---

## 1. Common interface components

### 1.1 General rules

- Every component has a rectangle `(x, y, w, h)`, a message (text), two flags "active" and
  "visible", an opacity `α` (1 by default) and a "focus" flag.
- **Hover**: recomputed every frame = mouse inside the rectangle (left/top edges included,
  right/bottom excluded) **and** inside the current clipping rectangle.
- **Click**: only the left button counts (number 1 in SDL convention: 1 left, 2 middle,
  3 right). An inactive or invisible component ignores the mouse.
- **Component text**: white, with shadow, alpha = the component's `α`. An inactive component
  shows its message recolored to **0xFFA0A0A0** (only the parts without a color of their own).
- **Centered scrolling text** (buttons, sliders, tabs): zone `[x + 2, x + w − 2]` (margin 2),
  line 9 high, `y_text = (top + bottom − 9) / 2 + 1` (that is `y + 6` for a height of 20).
  - If the text width `t` ≤ zone width `z`: centered on the middle of the zone, clamped
    so it does not overflow (`x_center = clamp(middle, left + t/2, right − t/2)`).
  - Otherwise: `overflow = t − z`, `period = max(overflow × 0.5; 3)` seconds,
    `s = sin(π/2 × cos(2π × time / period)) / 2 + 0.5` (real time in seconds),
    offset = `s × overflow`; text left-aligned at `left − ⌊offset⌋`, clipped to the
    zone `[left, right] × [top, bottom]`. Smooth back and forth, no pause.
- **Click sound**: `ui.button.click`, category "ui", volume 0.25, pitch 1, no
  attenuation, not positioned. Played on press (button, checkbox, cycling button); for a slider,
  on **release** only.
- **Confirm keys** ("select"): Enter (scancode 40), Space (44), numpad Enter
  (88). Esc = 41, Tab = 43.
- **System cursor** (option "change the cursor"): pointing hand when hovering an
  active component, "forbidden" when hovering an inactive one; horizontal arrows while dragging a
  slider; insertion bar on a text field.

### 1.2 Nine-slice sprites

The sprites of the GUI atlas (`textures/gui/sprites/…`, see `hud.md`) can carry in their
`.png.mcmeta` a `gui.scaling` section: `stretch` (default), `tile` (width, height) or
`nine_slice` (width, reference height, `border` as an integer or `{left, top, right, bottom}`,
`stretch_inner` boolean, false by default).

Drawing a nine-slice of reference `Rw × Rh`, borders `(bl, bt, br, bb)`, into `(x, y, w, h)`:
- Effective clamped borders: `bl' = min(bl, w/2)`, `br' = min(br, w/2)`, `bt' = min(bt, h/2)`,
  `bb' = min(bb, h/2)`.
- If `w = Rw` and `h = Rh`: the whole image.
- If only the height equals `Rh`: three columns (left `bl'`, middle, right `br'`) over the whole
  height. If only the width equals `Rw`: three rows. Otherwise the nine pieces.
- The middle pieces (edges and center) are **tiled** (the source slice is repeated, at
  its original size); stretched if `stretch_inner`. The corners are copied as is.
- Source coordinates: corners `(0, 0)`, `(Rw − br', 0)`, `(0, Rh − bb')`, `(Rw − br', Rh − bb')`;
  central slice `[bl', Rw − br'] × [bt', Rh − bb']`.

| Sprite | Reference | Borders |
|---|---|---|
| `widget/button`, `_highlighted`, `_disabled` | 200 × 20 | 3 |
| `widget/slider`, `widget/slider_highlighted` | 200 × 20 | 1 |
| `widget/slider_handle`, `_highlighted` | 8 × 20 | l 2, t 2, r 2, b 3 |
| `widget/text_field`, `_highlighted` | 200 × 20 | 1 |
| `widget/scroller`, `widget/scroller_background` | 6 × 32 | 1 |
| `widget/tab`, `tab_selected`, `tab_highlighted`, `tab_selected_highlighted` | 130 × 24 | l 2, t 2, r 2, b 0 |
| `widget/checkbox` (+ `_highlighted`, `_selected`, `_selected_highlighted`) | 20 × 20 | stretched (no meta) |
| `tooltip/background` | 100 × 100 | 9 |
| `tooltip/frame` | 100 × 100 | 10, stretched center |

### 1.3 Button

- Usual sizes: width 150 (default), 120 (small), 200 (large), height 20, spacing 8.
- Sprite (nine-slice, white tint with alpha `α`):
  - inactive → `widget/button_disabled`;
  - active and (hovered **or** focused) → `widget/button_highlighted`;
  - otherwise → `widget/button`.
- Message: centered scrolling text (§ 1.1).
- Action: left click inside the rectangle (sound on press), or a confirm key when it has
  focus (sound too).
- **Icon button**: same background; icon (fixed-size sprite, e.g. 15 × 15 in 20 × 20)
  centered; the message appears only as a tooltip when "icon only".

### 1.4 Cycling button "name: value"

- Button rendering (§ 1.3). Label = key `options.generic_value` ("%s: %s") with the name and the
  text of the value; a "value only" variant is possible.
- Booleans: values in the order **true then false**, texts `options.on` / `options.off`.
- Click or confirm: next value (modulo); with **Shift** held: previous value.
  Wheel over the button: up → previous, down → next (but inside an option list the wheel
  scrolls the list, § 1.9).
- Tooltip specific to each value, recomputed on change.
- Alternative list (Alt key): possible, not used by the options listed here.
- For an **option**: each change applies the value, then **writes `options.txt`**
  (§ 4.4, which also sends the client information if it changed).

### 1.5 Slider

- Internal value `v ∈ [0, 1]`. Background: `widget/slider` (or `widget/slider_highlighted` if active,
  focused and **not** in keyboard adjust mode) at `(x, y, w, 20)`; handle 8 wide at
  `(x + ⌊v × (w − 8)⌋, y, 8, h)`: `widget/slider_handle_highlighted` if active and (hovered or in
  adjust mode), otherwise `widget/slider_handle`. Message: centered scrolling text, margin 2.
- Mouse: on press and while dragging, `v = (mouse_x − (x + 4)) / (w − 8)`, clamped to [0, 1].
  Sound on release.
- Keyboard: confirm = toggles the "adjust mode"; in adjust mode, ← / →:
  - discrete slider (integers, enumerations): previous / next value;
  - otherwise: `v ± 1 / (w − 8)`.
  Adjust mode is enabled automatically when focus arrives by mouse or by Tab, and disabled
  when focus is lost.
- For an option: each change of `v` recomputes the label from the "snapped" value
  (§ 3.1) and applies it right away, **except** the "deferred" sliders
  (render distance, simulation distance, biome blend): applied 600 ms after the
  last movement, and when the screen is closed. On release of an immediate slider, the
  handle snaps back to the value actually retained. Sliders **do not write**
  `options.txt` themselves (it is written when the screen is closed).

### 1.6 Checkbox

- Square of `9 + 8 = 17` side at `(x, y)`: `widget/checkbox[_selected][_highlighted]`
  ("highlighted" = focus only), white tint with alpha `α`.
- Text (at most 2 lines, wrapped to the width) at `x + 17 + 4`, vertically centered on the
  square; total width = `min(17 + 4 + text_width, max_width)`, height = `max(17,
  text height)`.
- Click / confirm: toggles the state and notifies the screen. (No option of the options screens
  uses it: they go through cycling buttons.)

### 1.7 Text field

- Background (if "with border", the default): `widget/text_field` (or `_highlighted` if focused; sprites
  identical for inactive). Inner width = `w − 8` with border, otherwise `w`.
- Text at `x + 4`, `y + (h − 8) / 2` (without border: `(x, y)`); color 0xFFE0E0E0 (non-
  editable: 0xFF707070), with shadow; max length 32 by default; only the visible part
  (from a horizontal scroll position) is drawn.
- Cursor: blinks, visible when `⌊(t_ms since focus was taken) / 300⌋` is even. At the end
  of the text: `_` character; in the middle (or full text): 1 px bar from `y − 1` to `y + 10`.
- Selection: inverted rectangle from `y − 1` to `y + 10`. Gray hint if empty and unfocused;
  suggestion in 0xFF808080 after the cursor.
- Keys (focus): Backspace / Delete (with Ctrl: whole word), Home / End, ← / →
  (Ctrl: word; Shift: extends the selection), Ctrl+A, Ctrl+C, Ctrl+V, Ctrl+X. Click: places the
  cursor at the character under the mouse.

### 1.8 Component tooltip

- Text wrapped to **170** wide. Box height = `10 × lines − 2` (no extra space
  after the 1st line, unlike items); box and text: `inventories.md`
  § 4.7. Drawn last, in a separate layer.
- Shown if hovered, or if focused and the last input was the keyboard. Appearance delay: 0 by
  default.
- Position (mouse `(sx, sy)`, component rectangle `R`, box `w × h`):
  - **hover (normal case)**: `x = sx + 12`; if `x + w > W − 5`: `x = max(sx − 12 − w, 9)`.
    `y = sy + 3`. With `ofs(a, b, hR) = round(lerp(min(|a − b|, hR) / hR, hR − 3, 5))`: if
    `R.bottom + 3 + ofs(0, 0, hR) + h + 6 ≤ H − 5`, then `y += ofs(y, R.top, hR)` (below the
    component), otherwise `y −= h + 6 + ofs(y, R.bottom, hR)` (above).
  - **keyboard focus without hover**: `x = R.left + 3`, `y = R.bottom + 4`; if `y + h + 3 > H`:
    `y = R.top − h − 4`; if `x + w > W`: `x = max(R.right − w − 3, 4)`.

### 1.9 Scrolling list

- Rectangle = the screen's content area (§ 1.12): `x = 0`, `y = header height`,
  `w = W`, `h = H − header − footer`. Rows of fixed width (`row width`, 220 by default)
  centered: `left = x + w/2 − row_width/2`.
- Each entry has its own height (the list's default height, or a specific one). First entry at
  `y + 2 − scroll`, the next ones stacked. Content height = sum + 4. Usable area of an
  entry = the entry shrunk by 2 on each side.
- Scrolling: `max = max(0, content − h)`, clamped to [0, max]. **Wheel**:
  `scroll −= notch × (default height / 2)` (integer division: 12 for the option lists,
  10 for the key bindings). An entry selected with the keyboard is brought back into the area
  (margin 2).
- Rendering, in order:
  1. Background: `textures/gui/menu_list_background.png` (in a world:
     `inworld_menu_list_background.png`), repeated every 32 units (16 × 16 texture so
     2 units per texel), texture coordinates = `(right, bottom + scroll)` → the pattern follows
     the scrolling.
  2. Visible entries, clipped to the rectangle. Selected entry (lists with selection, e.g.
     languages): solid white rectangle (focused list) or 0xFF808080, then the inside
     (shrunk by 1) black.
  3. Separators 2 high over the full width: `textures/gui/header_separator.png` at
     `y − 2`, `footer_separator.png` at `bottom` (in a world: `inworld_header_separator.png`,
     `inworld_footer_separator.png`); 32 × 2 textures repeated horizontally.
  4. Scrollbar, **only if `max > 0`**: x = row right + 6 + 2, width 6;
     background `widget/scroller_background` over the full height; thumb `widget/scroller` of
     height `clamp(⌊h² / content⌋, 32, h − 8)` at
     `y_t = max(y, ⌊scroll⌋ × (h − h_t) / max + y)`.
- Dragging the thumb (click in the scrollbar column): mouse above the list →
  scroll 0; below → max; otherwise `scroll += dy × max(1, max(1, maxScroll) / (h − h_t))`.
- Navigation: ↑ / ↓ move from one entry to the next; inside an entry with several components,
  spatial navigation (§ 1.11) applies.

### 1.10 Tabs (menu bar)

(Not used by the options screens; present in world creation and the friends list.)
- Bar 24 high at the top of the screen; usable width `min(400, W) − 28`; tab width =
  `usable width / n` rounded toward even; bar centered (x rounded toward even), y = 0.
- Header separator (2 high) from 0 to the first tab and from the last tab to `W`, at
  `y = 22`.
- Tab: nine-slice sprite `widget/tab_selected[_highlighted]` (selected) or
  `widget/tab[_highlighted]`; "highlighted" = hover or focus. Selected: menu background
  (`menu_background`, § 1.12) in `(x + 2, y + 2, right − 2, bottom)`, then a 1 px underline
  (white, or 0xFFA0A0A0 if inactive) of width `min(text width, w − 4)`, centered, at
  `y + h − 2`. Scrolling label in `[x + 1, x + w − 1]`, top = `y` (selected) or `y + 3`.
- No click sound of its own. Ctrl + digit 1–9/0: the matching tab; Ctrl + Tab (Shift:
  backward): next active tab.

### 1.11 Focus and keyboard navigation

- Tab: next focus in children order (sorted by "order group", stable); Shift+Tab:
  previous. At the end of the list, focus is removed and then restarts from the beginning. Inactive
  components are skipped; containers (lists, entries) are descended into.
- Arrows: spatial navigation. Candidates = components that overlap the current component
  on the perpendicular axis and whose entry edge is beyond its exit edge (or equal,
  with a farther exit edge); sorted by proximity of the entry edge then by position on the
  other axis (top/left first). Failing that, the closest component (squared distance
  between the midpoints of the facing edges) among those lying in the direction.
- Initial focus: if the last input is the keyboard, the first component (like a Tab); otherwise
  none (some screens impose their own, e.g. the language search field).
- A focused component is drawn as hovered ("highlighted" sprite).
- Esc: closes the screen (back to the previous screen for sub-screens, back to the game for the
  pause menu).

### 1.12 Screen background, header, footer, titles

- Order: new layer → background → new layer → components → deferred elements
  (tooltip, always last).
- **"In-game" screens** (inventories, chat, etc.): full-screen vertical gradient from
  **0xC0101010** (top) to **0xD0101010** (bottom). No blur.
- **Menu screens** (options, pause…):
  1. out of a world only: panorama (`title-screen-servers.md` § 1.1);
  2. **blur** if the "background blur" option ≥ 1: blur applied to everything drawn
     **before** this layer (the world and the in-game HUD); effect `post_effect/blur.json` = **three
     times** the pair (horizontal pass, vertical pass) of a box blur of radius r = option
     (0–10); each pass samples bilinearly at offsets `−r + 0.5, −r + 2.5, …`
     (step of 2 texels, ≤ r), adds the sample at `+r` weighted ½, divides by `r + 0.5`. Depth
     is cleared before the draws that come after the blur;
  3. menu background over the whole screen: `textures/gui/menu_background.png` (in a world:
     `textures/gui/inworld_menu_background.png`), 16 × 16 textures repeated every 32 units;
  4. deferred HUD subtitles (if active).
- **"Header / content / footer" layout**: header of height `hH` (33 by default) at the
  top, footer of height `hF` (33) at the bottom, elements centered horizontally and vertically in
  their band (x truncated, y rounded to nearest). The content is centered horizontally and
  placed at `y = min(hH + 30, H − hF − content_height)`.
  - Header title: white shadowed text, width = text width, height 9 → `y = 12`.
  - "Done" button (`gui.done`) 200 × 20 in the footer: `x = (W − 200)/2`, `y = H − 26`.
- The header and footer separators are drawn only by lists (§ 1.9) and the tab bar;
  a screen without a list (the main Options screen, the pause menu) has none.

---

## 2. Pause menu

### 2.1 Opening

- Esc with no screen open (and no input in progress) opens the pause menu. Also: window that
  lost focus for more than 500 ms if `pauseOnLostFocus` (true by default; F3+P toggles it).
- In multiplayer (our case), the full menu is **always** shown; F3+Esc (pause without a
  menu, title `menu.paused`) exists only in singleplayer not opened to LAN.
- The mouse is released as long as a screen is open. **No packet** is sent on
  opening or closing; the world keeps running (no pause in multiplayer).

### 2.2 Background

Menu background (§ 1.12): blur (if the pause menu is the foreground screen) then
`inworld_menu_background.png`, then subtitles. No panorama.

### 2.3 Layout

Title `menu.game` ("Game Menu"): white shadowed text at `(W/2 − width/2, 40)`.

Grid of 2 columns of **106** (button 98 + margins 4 / 4); cell margins: left 4,
top 4, right 4, bottom 0. Grid 212 wide, placed at
`gx = ⌊(W − 212) × 0.5⌋`, `gy = ⌊(H − gh) × 0.25⌋` (truncation toward zero), `gh` = height of
the grid (166 without a "custom" button, + 24 per added button).

| Row | relative y | Content (relative x) |
|---|---|---|
| 0 (top margin 50) | 50 | "Back to Game" `menu.returnToGame`, 204 × 20, x 4 |
| 1 | 74 | "Advancements" `gui.advancements` 98 × 20 (x 4) · "Statistics" `gui.stats` (x 110) |
| 2 | 98 | row of 4 icon buttons 20 × 20, spaced 4 apart, centered: x 60, 84, 108, 132 |
| (option) | +24 | custom dialog button (204 × 20), see § 2.5 |
| 3 | 122 | "Options..." `menu.options` (x 4) · "World Options..." `options.worldOptions.button` (x 110) |
| 4 | 146 | "Disconnect" `menu.disconnect` (singleplayer: `menu.returnToMenu`), 204 × 20, x 4 |

Icon buttons (15 × 15 sprites): `pause_menu/bug` (report a bug → external link, after
confirmation), `pause_menu/social_interactions` (feedback → external link), "friends" button (friends
list of the account), `pause_menu/player_reporting` (social interactions / reporting). The
two "Options" rows exist only if a world is loaded (always the case in game).

### 2.4 Actions

| Button | Effect |
|---|---|
| Back to Game | closes the screen, recaptures the mouse |
| Advancements | advancements screen (data from the advancement packets) — uncertain point for Rustcraft |
| Statistics | statistics screen (asks the server, packet `client_command` "request stats") — uncertain point |
| Options... | Options screen (§ 3) |
| World Options... | § 3.12 |
| Disconnect | the button becomes disabled, the connection is closed **on the client side** (no leave packet; local reason `multiplayer.status.quitting`), progress screen, then the multiplayer server list (title screen in singleplayer) |
| Icons | external links / account screens: out of scope for Rustcraft (to hide or disable, project decision) |

- If a player report is in draft, an icon `icon/draft_report` 15 × 15 is
  drawn at `(x_disc + 204 − 17, y_disc + 3)`.
- If the "music notification" option ≠ never and the final music volume > 0, the
  "now playing" toast is drawn on the pause menu (detail: uncertain point).

### 2.5 Custom server button

Dialog registry received during configuration: if the tag `#minecraft:pause_screen_additions`
contains 1 dialog → that dialog; several → the dialog `minecraft:custom_options`; none
but the server sent server links (`server_links`) → dialog `minecraft:server_links`.
The button (external title of the dialog, tooltip `menu.custom_options.tooltip`) opens that dialog.
Dialog rendering: out of scope for this document.

---

## 3. Options screen and sub-screens

### 3.1 Option labels and values

- Templates: `options.generic_value` "%s: %s", `options.percent_value` "%s: %s%"
  (percentage = ⌊value × 100⌋), `options.pixel_value` "%s: %spx", `options.on.composed` /
  `options.off.composed` "%s: ON / OFF", `options.multiplier` "%sx".
- Value types:
  - **unit**: real in [0, 1], continuous slider (`v` = value);
  - **bounded integer** `[min, max]`: position → `⌊lerp(v, min, max + 1)⌋` (v = 1 treated as
    0.99999); value → position: `min` ↦ 0, `max` ↦ 1, otherwise `(value + 0.5 − min) /
    (max + 1 − min)`;
  - **transformed integer**: bounded integer then a conversion (e.g. ×10, /4); "discrete" = arrows
    step by step;
  - **enumeration**: cycling button; **enumeration with slider** (graphics preset):
    position `i / (n − 1)`, reading `⌊v × n⌋` clamped.
- A value rejected by the bounds on assignment falls back to the **default** (with a logged
  error).
- Any change to a "quality" setting (marked Q in § 3.4) switches the graphics preset to
  **Custom**, except while a preset is being applied.

### 3.2 Options screen (`options.title`)

- Header 61, footer 33. Header: vertical block (spacing 8), 308 wide, centered,
  `y = 12`: centered title (y 12) then a row (y 29): **FOV** slider (150 × 20,
  x `hx`) and "Online..." button `options.online` (150 × 20, x `hx + 158`),
  `hx = ⌊(W − 308)/2⌋`.
- Content: grid of 2 columns of 158 (button 150, horizontal margins 4, bottom margin 4),
  5 rows of 24: `gx = ⌊(W − 316)/2⌋`, `gy = min(91, H − 153)`; button (r, c) at
  `(gx + 158c + 4, gy + 24r)`, 150 × 20.

| Row | Left | Right |
|---|---|---|
| 0 | Skin Customization `options.skinCustomisation` | Music & Sounds `options.sounds` |
| 1 | Video Settings `options.video` | Controls `options.controls` |
| 2 | Language `options.language` | Chat Settings `options.chat` |
| 3 | Resource Packs `options.resourcepack` | Accessibility Settings `options.accessibility` |
| 4 | Telemetry Data `options.telemetry` (inactive if not allowed) | Credits & Attribution `options.credits_and_attribution` |

- Footer: "Done". Closing: back to the previous screen and `options.txt` is written.

### 3.3 Generic options sub-screen

- Header 33 (title at y 12), footer 33 ("Done"), content = **option list** (§ 1.9) with
  row 310, entries **25** high.
- "Large" entry: one component 310 × 20 at `(W/2 − 155, y_entry + 2)`.
- "Small" entry: one or two components 150 × 20 at `x = W/2 − 155` and `W/2 + 5`.
  The options are arranged two by two in list order.
- "Section title" entry: underlined bold text (focusable, margin 4 → box of
  `width + 8` × 17, black background and white outline only when focused), centered, at
  `y_content + top_margin`; entry height = `top_margin + 18`, `top_margin` = 0 for the
  first entry of the list, 18 otherwise.
- Closing (Done or Esc): applies the pending deferred sliders, goes back to the previous
  screen, **writes `options.txt`** (and therefore sends the client information if changed).

### 3.4 Video (`options.videoTitle`)

Header: title, and if the chosen graphics API differs from the one at startup, a line
`options.restartRequired` in **0xFFDF5050** (vertical block spaced 2 apart, centered).

Content, in order (L = large entry, otherwise in pairs); "Key" column = `options.txt`
key; Q = quality setting (§ 3.1); D = deferred slider:

**Section `options.video.display.header`**

| Setting (translation key) | Key | Values | Default | Client-side effect |
|---|---|---|---|---|
| L Exclusive fullscreen mode `options.fullscreen.exclusive.mode` | `fullscreenResolution` (text, absent if "current") | −1 = current resolution, otherwise index of the monitor's video mode | current | applied when the screen is closed; label `options.fullscreen.entry` (w, h, refresh rate, color bits) |
| Max Framerate `options.framerateLimit` | `maxFps` | 10–260 in steps of 10; 260 = unlimited | 120 | frame rate cap (see the inactivity limit below) |
| VSync `options.vsync` | `enableVsync` | bool | true | reconfigures the surface |
| Inactivity limit `options.inactivityFpsLimit` | `inactivityFpsLimit` | `"minimized"`, `"afk"` | afk | window minimized (or exclusive fullscreen without focus): 10 fps; afk: 30 fps (at most) after 60 s without input, 10 fps after 10 min; out of a world with a screen: 60 fps |
| GUI Scale `options.guiScale` | `guiScale` | 0 = auto, 1..computed max | 0 | `chat.md` (scale); Ctrl + wheel on the Video screen changes the scale (0 treated as max + 1) |
| Fullscreen `options.fullscreen` | `fullscreen` | bool | false | toggles the window; also F11 |
| Exclusive fullscreen `options.exclusiveFullscreen` | `exclusiveFullscreen` | bool | false | — |
| Brightness `options.gamma` | `gamma` | unit; labels "min" (0), "default" (50), "max" (100), otherwise ⌊v × 100⌋ | 0.5 | `lighting.md` ("brightness" setting) |
| Graphics API `options.graphicsApi` | `preferredGraphicsBackend` | `"default"`, `"opengl"`, `"vulkan"` | default | restart required; Rustcraft: "default" and "vulkan" try Vulkan then DX12, "opengl" tries DX12 then Vulkan (`gpu_backend.rs`). Tooltip: `options.needsRestart` + blank line if the value differs from the one at startup, `options.graphicsApi.tooltip`, then blank line + `options.graphicsApi.tooltip.vulkan` for Vulkan |
| (macOS) fullscreen menu `options.macFullscreenMenuVisibility` | `macFullscreenMenuVisibility` | bool | false | — |

**Section `options.video.quality.header`**

| Setting | Key | Values | Raw default | Effect |
|---|---|---|---|---|
| L Preset `options.graphics.preset` | `graphicsPreset` | `"fast"`, `"fancy"`, `"fabulous"`, `"custom"` (slider with 4 notches) | fancy | applies the table of § 3.5 |
| Q Biome Blend `options.biomeBlendRadius` | `biomeBlendRadius` | 0–7 (label `options.biomeBlendRadius.<2r+1>`) | 2 | `fluids.md` (averaged tint), recompiles the sections; D |
| Q Render Distance `options.renderDistance` | `renderDistance` | 2–32 (2–16 if max memory < 1 GB) | 12 | `environment.md` (effective distance = min(option, server if > 0)); sent to the server; D |
| Q Chunk Updates Priority `options.prioritizeChunkUpdates` | `prioritizeChunkUpdates` | 0 none, 1 by the player, 2 nearby | 0 | section recompilation order (not specified elsewhere) |
| Q Simulation Distance `options.simulationDistance` | `simulationDistance` | 5–32 (5–16) | 12 | no effect in multiplayer (remote server); D |
| Q Ambient Occlusion `options.ao` | `ao` | bool | true | `lighting.md` |
| Q Clouds `options.renderClouds` | `renderClouds` | `"true"` fancy, `"fast"` fast, `"false"` off (bool accepted when reading) | true | `sky.md` |
| Q Particles `options.particles` | `particles` | 0 all, 1 decreased, 2 minimal | 0 | `particles.md`; sent to the server |
| Q Mipmap Levels `options.mipmapLevels` | `mipmapLevels` | 0 (OFF) – 4 | 4 | `models.md` (mipmaps); textures reloaded when the screen is closed if changed |
| Q Entity Shadows `options.entityShadows` | `entityShadows` | bool | true | `entities.md` (shadows) |
| Q Entity Distance `options.entityDistanceScaling` | `entityDistanceScaling` | 0.5–5.0 in steps of 0.25 (%) | 1.0 | entity view distance factor = `clamp(effective distance / 8, 1, 2.5) × setting` |
| Q Menu Background Blurriness `options.accessibility.menu_background_blurriness` | `menuBackgroundBlurriness` | 0 (OFF) – 10 | 5 | § 1.12 |
| Q Cloud Range `options.renderCloudsDistance` | `cloudRange` | 2–128 chunks | 128 | `sky.md` |
| Q Cutout Leaves `options.cutoutLeaves` | `cutoutLeaves` | bool | true | cutout leaves (otherwise opaque); recompiles |
| Q Improved Transparency `options.improvedTransparency` | `improvedTransparency` | bool | false | `entities.md`, `sky.md` (ordered-transparency pipeline); possible GPU warning |
| Q Texture Filtering `options.textureFiltering` | `textureFiltering` | 0 none, 1 RGSS, 2 anisotropic | 0 | block sampling (RGSS flagged to the global shader) |
| Q Max Anisotropy `options.maxAnisotropy` | `maxAnisotropyBit` | 1–3 → 2×, 4×, 8× (capped by the GPU) | 2 | active only if filtering = anisotropic |
| Q Weather Radius `options.weatherRadius` | `weatherRadius` | 3–10 blocks | 10 | `particles.md` / rain |

**Section `options.video.preferences.header`**

| Setting | Key | Values | Default | Effect |
|---|---|---|---|---|
| Autosave Indicator `options.autosaveIndicator` | `showAutosaveIndicator` | bool | true | singleplayer only |
| Vignette `options.vignette` | `vignette` | bool | true | `hud.md` (vignette) |
| Attack Indicator `options.attackIndicator` | `attackIndicator` | 0 off, 1 crosshair, 2 hotbar | 1 | `hud.md` |
| Chunk Fade `options.chunkFade` | `chunkSectionFadeInTime` | 0–2 s in steps of 0.05 (0 = none) | 0.75 | fade-in duration of sections as they appear |

### 3.5 Graphics presets

Applied on selection **and at startup** (right after `options.txt` is read): the
saved preset therefore overwrites the individual values, unless it is Custom.
Default = **Fancy** → the effective values of a first launch are those of the
Fancy column, not the "raw defaults".

| Setting | Fast | Fancy (default) | Fabulous |
|---|---|---|---|
| Biome Blend | 1 | 2 | 2 |
| Render Distance | 8 | 16 | 32 |
| Chunk Updates Priority | none | by the player | by the player |
| Simulation Distance | 6 | 12 | 12 |
| Ambient Occlusion | no | yes | yes |
| Clouds | fast | fancy | fancy |
| Particles | decreased | all | all |
| Mipmaps | 2 | 4 | 4 |
| Entity Shadows | no | yes | yes |
| Entity Distance | 0.75 | 1.0 | 1.25 |
| Background Blur | 2 | 5 | 5 |
| Cloud Range | 32 | 64 | 128 |
| Cutout Leaves | no | yes | yes |
| Improved Transparency | no | no | yes |
| Weather Radius | 5 | 10 | 10 |
| Anisotropy (bit) | 1 | 1 | 2 |
| Filtering | none | RGSS | anisotropic (RGSS if the driver has a known defect) |

An out-of-range value (distance 32 with the bound 16) falls back to the raw default (§ 3.1).

### 3.6 Music & Sounds (`options.sounds.title`)

- L Master volume, then in pairs the 10 other categories, then L audio device, then
  (subtitles, directional audio), then (music frequency, music notification).

| Setting | Key | Values | Default | Effect |
|---|---|---|---|---|
| Volume per category `soundCategory.<name>` | `soundCategory_<name>` for `master`, `music`, `record`, `weather`, `block`, `hostile`, `neutral`, `player`, `ambient`, `voice`, `ui` | unit (0 = OFF) | 1.0 | final volume = category × master (master alone for itself); `sounds.md`; out of a world, a preview sound of the category is played |
| L Device `options.audioDevice` | `soundDevice` | `""` = system default, otherwise device name (prefix "OpenAL Soft on" hidden in the display) | `""` | reloads the sound engine + click |
| Show Subtitles `options.showSubtitles` | `showSubtitles` | bool | false | subtitles overlay (duration × notification setting) |
| Directional Audio `options.directionalAudio` | `directionalAudio` | bool | false | HRTF (`sounds.md`); reloads the engine |
| Music Frequency `options.music_frequency` | `musicFrequency` | `"DEFAULT"` (20 min max), `"FREQUENT"` (10), `"CONSTANT"` (5 s) | DEFAULT | `sounds.md` |
| Music Notification `options.musicToast` | `musicToast` | `"never"`, `"pause"`, `"pause_and_toast"` | never | current track on the pause menu, and as a toast |

### 3.7 Chat (`options.chat.title`)

All in pairs, in this order (details of the effects: `chat.md` § 3.1 and § 1):

| Setting | Key | Values | Default |
|---|---|---|---|
| Visibility `options.chat.visibility` | `chatVisibility` | 0 all, 1 commands only, 2 hidden (sent to the server) | 0 |
| Colors `options.chat.color` | `chatColors` | bool (sent to the server) | true |
| Web Links `options.chat.links` | `chatLinks` | bool | true |
| Prompt on Links `options.chat.links.prompt` | `chatLinksPrompt` | bool | true |
| Text opacity `options.chat.opacity` | `chatOpacity` | unit, displayed `v × 0.9 + 0.1` | 1.0 |
| Background opacity `options.accessibility.text_background_opacity` | `textBackgroundOpacity` | unit | 0.5 |
| Scale `options.chat.scale` | `chatScale` | unit (0 = OFF) | 1.0 |
| Line spacing `options.chat.line_spacing` | `chatLineSpacing` | unit | 0.0 |
| Delay `options.chat.delay_instant` | `chatDelay` | 0–6 s in steps of 0.1 | 0.0 |
| Width `options.chat.width` | `chatWidth` | unit → `⌊v × 280 + 40⌋` px | 1.0 |
| Height when open `options.chat.height.focused` | `chatHeightFocused` | unit → `⌊v × 160 + 20⌋` px | 1.0 |
| Height when closed `options.chat.height.unfocused` | `chatHeightUnfocused` | same | 70/160 = 0.4375 |
| Narrator `options.narrator` | `narrator` | 0 off, 1 all, 2 chat, 3 system (inactive if there is no narrator) | 0 |
| Command Suggestions `options.autoSuggestCommands` | `autoSuggestions` | bool | true |
| Hide Matching Names `options.hideMatchedNames` | `hideMatchedNames` | bool | true |
| Reduced Debug Info `options.reducedDebugInfo` | `reducedDebugInfo` | bool | false |
| Only Show Secure Chat `options.onlyShowSecureChat` | `onlyShowSecureChat` | bool | false |
| Drafts `options.chat.drafts` | `saveChatDrafts` | bool | false |

Changing opacity, scale, width or heights recomputes the chat layout. Changing the
visibility updates the player's chat capabilities.

### 3.8 Accessibility (`options.accessibility.title`)

First entry: (Narrator, "Controls..." button leading to § 3.9). Then in pairs. Footer: button
"Accessibility Guide" (external link) and "Done" (150 each, spaced 8 apart).

| Setting | Key | Values | Default | Effect |
|---|---|---|---|---|
| Show Subtitles | `showSubtitles` | bool | false | § 3.6 |
| High Contrast `options.accessibility.high_contrast` | `highContrast` | bool | false | adds / removes the built-in pack `high_contrast` and reloads the resources; inactive if the pack is missing |
| Menu Background Blurriness | `menuBackgroundBlurriness` | 0–10 | 5 | § 1.12 |
| Text background opacity | `textBackgroundOpacity` | unit | 0.5 | `chat.md`, `hud.md` |
| Text Background `options.accessibility.text_background` | `backgroundForChatOnly` | true = "chat", false = "everywhere" | true | `hud.md`, `hud-advanced.md`, `scoreboard-teams.md` |
| Chat opacity, line spacing, delay | (§ 3.7) | | | |
| Notification Display Time `options.notifications.display_time` | `notificationDisplayTime` | 0.5–10 in steps of 0.1 ("×N") | 1.0 | durations of toasts, subtitles, held item name (`40 × setting` ticks) |
| View Bobbing `options.viewBobbing` | `bobView` | bool | true | camera and hand bobbing (camera rendering) |
| Distortion Effects `options.screenEffectScale` | `screenEffectScale` | unit (0 = OFF) | 1.0 | `hud-advanced.md` (nausea, portal) |
| FOV Effects `options.fovEffectScale` | `fovEffectScale` | slider `s`, value = `s²` | 1.0 | FOV modifier = `lerp(value, 1, modifier)`; underwater / in lava: `fov × lerp(value, 1, 6/7)` |
| Darkness Pulsing `options.darknessEffectScale` | `darknessEffectScale` | slider `s`, value = `s²` | 1.0 | `lighting.md` |
| Damage Tilt `options.damageTiltStrength` | `damageTiltStrength` | unit | 1.0 | tilt angle on injury `−14° × intensity × setting` |
| Glint Speed `options.glintSpeed` | `glintSpeed` | unit | 0.5 | `items.md` (glint) |
| Glint Strength `options.glintStrength` | `glintStrength` | unit | 0.75 | `items.md` |
| Hide Lightning Flashes `options.hideLightningFlashes` | `hideLightningFlashes` | bool | false | `environment.md`, `lighting.md` |
| Dark Mojang Studios Background `options.darkMojangStudiosBackgroundColor` | `darkMojangStudiosBackground` | bool | false | loading screen |
| Panorama Scroll Speed `options.accessibility.panorama_speed` | `panoramaScrollSpeed` | unit | 1.0 | `title-screen-servers.md` |
| Hide Splash Texts `options.hideSplashTexts` | `hideSplashTexts` | bool | false | title screen |
| Narrator Hotkey `options.accessibility.narrator_hotkey` | `narratorHotkey` | bool | true | Ctrl+B |
| Rotate with Minecart `options.rotateWithMinecart` | `rotateWithMinecart` | bool | false | active only if the experimental feature "minecart_improvements" is enabled |
| High Contrast Block Outline `options.accessibility.high_contrast_block_outline` | `highContrastBlockOutline` | bool | false | `targeting.md` |

### 3.9 Controls (`controls.title`) and mouse

- Entry 1: ("Mouse Settings..." `options.mouse_settings`, "Key Binds..."
  `controls.keybinds`). Then in pairs:

| Setting | Key | Values | Default | Effect |
|---|---|---|---|---|
| Sneak `key.sneak` | `toggleCrouch` | `options.key.hold` (false) / `options.key.toggle` (true) | hold | in toggle mode, each press flips the state |
| Sprint `key.sprint` | `toggleSprint` | same | hold | same |
| Attack `key.attack` | `toggleAttack` | same | hold | same |
| Use `key.use` | `toggleUse` | same | hold | same |
| Auto-Jump `options.autoJump` | `autoJump` | bool | false | `movement.md` |
| Sprint Window `options.sprintWindow` | `sprintWindow` | 0 (OFF) – 10 | 7 | `movement.md` (double press forward) |
| Quit Shortcuts `options.quitShortcuts` | `quitShortcuts` | bool | true | system close shortcuts |
| (macOS) Ctrl+click = right click | `ctrlClickEmulatesRightClick` | bool | false | — |
| Operator Items Tab `options.operatorItemsTab` | `operatorItemsTab` | bool | false | creative inventory |

- In toggle mode, a keyboard key force-released by a screen opening gets its state back on
  close (sneak, sprint, attack; not use).
- **Mouse** (`options.mouse_settings.title`), in pairs:

| Setting | Key | Values | Default | Effect |
|---|---|---|---|---|
| Sensitivity `options.sensitivity` | `mouseSensitivity` | unit; displayed `2v` in %, 0 "min", 1 "max" | 0.5 | `f = v × 0.6 + 0.2`, `k = f³ × 8`; rotation = accumulated movement × k (× f³ only when aiming through the spyglass in first person); the player turns 0.15° per unit |
| Wheel Sensitivity `options.mouseWheelSensitivity` | `mouseWheelSensitivity` | `10^(n/100)`, n ∈ [−200, 100] → 0.01–10 (not discrete), displayed "%.2f" | 1.0 | multiplies each notch |
| Discrete Scrolling `options.discrete_mouse_scroll` | `discrete_mouse_scroll` | bool | false | notch = sign only |
| Invert Mouse X `options.invertMouseX` | `invertXMouse` | bool | false | — |
| Invert Mouse Y `options.invertMouseY` | `invertYMouse` | bool | false | — |
| Allow Cursor Changes `options.allowCursorChanges` | `allowCursorChanges` | bool | true | § 1.1 |

### 3.10 Key binds (`controls.keybinds.title`)

- List with row 340, entries of 20. Footer: "Reset All" `controls.resetAll` (inactive
  if all keys are at their defaults) and "Done", 150 each, spaced 8 apart.
- Sort: by category in the order movement, miscellaneous, multiplayer, gameplay, inventory, creative,
  spectator, debug; within a category, by "order" (0 except debug) then by translated
  name. Category title `key.category.minecraft.<name>` (underlined bold, centered, at the bottom of
  its 20-high entry).
- Key entry: translated name in white at `(x_content, middle − 4)`; "Reset" button
  `controls.reset` 50 × 20 at `x = bar_x − 50 − 10` (bar_x = row right + 8),
  `y = y_content − 2`, inactive if the key is at its default; binding button
  75 × 20, 5 to the left.
- Binding button label: key name (§ 4.3). Conflict (same key as another
  binding, unless both are at their defaults): "`[ ` white name ` ]`" in yellow, yellow bar
  0xFFFFFF00 3 wide, 6 to the left of the button (from `y_content − 1` to the bottom of the content), tooltip
  `controls.keybinds.duplicateKeybinds` with the list of the others. Waiting for input:
  "`> ` underlined white name ` <`" in yellow.
- Reassignment: click on the button → waiting. Next key = new key (Esc =
  "none", `key.keyboard.unknown`); next mouse click = mouse button. Then the
  conflicts are recomputed.

Keys and defaults (binding name → default key):

| Category | Bindings |
|---|---|
| Movement | `key.forward` W · `key.left` A · `key.back` S · `key.right` D · `key.jump` Space · `key.sneak` Left Shift · `key.sprint` Left Ctrl |
| Gameplay | `key.attack` left mouse · `key.use` right mouse · `key.pickItem` middle mouse |
| Inventory | `key.inventory` E · `key.swapOffhand` F · `key.drop` Q · `key.hotbar.1`…`9` keys 1…9 |
| Multiplayer | `key.chat` T · `key.playerlist` Tab · `key.command` / · `key.friends` O · `key.socialInteractions` P |
| Miscellaneous | `key.screenshot` F2 · `key.togglePerspective` F5 · `key.smoothCamera` (none) · `key.fullscreen` F11 · `key.advancements` L · `key.quickActions` G · `key.toggleGui` F1 · `key.toggleSpectatorShaderEffects` F4 |
| Creative | `key.saveToolbarActivator` C · `key.loadToolbarActivator` X |
| Spectator | `key.spectatorOutlines` (none) · `key.spectatorHotbar` middle mouse |
| Debug | `key.debug.overlay` F3 (order −2) · `key.debug.modifier` F3 (−1) · `reloadChunk` A · `showHitboxes` B · `clearChat` D · `crash` C · `showChunkBorders` G · `showAdvancedTooltips` H · `copyRecreateCommand` I · `spectate` N · `switchGameMode` F4 · `debugOptions` F6 · `focusPause` P · `dumpDynamicTextures` S · `reloadResourcePacks` T · `profiling` L · `copyLocation` C · `dumpVersion` V · `profilingChart` 1 (order 1) · `fpsCharts` 2 (2) · `networkCharts` 3 (3) · `lightmapTexture` 4 (4) · `improvedTransparency` X |

(Full debug names: prefix `key.debug.`.) Keyboard keys are **physical positions**
(USB HID / SDL scancodes): W = 26 designates the "Z" key of an AZERTY keyboard.

### 3.11 Skin, language, font, online

- **Skin customization** (`options.skinCustomisation.title`): in pairs, an ON/OFF cycling
  button per part in the order cape, jacket, left sleeve, right sleeve, left pants leg,
  right pants leg, hat (`options.modelPart.<id>`), then Main Hand `options.mainHand`
  (`"left"` / `"right"`, default right). Keys: `modelPart_<id>` (ids `cape`, `jacket`,
  `left_sleeve`, `right_sleeve`, `left_pants_leg`, `right_pants_leg`, `hat`), default true,
  `mainHand`. Mask sent to the server: bit 0 cape … bit 6 hat (same order). Changing a
  part does not write immediately: written and sent when the screen is closed.
- **Language** (`options.language.title`): header 36 (title, search field 200 × 15,
  spacing 4), footer 53 (warning `options.languageAccuracyWarning` in 0xFFBABABA, then
  "Font..." and "Done"). Selectable list, row 270, entries 18, name "language
  (region)" centered in white, current selection centered on opening. Filter: case-insensitive
  substring of the name or the region. Double click, confirm or Done: if the language changes,
  `lang` = code, resources reloaded (and `language` sent to the server). Language
  list: pack metadata (uncertain point).
- **Font** (`options.font.title`): (Force Unicode Font `forceUnicodeFont`, default false;
  Japanese Glyph Variants `japaneseGlyphVariants`, default true if the system language is
  Japanese). Effect: `text.md`; recomputes the interface scale.
- **Online** (`options.online.title`): friends section (friends list, requests, in-game
  notifications `inGameNotification` default false, presence sharing `sharePresence` `"none"` /
  `"limited"` / `"all"` default all, Xbox settings link), servers section ("Allow Server
  Listings" `allowServerListing`, default true, sent to the server), Realms section
  (`realmsNotifications`, default true). Everything else depends on the Microsoft account: out of scope.
- **Field of view** (header of the Options screen): `options.fov`, integer 30–110, default 70;
  labels "Normal" (70) and "Quake Pro" (110). Stored as a real (§ 4.2).

### 3.12 World options (from the pause menu)

Header and footer 33, scrolling area (list background); centered content, column of 308 (2 × 150,
gap 8), section title `options.worldOptions.general.title` (underlined bold, 17 high) at
`y = 41`, rows of 20 spaced 4 apart.

- **Multiplayer** (no integrated server): general section only — [Difficulty 130 + lock
  20 (sprites `widget/locked_button` / `unlocked_button`, `_highlighted`, `_disabled`)]
  ["Edit Game Rules..." `editGamerule.inGame.button`] / ["Restrictions..."].
- **Singleplayer (host)**: in addition, at the top "Default Game Mode" and "Personal Game Mode"
  (308, values `selectWorld.gameMode.<mode>`), then "Commands" (150) before the difficulty;
  "Multiplayer" section (LAN, port, guests' access to commands, forced mode).
- Difficulty and lock are active with the "game master" permission (level ≥ 2) or as
  singleplayer host, and a world that is neither locked nor hardcore; inactivity tooltips
  `options.worldOptions.difficulty.disabled.{operator,locked,hardcore}.tooltip`. Game rules:
  active with the permission (in singleplayer: commands allowed), never in hardcore
  (`editGamerule.inGame.disabled[.hardcore].tooltip`).
- Footer: "Apply" (`menu.multiplayerOptions.applyChanges`, inactive without changes) and
  "Cancel". Apply: lock changed → popup `difficulty.lock.title` /
  `difficulty.lock.question` (Yes / No, buttons of 104, background `popup/background` of 250 around
  the content, margin 18, the screen underneath stays visible under a dimming overlay); then
  `change_difficulty` (difficulty, varint 0–3) if changed and `lock_difficulty` (true) if
  locked. Back to the pause menu.

**Game rules** (`editGamerule.title`): header 36 (title, search 200 × 15,
`gui.game_rule.search`), footer 33 ("Done" / "Cancel"). On opening: `client_command`
action 2; text `editGamerule.inGame.downloadingGamerules` and animated dots until
`game_rule_values` (map id → text value; only the first response counts). List
of 220, rows of 24: by category (`gamerule.category.minecraft.<id>`, yellow bold underlined, centered;
categories sorted by id), rules sorted by id; label
`gamerule.minecraft.<id>` wrapped to 170 (1 line at y + 5, 2 lines at y and y + 10); on the right
(x = right − 45) an ON/OFF button 44 wide or a 44 × 20 field (red if invalid: integer out of
bounds; "Done" is then inactive). Tooltip: id in yellow, description wrapped to
150, `editGamerule.default` in gray. Filter: case-insensitive substring of the id, name,
category or description. Done: `set_game_rule` (list of (full id,
text value)) for the changed rules only. Cancel / Esc with changes: confirmation
`editGamerule.inGame.discardChanges.*`. Only the rules known to the client are listed (the game
ignores the others).

**Restrictions** (`restrictions_screen.title`): depending on `chatVisibility` — hidden:
`chat_restriction.chat_and_commands_disabled_by_options`, system: `…chat_disabled_by_options`
(bold red, 250 wide, then a 200 button leading to the Chat screen); then the four permissions
`restrictions_screen.permission.<id>.allowed|denied` (green / red): sending messages,
sending commands, receiving system messages, receiving player messages.

### 3.13 Resource packs (screen only)

- Header 49: title, `pack.dropInfo` in gray, search field 200 × 15 (spacing 4).
  Two lists 200 wide, entries of 36: available at `x = W/2 − 215`, selected at
  `x = W/2 + 15`; first entry of each list = title (`pack.available.title` /
  `pack.selected.title`, underlined bold, height 13). Footer: "Open Pack Folder" and "Done".
- Entry: 32 × 32 icon (`textures/misc/unknown_pack.png` by default), name at `(x + 34, y + 1)`,
  description (gray 0xFF808080, max 157 wide) at `(x + 34, y + 12)`. Incompatible pack:
  background 0xFF770000. When hovering the icon: overlay 0xA0909090 then sprite
  `transferable_list/select` (available), or `unselect` (left half) / `move_up` (upper-right
  quarter) / `move_down` (lower-right quarter) for a selected pack (`_highlighted` variants under
  the mouse).
- Done: list validated; `resourcePacks` / `incompatibleResourcePacks` rewritten (non-pinned packs,
  in order) then resources are reloaded if the list changed.

### 3.14 Credits and attribution, telemetry

- Options screen: "Telemetry Data..." **inactive** with the tooltip
  `options.telemetry.disabled` (the game does the same when telemetry is not allowed;
  Rustcraft sends none).
- "Credits & Attribution" (also via the copyright notice of the title screen): three
  buttons of 210 spaced 8 apart (Credits, Attribution, Licenses), content at
  `y = min(63, H − 33 − 76)`, "Done" 200. Attribution and licenses: link confirmation
  (`chat.link.confirmTrusted`, address; "Open in Browser", "Copy", "Cancel",
  buttons of 100 spaced 4 apart).
- Credits: scrolling of `texts/credits.json` (without the poem), 0.75 unit per tick, logo at
  `H + 50`, text starting at logo + 100, 12 per line; sections framed by
  `============`, names indented by 11 spaces; last line stops in the middle of the screen; end
  after `lines × 12 + 2H + 24`; menu background shifted by `scroll / 2`, vignette.

### 3.15 Social interactions

Title `gui.socialInteractions.title`; window `social_interactions/background` (236 wide,
x = (W − 238)/2 + 3, y = 64, height max(52, H − 144) + 16); tabs All / Hidden / Blocked
(width 220/3, y 45, current tab underlined); search 200 × 15 at (margin + 28, 74), icon
`icon/search`; server and player count at (margin + 8, 35); list of 220, rows of 36
(background 0xFF4A4A4A, 0xFF303030 offline or hidden; name in white; status in italics). Buttons
20 × 20: hide / show (`social_interactions/mute_button` / `unmute_button`) at right − 48,
report (inactive offline, `…tooltip.report.disabled`) at right − 24. Hide: the chat messages
of that player are no longer displayed for the rest of this launch; message
`gui.socialInteractions.hidden_in_chat` / `shown_in_chat`. Opened by the icon
`pause_menu/player_reporting` of the pause menu.

### 3.16 Out-of-world warnings

- **Online play** (first time, `skipMultiplayerWarning` false): on clicking Multiplayer,
  centered column (spacing 8): `multiplayerWarning.header` bold, message wrapped to W − 100
  (margins 12), checkbox `multiplayerWarning.check`, "Proceed" / "Back" (150, gap 8).
  Proceed with the checkbox ticked: `skipMultiplayerWarning` true, written.
- **Code of conduct** (configuration, `code_of_conduct`): same layout,
  `multiplayer.codeOfConduct.title`, text from the server, checkbox `multiplayer.codeOfConduct.check`,
  "I Understand" / "Disconnect"; Esc has no effect. Already accepted if the entry of
  `servers.dat` has `acceptedCodeOfConduct` = Java fingerprint of the text: immediate acceptance
  (`accept_code_of_conduct`) without a screen. Accepted with the checkbox: fingerprint saved; otherwise
  erased. Refusal: disconnect `multiplayer.disconnect.code_of_conduct`.
- **Accessibility onboarding** (`onboardAccessibility` true, at the first title screen): logo, text
  `accessibility.onboarding.screen.title` (374 wide), 2 × 2 grid (Narrator, Sounds,
  Accessibility, Language), "Continue"; fade-in (2 s), fade-out (1 s) then
  `onboardAccessibility` false, written.
- **Reconfiguration** (`start_configuration` in game): `connect.reconfiguring` and
  "Disconnect" centered (margins 10), button active after 600 ticks; Esc has no effect.


---

## 4. The `options.txt` file

### 4.1 Location and format

- File `options.txt` in the game folder, in UTF-8, one option per line: `key:value`
  (split at the **first** colon; a line without a colon is ignored with a
  warning).
- First line written: `version:<data version>` (**5023** for 26.3). When reading,
  a missing version counts as 0 and the options data fixes (renames, old key
  codes, etc.) are applied up to the current version (uncertain point: content of the
  fixes not re-read).
- Values of typed options (all those in the tables of § 3): **JSON** — boolean `true` /
  `false`, number (`12`, `0.5`, `1.0`), quoted string (`"fancy"`, `""`, `"right"`).
  Tolerant reading (lenient JSON; an empty value is read as `""`); an invalid value is
  ignored (error logged, default kept). Unit reals also accept `true` (1)
  and `false` (0).
- Raw fields (without quotes): `lang`, `lastServer`, `key_*`, `tutorialStep`, `fullscreenResolution`
  (text); `hideServerAddress`, `advancedItemTooltips`, `pauseOnLostFocus`, `useNativeTransport`,
  `skipMultiplayerWarning`, `joinedFirstServer`, `syncChunkWrites`, `onboardAccessibility`,
  `startedCleanly`, `modelPart_*` (booleans: only the exact text `true` is true);
  `overrideWidth`, `overrideHeight`, `glDebugVerbosity` (integers). `resourcePacks` and
  `incompatibleResourcePacks`: JSON list of strings.

### 4.2 Encoding pitfalls

- `fov` is stored as a real `(fov − 70) / 40` (70 → `0.0`, 110 → `1.0`, 30 → `-1.0`).
- `fovEffectScale` stores the **value** (square of the slider position); `darknessEffectScale`
  stores the **slider position** (square root of the value, squared again when read).
- `entityDistanceScaling`, `chunkSectionFadeInTime`, `chatDelay`, `notificationDisplayTime`,
  `mouseWheelSensitivity`, `maxFps` store the real value (not the slider index).
- Read bounds of the integers: `[min, max + 1]` accepted by the decoder, then `max + 1` is
  rejected on assignment → default. `guiScale` accepts 0 to 2,147,483,647 and is then clamped to the
  computed maximum.
- `renderClouds`: `"true"` = fancy, `"false"` = off (strings), JSON booleans
  accepted when reading.

### 4.3 Keys and write order

Exact write order (after `version`):

1. `ao`, `biomeBlendRadius`, `chunkSectionFadeInTime`, `cutoutLeaves`, `enableVsync`,
   `entityDistanceScaling`, `entityShadows`, `forceUnicodeFont`, `japaneseGlyphVariants`, `fov`,
   `fovEffectScale`, `darknessEffectScale`, `glintSpeed`, `glintStrength`,
   `preferredGraphicsBackend`, `graphicsPreset`, `prioritizeChunkUpdates`, `fullscreen`,
   `exclusiveFullscreen`, `macFullscreenMenuVisibility`, `gamma`, `guiScale`, `debugGuiScale`
   (−1 "unchanged", 0 auto, default 0; debug options screen), `maxAnisotropyBit`,
   `textureFiltering`, `maxFps`, `improvedTransparency`, `inactivityFpsLimit`, `mipmapLevels`,
   `narrator`, `particles`, `reducedDebugInfo`, `renderClouds`, `cloudRange`, `renderDistance`,
   `simulationDistance`, `screenEffectScale`, `soundDevice`, `vignette`, `weatherRadius`.
2. `autoJump`, `rotateWithMinecart`, `operatorItemsTab`, `autoSuggestions`, `chatColors`,
   `chatLinks`, `chatLinksPrompt`, `discrete_mouse_scroll`, `invertXMouse`, `invertYMouse`,
   `realmsNotifications`, `showSubtitles`, `directionalAudio`, `bobView`, `toggleCrouch`,
   `toggleSprint`, `toggleAttack`, `toggleUse`, `sprintWindow`, `darkMojangStudiosBackground`,
   `hideLightningFlashes`, `hideSplashTexts`, `mouseSensitivity`, `damageTiltStrength`,
   `highContrast`, `highContrastBlockOutline`, `narratorHotkey`, `resourcePacks` (default `[]`),
   `incompatibleResourcePacks` (`[]`), `lastServer` (empty), `lang` (`en_us`), `chatVisibility`,
   `chatOpacity`, `chatLineSpacing`, `textBackgroundOpacity`, `backgroundForChatOnly`,
   `hideServerAddress` (false), `advancedItemTooltips` (false; F3+H), `pauseOnLostFocus` (true),
   `overrideWidth` (0), `overrideHeight` (0), `chatHeightFocused`, `chatDelay`,
   `chatHeightUnfocused`, `chatScale`, `chatWidth`, `notificationDisplayTime`,
   `useNativeTransport` (true), `mainHand`, `attackIndicator`, `tutorialStep` (`movement`,
   `find_tree`, `punch_tree`, `open_inventory`, `craft_planks`, `none`; default `movement`),
   `mouseWheelSensitivity`, `allowCursorChanges`, `quitShortcuts`, `ctrlClickEmulatesRightClick`,
   `glDebugVerbosity` (1), `skipMultiplayerWarning` (false), `hideMatchedNames`,
   `joinedFirstServer` (false), `syncChunkWrites` (true on Windows), `showAutosaveIndicator`,
   `allowServerListing`, `inGameNotification`, `sharePresence`, `onlyShowSecureChat`,
   `saveChatDrafts`, `panoramaScrollSpeed`, `telemetryOptInExtra` (false),
   `onboardAccessibility` (true), `menuBackgroundBlurriness`, `startedCleanly` (true),
   `musicToast`, `musicFrequency`.
3. One line per binding, in declaration order (attack, use, forward, left, back, right,
   jump, sneak, sprint, drop, inventory, chat, player list, pick block, command, friends,
   social interactions, hide the interface, spectator effects, screenshot, perspective, smooth camera,
   fullscreen, spectator outlines, spectator hotbar, offhand, save / load toolbar, advancements,
   quick actions, F3, modifier, hotbar 1–9, debug keys): `key_<binding name>:<key name>`, e.g.
   `key_key.forward:key.keyboard.w`, `key_key.attack:key.mouse.left`.
4. `soundCategory_<name>` for the 11 categories (order: master, music, record, weather, block,
   hostile, neutral, player, ambient, voice, ui).
5. `modelPart_<id>` for the 7 parts.
6. `fullscreenResolution:<mode>` only if an exclusive mode is chosen.

**Key names**: `key.keyboard.<name>` (fixed table: letters `a`…`z` = scancodes 4…29,
digits `1`…`9`, `0` = 30…39, `enter` 40, `escape` 41, `backspace` 42, `tab` 43, `space` 44,
`minus`, `equal`, `left.bracket`, `right.bracket`, `backslash`, `world.2`, `semicolon`,
`apostrophe`, `grave.accent`, `comma`, `period`, `slash` 45…56, `caps.lock` 57, `f1`…`f12`
58…69, `print.screen`, `scroll.lock`, `pause`, `insert`, `home`, `page.up`, `delete`, `end`,
`page.down` 70…78, `right`, `left`, `down`, `up` 79…82, `num.lock` 83, keypad `keypad.divide`,
`multiply`, `subtract`, `add`, `enter` 84…88, `keypad.1`…`9`, `keypad.0` 89…98,
`keypad.period` 99, `world.1` 100, `menu`/`application` 101, … , `left.control` 224,
`left.shift` 225, `left.alt` 226, `left.win` 227, `right.control` 228, `right.shift` 229,
`right.alt` 230, `right.win` 231, plus the media keys 257–290; full table = USB HID
usage codes), `key.keyboard.unknown` (0) for "none", `key.mouse.left` (1),
`key.mouse.middle` (2), `key.mouse.right` (3), `key.mouse.4`…`key.mouse.8`. An unknown name of
the form `key.keyboard.<n>` or `key.mouse.<n>` designates code `n`; any other invalid name →
unassigned key. Display of a keyboard key: character produced by the active layout
(uppercase) if it is a single character, otherwise the translation of its name; mouse:
translation of the name, otherwise the `key.mouse` fallback "Button %s".

### 4.4 Reading, writing, sending to the server

- Reading: at startup, before the window is created; file missing → defaults. Then the
  graphics preset is applied (§ 3.5) and the selected packs are validated (a pack
  that is missing or has become incompatible is removed from the list, `file/` is tried as a prefix).
- Full write (file rewritten): on every click on an option cycling button, when
  any options screen is closed, when packs change, at the end of the accessibility
  onboarding.
- **After every write**, if a player exists, the client builds its information (language,
  render distance of the option — not the effective distance —, chat visibility, colors,
  skin parts mask, main hand, text filtering of the account, `allowServerListing`,
  particles) and sends the `client_information` packet **only if it differs from the last
  one sent** (format: `connection.md`).

---

## Uncertain points

1. Advancements and Statistics screens opened from the pause menu: not specified here (advancement
   packets, statistics request).
2. "Game rules" and "Restrictions" screens of the world options: not read in detail
   (possible packets for game rules in multiplayer).
3. Rendering of server dialogs (custom button of the pause menu): out of scope.
4. "Now playing" toast of the pause menu (position, note scrolling): not recorded.
5. Data fixes of `options.txt` (old formats): list of fixes not re-read.
6. Language list: built from the packs' metadata (the jar contains only
   `en_us.json`, the others come from the asset index) — to be confirmed at implementation.
7. "Friends" button and screens tied to the Microsoft account (friends, presence, Xbox, Realms, telemetry):
   exact behavior not followed; to be hidden on the Rustcraft side.
8. Repetition of the background textures (`menu_background`, `menu_list_background`, separators)
   beyond their size: assumed to use the sampler's "repeat" mode (UV > 1).
9. `title-screen-servers.md` § 1.1 describes **two** blur passes; `post_effect/blur.json`
   contains six (three horizontal + vertical pairs), the latter is retained here.
10. "Order" value of the categories and sorting by **translated** name: the order of the key list
    depends on the language.
11. Exact size and position of the icon of an icon button (centering assumed) and sprites of the
    "friends" button: not recorded.
