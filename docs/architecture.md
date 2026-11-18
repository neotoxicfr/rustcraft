# Proposed architecture

Proposal to validate before any code. The choices marked **[to decide]** are waiting for your decision.

## 1. Principles

- **Nothing from Mojang in the repository**: no code, no resource, no name table. Everything that depends on the version (packet ids, block state list, collision shapes, properties) is **extracted at run time** from the player's installation and cached outside the repository.
- **Isolated version**: one crate per protocol version, which translates to neutral internal events. The rest of the client does not know the version number.
- **The game never blocks the frame**: rendering, logic, network and heavy computation on separate threads, exchanging through messages and snapshots.
- **No memory pauses**: no GC, allocations through pools and arenas, measured budgets.
- **Building blocks shared with the Rust singleplayer**: `rc-proto`, `rc-proto-v777`, `rc-world`, `rc-data`, `rc-light` and the collision part of `rc-physics` stay independent of rendering and symmetric (encoding and decoding both ways, already required by `rc-capture`). The singleplayer (§ 14) reuses them as they are.
- **Verifiable fidelity**: every reproduced behaviour (movement, collisions, lighting) has a test that compares it with a trace of the official client.

## 2. Crate layout (Cargo workspace)

```
crates/
  rc-app        entry point, main loop, configuration
  rc-net        transport: TCP, VarInt framing, zlib compression, AES-CFB8, connection states
  rc-proto      common types: VarInt, strings, positions, NBT, text components, generic codecs
  rc-proto-v777 26.3 packets: decoding/encoding, translation to rc-proto::Event
  rc-data       loading of the extracted facts (block states, shapes, properties, registries)
  rc-assets     jar reading + asset index + stacked resource packs, neutral fallbacks
  rc-world      chunk storage, palettes, synchronized registries, tags
  rc-light      lighting engine (sky + blocks), incremental updates
  rc-mesh       block model baking, section meshing
  rc-render     wgpu: terrain pipeline, entities, sky, particles, UI, post-processing
  rc-physics    player prediction, collisions, entity interpolation
  rc-entity     entity storage, re-coded models, animations
  rc-ui         game interface (HUD, inventory, chat, menus)
  rc-audio      positional sounds
  rc-auth       (milestone 4) Microsoft OAuth → Xbox Live → services
  rc-sim        (milestone 3 bis) world simulation for singleplayer: generation, blocks, mobs, redstone, saves
tools/
  extractor/    small Java program OF OURS, run with the launcher's JRE against the player's jar
  rc-capture    local proxy that logs the decoded traffic official client ↔ server
  rc-bench      fixed scene, measurements, comparison
docs/
```

## 3. Threads and ECS

| Thread | Role | Rate |
|---|---|---|
| Main | window, input, GPU preparation and submission | vsync or unlimited |
| Logic | local authoritative client state: player, prediction, entities, world, inventory | fixed 20 ticks/s + interpolation for display |
| Network | single-threaded tokio: socket read/write, decompression, decryption | event-driven |
| Compute pool (dynamic size, see below) | decompression and decoding of large packets, lighting, meshing, image decoding, LOD | on demand, priority queues (closest to the camera first) |
| Audio | cpal mixing | real time |

**Pool size: never fixed.** `std::thread::available_parallelism()` minus the dedicated threads (main, logic, network, audio), at least 1; `RUSTCRAFT_THREADS` to force it. The benches measure each parallelizable step on 1 thread, then in powers of 2 up to the machine's number of logical cores, and a step is only parallelized if the gain is measured.

Exchanges: bounded crossbeam channels; the logic thread publishes a **snapshot** (positions, states) on every tick, read by the renderer without locking (double buffer). Chunk sections are shared read-only through `Arc` + a version number: an obsolete mesh is dropped on receipt.

**ECS: no, no framework.** The client simulates almost nothing (the server is authoritative), holds a few hundred to a few thousand entities, and most of the data (world, meshes) does not have an entity shape. A generic ECS costs in scheduling and in unstable dependencies (bevy_ecs changes with every version). Instead: world in dedicated dense structures, entities in a generational arena in SoA (positions, velocities, boxes, type). If a need for flexible composition appears for entities, `hecs` remains a light option. **[to decide]**

## 4. Version data: the extractor

Problem: the client needs version-specific tables that are neither sent by the server nor present in the resources: block state order (a block's network id depends on it), collision and occlusion shapes, light emission and opacity, friction, entity dimensions, packet ids.

Solution:
1. On first launch for a version, `rc-app` locates `.minecraft/versions/26.3/` and the launcher's JRE.
2. It runs the official generator of the server jar (downloaded from the official URL) for the `packets`, `registries` and `blocks` reports.
3. It runs `tools/extractor` (our Java code, a few hundred lines) which loads the player's jar and serializes what the reports lack: shapes, physical and light properties, entity dimensions.
4. Everything is compiled into one compact binary file in `%APPDATA%/rustcraft/cache/26.3/`, then reloaded in a few ms (mmap).
5. Safeguard: at connection time, the list of packets decodable by `rc-proto-v777` is checked against the extracted report; a difference stops cleanly with a message.

Without an installed game: the server jar (official, downloadable) and a JRE are enough for the tables; only the textures and sounds come from the provided free pack.

The code references **no id** hard-coded. Special behaviours go through the **tags** synchronized by the server (`climbable`, ice, etc.) or through the extracted properties (friction, bounce). The few hard-coded cases (water, lava, air) are resolved by a lookup table filled from the extraction.

## 5. Versioned protocol layer

- `rc-net` handles the five states (handshake, status, login, configuration, play), compression above the threshold, encryption.
- `rc-proto-v777`: one module per state, each packet = struct + codec written by hand from the community wiki, verified by decompilation and by `rc-capture` traces. The numeric ids come from the extracted report (not hard-coded), matched by resource name.
- Output: internal neutral `enum Event` (`ChunkData`, `BlockUpdate`, `EntitySpawn`, `PlayerTeleport`…), input: `enum Command`. A 26.4 version = a new crate that reuses the unchanged codecs and only rewrites the differences.
- Data registries (biomes, dimensions, damage, enchantments…) received during the configuration phase, stored in `rc-world`; item components decoded lazily.
- The network thread only decrypts and splits frames (sequential by nature, > 1.4 GiB/s). Large packets (chunks, light) are sent **still compressed** to the pool, which decompresses and decodes them in parallel; the logic thread applies the results **in arrival order** (sequence number), so the behaviour is identical to sequential processing. Bench (1,547 real chunks, 7950X): 34.6 ms on 1 thread → 1.72 ms on 32 (×20).
- Tests: traffic files captured locally (not committed, gitignored) replayed through decoding/re-encoding byte for byte; cross-check against azalea's 26.3 branch in tests only.

## 6. World memory format

- **Circular grid** of columns indexed by `(x mod N, z mod N)`, N = 2×distance+3: O(1) access, no hash table, like the official chunk cache.
- Column = 16³ sections over the dimension's height (−64..320 → 24 sections).
- Section: **palette container** packed in bits (4 to 8 bits/entry, then direct on ⌈log₂(number of states)⌉ bits, i.e. 16 bits in 26.3 for 35,723 states; width read from the extraction, never hard-coded), in the same format as the network → receiving is almost a copy. Uniform section (all air, all stone) = one value, no array. Biomes in a 4³ palette.
- Lighting: half-byte arrays allocated only if the section is not uniform in light.
- Block states: `u16`, properties precomputed in a flat table indexed by state (opacity, shape, emission, flags) loaded from the extraction.
- Estimate at 32 chunks of distance (≈ 4,400 columns): in the order of 300–600 MB world + light, to be measured against the official game.
- Beyond the server distance: **Bobby-style disk cache** of the received chunks, used for LOD.

## 7. Meshing pipeline

1. A modification (chunk received, block changed, light updated) marks the section and its neighbours dirty.
2. Task on the pool, prioritized by camera distance and visibility: copy of an 18³ neighbourhood (section + border), lock-free meshing.
3. Block models **baked once** at resource load (pack JSON → quads with atlas UVs, rotations, cullable faces).
4. No greedy merging (it breaks per-vertex lighting and texture repetition): hidden-face elimination + ambient occlusion and smooth lighting computed per vertex, faithful to the official rendering.
5. Compressed vertex (≈ 16 B: position relative to the section, UV, sky/block light, AO, tint), separate opaque / cutout / translucent passes (translucent sorting per section, on the GPU if possible).
6. Per-section visibility graph (which faces communicate) for cave culling.
7. Upload into one **big sub-allocated GPU buffer** through a staging ring; no GPU allocation per section.

## 8. Rendering (wgpu 30)

- **Vulkan** backend by default (required by dlss_wgpu, the most advanced mesh shaders and ray query), DX12 as a fallback: each API is tried alone, in that order, and the first that offers a hardware adapter wins (`gpu_backend.rs`). DX12 compiles its shaders with FXC. Differences measured against Vulkan: `docs/dx12-rendering.md`.
- Terrain: a single set of pipelines, **multi draw indirect**; a culling compute (frustum + Hi-Z occlusion from the previous frame) fills the commands. Mesh shader variant if the extension is stable.
- Textures: texture array or bindless for the atlas + animated textures; mipmaps generated at load.
- Entities: re-coded models (geometry described in our own data), instancing, animations on the GPU.
- Sky, clouds, fog, weather, particles (compute), interface in a single batch.
- Post-processing: tonemapping, then later upscaling (DLSS / FSR), optional ray tracing (shadows, reflections).
- Far LOD (phase 5): simplified meshes per region from the disk cache.
- Shaders in WGSL, written by us.

## 9. Physics and prediction

- Exact reproduction of the local player's movement: order of operations, friction, gravity, jump, sneaking, swimming, ladders, per-axis collisions with extracted boxes, step-up.
- Sending positions like the official client (thresholds, frequency), handling server corrections (teleport + acknowledgement).
- Interpolation of remote entities over the number of ticks given by the server.
- Validation: **reference traces** recorded with the official client through `rc-capture` (inputs + sent positions), replayed on our engine, zero tolerated difference at double precision.
- The official code is read through local decompilation: we draw from it a **description in our own words** (behaviour notes in `docs/specs/`), the Rust code is written from those notes, never by line-by-line translation.

## 10. Resources

- `rc-assets` stacks: packs chosen by the player > version jar > asset index. The jar is read through a memory-mapped zip, nothing is extracted to disk.
- Fallbacks: missing texture → neutral checkerboard, missing model → neutral cube, missing sound → silence, missing language → raw key. Never a crash.

## 11. Measurement

- Integrated Tracy profiling (like the official game → direct comparisons).
- `rc-bench`: fixed test world (seed, position, time, weather frozen) on the local server, same render distance, same equivalent graphics options; readout of frames/s, frame time p50/p99 (PresentMon on both sides), resident memory, load time until the world is playable.
- Every optimization delivered with before/after on this scene.

## 12. Milestones

| # | Content | Demonstration |
|---|---|---|
| 0 | Tooling: local offline 26.3 server, extractor, data cache, capture proxy | official traffic logged and decoded, tables extracted |
| 1 | Offline connection, configuration, world reception, section wireframe, free camera | join the server and fly above the terrain in wireframe |
| 2 | Player resources, baked models, textures, full meshing, predicted movement, collisions | walk, jump, swim without server correction, compared with an official trace |
| 3 | Lighting, entities and their models, chat, inventory | lit caves, animated mobs, two-way chat |
| 3 bis | Rust singleplayer (§ 14), in steps: opening an existing save, place/break, physics; then terrain generation; then mobs, redstone, loot | open a world from the official game and play it in singleplayer without Java |
| 4 | Microsoft OAuth, online servers | connection to an online-mode server |
| 5 | Sound, full interface, LOD, upscaling, measurements against the official game | numeric comparison table |

## 13. Decisions (made on 11/10/2026)

1. **No generic ECS.** World in dedicated dense structures; entities in a home-made generational SoA arena. Hybrid: rare and heterogeneous entities (projectiles, vehicles, displays) carry a small bag of optional components next to the SoA core, without a framework.
2. **Vulkan by default, DX12 as a fallback**, chosen automatically according to the adapter's capabilities (ray query, mesh shaders, multi-draw indirect count); an option to force it in the config. Advanced features are optional paths: the base path (draw indirect + compute culling) works everywhere.
3. **Hybrid run-time extraction**: numeric ids from the official generator's reports (authoritative), shapes and properties from our Java extractor; cross-check of both at every extraction and verification at connection time. JRE detection: Store launcher (`%LOCALAPPDATA%\Packages\Microsoft.4297127D64EC6_8wekyb3d8bbwe\LocalCache\Local\runtime\`), classic launcher (`.minecraft\runtime\`), then `JAVA_HOME`.
4. **azalea: reference and test oracle only** (dev-dependency behind a test feature, never in the shipped binary). Microsoft auth rewritten by us at milestone 4, azalea-auth as a reference.
5. **Toolchain `stable-x86_64-pc-windows-msvc`**, pinned by `rust-toolchain.toml` when the repository is created. Requires Visual Studio Build Tools 2026 (C++ component + Windows SDK), absent for now.
6. **Decompilation**: reading the official game to write the specs, locally only: Vineflower 1.12.0 pinned and verified by fingerprint, the launcher's Java, output outside the repository, clean-room procedure (nothing decompiled enters the repository).
7. **Microsoft login postponed**: development offline (`online-mode=false`) until milestone 4.

## 14. Singleplayer in Rust (decided on 11/10/2026, starting after milestone 3)

- **Integrated server in the client**, in the same process, on dedicated threads. Client ↔ simulation link through in-memory channels carrying the internal events (`Event`/`Command`): no TCP, no encoding, no compression. Above this boundary the client keeps exactly the same code as against a remote server.
- **No hostable server** for now; `rc-sim` depends on no graphics layer, which leaves the door open.
- **Official data read at run time** from the player's jar: noises and density functions, biomes, features, structures, loot, recipes, tags (thousands of JSON/NBT files). A Rust interpreter of this data reproduces the terrain of a given seed without copying anything.
- **Code to re-write** (described in our own words in `docs/specs/` before implementation): block behaviours, redstone, mobs and their AI, procedural features, tick handling.
- **Saves** in the official game's world format (Anvil regions, NBT): a world opened by the Rust singleplayer stays openable by the official game and vice versa.
- **Measured fidelity**: same seed → block-by-block comparison with the world generated by the official server; behaviours compared through traces.
- **Multi-core**: generation and lighting by regions on the pool, ticks of independent regions in parallel when the rules allow it, without changing the result.
- Meanwhile: the official server launched locally serves as a provisional singleplayer (already in place for development).

## 15. CPU / GPU split

Principle taken from MTGScan: **GPU first when it wins, automatic and silent fallback to the CPU**, chosen at run time according to the detected capabilities, `RUSTCRAFT_CPU=1` variable to force the CPU paths, and every dual-path step measured on the bench on both sides.

Technology: **wgpu (Vulkan/DX12), WGSL compute shaders**, not ONNX Runtime or TensorRT. The game does no neural-network inference: its heavy computations are geometric and massively parallel, which compute shaders do better and without an extra dependency. ONNX Runtime + TensorRT-RTX through the Windows ML catalog (MTGScan's stack) remains the route if a neural function not covered by DLSS is added later (DLSS already goes through the NVIDIA driver).

| Step | Where | Why |
|---|---|---|
| Network, decryption, decompression | CPU | sequential stream, latency; already > 1.4 GiB/s per core |
| Chunk decoding | CPU (pool) | variable branches and palettes, a GPU round trip costs more than the computation |
| Physics, prediction, collisions | CPU | must be bit-for-bit identical to the official game (double precision, order of operations) |
| Lighting (propagation) | CPU (pool) | irregular propagation queue; GPU to evaluate for the massive relight when chunks arrive |
| Section meshing | CPU (pool), GPU to evaluate | arbitrary block models; a GPU mesher for full cubes will be compared on the bench |
| Culling (frustum, occlusion), translucent sorting | GPU | tens of thousands of sections per frame, perfect parallelism |
| Particles, entity animation, animated textures | GPU | parallel, result only displayed |
| Far LOD (simplification) | GPU (compute), CPU as a fallback | large volumes, result only displayed |
| Upscaling, frame generation | GPU (DLSS/FSR) | dedicated hardware |
| Singleplayer simulation (`rc-sim`) | CPU (pool) | exact game rules; terrain generation: noises evaluated on SIMD CPU, GPU to evaluate with identical result |

Rule: a GPU path is only adopted if it produces **exactly** the same result as the CPU path when that result influences the game (not just the display), and if it wins on the bench.
