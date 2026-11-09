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

## 3. Which dirty sections are rebuilt (per frame)

- The list of visible sections holds the sections the walk reached whose box is in the view
  frustum, the frustum being moved back until the camera's 8-block cell lies wholly inside it. The
  list is refreshed when the walk changes or when the camera turns (yaw or pitch rounded down to a
  multiple of 2 degrees).
- Each frame, only the sections of that list are looked at: a dirty one is rebuilt if it already
  has a mesh, or, never built, once the 8 columns around it are loaded; its mark is then cleared.
  A dirty section outside the list keeps its mark and is rebuilt on the first frame it enters the
  list. Its old mesh stays drawn meanwhile, and while its rebuild runs.
- The walk enters a never-built section (so it is in the list and gets built) but does not cross
  it: its faces communicate only once it has a mesh, and the walk then resumes from it.
- Priority (`prioritizeChunkUpdates`): among the sections rebuilt, one changed by the player
  (setting 1 or 2), or whose centre is within √768 blocks of the camera (setting 2), is rebuilt
  before the frame is drawn; the others in the background.
- Our version (`RUSTCRAFT_OPT_MESHVISIBLEONLY`, default; `=0`: every dirty column whatever the
  view, as before), same rule per section of a column, on the same frustum moved back (steps of 4
  blocks until the camera's 8-block cell lies inside it), with these differences:
  - our walk does not enter a column without a mesh, so a column the renderer holds no mesh of
    (never meshed, or its first mesh still on its way) is meshed whatever the view: the loading
    order does not change;
  - the view is that of the last frame drawn (section box plus one block): a section entering the
    view is launched on the next frame. Thanks to the moved-back frustum, the sections next to the
    camera are already in it before a turn shows them; a section entering the view farther away is
    rebuilt in the background in both games, and ours waits one frame more before it starts;
  - a column whose remesh would be a priority one is never held (priority unchanged);
  - a column whose last mesh is no longer kept as a base, or that must be redone whole, is meshed
    whole (all its marks) as soon as one of its dirty sections is in view: its task meshes the
    whole column in any case;
  - the player mark is per column (the game's is per section), so a player change makes the whole
    column a priority one.
- Cautious variant (`RUSTCRAFT_OPT_MESH_HIDDEN_IN_SPARE_SLOTS=1`, off by default): the held
  columns take the task slots the visible ones leave free.
- How we find the sections to rebuild (no effect on which ones): the list in view is the walk's
  sections crossed with the frustum, and the walk does not move when only the view turns. So a
  dirty column whose marks miss every section the walk reached is held under every view until the
  walk or its marks change; the pick tests against the view only the other dirty columns (never
  drawn, priority, or marked on a reached section), listed again when the marks, the walk or the
  columns drawn change (`RUSTCRAFT_OPT_MESH_PICK_REACHED_MARKS`, default; `=0`: every dirty column
  each frame). The game walks its whole list each frame instead; the result is the same.

## Particles (per frame)

- A particle is drawn when its position of the last tick (not the interpolated one) lies in the
  culling frustum whose eye has been moved back by three "view vectors". There is no occlusion
  test: a particle in a section the walk did not reach, or above the world, is still drawn, and the
  depth buffer hides it behind the terrain.
- View vector: the culling matrix (projection × camera rotation) applied, transposed, to the depth
  axis, that is its depth row. The game's culling projection maps depth to [0, 1] (OpenGL with
  clip control, Vulkan), so the vector is the look direction scaled by far / (far − near), a little
  more than one block: the eye moves back by just over three blocks.
- The test is a point test: the point, relative to the moved eye (difference in doubles, then
  floats), is on the inner side of the six planes (dot product plus constant ≥ 0, no corner choice).
  The game takes its near plane from the rows with the rule written for a [−1, 1] depth, which puts it
  at half the near distance; three blocks behind the camera, it never changes what is on screen.
- Effect: a particle whose centre is just off screen (within about three blocks of the edge of the
  view) is still drawn, so the part of its quad that is on screen does not vanish at the edge.
- Our version (`RUSTCRAFT_OPT_PARTICLE_GAME_FRUSTUM`, default): the same test on our planes (frame
  of the camera's integer origin, our own near plane, so the slab between the two near planes, behind
  the camera, differs). `=0`: the test used before, the entities' box test on the point with the
  occlusion of the walk; it hid particles whose centre was off screen and those of unreached
  sections (an approximation of the walk, rule 4, could then hide a visible one).
  `RUSTCRAFT_OPT_PARTICLE_FRUSTUM_SIMD`: the six planes four at a time in SIMD lanes, same answer.

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
