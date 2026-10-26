# Crosshair targeting and block outline (26.3)

Source: reading of the decompiled client (outside the repository), reworded.

## 1. Targeting result (computed every frame, partial tick included)

Ranges: player attributes `block_interaction_range` (4.5 by default) and
`entity_interaction_range` (3 by default). A held item with the `attack_range` component
(spears) has its own targeting, applied first (not tracked here for now).

1. `max` = max(block range, entity range). Origin = camera eyes, direction = view vector;
   end = origin + direction × `max`.
2. **Blocks**: walk (§ 2) from the origin to the end with the **outline** shape
   of each state, fluids ignored. If a block is hit, `max` becomes the distance to the
   point hit.
3. **Entities**: candidates = selectable entities (§ 3) other than the camera. For each one, its
   box inflated by the selection radius (0, or 1 for selectable projectiles):
   - if the box contains the origin (min ≤ v < max on all three axes): kept at distance 0,
     point = entry of the box if there is one, otherwise the origin;
   - otherwise, if the segment crosses it at point *p* with d² = |p − origin|² < best (or if
     the best is 0): kept (an entity of the same root vehicle as the camera is
     only kept if the best is 0, without changing it).
   The best starts at `max`².
4. Result: the entity if it exists and its point is closer than the block point
   (squared distance, the "missed" point at the end of the segment otherwise), filtered by the entity
   range; otherwise the block, filtered by the block range. Filter: point at distance ≥ range →
   "missed" result.

The targeted entity is used for the name at the crosshair (label of named mobs).

## 2. Block walk

- Both ends are first pushed outwards by 1e-7 of the length.
- Start block = floor of the start, tested first.
- Grid step (DDA): `t` per axis = remaining fraction up to the next face, divided
  by |delta|; we advance on the axis with the smallest `t` (ties: z before x, then y before z
  following the order `tX < tY ? (tX < tZ ? x : z) : (tY < tZ ? y : z)`), while one of the `t` ≤ 1.
- Testing a shape: empty → nothing. If the point origin + 0.001 × (end − origin) is inside the
  shape: hit "from the inside" at that point. Otherwise, for each box (offset by the block),
  entry face: for each axis whose |delta| > 1e-7, min plane (delta > 0) or max plane (delta < 0),
  `s` = (plane − start) / delta, accepted if 0 < s < best `s` (1 at the start) and the other
  two coordinates in [min − 1e-7, max + 1e-7] (strict); point = start + s × delta.
- First block with a point: result.

## 3. Selectable entities

Living entities (armor stand except "marker", player except spectator), End crystals,
item frames, paintings, leash knots, `interaction`, falling blocks, primed TNT, arrows and
tridents that are not stuck, redirectable projectiles (fireballs, wind charges…), shulker
bullets, boats and rafts, minecarts.

Box: current width × height centered on the entity's position (feet).

## 4. Outline of the targeted block

Conditions: HUD visible, result of block type, non-empty state. The player must be able to
build (survival, creative); in adventure only if the held item can break or place
on this block; as a spectator only for blocks with a menu.

- Shape: outline (`outline`) of the state.
- Edges: those of the merged shape (not those of each box): on the grid of the
  distinct coordinates of the boxes, an edge is drawn when, among the four cells that
  surround it, one or three are full, or two diagonally; consecutive aligned edges are
  merged.
- Black color with alpha 102 (0.4); thickness max(2.5, window width / 1920 × 2.5) pixels.
  High contrast option: black 7 px pass then color 0xFF57FFE1 (not tracked here).
- Line drawn as a quadrilateral in screen space: each end projected, normalized screen
  direction, perpendicular offset of ± thickness / 2 pixels; the view is shrunk by
  1/256 towards the camera to come in front of the faces.
- Opaque block: with the terrain; translucent block: after the translucent terrain.
