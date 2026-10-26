# Section visibility (occlusion sorting)

Behaviour of the official client, re-read in the game and rewritten in our own words. Directions in
the order down, up, north, south, west, east.

## 1. Communication between the faces of a section (at meshing)

- A block is "full" if it has the solid render flag (complete opaque cube).
- Fewer than 256 full blocks: all faces communicate. 4,096 full blocks: none.
- Otherwise: for each cell on the edge of the section (x, y or z equal to 0 or 15), in the order x,
  then y, then z increasing, if it is neither full nor already visited, breadth-first fill
  through the six neighbors inside the section, across the non-full cells. The section faces
  touched by the filled area (x = 0 → west, x = 15 → east, etc.) all
  communicate pairwise.
- Empty section (no block): all faces communicate. Section not yet meshed:
  none.

## 2. Walk from the camera (per frame)

- Start: the camera's section, with no arrival direction. Camera outside the world vertically: all
  the sections of the highest (or lowest) layer within the view
  distance, with the arrival direction "down" (or "up") and the east/west
  and south/north directions depending on the side, sorted by distance to the camera.
- Queue (first in, first out). Each node keeps: its arrival directions (the
  directions through which neighbors reached it) and the cumulative set of the directions
  followed from the camera.
- For a node, each direction d in order:
  1. the neighbor in direction d must exist (column received, height inside the world, inside the
     view distance);
  2. we never go back towards the camera: d is refused if the opposite of d is in the node's
     cumulative set;
  3. if the node has arrival directions, one of them (taken from the node's side, so its
     opposite) must communicate with d in the node's section (§ 1);
  4. distant section (more than 3 sections apart on an axis, 60 blocks): line test towards the
     camera. Start point: the corner of the node's section located on the camera's side
     (on the axis of d: high corner if the center of the camera's section is beyond the
     origin of the section, otherwise low; on the other two axes: high corner if this center is
     before the origin). We advance towards the camera in steps of 28 blocks (diagonal of a
     section rounded up) while the squared distance exceeds 3,600; we stop if the
     point leaves the world vertically; each point must fall in a section already reached
     by the walk, otherwise d is refused;
  5. if the neighbor already has a node, we only add d to it as an arrival direction; otherwise
     we create its node (arrival direction d, cumulative set = that of the node plus d) and
     queue it.
- The sections reached and not empty are drawn if their box is in the field of view.
- Rule 4 is an approximation of the game: it can discard a few distant sections that are
  actually visible (bench: 252 pixels out of 1.44 M, at the bottom of the ocean seen through water).
  Our frame follows that of the game, not that of a rendering without sorting.
- The "smart" test (rules 2 to 4) is turned off when the camera is inside an opaque block
  as a spectator.

## Sorting of translucent surfaces (translucent layer without improved transparency)

- Within each section, the translucent quads are drawn from farthest to nearest
  (squared distance from their center to the camera), ties in meshing order.
- Viewpoint of a section: on each axis, the camera's section minus this one, clamped to
  −1..1 (27 values). It is noted at each sort, and at meshing (sorted with the camera of the moment).
- A section is "in the axis" if one component of its viewpoint is zero (the camera is
  in its x, y or z slice).
- On each frame, we examine the nearby visible sections (within ±2 sections on all three axes),
  then max(visible sections / 8, 15) visible sections in round-robin (the cursor advances by the number
  examined). An examined section is re-sorted if its viewpoint changed since its last sort,
  or if the camera just changed block this frame and it is in the axis. Being nearby
  only speeds up the examination: a nearby section out of the axis, with an unchanged viewpoint,
  is not re-sorted.
- Improved transparency: no re-sort (the order does not change the image there).
- In our code (`RUSTCRAFT_OPT_TRANSPOV`, default): the same rule per section, the examination being done
  per column (the 5 × 5 columns around the camera each frame, then the round-robin over the
  loaded translucent columns, visible or not); difference without visible effect: a section outside the
  field of view can be re-sorted, a section nearby vertically is examined earlier. One GPU write
  per run of contiguous re-sorted sections. `=0`: old path (whole column re-sorted after
  each block change, round-robin), which re-sorts more often than the game.
