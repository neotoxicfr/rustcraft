# Block models: reading, variant choice, face construction (26.3)

Sources: public resource pack format, reading of the official game (`decompilation` procedure).
Resources (states, models, textures) are read at run time from the player's jar and its packs.

## Choosing the model of a state
- Blockstate file: either `variants` (key = list of `property=value` separated by commas, a subset of the state's properties; empty key = all states), or `multipart` (list of `when` + `apply`, `OR`/`AND` conditions and alternative values separated by `|`).
- Each entry is a variant or a weighted list (`weight`, default 1).
- Draw: seed = function of the position (below), 48-bit linear congruential generator (multiplier 25214903917, increment 11, initial seed `(s ^ 25214903917) & (2^48 − 1)`), integer bounded by the classic method (power of 2: `(bound × next(31)) >> 31`, otherwise rejection). The same seed is used successively for all the weighted lists of the block (order of the parts).
- Seed of a position: `s = (x × 3129871) ^ (z × 116129781) ^ y` (64 bits), then `s = s × s × 42317861 + s × 11`, result `s >> 16`. Some blocks shift the position (upper half → lower half, bed head → foot): shift measured by the extractor, state by state.
- Variant: model, rotations `x`, `y`, `z` (0/90/180/270), `uvlock`. Total rotation = Z ∘ Y ∘ X (X first); `x: 90` = −90° around +x, `y: 90` = −90° around +y (north → east).

## Models
- Inheritance through `parent`: textures are merged (the child wins), `elements` come from the closest one that defines them. `#name` references resolved step by step; missing texture → neutral fallback.
- A texture can be an object `{ "sprite": …, "force_translucent": true }`.
- Element: `from`/`to` (units 0–16), `rotation` (`origin` in units 0–16, either `axis` + `angle`, or `x`/`y`/`z` in degrees applied Z∘Y∘X), `rescale` (scaling by 1/max|component| per axis after rotation), `shade` (default true), faces.
- Face: optional `uv`, `texture`, `cullface`, `rotation` (0/90/180/270: shifts the vertex index used to read the UVs by 0/1/2/3), `tintindex` (−1 by default).

## Building a face
- Corners (fixed order per direction, `m` = min, `M` = max):
  down `(m,m,M)(m,m,m)(M,m,m)(M,m,M)`; up `(m,M,m)(m,M,M)(M,M,M)(M,M,m)`; north `(M,M,m)(M,m,m)(m,m,m)(m,M,m)`; south `(m,M,M)(m,m,M)(M,m,M)(M,M,M)`; west `(m,M,m)(m,m,m)(m,m,M)(m,M,M)`; east `(M,M,M)(M,m,M)(M,m,m)(M,M,m)`.
- Default UVs (without `uv`): down `(fx, 16−tz, tx, 16−fz)`; up `(fx, fz, tx, tz)`; north `(16−tx, 16−ty, 16−fx, 16−fy)`; south `(fx, 16−ty, tx, 16−fy)`; west `(fz, 16−ty, tz, 16−fy)`; east `(16−tz, 16−ty, 16−fz, 16−fy)`.
- UV of vertex i: `u = (i ∈ {0,1}) ? u_min : u_max`, `v = (i ∈ {0,3}) ? v_min : v_max`, read at index `(i + rotation/90) mod 4`, divided by 16.
- Position: corner / 16, element rotation around its origin, then variant rotation around the center (0.5, 0.5, 0.5). The final direction of the face is that of the normal after rotation; `cullface` follows the same rotation.
- `uvlock`: the UVs (centered at 0.5) go through the inverse of the face transform: action = rotation ∘ (local→global frame of the original face); new face = direction closest to action·(0,0,1); transform = (global→local of the new face) ∘ action. Local→global frames: south = identity, east = +90° around y, west = −90° around y, north = 180° around y, up = −90° around x, down = +90° around x.

## Render layer
Deduced from the texture of each face, over only the area covered by its UVs (the whole texture if the UVs cover 0–16): a pixel with alpha 0 → "cutout", a pixel with alpha strictly between 0 and 255 → "translucent" (takes priority); otherwise opaque. `force_translucent` forces translucent. Leaves can be forced opaque by turning off the "cutout leaves" graphics option.

## Directional shading
All faces are shaded according to a direction (table of the dimension; default dimension:
down 0.5, up 1, north/south 0.8, west/east 0.6). Direction = the element's `shade_direction_override`
if given ("up" for crosses, crops, vines, ladders, torches…), otherwise the
direction of the face after rotation. The old `shade` boolean is no longer read.

## Random model offset
Some blocks (grasses, flowers, bamboo, pointed dripstone…) offset their model according to the seed
of the position (x, 0, z): x = clamp(((seed & 15)/15 − 0.5) × 0.5, ± horizontal bound of the
block); z likewise with (seed >> 8) & 15; for blocks with a vertical offset, y =
(((seed >> 4) & 15)/15 − 1) × vertical bound. The divisions by 15 are done in single
precision, the rest in double. The kind and bounds are collected per block from the installed game.

## Mipmaps of block textures
Four levels below the original image (default setting), computed on the whole image
(full strip for an animated texture, cut afterwards frame by frame).
- Strategy: `texture` section of the `.png.mcmeta`, field `mipmap_strategy` ("auto" by default,
  "mean", "cutout", "strict_cutout", "dark_cutout") and `alpha_cutoff_bias` (0 by default).
  "auto" = "cutout" if any pixel of the image has zero alpha, otherwise "mean".
- Level 0, cutout strategies: "cutout"/"strict_cutout" replace the color of each
  zero-alpha pixel with that of the nearest non-empty pixel (breadth-first walk from all the
  non-empty pixels, taken column by column, neighbors in the order +x, −x, +y, −y; alpha left at
  0); "dark_cutout" gives them the color of the darkest non-empty pixel (minimal r+g+b
  sum, first found column by column), components × 3 / 4 (integer division), alpha 0.
- Level n+1 from level n, per 2×2 square: "dark_cutout" = average in linear light of the
  non-zero-alpha pixels only (alpha included, converted like a color), divided by 4; otherwise
  alpha = integer average of the 4 alphas, colors = integer average of the 4 linear values.
  Linear: table of 256 entries, value = round(1023 × sRGB→linear(i/255)); back: table
  of 1024 entries, value = round(255 × linear→sRGB(i/1023)) (standard sRGB formulas, thresholds
  0.04045 and 0.0031308, exponents 2.4 and 1/2.4).
- Cutout strategies, at each level ≥ 1: we look for an alpha factor (bisection between 0 and
  4, start 1, 5 tries at most, stop if the coverage is exact) that makes the coverage equal to
  that of level 0; coverage = share of the 16 sample points (4×4 per square of 4 neighboring
  pixels, bilinear interpolation of the alphas × factor, clamped to [0, 1]) whose alpha exceeds the
  threshold (0.5, or 0.3 in "strict_cutout"). Then each alpha becomes clamp(alpha × factor +
  bias + 0.025, 0, 1), converted to 8 bits by truncating × 255.

Rendering (Rustcraft choice): the layers of the texture array have the size of the largest
texture (32 for the base game). A 16 texture is placed one level lower there: level 0 =
its level 0 enlarged by duplication, following levels = its levels 0 to 4. Beyond the 4 levels
of the game (case of 32 textures), the last level is completed by the same rule.

## Animated textures
A `<texture>.png.mcmeta` file with an `animation` section (public resource pack format)
describes a strip of frames:
- frame size: `width` and `height` if given (only one: the other side takes that of the whole
  texture), otherwise a square of the smaller side; frames numbered row by row;
- steps: the `frames` list (a number = frame number, or an object `index` + `time`), otherwise
  all the frames in order; default duration `frametime` (1 tick); a step whose number is
  outside the strip or whose duration is ≤ 0 is ignored; fewer than two steps → fixed texture;
- playback: we start at the first step; the step counter advances by one per
  rendered frame that is owed at least one client tick (only one advance even if the frame catches up on
  several: below 20 frames per second, all animations slow down), before those ticks,
  and not when the world is frozen (`/tick freeze`); playback continues while paused; the next
  step (looping) starts when the counter reaches its duration;
- same clock for all the atlases: blocks, item sprites (interface and world), animated
  entity textures (conduit wind), animated interface sprites (notes of the music
  toast), and the flames of an entity on fire, which show the two textures of the fire block
  (`fire_0`, `fire_1`) at their current frame;
- `interpolate`: the displayed frame is the linear blend from the current step to the
  next, with weight counter / duration, truncated to the thousandth; each mip level blends the
  corresponding mips of the two frames. Without blending, the frame of the step is displayed as is.
- Without a `.mcmeta` (or unreadable), a vertical strip is reduced to its first square frame.

Rendering (Rustcraft choice): each frame in use occupies a layer of the texture array, the
first step reusing the layer of the texture; a buffer with one entry per texture gives,
on each tick, the displayed layer, the next one and the blend weight. Beyond the maximum number
of GPU layers, the excess frames are dropped and the texture stays fixed. In the
sheets with a single frame per texture (item sprites of the interface, entity textures, where
the item sprites and the textures of blocks drawn within an entity are copied), the current
frame is rewritten in place when it changes, on every advance for a blended animation.
