# Test maps

Two local offline 26.3 servers, in addition to the bench server (`127.0.0.1:25565`, generated
world, seed 20261011):

| Server | Address | World | Used for |
|---|---|---|---|
| Test map | `127.0.0.1:25570` | flat, built by our pack `tools/testmap/rustcraft-test` | movement, rendering, redstone, entities |
| Debug | `127.0.0.1:25571` | the game's "all block states" type | rendering of every block state, one per cell |

Folders outside the repository: `%LOCALAPPDATA%\rustcraft-re\26.3\server-test` and `...\server-debug`.
Launch the client: `rustcraft 127.0.0.1 25570` (or `25571`). One server at a time.

Tests without a keyboard: RCON enabled on the test server (127.0.0.1:25580, random password
in its `server.properties`), driven by `tools/testmap/rcon.py <folder> "tp
Rustcraft x y z 0 0" "sleep 5" …`. The client walks on its own with `RUSTCRAFT_AUTOWALK=<s>` and
writes every tick (position, velocity, ground) and every teleport received to the file
`RUSTCRAFT_TRACE`. Validation: no "moved wrongly / too quickly" line in the server
log, no correction teleport in the trace.

All of this in one command: `tools/testmap/scenario.sh tools/testmap/scenarios/<name>.txt
[seconds of auto-walk]` (scenarios `effects`, `current`), summary by `tools/testmap/trace.py`.

Pitfall: the server pauses after 60 s with no player (`pause-when-empty-seconds`); RCON
checks (`execute if block …`) must therefore be done with the client connected, otherwise water and sand
look frozen.

Image comparisons (A/B): the sky and fog follow the world's time and weather;
freeze both on each server before comparing (`gamerule advance_time false`,
`gamerule advance_weather false`, `time set noon`, `weather clear`). The bench server also
has RCON (port 25581, random local password in its `server.properties`):
`python tools/testmap/rcon.py "%LOCALAPPDATA%/rustcraft-re/26.3/server" "<command>"`.

## Test pack (`tools/testmap/rustcraft-test`)

Our own content (command functions). Builds the map on first start
(score marker), freezes time and weather, turns off monster spawning and random ticks.
To rebuild: delete the `rustcraft-test` world from the test server.

Ground: grass at y = −60, you walk at y = −59, spawn point (0, −59, −8).

| Zone | Location | Contents | Checks |
|---|---|---|---|
| Friction | x 0..77, z 0..7 | ice, packed ice, blue ice, soul sand, honey, slime, dirt, soul soil; tower above the slime; bed | sliding, slowdown, reduced jump, bounces |
| Steps | x 0..44, z 12..20 | slabs, stairs, carpet, path, snow 1/4/7/8, fences, walls, fence gate, farmland, tall slabs, walkway above a pit | step-up, support, stopping at the edge when sneaking |
| Climbables | x 0..16, z 24..25 | ladder, vines, scaffolding, twisting vines, open trapdoor on a ladder | climbing, descending, hanging on |
| Slowing blocks | x 0..21, z 34..39 | cobwebs, sweet berries, powder snow | slowdown multipliers |
| Fluids | x 0..46, z 43..56 | pool with waterfall, aquarium with bubble columns (up and down), lava under glass, stepped flow | fluid rendering, current, swimming, bubbles |
| Rendering | x 0..42, z 70..77 | coloured glass, tinted glass, panes, leaves, waterlogged blocks, water against glass, grasses and ferns, doors | layers, overlap, tints, inner faces |
| Biomes | x 0..63, z 100..131 | 8 bands (plains, swamp, dark forest, desert, jungle, badlands, snowy plains, cherry grove) over leaves, water and grass | biome tints, modifiers, 5×5 blending, sky |
| Redstone | x 80..112, z −2..14 | wire shapes, repeater, comparator, lamps, observer clock with sticky piston, quasi-connectivity, BUD switch, powered rails, lever trapdoor | state rendering, moving blocks, "bug" behaviours |
| Entities | x 2..34, z 142..148 | cow, sheep, pig, chicken, zombie, skeleton, creeper, villager, horse (no AI), armor stand, boat, minecart on rail | milestone 3 (entity models) |
