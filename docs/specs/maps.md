# Maps (data packet, storage, colors, texture, decorations, rendering)

Target version: 26.3 (protocol 777). Read in the decompiled client on 2026-10-12. Our own words.

Cross-references (not described again):
- map **in hand** (poses, background, one/two hands): `hand-camera.md` § 2.9;
- map **in a frame** (transform stack, decorations, light): `entities-objects.md` § 3.2-3.3;
- component `map_id` (varint) and `map_post_processing`: `item-components.md`; `map_decorations`
  is never sent over the network.

## 1. `map_item_data` packet (server → client, play phase)

Fields, in order:

| Field | Type | Meaning |
|---|---|---|
| id | varint | map id (the same as the `map_id` component) |
| scale | i8 | 0..4 (1 pixel = 2^scale blocks) |
| locked | bool | frozen map (cartography table + glass pane) |
| decorations | optional(list(decoration)) | presence bool, then varint length, then the elements |
| color rectangle | see below | partial pixel update |

Decoration:

| Field | Type | Meaning |
|---|---|---|
| type | varint | index in the built-in registry `map_decoration_type` (§ 4) |
| x | i8 | −128..127; pixel position = x/2 + 64 |
| y | i8 | same for y (y towards the BOTTOM of the map) |
| rotation | i8 | kept modulo 16 (`& 15`) from the read; in sixteenths of a turn |
| name | optional(text component) | presence bool then text component (usual NBT network format) |

Color rectangle:
- `width: u8`. If width = 0 → **absent**, nothing else is read.
- Otherwise: `height: u8`, `x0: u8`, `y0: u8`, then `bytes: varint length + bytes`.
- Byte at index `i` → pixel (x0 + i mod width, y0 + i div width); the game walks by columns
  but the order does not matter (pure assignment).
- The game checks nothing (rectangle outside 0..127 or array too short → exception). Us: ignore
  the bytes that would fall outside the map or are missing, without cutting the connection (tolerated difference,
  never observed with a vanilla server).

## 2. Client storage

- Table `id → map data` carried by the **client world**. Data: scale, locked, array
  of 128 × 128 color bytes (index x + 128·y, initialized to 0 = transparent), ordered list of
  decorations.
- Reception:
  1. if the id is unknown → create the entry with the scale and lock **of the packet**; if the id is already
     known, the scale and lock of the packet are **ignored** (frozen at creation);
  2. if the decorations are present → they **replace** the list entirely (packet order
     kept); absent → the old list stays;
  3. if the rectangle is present → copy its bytes into the array;
  4. mark the texture of this map "to be re-sent to the GPU".
- Dimension change (respawn into another dimension): the table is **copied** into the
  new world. New connection / new connection world: empty table.
- The GPU textures are all released when the rendering engine resets its data (disconnect,
  world change); they are recreated lazily from the table.
- "Known data" (used by the frame, the hand, the tooltip, the cartography table) = the item
  has a `map_id` AND the table contains that id.

## 3. Map colors

A pixel byte `p` (unsigned) = `base = p >> 2`, `shade = p & 3`.

The base colors are a **fixed table in the code** (64 slots, 62 defined). The mapping
block → base color is only used by the server, which computes the pixels; the client does not need it.

Base (id = hexadecimal RGB):

```
 0 transparent  1 7FB238  2 F7E9A3  3 C7C7C7  4 FF0000  5 A0A0FF  6 A7A7A7  7 007C00
 8 FFFFFF       9 A4A8B8 10 976D4D 11 707070 12 4040FF 13 8F7748 14 FFFCF5 15 D87F33
16 B24CD8      17 6699D8 18 E5E533 19 7FCC19 20 F27FA5 21 4C4C4C 22 999999 23 4C7F99
24 7F3FB2      25 334CB2 26 664C33 27 667F33 28 993333 29 191919 30 FAEE4D 31 5CDBD5
32 4A80FF      33 00D93A 34 815631 35 700200 36 D1B1A1 37 9F5224 38 95576C 39 706C8A
40 BA8524      41 677535 42 A04D4E 43 392923 44 876B62 45 575C5C 46 7A4958 47 4C3E5C
48 4C3223      49 4C522A 50 8E3C2E 51 251610 52 BD3031 53 943F61 54 5C191D 55 167E86
56 3A8E8C      57 562C3E 58 14B485 59 646464 60 D8AF93 61 7FA796
```

Bases 62 and 63 are undefined → treated as 0.

Shades (multiplier m out of 255): 0 → 180, 1 → 220, 2 → 255, 3 → 135.

Final color: base 0 → fully **transparent** pixel (ARGB 0, whatever the shade); otherwise
alpha 255 and each channel c' = ⌊c × m / 255⌋ (integer, clamped 0..255). Precomputable table: 256 RGBA
entries, computed once.

## 4. Decoration types (built-in registry `map_decoration_type`)

Network index = order below. Columns: registry name, sprite (`textures/map/decorations/<sprite>.png`),
visible in a frame (F), counted (C, used by the server only).

| # | name | sprite | F | C |
|---|---|---|---|---|
| 0 | player | player | no | yes |
| 1 | frame | frame | yes | yes |
| 2 | red_marker | red_marker | no | yes |
| 3 | blue_marker | blue_marker | no | yes |
| 4 | target_x | target_x | yes | no |
| 5 | target_point | target_point | yes | no |
| 6 | player_off_map | player_off_map | no | yes |
| 7 | player_off_limits | player_off_limits | no | yes |
| 8 | mansion | woodland_mansion | yes | no |
| 9 | monument | ocean_monument | yes | no |
| 10-25 | banner_<color> (white, orange, magenta, light_blue, yellow, lime, pink, gray, light_gray, cyan, purple, blue, brown, green, red, black) | <color>_banner | yes | yes |
| 26 | red_x | red_x | yes | no |
| 27 | village_desert | desert_village | yes | no |
| 28 | village_plains | plains_village | yes | no |
| 29 | village_savanna | savanna_village | yes | no |
| 30 | village_snowy | snowy_village | yes | no |
| 31 | village_taiga | taiga_village | yes | no |
| 32 | jungle_temple | jungle_temple | yes | no |
| 33 | swamp_hut | swamp_hut | yes | no |
| 34 | trial_chambers | trial_chambers | yes | no |
| 35 | abandoned_camp | abandoned_camp | yes | no |
| 36 | ancient_city | ancient_city | yes | no |
| 37 | desert_pyramid | desert_pyramid | yes | no |
| 38 | mineshaft | mineshaft | yes | no |
| 39 | ocean_ruin_warm | warm_ocean_ruins | yes | no |

Sprites: `map_decorations` atlas (source: the whole `textures/map/decorations/` folder, no prefix),
8×8 images in vanilla. Unknown type → index outside the registry: ignore the decoration (tolerated difference).

Players off the map: nothing special on the client side — the server already sends `player_off_map` (player
near the edge) or `player_off_limits` (far away) with x/y clamped to the edge; the client draws them like the
others. Names come from the server (named banners, frames, etc.).

## 5. Map texture

- One 128×128 RGBA texture per id (internal name `map/<id>`), created the first time this map is rendered.
- The game, when the map is marked, **reconverts the 16,384 pixels and re-sends the whole image** to the
  GPU, lazily, when the map is drawn (not when the packet is received).
- Us: same result with a **partial** upload — accumulate the dirty rectangle (union of the rectangles
  received since the last upload) and write only that rectangle (`write_texture` of a sub-region); a
  new map or a recreated texture → whole image. Pixel-for-pixel identical result.
- Nearest-neighbor sampling, no mipmaps.

## 6. Common map drawing (frame 0..128, y towards the bottom of the map)

1. Quad (0,0)-(128,128) at z = −0.01, UV (0,0) at (0,0) and (1,1) at (128,128), white, "text"
   render type (alpha blend, rejection of nearly transparent pixels), map texture.
2. Decorations, in list order, n = counter of the **drawn** decorations (0, 1, …):
   T(x/2 + 64; y/2 + 64; −0.02), RZ(rot × 22.5°), S(4; 4; 3), T(−0.125; 0.125; 0); quad from
   (−1,1) to (1,−1) at z = −0.001 × n; UV: (−1,1) ↔ (u0,v0), (1,1) ↔ (u1,v0), (1,−1) ↔ (u1,v1),
   (−1,−1) ↔ (u0,v1); decoration atlas texture, white, same light as the map.
3. Optional name (outside the sprite's stack): w = text width (font), k = clamp(25 / w, 0, 2/3);
   T(x/2 + 64 − w k / 2; y/2 + 68; −0.025), S(k; k; −1), T(0; 0; 0.1); white text without shadow,
   black background alpha 0x80, normal mode, pass of order 1 (after the quads).
4. Filter: **in a frame** and **in the interface** only the decorations "visible in a frame" (column F)
   are drawn (n counts only those); **in hand**, all of them.

Transparent pixels (base 0): in hand you see the checkered background (drawn before, see hand-camera); in a
frame you see the wall behind.

## 7. Frame (summary, detail in entities-objects.md § 3.3)

- Map whose data is known: frame model variant `map=true` (`item_frame_map` /
  `glow_item_frame_map`, full 16×16 frame instead of 10×10 in the center); the map covers the whole face
  (128 px × 1/128 = 1 block).
- Offset: T(0; 0; 0.4375) (0.5 if the frame is invisible: map flat against the wall, frame
  model not drawn); content rotation = (rotation mod 4) × 90°.
- Light: normal frame → entity light; glow frame → (block 210, sky 240) raw.
- **Unknown data**: no map drawn, the `map=false` model is chosen and the map item is
  drawn as an ordinary item (S(0.5), RZ(rotation × 45°), `fixed` display).

## 8. Hand

See `hand-camera.md` § 2.9: the "map content" it returns out of scope is § 6
above with **all** the decorations and the hand's light.

## 9. Tooltip

No graphical preview in the Java client: only gray lines of text, added in place
of the `map_id` component in the order of the component lines (after instrument, before bees):
- unknown data → `filled_map.unknown`;
- otherwise: if no custom name AND no `map_post_processing` → `filled_map.id` (argument = id);
  if locked or post-processing = lock → `filled_map.locked`;
  in advanced mode (F3+H): s = min(scale + (1 if post-processing = enlarge), 4) →
  `filled_map.scale` (argument 2^s) then `filled_map.level` (arguments s and 4).

## 10. Cartography table (preview)

The screen draws a preview of the map in slot 0 (if the data is known) depending on the item in slot 1:
- **paper** (enlarge) and no error: sprite `container/cartography_table/scaled_map` at (67, 13)
  66×66, map at (85, 31) scale 0.226;
- **empty map** (duplicate): sprite `duplicated_map` 50×66 at (83, 13), map at (86, 16) scale 0.34;
  new layer; second sprite at (67, 29), map at (70, 32) scale 0.34;
- **glass pane** (lock): sprite `map` 66×66 at (67, 13), map at (71, 17) scale 0.45, then padlock
  `locked` 10×14 at (118, 60);
- otherwise (or paper in error): sprite `map` at (67, 13), map at (71, 17) scale 0.45.
Error (sprite `error` 28×21 at (35, 31)): map already locked with paper or glass pane, or paper on
a non-extendable map (outside the `extendable_maps` tag) or of scale ≥ 4. Coordinates relative
to the corner of the background `textures/gui/container/cartography_table.png`.
Drawing the map in the interface = § 6 (pixels over the full 128×128 then decorations filtered "visible
in a frame", names included), without light.

## Uncertain points

- Network index of the decoration types: deduced from the registration order in the code (built-in
  registry, not synchronized); to be confirmed with a real packet (player = 0, frame = 1).
- Visual direction of the sprites: the quad puts v0 at the top of the y+ axis (so towards the bottom of the map); same
  convention in the interface. Check on screen that the player arrow points the right way.
- Map texture filtering (nearest assumed) and the rejection alpha of the "text" type: not
  re-read in this mission.
- Name text component: standard optional codec assumed (bool + network NBT).
- If a decoration's sprite cannot be found, the game forgets to pop the pose (bug); the atlas
  normally returns the "missing" sprite, so it has no practical effect. Us: always pop.
