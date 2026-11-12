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
- Empty section (no block): all faces communicate. Section of a loaded column not yet meshed: all
  of them if it holds air alone, none otherwise (the walk holds it without going through it, unless
  it is the camera's). Ours: `apps/client/src/render/unmeshed_columns.rs`, from the section's count
  of non-air blocks, kept while the column waits for its mesh (at loading, after a teleport, and
  always for the outermost ring of the loaded columns, which never has its 8 neighbours).

## 2. Walk from the camera (per frame)

- View distance `vd`: the render distance, at most the server's view distance. A column is within
  it when, each axis gap to the camera's column shortened by one column (never below 0), the sum
  of their squares is below `vd²` (the shape of the columns the server sends); a section, when its
  column is and it lies at most `vd` sections above or below the camera's section. Columns held
  beyond are not walked.
- Start: the camera's section, with no arrival direction. Camera outside the world vertically: all
  the sections of the highest (or lowest) layer whose column is within the view
  distance, whatever their height, with the arrival direction "down" (or "up") and the east/west
  and south/north directions depending on the side, sorted by distance to the camera. From more
  than `vd` sections above the world, the walk stops there: only that layer is drawn.
- Queue (first in, first out). Each node keeps: its arrival directions (the
  directions through which neighbors reached it) and the cumulative set of the directions
  followed from the camera.
- For a node, each direction d in order:
  1. the neighbor in direction d must exist (column received, height inside the world, section
     within the view distance);
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

### Sections the fog hides whole (ours)

- The game draws every reached section in the field of view, even one the fog covers whole: each
  of its fragments is then the plain fog colour (the fog value is 1 once the spherical distance
  reaches the end of the environment fog, or the cylindrical one the render distance,
  `environment.md` § 7).
- Our version (`RUSTCRAFT_OPT_FOG_CULL`, default; `=0`: every reached section drawn): a reached
  section is left out of the terrain draw lists when each of its points, two blocks of model
  overflow included, is in plain fog for every eye of the camera's 8-block cell, and nothing but
  the frame's background, cleared to the fog colour, can show behind it: no sky dome, no End sky,
  no cloud still visible that far (the clouds fade at the end of a fog of their own), no
  fog-piercing draw that far (below). The picture is the same: whatever else is drawn behind such
  a section lies farther on the same sight line, in plain fog too. This is the case in the Nether
  (fog ending at 96 blocks whatever the render distance), in lava and powder snow, with
  blindness, darkness or a boss fog. The walk, the entity visibility (a glowing outline shows
  through the fog) and the meshing still see the section.
- Draws behind a hidden section, by what plain fog does to them (the inventory
  `every_world_shader_and_blend_is_classified` keeps, `apps/client/src/render/fog_cull_test.rs`):
  - turned into the fog colour, so nothing changes: every draw whose shader applies the world fog
    and whose blend is opaque or alpha (terrain, entity models, shadows, eyes, beams, block
    effects, particles, rain and snow, signs, labels, text displays, leashes, block outline, debug
    segments and texts). Their fog distances are interpolated from the vertices and are convex in
    the position, so they are never under the true ones. The enchantment glint fades to nothing;
  - not depth-tested, so the section never hid them: glowing outlines, see-through labels and text
    displays, "always in front" debug traces;
  - fog-piercing, the section used to hide them and the game shows them through its fog: no fog
    in their shader (world border wall, filled faces of the debug overlays), or a blend that adds
    to the pixel (lightning, energy swirl of a charged creeper or of the wither's armour, death
    rays of the dragon: fog colour added to fog colour is twice as bright) or multiplies it
    (mining cracks). While one of them reaches as far as a hidden section can stand (the nearer
    end of the two fogs, rounded up like the reach), every section is drawn, under every switch.
    Measured from the eye: the energy swirl (twice the entity's box around its feet) and the
    cracks (their vertices, at most 32 blocks away); taken as infinitely far: the wall, the debug
    faces, lightning and the dragon's rays. A crack under the crosshair or a wither fought at
    close range in the Nether does not stop the cull.
- Improved transparency: a fog-coloured translucent fragment that a hidden section used to cover
  (it belongs to a farther section that is not hidden whole) enters the depth bounds and the
  wavelet transmittance of its pixel, which then weighs the nearer translucent fragments by a
  slightly different approximation. Every section is drawn under that option;
  `RUSTCRAFT_OPT_FOG_CULL_UNDER_IMPROVED_TRANSPARENCY=1` (off) lets the paths that are on cull
  there too, to be settled by screenshots.
- Under a drawn sky the section stands as a fog-coloured patch in front of sky that is not
  fog-coloured, so two more paths stay off until a screenshot comparison settles them:
  `RUSTCRAFT_OPT_FOG_CULL_BELOW_SKY_RIM=1` leaves out the sections that stay under the slope where
  the sky dome itself is plain fog colour (slope 16 × 496 / (512 × (sky fog end − 16)): 1.8° at
  render distance 32, 11° under water at full vision), not when the dark disc or the End sky is
  drawn; the sun, the moon, the stars, the sunrise glow and low clouds can then show where such a
  section hid them. `RUSTCRAFT_OPT_FOG_CULL_AGAINST_SKY=1` leaves out every hidden section: the
  fog-coloured outline of far relief against the sky goes away too.
- `RUSTCRAFT_OPT_FOG_CULL_CUTS_WALK=1` (off): the walk itself does not enter the sections wholly
  beyond the sphere that holds everything the fog shows (same terrain drawn, shorter walk). Off
  because those sections are then never compiled for the entities (§ Entities), which removes a
  glowing outline or a see-through label the game shows through the fog, and their dirty sections
  wait to be remeshed.

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
  - a column the renderer holds no mesh of (never meshed, or its first mesh still on its way) is
    meshed whatever the view: the loading order does not change;
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
- How we find the sections to rebuild (no effect on which ones): the list in view is the walk's
  sections crossed with the frustum, and the walk does not move when only the view turns. So a
  dirty column whose marks miss every section the walk reached is held under every view until the
  walk or its marks change; the pick tests against the view only the other dirty columns (never
  drawn, priority, or marked on a reached section), listed again when the marks, the walk or the
  columns drawn change (`RUSTCRAFT_OPT_MESH_PICK_REACHED_MARKS`, default; `=0`: every dirty column
  each frame). The game walks its whole list each frame instead; the result is the same.

## Entities and block entities (per frame)

- Compiled section: one the game has a mesh of, possibly empty. It compiles a section the first
  time its walk reaches it in view and its 8 neighbouring columns are ready, and takes an empty
  section (air alone) of a loaded column as compiled the first time its walk reaches it. Its
  appearance fade starts when it became compiled.
- The game holds its sections in a square of columns around the camera's section, as many on each
  side as the render distance (the option, at most the server's view distance). A section outside
  the square is never compiled, so nothing in it is drawn, even in a column the server still sends.
  When the camera changes section, the sections the square leaves are reset (mesh and fade
  forgotten); a new render distance resets them all. Unloading a column resets nothing: sent again
  within the square, its sections are still compiled. The square moves at the start of the game's
  drawing, after the entity test of that frame (one frame behind the camera, ours likewise).
- An entity is drawn when its box is in the view frustum (or it is one of those drawn whatever the
  field) and the section of its block position (its position of the last tick, rounded down) is
  compiled and has appeared to 0.3 of its fade, or that position is above or below the world. The
  occlusion walk is not asked: an entity behind a hill, in a section seen once, is drawn, and its
  glowing outline and see-through name show through the hill; one in a cave never seen is not.
  A leash is drawn with its leashed entity, and the debug box of an entity (F3+B) with the entity
  (one test for the three, `FrameVisibility::entity_shown`).
- A block entity is drawn from a visible section (reached by the walk, box in the frustum) that is
  compiled and has appeared to 0.3 of its fade; beacons and test instances are drawn whatever the
  section.
- Ours (`apps/client/src/render/compiled_sections.rs`): a column is meshed whole, so a section
  counts as compiled from the first walk that reaches it while its column is meshed (or from the
  first mesh of a column the last walk reached), and an empty section of a column waiting for its
  mesh from the first walk that reaches it. Difference: the game also asks the view frustum of a
  section it compiles; a section the walk reached only behind the camera is compiled here, there
  when the camera turns to it (the box of an entity in a section out of view is out of view too:
  only the 0.3 fade, 225 ms by default, of such an entity after the turn differs). Second
  difference: on a new render distance the game resets its sections before that frame's entity
  test, ours at the frame's drawing, one frame later.
- Terrain fade: the game draws a section's terrain with the fade of that same section, so a
  section the square reset fades in again once compiled anew, and one never compiled is not drawn.
  Ours (`FadeRing`, per column, `terrain_shader.rs`): a column's fade starts when the first of its
  sections counts as compiled while it is meshed (same instant as above), the square forgets it
  with its sections (and all of them on a new render distance), and unloading keeps it, so a
  column sent again within the square does not fade in again. A column meshed but not born yet
  (reached by the walk applied after the frame sent its fades, or by a walk still in use after
  the square left it) is drawn at visibility 0, the fog colour beyond 16 blocks, where the game
  draws nothing; that lasts one frame, or until the late walk arrives.

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
