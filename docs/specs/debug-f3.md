# Debug screen (F3), debug options, F3 + … keys (26.3)

Clean-room spec, written from a reading of the decompiled 26.3 client (kept outside the repository). In our
own words; only data identifiers (entries, option keys, translation keys, displayed strings) are taken as is,
because they are visible to the player or persisted.

Cross-references:
- `hud.md`: GUI frame, layer order (the debug screen comes after the open screen,
  § 1 point 6), 2D crosshair that the 3D crosshair replaces.
- `option-menus.md`: `key.debug.*` assignments (§ 3.10, keys = physical positions),
  `reducedDebugInfo`, `debugGuiScale`, `pauseOnLostFocus`, `advancedItemTooltips`,
  `improvedTransparency`, F3+Escape pause (§ 2.1), frame limit and inactivity.
- `text.md`: font, glyph width, `§` formatting codes (underline, colours).
- `chat.md`: client system message (the F3 + … feedback arrives there and is read by the narrator).
- `sounds.md`: channels (§ 3.5), mood.
- `targeting.md`: targeted block and entity.

Conventions: colours `0xAARRGGBB`; "GUI" = virtual interface pixels; integer divisions
truncated; numbers formatted in the "root" locale style (decimal point, no thousands separator).

---

## 1. Model: entries, statuses, profiles

### 1.1 Entries

The debug screen is no longer a fixed block: it is a **list of identified entries**
(`minecraft:<name>`), each producing zero or more lines, or driving a renderer.
Two categories (for the options screen):
- **text** (`debug.options.category.text`, "Debug Screen Text", sort key 1);
- **renderers** (`debug.options.category.renderer`, "Debug Renderers", sort key 2): entries with no
  text that enable a 3D rendering (§ 4).

### 1.2 Status of an entry

Three statuses (serialised name):
- `alwaysOn`: always displayed, even with F3 closed;
- `inOverlay`: displayed only when the F3 screen is open;
- `never`: never. **Default status of any entry absent from the table.**

"F3 screen open" (called *overlay visible* here) is a runtime boolean, **not persisted**,
false at startup.

### 1.3 Active list

Recomputed on every change of status, overlay visibility, profile, and when the
"reduced info" state changes (option `reducedDebugInfo`, or the "reduced debug info" flag
received from the server in the world join packet / entity event, see `option-menus.md`).
Reduced info = server flag **or** local option.

An entry is active if its status is `alwaysOn`, or `inOverlay` with the overlay visible, **and**
if it is allowed in the current reduced-info state (§ 3, "reduced" column).
The active list is **sorted by identifier path** (lexicographic order of the string after
`minecraft:`, then namespace): this order fixes the order in which entries are called, hence
the order of the lines (§ 2). A version counter is incremented on every rebuild (the 3D
renderers resynchronise on it).

### 1.4 Profiles

Two predefined profiles (status of the entries; everything else `never`):

| Entry | `default` | `performance` |
|---|---|---|
| `3d_crosshair` | inOverlay | — |
| `game_version` | inOverlay | — |
| `fps` | inOverlay | **alwaysOn** |
| `tps` | inOverlay | inOverlay |
| `memory` | inOverlay | inOverlay |
| `system_specs` | inOverlay | — |
| `player_position` | inOverlay | — |
| `player_section_position` | inOverlay | — |
| `simple_performance_impactors` | inOverlay | inOverlay |
| `gpu_utilization` | — | inOverlay |

Labels: `debug.options.profile.default` "Default profile", `debug.options.profile.performance`
"Performance profile".

Loading a profile replaces the whole status table. Changing a status by hand switches to
"custom" (no current profile any more).

### 1.5 Quick toggle of an entry (used by F3+B, F3+G…)

| Current status | Overlay visible | New status | "Shown" result |
|---|---|---|---|
| alwaysOn | either | never | no |
| inOverlay | yes | never | no |
| inOverlay | no | alwaysOn | yes |
| never | yes | inOverlay | yes |
| never | no | alwaysOn | yes |
| absent | either | alwaysOn | yes |

Each toggle switches to custom and **saves** the file (§ 5.2).

---

## 2. Text layout

### 2.1 Display conditions

Nothing is drawn until the game's initial loading is finished, nor if the HUD is hidden
(F1) **unless** a screen is open. Nothing either if the active list is empty (the footer
§ 2.3 is therefore only displayed if there is at least one active entry). The F3 screen is drawn in
its own GUI layer, above the HUD and the open screen (`hud.md`). The debug options screen
redraws it under its background blur.

### 2.2 Collection

Each active entry, in the order of § 1.3, may emit:
- **priority line**: added on the left if the left column has **at most** as many lines as the
  right one, otherwise on the right (alternating left, right, left…);
- **normal line**: common queue;
- **group lines**: added to a named group; groups keep the order of their **first**
  appearance, and several entries may fill the same group (in their call order). A group may be
  created empty (target entry without a target): it still counts in the split below, but emits
  nothing.

The "server" values only exist in singleplayer (integrated server); for our client, the world
consulted is always the client world and the "server chunk" is always absent.

### 2.3 Column assembly

In order:
1. If the left column (priority lines) is non-empty, an empty line is added to it; likewise on the
   right.
2. Normal lines (n): `m = (n + 1) / 2`; the first `m` on the left then an empty line; the
   following ones on the right then an empty line only if there was at least one.
3. Groups (g, empty ones included): `m = (g + 1) / 2`; the group of index `i < m` goes on the left,
   otherwise on the right; each non-empty group is followed by an empty line.
4. If the overlay is visible, footer on the left: an empty line, then
   - `Debug charts: [M+1] Profiler <state>; [M+2] <fps> <state>;`
   - `[M+3] <network> <state>; [M+4] Lightmap <state>`
   - `To edit: press [M+F6]`

   where `[M+X]` = `[` + translated name of the modifier key + `+` (omitted if it is not
   assigned) + translated name of the action key + `]` (current keys, e.g. `[F3+2]`);
   `<state>` = `visible` / `hidden`; `<fps>` = `fps + tps` in singleplayer, `fps` otherwise; `<network>` =
   `Ping` in singleplayer, `Bandwidth + Ping` otherwise.

Empty lines occupy a row but are not drawn.

### 2.4 Scale

`debugGuiScale` (−1 "Unchanged", 0 "Auto" **default**, otherwise integer):
- −1: current GUI scale;
- 0: `max / 2`, where `max` is the window's maximum automatic scale (the largest `s` such that
  width/s ≥ 320 and height/s ≥ 240, bounded by the forced Unicode setting);
- n > 0: n bounded by that same maximum scale.

If the resulting scale `e` satisfies `0 < e < GUI scale`, a factor `e / GUI scale` is applied
and the virtual surface becomes `pixel_width / e` × `pixel_height / e`
(integer divisions). Otherwise we stay at the GUI scale (never enlarged beyond it). This factor
also applies to the charts, the lightmap and the pie chart.

### 2.5 Drawing

- Line height 9, margin 2 at the top, left and right.
- Line `i`: `y = 2 + 9·i`, `x = 2` on the left, `x = L − 2 − width` on the right (L = virtual
  width of § 2.4).
- Background per non-empty line: rectangle `[x − 1, y − 1] → [x + width + 1, y + 8]`, colour
  `0x90505050`. All the backgrounds of a column are drawn before all its texts.
- Text `0xFFE0E0E0`, **without shadow**. The `§` codes present in the strings (underline,
  green/red of properties, red of the GPU) apply normally.

### 2.6 Example (multiplayer, `default` profile, F3 open, nothing targeted)

Call order: `3d_crosshair` (nothing), `fps` (priority → left), `game_version`
(priority → right), `memory` (group "memory"), `player_position` (group "position"),
`player_section_position` (group "position"), `simple_performance_impactors` (3 normal),
`system_specs` (group "system"), `tps` (normal).

Left: fps; empty; `B: 2`; `Filtering: None`; empty; 3 memory lines; empty; 6 position lines
(XYZ, Block, Chunk, Facing, dimension, Section-relative); empty; empty; footer.
Right: version; empty; `Terrain Rendering: …`; server line; empty; 6 system lines; empty.

---

## 3. Text entries

"Type" column: P = priority, N = normal, G(x) = group x. "Reduced" column: yes = stays
displayed with reduced info. "Singleplayer only" = produces nothing for us.

| Identifier | Type | Reduced | Condition |
|---|---|---|---|
| `game_version` | P | yes | always |
| `fps` | P | yes | always |
| `tps` | N | yes | connection and world present |
| `memory` | G(memory) | yes | always |
| `detailed_memory` | G(memory) | yes | always |
| `system_specs` | G(system) | yes | always |
| `gpu_utilization` | N | yes | always |
| `simple_performance_impactors` | N ×2 or 3 | yes | always |
| `chunk_render_stats` | N | yes | view area exists |
| `entity_render_stats` | N | yes | world present |
| `chunk_source_stats` | N | yes | world present |
| `sound_cache` | N | yes | always |
| `player_section_position` | G(position) | yes | camera entity present |
| `player_position` | G(position) | no | camera entity present |
| `player_speed` | G(position) | no | camera entity present |
| `looking_at_block_state` / `_tags` | G(looking_at_block) | no | block targeted |
| `looking_at_fluid_state` / `_tags` | G(looking_at_fluid) | no | fluid targeted |
| `looking_at_entity` / `_tags` | G(looking_at_entity) | no | entity targeted |
| `light_levels` | G(light) | no | camera and world |
| `heightmap` | G(heightmaps) | no | camera, world and client chunk |
| `biome` | N | no | feet within the buildable height |
| `local_difficulty` | N | no | **singleplayer only** |
| `day_count` | N | no | world present |
| `entity_spawn_counts` | N | no | **singleplayer only** |
| `chunk_generation_stats` | G(chunk_generation) | no | **singleplayer only** |
| `sound_mood` | N | no | player present |
| `post_effects` | N | no | at least one active effect |
| `particle_render_stats` | N | no | always |

The renderer entries (§ 4) are all "no" (hidden with reduced info).

### 3.1 Version — `game_version`

`Minecraft <version name> (<launched version>/<client brand>)`, e.g.
`Minecraft 26.3 (26.3/vanilla)`. Version name = `26.3`; launched version = value passed by the
launcher (for us: `26.3`); client brand = `vanilla` (implementation choice: keep
`vanilla`, which is also the brand announced to the server, or announce `rustcraft` everywhere
consistently).

### 3.2 Frames — `fps`

`<fps> fps T: <limit><mode> @<Hz>Hz`, e.g. `144 fps T: inf (fifo) @144Hz`.
- `<fps>`: number of frames rendered during the last elapsed second (counter reset
  every 1,000 ms).
- `<limit>`: **effective** frame limit (option, or 60 out of world in a menu, 10 minimised window
  or long inactivity, min(option, 30) short inactivity, see `option-menus.md`);
  `inf` if it equals 260 (= "unlimited").
- `<mode>`: surface presentation mode: ` (immediate)`, ` (mailbox)`, ` (fifo)`,
  ` (fifo relaxed)`, empty if unknown. Direct correspondence with the wgpu modes.
- `<Hz>`: refresh rate of the screen's active video mode, an integer if round otherwise `%.2f`; `0`
  if unknown.

### 3.3 Server — `tps`

Multiplayer: `"<server brand>" server<state>, <tx> tx, <rx> rx`, e.g.
`"Paper" server, 12 tx, 85 rx`.
- Brand: last value received on the `minecraft:brand` channel (`null` if never received).
- `<state>`: ` (frozen - stepping)` if the world advances step by step, ` (frozen)` if it is frozen,
  otherwise empty (tick-rate state received from the server).
- `<tx>`/`<rx>`: smoothed averages of the **packets** sent/received per second, `%.0f`. Every
  20 iterations of the network loop (1 s): `avg = 0.25·count + 0.75·avg`, then count reset to 0.

(Singleplayer: `Integrated server @ <ms>/<target> ms<state>, …` — not concerned.)

### 3.4 Memory — `memory`, `detailed_memory` (group "memory")

JVM values. `memory` emits 3 lines:
- `Mem: <p>% <u>/<m>MiB` with `%2d%%` and `%03d` (e.g. `Mem: 23% 941/4096MiB`); u = used,
  m = maximum allowed, p = u·100/m as an integer;
- `Allocation rate: <r>MiB/s` (`%03d`): recomputed at most every 500 ms; if no garbage
  collector has run since the previous measurement, `r = (u − u_prev) / Δt` rounded, otherwise the
  previous value is kept;
- `Allocated: <p>% <a>MiB`: a = memory reserved by the JVM, p = a·100/m.

`detailed_memory` emits 2 lines, **before** those of `memory` (alphabetical order):
`Memory (heap): i=%03dMiB u=%03dMiB c=%03dMiB m=%03dMiB` then the same with `non-heap`
(initial, used, committed, maximum). MiB = bytes / 1024 / 1024 truncated.

**Implementation choice (native client)** — keep the formats, change the source:
- `Mem`: u = live bytes counted by our global allocator (atomic counter), m = total physical
  memory of the machine;
- `Allocation rate`: bytes allocated (cumulative, not net) per second, 500 ms window;
- `Allocated`: committed memory of the process ("commit" / private working set);
- `detailed_memory`: `heap` = Rust allocator, `non-heap` = GPU memory allocated by the rendering
  engine (wgpu allocation report); fields without an equivalent set to 0.

### 3.5 System — `system_specs` (group "system")

Six lines:
1. `Java: <Java version>` — **choice**: `Rust: <compiler version>` (e.g. `Rust: 1.99.0`),
   same place.
2. `CPU: <n>x <name>`: number of logical processors and commercial name, multiple spaces reduced
   to one; `<unknown>` if unreadable. Computed only once.
3. `Display: <W>x<H> (<GPU vendor>)`: size of the framebuffer in pixels.
4. `Window: <w>x<h> (<d>x pixel density)`: size of the window in screen coordinates, density
   `%.2f` (buffer pixels / screen coordinates).
5. `<GPU name><type>`: type ` (iGPU)` integrated, ` (dGPU)` discrete, ` (vGPU)` virtual,
   ` (software)` processor, empty otherwise (direct correspondence with the wgpu adapter type).
6. `<backend> <driver>`: name of the API (`Vulkan`, `DX12`…) then the **first line** of the
   driver description (for us: name + driver info of the adapter).

### 3.6 GPU — `gpu_utilization`

`GPU: <n>%` with n = GPU time of the last measured frame / CPU duration of that frame × 100,
rounded. Above 100: `GPU: ` then `§c100%` (red). The measurement (GPU timing query)
is only made if the entry is active; otherwise the value is reset to 0. Native equivalent:
wgpu timestamp queries around the frame (implementation choice; if not supported,
display `GPU: 0%`).

### 3.7 Settings that weigh — `simple_performance_impactors`

1. `<transp><clouds>B: <radius>`: `improved-transparency ` if the option is active; clouds
   `fast-clouds ` / `fancy-clouds ` / nothing if disabled; biome blend radius (0–7).
   E.g. `fancy-clouds B: 2`.
2. `Filtering: <name>` with the translated label (`None`, `RGSS`, `Anisotropic`); if anisotropic:
   `Filtering: Anisotropic <k>x` (k = effective level).
3. `Terrain Rendering: multidrawindirect` or `naive` depending on the section drawing path.
   **Choice**: `multidrawindirect` if our terrain rendering uses multi-draw indirect,
   otherwise `naive`.

### 3.8 Rendering statistics

- `chunk_render_stats`: `C: <rendered>/<total> <s>D: <dist>, pC: <queue>, aB: <free>`
  (`pC` as `%03d`, `aB` as `%02d`). rendered = sections visible this frame having at least one
  non-empty layer; total = number of sections of the view area; `<s>` = `(s) ` if the smart
  section sorting is active (true by default); dist = effective render distance;
  queue = sections waiting to be compiled; free = compile buffers available.
  E.g. `C: 412/9248 (s) D: 12, pC: 000, aB: 07`.
- `entity_render_stats`: `E: <rendered>/<total>, SD: <simulation distance>` (entities whose
  render state was extracted this frame / entities of the client world; SD received from the server).
- `chunk_source_stats`: `Chunks[C] W: <capacity>, <loaded> E: <entities>,<sections>,<chunks>`:
  capacity = number of cells of the client chunk array (square grid of the view distance),
  loaded = chunks present; then tracked entities, entity sections, chunks where entities
  are active (separated by commas **without spaces**).
- `particle_render_stats`: `P: <number of live particles>`.
- `post_effects`: `Post: <id1>, <id2>` (post-processing effects applied, e.g.
  `minecraft:spider` when spectating a spider); nothing if none.

### 3.9 Position — "position" group

`player_position` (**camera** entity, the one we are watching as a spectator):
1. `XYZ: %.3f / %.5f / %.3f`.
2. `Block: x y z` (block of the feet, integer coordinates by floor).
3. `Chunk: cx sy cz [rx rz in r.RX.RZ.mca]`: chunk, vertical section (`y >> 4`), position
   within the region (`c & 31`), region (`c >> 5`).
4. `Facing: <dir> (<text>) (%.1f / %.1f)`: horizontal direction in lowercase (`north`,
   `south`, `west`, `east`, from the yaw: `floor(yaw/90 + 0.5) & 3` → south, west, north,
   east), text `Towards negative Z` / `Towards positive Z` / `Towards negative X` /
   `Towards positive X`, then yaw and pitch brought into [−180, 180).
5. `<dimension> FC: <n>`: dimension identifier (e.g. `minecraft:overworld`), n = forced
   chunks, **always 0** in multiplayer.

`player_section_position`: `Section-relative: %02d %02d %02d` (x & 15, y & 15, z & 15 of the feet
block).

`player_speed`: `Speed: %.3f blocks/tick` = 3D norm of the camera entity's displacement between
its last two ticks (if it has a controlling player passenger: that player's speed).

Order within the group (alphabetical by entry): lines of `player_position`, then
`player_section_position`, then `player_speed`.

### 3.10 Targeted target

Ray cast from the camera entity, range **20 blocks**, with frame interpolation = 0
(end-of-tick position), from the eyes, block outline shapes, independent of the interaction
range; two casts: without fluids (block) and stopped by any fluid, source or not
(fluid). Nothing if the ray does not hit a block.

- `looking_at_block_state` (group "looking_at_block"):
  `§n` + `Targeted Block: x, y, z` (underlined), then the block identifier
  (`minecraft:oak_stairs`), then one line per property **sorted by name**:
  `<name>: <value>`; true boolean value preceded by `§a` (green), false by `§c` (red).
- `looking_at_block_tags`: one line `#<tag>` per block tag
  (e.g. `#minecraft:mineable/axe`), from the tags received from the server.
- `looking_at_fluid_state` / `_tags` (group "looking_at_fluid"): same with
  `Targeted Fluid: x, y, z`, the fluid identifier (`minecraft:empty` if the hit block has no
  fluid), its properties (`falling`, `level`), its tags.
- `looking_at_entity` (group "looking_at_entity"): if an entity is under the crosshair (same
  entity as in `targeting.md`, interaction range): `§nTargeted Entity`, then the type
  identifier (`minecraft:pig`). `looking_at_entity_tags`: `#<tag>` per tag of the type.

The "server" states (development flag) are not concerned.

### 3.11 Environment

- `light_levels` (group "light"): `Client Light: <raw> (<sky> sky, <block> block)` at the feet
  block; raw = max(sky, block) (sky darkening ignored); values from the client light engine
  (`lighting.md`).
- `heightmap` (group "heightmaps"): two lines at the feet's (x, z):
  `CH S: <a> M: <b> ML: <c>` (maps sent to the client: world surface, motion blocking,
  blocking without leaves; value = y of the first free block above) and
  `SH S: ?? O: ?? M: ?? ML: ??` (server maps, **always `??`** for us).
- `biome`: `Biome: <identifier>` (e.g. `minecraft:plains`) at the feet block, only if its y
  is within the buildable height; `[unregistered …]` for a biome without a key.
- `day_count`: `Day #<n>` = total ticks of the Overworld day clock / period (24,000),
  truncated; the line only exists if the `overworld_day` timeline exists in the received
  registries.
- `sound_mood`: `Sounds: <a>/<A> + <b>/<B> (Mood <m>%)`: point sources used/max,
  streaming sources used/max (`sounds.md` § 3.5, e.g. 247 and 8), ambient mood × 100 rounded
  (0 if the dimension has no biome ambience handler).
- `sound_cache`: `Sound cache: <n> buffers, <t> MiB` (decoded buffers in cache, size rounded
  **up** to the MiB).
- `local_difficulty` (`Local Difficulty: %.2f // %.2f`), `entity_spawn_counts` (`SC: …`),
  `chunk_generation_stats`: singleplayer only, not to be implemented (they produce nothing in
  multiplayer).

---

## 4. Renderer entries (no text)

| Identifier | Effect | Access |
|---|---|---|
| `entity_hitboxes` | entity boxes (§ 8.1) | F3+B |
| `chunk_borders` | chunk borders (§ 8.2) | F3+G |
| `3d_crosshair` | 3D crosshair instead of the 2D one (§ 8.3) | default profile |
| `chunk_section_paths`, `chunk_section_octree`, `chunk_section_visibility` | section sorting diagnostics | options screen |
| `visualize_water_levels`, `visualize_heightmap`, `visualize_collision_boxes`, `visualize_entity_supporting_blocks`, `visualize_block_light_levels`, `visualize_sky_light_levels`, `visualize_solid_faces`, `visualize_sky_light_sections` | 3D development visualisations | options screen |
| `visualize_chunks_on_server` | map of the integrated server's chunks | singleplayer only |

All those of the options screen are reachable by the player, hence to be reproduced. Done: the
first three, the eight `visualize_*` (§ 8.4), `chunk_section_paths` and
`chunk_section_visibility` (nodes of the occlusion walk, § 8.5). Remaining without effect:
`chunk_section_octree` (our renderer has no section octree) and `visualize_chunks_on_server`
(it shows the ticket levels and generation statuses of the integrated server's chunks,
which `rc-sim` does not model: chunks loaded full or absent).

### 8.5 Section paths and visibility

For each section reached by the occlusion walk and within the view:

- paths: segment from the centre of the section to the centre of each neighbour from which the
  walk arrived there (all arrivals, not just the first), width 3, opaque, HSV hue
  (step / 50, 0.9, 0.9); black for the starting nodes (step 0: camera section, or
  edge layer when the camera is outside the world);
- visibility, only if the section has geometry: for each ordered pair of faces
  (a, b) that do not communicate, opaque red segment from the middle of a to the middle of b; if there
  is at least one, solid box of the section shrunk by 0.5 per side, colour
  (alpha 0.2; 0.9; 0.9; 0).

---

## 5. Debug options screen (F3+F6)

### 5.1 Content

- Title `debug.options.title` "Debug Options", preceded by a space of 1/3 of the list width
  and followed by a search field (width 1/3, height 20, hint `debug.options.search`
  "Search..." in the search hint style); the field has focus on opening.
- Centred subtitle `debug.options.warning`, colour `0xFFDF5050`, wrapped to the list width.
- Header 61, footer 33; list of rows of 20, row width 350.
- List: all entries sorted by category (text then renderers) then by identifier; a
  category row (bold underlined label, centred) before each category change. The
  filter keeps the entries whose **path contains** the typed text (case-sensitive).
- Entry row: the path (`fps`, not `minecraft:fps`) on the left in white; in grey
  `0xFF808080` and italic if the entry is not allowed under reduced info while that is
  active (tooltip `debug.options.notAllowed.tooltip` when hovering the name). On the right, three
  buttons of 60×16 joined, from left to right: `options.off` "OFF", `debug.entry.overlay`
  "In Overlay", `debug.entry.always` "Always". The button of the current status is **inactive** and
  coloured (OFF white, In Overlay `0xFFFFFF55`, Always `0xFFDF5050`); the other two active, text
  `0xFFBABABA`. Clicking sets the status, saves, and re-enables the profile buttons.
- Footer: button "Default profile" (120), "Performance profile" (120), option button
  `debugGuiScale` (110; GUI scale label, values `Unchanged` / `Auto` / integer,
  tooltip `options.debugGuiScale.tooltip`), "Done" (60); spacing 4. The button of the current
  profile is inactive; clicking a profile loads it, saves and refreshes the rows.
- Narration of a button: `debug.entry.currently.<status>` ("<name>: Currently only in debug
  overlay", etc.) + value.
- The F3 screen stays drawn behind the blur. An F3+… action made in this screen refreshes the
  rows.

### 5.2 File `debug-profile.json`

In the game folder. Compact JSON, two exclusive forms:
- `{"profile":"default"}` or `{"profile":"performance"}`;
- `{"custom":{"minecraft:fps":"alwaysOn","minecraft:entity_hitboxes":"inOverlay",…}}`.

The format is versioned like the other data (data version field added on
writing; absent = old version). Read at startup: file absent → `default` profile
(without writing); unreadable → `default` profile **and** the file is rewritten. Written on
every status change or profile choice. Unknown identifiers kept but ignored.

---

## 6. F3 + … keys

### 6.1 F3 alone

`key.debug.overlay` and `key.debug.modifier` are both on F3 by default. When they are on the
same key, the overlay toggles on the **release** of F3, unless an F3+… action was
triggered during the press (F3 served as a modifier). If they differ, the overlay toggles when
its own key is pressed. This also works with a screen open (unless the screen consumes the
key, e.g. text field).

While the modifier is held, the global keys (fullscreen, screenshot…) are ignored,
and a key that triggered an F3+… action is **not** passed to the game (no opening of the
inventory on F3+E, etc.). F3+Escape: pause without menu (`option-menus.md` § 2.1).

### 6.2 Feedback in the chat

Each feedback is a client system message: `[Debug]:` (`debug.prefix`, **bold**, yellow `§e`,
red `§c` for warnings) + space + text (white). See `chat.md`.

### 6.3 Actions (default keys; each action checks its own assignment)

| Key | Key id | Effect | Message |
|---|---|---|---|
| A | `reloadChunk` | recompiles all terrain sections | `debug.reload_chunks.message` "Reloading all chunks" |
| B | `showHitboxes` | toggles `entity_hitboxes` (§ 1.5); ignored without a player or with reduced info | `debug.show_hitboxes.on/off` "Hitboxes: shown/hidden" |
| D | `clearChat` | clears the **displayed** chat history (not the history of sent messages) | none |
| G | `showChunkBorders` | toggles `chunk_borders`; same restrictions as B | `debug.chunk_boundaries.on/off` "Chunk borders: shown/hidden" |
| H | `showAdvancedTooltips` | toggles `advancedItemTooltips`, saves options.txt | `debug.advanced_tooltips.on/off` "Advanced tooltips: shown/hidden" |
| I | `copyRecreateCommand` | copies the recreate command of the target (§ 6.4); nothing with reduced info | `debug.inspect.*` |
| N | `spectate` | "game master" permission required (level 2): if not a spectator → requests spectator mode, otherwise → previous mode (creative if unknown), through the game mode change packet | without permission: `debug.creative_spectator.error` "Unable to switch game mode; no permission" |
| F4 | `switchGameMode` | if world present and **no screen**: opens the mode switcher (§ 6.5) if permitted, otherwise message | `debug.gamemodes.error` "Unable to open game mode switcher; no permission" |
| F6 | `debugOptions` | closes the options screen if open; otherwise, if the current screen can be interrupted, closes it and opens the options screen | none |
| P | `focusPause` | toggles `pauseOnLostFocus`, saves | `debug.pause_focus.on/off` "Pause on lost focus: enabled/disabled" |
| S | `dumpDynamicTextures` | writes the dynamic atlases as PNG in `screenshots/debug/` | `debug.dump_dynamic_textures` "Saved dynamic textures to %s" (underlined relative path, clickable → opens the file) |
| T | `reloadResourcePacks` | message **then** reload of resources | `debug.reload_resourcepacks.message` "Reloaded resource packs" |
| L | `profiling` | starts a 10 s profiling, or stops it if running | `debug.profiling.start` "Profiling started for 10 seconds. Use F3 + L to stop early"; at the end `commands.debug.stopped` (duration `%.2f` s, ticks, ticks/s `%.2f`) and `debug.profiling.stop` "Profiling ended. Saved results to %s" (underlined clickable path) |
| C | `copyLocation` | copies `/execute in <dimension> run tp @s %.2f %.2f %.2f %.2f %.2f` (x, y, z, yaw, pitch **not wrapped**); ignored without a player or with reduced info | `debug.copy_location.message` "Copied location to clipboard" |
| C (held) | `crash` | see § 6.6 | |
| V | `dumpVersion` | version info in the chat | `debug.version.header` "Client version info:" then the `commands.version.*` lines (id, name, data, series, protocol `777 (0x309)`, build_time, pack.resource, pack.data, stable/unstable) |
| 1 | `profilingChart` | toggles the profiler pie chart | none |
| 2 | `fpsCharts` | toggles the frame charts (+ tick) | none |
| 3 | `networkCharts` | toggles the network charts | none |
| 4 | `lightmapTexture` | toggles the lightmap preview | none |
| X | `improvedTransparency` | toggles `improvedTransparency`, saves | `debug.improvedTransparency.on/off` "Improved transparency: enabled/disabled" |

**F3+Q no longer exists** in 26.3: the `debug.*.help` and `debug.help.*` strings are present
in the language but no key displays them. Do not implement it.

Chart toggles (1–4): if the overlay is hidden, the toggle **forces** the chart to be displayed
(and opens the overlay); if the overlay is visible, it inverts the state. The frame charts (2),
network charts (3) and lightmap (4) are **mutually exclusive** (enabling one switches off the
other two); the pie chart (1) is independent. A chart is only drawn if the overlay is
visible; its state is remembered during the session. While the pie chart is displayed, the
digits 0–9 **without** a modifier navigate the profiler tree (0 = parent).

Profiling, the pie chart and the texture dump depend on an internal profiler:
**implementation choice** — hook them to our own measurement zones, or only display the
messages for the time being.

### 6.4 F3+I: recreate command

From the current crosshair target (`targeting.md`); nothing if nothing is targeted (no message).
- With the "game master" permission and **without Shift**: request to the server for the NBT data
  (block/entity tag query packets with a transaction number); on the answer,
  copy and message `debug.inspect.server.block` / `.entity` ("Copied server-side … data to
  clipboard").
- With the permission and Shift: NBT data known to the client (local block entity / locally
  saved entity), message `debug.inspect.client.*`.
- Without permission: without NBT, message `debug.inspect.client.*`.
- Block: `/setblock x y z <state>` + NBT appended directly, state in the format
  `minecraft:oak_stairs[facing=east,half=bottom,shape=straight,waterlogged=false]` (properties
  sorted by name).
- Entity: `/summon <type> %.2f %.2f %.2f` + ` <SNBT>` if NBT; the keys `UUID` and `Pos` are
  removed.
- The clipboard is not modified if the string is empty.

### 6.5 Game mode switcher (F3+F4)

- Background: texture `textures/gui/container/gamemode_switcher.png`, area 125×75 from (0, 0) of
  a 128×128 sheet, placed at `(L/2 − 62, H/2 − 58)`.
- Four slots 26×26 (sprite `gamemode_switcher/slot`, selection `gamemode_switcher/selection`),
  step 31, together `4·31 − 5 = 119` wide, at `x = L/2 − 59 + 31·i`, `y = H/2 − 31`; item
  icon at (+5, +5): creative = grass block, survival = iron sword, adventure = buried treasure
  map, spectator = Eye of Ender. Order and cycle: creative → survival → adventure →
  spectator → creative.
- Name of the hovered mode centred at `y = H/2 − 51`, white; at `y = H/2 + 5`:
  `debug.gamemodes.select_next` "%s Next" with the name of the F4 key in aqua.
- Initial selection: previous mode; failing that survival if we are in creative, otherwise creative.
- F4 (while F3 is held) advances the selection; the mouse changes it as soon as it has moved
  since opening (or since the last F4).
- **Releasing F3** (or the mouse button assigned to the modifier) applies: if the chosen mode
  differs from the current mode and the permission is there, sends the game mode change packet;
  then closes. The screen does not pause the game.

### 6.6 F3+C held: deliberate crash

With `crash` and the modifier pressed together, a timer starts; releasing either
cancels it. Beyond 100 ms, any other F3+… action is blocked. Every second: first
`debug.crash.message` ("F3 + C is held down. This will crash the game unless released.",
yellow), then `debug.crash.warning` "Crashing in %s..." (red, remaining seconds rounded
up). At 10 s: crash with report "Manually triggered debug crash".
**Implementation choice**: write a crash report (log) and exit cleanly with
an error code; no real crash. Since C is also used by `copyLocation`, the initial press
copies the position.

---

## 7. Charts

### 7.1 Common geometry

- 240 samples at most (ring). Width = `min(240 + 2, L / 2)`; height of the area 60,
  placed on the **bottom** of the (virtual) screen. A row of 9 above for the labels
  (total height 69).
- Left chart at `x = 0`, right chart stuck to the right edge (`x = L − width`).
- Background `0x90505050` over `[x, bottom − 60] → [x + width, bottom]`.
- Bars: one pixel wide per sample, from `x + 1` to the right, from the oldest to the most
  recent; only the last `width − 2` (and at most 240) are shown. Bar from the bottom upwards,
  height according to the chart's own scale (no clamping: may exceed the top).
- White 1 px frame: horizontal lines at `bottom − 60` and `bottom − 1`, verticals at both edges.
- Above (y = `bottom − 69`), if there is at least one sample: `<min> min` on the left (x + 2),
  `<avg> avg` centred, `<max> max` on the right (− 2), colour `0xFFE0E0E0` with shadow.
- Inner labels: background `0x90505050` of the text width + 1 and height 9, text
  `0xFFE0E0E0` without shadow offset by (1, 1).
- Colour of a bar: three-point gradient (min → middle → max, value clamped), **linear
  per-channel interpolation in sRGB space**, each channel `c0 + floor(t·(c1 − c0))`.

### 7.2 Frames (F3+2, left)

- Sample = real duration of each frame (ns, from the start of the frame to the next, limiter
  included).
- Height = `round(ms × 60 / 33.333…)` (top = 30 fps). Text value `%d ms` rounded.
- Colours: 0 ms green `0xFF00FF00`, 28 ms yellow `0xFFFFFF00`, 56 ms red `0xFFFF0000`.
- Marks: label `30 fps` at the top, `60 fps` at mid-height with a white line at `bottom − 30`;
  if the option's frame limit is between 1 and 250, cyan line `0xFF00FFFF` at the height of
  `1/limit` (minus 1).

### 7.3 Server tick (F3+2, right)

Displayed only if there are samples. In multiplayer they come from the debug sample packet
(type "tick time"), to which the client **subscribes** (debug subscription packet) as long as the
frame charts are displayed; the server only answers players having the required permission
(operators). Each sample has 4 dimensions: full tick, tick method, scheduled tasks, wait (ns).
- Scale: `ms × 60 / mspt` with mspt = milliseconds per tick of the current tick rate (50).
- Main bar = full tick, colour green (≤ mspt) → yellow (1.125·mspt) → red
  (1.25·mspt); then on top, stacked from the bottom: tick method `0xFF991111`, tasks
  `0xFFBA995F`, other (= full − wait − method − tasks) `0xFF5F0E8C`.
- min/avg/max computed on full − wait. Label `%.1f tps` (= 1000 / mspt) at the top.
- On disconnection, the tick, ping and bandwidth logs are cleared (not the frames one).
- The set of subscriptions sent restarts empty on each entry into the game (`login`, including
  after a reconfiguration): chart open, the subscription is sent again to the new server.
- Subscription and ping requests go out from the play phase on (no waiting for the position).
- Singleplayer: the integrated server ALWAYS logs its ticks directly into the client's log
  (full = time since the previous reading, method = world tick, tasks = message processing,
  wait = sleep); chart visible without subscription. F3 server line:
  `Integrated server @ <smoothed>/<mspt> ms<state>, <tx> tx, <rx> rx`, smoothed = 0.8 × old +
  0.2 × method duration, on every server tick.
- Negative height ("other" part < 0): rectangle with its corners put back in order, hence drawn
  downwards, over the method and tasks parts.

### 7.4 Ping (F3+3, right)

- As long as the network charts are displayed, the client sends **on every tick** a ping
  request packet carrying the local time (ms, monotonic clock); sample = now −
  returned time, computed when the main thread processes the answer (start of the next frame),
  hence frame wait included.
- Singleplayer: the integrated server answers on its I/O thread, without waiting for its tick
  (≈ 0 ms); only the ping is shown (no bandwidth), footer `Ping`, and
  `fps + tps` for F3+2.
- Scale: top = 500 ms (`round(ms × 60 / 500)`). Colours: 0 green, 250 yellow, 500 red.
  Label `500 ms`. Text `%d ms`.

### 7.5 Bandwidth (F3+3, left, multiplayer only)

- Sample per tick = bytes **received** during the tick (all packets, measured at raw
  reception), logged continuously since the connection.
- Value in bytes/s = bytes/tick × 20. Logarithmic height:
  `round(ln(v + 1) × 60 / ln(1,048,576))` (top = 1 MiB/s).
- Colours: 0 cyan `0xFF00FFFF`, 8,192 lavender `0xFFA0A0FF`, 10,485,760 red.
- White labelled lines at 64 B/s, 1.0 KiB/s, 16.0 KiB/s; label `1.0 MiB/s` at the top.
- Text: `≥ 1 MiB/s` → `%.1f MiB/s`, `≥ 1 KiB/s` → `%.1f KiB/s`, otherwise `%d B/s` (floor).

### 7.6 Lightmap (F3+4)

Lightmap texture (16×16) displayed at 64×64, nearest filtering, at the bottom
right: `x = L − 66`, `y = H − 66`, opaque black frame of 1 around it; image flipped
vertically (v from 1 to 0).

### 7.7 Profiler pie chart (F3+1)

Block 260 wide at the bottom right (centre x = `L − 140`), above the displayed charts
(bottom offset = 69 if a chart is shown, otherwise 10, plus 5 margin). Background `0x90505050`.
Disc of radius 105, thickness 10; title = current node (`[0] ` if it is not the root),
global percentage on the right; below, one line per child `[n] name`, local and
global percentage `##0.00%`, in the child's colour. Content depending on the profiler: to be adapted (§ 6.3).

Details:
- Profiler (on as long as F3+1 is shown): nested named zones, paths separated by
  0x1E, cumulative (duration, passes) since switch-on, never reset per frame; live
  reading during the frame. Nothing is shown on the first frame (results set at the end of
  the frame). Shares of a node: direct children, total = max(sum of children, node duration),
  global = max(root, total); the remainder is "unspecified" (single-precision float comparison); sorted
  by decreasing share then decreasing name; at the top the node itself (100%).
- Colour of a node: `(Java hash of the name & 0xAAAAAA) + 0xFF444444`.
- Layout: `cx = L − 140`, bottom = `H − offset − 5`, child lines above the bottom
  (9 px each), disc centre 67 px above the lines; title split on the dots
  (first line ≤ width − global % − 5 − "[0]", following ones 10 less, offset by
  10 + "[0]"); text with shadow, title white; "unspecified" child noted `[?]` (the
  numbers follow the index).
- Disc: per share, a fan of `floor(% / 4) + 1` segments, angle = cumulative × 2π / 100,
  x = sin × 105, y = cos × 52.5 (y downwards: the first share starts from the bottom); then a slice of
  10 px (colour × 128 / 255) under the segments whose middle has y ≥ 0.
- Digits (row or pad, F3 not held, pie chart shown and results set): 0 goes up to the
  parent (nothing at the root), n goes down into the n-th child except "unspecified"; the key
  keeps its ordinary effect.
- Our zones (game names): `root` → `scheduledPacketProcessing`, `scheduledExecutables`,
  `mouse`, `tick` (→ `Keybindings`, `gui`, `entities`, `blockEntities`, `animateTick`, `level`,
  `particles`, `playerSync`), `sound`, `frame` (→ `update`, `extract`, `render`, `fpsUpdate`).

### 7.8 F3+L recording

- Start: recording profiler switched on (it counts frames as "ticks"), start message; F3+L during
  recording requests the stop, effective at the end of the frame; otherwise end at the first
  end of frame after 10 s.
- End: `commands.debug.stopped` (seconds `%.2f`, frames, frames/s `%.2f`), then archive
  `debug/profiling/<local date>-<world or server entry name>-<version>.zip`, name
  passed through the game's free-name search (dots and forbidden characters → `_`); message
  `debug.profiling.stop` with the relative path (system separator), underlined, click = opens
  the folder.
- Content: `system.txt` (`key: value`), `client/options.txt` (options sorted `name: value`),
  `client/metrics/<category>.csv` (`@tick` then columns, CRLF, numbers in Java's `Double`
  format, first partial frame not recorded), `client/profiling.txt` (profiling report
  header, durations, tree `[nn] |   name(passes/per tick) - %local/%global`).

---

## 8. Associated 3D rendering

World primitives: lines (thickness in screen pixels, world fog), points (square of
N pixels), solid faces (translucent, no fog, sorted from farthest to
nearest) and floating text facing the camera (interface glyphs, `scale / 16` block per pixel,
0.32 by default, centred, descending from its point, full light, no background). Depth-tested
unless stated otherwise, drawn with the block outlines, **before** the translucent terrain
(water in front tints them). Ordering by colour opacity: opaque strokes (alpha 255)
write depth, then translucent ones pass without writing it (≤ test) — this is what makes the
edges of the current chunk purplish where 50 % red overlays blue. Order of the kinds:
each at its first appearance in "opaque faces, opaque lines, opaque texts, faces,
translucent lines, texts". Lines shrunk towards the camera by (1 − 1/256)(1 − 1/4096), faces
by (1 − 1/4096).

Arrow: segment plus four strokes from the tip towards points moved back by
`l = clamp(0.1 × length, 0.1, 1)` and spread by `l`, frame rotated from +X towards the direction.

### 8.1 Entity boxes (`entity_hitboxes`, F3+B)

For each entity rendered this frame (same culling as rendering), **not invisible**, and except
the camera entity in first person. Interpolated position of the frame; the boxes are
offset by (interpolated position − end-of-tick position).
- Collision box: **white** outline `0xFFFFFFFF`, thickness 2.5; white point of size 2 at
  the position (feet).
- If the entity is mounted: **yellow** plate `0xFFFFFF00` at the seat point provided by the
  vehicle, half-width = min(vehicle width, entity width)/2, height 1/16.
- Living entities: **red** plate `0xFFFF0000` at eye height, full width of the
  box, from `eyes − 0.01` to `eyes + 0.01`.
- Ender Dragon: outline of each part `0xFF3FFF00` (head 1×1, neck 3×3, body 5×3, three
  tails 2×2, two wings 4×2), placed as the client places them on each tick from the
  flight history, interpolated between two ticks; never placed (no AI since spawning):
  at the world origin.
- Gaze: **blue** arrow `0xFF0000FF` from the eye (position + eye height) over 2 blocks in
  the interpolated gaze direction (head for living entities, entity rotation otherwise), for
  **all** entities, living or not.
- Box = current box of the entity (`Entity::bounding_box`): dimensions according to pose, babies
  with their own size per type, the size of slimes/magma cubes and phantoms, the pufferfish's
  inflation, the salmon's size, the scale attribute (clamped for happy ghast, shulker,
  dragon), interaction (data 8 and 9), the effect cloud (radius × 2 over 0.5), the small
  or marker armour stand; own boxes of paintings, item frames (map: whole block) and leash
  knots (recomputed position), of the shulker (opening).
- Spectator following an entity: borders around the followed entity, its box omitted in
  first person, the local player's drawn.

### 8.2 Chunk borders (`chunk_borders`, F3+G)

Around the section that contains the camera entity's block; `x0`, `z0` = corner of the chunk;
verticals from the world's `y_min` to `y_max + 1`.
- Corners of the 8 neighbouring chunks (x, z ∈ {−16, 0, 16, 32} relative): **semi-transparent
  red** verticals `0x7FFF0000`, thickness 4.
- Faces of the current chunk: verticals every 2 blocks (2…14) on the 4 faces; colour
  `0xFF009B9B` (dark cyan) if the offset is a multiple of 4, otherwise **yellow** `0xFFFFFF00`;
  thickness 1.
- Horizontals: square outline of the chunk every 2 blocks in y (from `y_min` to `y_max + 1`),
  dark cyan if y is a multiple of 8, otherwise yellow; thickness 1.
- **Blue** major lines `0xFF3F3FFF`, thickness 4: the 4 vertical edges of the chunk, and
  the square outline at each y multiple of 16 starting from `y_min`.
- Cube of the current **section** (16×16×16): blue outline `0xFF3F3FFF` thickness 1, **always
  in the foreground** (no depth test).

### 8.3 3D crosshair (`3d_crosshair`)

Replaces the 2D crosshair (the 2D one and the attack indicator are not drawn), only in
first person and with the HUD not hidden. Three axes of unit length, placed 1 in front of the camera,
oriented along the **world** axes (rotated by the camera's pitch then yaw),
scale `0.01 × GUI scale`, X and Z inverted. First the three axes in black thickness 4
(outline), then on top with a slight depth bias: X red `0xFFFF0000`, Y green
`0xFF00FF00`, Z light blue `0xFF7F7FFF`, thickness 2.

### 8.4 Visualisations of the F3+F6 screen

- `visualize_collision_boxes`: collision shapes that touch the camera entity's box
  inflated by 6 (whole shape of a block as soon as one of its boxes touches; those of the local player
  depending on its state: scaffolding, powder snow), then boxes of solid entities; white outline
  2.5; list rebuilt every 100 ms.
- `visualize_solid_faces`: blocks (except plain air) within ±6 of the camera's block; for each
  outline box inflated by 0.002, faces of the sturdy sides, solid `0x80FF0000` (order west,
  south, east, north, down, up).
- `visualize_water_levels`: water within ±10 of the player's block; solid shape `0x2600FF00` from
  (x + 0.01, y + 0.01, z + 0.01) to (x + 0.99, y + height, z + 0.99), then the amount as black
  text at the centre of the top (height 1 under water, otherwise amount / 9).
- `visualize_heightmap`: 5 × 5 chunks around the camera, each known heightmap of the
  column (type order): solid plate from x + 0.25 to x + 0.75, height 3/32, placed at the
  first free block + type rank × 3/32; colours per type (generation surface yellow,
  generation ocean floor magenta, surface green 0.7, ocean floor blue 0.5, motion blocking cyan 0.3,
  motion blocking without leaves cyan 0.5).
- `visualize_block_light_levels`, `visualize_sky_light_levels`: within ±10 of the camera's block; at
  each touched section, its sky light section type in red, scale 4.8, at the centre
  ("0" the section has blocks, "1" one of the 26 neighbours has some, "2" otherwise); sky ≠ 15 as
  text at (x + 0.5, y + 0.25, z + 0.5), colour blended from `0xFF0000FF` to `0xFF00FFFF`; block ≠ 0
  at the centre, from `0xFFAA0000` to `0xFFFFFF00` (channel-by-channel blend, truncation).
- `visualize_entity_supporting_blocks`: supporting block of the local player in red, of the other
  entities on the ground (non-invisible, within 16 of the camera entity's box, list rebuilt every
  100 ms) in green, effect block (supporting, or column 0.2 below the feet) in cyan if it differs;
  outline of the block at 40 % widened by 2 × offset on each side (offset 0.02, 0.04 for the effect, plus
  a per-entity bias drawn from the hash of the decimal text of "identifier + 0.132453657"), then its
  collision shape in full colour.
- `visualize_sky_light_sections`: sections within ±10 of the player's (rebuilt every 500 ms);
  volumes "with blocks" (`0x3FFFFF00`) and "light" (with blocks or neighbour: `0x1F3F1F00`):
  merged edges in opaque colour, thickness 3, then solid outer faces.

---

## 9. Summary of implementation choices (native client)

| Topic | Vanilla | Proposal |
|---|---|---|
| `Java:` line | JVM version | `Rust: <rustc version>` |
| `Mem` / `Allocated` / `Allocation rate` | JVM heap | global allocator counter, physical memory, committed memory of the process, raw allocation rate |
| `detailed_memory` | JVM heap / non-heap | Rust allocator / wgpu GPU memory |
| GPU % | GPU timing query | wgpu timestamp queries, 0 % if unavailable |
| `Terrain Rendering` | MDI or naive | according to our drawing path |
| `pC` / `aB` | compile queue / free buffers | meshing queue / available buffers of our pool |
| Profiler, pie chart, F3+L | internal profiler | our measurement zones, game names (§ 7.7, 7.8); `system.txt`: our real values (no JVM fields) |
| F3+C 10 s | real crash | written report + clean exit |
| Client brand | `vanilla` | consistent with the one sent to the server |

---

## Uncertain points

1. **Order of the tags** (`#…`) of the targets: it follows the registry's tag set, with no
   explicit sort; the vanilla order is not guaranteed. Proposal: order of reception of the
   tags, or alphabetical.
2. **Line thickness** of the boxes and borders: values (1; 2.5; 4) read as screen
   pixels; not verified whether they are scaled with the resolution (the "adapted" width
   `max(2.5; width/1920 × 2.5)` is used by other strokes).
3. **Version field** in `debug-profile.json`: added by the data versioning wrapper
   (exact name and value not noted; default version on reading = 4649).
4. `debug.crash.message` and `debug.profiling.start` exist in `.rebindable` variants with the
   key names, but the code uses the plain keys (fixed text "F3 + C", "F3 + L") even
   if the keys were reassigned.
5. SNBT of F3+I on the entity side: "pretty" form (spaces after `:` and `,`) — format detail to
   confirm against the NBT spec before implementing.
6. `Day #`: the period (24,000) comes from the data of the `overworld_day` timeline received from the
   server; not verified whether modified servers can alter it.
7. The development visualisation entries (§ 4) were not specified in detail.
8. The tick charts (§ 7.3) depend on the server permission for the subscription; on a server
   where one is not an operator, nothing is displayed (expected), not verified on Paper.
