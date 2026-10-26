# Improved transparency ("improvedTransparency" option)

Video option of the game (off by default, turned on by the "Fabulous" preset, toggled by
F3 + X). With the option off, classic rendering is unchanged. With the option on, translucent surfaces are
no longer blended in drawing order: an **order-independent** transparency composites
them. Code: `apps/client/src/oit.rs`, `oit.wgsl`, `oit_passes.wgsl`, and one
`<family>_oit.wgsl` file per translucent family.

## 1. Targets (frame size, created on activation)

| Target | Format | Clear | Role |
|---|---|---|---|
| bounds | RGBA32F, MAX blend | (−FLT_MAX, 0, 0, 0) | (−distance, distance, depth, depth if opaque) |
| pruned bounds | RGBA32F | copy of the bounds | bounds after pruning, then bounds of the clouds |
| coefficients 0-3, 4-7 | RGBA16F, additive | 0 | absorbance in wavelets |
| accumulation | RGBA16F, additive | 0 | attenuated premultiplied color |
| cloud depth | D32F | copy of the main depth | nearest cloud face |
| terrain depth | D32F | copy + boat water mask | translucent terrain test |

MAX blending on an RGBA32F target requires the GPU feature "float32 blendable" (Vulkan and DX12
offer it); without it the option has no effect.

## 2. Steps of each translucent surface

Distance from a device depth `d` (reversed depth [0, 1]): `P[3][2] / (d + P[2][2])`
with the frame's projection (shake and bob included).

1. **Bounds**: fragment discarded below 0.01 alpha; output (−dist, dist, d, d if alpha > 0.99
   otherwise 0).
2. **Transmittance**: depth brought into [0, 1] between the near bound and the far bound
   of the pixel; if 3/4 of the range exceed 10 blocks, the first 6 of the 8 bins cover the
   first 10 blocks, the last 2 the rest. Absorbance `clamp(−ln(max(1 − α, 1e-4)), 0, 4)`
   written as 8 Haar coefficients (mean + rank-2 tree) into the two targets.
   Additive surfaces (lightning, dragon beams, energy swirls, border): nothing.
3. **Accumulation**: transmittance in front of the fragment (own contribution removed, interpolated
   between its bin and the previous one) × (rgb·α, α); additive surfaces: own transmittance 1,
   accumulated alpha 0. Fog afterwards, towards the fog color × accumulated alpha.

All the steps test depth ("≥") without writing or shifting it, except the step of the
bounds of the detailed clouds, which writes into the cloud depth. Face culling: that
of the game's pipeline for each type (terrain, items and moving blocks, particles, eyes, lightning,
beams, shadows, detailed clouds, halos, cracks, displays: back faces; translucent
models, swirls, weather, border, outlines, debug, flat clouds: both faces).

## 3. Frame order

1. Solids pass: sky, opaque and cutout terrain, opaque and cutout entities (including the
   special item models and their blended patterns, and the glints in a separate pass), faces without
   translucency of items, leashes, opaque texts without background (name tags, signs, displays),
   beam cores and portals, contrasted outline, opaque particles, opaque lines.
2. Water mask: main depth copied into the terrain depth, then boats.
3. For each step (bounds, transmittance, accumulation): translucent terrain (terrain
   depth); elements (shadows, translucent models, eyes, lightning, swirls, beams, translucent name tags and
   displays, normal outline, halos, cracks of translucent blocks only (textures `destroy_oit_stage_<n>`; those of opaque
   blocks stay in the solids pass, classic blending), translucent particles, translucent
   lines, border, weather) in the main depth; after the bounds: pruning
   (copy of the bounds, then nearly opaque fragments written into the main depth, far
   bound brought in front of them); clouds (bounds: main depth copied into the cloud depth,
   then writing); after the bounds: water mask redone.
4. Composition (premultiplied alpha): coverage = 1 − total transmittance (1 below 0.02), accumulated
   color × coverage / accumulated alpha (1 if the alpha is zero: additive light); pixel with no
   coverage or light left as is; depth of the nearest translucent surface
   written.
5. Texts seen through, then "always in front" lines; outline of glowing entities, hand and
   interface as in classic.

## 4. Known differences

- Block meshes carried by entities (falling blocks, minecart contents): filed
  entirely as translucent (the game separates the layers of the block); an opaque block there stays opaque
  (transmittance below 0.02).
- Debug lines: opaque faces and texts go through the steps (the game keeps them opaque).
