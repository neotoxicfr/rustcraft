# Performance measurement log

Bench: Ryzen 9 7950X (16 cores, 32 logical threads), RTX 5080, Windows 11, Rust 1.99 msvc,
criterion. Real data = capture of the official 26.3 client (1,547 chunks) and `data.bin` cache.
Rule: nothing is ruled out off-hand; a variant that loses is noted here with its figure so it
can be re-measured later (other hardware, other load). Commit hashes cited below belong to the
development history before the first public release, which is not published.

## Adopted

| Building block | Before | After | Gain | Variant |
|---|---|---|---|---|
| Bidirectional text, 60-character line (G1-F13, micro-bench `bench_visual`, loaded machine) | — (no reordering) | Latin: 25 ns (fast path, no allocation); Arabic/Hebrew: 3.9 to 9 µs without cache, 32-37 ns with the interface's per-line cache | ×110 on a redrawn right-to-left line | cache (text, direction) → visual order, forgotten after 64 frames without drawing (`RUSTCRAFT_OPT_NOBIDICACHE=1` disables it). Multi-core and GPU not applicable: a few lines per frame, cost < 2 µs in total once cached. Scene bench (ar_sa language, full chat) to do on a quiet machine |
| Tick of an empty campfire (G6-F01, micro-bench `bench_campfire_empty_tick`, 256 campfires × 2,000 ticks, loaded machine) | 808 ns (block entity re-read) | 20.8 ns | ×39 | "food" / "cooking started" flags kept with the ticker; tick skipped without re-reading the NBT (identical result: a tick with nothing to cook or cool down does nothing; flags re-read at every write of the block entity, `/data`, `/setblock`, `/clone`, `/item` commands included, test `campfire_food_written_by_command_cooks`) |
| CFB8 decryption, 1 core | 58 MiB/s | 943 MiB/s | ×16 | AES blocks of a batch of 64 computed together |
| CFB8 decryption, ≥ 128 KiB | 133 µs (128 KiB) | 26 µs | ×5 | 4 KiB pieces on the pool; 1 MiB ×8, 16 MiB ×11 (10.7 GiB/s) |
| Frame decoding (decryption + decompression) | 286 MiB/s | 1.42 GiB/s | ×5 | libdeflate (known size) + AES batches |
| Frame encoding | 102 MiB/s | 180 MiB/s | ×1.8 | libdeflate |
| CFB8 encryption, 1 core (C5-M39, loaded machine) | 49.7 MiB/s | 56.2 MiB/s | +13 % | register kept in an SSE2 vector register (one 16-byte write of the AES input); `RUSTCRAFT_OPT_CFB8_VECTOR_REGISTER=0` restores the old path |
| Frame of a packet below the compression threshold (C5-M39, loaded machine) | 4,149 MiB/s | 6,432 MiB/s | +55 % | written straight into the output instead of through `scratch`; `RUSTCRAFT_OPT_FRAME_COMPRESS_IN_PLACE=0` restores the old path |
| Decoding of `data.bin` | 15.0 ms | 1.16 ms | ×13 | error messages built lazily |
| Decoding of `data.bin` | 1.10 ms | 0.59 ms | ×1.9 | 4 sections in parallel + states in batches of 4,096 |
| Preparation at 1st launch | 18.1 s | 4.6 s | ×3.9 | client jar installed (no download), generator and extractor simultaneous |
| Reading a full section | 4.7 µs | 1.7 µs | ×2.7 | grouped decompression instead of one access per block |
| Decoding of 1,547 chunks, 1 core | 9.3 ms | 6.0 ms | ×1.6 | mimalloc allocator |
| Decoding of 1,547 chunks | 6.0 ms (1 t) | 0.47 ms (32 t) | ×12.8 | independent chunks on the pool |
| Decompression + decoding of 1,547 chunks | 34.6 ms (1 t) | 1.72 ms (32 t) | ×20 | heavy packets sent compressed to the pool |
| Wireframe meshing (historical: code removed on 22/10/2026), 1,248 real columns (28.8 M faces) | 851 ms (1 t) | 543 ms | ×1.6 | one 8-byte entry per face instead of 4 vertices (corners rebuilt by the shader) |
| Wireframe meshing (historical: code removed on 22/10/2026), same scene | 544 ms (1 t) | 171 ms | ×3.2 | visible faces computed in rows of 16 blocks (bit operations); identical face by face to the reference |
| Wireframe meshing (historical: code removed on 22/10/2026), same scene, all cores | 57 ms | 10.3 ms | ×5.5 | combination of the previous two |
| Faces drawn (spawn view) | 24.98 M | 10.62 M | ×2.4 | columns outside the field of view discarded on the CPU (336 drawn out of 1,057) |
| Entering the game (warm cache) | ≈ 330 ms (289–363) | ≈ 210 ms (194–245) | ×1.6 | base data pack announced as known, like the official client: registries without content (9.7 KiB instead of 154 KiB) |
| Textured meshing, 1,248 real columns | 1.528 s (1 t), 89.4 ms (all) | 0.890 s, 56.8 ms | ×1.72 / ×1.57 | buried blocks discarded before the variant draw (removal directions per state); identical column by column to the reference |
| Textured meshing, same scene | 0.890 s (1 t), 56.8 ms (all) | 0.591 s, 36.7 ms | ×1.51 | "buried block" test on an 18³ grid of covering faces per section instead of 6 accesses per column; breakdown measured before: walk 21 ms, fluids ≈ 190 ms, burial test ≈ 534 ms, visible blocks ≈ 143 ms |
| Textured meshing, same scene | 0.591 s (1 t), 36.7 ms (all) | 0.563 s, 34.7 ms | ×1.05 | border grid in states (fluids and face removal read from it too); remaining: fluids 150 ms, biome blend 32 ms, writing the ≈ 25 M visible faces |
| Textured meshing, same scene | 0.563 s (1 t), 34.7 ms (all) | 0.457 s, 28.1 ms | ×1.23 | guided by samply profile: grid border by vertical stacks (0.534), biome blend by weighted 4×4 cells (0.505), "hidden" mask per block read by each face (0.457); cumulative since 1.53 s: ×3.35 |
| Faces drawn (spawn view, bench) | 1.33 M, GPU 0.68 ms | 0.83 M, GPU 0.43 ms | −38% / −37% | occlusion sorting of the sections (communication graph + walk from the camera, game rule) |
| Visibility walk, 1,248 real columns | 6.4 ms (hashing) | 2.4 ms (dense grid); 0.19 ms on the render thread | ×2.7 then off the render thread | dense grid reused; redone only if camera or columns change; as a background task like the game |
| Face communication, 13,768 real sections | 63 ms (cell-by-cell fill, 1 t) | 28.9 ms (dilation of 64-bit masks, 1 t); 1.9 ms all cores | ×2.2 | identical section by section; full textured meshing 0.544 → 0.513 s single-threaded |
| CPU per frame (bench, spawn view) | 0.84–0.99 ms | 0.57–0.59 ms | −38% | buffers per region of 16 × 16 columns + grouped indirect draws (instead of one binding and one buffer per column); pixel-identical image |
| CPU per frame, still camera (test map, A/B 3 passes) | 0.209–0.213 ms, 3,035 fps | 0.100–0.102 ms, 4,591 fps | −52% | draw commands kept as long as eye, angles, walk, columns and frame size are unchanged (identical by construction); moving camera: unchanged; 0 pixel difference |
| CPU per frame (test map, A/B 3 passes) | 0.228–0.230 ms, 2,877 fps | 0.211–0.214 ms, 3,021 fps | −7% | column's box tested first: out of view, its sections are discarded; entirely in view, all kept without a per-section test; 0 pixel difference |
| CPU per frame (bench, A/B 3 passes) | 0.364–0.392 ms, 231 batches of which 210 translucent | 0.308–0.322 ms, 22 batches | −17% | translucent quads of all columns in a single global buffer (absolute column in the quad): the translucent pass, sorted from the farthest to the nearest, only has one binding and one batch instead of a region change per column; GPU unchanged; test map (glass in front of water): 0 pixel difference |
| GPU per frame (bench, spawn view, A/B 3 passes) | 0.45 ms, 0.84 M faces submitted; CPU 0.31 ms | 0.355 ms, 0.66 M; CPU 0.37 ms | −21% GPU, limited frame 0.45 → 0.37 ms | faces sorted by visible direction (6 axes + unaligned) then by section; groups necessarily facing away not submitted (±x, ±z per column, ±y per section, 2-block margin); CPU +0.05 ms (≈ 4× more indirect commands); screenshots within noise (200 px against 144 old/old); CPU alone: not applicable, GPU alone: that is the gain |
| CPU per frame (bench, spawn view, A/B 3 passes) | 0.415–0.450 ms | 0.306–0.329 ms | −27% | reached sections sorted by column in the background task, series built in one pass in a reused buffer (no more `HashMap<column, Vec<bool>>` nor clone of the translucent starts per column and per pass); at equal distance, fixed column order; screenshots: new/old difference (186 px) of the same order as old/old (144 px, animation phase) |
| Rust heap (bench, spawn view) | 134.5 MB | 124.9 MB | −7% | texture images freed after upload to the GPU (the mesher only reads the baked faces) |
| Rust heap (bench, spawn view) | 124.9 MB; working set 466 MB, private 1,350 MB | 78.4 MB; 442 MB, 1,303 MB | −37% of the heap | resource jar (46.5 MB in memory) freed as soon as the baking and the colors are done; readings per step (`memstats`): data 2.7, jar index +46.5, baking +27.3, tables +7.9 MB |
| Rust heap (bench, test map, 12/10) | 167.6 MB (regression: jar kept alive by audio and on-demand entity textures, unifont read twice) | 95.4 MB | −43% | jar read from disk through positioned reads (central index only in memory): −40 MB; interface atlas pixels freed after upload (−12.5); block textures of the entities read on demand (−2.9); unihex table shared between `default` and `uniform` (−11.6) then compact (sorted array, `u16` rows, glyph rebuilt on read); entity textures read at first use (−1). Steps (`memstats`): baking 39.2, mesher/physics tables +7.6, fonts +15.5 → ≈ +4, items +18.5, entities +9.4, interface +8.1 MB |
| Private memory (bench) | (measurement) read every 0.4 s at startup | 300 MB before the window, ≈1,000 MB as soon as the device is created, 1,300 MB steady state; working set 443 MB | driver commitment (identical in Vulkan and DX12, Rust heap ≈ 80 MB): outside our heap, no lever on the client side; the working set is the figure that matters |
| Block update (network packet), 1 core | 21.6 µs (12 states), 36.8 µs (50), 8.9 µs (direct) | 6.7 ns, 13 ns, 4.8 ns | ×1,800–3,200 | in-place write like the game (existing palette index, added entry, repacking only if the palette overflows); identical block by block on 3,200 random changes; GPU not applicable (one write) |
| Decoding of `data.bin` with mimalloc (production) | 0.60 ms (system allocator) | 0.37 ms | ×1.6 | production allocator in the bench; 1 thread 0.51 ms |
| Textured meshing, 1 core (bench, 1,248 columns) | 1.218 s | 1.195 s | −1.9% | water tint computed at the first emitted face (buried water blocks draw none); biome blend without allocation; fingerprint unchanged; profile: smooth lighting makes up ≈ 55% of the meshing (light cache 29%, `quad_light` 9%) |
| Cost of the entities (test map, 200 still cows in view, A/B 3 passes, `RUSTCRAFT_NOENT`) | vertices transformed on the CPU (cubes × matrices): frame 0.23 → 0.85 ms (4,300 → 1,170 fps) | cubes of all layers baked once in a static buffer; per frame, part matrices (storage) and one instance per entity layer, draws grouped by layer; pose buffers reused; animated parts designated by precomputed indices (no more SipHash on names); layers resolved by type id: frame 0.23 → 0.32 ms (3,090 fps) | −85% of the overhead (3.1 → 0.45 µs per cow) | GPU +0.006 ms; remaining: poses and part matrices on one core |
| Ground shadows of the entities (200 cows, A/B 3 passes, shadows hidden / drawn) | 6 vertices of 24 B per tile, collision shape read per state, radius per string, texture looked up by name: frame +0.05 ms | "full cube" table per state precomputed, radius per kind cached, one 28 B instance per tile (corners and UV computed in the shader): frame +0.025 ms | −50% | rendering identical to the screenshot; field test fixed (box visible, not entirely included) |
| Crosshair targeting per frame (200 cows, A/B 3 passes) | kind tested (strings) before the distance: +0.002 to +0.036 ms (noise) | distance first, kind next: +0.004 ms | — | block walk: a few blocks; outline edges cached per shape |
| Name tags (200 named cows visible, A/B 3 passes) | text reshaped every frame (two passes), 6 vertices of 44 B per glyph placed on the CPU (≈ 880 KB sent per frame): frame 0.36 → 0.75 ms | one 60 B instance per glyph, camera-facing corners computed in the shader; shaping kept per entity as long as the name and the posture do not change (except obfuscated text): 0.36 → 0.41 ms | −87% of the overhead | rendering identical to the screenshot |
| Entity metadata (200 cows, A/B 3 passes; samply profile) | `HashMap<u8, Meta>` (SipHash) read per entity and per frame (flags, pose, baby, variant, dimensions): ≈ 7% of the process CPU, frame +0.125 ms | array indexed by the index (`MetaMap`): hashing 1.4 s → 0.15 s over 20 s of profile, frame +0.105 ms | −16% of the overhead | |
| Validation of wgpu's indirect draws (`InstanceFlags::VALIDATION_INDIRECT_CALL`, active by default in release) | CPU cost per draw + GPU compute pass: encoding 0.115 ms, frame 0.344 ms (test map, 200 cows) | disabled (arguments built on the CPU, bounds known; `WGPU_VALIDATION_INDIRECT_CALL=1` restores it): encoding 0.098 ms, frame 0.326 ms | −15% of encoding | bench map (spawn view, 3 passes): encoding 0.150 → 0.108 ms (−28%), 2,338 → 2,378 fps; GPU unchanged (0.193 ms) |
| Real VRAM (Vulkan allocator report, test map) | home-made readout: 51 MB (terrain alone); allocator: 98.6 MB allocated, 256 MB reserved (blocks of 128 to 256 MB, `MemoryHints::Performance`) | `MemoryHints::MemoryUsage` (blocks of 8 to 64 MB): 156 MB reserved, frames per second identical within 1%; 1-page glyph sheet doubled on demand instead of 32 pages (8 MB) from the start: 90.7 MB allocated | −100 MB reserved, −7.9 MB allocated | remaining: entity textures 17 MB (full sheet), blocks 14 MB, interface and item sprites 4 + 4 MB, depth 6.25 MB |
| Entity textures (VRAM, test map, 9 kinds of mobs + armor + items) | 2,477 textures (743 from the game + item sprites) decoded at startup and packed into a 2048-wide sheet: 17 MB | PNGs kept compressed, decoded and placed at first use in a 1024-wide sheet that doubles in height (GPU copy); rectangles in pixels normalized in the shader: 74.5 MB allocated in total (90.7 before) | −16 MB | decoding at first use: ≈ 50 µs per 64 × 64 texture; shadow edge clamped to the texel centre (the neighbouring texture is no longer read) |
| Armed zombies (100: dyed leather, iron, diamond, sword; A/B 3 passes, old binary in a worktree) | per piece and per frame: `format!` ×2, definitions cloned, pose re-animated per texture layer; held item: stack and name cloned, new vectors, `norm()` + `model()` allocated; part matrices: 3 rotations + 5 4 × 4 products, normal by 3 × 3 inverse: +0.349 ms | pieces resolved once (arena), body pose copied per known part (like the game's property copy), one animation per piece; no allocation per frame; composed matrix (Euler ZYX quaternion, zero angles skipped, polynomial sin/cos < 1e-6); normal by cofactors + determinant sign: +0.186 ms | −47% | 200 cows: +0.105 → +0.093 ms; equivalence tested (2,000 poses, 1e-5) |
| Cost of the HUD with items in slots (bench, 9 items including a sword with a glint, A/B 3 passes ×2, `RUSTCRAFT_NOHUD`) | evaluation of the 9 stacks every frame, glint slot redrawn every frame, vertex buffer recreated at every pass: CPU +0.027 ms | evaluation kept per slot as long as stack, selection and tick are unchanged, animated slots redrawn at most every 4 ms, buffer reused: CPU +0.012 ms (median), GPU +0.004 ms | −55% of the overhead | 4 ms: invisible even at 240 Hz (glint scroll over 27.5 s); remaining: building the GUI vertices every frame |
| Cost of the HUD (bench, survival, A/B 3 passes HUD hidden / shown, `RUSTCRAFT_NOHUD`) | heart names formatted and level text flattened every frame: CPU +0.007 ms | names in static tables, level text kept as long as the level does not change: CPU +0.004 ms, GPU +0.004 ms (2,400 → 2,365 fps) | −40% of the CPU overhead | remaining: ≈ 45 quads, 3 pipeline changes (vignette, inversion, normal); full-screen vignette skipped when `b < 1/510` (no byte moves, the case of full daylight after ≈ 31 s of smoothing) |
| VRAM in game (Vulkan allocator, test map, `RUSTCRAFT_VRAMDUMP`, loaded machine: memory does not depend on the load) | title screen panorama (6 faces) kept in game: 24 MB; interface sprite atlas 2048 high (height rounded to the power of 2): 16 MB; total 106.3 MB allocated | panorama freed as soon as the game is entered, reloaded from the resource cache when returning to a screen outside the world; atlas high to the nearest multiple of 4: 11 MB; total 78.5 MB | −27.8 MB (−26%) | reserved unchanged (156 MB, blocks kept by the allocator); panorama seen after a kick; Rust heap 97 MB, working set ≈ 400 MB, private ≈ 830 MB (driver included) |
| Rust heap in game (`memstats` binary, test map, warm resource cache; the 97 MB above came from a cold launch, which keeps the encoded sections until the cache is written) | images of the item sprites (first frames, animations, glint) kept in the item atlas after upload to the GPU: 78.4 MB | freed at the creation of the atlas (the baking is finished at loading, nothing re-reads them): 76.4 MB | −2 MB | warm breakdown (`memstats` steps): baked faces 7.3 MB (80,011 × 96 B), item quads 10.3 MB (111,748 × 92 B, mostly the extruded edges of flat items: lead, store them as pixels and unfold them on use), world 7.3 MB; startup peak 100 MB (panorama 24 MB and interface atlas 11 MB on the CPU side, freed at the creation of the renderer) |
| Integrated server tick (bench `tools/bench/scenes.py`, reference = development version before any optimisation, 3 alternating passes, non-quiet machine: another agent was compiling) | median 15.0 / 14.4 / 20.1 / 18.7 / 17.9 / 17.6 ms (plains12, crowd, redstone, flight, plains32, cave); p99 24.8 / 20.2 / 40.7 / 29.3 / 25.1 / 45.1 | median 1.35 / 1.62 / 5.20 / 4.02 / 2.43 / 2.96 ms; p99 7.2 / 4.8 / 17.1 / 10.5 / 6.6 / 20.6 | ×4 to ×11 (median) | world generation settings (`world_gen_settings.dat`) read once at opening (and at `/reload`) instead of at every question about the dimension type (open, inflate, NBT several times per chunk and per tick); simulation fingerprint identical (`tests_opt`) |
| Hash tables of all the crates (same bench, after the previous line) | median tick 1.35 / 1.62 / 5.20 / 4.02 / 2.43 / 2.96 ms; main CPU plains32 0.135 ms/frame | median tick 0.94 / 1.19 / 4.81 / 3.13 / 1.58 / 2.41 ms; main CPU plains32 0.124 ms/frame | tick −7 to −35%, main −8% (plains32) | `hashbrown` + foldhash instead of SipHash (the standard library's `HashMap`/`HashSet`); random iteration order per table as before (no order leaks out: `tests_opt` fingerprint identical with different hash seeds) |
| Integrated server tick, constant data re-read (wave OPT1B, 19/10; `rc-sim` bench `tests_opt::bench_ticks`: model world, 5 sites, 400 ticks measured per site, 3 alternating passes base / A / A+C, quiet machine; median of the passes) | median tick (1 thread) plains 0.646, forest 0.476, village 2.531, ocean 0.553, Nether 1.635 ms; total 2,000 ticks 3.88–4.00 s | A alone: 0.618 / 0.451 / 2.531 / 0.522 / 1.421 ms, total 3.75–3.87 s; A+C: 0.528 / 0.438 / 2.491 / 0.500 / 0.866 ms, total 3.43–3.52 s | A+C: median −19% plains, −8% forest, −2% village, −10% ocean, −47% Nether; total −11% | A: `DataPack::get` without `format!` or allocated key (borrowed key), trial generator settings converted JSON → NBT once (`get_nbt`) instead of every tick, fluid type resolved by table (no more name compared nor `Arc` view copied per call in `fluid_of`, `mob_fluid`, `mob_source_water`), weather without copying the dimension name; C: spawn lists (biome, structure) kept by the pack instead of a deep copy of the JSON at every attempt, structure pieces read in place (no more `Vec` of boxes per attempt, structure without replacement never read), biome name borrowed (`biome_str`) in the hot paths, structure references of the predicate read without copy. 1 thread = 32 threads within noise (sequential tick, the light does not use the pool at these sizes); GPU not applicable (game decisions in series, isolated accesses). `tests_opt` fingerprints unchanged (`6a557b6a…`, `9abd8cbe…`). Scene bench (`scenes.py`, label `opt1b`, 2 alternating passes old code (before any optimisation) / main 1cf4e91 / this series, true release): median tick plains12 15.33 / 0.867 / 0.786 ms, crowd 14.37 / 1.103 / 1.036, Nether 14.22 / 0.927 / 0.869, village 16.97 / 2.558 / 2.406 (−6 to −9% against main); redstone 4.684 → 4.760 and flight 3.070 → 3.158 within the dispersion of the passes (4.55–4.97; 2.93–3.39); main CPU per frame, GPU, private memory and loading unchanged within noise |
| Generation noise kernel (C1-M02, 22/10; micro-benches `rc-worldgen --test noise_opt -- --ignored`, trial profile, LOADED machine: other agents were compiling; scene bench flight / generation to do, quiet machine) | `floor` through a `ucrtbase` call; work in y (mul, wrap, floor, smoothing, spreading) redone for each column; masked 256 permutation, i8 gradients converted at every read; separate `sin` and `cos`. `final_density_many` 256 chunks: 1 thread 2.62 / 2.57 ms per chunk, 32 threads 0.187 / 0.202; volume 16 × 384 × 16 at 8 octaves: 9.26 ms (y scale 1), 6.16 ms (y 0); 10⁶ `get`: 20.9 ms; 10⁷ floor: 13.7 ms; 10⁶ sin+cos: 697 ms | Java-style `floor` without a library (`RUSTCRAFT_OPT_FLOOR_JAVA`, 1.5·2⁵² trick under 2³¹); y precomputed once per octave (`NOISE_YPRE`); column with all-equal y inputs computed once (`NOISE_COLCONST`); 512 table and f32 gradients (`PERM512`); runs of the same cell in a vectorized branch-free loop (`NOISE_SIMD`), same body compiled with AVX2 by detection (`NOISE_AVX2`); `sincos` with shared reduction (`SINCOS`). `final_density_many` 1 thread 1.37 / 1.47 ms (without AVX2 1.54 / 1.42), 32 threads 0.169 / 0.166; volume y 1: 3.06 ms (AVX2 3.09), y 0: 0.116 ms (AVX2 0.091); `get` 16.9 ms; floor 6.4 ms (but 32.9 ms when 28% of the values are outside ±2³¹, a slow path the generator never reaches; after moving the slow path to saturation, 40.8 ms against 19.5 libm, loaded machine: the mispredicted branch costs, not the computation); sincos 362 ms | 1 thread −45%, 32 threads −10%; volume −67% (y 1), −98% (y 0); `get` −19%; floor −53%; sincos −48% | Alone: floor_java −2% of the volume (−7% at y 0), perm512 −1% of the volume and −21% of `get`, ypre −40% (y 1) / −56% (y 0), +colconst −98% at y 0, +simd −54% (y 1). AVX2 within noise (no proven gain on y 1, −20% on y 0): kept by default, to settle on the scene bench. FMA not used (would change the rounding; LLVM does not contract by itself). Octaves with zero amplitude: none in the routers (inventory `router_noise_inventory`: surface 176 layers, Nether 48, End 40), nothing to skip. Noise nodes with a zero y scale: 3 on the surface, 2 in the Nether, 0 in the End. GPU ruled out without a bench: `wrap` in f64 (SHADER_F64 not guaranteed) and FMA contraction not forbidden in WGSL, hence bits not guaranteed. Outside ±2³¹ and for NaN, the floor converts with saturation like game 26.3 (`MIN` below −2³¹ and for −∞, `MAX` above, 0 for NaN): identical to the old path everywhere. Switch dependencies: `NOISE_COLCONST`, `NOISE_SIMD` and `NOISE_AVX2` only act with `NOISE_YPRE`, and `NOISE_AVX2` only with `NOISE_SIMD`; `NOISE_YPRE=0` restores the whole old path. `sincos` without a caller in this mission (conversion of the callers: C1-M13). Memory: 4 y buffers per thread (≤ 600 values) |
| Ticks and marks placed by the decoration (fidelity G6-F18, 22/10; `rc-worldgen` bench `decor_perf`: 49 decorated chunks, seed 1, best of 5 passes, 3 alternating launches new / old / new, loaded machine) | decoration 34.6 / 57.2 ms; post-processing of 25 chunks 0.031 / 0.037 ms (1,022 marks, 0 ticks) | decoration 35.3 / 54.5 / 35.6 ms; post-processing 0.029–0.036 ms (1,251 marks, 8 fluid ticks) | none measurable (launch dispersion ±30%) | ticks kept in the chunk (pushed `Vec`, 16 B per tick), deduplicated once at save time (hash table, no search at every placement: a monument places thousands of waters); marking above = 2 reads at most; post-processing shape update: allocation-free exit for blocks not concerned (the name is no longer copied); multi-core / GPU not applicable (serial placements, a few dozen per chunk); scene bench to do on a quiet machine |

### Partial re-meshing and visibility, re-check (wave OPT4B, 19/10/2026)

Adversarial review of `d7e7462` (per-section re-meshing) then leads measured alone and
combined. Final bench (`target/perf/final2`): `tools/bench/scenes.py`, LTO binaries, 2 alternating
passes, ref. = `d838d52` (`rustcraft-ref.exe`), v1 = `main` after `d7e7462` (`rustcraft-v1.exe`),
OPT4B = this wave with a 32 MB base cache (64 MB kept afterwards, "cache" line further
down); no other agent during this bench (others were compiling during the intermediate benches:
only their net differences are kept). Medians [min–max when useful], ref. / v1 / OPT4B.

| Measurement | Ref. | v1 | OPT4B | Cause |
|---|---|---|---|---|
| Meshed load plains32 (ms) | 1,431 [1,244–1,618] | 2,202 [2,168–2,236] | 1,496 [1,484–1,508] | integrated server regions read in parallel again (below), pool queue 2 → 4 tasks per thread, columns meshed once |
| Entering the game plains32 (ms) | 819 [608–1,030] | 1,328 | 986 | regions in parallel |
| Meshed load plains12 / building / flight / redstone / cave (ms) | 1,492 / 1,360 / 1,346 / 1,174 / 1,402 | 1,354 / 1,404 / 1,402 / 1,346 / 1,397 | 1,632 [988–2,276] / 1,354 / 1,356 / 1,356 / 1,376 | limited by delivery (last column + 0.5 s of stability); plains12: one launch with a late first frame (1.7 s), also seen with the ref. |
| GPU ms/frame plains32 / plains12 / redstone / flight / cave | 0.331 / 0.105 / 0.183 / 0.097 / 0.206 | 0.327 / 0.105 / 0.184 / 0.098 / 0.207 | 0.310 / 0.092 / 0.152 / 0.085 / 0.181 | outer ring no longer drawn (game rule) |
| VRAM plains32 / plains12 / cave (MB) | 820 / 206 / 313 | 824 / 208 / 316 | 725 / 171 / 279 | same |
| Private memory plains32 / plains12 / building / flight / cave (MB) | 3,048 / 1,888 / 1,886 / 2,528 / 2,400 | 2,718 / 1,500 / 1,554 / 2,426 / 2,062 | 2,386 / 1,336 / 1,458 / 2,300 / 2,028 | base cache 128 → 32 MB, fewer re-meshes |
| Working set plains32 / building / cave (MB) | 1,066 / 557 / 694 | 1,264 / 688 / 898 | 1,033 / 625 / 814 | same |
| Flight: sections meshed/s, pool ms/s, process CPU ms/s | 12,571, 1,150, 5,259 | 5,273, 550, 4,104 | 2,657, 270, 3,887 | new columns meshed once (neighbours all there) |
| Cave: sections meshed/s, pool ms/s | 6,250, 1,031 | 473, 240 | 2,824, 508 | 32 MB cache too small for the caves (112 re-meshes without a base per s): 64 MB kept |
| Frame p99 plains32 / flight / redstone / cave (ms) | 1.73 / 1.22 / 0.81 / 0.87 | 0.79 / 1.06 / 0.61 / 0.80 | 0.81 / 0.85 / 0.55 / 0.75 | fewer re-meshes, pool below normal priority |
| Light relight ms/s redstone / cave (client + server) | 140 / 66 | 134 / 68 | 147 / 67 | relight accumulated per section: no measurable gain (ref. dispersion 131–149) |

Entering the game: between event 13 and the first chunk, the integrated server loaded the
3,461 chunks of plains32 in 145 ms before `a79e709`, in 425 to 500 ms after (temporary log of the
loading). The chunks are grouped by region in a hash table then read "in
parallel"; the 8-region `hashbrown` table fits in a single group of cells, which its
parallel iterator does not split: the regions were read one after the other. List first
(`rc-sim/src/world.rs`, loading and raw reading): 139 ms, regions all at the same time. No
other parallel iteration over a `hashbrown` table in the repository.

Leads (one line per lead, each alone then in the kept combination):

| Lead | 1 core | All cores | GPU | Decision |
|---|---|---|---|---|
| Game rule: a never-meshed column waits for its 8 neighbours (`remesh.rs`, waiting columns kept out of the list walked every frame) | a new column meshed 1 time instead of 1 to 9 | flight −50% of sections meshed, −52% of pool time | not applicable | kept (parity + gain) |
| Whole column to redo launched even if a partial re-mesh is in flight | — | plains32: no net difference alone | not applicable | kept (no base needed, safe) |
| Pool queue: 2 → 4 tasks per thread (`RUSTCRAFT_MESHQUEUE`) | — | plains32: end of meshing after the last column 1,000 → 538 ms (6: 520, 12: 514); plains12, flight: unchanged | not applicable | 4 (6 and 12 bring nothing more; each queued task retains its 9 columns, copied if the main thread modifies them) |
| Pool threads below normal priority (`RUSTCRAFT_POOLPRIO=normal` to compare) | — | plains32: loading 2,516 → 2,330 ms; flight: tick p99 16.5 → 11.1 ms; frame p99 identical | not applicable | kept |
| Base cache 128 / 64 / 32 / 0 MB | — | plains32: private 2,588 / 2,499 / 2,360 / 2,379 MB, without base 0.2 / 0.6 / 3.1 / 38 per s; cave: private 1,992 / 1,980 / 2,036, pool 278 / 370 / 497 ms/s; redstone (128 / 32 / 0): pool 188 / 187 / 486 ms/s; identical images | not applicable | 64 MB: plains32 −89 MB of private against 128, cave +92 ms/s of pool; 32 MB wins back 228 MB but nearly doubles the pool of the caves (no effect on the images) |
| Light differences of a packet compared by 64-bit words instead of cell by cell | 696 ns against 8,121 ns per different sheet (×11.7); equal sheets: memory comparison first | ≤ 50 sheets per packet: 35 µs, nothing to distribute | not applicable (memory access) | kept |
| "Sealed" sections (only covering cubes, covering border): per-block loop skipped | textured meshing 1,248 columns 2.182 → 2.153 s (−1.3%), other series −4.5% | same gain per column (independent) | not applicable | kept (fingerprint `b049d4a72f419192` identical) |
| Mesher work buffers kept per thread (unfolded sections 16 KiB, 18³ grid) | 2.300 → 2.150 s then 2.245 → 2.190 s (−2.5 to −6.5%) | 124.2 → 119.5 ms then 121.7 → 121.9 ms (0 to −3.7%) | not applicable | kept (≤ 0.5 MiB per pool thread) |
| Relight: edges of the changed cells accumulated per section before entering the table (one entry per cell since `d7e7462`) | fewer table entries per recalculation (client and integrated server) | — | not applicable | kept (same table returned, `rc-light` tests); final bench redstone 134 [125–144] → 147 [146–147] ms/s, cave 68 → 67: no measurable gain, to re-measure with a relight-only bench |
| Copy of the communications for the walk kept as long as they do not change | one table of 3,461 entries fewer per relaunched walk (camera changing 8-block cell) | — | walk sequential by nature (`perf.md`, below) | kept |
| Textured meshing, scaling (bench `RC_BENCH_SCALE=1`) | 2.157 s | 2 t 1.088, 4 t 0.559, 8 t 0.285, 16 t 0.151, 32 t 0.120 s (×18) | not done: arbitrary models, smooth lighting, tints and fluids, identical result not guaranteed | all-core pool (unchanged) |
| Partial re-mesh of a section (base = full meshing) | 0.29 ms per column against 1.73 (×6) | independent columns on the pool | not applicable | (measurement of the v1 building block); remaining: sorting by group of the whole column, lead |
| Section granularity instead of column in the pool | not done: a whole column is now only launched at loading (1 time per column), where the pool is saturated; a partial re-mesh already only does 1 to 3 sections | — | — | noted, to re-measure if the block placement → frame latency requires it |
| Large sub-allocated GPU buffer (Sodium idea) | already done: regions of 16 × 16 columns, one face buffer and one fluid buffer per region, allocated ranges (`alloc_in`) | — | — | nothing to do |
| Less frequent translucent sorting | the game re-sorts a section when the viewpoint (camera direction clamped to −1..1 per axis) changes, or at every block if it is near or on the axis; we re-sort per column at every block (near ones immediately, the others in rotation); measured cost 0.05 ms per frame in flight | — | — | done (I-06, 24/10): `RUSTCRAFT_OPT_TRANSPOV` (game rule per section, `docs/specs/visibility.md`) and `RUSTCRAFT_OPT_TRANSKEY` (distances computed once); figures below, decision on the scene bench |

Differences found by the review (fixed, tests):
- ambient occlusion reads the state two cells along the normal (opacity of the corners behind the
  neighbouring cell): a block changed one cell from a section edge did not re-mesh the neighbour
  (`RUSTCRAFT_MESHCHECK` check of the village: one column different from the full meshing);
  `edge::near_of_index`, test `block_two_cells_from_edge_reaches_neighbour` (fails before);
- column meshed without any face missing from the visibility walk (End void between the islands);
- column unloaded with a mesh in flight: result ignored, the reloaded column waits for its neighbours.
`RUSTCRAFT_MESHCHECK` check: before the two-cell fix, only one column of the village
differed over the 13 scenes; after, replayed on village, building, redstone, cave and flight
only (the 8 other scenes remain to replay): columns identical to the full meshing (in flight,
only those bordering an unloaded column differ, as in the game), walk identical to
our own full walk.
Known minor differences with the game, with no proven visible effect: a neighbour counts for the rule
of the 8 neighbours as soon as it exists (the game also waits for its light to be enabled, queue limited per
frame: only the pace of appearance at loading changes); a loaded never-meshed column
blocks the visibility walk (the game crosses its empty sections).

Remaining seen at loading (outside this wave): the client's light recalculation on the main thread
(up to 1.2 s of recalculation per second while loading plains32, 70 ms frames) delays
sending tasks to the pool; building the draw lists at every camera movement
costs 0.29 ms per frame at distance 32 (`sort` of the readout).

### Environment variables read once (series 2, C4-M02, 22/10/2026)

Root cause: 129 reads (in 61 files) `std::env::var`/`var_os` of the game redone at every frame, tick, packet
or event (RELOADTEST per frame, ENTLOG/INVLOG/LIGHTCHECK per packet, DEATHTEST, JUMPTEST,
CHATTEST, NOENT ×2, SOLOLOG, GENLOG…). All of them now go through `rc_data::env_os!`/`env_str!`/`env_flag!`
(`crates/rc-data/src/env_flags.rs`): one static cell per site, read at the first call.
`clippy.toml` forbids `std::env::var`, `var_os`, `set_var`, `remove_var`; explicit exceptions:
tests, bench, example, `build.rs`, `rc-capture`. Common mechanism of the `RUSTCRAFT_OPT_*` switches
of the series (a switch is read `rc_data::env_str!("RUSTCRAFT_OPT_X") != Some("0")`).
Switch `RUSTCRAFT_OPT_ENVONCE=0`: re-read at every call (A/B in the same executable; removed in
series 3, see below).

Micro-bench `cargo test --release -p rc-data env_bench -- --ignored --nocapture`, loaded machine
(other compilations running), 1 M calls:

| Read | `var_os` | guarded macro |
|---|---|---|
| absent key, 1 thread | 136 ns | 0.6 ns |
| long present key (`PATH`), 1 thread | 4.6 µs | 0.6 ns |
| absent key, 32 threads at the same time | 17.4 µs (environment lock) | 20 ns (including thread launch) |

Reading an absent key under 0.2 µs: announced gain in regularity (no system call or global
lock on the main thread and the singleplayer thread), with no scene CPU figure until the scene bench
(crowd, building, plains12, redstone, nether; `samply`: getenv absent from both threads) is
replayed on a quiet machine. A demonstration variable that was set (TICKLOG, INVLOG…) cost an
allocation and a conversion per call.

Series 3 (C4-M02, 03/11/2026): the site body is `#[inline]` and reduced to its own cell (one atomic
load once filled; the cold first read stays out of line in `get_or_init`). `RUSTCRAFT_OPT_ENVONCE`
and its re-read path are removed: the A/B against the re-read reader is done with two executables
(reference = `main` before the series). `env_str!` keeps the validated text in its cell instead of
redoing the UTF-8 check at every call while the variable is set. New `rc_data::opt_switch!` caches
the final `bool` of a `RUSTCRAFT_OPT_*` switch (rule `0` off, `1` on, otherwise the default:
identical to the `!= Some("0")`, `== Some("1")` and three-way readers it replaces, test
`switch_state_matches_the_replaced_readers`). Root cause fixed with it: the client helper that
took the value as an argument (`VARIANTIDX`, `NAMECACHE`, `RIDERLIST`) read the site and
converted it BEFORE looking at its `bool` cache, at every call (`Entity::variant` and the custom
name: per entity and per frame in crowd). Proofs: `each_site_reads_the_environment_once`
(counter of raw reads = 1 after 50 calls; 50 when the value is read outside the cache) and, in the
client, `switches_follow_their_variable` (child test processes under absent, `0`, `1`, empty and
each switch alone at `0`; fails on a wrong default or a wrong name).

Micro-bench `env_bench`, 10 M calls, loaded machine (other compilations running), previous reader
(two cells, out of line) against the current one:

| Read | previous, 1 thread | now, 1 thread | previous, 32 threads | now, 32 threads |
|---|---|---|---|---|
| `env_flag!`, absent key | 1.37 ns | 0.29 ns | 6.37 ns | 2.64 ns |
| `env_str!`, set key (`PATH`) | 31.68 ns | 0.29 ns | 79.81 ns | 2.83 ns |
| switch, value read before its cache | 1.88 ns | 0.47 ns | 8.00 ns | 2.62 ns |

(32-thread figures: wall time per call and per thread.) Scene check still to do on a quiet
machine: `env_flags::site` share in samply on crowd-1000 and crowd (0.02-0.21 % self before, must
fall to about 0; no conclusion in frames/s, under the noise).

### Generation: forgotten chunks not yet written (fix, 22/10/2026)

Fix without a switch (the old path lost decorations). The generation thread
forgot chunks whose write was still queued at the server, then re-read them from
disk: regenerated terrain or old proto-chunk. Bench `evicted_chunks_survive_slow_writer`
(191 chunks, cap lowered to 16): old code 98 chunks different from the reference without a
cap (late writer), 31 even with an immediate writer (full chunks written at save time
only); new code 0 and 0. Memory cost of the overlay: a full chunk set aside keeps its blocks
as runs, 34 KB on average (45 full chunks = 1.5 MB), compaction 0.08 ms per chunk (loaded
machine); a proto-chunk stays whole (≈ 400 KB) until the write thread acknowledges it, in game
a tick or two. No effect expected on generation time: loading scene to replay.

Follow-up after review (22/10): the neighbours of a relight are re-read from the overlay then from disk
(before: overlay alone, a forgotten neighbour already written counted as absent); a full chunk returned by the thread
then saved and re-read stays "ours" (relit); a full chunk is only forgotten without an overlay if
everything is full within two chunks (at one chunk, it could still serve as a relight neighbour,
not saved). Bench with a "save after each batch" mode and the number of relights in
the fingerprint: 0 difference everywhere (cap 16 slow / fast / save, no cap with
save); sabotages: relight through the overlay alone 5 differences (lateral case), saved full chunk
re-read as foreign 6 and 10, acknowledgement without write 28 and 31, old forgetting rule 1 (even without
a cap: the result depended on the saves).

Overlay memory in a long flight (bench `long_flight_memory`, corridor of 9, 40 rows, cap 64,
no save): 180 full chunks retained out of 360 (table + overlay, 36 KB each as runs),
against 227 with the old forgetting rule (the neighbours of an already forgotten full chunk no longer counted as
full). The two edge columns on each side of the corridor remain, independently of the
view distance: about 145 KB per row travelled, freed at each automatic save
(6,000 ticks). Worst case over 5 min: 44 MB at 1 row/s, 109 MB at 2.5 rows/s (elytra). No
cap: the thread cannot write a full chunk itself (the server may have a more recent
version, overwriting it would lose modifications); a cap would require an early save
of the chunks on the server side. Positions of the returned full chunks: ≈ 20 bytes per chunk generated in the
session, never freed.

### Stronghold rings and starts cache (C1-M01, 22/10/2026, micro-bench, loaded machine)

Cause: the old `ring_positions` read the cache, released the lock, computed the 128
biome searches then inserted; every pool thread that missed the cache redid everything
(bench: 3,840 searches instead of 128 at 32 threads). Now the 128 draws (center, seed)
are made without noise at construction; a stronghold is only searched at the first question about a
chunk within ≤ 7 chunks of its draw (proven bound: radius 112 blocks in quarters of 4, reached
on the 5 seeds tested), only once (`OnceLock` per stronghold, init without a radius inside).

Bench `structure::tests::bench_rings_first_chunks` (1,000 chunks around the spawn,
`structures_nbt`, seed 20261011), first batch of 32 / total, in ms:

| Threads | old | lazy | lazy + prefilter |
|---|---|---|---|
| 1 | 975 / 1,117 | 359 / 590 | 232 / 441 |
| 2 | 908 / 1,038 | 117 / 325 | 72 / 230 |
| 4 | 1,336 / 1,537 | 141 / 392 | 100 / 315 |
| 32 | 1,552 / 2,602 | 212 / 1,216 | 227 / 1,229 |

Stronghold searches: 128 to 3,840 (old), 0 (lazy: no stronghold near the spawn).
References prefilter: neighbours without a candidate chunk skipped, starts cache 4,096 →
120 entries (main world) and 1,030 (Nether) on a 64 × 64 square. Biome context
kept (`GENSTART_CTX`), `or_insert` and shared empty list: within the bench noise (±15%),
kept by default, decision on the scene bench. The total that grows beyond 2 threads comes from the
loaded machine and the single lock of the starts cache (to re-measure on a quiet machine before
considering shards). GPU ruled out: biome search in exact f64.
Switches: `RUSTCRAFT_OPT_RINGS_LAZY`, `_STARTS_PREFILTRE`, `_STARTS_ORINSERT`,
`_STARTS_EMPTYARC`, `_GENSTART_CTX` (default 1); `_RINGS_WARM` (default 1 since the quiet-machine bench
of series 2: crowd generation −20%, flight p99 −10%); `_RINGS_BG` (default 0, background computation to
measure in a scene).

### Density evaluator: branches restricted in y (C1-M05, 22/10/2026, micro-bench, loaded machine)

Cause: in volume, `range_choice`, `interval_select` and `lerp` computed all their branches
over the whole volume, whereas the game only computes a branch at the points that read it. In the
26.3 data, the two `lerp`s with an alpha in y of `final_density` (−64..−40 and 240..256) wrap
the whole cave computation, the veins are under `range_choice(y)`, and the choice on
`sloped_cheese` separates air and caves. Now each branch is only computed over the envelope
of the y rows where an element reads it (whatever the input), the start brought back to a cell
boundary of the interpolated nodes of the subtree (otherwise the `val += step` accumulation starts elsewhere:
the "bound + 1" sabotage gives 3 to 152 differences per chunk); a cache filled for a volume also
serves its row sub-volumes.

Limit (review): a choice is NOT restricted when a cache of its subtree has an entry
whose point path differs from the volume (interpolated, noise with a non-power-of-2 scale, § 2.4 of
the noise spec). Restricted, this cache would only be filled for a few rows (or not at all,
or in another order: `range_choice` reads its input first) and a following point read
outside these rows would take the point path instead of the buffer: 9 differences on 3 chunks with the
built function of `point_after_volume_cache_under_choice` before this rule, 0 after. No
point reader of the 26.3 data was affected (`point_after_volume_on_every_cache`: 16 caches,
184,320 values), but a data pack could be. Price: the gain on the veins disappears.

Bench `tests/sampler_opt.rs::bench_final_density` (256 chunks, seed 42), ms/chunk. Pass 2
of the mission (before the limit):

| Path | 1 thread | 4 threads | 32 threads | allocations / chunk |
|---|---|---|---|---|
| old | 1.71 | 0.45 | 0.105 | 44, 2,379 KB |
| RANGE_YVOL | 1.15 (−33%) | 0.34 | 0.073 | 50, 2,073 KB |
| VOL_POOL | 1.64 (noise) | 0.46 | 0.131 | 44, 2,379 KB |
| SPLINE_STACK | 1.69 (noise) | 0.44 | 0.105 | 44, 2,379 KB |
| all | 1.26 | 0.35 | 0.078 | 50, 2,073 KB |

Re-measurement of the combinations (pass 2, very loaded machine), 1 thread: old 1.29, VOL_POOL 1.29,
SPLINE_STACK 1.29, RANGE_YVOL 0.945, RANGE_YVOL + VOL_POOL 0.952, RANGE_YVOL + SPLINE_STACK 0.949,
all 0.952; VOL_POOL allocations identical to the old ones (44, 2,379 KB: new context per chunk,
the pool has nothing to give back). VOL_POOL (buffers reused, cache lent without copy, cells
pre-sized) and SPLINE_STACK (point spline on the stack) gain nothing alone nor
combined: ruled out, code removed (C3, B7). To re-measure if a context lives across several chunks.

Reintroduced (C1-M05B, 23/10): a removal on a loaded-machine measurement goes against C6. Code
restored identically, next to the PINNED rule, behind `RUSTCRAFT_OPT_VOL_POOL` and
`RUSTCRAFT_OPT_SPLINE_STACK`, default OFF (`=1` enables) until the quiet-machine bench (SPLINE_STACK on
by default since C1-M13, measured on the point paths, see below). Equivalence:
`sampler_opt`, 6 variants (each lead alone, RANGE_YVOL + each one, all), buffers given back to the
context after each volume; sabotages verified (cache lent without checking the volume's shape:
up to 92,160 differences per volume; point spline that only reads its 1st coordinate: 4 tests
failing; direct volume spline × 1.0000001: 4 tests failing). `bench_final_density` now has a
"keep" mode (one context per thread kept from one chunk to the next, output given back), very
loaded machine, 2 passes, 1 thread, ms/chunk (±40% noise between passes, no timing conclusion):

| Path | new | keep | allocations / chunk, new → keep |
|---|---|---|---|
| old | 1.49 / 1.47 | 1.28 / 2.06 | 44, 2,379 KB → 7, 400 KB |
| RANGE_YVOL | 1.01 / 1.65 | 0.99 / 1.46 | 43, 2,053 KB → 4, 409 KB |
| VOL_POOL | 1.26 / 1.93 | 1.31 / 1.85 | 44, 2,379 KB → 6, 19 KB |
| SPLINE_STACK | 1.22 / 1.87 | 1.32 / 1.93 | 44, 2,379 KB → 7, 400 KB |
| RANGE_YVOL + VOL_POOL | 1.10 / 1.59 | 1.12 / 1.60 | 43, 2,053 KB → 3, 49 KB |
| RANGE_YVOL + SPLINE_STACK | 1.19 / 1.64 | 1.07 / 1.63 | 43, 2,053 KB → 4, 409 KB |
| all | 1.19 / 1.55 | 1.57 / 1.48 | 43, 2,053 KB → 3, 49 KB |

Only stable figure: with a kept context, VOL_POOL brings the allocated bytes from 400 down to 19 KB per
chunk (−95%), 49 against 409 KB with RANGE_YVOL. The terrain takes a new context per chunk
(`terrain.rs`) and does not give back its outputs: in game, the allocation gain only exists if a
caller keeps its context and calls `Ctx::recycle` (not done, lead noted). Veins and climate:
within noise (veins 0.57 to 1.00 ms depending on the pass for the same path). Decision on the scene bench,
quiet machine.

After the limit (passes 1 and 2, very loaded machine): old 1.62 / 1.62, RANGE_YVOL 1.47 / 1.01
(−9 to −38%) at 1 thread; 32 threads 0.243 / 0.175 → 0.173 / 0.117 (−29 to −33%); 43 allocations,
2,053 KB. Veins (copper/iron density + richness, 16 × 200 × 16, same context): 0.75 → 0.44 before the
limit, 0.50 → 0.50 after (no gain). Point climate (6 roots, 100,000 quarters): 3.0 µs/quarter
in both paths. Equivalence: `sampler_opt` (8 roots + veins + noodle, 200 chunks,
6 volume shapes, with and without caches; 10⁶ points; point after volume on each cache and on
the built function), `density_ref`, `terrain_ref`, full rc-sim suite. Switch
(default 1): `RUSTCRAFT_OPT_RANGE_YVOL`. Lead noted: lazily filling the entire volume of a
restricted cache at the first point access would give back the gain on the veins. Ruled out without code: common subexpressions (census `cse_census`:
2 noises recomputed out of 39 in `final_density`, 0 in the climate and the veins, ceiling < 5%);
GPU (f64, exact floating-point order). Operation band and climate memoized per quarter: not done.

### Density evaluator: kept context, buffers without zeroing, best fit (C1-M06, 03/11/2026, micro-bench, loaded machine)

Three tracks, each behind its switch (default 1, `=0` old path):
- `RUSTCRAFT_OPT_TAKE_WITHOUT_ZEROING`: a buffer taken from the context keeps its old values
  (only the part beyond its old length is written); every caller writes what it reads. The one
  node that added into its buffer (shifted-B noise) now zeroes it itself (prior fix, result unchanged).
- `RUSTCRAFT_OPT_BEST_FIT_BUFFERS`: the smallest spare that holds the request instead of the last one
  given back (a request of 8 values no longer takes a 393 KB chunk volume), new buffers rounded to
  the series 2^k, 3·2^(k−1) (a chunk volume is exactly 96 Ki values), spares capped at 8 MB.
- `RUSTCRAFT_OPT_KEEP_EVAL_CONTEXT`: the terrain (chunk and base column) uses a context kept by the
  thread, reset at the start of each chunk (each cache cell forgets its volume and last point, its
  buffer goes back to the spares), the final density buffer taken from the spares and handed back.
  Off by default since the quiet-machine scene bench of series 3 (03/11, `redstone` and
  `crowd-1000`, 2 passes each, same exe with and without the switch): 1.37 / 1.38 and 1.52 / 1.51 ms
  per generated chunk, private memory 1,536 / 1,498 MB and 1,520 / 1,470 MB. The kept spares of every
  generation thread cost 40 to 50 MB for no gain at scene level; `=1` enables it.

Allocations per chunk (deterministic, `bench_final_density`, 256 chunks, counter around mimalloc with
the client's option, 1 thread):

| Paths | new context | kept context | reserve of a kept context |
|---|---|---|---|
| old | 44.2, 2,379 KB | 6.5, 69 KB | 9,600 KB |
| best fit alone | 46.1, 2,399 KB | 6.3, 19 KB | 2,395 KB |
| range_yvol (default before) | 43.1, 2,053 KB | 3.6, 94 KB | 10,191 KB |
| default now (range_yvol + no zeroing + best fit) | 41.9, 2,021 KB | 3.3, 25 KB | 3,541 KB |

Without rounding (first version), best fit + range_yvol filled the reserve up to the cap (8,152 KB,
759 KB reallocated per chunk): the row sub-volumes change height from chunk to chunk and each miss
added a buffer. Replacing the largest spare on a miss instead brought range_yvol to 4,994 KB but
best fit alone to 8,083 KB (small spares turned one by one into large ones): rounding kept.
`VOL_POOL` with a kept context: no allocation saved (3.3, 25 KB with or without it, the output already
comes from the spares), no time gained (below): stays off. Page faults: 0 to 5 per chunk in every
case (mimalloc reuses its pages), no difference.

Terrain (`bench_terrain_kept_context`, 64 chunks, filling + surface + caves, default switches):
276 to 288 allocations and 2,285 to 2,334 KB per chunk → 233 and 238 KB (−90% of the bytes).
Memory kept by the thread that generated the 64 chunks (`kept_context_memory`): 3,162 KB, spares
and the cache volumes of the last chunk, the same once reset (under the 8 MB cap, nothing dropped).
It stays held for as long as the thread lives, after the world is left too (threads of the global
rayon pool): about 3.1 MB × the generating threads, to weigh on the scene bench.

Time, 1 thread, the six cases alternated over 12 rounds, minimum per chunk, two runs
(`bench_kept_context_alternated`): final density before the mission (new context, without the two
buffer tracks) 0.973 / 0.955 ms; default, new context 0.951 / 0.959; default, kept 0.912 / 0.919
(≈ −4%); kept without no-zeroing 0.947 / 0.931; kept without best fit 0.993 / 0.936; kept + vol_pool
0.954 / 0.921. Whole terrain: 3.773 → 3.776, 4.024 → 4.054, 3.805 → 3.777, 3.814 → 3.817,
3.821 → 3.779 ms (within ±1%). Blocks of 1 to 32 threads (`bench_final_density`): ±50% between
passes, no conclusion. Decision on the scene bench, quiet machine (generation, generation-1thread,
crowd, crowd-1000, redstone, tnt, dropped-items, equipped-humanoids): ms/chunk, chunks/s, private
memory (a kept context holds 3.1 MB per generating thread after the whole terrain, 3.5 MB after 256
chunks of the final density alone).

Equivalence: `sampler_opt` (each new variant alone and in combination against the old code, plus all
the paths with every taken buffer filled with NaN; context kept and reset against a fresh one,
200 chunks × 3 seeds × 3 dimensions, final density and a built cache over an interpolation, point
reads on every cache in the previous, current and next chunk; terrain chunks one after another with
the kept context against a fresh one, base heights of columns inside the chunk and in the next one,
8 chunks × 3 dimensions × 3 seeds; the same chunk generated by four generators in turn on one
thread, overworld under three seeds and the nether), `density_ref`, `terrain_ref`, `terrain_opt`.
Sabotages: shifted-B noise without its zeroing → `density_ref` (46 differences) and both volume
tests of `sampler_opt` fail, only on the variants without zeroing and the poisoned one; reset made
a no-op → 45 chunks differ (the real overworld caches included); the reset call of the terrain
removed → `kept_context_survives_generator_switch` fails (8 of 16 chunks: every overworld chunk
right after the same chunk under another seed reads the other seed's cached volumes). With a single
generator the terrain passes even without the reset, base heights inside the chunk included: the
real 26.3 caches give the same value from a stale volume as from a point read.

Leads noted, outside the files of the mission, left as they are (a follow-up mission is to give
them the same treatment, with its proof of equivalence; protocol item B6 stays open until then): the aquifer drops its surface buffer instead of handing it back (`aquifer.rs`,
`max_surface_level`); the material state zeroes its spare buffers the same way (`material.rs`,
`MatState::buf`); structures (`structure.rs`), monuments (`structure_monument.rs`), `/locate`
(`rc-sim` `cmds_locate.rs`) and the debug screen (`rc-sim` `gen_debug.rs`) take a new context per
call, so they keep the cost of a fresh context (41.9 allocations, 2 MB per chunk-sized volume).

### Density evaluator: column probes, splines, sub-lattice reads (C1-M13, 03/11/2026, counts and micro-bench, loaded machine)

Counted first (`sampler_opt::point_evaluation_census`, 64 chunks, seed 42, per chunk, deterministic;
`sampler::count_evaluations` turns per-thread counters on, off in the game: one static load and a
branch per counting site). Overworld: the aquifer's creation probes the top surface 121 times, one
probe per column of its 11 × 11 quart grid, with 0 point noise and 0 spline: the 2D caches (offset,
factor) answer from the buffer their upper-bound volume just filled (242 reads). The game's slice
rule already reads a 2D child of a 3D parent at y = 0, so the probes of a column share the cache
key. The filling then evaluates 488 point noises and 140 splines (the aquifer's cells, at random
x, z: nothing to share per column); 16 base heights cost 63 noises, 24 splines and 61 probes each.

| Lead | Switch (default) | Result | Decision |
|---|---|---|---|
| Sub-lattice cache read: a cache whose input is exact serves a volume whose points it holds (the final density's 5 × 5 corners inside the aquifer's 11 × 11 grid, the surface's step-16 corners), then holds that volume as if it had computed it | `RUSTCRAFT_OPT_SUBLATTICE_CACHE_READ` (1) | filling: cache volumes computed 23.0 → 16.0, 8,447 → 8,272 values; surface 9.0 → 2.0; 16 base heights 374 → 262, 21,316 → 20,952 values. Aquifer + final density, 1 thread, 12 alternated rounds: 0.974 / 0.999 → 0.956 / 0.982 ms (min / median, −1.8%); 32 threads 0.078-0.082 either way | kept |
| Column memo: a cache whose input never reads y answers, where the game would compute the point, from another height of the column (last point, or buffer at y = 0 for an exact input) | `RUSTCRAFT_OPT_COLUMN_MEMO_Y_INDEPENDENT` (1) | 0 answers in the 26.3 terrain (slice rule); built test: 171,300 → 100,980 cache points computed; a memo answer moves the key to the point read, as the game's computed point does; time unchanged (0.955 / 0.977) | kept for data packs, scene bench decides |
| Volume spline: coordinate buffers on the stack (≤ 8) | `RUSTCRAFT_OPT_SPLINE_INPUTS_ON_STACK` (1) | 9 → 6 allocations per chunk (aquifer + final density); time within noise | kept |
| Point spline on the stack, re-measured on point benches | `RUSTCRAFT_OPT_SPLINE_STACK` (now 1, was opt-in) | point climate (6 roots, 20,000 quarts, 12 alternated rounds): no caches 2.916 → 2.898 µs (−0.6%), with caches 1.149 → 1.135 (−1.2%); first run −1.0 / −1.3%; 1 → 0 allocation per quart | on by default |
| Top surface probes by volume slices | none | 1 probe per column, no noise: nothing to save | not coded |
| Choice branches cut per column (hull per column, sub-volumes 1 × k × 1) | none | filling: 25.4 restricted branches, 105,269 values read, 116,284 computed over the hull of all columns, 110,181 with a hull per column; branches without interpolation (the only ones that could be cut): 8,978 → 8,598 noise values (−4.2%), nether −3% | not coded: one sub-volume per column redoes the y work per column for 4% |

Exactness tightened (fidelity of the shortcuts, A3): at a point the game skips the right side of a
product or quotient whose left side is zero and compares through Java's min and max; over a volume it
multiplies and compares. −0, +0 and NaN can then differ between the two paths, in the game as here,
so `mul`, `div`, `min`, `max` (and a constant min or max at zero) are no longer counted exact. Found by
`point_after_volume_on_every_cache` (−0 against 0 on 6 reads when the memo answered before the buffer);
the memo now keeps the game's order (last point, buffer, then the column). The 26.3 choices restricted in
y are unchanged (25.4 branches per chunk before and after). Lead noted: the rows read of a restricted
choice (`rows_of`) keeps the larger volume where the game's cache would hold the smaller one; with the
tightened rule only exact caches are affected, whose buffer and point give the same bits.

Equivalence: `sampler_opt` (each new variant alone and combined with the old code, 200 chunks, six volume
shapes; `sublattice_reads_match_old_code`: nested volume pairs over overworld, amplified, nether and end,
every root, the vein and cave functions, built caches, point reads after on every cache, 1,536 reads served
by a sub-lattice; `column_memo_reads_match_old_code`: 3 overworld presets, point heights changing between
reads, every cache and five built ones, then a point, its one-block volume, the column higher and the point again), `point_after_volume_*`, `kept_context_*`, the whole rc-worldgen
suite (`density_ref`, `terrain_ref`, `terrain_opt`, `biome_opt`, `material_opt`). Sabotages, each caught:
sub-lattice without the exactness rule → 88 differences (vein caches over an interpolation); rows read from
the bottom of the larger volume → 48; first index + 1 → out-of-bounds panic; y gradient marked as not
reading y → 96 differences per chunk; noise y scale ignored → 36 chunks; buffer at y = 0 for an inexact
input → −0 ≠ 0 on 24 chunks; product, min and max counted exact again → −0 ≠ 0 (12 chunks memo, 15
sub-lattice); volume spline reading zero coordinates → 32 volumes; memo answer leaving the key on the old point → 0 ≠ −0, 54 chunks. Placement: one chunk per task already
(1 core per chunk); GPU without object (f64, exact order). To measure on the scene bench, quiet machine:
generation, generation-1thread, fresh-mountain-rd16.

### Biome tree and zoom (C1-M04, 22/10/2026, micro-bench, loaded machine)

Bench `rc-worldgen --test biome_opt` (ignored), real climates of 1,024 chunks of the main
world (seed 0, 1.57 million searches, one `Searcher` per chunk like the fill).
`find`, 1 thread, ns per search:

| Path | ns | difference |
|---|---|---|
| old | 320 | |
| carried distance + early exit (`BTREE_EARLY`) | 305 | −5% |
| scalar flattened SoA tree | 359 | +12% (loses) |
| SoA + AVX2 | 164 | −49% |
| early + SoA + AVX2 (default) | 149 | −54% |
| memo of the last climate (`BTREE_MEMO`) | 321 | 0 (96 targets repeated out of 1.57 M) |

All cores (old → default, ms): 1 thread 618 → 203, 2: 255 → 100, 4: 127 → 51, 8: 74 → 34,
16: 49 → 22, 32: 33 → 15.5. Biome fill `BiomeSource::chunk` (climate included),
1,024 chunks, 1 thread: 542 → 272 ms. Early exit at zero distance: 116 searches out of
1.57 M (the gain of `EARLY` comes from the carried distance). Zoom, ns per call: 384-block-high
column from top to bottom 42.7 → 24.2 (owned memo) / 29.0 (thread memo, materials path);
positions scattered around a chunk 44.9 → 53.7 (loses: the decoration keeps the direct zoom).
Decisions: `RUSTCRAFT_OPT_BTREE_EARLY`, `_BTREE_SOA` (alias `_BIOMETREE_FLAT`), `_BTREE_SIMD`
(AVX2 detection at run time), `_ZOOM_MEMO` at 1 by default; the flattened tree is only taken with
AVX2 (scalar losing); `_BTREE_MEMO` at 0 (`=1` enables it). SIMD of the zoom corners ruled out
(64-bit products without AVX-512). GPU ruled out: ≤ 19 distances per node, sequential search
dependent on the last leaf. Memory: +56 B per node (≈ 0.45 MB for the main world).
Scenes to measure: flight, generation, crowd, redstone.

### `/locate biome` in two phases (I-09, 24/10/2026, micro-bench, loaded machine)

In the no-overshoot regime, the pruning of the tree is exact: the chained search returns the
last leaf if it is at the minimal distance dmin(t), otherwise F(t), the first leaf at dmin
in walk order. Stateless phase (climate + (leaf, dmin)) in parallel tasks of 64
points, growing batches (64 × threads, doubled up to × 32); serial phase: last leaf if
at dmin, otherwise F(t), otherwise (F(t) unknown, or target outside the regime) chained search. Biome
fill at generation not touched (already parallel across chunks). `biome_opt` bench in series
(`--test-threads=1`), best of 3 to 5:

| Bench | serial | 1 thread | 2 | 4 | 8 | 16 | 32 threads |
|---|---|---|---|---|---|---|---|
| `bench_locate` (full walk, 71,407 points), without help, ms | 49.1 | 65.6 | 34.4 | 18.1 | 9.8 | 7.0 | 6.8 |
| same, neighbour bound d + 1 (`BTREE_SPLIT_CAND=0`) | | 62.6 | 32.6 | 17.0 | 9.5 | 7.0 | 6.4 |
| same, candidate neighbour (default) | | 49.2 | 26.1 | 13.7 | 7.8 | 5.8 | 5.5 |
| leaves only, 1.57 M real climates, chained 177 ms: without help | | 821 | 638 | 326 | 162 | 105 | 81 |
| bound d + 1 | | 507 | 416 | 172 | 115 | 80 | 62 |
| candidate neighbour (default) | | 336 | 155 | 102 | 77 | 52 | 45 |

The bound d + 1 forces going back down to the neighbouring leaf; as a candidate, it is already
retained and only a result that strictly beats it is surely F(t). /locate: 1 thread without loss
(49.2 against 49.1 ms), ×8.9 at 32 threads; extrapolated to radius 6,400 (≈ 1.1 M points): ≈ 0.77 s → ≈ 85 ms
of server thread. Memory: 64 B per batch point (≤ 64 × threads × 32 points, ≈ 4 MB at 32 threads).
Finding: over 200 real searches, breaking ties by the last leaf never changes
the returned biome (tied leaves of the same biome); the rule is kept bit for bit (constructed case
`split_keeps_last_leaf_on_ties`). GPU ruled out: climate = density functions in f64 with
exact floating-point order. Decision (review of 24/10): `RUSTCRAFT_OPT_BTREE_SPLIT` OFF by
default (`=1` enables it), `_BTREE_SPLIT_HINT` and `_BTREE_SPLIT_CAND` at 1 (no effect without it). The
×8.9 is taken on an empty reserve; in game, the batches go through the shared reserve, shared
with the client's generation and meshing (lowered-priority threads), and a radius thread only
takes an injected task after its queue and stealing from the others: with generation in
progress (flight in singleplayer), each of the ≈ 20 batches may wait for the end of a generation batch, and the
command's tick may exceed the ≈ 0.77 s of the serial path, which waits for nobody (C5 not
settled). Scenes to measure before turning it on: `/locate biome` of an absent biome (radius 6,400,
find bench seed 0) with an idle reserve, then the same during a singleplayer flight (generation in progress);
if the second loses, leads: reserve of the server's own (like the light), or fallback to serial
when the first batch is late.

### Sheets, base height, carved caves, terrain ids (C1-M09, 22/10/2026, micro-bench, loaded machine)

Bench `rc-worldgen --test terrain_opt_bench` (12 × 12 grid from (0, 0), seed 0, main
world, 1 thread, two alternating passes; bench allocation counter). Equivalence:
`--test terrain_opt` (chunks of 4 settings × 3 seeds × 10 chunks, each switch alone
then all; sheet block by block on drawn densities; 10,000 heights × 2 types × 3 dimensions;
500 cave masks per seed), `aquifer::tests::net_matches_insertion`, and `terrain_ref`
(51 chunks against the Java probe, 0 difference, new paths).

| Item | old | new | Switch (default 1) |
|---|---|---|---|
| Fill (sheets), ms / chunk | 2.65-2.75 | window 2.35; + network 2.28; + pretest 2.23-2.50 | `AQ_COLWIN`, `AQ_SIMD`, `AQ_PRETEST` |
| Density > 0 pretest alone | 2.65-2.75 | 2.55-2.57 | `AQ_PRETEST` |
| Cave masks, ms / chunk | 0.085 | yd 0.082; warm replay 0.071; all warm 0.066 (cold 0.082-0.087) | `CARVER_YD`, `CARVER_WIDTHS`, `CARVER_REPLAY` |
| Mask allocations / chunk | 4.6 | reused widths 2.0; warm replay 2.0, cold 27.8 | `CARVER_WIDTHS` |
| Full chunk 1 thread, ms | 5.1-5.4 | 4.8-4.9 | all |
| Base height, µs / call (world, Nether, End) | 168 / 25 / 73 | see below | `BASEH_LAZY`, `BASEH_MEMO` |

Lazy base height (density slices aligned on the interpolation cells, stop
at the first block found): fixed slices of 1 / 4 / 8 cells = 565 / 262 / 197 µs in the main
world (the fixed cost of a volume, 2D caches included, dominates), 6 / 10 / 16 µs in the Nether
(ceiling found from the first slice). Doubling slices: 238 µs (world), 6 µs (Nether).
Probe of one cell then the rest at once: 185 µs (+10%), 6 µs (÷4), 78 µs End (+7%). Kept:
ADAPTIVE probe (abandoned per generator when it succeeds less than once in 4 after 64
attempts): world 190-245 against 222-237 µs, Nether 6-8 against 24-31 µs, End 106-114 against
93-100 µs (more loaded pass, to confirm on a quiet machine; all threads: 16.3 against 21.6 µs);
the other splits remain reachable (`lazy_mode`, hidden field, for the benches).
Shared memo (16 shards, emptied at 4,096 entries): 99 / 69 / 132 calls for 1,024 chunks of
structures (3 seeds), of which 38 / 12 / 17 repetitions served. The base height remains a small
item (≈ 100 calls per 1,024 chunks).
Cave replay: trace of each (cave, rank, source) recorded without a target (range tests
assumed successful), replayed per target with the same stops; 2,352 traces = 4.05 MB for the grid
(≈ 1.7 KB per trace), cache capped at 16 MB (shard emptied beyond 1 MB). Terrain ids
resolved once and `BlockTable::get` without `full_id` for a full name: within noise in time,
2 allocations per chunk (sheet) and 3 per carved chunk fewer (`TERRAIN_IDS`).
Multi-core: generation remains one chunk per task (rayon), 1 to 32 threads measured, no
change of placement. GPU ruled out: sheet and caves are exact Java branches and randomness,
without a regular compute volume.
Scenes to measure on a quiet machine: `flight`, `generation`.

### Background audio output, reopening, command channel (C4-M03, 22/10/2026)

Micro-benches `rc-audio/examples/open_steps.rs` and `engine_opt_bench.rs`, loaded machine (other
compilations running); scene bench ("1st frame", plains12, pause) to do on a quiet machine.

- Diagnosis of the ≈ 370 ms of startup (7 cold openings): creation of the WASAPI stream
  (activation + initialization) takes 330-460 ms; host 0, device choice 4-30, format
  3-9, read 0 ms; sound table 15-94 ms. The 22 / 393 ms bimodality of the bench is explained: another
  stream already open on the same output point keeps it "warm" (stream 3.5-4.6 ms). The player's common
  case is the cold case.
- `RUSTCRAFT_OPT_AUDIOFOND` (default 1): output opened and held by the "audio-output" thread;
  the engine is returned immediately. Expected gain ≈ −370 ms on the 1st frame (to confirm on the
  scene bench). `RUSTCRAFT_OPT_AUDIOFILE` (default 1): sounds requested during the opening queued
  and played as soon as the output responds. This is the game's behaviour, which reloads its
  engine synchronously (frozen frame) then plays what follows, including the click of the "Device"
  setting; at 0 (sounds dropped as with no device), that click was always lost.
- `RUSTCRAFT_OPT_AUDIOREOPEN` (default 1): device change without re-reading `sounds.json`
  or emptying the PCM cache; the old output, stopped and mute, lives until the new one
  responds (same output point: opening ≈ 5 ms instead of ≈ 350 ms). Main thread, 8
  reopenings: 21-128 ms (old: table re-read + new engine) against 0.1-1.4 ms.
- `RUSTCRAFT_OPT_AUDIOLISTENER` (default 1): listener in a shared cell (sequence lock)
  instead of one command per frame. 1,000,000 frames: 78 / 111 ns against 74 / 146 ns per frame
  on the main thread, mixing 188 / 207 against 237 / 143 µs per block: within noise. Kept:
  output identical bit for bit (1,000 blocks) and the channel no longer fills with listeners at
  3,500 fps.
- `RUSTCRAFT_OPT_AUDIOOUTBOX` (default 1): commands refused by a full channel kept in
  order and sent back at the head of the next batch; the old path loses launch, loop stop and
  total stop under saturation (test that proves it). Beyond 65,536 pending commands, only
  the last position, the last gain and the last pitch of each voice, the last listener
  and the last category volumes remain; the next threshold goes to double what remains
  (no full walk at every send). With `AUDIOOUTBOX=0`, a total stop lost to a full
  channel immediately frees the old output of a reopening.
- `RUSTCRAFT_OPT_AUDIODEVICES` (default 1): a persistent "audio-devices" thread instead of a thread
  created every second, first readout off the main thread (list: 0.29-3.5 ms). The first
  readout is retained without a decision (same sequence of decisions as the old tracking, test).

### Client audio engine joined at first use, device from creation (C4-M04, 22/10/2026)

Micro-bench `audio_slot::tests::bench_audio_main_thread` (real resources, real output, 6
passes, medians, loaded machine: other compilations running); scene bench (1st frame of the
13 scenes, first-launch, `soundDevice` empty and named, F3+T) to do on a quiet machine.

- Finding: after C4-M03, the sound table and the creation of the engine remained at startup on the
  main thread; and when `soundDevice` is named, two openings (engine created on the
  default device, then `apply_video_options` reopens it), likewise at every F3+T.
- `RUSTCRAFT_OPT_AUDIOJOIN` (default 1, specific to the client; `AUDIOFOND` of C4-M03 now only controls
  the opening of the output in the background in rc-audio, the two are measured separately): engine
  built on the "audio-engine" thread launched as soon as the resources are indexed (before the baking), joined at first
  use (`audio_slot::Audio::get`, the only access to the engine; unchanged listener, volumes and device
  do not join). Main thread at startup: 7.0-24.7 ms → 0.03-0.06 ms of
  launch + 0.05-0.07 ms of join (after 300 ms simulating window and GPU: no wait).
  Log "audio: engine joined … after its launch, wait … ms".
- `RUSTCRAFT_OPT_AUDIODEVICE1` (default 1): engine created directly on `soundDevice`, volumes
  set at creation; one opening instead of two (test `named_device_opened_once`). Output
  ready since the request, cold output point (2 s without stream, alternating order): 670.8 → 78.3 ms
  (first pass 351.5 → 21.3 ms with only 300 ms of rest). Fidelity fix at
  F3+T with a named `soundDevice`: the old path recreated the engine on the default device
  then `apply_video_options` reopened it and played an interface click; the game reopens its
  engine directly on the chosen device, without a click (the click only follows a change of the
  setting by the player): 0 sounds at F3+T with `AUDIODEVICE1=1`, 1 with `=0` (test
  `reload_with_named_device_plays_no_click`).
- `RUSTCRAFT_OPT_RELOADAUDIO` (default 1): at F3+T, engine of the new stack built during
  the baking ("resources" thread) and returned with it; `apply_loaded` only does the stopping of the sounds
  and the swap, at the same instant as before; the old engine is freed on "audio-release"
  (1.4-1.9 ms). Main thread: 107-166 ms → 0.03-0.06 ms. Wiring tested
  (`reload_uses_engine_built_during_load`, `retired_engine_released_in_background`).
- `RUSTCRAFT_OPT_AUDIOEARLY` (default 1, 24/10): the resource hook that launches the engine
  fires right after the index of each stack tried, in parallel with the font check, instead of
  after it (moment wrongly retained by the series 2 merge). Stack abandoned afterwards (default
  font failing): the call of the next stack replaces the engine (`Audio::respawn`); its
  thread joins and frees the old one, which played no sound, before creating the new one (a single output at
  a time). Startup and F3+T. Test `early_hook_on_abandoned_stacks_yields_retained_engine`
  (fails if the abandoned engine is kept or never freed). Micro-bench `bench_audio_early`
  (real resources without a pack, without output, fonts read without the baked cache, loaded machine,
  median of 10, two passes): engine ready since the index 83.2 → 64.4 and 89.1 → 68.3 ms;
  fonts on the main thread 63.0 → 64.1 and 69.2 → 68.3 ms (read contention, noise). With the warm baked cache, the fonts
  are shorter and so is the gain: to confirm on the scene bench (startup, milestones
  "resources indexed" → "audio joined").
- Milestones "[startup] <step> at N ms" (origin: process creation): version data,
  options, resources (index, models, fonts/language/items, entities, particles, interface, end),
  physics and light, audio, singleplayer world, event loop, window, instance, adapter,
  GPU device, block textures written, audio joined, first frame.
- Sound table cache: ruled out (see "Ruled out").

### Sound launch and ambience without allocation (C4-M24, 23/10/2026)

A single switch, `RUSTCRAFT_OPT_SOUNDIDS` (default 1, `0` = old path, read once;
the client reads it through `rc_audio::sound_ids_enabled`). Micro-bench figures, loaded machine
(other agents compiling); final decision on the `village` and `crowd` scene bench, quiet
machine.

- Sound table compiled at the first draw (forgotten at every merge of `sounds.json`):
  interned ids and paths (`Arc<str>`), event index, precomputed effective weights
  and height of the longest chain of references. The depth bound (16) is kept: the
  precomputation is only used if `depth + height <= 17` (exact threshold: beyond it, the deepest
  reference would be weighed 0), otherwise the weight is recomputed as before; a cycle always forces
  the recomputation. Same `next_int_bounded(total)`, same walk. Draw alone (1,991 events × 200
  seeds, `examples/sound_ids_bench.rs`): 46-78 ns and 1 allocation → 22-35 ns and 0.
- Launch by the engine (`AudioEngine::play`, warm PCM cache, 12 village and crowd sounds,
  3 per tick, 6,000 sounds, 2 alternating passes 0/1/0/1): 1,035-1,164 ns and 4.00 allocations per
  sound → 438-575 ns and 0.00; same sounds launched (5,350/6,000 in both paths once the
  variants are cached). Removed: request name copied (the direct launch borrows the name),
  full id built (allocation-free lookup, `minecraft:` prefix on the stack),
  path copied (shared), O(n) recount of the channels per reserve (counters kept up to date at each
  entry and exit, verified by recount in the tests), weights recomputed twice per draw.
  PCM cache key: the shared path (`Arc<str>`, lookup by `&str`), not the variant index:
  a single kind of cache for both paths and for the loading threads; hashing a
  path of ~40 bytes is negligible next to the cache lock.
- Repeated subtitles (every 10 ticks) pushed directly into the queue, without an
  intermediate list or event copy. Their order within a tick follows the channel table
  (randomly seeded hash), as before: the tests compare each tick sorted.
- Ambience and music per tick (client): `Env::ambient_sounds_ref` / `background_music_ref`
  borrow the attributes; `Track`, `Mood`, `AmbientSettings` as `&str` / slice. Copy
  avoided: 4.0 allocations and ~217 ns per tick (crimson forest and plains alternating, test
  `audio_attributes_borrowed_allocate_nothing`), plus the copy of the track event.
- Placement: main thread, one sound at a time where it is requested; neither multi-core nor GPU
  applicable (a few hundred ns per sound). RAM: compiled table ≈ 1,991 events and
  their variants on top of the original table (a few hundred KB), kept as long as the engine lives.
  Compilation 1.1 ms (reading `sounds.json` 9.7 ms), done at the construction of the engine ("audio-engine"
  thread of the client) and not at the first sound.
- Unbounded PCM cache (like the game): `cache_bytes` after 30 min of play to measure on the scene
  bench (LRU cap only beyond 100 MB); on the micro-bench, 19-30 sounds, 1.5-2.2 MB.
- Proofs: `indexed_pick_matches_recursive_pick_vanilla` (1,991 events × 10,000 seeds,
  variant, settings and generator after the draw identical, stored fingerprint),
  `indexed_pick_matches_recursive_pick_synthetic` (chain of 22 references, cycles, missing reference),
  `sound_ids_bit_exact` (mixer output bit for bit, subtitles, channels),
  `sound_ids_vanilla_launches_identical` (all events, saturated reserves: same handles
  and voice parameters), borrow/copy equality test of rc-env. Sabotages verified: depth
  threshold +1, counter not decremented, volume ×0.99, biome layer ignored, default weight
  changed in both draws (fingerprint).

### Audio engine: per-frame work, subtitles, sound table, loading, devices (C4-M29, 03/11/2026)

Micro-benches on a loaded machine (other agents compiling), 2 alternating passes in the same
process; final decision on the scene bench (`village`, `plains12`, `title`, `crowd`, quiet machine).
Placement: everything stays where it was (main thread for the engine, "audio-devices" and
"rc-audio-load-*" threads); GPU not applicable (a few ns to a few ms of branchy work).

- Per frame (`engine::tests_frame::frame_bench`, 32 loops, a block every 35 frames), ns/frame:
  old 19.8 / 21.0 (22.6 / 20.0 on a re-run). `RUSTCRAFT_OPT_AUDIO_PUMP_SKIP_IDLE` (default 1: outbox
  skipped when empty, loaded channel skipped while no load is in flight, counted per job; finished
  voices still polled every frame since the poll that sees an end sets the relaunch tick of a
  repeated sound): 18.9 / 19.7 (15.1 / 18.9). `RUSTCRAFT_OPT_AUDIO_LISTENER_DIFF` (default 0):
  24.0 / 25.7 still, 34-39 moving: on x86 the cell write is 10 plain stores, cheaper than comparing
  9 components. `RUSTCRAFT_OPT_AUDIO_PUMP_RATE_LIMIT` (default 0, poll at most once per ms): 33-47:
  the clock read costs more than the poll; with it at 1, an end seen in the last millisecond before
  a tick moves to the next tick (relaunch of a repeated sound one tick later, as random as before).
- Commands (48 attached sounds, half still, + the 11 sliders applied every tick):
  `RUSTCRAFT_OPT_AUDIO_COMMAND_DEDUP` (default 1: no position command for an unchanged position,
  category volumes coalesced and sent at the next poll or before the next command, only if they
  differ from the mixer's): 1,183 → 663 ns per tick (1,477 → 782), 59 → 24 commands per tick.
  `RUSTCRAFT_OPT_AUDIO_MIXER_VOICE_IDS` (default 1, voice found in a list of ids): 247 voices
  37.4 / 36.2 → 33.2 / 36.8 ns per command, 64 voices 9.8 / 10.7 → 9.7 / 10.2 (noise; 2 KB walked
  instead of 37 KB).
- Subtitles (`RUSTCRAFT_OPT_SUBTITLE_INTERNED`, default 1): key shared with the compiled table
  (`Arc<str>`, found by event index) instead of looked up by name and copied; launch with subtitles
  on 153 / 155 → 130 / 123 ns, repeated subtitle 65 / 55 → 31 / 23 ns. Client: each line keeps its
  text and width, recomputed when the translated template or the GUI font generation changes
  (`rc_text::Lang` has no identity; a translation without arguments depends only on its template).
- Sound table: `RUSTCRAFT_OPT_SOUND_TABLE_COMPILED_ONLY` (default 1): source table dropped once
  compiled, −1,565 KB of heap. `RUSTCRAFT_OPT_SOUND_TABLE_SORTED_LISTING` (default 0): binary
  search in the sorted `.ogg` listing instead of a set, load 11.6 / 12.6 / 16.3 → 14.3 / 14.6 / 18.6
  ms (the set costs 0.12 ms; the listing itself 3.2 ms, which only an existence query known to the
  baked resource cache could avoid: not available, a direct query would open the jar on a warm start).
- Loading: `RUSTCRAFT_OPT_AUDIO_LOADER_THREADS` (default 1): clamp(cores / 8, 2, 4) threads
  (`RUSTCRAFT_THREADS` forces the core count; not the rayon pool). Burst of 50 first sounds
  (`engine::tests_pcm::loader_bench`): 1 thread 53.1 / 56.3 ms, 2: 30.6 / 31.8, 4: 19.8 / 19.6,
  8: 13.4 / 12.0. `RUSTCRAFT_OPT_AUDIO_PRELOAD_VARIANTS` (default 1): at the first playback of an
  event whose sound is short (< 2 s), its other buffered variants decoded on a low-priority queue
  (a waiting sound is always taken first). Village-like run (50 first sounds, then 3 per tick for
  400 ticks): 107 → 5 of 1,200 launches waited for a decoding, burst unchanged (30.6-31.6 vs
  30.2-31.5 ms). 30-minute session (`pcm_budget_bench`): 242 → 136 waiting launches unbounded,
  983 → 876 at 32 MiB, same memory (89.2 / 33.2 MB) and evictions (797). The game also waits at the
  first playback of each variant; we now wait only at the first of each event (sooner, never later).
- Devices (`RUSTCRAFT_OPT_AUDIO_DEVICE_EVENTS`, default 1): Windows endpoint notifications (added,
  removed, state, default, property = rename) raise a flag; the poll keeps its 1 s rhythm and reads
  the names only after a notification, plus every 30 polls; without notifications (other systems,
  refused subscription), identifiers probed every second, names read when they change and every 10
  polls. Real devices (`devices::tests::device_poll_bench`): full read 4.737 / 4.203 ms per poll,
  identifier probe 3.566 / 3.798 ms (barely cheaper here), notifications 0.102 / 0.095 ms on
  average (−98%). CPU of the "audio-devices" thread in `title`, `plains12`, pause: scene bench.
- Proofs: `engine::tests_frame` (`frame_savings_bit_exact`, `frame_savings_skip_the_work`,
  `idle_poll_still_receives_loads`, `command_dedup_drops_only_no_ops`, `subtitle_keys_shared_with_table`,
  `loader_threads_follow_the_machine`), `mixer::tests::mix_digest` (16 combinations),
  `sound_ids_bit_exact` with copied keys, `sounds::tests::compiled_only_table_matches_source_table`,
  `engine::tests_pcm::first_playback_preloads_the_other_variants`,
  `devices::tests::device_events_replay_the_old_decisions`, client
  `subtitles::tests::kept_line_text_matches_fresh_text`. Sabotages checked in the commits.

### Bounded PCM cache (I-05, 24/10/2026, micro-bench and simulated session, loaded machine)

Prior measurement (`rc-audio/examples/pcm_census.rs`): 4,843 non-streamed files = 827 MB of
PCM; model survival session (41 families: cave, underwater, desert, rain, common blocks,
common mobs, interface): 512 files, 93.7 MB, of which 67 MB in 100 sounds of 2 s or more.
30-minute simulated session (ignored test `engine::tests_pcm::pcm_budget_bench`, 55,800 sounds,
1.5 common sound per tick and one long sound per second, mixer and ticks at the game's pace):
unbounded 89.2 MB at the end of the session; budget 32 MiB: 33.2 MB (−56 MB), 797 evictions, 741
re-reads of long sounds, 0 waits for a common sound. Scene bench to do on a quiet machine ("PCM cache" line of F3, long
walk with sound, menus and pause).

- `RUSTCRAFT_OPT_PCMBUDGET` (default 1; 0 = unbounded, like the game): beyond 32 MiB, eviction
  among the free sounds (only reference = the cache: no voice or loading in progress), of at
  least 2 s, not "frequent" (repetition score < 3, half-life 600 ticks) and not `preload`,
  lowest score first then least recent. The budget may be exceeded by the protected sounds.
- Short threshold: at 1 s, "least recent" order, 3 waits for common sounds in 30 min at the budget of
  32 MiB (0 with the score order, kept; 180 with 4 MiB:
  zombie, cow, sheep, arrow, deepslate, from 1 to 2 s); at 2 s, 0 at the default budget
  (`common_sounds_never_wait`, 50 common sounds, each one starts from the cache in the same call). Under a
  budget of 4 MiB (less than the ~13 MB of protected sounds), the 2.1 s zombie cry still waits
  14 times: a budget lower than 32 MiB would require keeping the score of the forgotten sounds.
- Eviction order "least recent" alone then "score then least recent": same result at the default
  budget (0 waits), 866 evictions and 807 re-reads against 797 and 741; the second is kept
  (−8% of re-reads, no switch: same protection rule, only the order changes).
- Re-reading a forgotten sound (decoding alone, 1 thread): median 6.0 ms, p99 28.9 ms, max 29.2 ms
  (14.7 s underwater ambience); only long and rare sounds (ambiences at a random moment)
  may start 1 to 3 mixer blocks later, as at the first launch of any sound, for
  us and in the game (asynchronous loading). PCM re-read identical bit for bit.
- Cost: eviction decision only at insertion beyond the budget (walk of a few
  hundred entries, rare); reading a cached sound: a rank and a score updated under the
  existing lock. Neither multi-core nor GPU applicable.

### Mixer and Vorbis decoding (C4-M23, 22/10/2026, micro-bench, loaded machine)

Bench `rc-audio/examples/mix_bench.rs` (real sounds, blocks of 480 frames, 2 alternating passes,
other compilations running); scene bench (plains32, pause) to do on a quiet machine, with the
readout of `active()` in plains32. All the switches give the same output bit for bit
(`mixer::tests::mix_digest`, 2,000 blocks, the 8 combinations against the frozen fingerprint of the
old mixer). The game does not refuse a sound that is "too far" (only a computed zero volume):
out-of-range voices play at gain 0 in OpenAL, so it is up to the mixer to skip them.

| ns / voice·frame (247 voices, µs / block) | in range | half out of range |
|---|---|---|
| old (all at 0) | 12.1 / 12.5 (1,432 / 1,484) | 11.7 / 12.3 (1,388 / 1,461) |
| `MIXMUTE` alone | 12.4 / 12.0 | 11.3 / 6.3 (64 voices: 6.3 / 5.9) |
| `MIXWEIGHTS` alone | 10.4 / 10.8 (−15%) | 10.3 / 10.3 |
| `MIXSIMD` alone | 6.5 / 6.6 (−46%) | 6.8 / 6.5 |
| `MIXMUTE` + `MIXWEIGHTS` | 10.3 / 10.1 | 5.3 / 4.9 |
| all three (default) | 6.9 / 6.1 (818 / 722) | 3.5 / 3.3 (416 / 390, −72%) |

- `RUSTCRAFT_OPT_MIXMUTE` (default 1): voices whose start and end gains are zero
  (out of range, category at 0, muted sustained sound, fade to 0) only advanced: 16-bit
  fixed-point buffer (exact, step and position are multiples of 2⁻¹⁶), stream drawn frame
  by frame without interpolation. Cost of a muted voice ≈ 0.
- `RUSTCRAFT_OPT_MIXWEIGHTS` (default 1): cubic weights once per frame (same order
  of operations), direct buffer read away from the edges, listener reference once per block.
- `RUSTCRAFT_OPT_MIXSIMD` (default 1): buffers in groups of 4 frames (weights and reads in
  arrays, vectorized by the compiler, without FMA); the biggest gain, kept by default.
- Multi-core (rayon, 247 voices spread over k mixers, everything enabled): 737 / 403 / 271 / 234 /
  193 µs per block for 1 / 2 / 4 / 8 / 16 mixers. Ruled out: the output thread is real time (one
  block every 10 ms) and the rayon pool is shared with generation and meshing (risk
  of a missed deadline); at 247 voices the mixer alone takes 0.7 ms per block (7% of a core). To
  re-measure if plains32 shows more than 100 active voices.
- GPU: ruled out without a bench (10 ms blocks, GPU round trip of the order of a ms, CPU cost already
  under 1% of a core at the usual voice counts).
- `RUSTCRAFT_OPT_DECODECOPY` (default 1), 4,869 sounds of the game (excluding music): `.ogg` decoded without
  copy, PCM reserved from the last granule (+ half a window) then margin returned:
  capacity kept 1,203 → 838 MiB (−30%, the PCM cache keeps every sound played), bytes allocated
  3,891 → 3,729 KiB per sound (the rest: a `Vec` per packet in lewton), 1,977-2,299 →
  1,858-1,973 µs per sound (noise). PCM identical bit for bit (`decode::tests::decode_digest`,
  42 real files, frozen fingerprint).
- `RUSTCRAFT_OPT_RINGCHUNK` (default 1): the stream thread writes each packet in slices of
  the ring and sleeps until the mixer has emptied it by half (≈ 1 wake-up per second instead of
  50 per second when the ring is full). Stream read == `decode_all` twice (loop), old
  and new thread (`stream_matches_decode_all`). CPU of `rc-audio-stream` when paused: to measure.
- On the read side, the ring is still read frame by frame: a read per slice would freeze what is
  available at the start of the block (different output if the decoder writes during the block) for
  a few atomic loads per frame, on 1 or 2 streams. Superseded by C4-M30 (`STREAM_SLICE_READ`,
  below): the slice is read only when the ring already holds the whole block, so the output stays
  identical.
- lewton against symphonia 0.5.5 (`examples/vorbis_compare.rs`, 552 sounds, 248 M samples):
  symphonia 10.7 ns per sample against 18.6 (×1.74), but PCM identical for only 56 sounds,
  different length for 206 (end of stream), 109,822 samples at ±1. Ruled out (output
  changed). Neither is the game's decoder (stb_vorbis through LWJGL): if one day we
  compare with the game, choose the one that resembles it most, and only then speed.
  Re-measured with symphonia 0.6.1 (27/10/2026, quiet machine, same 552 sounds): 7.5 ns per
  sample against 14.0 (×1.88); PCM identical for 10 sounds, different length for 337 (0.6 trims
  the end padding from the granule position, gapless decoding being the default; without it, it
  also returns the 128 priming samples lewton drops), 109,822 samples at ±1. Still ruled out.

### Mixer: 8-lane AVX2 groups, row reads, fixed-point positions, stream slices, stereo output (C4-M30, 03/11/2026, loaded machine)

All figures on a loaded machine (other agents compiling); the scene bench (plains32, village,
CPU of `cpal_wasapi_out` and `rc-audio-stream`) is still to do on a quiet machine. Every switch
gives the same output bit for bit: `mixer::tests::mix_digest` (144 combinations against the
frozen fingerprint of the old mixer), `group_renderers_match_the_old_one` (12 renderers, 1,000
edge cases), `stream_slice_matches_frame_reads`, `decode::tests::stream_matches_decode_all` (also
read by slices), `engine::tests::stereo_f32_output_matches_write_out`; each checked to fail on a
sabotage.

Reading the generated code of the 4-lane groups (`MIXSIMD`) showed where the time goes: every point
loaded, converted to `f32` and inserted into a vector one at a time, and every lane position
converted from f64 to an integer and back (no packed form of these before AVX-512). Groups of 8
alone change little; the three ways below are independent and measured alone, then combined.

Renderers called directly (`mixer::tests::mixer_lanes_bench`, synthetic noise, one voice in four
stereo, pitches 0.5-2): 31 interleaved rounds, median of the ratio to the old renderer measured
in the same round (the load moves absolute times by ±20 % between runs, the ratio far less).

| median ratio to the old renderer (4 lanes) | 64 voices | 247 voices |
|---|---|---|
| 8 lanes, AVX2, alone (`MIXER_AVX2`) | 0.919 | 0.908 |
| 8 lanes, baseline x86-64 (not used) | 0.906 | 0.951 |
| row reads alone (`MIXER_PACKED_READS`) | 0.903 | 0.908 |
| fixed-point positions alone (`MIXER_FIXED_POSITIONS`) | 0.775 | 0.819 |
| reads + positions, 4 lanes | 0.717 | 0.725 |
| reads + positions, 8 lanes baseline x86-64 | 0.662 | 0.686 |
| all three (default) | 0.619 (3.38 ns / voice·frame) | 0.626 (3.38) |

Real sounds (`examples/mix_bench.rs`, quietest of 2 alternating passes, base exe = the code before
the mission): 247 voices in range 7.57 → 3.87 ns / voice·frame (−49 %, 897 → 459 µs per 10 ms
block, 9.0 → 4.6 % of a core), 64 voices 7.73 → 3.82 (−51 %), 247 voices half out of range 3.27 →
1.89 (−42 %). The new code with its switches at 0 stays at the base level (7.05).

- `RUSTCRAFT_OPT_MIXER_AVX2` (default 1): the same generic group body compiled for AVX2 in a
  `#[target_feature]` function, chosen at run time (`is_x86_feature_detected!`, the only guard of
  the call; 4 lanes without AVX2). Same operations per lane, no FMA.
- `RUSTCRAFT_OPT_MIXER_PACKED_READS` (default 1): mono and stereo points gathered as rows of
  16-bit samples, each row converted at once. Tried first: one slice of 4 (8 in stereo) samples
  per lane, one bounds check per lane instead of per point: the compiler then keeps the points
  lane by lane and falls back to scalar products; rows win. The 32 bounds checks per group remain
  (the compiler cannot see that lane indices only grow).
- `RUSTCRAFT_OPT_MIXER_FIXED_POSITIONS` (default 1): lane positions in 16-bit fixed point
  (integer adds, shifts and masks pack into vector registers). Exact: the step is rounded to 2⁻¹⁶
  and the position only adds steps or wraps, so the f64 sums of the frame loop are exact too;
  checked once per block (`to_fixed16`, shared with the muted voices).
- `RUSTCRAFT_OPT_STREAM_SLICE_READ` (default 1): when the ring already holds every frame the block
  reads (`floor(pos + frames × step) + 1`, exact), the block is read from one slice and committed
  once, instead of one load of the producer position and one store of the consumer position per
  sample. Two streams (music stereo, record mono, rings kept full): 8.7 / 7.7 → 5.5 / 5.4 µs per
  block (−30 to −37 %). Underfed, not primed or more than 2 channels: the old path.
- `RUSTCRAFT_OPT_MIXER_STEREO_OUT` (default 1): on a stereo `f32` device, the mixer writes straight
  into the device buffer, clipped in place (same blocks of 8,192 frames). Output stage alone
  (`engine::tests::stereo_output_bench`, empty mixer): 0.48 → 0.20 ns per frame (median ratio
  0.42), about 13 µs saved per second of sound.
- Multi-core, new code (rayon, 247 voices over k mixers): 411 / 355 / 294 / 359 / 328 µs per block
  for 1 / 2 / 4 / 8 / 16 mixers, worst blocks 20 to 30 ms (shared pool). Still ruled out for the
  real-time output thread (decision of C4-M23).
- GPU: still ruled out (10 ms blocks, a GPU round trip costs about a ms, 4.6 % of a core at 247
  voices).
- Leads left: constant gains over a block (static listener) would let the gain ramp go and the
  output add vectorise; AVX2 gathers (`vpgatherdd`, two per row pair in mono) would replace the
  per-point loads but need raw pointers.

### Mobs: id/UUID/species index, push and dimensions (C6-M07, 22/10/2026, loaded machine)

Integrated server, `rc-sim`. "Loaded machine" bench figures (other compilations running);
final decision on the scene bench (crowd, village, scale 400/1,600, Nether, zoo-end, zoo-ocean).
- `RUSTCRAFT_OPT_MOBINDEX`: id → slot and UUID → slot index, verified at every read (fallback =
  the previous sweep). Micro-bench 10⁶ present lookups: ×10 (100 mobs), ×43 to ×85 (1,000),
  ×300 (5,000); absent ones unchanged (sweep), but 165 absent for ≈ 60,000 served on
  `bench_ticks` + `bench_crowd`: no fast path for the absent (it would require every mutation
  to go through the API).
- `RUSTCRAFT_OPT_PUSHFILTER`: exact prefilter of the boxes by static bound of the species before the
  computation of the box (push and 72 other `bbox().intersects` sites). Neighbours micro-bench:
  1,600 mobs 63 → 22 ms per tick (100: 0.197 → 0.058 ms).
- `RUSTCRAFT_OPT_DIMSONCE`: width and height in one pass; micro-bench 63 → 42 ms (1,600, alone).
- `RUSTCRAFT_OPT_PUSHIDX`: the push keeps the slot read (verified) instead of a search.
- `RUSTCRAFT_OPT_KINDINDEX`: species → slots in list order (`Mobs::of_kind`), first
  client `shulker_free`; micro-bench 1% of the species: ×8 (100), ×67 (1,000), ×163 (5,000).
- `tests_opt::bench_crowd` (full tick, 2 passes, median ms): 100 mobs 1.0–1.8 → 0.6–0.8;
  400: 7.0–9.3 → 3.2–3.4; 1,600: 64.7–65.3 → 26.7–32.7. Alone: PUSHFILTER carries the bulk
  (1,600: 26.6–31.1), DIMSONCE 52–55, MOBINDEX and PUSHIDX within noise at this scale.

### Mobs: several species in one query, index refreshed from the first removed slot (C6-M03, 03/11/2026, loaded machine)

Integrated server, `rc-sim`, 1 core (server thread; sequential structure of a few µs: no
multi-core or GPU lead). Micro-benches `mobs::tests_index::bench_of_kinds` /
`bench_tail_refresh` (`--ignored --nocapture --test-threads=1`), other agents compiling;
final decision on the scene bench (crowd-1000, tnt, enclos-bebes).
- `Mobs::of_kinds([Kind; N])` and `of_kinds_mut` (switch `RUSTCRAFT_OPT_KINDINDEX`, extended):
  the species' slot lists merged by smallest head; `of_kind` is `of_kinds([k])` with a
  one-species shortcut. 1,000 mobs, ns per query: species absent, scan 802-1,126, old
  `of_kind` 3.1-5.3, new 4.8-5.6 (the 2 ns left in a test build are the test-only check:
  without it 4.8 against 5.3); 1 % present, old 11.8-15.8, new 13.3-15.5; three species merged
  8-21 ns absent (scan 848-1,274) and 115-172 ns with 2 % present (scan 855-1,665). Without
  the one-species shortcut and with `Option`/`flatten` the single species cost 14-16 ns: fixed
  before delivery. Callers moved: zombie and same-species alerts (both passes), anger reset
  alert, polar bear, panda and zombified piglin alerts.
- Fallback count (test counters `index::stats::kind_queries` and `refreshes`, thread-local):
  world test `mobs::tests_index_world`, 120 mobs, `/kill` of the last 15 pigs (first removed
  slot 90 of 120): 1 refresh by tail re-index (asserted), then a zombie alert: 2 queries served,
  0 sent to the scan; TNT in the crowd (whole rebuild with this draw: the blast reaches early
  slots) then an alert: 2 served, 0 to the scan. The test's own final check is read after the
  counts. (A first version killed the 30 furthest pigs: slot 0 among them, so a whole rebuild,
  and its 5 queries were all made before the removals or by the test itself.)
  Births go through `Mobs::push`, which keeps the per-species index in step: the only server
  mutation outside it is the portal removal (marks the index stale until the end of the tick).
  `bench_ticks` now prints the count.
- `RUSTCRAFT_OPT_INDEXTAIL`: after the end-of-tick removals (`Mobs::drop_empty_slots`) and the
  rank sort of a rejoin (`Mobs::sort_by_rank`), only the slots from the first moved one are
  re-indexed. µs per refresh, rebuild against tail: 1,000 mobs, one mob near the end 13.7-17.3
  → 2.4-3.2, last ten 13.6-18.8 → 1.5-2.5; 5,000 mobs 98-102 → 15, 105-135 → 10-13. A tail
  longer than half the list goes to the whole rebuild: re-indexed one entry at a time it cost
  more (half removed from slot 1 on: 19.1 against 6.8 µs at 1,000, 126 against 49 at 5,000),
  now equal (8.4 / 41.0). Removal from slot 0: equal (whole rebuild). The tail path also
  checks that the index saw every mob of the old list (length kept whatever
  `RUSTCRAFT_OPT_KINDINDEX`), and rebuild and tail share one insertion loop; re-measured after
  that change: 1,000 mobs 13.4 → 2.1 and 13.7 → 1.3, 5,000 mobs 73.7 → 10.5 and 72.5 → 6.0.

### Palette container `rc_world::Container` (C5-M04, 22/10/2026)

Criterion micro-bench `cargo bench -p rc-world --bench container`, 1 core, **loaded machine**
(other agents compiling: ±15% differences between two passes); final decision on the scene
bench (flight, crowd, redstone, plains32, forest, village, cave, nether, pause). Switches read
once (`RUSTCRAFT_OPT_*=0` restores the old path, `=1` forces). Proofs: tests of `container.rs`
(old code kept as `*_reference`, bit-for-bit comparison, sabotages verified).

| Item | Switch (default) | Before | After |
|---|---|---|---|
| `pack` 12 / 50 / 256 / 5000 scattered values, layered terrain | `PACK_TABLE` + `PACK_RUN` (1, 1) | 47.8 / 64.2 / 246.7 / 9.2 / 27.9 µs | 5.7 / 11.6 / 8.4 / 2.7 / 5.4 µs (×4 to ×30) |
| `pack`, table alone / run alone | (measurement of each lead alone) | — | table 17.6 / 8.9 / 19.5 / 4.0 / 11.8 µs; run 25.6 / 32.2 / 75.4 / 4.4 / 4.7 µs |
| `pack`, all-core throughput (50 values, 1→32 threads) | same | 67.9 → 5.2 µs/section (16 threads) | 6.7 → 1.3 µs/section (16 threads); beyond 16 threads, no more gain (SMT) |
| `get` of a whole section (12 / 50 / 5000) | `GET_MAGIC` (1) | 27.9 / 27.8 / 24.3 µs | 13.0 / 12.4 / 10.1 µs; flag test under the noise (dispatched `get` 11.0 µs) |
| `get` in dependent reads (latency, 4 / 5 / 7 / 15 bits) | `GET_MAGIC` (1), `CONTAINERDIV` (1, used if `GET_MAGIC=0`) | 29.7 / 28.6 / 29.0 / 24.4 µs | table 17.5 / 17.7 / 17.6 / 17–21 µs; constant 13.6 / 16.8 / 16.7 / 16.6 µs |
| `unpack_into` (12 / 50 / 5000) | `UNPACK_MONO` (1) | 2.69 / 2.67 / 2.47 µs | 0.85 / 1.21 / 0.63 µs |
| count of non-empty (`count_where` against unfolding + filter) | none (new function, wired by C5-M05) | 2.17 / 1.97 / 2.11 µs | 1.30 / 1.25 / 2.05 µs |
| vertical stack of 16 (`column_iter` against 16 `get`) | none (consumer C2-M10) | 12.1 / 13.4 µs | 7.8 / 8.3 µs (256 stacks) |
| `set`: palette that grows / that overflows (repack) | `SET_CAP` (1) | 168 ns / 12.2 µs | 85 ns / 9.3 µs |
| `set`: full palette widened without unfolding | `REPACK_WIDEN` (0, scene bench to do) | 12.2 µs (repack) | 4.6 µs |

`get`: before, two variable divisions per read (`divb` for 64/bits, `divq`/`divl` for
the index; verified in assembly). 64-entry table (`vpl`, multiplier ⌈2²⁴/vpl⌉, mask):
exact for every index < 4096 (exhaustive test 1..=32 bits; multiplier rounded down =
failure). The constant version (dispatch by width) wins in latency, the table in throughput and in
code size: table by default, the other on the scene bench (`RUSTCRAFT_OPT_GET_MAGIC=0`).
`REPACK_WIDEN` was at 0 (C5-M04): the in-memory palette keeps its unused entries and its order
of addition (non-canonical). I-04 (24/10) made it safe for the output: disk and LAN recompute a
canonical palette without depending on the in-memory palette; proofs `chunk::disk_encode_tests::widen_keeps_disk_bytes`
(NBT identical WIDEN 0/1 after 3,000 writes, widening actually served) and `tests_opt`
fingerprints identical with `RUSTCRAFT_OPT_REPACK_WIDEN=0` and `=1`. It STAYS AT 0 by default: without
widening, a full palette is recompacted on the values present only; with it, it
goes up to 8 bits keeping every state already seen (redstone sections, farms). The palette
prefilters (clear sky of the light, POI search, two portal prefilters) then answer
"maybe" more often and walk the 4,096 blocks, and the linear search of `set`
gets longer (up to 256 entries). Gain of 7.6 µs per overflowing `set` (micro-bench, loaded
machine) against an unmeasured cost on the light and the POIs: to settle on the quiet scene bench
(redstone, crowd, plains32), light and POI cost included, with `RUSTCRAFT_OPT_REPACK_WIDEN=1`.

### LAN and disk container encoders without unfolding (I-04, 24/10/2026)

Micro-benches `cargo test --release -p rc-sim lan::encode::container_tests::bench` and
`chunk::disk_encode_tests::bench -- --ignored --nocapture`, **loaded machine**, best of 5,
µs per block section (90 sections: terrain, 2 to 1,000 values, palettes left by
writes with and without widening, out of palette, direct). New function
`Container::first_seen_indices`: canonical palette drawn from the raw indices (table by raw
index, one lookup per entry encountered), fallback to unfolding + a single pass of `pack` (inverse
table) for direct containers or > 8 bits.

| Lead (switch, default) | 1 thread | 32 threads | Note |
|---|---|---|---|
| LAN old (`LANREMAP=0`) | 30.3 (palettes ≤ 16: 14.2) | 1.91 | unfolding + linear search per value |
| LAN raw indices (`LANREMAP`, 1) | 5.30 (4.96) | 0.42 | same bytes (`remap_writes_the_reference_bytes`) |
| LAN in-memory palette written as is (`LANRAWPAL`, 0) | 1.84 (3.02) | 0.10 | different bytes, same decoded values; guards: indices < palette size, width = palette's network width, direct = global width |
| Disk old (`DISKREMAP=0`) | 19.4 | 1.60 | unfolding + new hash table (same pattern, B6) |
| Disk raw indices (`DISKREMAP`, 1) | 16.0 | 1.36 | same NBT (`remap_writes_the_reference_nbt`); −17% only: the remaining cost is building the NBT |

`LANRAWPAL` stays at 0: it changes the packet's bytes (non-canonical in-memory palette, unused
entries) for ×1.6 on short palettes beyond `LANREMAP`; to settle on the scene
bench (plains32 entry of a local-network client). No test compares the LAN packets with
a capture: the byte proof is relative to the old encoder, kept as `container_reference`.

Resource cache (C4-M05, 22/10, micro-benches `cargo test --release -p rc-assets cache::tests::real`
on the real 122 MB cache, LOADED machine bench, system allocator of the test, 1 thread, best of 5;
final decision on the scene bench, 1st frame of the 13 scenes and first launch, to do on a quiet
machine). Switches `RUSTCRAFT_OPT_<NAME>=0` = old path, all at 1 by default:

| Lead (switch) | Before | After | Note |
|---|---|---|---|
| `[T; N]` without a heap array (`CODECARRAY`) | blocks 36.5 ms / 922,335 alloc.; items 41.6 ms / 1,155,067 | blocks 13.7 ms / 122,225; items 13.9 ms / 8,592 | wins alone |
| Numbers in bulk, lists and arrays (`CODECBULK`) | fonts 2.40 ms | 1.04 ms; blocks 16.4; items 15.0 alone | combined with arrays: blocks 12.4, items 13.6 (best combination) |
| Both, 4 sections in series / 4 threads (decoding + freeing) | 95.3 / 83.2 ms | 74.3 / 59.6 ms | strong noise (other compilations); section parallelism is another mission |
| Reading blocks ≥ 1 MiB without zeroing (`CACHEREAD`) | 15.4 ms (83.7 MB, 11 blocks) | 13.3 ms | best of 6 alternating passes; positioned read on the open descriptor (re-measured: 17.9 → 12.9 ms, loaded machine) |
| Streaming write (`CACHESTREAM`) | 840 ms, peak +151.5 MB | 782 ms, peak +24.1 MB | identical bytes (real cache rewritten: identical to the original except the key) |
| Split cache (`CACHESPLIT`) | recompilation = 122 MB rewritten + 2,207 PNGs decoded | sections alone (≈ 62 MB) | `files-*.bin` snapshot kept, key without the executable; to measure: first-launch scene |
| Housekeeping as a true LRU (`CACHELRU`), write thread below normal + PNGs on ≤ 4 threads (`CACHELOWPRIO`) | — | — | effect expected on the 1st second of cold play, to measure on the bench |
| Baking view (`RECORDLOCAL`) | global counter: another thread reading during a baking is lost for the snapshot | view of its own to the computation | fix (prerequisite to the parallel startup), zero cost |

Not done: the `to_vec` copy of the decoded section images (≈ 50 MB, of the order of 0.1 ms/MB)
requires shared pixels (`Image` owns a `Vec`); LZ4 compression of the blobs (new
dependency, balance probably negative when warm) left for after the scene bench.

Item models (C4-M12, 22/10, `crates/rc-assets/src/item.rs`, micro-benches
`cargo test --release -p rc-assets --lib item::tests_opt -- --nocapture --test-threads=1` on the installed
26.3 jar, 1,658 definitions, LOADED machine bench, system allocator of the test, 1 thread, best
of 5; final decision on the building, village, crowd, inventory scene bench, quiet machine).
Switches `RUSTCRAFT_OPT_<NAME>=0` = old path, at 1 by default, except `ITEMENVFLAG` (off by
default, `=1` enables it):

| Lead (switch) | Before | After | Note |
|---|---|---|---|
| Allocation-free evaluation (`ITEMEVAL`): borrowed `select`, `norm_eq` by slicing, inline tints (≤ 4), short id composed on the stack (also `swap_animation`, every frame and every hand) | pass 1: 60.9 / 56.7 ns per evaluation; pass 2: 33.9 / 34.8 ns; 0.100 alloc. | pass 1: 48.8 / 49.7 ns; pass 2: 34.3 / 34.4 ns (noise); 0.033 alloc. | pass = all the definitions × (GUI slot, left hand) × (new stack, loaded stack); the remaining allocations come from the stack methods that return `String`/`Vec` (`trim_material`, `special_data`…), outside this file |
| "Environment read" flag (`ITEMENVFLAG`), OFF by default | `layers()` without envelopes: 45.7 / 46.4 / 45.6 ns per call | with envelopes: 47.0 / 48.8 / 47.9 ns (+1.3 to +3.2 ns, ≈ 4%, 3 alternating pairs, `bench_environment_flag`); 1,622 stable definitions, 36 dependent | `layers` returns the flag (off: always true); envelopes of the environment (14 reads) and of the stack (use, chosen slot); `generation()` new at every load or re-read. Prerequisite to the slot caches C4-M17/M19: no code reads the flag today, hence cost without gain; to re-enable (default 1) with the first consumer |
| Quads in columns in the cache (`ITEMQUADCOL`) | item section 12.4 MB decoded in 10.55 / 9.30 ms | 5.69 / 4.67 ms, same 8,592 alloc. | alone (pixel layers off) |
| Extruded layers in pixels (`ITEMQUADPACK`), unfolded at the 1st request | 102,706 quads × 92 B, 12.13 MB of heap | 1.42 MB as long as nothing is requested (1,260 / 2,109 models stored); section 4.6 MB decoded in 1.64 / 1.56 ms | unfolding ALL the models: 2.35 ms (102,706 quads, new re-read); each model held or displayed is only unfolded once |

Quad shape written at the head of each model (0 rows, 1 columns, 2 pixels): any shape is
re-read whatever the switch and rewritten identically; cache `FORMAT` bumped to 3.
`ITEMQUADCOL` and `ITEMQUADPACK` enter the key of the baked sections (`sections_key_text`):
each setting has its own cache file, so a warm A/B with one of them at 0 does measure
the old path (first launch of each setting = cold startup, to discard before the measurement).
Measured difference zero: quads, transforms and layers compared field by field (tests above),
plus four stored fingerprints (layers and keys of the trial definitions and of the jar, quads of the jar,
extrusion) that do not depend on the old path.

### Light of a new chunk (`light_chunk`, C2-M08, 22/10/2026, loaded machine)

Bench `cargo bench -p rc-light --bench light_chunk` (capture, 1,248 columns, all verified
first against the frozen copy, on-the-fly and cached scans): switch matrix in a
single process, 7 alternating passes, rotated order, minimum per variant (from one process to another,
the reference varied from 149 to 256 ms: cross-process comparison unusable on a loaded bench).
Switches `RUSTCRAFT_OPT_<NAME>=0` = old path, all at 1 by default.

| Variant | 64 columns, 1 thread | 256 columns, 32 threads |
|---|---|---|
| old (all at 0) | 152.0 ms (2.4 ms/chunk) | 53.2 ms |
| `FULLSKIP` (empty skipped above the highest non-neutral cell + 15) | 94.4 | 28.2 |
| `FULLLAYOUT` (per-thread buffers, emitters by rows, sheets 2 cells/byte) | 119.0 | 39.9 |
| `FULLSKIP` + `FULLLAYOUT` (without scan: mushroom generation, new world) | 100.5 | 26.2 |
| cached scans (`FULLCACHE`) alone / + `FULLSKIP` / + `FULLNOEMIT` | 86.9 (with LAYOUT) / 61.9 / 52.3 | 26.5 / 21.3 / 15.9 |
| all (`SKIP` + `NOEMIT` + `LAYOUT` + scans) | **49.0 (0.77 ms/chunk, ÷3.1)** | **15.8 (÷3.4)** |
| scan of a column (`scan_column`, once per chunk and per batch) | 3.8–4.9 ms / 64 = 0.06–0.08 ms | — |

`SKIP` alone and `SKIP` + `LAYOUT` without scan are within noise of each other at 1 thread (94 / 100 ms),
`LAYOUT` wins on all threads (28.2 → 26.2) and with scans (61.9 → 55.4): kept. `FULLNOEMIT` only acts
with scans (mask of the sections with an emitter). Memory: per-thread buffers ≤ 4 MiB (≈ 1 MiB for
384 high), scan ≈ 1.1 KiB per column and per batch (≤ 100 columns per batch). Generation:
batch scans done in parallel after the post-processing (`RUSTCRAFT_GENLOG` counts new
chunks, relit neighbours and scanned columns); in tests, each lighting with scans is compared
with the one without a scan (cache staleness = failure). New world (`NEWWORLDLIGHT`): columns of
the full chunks unfolded once per position, the generator's ones borrowed (no more 9 clones of
393 KiB per chunk), lightings in parallel; gain to measure on the scene bench.

Ruled out or not done here (C3: to take up again): union of the batch in a single propagation (`FULLUNION`) and
per-row bit masks (`FULLBITS`) not implemented; W = 46 not studied; post-processing as a
graph, reuse of the mushrooms' light, speculation and generation pipeline (leads
merged from C5-M08) not done. GPU ruled out on paper: `rc-sim` stays without a device
(dedicated server), sending the 9 columns (≈ 3.5 MiB per chunk) would dominate a 0.77 ms computation,
same reason as the section communication (6.2 ms GPU against 1.9 ms CPU). Final decision:
flight, crowd, redstone (startup), plains32, cave scene bench, quiet machine.

### Visual environment: allocation-free evaluation, fog and world reads (C5-M27, 22/10/2026)

Micro-bench `cargo-file.ps1 test --release -p rc-env --test noalloc -- --ignored --nocapture`, 1 core
(sequential work, once per frame: neither multi-core nor GPU relevant), **loaded machine**,
3 passes. "Frame" = fog + 5 probe reads + 3 global reads (what `environ` reads).

| Combination | `eval_global` | `eval` (2 biomes) | frame | allocations/frame |
|---|---|---|---|---|
| old (all at 0) | 128–187 ns | 155–194 ns | 543–746 ns | 12 |
| `ENV_NOALLOC` alone | 22–30 ns | 28–37 ns | 168–244 ns | 0 |
| `ENV_KEYCACHE` alone | 5.5–6.2 ns | (unchanged) | 176–191 ns | 0 |
| `FOG_MEMO` alone | (unchanged) | (unchanged) | 685–1,021 ns (noise, dominated by allocations) | 12 |
| `NOALLOC` + `KEYCACHE` | 2.2–2.8 ns | 19–33 ns | 101–125 ns | 0 |
| all three (default) | 2.2–3.8 ns | 20–36 ns | 78–127 ns | 0 |

`FOG_MEMO` only shows once the allocations are removed: 101 → 78 ns per frame (−22%).
`RUSTCRAFT_OPT_ENVIRONCACHE` (client): biome blur zoom kept per (block, seed), 3×3×3 around the eye
without water, lava or snow kept per (block, stamps of the columns `ChunkGrid::stamp`); no isolated
micro-bench (real-world reads), to measure on the scene bench (plains12, nether, village, inventory,
pause: share of `environ::frame`, 0.15–0.43% before). Not done: `partition_point` on the keys of the
tracks (sorting not guaranteed: two keys at the same tick for the sun angle).
### Pistons: moving blocks in the collision searches (G5-F14, 22/10/2026)

LOADED machine (other agents compiling): indicative figures, to re-measure on a quiet machine.
Bench `rc-physics` `bench_collision_moving_check` (`collide::resolve` of a player walking on the
ground, best of 5 × 10⁶, two passes): before 261.8 / 263.1 ns; "moving block?" test
on every cell without a guard 267.2 / 271.0 ns (+2 to +3%, first measurement 253.3 → 263.9: +4%);
with a guard (test skipped when no block is moving: client `moving` counter, empty server
`movers` table) 257.0 / 281.8 ns (±10% noise, inconclusive); moving block next to
the player 293.0 / 296.5 ns (shape computed according to the progress, only during the 2 ticks of a
movement). Guard ADOPTED: same result proven (the `moving_piston` block without a block entity has
no shape, assertion in `player_pushed_by_moving_stone`), it avoids the work in the
common case. Client replay (`bench_replay`, output that pushes 12 blocks): 1.27 µs per event,
construction 5.7 ms at startup. Axes: client and server CPU (collision searches of
all the entities) only affected; no notable GPU, network, disk or RAM (one table per
moving piston); multi-core not applicable (2 ticks per movement). Scenes to measure:
`perf:redstone` (sticky pistons), spawn, crowd (mobs, collisions without a piston).

### Integrated server block entities (G5-F01, 22/10, loaded-machine bench)

Bench `bench_g5f01_pull_and_hopper` (`cargo test --release -p rc-sim bench_g5f01 -- --ignored
--nocapture`, 20,000 iterations, trial profile). Other agents compiling: indicative figures,
to redo on a quiet machine.

| Item | Measurement | Decision |
|---|---|---|
| Re-reading the open menu from its source (full double chest, 54 stacks), per call | 2.24–2.30 µs (3 passes); version before the re-read 2.35–2.39 µs (code unchanged: difference = noise) | kept: necessary for fidelity (a hopper or a command that changes the open container); ≈ 0.005% of a 50 ms tick, only with the menu open. Lead noted: version counter per container to skip an unchanged re-read (not measured, gain bounded by these 2.3 µs) |
| Simulated hopper → furnace transfer (insertion, slot per face, removal), per transfer | 2.57–2.63 µs against 2.70–2.78 µs before the re-read | kept (−5%): the "empty hopper destination" test no longer reads the slots of a destination that is not a hopper |
| Multi-core, GPU | not applicable | logic of a tick, sequential in ticker order (the order is observable) |

Advancement triggers (G1-F02, fidelity, 22/10/2026): `effects_changed` now starts from the
effect events (add, update, remove, decrease, refresh of the 600 ticks) instead of
a per-tick sweep that copied the list of effects (a string allocated per active effect at
every player tick): allocation per tick removed, work only when an effect changes.
Sliding on honey reuses the block box of `enter_block` and only walks it on the
game ticks that are multiples of 20, player in the air, criterion active: zero cost the rest of the time. Gain
not quantified on a scene (series of agents in progress, loaded machine); no hot spot touched.

Sleep, beds, signs, multiplayer spawns (G6-F11, fidelity, 24/10/2026): no hot
spot. Sleepers list kept up to date only on changes (lying down, getting up, arrival, departure,
game mode), like the game: the night skip no longer recounts the sleeping players at every tick
(only deep sleep is counted, and only when enough players are in bed: zero
work per tick when nobody sleeps). Bed rule (dimension JSON attribute) read at
every tick of the sleeping player only (a few table lookups). Phantom and patrol
spawner: a state exchange per guest every 60 to 120 s (phantoms); nothing per tick.
Spectator with `spectators_generate_chunks` false: the send queue is re-sorted at every tick
while it waits (rare case, bounded by the view area). Block destruction: one more name comparison
(hay bale particles), negligible next to the block write. Multi-core,
GPU: not applicable (sequential logic of a tick, observable order).

### Fidelity fixes G2-F07 (22/10/2026, loaded-machine bench)

- Targeting and outline: shape according to the held item + offset of the blocks whose shape follows the model
  (`crack::shape_shift`). Cost per block crossed (≈ 10 per frame): 15.7 ns (flower), 9.1 ns (offset grass with a fixed shape), i.e. < 0.2 µs per
frame (bench `bench_shape_shift`, 10⁶ calls). Fast path
  (block without offset): a byte test. Lead ruled out: precomputed table per state (gain
  not measurable at 10 calls per frame, +2 bytes × states of RAM).
- Attack targeting (`attack_range`): candidates walked a second time only if the active
  item has the component; no allocation (replayed iterator).
- Labels: background colors recomputed only when the option changes (cache emptied then).
- End of loading: one hash table read (`born`) per tick during the screen only.
- None of these items is a hot spot (B4 multi-core / GPU not applicable: a few calls per
  frame on the main thread); scenes to re-measure on a quiet machine: spawn, plains32.

### Block particle tints (fidelity G2-F10, 22/10/2026, loaded machine)

Tint of the block pieces and of the falling dust read per state (client extractor); biome
tint resolved at entry into the queue, 5 × 5 blend like the meshing, reusing the color of
the previous particle if same kind and same block (one break = 64 pieces of the same block).
Bench `particles::tests::bench_piece_tint` (200 breaks, best of 5): fixed tint
4.1 µs per break, biome tint 5.6 µs with the reuse, 169 µs without (×30). Main thread (the engine
tick): 1.5 µs per break justifies neither a pool nor the GPU. Memory: per-state tables 0.57 MB
(tint and dust, 8 bytes per state) + 0.1 MB × 2 (noxious gas); 4-byte packing not
retained (−0.29 MB, no CPU effect). Cold resource loading: average of the block
textures removed (no longer used), evaluation of the item models for the shards added (not
measured: no client launched during the series).

### Mining and breaking on the integrated server side (G3-F02, 22/10/2026)

Fidelity fix, no hot loop added. Cost per mining tick: one block `get`,
one gain computation (already done before) and at most one world event; block hits, block
choice, client information, spectator action, break reactions: once per player
packet. Crack for the other players: nothing in singleplayer (closed world: immediate return), one
shared encoding per packet on the local network. Upward push (snow that thickens, tilled
dirt that turns back to dirt): walk of the entity list only when the shape grows,
at the pace of the snow (columns drawn at random per chunk). Leads ruled out without measurement
(nothing to gain: < 1 µs per tick at worst, under the bench noise): spatial index of the entities for
the push (the full list is already walked elsewhere at every tick), cache of the block names
of the hit. Scenes to re-measure on a quiet machine: none mines; `village` and `plains32` under
snow for the push.

### Integrated server minecarts (G5-F06, 22/10/2026, loaded machine)

Bench `minecart::tests::bench_minecarts` (ignored, `-- --ignored bench_minecarts --nocapture`,
test world required), trial profile, other agents were compiling: indicative figures.

| Measurement | Value |
|---|---|
| Tick of 64 chest minecarts moving on a track | 30.7 µs (≈ 0.48 µs per minecart) |
| Search for a container minecart by a hopper, 64 minecarts, none touched | 137 ns |
| Same, one minecart touched (random draw included) | 134 ns |

- Search by the hopper: called at every tick by every hopper without a neighbouring block container;
  two allocation-free passes (count, take the n-th) instead of a list; difference with the
  list not measured separately (one allocation fewer when a minecart is found).
- Placement: integrated server thread, sequential like the game (minecart order, random
  draws shared with the world); neither multi-core nor GPU (a few entities, order dependencies).
- To re-measure on a quiet machine: minecart index per chunk if a scene with hundreds of
  minecarts shows the hopper search or the pushes between minecarts (linear sweep).
- Block effects (review G5-F06, 22/10, loaded machine): called after each movement in
  addition to the tick, like the game. First version: 46.0 µs for the bench above; the double
  rain search (only useful to a burning minecart) and the freeze read (only useful if a
  fire or lava makes it stop) skipped without changing the result: 24.5 / 24.4 µs (two
  passes). Figures from different sessions are not comparable (load): A/B to redo on a
  quiet machine against `main`.

### Beacon, crafter and recipe book (G5-F11, 23/10/2026, loaded machine)

Micro-benches `tests_g5f11::bench_beacon_tick` and `bench_recipe_book` (ignored, `-- --ignored
bench_ --nocapture`, new flat world), trial profile, other agents were compiling: indicative.

| Measurement | Value |
|---|---|
| Tick of a beacon with a 4-level pyramid, off a multiple of 80 (column examination) | 33.6 / 35.3 ns |
| Same tick at a multiple of 80 (164-block pyramid, effects, sound) | 2,069 / 2,210 ns, i.e. ≈ 27 ns per tick on average |
| Full book (1,746 entries, all recipes known) | 2.65 / 2.94 ms |
| Group number, old computation (list of groups sorted at every entry) | 20.5 / 42.5 ms per full book |
| Group number precomputed at recipe loading | 0.004 ms per full book |

- Beacon: the state kept by the server is reduced to four values (open section, beam of the
  last column, examined height, levels) instead of the game's colored sections, which are only used
  for rendering (computed by the client); the label of the base blocks is looked up once per
  recount, then a binary search per block. The item is not hot (≈ 60 ns per
  beacon and per tick, average included): neither multi-core nor GPU (a few beacons per world,
  ticker order and effects on players to keep); no other lead measured, to take up again if
  a scene with many beacons appears.
- Book: group numbers and recipe index by id precomputed at loading (the linear
  search of `index` was used at every entry, at every recipe learned); gain of the
  index not separable from this bench's noise (2.65 → 2.94 ms, more loaded session), kept (same
  result, O(1)). The rest of the book is re-reading the recipe JSON and encoding the stacks.
- Crafter: grid re-read at every tick like the other containers; the displayed result is only
  recomputed if the grid changed (the game recomputes it at every slot change of the menu,
  player inventory included: same result).
- Scenes to measure on a quiet machine: no bench scene contains a beacon or a crafter;
  entering the game (sending the book) on a world where many recipes are known.

### Server-side decorations (paintings, frames, armor stands, cushions, mannequins; 22/10/2026)

The NBT compound remains the truth; a light registry (id, kind, last seen chunk, survival
counter, last hit) avoids re-reading the entities. Tick: empty registry → immediate
exit; paintings, frames, cushions: one counter per tick, survival check (read of a
few blocks) every 101 ticks, like the game. Armor stands and mannequins: fall collision
per tick (like the game), fields read in the compound without cloning it, search first in the
known chunk (before: sweep of all the world's entities and clone of the compound, equipment
included, at every tick and for every stand). Single-threaded: a few dozen entries at most per
world, nothing to parallelize or port to the GPU; the "armor stand room" scene remains to be
measured on a quiet machine (scene bench not launched during the series).
Follow-up (22/10): survival only when the 8 neighbouring chunks are loaded (9 table reads
per entry and per tick, instead of one). Comparator through a conductor: search for the frame
in the registry, filtered by chunk (±1) before any compound read; empty registry → null
loop. Leads ruled out without measurement: spatial index of the frames (a few dozen entries at most,
the filtered loop costs less than a table to keep up to date at every placement and break).

### Projectiles against portals (G4-F05, 22/10/2026)

Each projectile checks, at every tick, the portals touched along its path (game rule:
cells swept between the old and the new position). Cost added per living projectile, bench
`portal_tests::bench_portal_sweep` (loaded machine, 1 core, 3-block path):

| Case | Cost per call |
|---|---|
| Far from any portal (common case) | 63 ns |
| Section containing a portal, path beside it | 1,103 ns |
| Path through the portal | 1,151 ns |

Decision: prefilter kept. The common case only reads the palette of the sections covered by the
path's box (single value or state list), without reading a block; the exact cell-by-cell
sweep only runs if a section may contain a portal. Leads ruled out: per-tick cache of the
"portal-free" sections (the prefilter already costs 63 ns per projectile; for a few dozen
projectiles, less than 2 µs per tick, under the noise of a tick),
multi-core and GPU (one call per projectile and per tick, work of the order of a microsecond,
order of the marks dictated by the tick: the distribution overhead exceeds the computation). Placement:
integrated server thread, like the rest of the entity tick. Scenes to re-measure on a quiet machine:
bow shooting and TNT near a Nether portal.

### Mob fidelity fixes (G2-F02, 22/10/2026, loaded-machine micro-bench)

New work added by the fidelity, quantified by `bench_water_and_cat_track` (`animals.rs`,
`--ignored --nocapture`, 2,001 entities: 1,000 equids in the water, 500 cows, 500 cats lying down
with no sleeping player). Machine loaded by other compilations: ×2 differences from one pass to the next.

| Building block | Variant | Result | Decision |
|---|---|---|---|
| Water contact (frog, turtle, equids) | at the tick, before the entity tick (like the game) rather than at every frame | 20 times per second instead of every frame (up to 2,400 fps on the bench) | adopted (and more faithful: the game's state is per tick) |
| Water contact | water fluid recognized by a bit mask computed once per call instead of a `contains("water")` per block read | 34.3 µs/tick → 11.9 to 25.9 µs/tick (3 passes) | adopted; also benefits the swimming player (same function) |
| Water contact | spread over several cores | not measured: 12 to 26 ns per entity, launching a pool task costs more than the loop | ruled out; GPU not applicable (a few block reads) |
| Cat lying on a sleeping player | sleeping players collected first, no walk over the cats nor allocation without a sleeper | 3.8 to 6.3 µs/tick for 2,001 entities (kind comparison per entity) | adopted |
| Swallowed content of the sulfur cube | cache (item, fingerprint of the `block_state` component) → skull or state; component id precomputed | per frame: one component lookup and one hash table; names, skulls and states read at the first pass | adopted (no bench: a carrying sulfur cube is rare) |

### Allay dance and parrot party near a jukebox (fidelity G4-F10, 23/10/2026, loaded machine)

Benches ignored by default: `bench_allays_hear_jukebox` (`rc-sim`, `blocks/g4f10_tests.rs`, 510 mobs of
which 10 allays) and `bench_party_track` (`rc-app`, `animals.rs`, 2,001 entities of which one marked parrot).

| Building block | Variant | Result | Decision |
|---|---|---|---|
| Allays' jukebox listener (server) | one walk of the mob list per `jukebox_play` / `jukebox_stop_play` event (every 20 ticks per playing jukebox), integer test on the eyes, no allocation | 472 ns per event for 510 mobs, i.e. ≈ 24 ns/tick per jukebox | adopted; registering the allays in the vibration section registry would avoid the walk but would change the delivery order (immediate in the game) for a gain under a microsecond: ruled out, to re-measure if a jukebox scene enters the bench |
| Parrot imitation (server) | count then n-th candidate (two walks, no allocation) instead of a `Vec` of candidates | 1 draw per 800 ticks per parrot; allocation removed | adopted (no bench: rare call) |
| Jukebox forgetting (client, `party_track`) | one `Option` test per entity and per tick, distance and block only for a marked parrot | 2.09 µs/tick for 2,001 entities | adopted; a counter of marked parrots to skip the walk (gain ≤ 2 µs/tick) ruled out, figure noted for re-measurement |
| Marking 1010 / 1011 (client, `jukebox_notify`) | entity walk, box computed for the parrots only | 12.0 µs per event (rare: start and end of a track) | adopted |

Multi-core and GPU not applicable (a few nanoseconds per entity, order imposed by the shared state).
Party pose: a few sines per partying parrot and per frame, like the other poses.

### Scoreboard: player criteria and entity cleanup (fidelity G3-F12, 22/10/2026)

New code on the integrated server thread, off the hot path: decision by reading the code, no scene
bench (no scene creates an objective). Criteria `health/food/air/armor/xp/level`: 6 comparisons
and one attribute read (armor) per player tick, state kept even without an objective (the game only writes
on change: a suspended tracking would change the result). Bench `criteria_tick_cost` (ignored by
default, loaded machine, 3 passes): 20.0 ns per tick without an objective; the first version also evaluated
armor toughness (+18.5 ns, i.e. ≈ 38.5 ns), removed. Each hook (statistic, mob
death, entity removal, criterion) exits before any allocation when the board is empty
(`objectives.is_empty()`, `has_entries()`), which also removes the old formatting of the criterion
name at every statistic awarded. Multi-core and GPU not applicable (a few bytes per tick,
packet order imposed). To re-measure on a quiet machine if a scene with objectives is added to the bench.

### Projectile impacts (fidelity G4-F04, 23/10/2026)

New code on the integrated server thread, off the hot path: decision by reading the code, no scene
bench (no bench scene fires a projectile). Each fix only costs at impact:
last victim = one write; enderman = one species comparison; water potion = one more pass over the
mob list (the same as the search for the living ones hit, at impact only);
rocket against a block = one state name read. Only per-tick addition: the known speed of the
shulker bullet (3 subtractions, 32 bytes in its state already allocated separately, never on the other
projectiles). Gain kept: the bullet's state is no longer taken out of its box then reallocated at every
tick (one allocation and one free fewer per bullet and per tick). Honey slide shared
(`rc_physics::honey_slide`) between the player, the engine entities and the rockets: same
computation bit for bit (equivalence test on 1,260 cases). Multi-core and GPU not applicable (a handful
of operations per impact, packet order imposed). No switch: nothing was ruled out.
### Item uses (fidelity G5-F10, 23/10/2026, loaded-machine micro-bench)

New code on the integrated server thread, off the hot path. Work added at every tick:
sweep of the 41 inventory slots looking for a tracked lodestone compass (reading the
component, nothing else without a compass), one tick per loaded End gateway (read and
write of the `Age` field), one tick per eye of ender in flight (movement without collision), the
recoil of brushed suspicious blocks (only if they have a brushing in progress, table by position).
End of cooldown: "is a cooldown due?" test without allocation before the list of ends.
Bench `tests_g5f10::bench_g5f10_ticks` (ignored, `--ignored --nocapture`): figures below.

| Measurement (2 alternating passes) | Value |
|---|---|
| Compasses: full inventory, no compass | 37 to 53 ns/tick |
| 36 compasses linked to the same lodestone, verdict of the last target kept | 896 / 896 ns/tick |
| Same, one POI search per compass (`RUSTCRAFT_OPT_NO_COMPASS_MEMO=1`) | 1,120 / 1,313 ns/tick |
| End gateway (one) | 82 to 87 ns/tick (148 at the first pass, more loaded machine) |
| Eye of ender in flight (one) | 49 to 52 ns/tick |

Decision: verdict of the last target kept during the sweep (−20 to −32%, the POIs do not
change between two slots), the old path behind its switch until the quiet-machine
bench (C6). The rest (≈ 25 ns per compass) is the reading of the component. Leads ruled out with no possible gain at this scale: "a
tracked compass exists" flag maintained at every slot change (the sweep costs less than a correct
tracking of all slot writes), multi-core and GPU (a few entities, tick order
imposed, work of the order of a microsecond). Placement: integrated server thread. Scenes to
re-measure on a quiet machine: no bench scene contains these items (End stronghold and gateway
island to add if a measurement shows them).

Follow-up after review (24/10, same bench, loaded machine, 2 passes): the sweep only reads the
component on compasses (fidelity: the game only tracks lodestones for this item);
inventory without a compass 26 / 35 ns/tick (37 to 53 before), 36 compasses 1,006 / 1,049 ns/tick
(896 before: one more id comparison per slot, difference within the noise of a loaded
machine, to re-measure on a quiet machine). Tracking of the item in use: one string comparison per
tick and only during a use, no allocation outside the start of the use. Gateway 88 / 89,
eye 54 / 49 ns/tick (unchanged).
### Targeting, interactions and tooltips (fidelity G5-F15, 23/10/2026)

Decision by reading the code (off the hot path, no scene measurement: loaded-machine series).
Interaction shape of the rays (`rc_physics::ray::interaction_shape`): looked up only once per
ray, at the block whose outline is hit (never in the walk over empty cells), by block name
(≤ 6 string comparisons) and `facing` property of the hopper; shapes as constants, no
allocation. A precomputed table per state ("avoid the work" leads) would only save these
few comparisons per ray (targeting: 1 ray per tick and per frame): not retained, nothing
measurable to gain. Targeting candidates extracted as an iterator (`pick_candidates`) without a `Vec`; the
brush redoes the targeting (blocks + entities) once per use tick, like the game. Predicted placement: the
item → block table moves from rc-sim to rc-data (same `match`); cooldowns tested once per click.
Tooltips: context (peaceful, operator) in one `AtomicU8` set per tick; type of the typed
data read as a varint; block search (op warning) only if a reserved datum is
present. Multi-core and GPU not applicable (work per click or per ray). Rendering: targeted face
changed on 4 block families (cauldrons, composter, scaffolding, hopper), matching the game.

### Hand, leash, elytra sounds and End flash (fidelity G2-F06, 24/10/2026)

Decision by reading the code (off the hot path, no scene measurement: loaded-machine series).
Hand selection (bow, crossbow): once per frame, four id comparisons; the
crossbow charge is only decoded if the real hand holds a crossbow (short-circuit), otherwise
no added work; the pieces of hidden hands are no longer built (fewer instances).
Leash in first person: one rotation per rope of the local player, instead of the model's;
vertical rope: a single-precision test, no vertex emitted (48 vertices fewer). Elytra:
one loop per glide pass, updated once per tick (≤ 2 loops, `retain_mut` without
allocation); End flash: one counter per tick, one sound per flash (≥ 400 ticks apart), one
movement per tick while it plays. "Hide flashes" option read once per tick (same
cost as the thunderstorm). Multi-core and GPU not applicable (a few operations per tick or per frame); no
switch: no competing lead, nothing to measure above the noise.

### Advancement and loot predicates (fidelity G1-F07, 23/10/2026)

Code evaluated only when a criterion, a loot condition or `execute if predicate` asks for it:
off the hot path, decision by reading the code, no scene bench (no scene evaluates these
predicates in a loop). Block-under-the-feet rule: a single function for mobs, player,
minecarts and predicates (same operations as before for the three copies, nothing more per tick);
the player computes its supporting block once for its two offsets. Smoke: the beehive rule
keeps a single box buffer for its 5 cells (at most 1 allocation instead of 5). `looking_at`:
the game's box-entry test (at most 3 divisions per entity, against 6 for the old slab method); the line of sight (a
ray cast in the blocks) only starts for the retained entity, after its predicate. NBT compared:
a `retain` on the head fields (without copy). Multi-core and GPU not applicable (an isolated
evaluation, a few microseconds at most). To measure if a bench scene evaluates predicates.
### Text input, IME and widgets (fidelity G1-F09, 23/10/2026, loaded-machine micro-bench)

Interface only (main thread, once per frame and per displayed field). End of the visible part
of a field: the old search redid a full measurement per added character (quadratic);
a single accumulation of the advances, in the same order as the measurement of a text (identical result checked by
the bench `widgets::bench_visible_widths`, ignored by default): 43.2 µs → 1.09 µs per 250-character
field. The same accumulation serves the click (position by real widths). IME: per-thread state (`ime.rs`,
lock-free), one write per focused field and per frame; the composition is only measured and drawn
during IME input. Restricted chat prompt: a cut to the width only if it overflows.
Clickable hover: search for the style under the mouse (open chat, book, death screen) once per
frame, already done for the tooltips. Multi-core and GPU not applicable (a few dozen glyphs).
To re-measure on a quiet machine: menu scenes (title, world list, open chat).
### Item predicates, components and command texts (fidelity G3-F11, 24/10/2026, loaded-machine micro-bench)

Command code (an isolated execution) and component writes. Item predicate: read once
per command then applied to each stack (`clear`, `execute if items`) instead of being re-read per stack;
bench `bench_g3f11` (ignored by default), 36 stacks: 39.3 µs re-read per stack → 10.1 µs read once.
Writing a component (`set_component`): comparison with the item's default added (fidelity: the
patch only keeps the differences); 11.1 ns → 143.6 ns per write (lock of the defaults cache, key of
the item, JSON/NBT comparison). Rare writes (wear, enchantment, a few per tick at most):
kept; lead noted to re-measure on a quiet machine if a scene writes components in a loop: defaults
pre-converted to NBT per item, lock-free read (`OnceLock` per item). `nbt`/`score`/
`selector` contents: resolved only when a text command is executed. Multi-core and GPU not applicable.
### NBT, SNBT and /data (G3-F10, 23/10, loaded machine)

Binary writing of lists: the element type is the common type (10 with wrappers if the list
is mixed), hence a type comparison pass per list; the chunks' palette is no longer
wrapped by hand (the writing does it without allocation). Bench `bench_list_writing` (rc-proto,
ignored by default, typical chunk of 24 sections and 40 entities, 2,000 writes, 2 passes):
old 68.8 / 67.3 µs, new 67.3 / 68.0 µs per chunk: equal within noise, identical bytes on
homogeneous lists (verified by the bench). Reading: one test per element of a list of compounds
(wrapper with an empty key), negligible. NBT paths: recursive walk in a single pass with
per-level counting (no intermediate list of references like the game), typed arrays read
by copying a number; `to_snbt` sorts the keys (UTF-16 unit sort without allocation): commands
and messages only, outside the game loop. Multi-core and GPU not applicable (work of a few
microseconds, order of the effects imposed). `\N{name}`: Unicode names table (crate `unicode_names2`,
≈ 500 KB of static data, nothing at run time outside this escape). To re-measure on a quiet machine
with the bench's save scene.
### NBT writing: strings without a buffer, arrays in one block, sized roots (C5-M10, 03/11, loaded machine)

Writer used by every save (chunk columns, `.dat`), the generator's proto-chunks and the LAN.
Four switches, default on, `=0` restores the old path:

- `RUSTCRAFT_OPT_NBT_STRING_FAST_PATH` (writer only): a string without a null character and
  without a character outside the base plane has the same bytes in UTF-8 and in modified UTF-8, so
  it is copied as it is; the others are encoded straight into the output after a 2-byte length
  patched at the end (back to an empty string past 65,535 bytes, like before). Old path: one
  temporary `Vec` per key and per value, then a copy.
- `RUSTCRAFT_OPT_NBT_STRING_READ_FAST_PATH` (reader, separate so each side is measured alone):
  valid UTF-8 is copied as it is when no byte reaches 0xF0, one byte test instead of the old
  two (0xC0 never occurs in valid UTF-8; 0xED there only leads hangul, decoded the same).
  Anything else goes to the unit decoder, which refuses 4-byte forms like the game's reader.
- `RUSTCRAFT_OPT_NBT_ARRAY_BULK`: int and long arrays written with one resize then a loop over
  fixed-size blocks (byte swaps vectorised), instead of one `extend_from_slice` per element.
- `RUSTCRAFT_OPT_NBT_SIZE_HINT`: `nbtio::RootEncoder` (save loop, generator batches, new world,
  cache erasing) starts each buffer at the size of the previous root plus an eighth, never below
  4 KiB, instead of 4 KiB grown by doubling. In the generator, `map_init` gives one encoder per
  rayon task, not per thread: rayon splits a batch into as many tasks as it likes and the first
  root of each starts at 4 KiB, so the hint helps less there than in the save loop. If the
  `generation` scene shows a gain worth more, one encoder per thread (`thread_local`, or
  `par_chunks` with an encoder per chunk) is the next step.

rc-proto now depends on rc-data, only for `env_str!` (clippy.toml forbids any other environment
read): rc-proto and rc-net compile after all of rc-data (HTTP client, zip, regex, rayon); the
executable does not change. Moving `env_flags` into a crate without dependencies would undo it.

Proof: `nbt::write_tests` (rc-proto): 100,000 generated strings (ASCII, nulls, Latin-1, hangul,
base plane, outside it, mixed) plus 11 boundary strings (0, 65,535, 65,536, 70,000 bytes,
65,535 UTF-8 bytes with a 4-byte character that overflows in modified UTF-8, nulls at the
limit, 21,845 and 21,846 hangul): same bytes as the old writer and round trip; frozen bytes for
both writers; 4,000 random arrays; whole trees with the 4 option combinations. Sabotages
checked: null test removed from the copy path (fails on the boundary and null strings), long
arrays in little-endian, length patched one byte off (both fail). rc-sim
`model_world_encodes_identically`: the 17,689 chunks of the model world (terrain, entities, POI;
131.6 MB) give the same bytes with the old writer, the new one and `RootEncoder`.

Only difference in behaviour (reader), closer to the game: valid UTF-8 holding a 4-byte form (a
byte of 0xF0 or above) without hangul or 0xC0 was accepted by the old test and is now refused, as
the game's reader refuses every 4-byte form (its string tag reads Java's standard modified UTF-8).
A region written by a third-party tool with an emoji in standard UTF-8 now fails to read like in
the game instead of loading. Our writer never produces such bytes (surrogate pairs). Proof:
`fast_string_reader_agrees` (150,000 inputs: this case only, all others identical) and
`four_byte_utf8_form_is_refused` (frozen bytes `ED 95 9C F0 9F 98 80` and `F0 9F 98 80`).
`RUSTCRAFT_OPT_NBT_STRING_READ_FAST_PATH=0` restores the old leniency.

Micro-benches (trial profile, loaded machine: other agents compiling, 3 passes):

| Bench | Old | Strings | Arrays | Strings + arrays | + size hint |
|---|---|---|---|---|---|
| `list_write_bench`, typical column (24 sections, 40 entities) | 59.1–59.7 µs | 15.0–15.2 µs | 59.7–60.1 µs | 14.8–15.2 µs | n/a |
| `encode_root_bench`, 17,676 real columns (7 KiB average) | 5.2–8.1 µs | 3.3–5.6 µs | 5.3–9.4 µs | 3.2–5.0 µs | 3.4–5.0 µs |
| `encode_root_bench`, 9,626 full columns (10 KiB average) | 10.6–11.8 µs | 6.3–7.9 µs | 8.3–10.8 µs | 5.6–6.3 µs | 5.0–6.6 µs |
| `array_write_bench`, 4,096 longs | 2.26–2.56 µs | | 1.80–1.81 µs | | |
| `array_write_bench`, 4,096 ints | 2.06–3.01 µs | | 0.72–0.93 µs | | |

Strings: ×4 on the typical column, −35 to −45% on the real ones. Arrays: −20% (longs) and ×2.5
(ints) alone, lost in the noise inside a column. Size hint alone (old writer): 9.9–10.8 µs
against 10.6–11.8, within noise. Arrays and size hint kept on behind their switch (C6) until the
quiet-machine bench of the `autosave-1` and `generation` scenes. Multi-core: one root is a
sequential walk; the generator already encodes its batch in parallel, the save loop in series
(its parallel version is another mission: it must keep one `RootEncoder` per rayon task through
`map_init`). GPU not applicable (tree walk, a few µs). Not done: AVX2 byte swaps behind runtime
detection (≈ 1 µs per column at most); the same per-element long writing in the LAN chunk packet
(`lan/encode.rs`, `container_raw`), left to the mission that owns that file.
### Piglins and spear (fidelity G4-F13, 23/10/2026)

New code on the integrated server thread, off the hot path: decision by reading the code, no
scene bench. Mount of the spear carrier: climbing up the vehicle chain (in practice 0 to 2
lookups by id) once per goal tick or spear behaviour tick, only for
a mob armed with a spear that has a target; no allocation. Sworn enemy: one more kind
comparison per entity seen (same loop). Item that parries: reading of the stack's component then of the
defaults already in cache, at the trigger of the end of admiring and at the use of a transforming
tool only. Cache, multi-core and GPU not applicable (a few nanoseconds, order of the calls imposed by
the tick). Nothing to re-measure unless a scene of mounted mobs enters the bench.
### Saving scattered pieces and structure mobs (fidelity G6-F19, 23/10/2026, loaded machine)

New code on the generation thread, off the hot path: re-reading the recorded state of the scattered
pieces (pyramid, temple, hut) before each decoration and rewriting after, over the 9 chunks of
the region. Bench `scattered_sync_cost` (ignored, 100,000 calls): 22.6 ns per chunk without a scattered piece
(common case: only the `starts` compound is walked, no lock or read of the starts cache),
107.5 ns for a hut's chunk; i.e. < 1 µs per decoration of several milliseconds (< 0.1%).
Leads not retained, for lack of a possible measurable gain: keeping the state outside the compound (separate table),
parallelizing (9 walks of a few ns). Search for the dimension's origin chunk done only
once and only if a structure set uses it (none in the game data: 0 cost).
Finalization markers and allay draw: a few bytes per placed mob. GPU not applicable.
### World opening: jar indexed once per process (C1-M12, 23/10/2026, loaded machine)

Before: five readers reopened the jar at every singleplayer opening (registries, two generation
readers per generator, spawn search at creation, English language), each on an
unbuffered file opening the 34,227 entries (local header read even for the classes); `/locate`
and `/place` also reloaded all the generation data on the tick thread. Micro-benches
`rc-data/examples/jar.rs` and `rc-worldgen/examples/worlddata.rs` (loaded machine: other agents
compiling, figures to re-measure):

| Measurement (ms, min / median) | Before | After |
|---|---|---|
| one old jar reader (registries, generation, decoration) | 184-226 | — |
| jar index (all the `data/**` entries + `en_us`), 1 thread, file read | — | 51-55 |
| same memory-mapped, 1 thread / 2-4 threads | — | 47-48 / 33-41 |
| same 8 threads / 16-32 threads (mapped) | — | 43-49 / 37-62 |
| generation data (`WorldData`) loaded | 839-2,512 / 931-6,501 | 177-223 (jar already indexed) |
| generation data already held (`/locate`, `/place`, other dimension) | same as loading | 0.18 |

Decisions (to settle on a quiet machine on the scene bench: opening, "generator ready", first
`/locate`, Nether):
- `RUSTCRAFT_OPT_JARINDEX=1` (default): one index per process, bytes borrowed (`Arc`) by all
  the readers; RAM: ≈ 10 MB of entries kept for the process (instead of 2 to 4 copies while
  a world is open).
- `RUSTCRAFT_OPT_JARIO=mmap` (default) / `read` / `file`: `read` and `mmap` indistinguishable within noise;
  `mmap` retained because it does not add the transient 41.5 MB of `read`. `file` ≈ 170-184 ms.
- `RUSTCRAFT_OPT_JARFILTER=1`: sort by name before opening the entry (file: 230 → 170-184 ms;
  memory: 66-88 → 47-55 ms).
- `RUSTCRAFT_OPT_JARPAR=1`: slices of at least 256 entries; gain 1 → 4 threads ≈ 30%, beyond 8 threads
  slower (waking threads for ≈ 10 MB to decompress); kept on, to re-measure.
- `RUSTCRAFT_OPT_WORLDDATA_SHARE=1`: generation data shared as long as a generator
  holds it (weak reference: nothing remains with the world closed).
- `RUSTCRAFT_OPT_TEMPLATES_SHARE=1`: templates borrowed (no more deep copy of the ≈ 4 MB of `.nbt`).
- `RUSTCRAFT_OPT_JARPRELOAD=1`: index built on a background thread during the loading of the client's
  resources (first opening without waiting for the jar).
### Common upload staging and frame uniforms (C3-M04, 23/10/2026, loaded-machine micro-bench)

wgpu 30.0.1 creates and destroys a staging buffer per queue write (read in the source). New
module `render/upload.rs`: ring of recycled 1 MiB pieces, copies in a staging encoder
submitted ahead of the frame's (same order as the queue write), cameras grouped in 4 × 256 B.
Main thread only (single GPU queue: multi-core not applicable; the pool writing directly into
a mapped staging buffer is only worth trying if the memory copy remains visible on the scene bench).

| Building block | Variant | Result | Decision |
|---|---|---|---|
| Writes (`RUSTCRAFT_OPT_UPLOADRING`) | persistent ring instead of a staging buffer per call | `upload_bench`, 15 writes/frame, 3 passes: writes 20.1-31.0 → 6.9-8.7 µs, submit 19.3-30.1 → 29.0-49.5 µs, total 58.0-118.4 → 53.6-125.3 µs; staging buffers created 45,000 → 3 | kept at 1 by default, net gain not proven (submit more expensive: second command buffer, re-mapping of the piece); to settle on the scene bench |
| Cameras (`RUSTCRAFT_OPT_FRAMEUNIF`, `RUSTCRAFT_OPT_UNIFSKIP`) | one buffer, one write of the changed range; identical values skipped | 300-frame script: 825 writes (separate) → 545 (skip), 297 (grouped), 268 (both) | kept at 1; CPU to measure on the bench |
| End of frame (`RUSTCRAFT_OPT_NOPOLL`) | `device.poll` removed (wgpu picks up finished work at every submit) | frame-by-frame proof without any poll (`no_poll_delivers_at_next_submit`, `chunks_come_back_without_poll`): timestamp of frame N read at the start of N+1 as with poll, staging piece returned at every frame (1 piece for 60 frames); fails if the callbacks are only delivered by a poll. Old test (10,000 frames, final wait) removed: it could not fail | kept at 1; CPU to measure on the bench |

### Terrain vertex shader (C3-M05, 24/10/2026, loaded-machine micro-bench)

`terrain.wgsl` variants chosen at startup (`terrain_shader.rs`, `//#if` blocks), each
behind its switch (`=0`: old path), default = new. Proof on a real GPU
(`shader_variants_render_identically_on_gpu`, RTX 5080 Vulkan): each variant alone and
combined, two quad formats, three views, image blended by maximum AND last triangle winning
identical bit for bit to the old path; 6 sabotages seen. Bench `shader_variants_cost`: rich
column drawn 400 times, best of 7 alternating rounds, time from submission to the end of the GPU (old
format / new format). "Out of view" = everything clipped, vertex work alone.

| Lead | Variant | Result (loaded machine) | Decision |
|---|---|---|---|
| Vertex / fragment ceiling (`RUSTCRAFT_GPUPROBE=vs\|fs`, diagnostic) | vertices sent out of view; fragments with a constant color | micro-bench: vertices alone 2.28-2.49 ms out of 10.8 ms of "from above" view (≈ 21-23%); scenes plains32 and cave to measure | diagnostic never active; scene measurement to do |
| Targeted read (`RUSTCRAFT_OPT_VSREAD`) | quad fields read one by one (vs, vs_sorted, vs_sorted_c) instead of an indexed 96 to 128 B copy | no array held by value and indexed in the vertices (naga check); out of view −0.2 to +0.4%, from above −0.0 to −0.4%: noise | kept at 1 (the NVIDIA driver already optimized the copy); to re-measure on DX12 and another GPU |
| Corners in nibbles (`RUSTCRAFT_OPT_QUADBITS`) | `(0x320210 >> 4v) & 15` instead of the constant array (the plan's constant 0x230210 was wrong: 0,1,2,0,3,2) | out of view +0.1 to +0.6%, from above −0.2 to −0.7%: noise | kept at 1 |
| Grouped outputs (`RUSTCRAFT_OPT_VARYINGS`) | 3 slots instead of 7 (uv + fog, tint + visibility, flat animation) | from above −0.3 to −0.5%, out of view +0.6 to +0.8%: noise | kept at 1; "flat vis outside fade" not done (depends on the distance per vertex) |
| 4 vertices per face, indexed (`RUSTCRAFT_OPT_FACEIDX`) | without instances: shared u16 indices (16,384 faces per command, 192 KiB), one instance per command whose attribute (identity buffer) gives the 1st face, faces read from storage; no implicit offset (DX12 without indirect validation) | out of view 2.49 → 1.25 ms (−50%) and 2.28 → 0.99 ms (−57%); from above −0.4 to −0.6% (view limited by fragments) | kept at 1; VRAM (counted in "other"): shared indices 192 KiB, indexed commands 64 KiB then power of 2, identity buffer 4 B per face of the largest region rounded to the power of 2 (up to 8 B per face, 256 KiB at the start, returned to 256 KiB when the world is emptied) |
| 4-vertex strip (`RUSTCRAFT_OPT_STRIP`, without FACEIDX) | TriangleStrip corners 1,2,0,3 (block faces only: the double face of fluids changes the order of its triangles, impossible in a strip) | identical bit for bit on RTX 5080; out of view −3.7 to −5.0% | kept at 1 as a fallback for FACEIDX=0 |
| Fade on the GPU (`RUSTCRAFT_OPT_FADEGPU`) | birth in µs written once, sentinel at the end of the fade, visibility computed in the shader; camera clock at 0 when nothing is fading or switch at 0, and in the totem and interface cameras (otherwise the 4 cameras change at every frame and `UNIFSKIP` no longer skips anything) | whole ring sent 3,521 frames → 2 over 4,000 simulated frames (4,287 cells alone); `fade_ring_cost`: 4.40 → 2.16 µs/frame (−51%) excluding the GPU upload; visibility difference ≤ 2 µs of fade (≤ 2·10⁻⁵, threshold 1/255) | kept at 1; CPU and bytes sent to measure on the bench (flight, plains32 #0, building) |
| Fluids read from storage (`FLUIDSTORAGE`) | `region_quads[instance_index]` | not done: `instance_index` does not contain `first_instance` in DX12 indirect without validation (wgpu-hal 30: special constants only with a dedicated signature); announced gain small | to take up again with the identity buffer if the bench shows the fluids |
### Sky and clouds: compact faces, persistent buffer, grouped uniforms (C3-M10, 24/10/2026, loaded-machine micro-bench)

Game 26.3 (read by decompilation) already stores a cloud face per 3-byte record and
draws the corners from the vertex number in the shader, in a fixed-size buffer per radius; our
old mesh sent 96 B per face in a new buffer at every 12-block cell crossed.
Bench `cloud_bench` (ignored by default, 256 × 256 patchy map, 20 passes, best time), main
thread, construction only:

| Building block | Variant | Result | Decision |
|---|---|---|---|
| Format (`RUSTCRAFT_OPT_CLOUDINST`) | `face`: 4 B per face, 6 vertices per instance | range 128 below: 1.328 → 0.621 ms (sweep), 5,691,360 → 237,140 B (÷24); inside 2.051 → 0.647 ms, 9.7 MB → 404 KB; flat 1.113 → 0.528 ms; range 64 below 0.263 → 0.093 ms. Vertices processed identical (355,710) | default |
| Format | `cell`: 8 B per instance (i, k, 4 face codes), empty slots degenerate | range 128 below 0.665 ms, 334,456 B (more than `face`), 1,003,368 vertices processed (×2.8); inside 334,456 B (less than `face`: 404,368) | kept at `cell`, loses on the bench; GPU to re-measure |
| Buffer (`RUSTCRAFT_OPT_CLOUDBUF`) | persistent at the radius's worst case, rewritten through the common staging | no more GPU buffer creation per cell crossed; 1 staging write per rebuild | kept at 1; p99 in flight to measure |
| Order (`RUSTCRAFT_OPT_CLOUDSPIRAL`) | disk precomputed per radius | faces 0.621 → 0.559 ms (−10%), range 64 0.093 → 0.078 ms | kept at 1 |
| Multi-core (`RUSTCRAFT_OPT_CLOUDPAR`) | slices of the spiral on the common pool, put end to end | range 128 below, threads 1/2/4/8/16/32: 0.572 / 0.279 / 0.144 / 0.103 / 0.115 / 0.068 ms; range 64: 0.087 → 0.026-0.036 ms (dedicated pool empty). Pool busy as in flight (32 threads, 256 tasks of 1 ms already queued, construction launched outside the pool): 8.2 / 9.1 / 9.3 ms (min / median / max, 10 passes) against 0.58 ms on the main thread alone: the task waits its turn behind the meshings. Slices kept from one crossing to the next (`cloud_par_reuses_slice_buffers`) | **off by default** (`=1` to enable it): wait confirmed on the micro-bench; to take up again only with a priority queue or a separate pool, measured on the "flight" scene bench |
| GPU (compute) | construction by compute shader | not done: 0.07-0.6 ms per cell crossing (≈ 1/s in flight), GPU construction would require an ordered compaction (prefix) to keep the draw order; to reconsider only if a spike remains | ruled out without measurement (cost of a round trip) |
| Uniforms (`RUSTCRAFT_OPT_SKYUNIF`) | 7 slots of 256 B in a buffer (`upload::Slots`), one write of the changed range, unchanged values skipped | 48-frame script: up to 5 queue writes per frame (glow, sun, moon, stars, clouds) → 1 staging write at most (test `sky_buffers_match_the_old_path`) | kept at 1; CPU to measure on the bench |

Cloud VRAM at range 128: old mesh 4 to 9.7 MB recreated at every crossing; persistent
`face` buffer reserved once at (cells of the disk × 4 + 54) × 4 B ≈ 1.5 MB.
Real drawing proven by `packed_clouds_render_like_the_mesh`: six views (below, inside, above,
"fast" mode) rendered at 64 × 64 by the old mesh then by `face` and `cell`, in classic
pass and through improved transparency: byte-identical images. The test fails if the
halves of the attribute (i, k) are swapped in the shader (3,180 bytes differ) or if the pair
(vertices, instances) of the draw is permuted.
Radius of the compact formats: bounded to 511 cells (10 signed bits); `cloudRange` ≤ 128 gives 171
(test `cloud_range_fits_the_packed_radius`, which fails if the option's bound exceeds 383).
Note: the End's box is not constant (its matrix follows the camera's rotation); skipping
unchanged values writes it once as long as the view does not rotate.
### UI upload, pre-resolved sprites, repeated background in the shader (C4-M14 + C3-M24, 24/10/2026, loaded-machine micro-bench)

Bench `gui::tests_opt::gui_bench` (trial scene: pause background, list, 32 repeated strips, buttons,
boss bars, tabs, styled text, F3, chat, tooltip; 1600 × 900 scale 3, 400 frames,
RTX 5080 Vulkan, other agents were compiling). "build" = layer construction, "upload" =
`Gui::upload` THEN `Uploader::submit`: the ring pays at submit the closing of the piece, its command
buffer and the copy, which the first measurement (upload alone, "1st measurement" column) did not count
and which favoured the new paths. Four passes, loaded machine; the table gives the first
two (a trial sabotage was still compiled in: a per-instance comparison at encoding and
a variant of mode 7, with no effect on the old path); the next two, with clean code, are
2 to 3 times slower on the same cases (old: upload + submit 40.2 then 134.8 µs; "all but
changes only" 120.8 then 79.9 µs) and unusable. No upload gain is established by this
bench: decision on the scene bench, quiet machine. Bytes = writes
going through the staging (the old path writes through the queue: not counted). Main thread alone:
the interface is built and sent in call order (multi-core not applicable; GPU: the
shader already does the moved work, GPU cost to read on the bench).

| Building block | Variant | 1st measurement (upload alone, obsolete) | Build; upload + submit (2 passes) | Decision |
|---|---|---|---|---|
| Old path | | build 61.5, upload 22.4 µs | 68.1-80.9; 41.5-46.9 µs; 7,524 vertices | reference |
| Upload (`RUSTCRAFT_OPT_GUIWRITEWITH`) | layers encoded directly into the staging (C3-M04), no more intermediate copy | upload 15.3 µs | 80.4-86.6; 42.4-47.0 µs | kept at 1, gain not proven: to settle on the scene bench |
| Uniform (`RUSTCRAFT_OPT_GUIUNIFORM`) | screen uniform written only if it changes; interleaved draws without `Box` or `VecDeque` | upload 21.9 µs (noise) | 73.9-75.2; 46.8-49.1 µs; 1 queue write and 2-3 allocations fewer per frame | kept at 1 (work avoided, no cost) |
| Index (`RUSTCRAFT_OPT_GUIINDEX`) | 4 vertices per quad + static u32 indices | build 56.9, upload 15.1 µs | 61.9-84.9; 34.7-45.1 µs; 300,960 → 200,640 B | kept at 1 (fallback if instances at 0) |
| Instances (`RUSTCRAFT_OPT_GUIINSTANCE`, formerly `GUIINST`) | one 88 B instance per quad, corners redone in the shader | build 56.6, upload 10.9 µs | 72.9-81.6; 35.8-42.6 µs; with direct write 69.1-69.7; 33.0-35.4 µs; 110,352 B (÷ 2.7) | kept at 1 (takes precedence over the index); GPU cost of the vertices to read on the bench |
| Changes only (`RUSTCRAFT_OPT_GUIKEEP`) | only the byte range changed since the previous frame is sent | with instances: upload 10.7 → 11.5 µs (slower) | old format 86.5-99.7; 63.4 µs (slower than the old path); with instances 64.5-79.4; 35.0-38.7 µs; 14,361 B | **set to 0 by default** (24/10, review): gain in bytes only, never turned into time; to re-measure on the scene bench (frozen HUD = no upload, obfuscated text or F3 = encoding + comparison on top) before enabling it |
| Sprites (`RUSTCRAFT_OPT_SPRITEIDS`) | buttons, tooltip, boss bars, creative tabs and scrollbar + 3 uniform rectangles resolved in `Gui::new`; no more `format!` per frame (boss, tabs) | not measurable (noise ± 15 µs) | 77.2-90.7; 50.1-56.2 µs (noise) | kept at 1; samply in inventory. Still looked up by a name built at every frame (work screens only, to do): `container_bg/…` backgrounds (work screens, creative), cartography table, enchanting table (2 per row), beacon effects, recipe book filter, creative tabs beyond 7 columns, pack icons |
| Repeated background (`RUSTCRAFT_OPT_GUITILESHADER`) | mode 7: one quad, uv in tiles, atlas rectangle in the color, sample clamped to the sprite | build 57.8, upload 15.6 µs | 58.3-145.9; 36.6-72.4 µs; 7,524 → 4,872 vertices | kept at 1; difference below |
| All but changes only (default) | | build 48.2, upload 6.4 µs ("all") | 52.1-116.6; 27.6-56.2 µs; 812 instances | default; gain to prove on the scene bench |

Equivalence (real GPU, 1600 × 900, scales 1 to 6, 3 frames of which one identical, with and without staging
ring): 0 pixel difference for all the paths, except the repeated background in the shader, and only
on the pixels whose center falls exactly on a tile seam (position checked pixel by
pixel, all scales: 188 at scale 1, 400 at scale 2, 384 at scale 3 on the RTX 5080).
On these pixels, the old path (one quad per tile) reads beyond the sprite's edge in the atlas
(observed: undrawn pixel, the texel read outside the rectangle being transparent here; the mechanism
holds for any atlas sprite with an edge falling exactly on a pixel center, fallback path
left as is); the repeated quad clamps the sample to the sprite and takes its first or last
texel, like the game which repeats a texture of its own (checked on the processor by
`tiled_seams_take_own_texels`, all scales). Without this clamping (first version), mode 7
also read the neighbouring sprite: 1,772, 560 and 10,475 pixels outside the sprite at scales 1, 2 and 3.

### Surface rules and veins (C1-M08, 23/10/2026, loaded-machine micro-bench)

Bench `material_opt_bench` (ignored by default): surface cost = surface step − fill
step, 12 × 12 grid seed 0 and 24 badlands chunks, 1 thread, 2 alternating passes,
LOADED machine (other agents compiling: ±0.6 ms differences between passes). All the leads
stay behind their switch (`RUSTCRAFT_OPT_<NAME>=0` = old path), default = new;
final decision on the scene bench (flight, generation, crowd, redstone), quiet machine.

| Lead (switch) | grid ms/chunk (passes 0; 1) | badlands | Note |
|---|---|---|---|
| old | 3.94; 3.24 | 3.27; 3.23 | 18 allocations, 0.70 MB per chunk |
| `ORE_IDX` | 3.07; 3.64 | 3.16; 3.06 | within noise |
| `ORE_EARLY` | 3.65; 2.67 | 3.07; 3.09 | |
| `SURF_SKIP` | 3.59; 3.00 | 3.16; 3.30 | alone, within noise (the bands cover it) |
| `SURF_BANDS` | 2.56; 2.14 | 2.51; 2.46 | biggest gain alone (−25 to −35%) |
| `VEIN_YRANGE` | 3.76; 2.81 | 2.93; 2.70 | +0.04 MB (row buffer) |
| `+VEIN_LAZYRICH` | 4.22; 3.15 | 2.92; 3.04 | −0.25 MB allocated |
| `+VEIN_SHARED` | 2.53; 3.39 | 3.11; 3.23 | |
| `MAT_REUSE` | 3.05; 2.54 | 3.25; 5.30 | 10 allocations, 0.13 MB (−0.57 MB) |
| `MAT_COLCACHE` | 3.05; 3.05 | 3.09; 4.10 | |
| all except `VEIN_SHARED` | 2.19; 2.03 | 2.26; 1.84 | best combination measured |
| all | 2.33; 2.20 | 2.10; 2.09 | 7-8 allocations, 0.17-0.21 MB |

Threads (surface, old → new, ms/chunk): 1 thread 3.59 → 1.87; 2: 1.90 → 0.99; 4: 0.84 →
0.61; 8: 0.46 → 0.33; 16: 0.26 → 0.19; 32: 0.19 → 0.08. Caves (top material replayed):
difference drowned in noise (±1 ms), allocations 178-203 → 70-71 and 0.02-0.12 → 0.01 MB per
chunk. GPU not applicable (small volumes per chunk, branching rules; FMA not allowed for the
pre-fill). No temperature condition in the 26.3 rules: the temperature part of
`MAT_COLCACHE` only serves packs (only the badlands bands offset plays in vanilla).
`VEIN_SHARED` gains nothing clear (copper [0, 50[ and iron [−64, −8[ ranges almost disjoint):
kept at 1 until the quiet bench (C6). Byte code of the biome conditions: not done (low
priority, to decide after the scene bench).

### Surface under the preliminary surface, vein bound, cave top material (C1-M04, 03/11/2026, loaded-machine micro-bench)

Measured first (`surface_phases_bench`, 1 thread, seed 0, 12 × 12 grid): with the paths above,
the surface step was 1.70-2.28 ms/chunk, of which the column walk 1.30-1.82, the vein
pre-filling 0.43-0.53 and the row classes ≈ 0; the caves replay the top material 11.9 times per
chunk at 4.9-5.1 µs each (≈ 0.06 ms/chunk). The walk, not the veins, was the hot part.

Leads (each alone over the earlier paths, then combined; "step" = whole surface step, 2 passes,
LOADED machine: ±0.5 ms between passes, the second pass was the calmer one):

| Lead (switch) | grid | badlands | deep ocean | mountain | Note |
|---|---|---|---|---|---|
| old | 2.15-3.61 | 2.24-4.51 | 1.70-2.48 | 3.07-6.48 | |
| earlier paths | 1.47-2.64 | 1.63-2.87 | 1.24-2.08 | 1.66-2.48 | reference |
| `SURFACE_BAND_KERNELS` | 0.96-1.18 | 0.95-1.15 | 0.82-0.88 | 1.08-1.11 | walk ÷ 2 to 3 |
| + `SURFACE_ROW_KERNEL` | 0.99-1.49 | 0.95-1.15 | 0.85-1.33 | 1.06-2.16 | no gain: off |
| `DIRECT_SOLID_WRITE` | 1.43-1.45 | 1.40-1.51 | 1.17 | 1.56-1.98 | alone |
| kernels + direct write | 0.90-0.91 | 0.89-0.90 | 0.77-0.78 | 0.99-1.01 | best measured |
| `VEIN_TOP_BOUND` | 1.46-1.55 | 1.46-1.62 | 1.18-1.25 | 1.62-1.79 | see veins alone |
| all (row kernel on) | 0.90-1.36 | 0.86-1.55 | 0.74-1.41 | 0.99-1.36 | |

Band kernels: under the preliminary surface the 26.3 overworld trees read no walk state (stone
depth, water, depth under a ceiling are all behind `above_preliminary_surface`), so the walk stops
there and each y band is decided by a kernel: nothing (above the veins), one block (bedrock bottom
row, deepslate [−8, 0[), a vein (copper [8, 50[, densities tested 8 rows at a time, only
the cells whose density is not ≤ 0 are evaluated), a vein then a block (iron over deepslate
[−59, −8[, copper over deepslate at y = 0), or the band's tree cell by cell (bedrock and
deepslate gradients). The underground rule reads the biome (sulfur caves bands): chunks where
sulfur caves are possible use the "tree" kernels with the biome read per cell (still no walk),
the others the shapes above (23 of the 90 overworld test chunks had sulfur caves around). Kernels
are off if a tree under the surface reads the walk state (checked on the compiled tree) and for the
dimensions that never test the preliminary surface (nether, end, caves, floating islands).

Direct solid write: a plain solid (not air, no fluid, blocks motion) over a plain solid changes
no section count, height map or mark; written in place, by the walk as well as by the kernels.

Layer kernel (`SURFACE_ROW_KERNEL`, compare and select over the 256 states of a layer): within
the noise (grid 1.21-1.26 against 1.25-1.28 ms/chunk without it, the other sets both ways): the
"one block" bands are only 9 layers of ≈ 130 and the column kernel already writes them in place.
Kept behind its switch, OFF by default (`=1` enables it) until the quiet-machine bench (C6).

Vein bound (`VEIN_TOP_BOUND`): pre-filling stopped at the highest solid of the chunk (the rules
are only evaluated at solids; the whole range when eroded badlands, which add solids before the
walk, are possible). Pre-filling alone: deep ocean 0.351-0.380 → 0.321-0.330 ms/chunk (−8 to
−13 %), unchanged on land (the highest solid is above the copper range). Frozen oceans are not
excluded: their icebergs are placed after the walk and no rule is evaluated there.

Cave top material (11.9 calls per chunk on the grid): 4.6-7.0 µs per call; column levels (surface
depth, preliminary surface) kept from the surface step of the same chunk on the same thread
(`CAVE_TOP_COLUMN_MEMO`): 3.8-5.6 µs (−18 %); biomes read from the chunks around
(`BiomeView::ChunkThenSource`, `CAVE_TOP_FROM_CHUNK_BIOMES`, to be wired into the caves step by
the owner of `terrain.rs`): 1.05-1.35 µs; both: 0.24-0.41 µs (−92 to −95 %, ≈ 60-80 µs/chunk
saved). Faithful-to-the-game difference (A3): on 13,028 real calls (4,500 chunks, 3 settings,
3 seeds), 37 differ, all on a quart whose climate is tied between biomes; the last leaf of the
search decides there, which the game keeps per thread (so the game itself depends on the history
of the thread); never at an untied quart. No clamp in y: outside the world height the view asks the
source (routing checked on marker chunks; the real biomes cannot tell, the quarts just inside give
the same biome on 368,000 built calls).

`MIN_SURFACE_FROM_AQUIFER`: not applicable, no switch. The aquifer keeps
`preliminary_surface_level` floored to an integer at quart positions; the material rules read
`chunk_surface_level`, an interpolation of it over 16-block cells, per block, in f32: another
function, other positions, another precision.

`BASE_BLOCK_IDS` ("same block" by an identifier numbered at interning instead of the names cut at
the properties, eroded badlands only): 8.84-8.89 → 0.68-0.70 ns per pair.

Threads (surface = step − filling, earlier → new, ms/chunk, noisy): 2: 1.16 → 0.53;
4: 0.55 → 0.36; 8: 0.33 → 0.23; 16: 0.24 → 0.16; 32: 0.11 → 0.05. Multi-core inside a chunk not
done (the pool is already saturated by chunks, the columns are tied by the gradients and the eroded
badlands). GPU not applicable (f64 noises, branching rules, positional randomness, a few hundred
µs per chunk). Allocations (`material_opt_bench`, earlier → new): grid 7 and 0.17 MB per chunk
both, badlands 8 and 0.21 MB both, caves 70 → 69 (the column levels are a 2 KB array on the
stack, one boxed memo per thread); same bench, surface ms/chunk: grid 1.50-1.63 → 0.93-1.12,
badlands 1.51-1.52 → 0.89-1.00.

### Effects and player (fidelity G4-F09, 24/10/2026, loaded-machine micro-bench)

New code on the integrated server thread, off the hot path. Turtle helmet: bench
`bench_turtle_helmet` (ignored, 20,000 player ticks + attribute synchronization, 2 passes):
1.4-1.7 µs without a helmet, 5.3-5.7 µs with (+3.6 to +4.3 µs per tick, i.e. < 0.01% of the 50 ms budget).
The overhead is the effect update that the game also redoes every tick (packet, trigger,
attributes recomputed). Leads: (1) location of the `equippable` component read without copying the JSON of the
defaults (kept: 5.7-6.1 -> 5.3-5.7 µs, same result); (2) skipping the attribute recomputation for an
effect without a modifier (4.0-4.2 µs, -1.2 µs): ruled out, it removes the capping of
absorption that this recomputation does along the way (see the table). The equipment test only looks at the
stack after an id comparison (6 slots); eyes in water read only if the
helmet is worn. 1.21 effect hooks: one walk of the effects list per hit received
(infestation), nothing outside death; oozing: one walk of the mob list per death
of a carrier (rare). Multi-core and GPU not applicable (a few microseconds, order imposed by the tick).
Follow-up: "eyes in water" follows the game's rule (eye height of the pose, fluid height
of the cell, one or two cells read through the fluid table, without name comparison or walk of the
properties); same bench: 1.42-1.50 µs without a helmet, 5.87-5.95 µs with, within the noise of the loaded
machine. Nothing else to cut: one cell read per tick.
### G5-F09: block rules (24/10, loaded-machine micro-bench)

- Oxidation: stage of each block resolved once (`Table::weather`) instead of parsing the name
  for each of the 128 neighbours of the Manhattan sweep: 6.4 ns against 31.8 ns per neighbour
  (bench `bench_g5f09_tables`). Kept.
- Biome of a block on the server side: palette biome index resolved once
  (`States::biome_data`) instead of a name lookup; the blur zoom, now mandatory
  (fidelity), eats the gain: 54.3 ns (index + zoom) against 45.5 ns (name, no zoom). Direct
  zoom kept (scattered positions: the `zoom_cached` memo loses there, C1 measurement).
- Block families (big dripleaf, stem, chorus, budding amethyst, heads, beehive) as
  `Kind`: no name comparison added to the shape and neighbourhood updates.
- Multi-core, GPU: not applicable (point rules, sequential server order). No
  switch: no competing lead to decide between.
### Sculk sensors, allay, vibration particle, arrows and buttons (fidelity G6-F10, 24/10/2026, loaded-machine micro-bench)

Only new hot spot: the "entity on the ground on a sensor" test (each ground mob and the
player, every tick, only if a vibration block is loaded, otherwise skipped). Bench
`bench_step_on_sensor` (ignored, 2 M calls, entity off the sensor): direct version (effect block
then block name) 56.7 ns; comparison of block ids instead of the name 58.2 ns (no
gain, kept: same cost, simpler to read); table of the registered vibration blocks consulted
before reading the chunk 44.7 ns; possible cells of the effect block tested in this table
before even computing it (exact: feet column, carrier, carrier's column) **10.7 ns
(retained, −81%)**. Multi-core and GPU not applicable (single call of a few ns in the sequential
mob loop). Vibrations: warden/allay/block listeners in the existing per-section
registry (no sweep added); a block's data copied into its block entity
only when it changes (selection, path, reception: ≤ 16 ticks per vibration).
Arrows: one more block kind test per cell of the box (≤ 8 cells), stuck arrow
included. Weak signal: name comparison only for the sources with a `power` property of the
generic case. Vibration particle on the client side: two instances, angles packed in the free
bits of `tex` (quantization 1e-4 rad, imperceptible), no instance field added;
tracking of the targeted entities skipped without a signal of this kind. Scenes to re-measure on a quiet machine: none
of the current bench has a sensor; to add if a sculk scene enters the bench.

Follow-up (review): the "blocked chest" test (cat sitting on top) walked the whole mob list
at every comparator read of a chest, twice for a double chest. Bench
`bench_blocked_chest` (ignored, double chest, 500 mobs of which 10 standing cats, 200,000
reads): full sweep 975 to 1,113 ns/read (code from before); sweep alone with the index
by species cut (`RUSTCRAFT_OPT_KINDINDEX=0`) 671 to 751 ns; **existing index by species
(`of_kind`, only the cats walked) 125 to 127 ns (retained, −83%)**, same result (identical
sequence, verified by the index during the tests). Same pattern swept: nearest enemy to the
warden, End fight dragon, baby polar bear, school of fish, mobs
near a spawner, cats of a village, bees of a beehive, pandas: all through `of_kind`
(same order, same result). Trampling warden: passed through the tick's block effects (prefilter
`vib_blocks` already paid, no cost added). Strip image of the vibration signal chosen at
rendering by a common clock (one modulo per visible signal).
### Fonts during block baking (I-01, 24/10/2026, loaded-machine micro-bench)

`RUSTCRAFT_OPT_RESPAR` (default 1, `=0` old order): the baking of the block models starts on a
separate thread before the fonts; the audio engine is launched as soon as the default font is validated; the
"blocks" section is only entrusted to the cache once the stack is retained (never for a failed
stack, whose baking is awaited then thrown away). Section order of the file unchanged. Micro-bench in
`respar_real_assets_identical_cold` (fonts + models, cold cache, vanilla, 2 alternating passes):
serial 151 and 135 ms, parallel 107 and 102 ms, i.e. −33 to −44 ms cold only (warm: two
section decodings, not measured). Rayon threads not varied (the baking keeps its own
parallelism; the gain comes from overlapping the fonts). GPU not applicable (file reading and decoding).
Decision on the quiet-machine scene bench (cold boot, cold F3+T, broken stack);
abandon if the A/B stays within noise.
### Saved chunks: sending in slices and reading off the tick (I-02, 24/10/2026, loaded machine)

Before: `send_chunks` read the whole queue (chunks, entities, points of interest) on the server
thread, then sent everything in the same tick. The tick was blocked all that time, and no chunk
left before the end. Leads (`crates/rc-sim/src/loader.rs`), all behind a switch,
all at **0 by default** until the quiet-machine A/B, teleportation included (condition of the
mission):
- `RUSTCRAFT_OPT_CHUNKSTREAM`: queue loaded then sent in slices. The first goes up to the
  last chunk of the 17 × 17 square, the following ones are 32 chunks, as long as the tick budget
  allows (`RUSTCRAFT_CHUNKSTREAM_MS`, 10 ms).
- `RUSTCRAFT_OPT_ASYNC_LOAD`: queue read by a "singleplayer-read" thread. Batches picked up at the start of the tick
  (and just before sending, `RUSTCRAFT_OPT_ASYNC_LATE`, 1), adopted in the order of the requests by the
  same code as the synchronous read. A position loaded in the meantime, or out of view, is dropped.
- Sub-settings, active only with both paths: `RUSTCRAFT_OPT_CHUNKSPLIT` (1, tasks of 4
  chunks per region) and `RUSTCRAFT_OPT_ASYNC_NEARSYNC` (1; both paths together: the spawn
  square is read right away, the rest on the thread).
- Tracking: the queue is put in a table at center change (the linear search became
  quadratic with a queue that stays long).

Proof: `tests_opt::loading`, on the model world, three sites (plains, teleportation into the
explored plains, Nether). All the variants are equal to the old path: chunks sent and
SHA-256 of each packet, UUIDs of the announced entities, adoption order, saveable state, no
duplicate mob. Sabotages 1 to 5 are all detected: marked chunk sent before its read,
forgotten chunk, batches backwards, late arrival not filtered after a synchronous read, read
kept out of view. `tests_opt` (fingerprints) also passes with both switches at 1: it does not
go through the sending.

Micro-bench `bench_chunk_sending` (`--ignored`), 50 ms ticks, 3 loaded-machine passes
(the old path varies from 190 to 296 ms from one pass to the next). Columns: first chunk sent /
all sent / longest call of the "chunks" part of the tick, in ms.

| Variant (32 threads) | Entering plains32 (3,461) | Teleportation forest (533) | Teleportation village (533) |
|---|---|---|---|
| old path | 208–257 / 211–261 / 208–257 | 39–53 / 40–53 / 39–53 | 63–86 / 63–87 / 63–86 |
| CHUNKSTREAM | 48–59 / 1,437–1,681 / 48–59 | 47–48 / 64–105 / 47–48 | 77–78 / 133–134 / 78 |
| CHUNKSTREAM without splitting | 30–66 / 1,276–2,316 / 35–66 | 58–77 / 121–141 / 58–77 | 85–97 / 149–162 / 85–97 |
| ASYNC_LOAD | 281–349 / 286–353 / 27–40 | 55–56 / 56 / 5 | 106–107 / 107 / 6 |
| both, without NEARSYNC | 58 / 363–773 / 11–12 | 58–118 / 59–165 / 8–12 | 108–117 / 108–154 / 7–14 |
| both, with NEARSYNC | 49–71 / 361–1,180 / 49–71 | 45–72 / 102–128 / 44–72 | 67–101 / 120–158 / 67–101 |

One thread (`RAYON_NUM_THREADS=1`), same order: old 1,151 / 1,153 / 1,151 at entry, 75 / 76 / 75
in forest; CHUNKSTREAM without splitting 61 / 3,204 / 774 and 106 / 169 / 106, with splitting
76 / 4,951 / 920 and 126 / 193 / 125; ASYNC_LOAD 1,550 / 1,555 / 41 and
157 / 158 / 6; both without NEARSYNC 1,061 / 1,759 / 26 and 107 / 151 / 6; with 1,112 / 1,919 / 1,112.
With a single thread, the read thread shares the pool with everything else.

Reading the figures: sending in slices brings the first chunk forward (÷ 4 at entry) but spreads the
rest over many ticks. The 10 ms budget per 50 ms tick limits the throughput, not the read:
splitting the reads by region gains nothing clear there. The separate read brings the longest
tick down from 190–257 ms to 5–40 ms, but adds up to one tick before the first chunks of a
teleportation (55 against 39 ms in forest). Both together without NEARSYNC: tick ≤ 12 ms, first
entry chunk ÷ 4, teleportation slower at the first chunk. With NEARSYNC: teleportation
close to the old path at the first chunk, but a synchronous read tick comes back. Decision:
quiet-machine scene bench (plains32 entry, teleportation, dimension change, flight p99,
new world). GPU not applicable (file reading and NBT decoding).
### Proto-chunks written at forgetting or at save (I-03, 24/10/2026, loaded-machine micro-bench)

Before: at every batch, all the modified proto-chunks were encoded (uncompressed NBT) then
compressed and written by the writer, whereas a proto touched by several neighbouring batches was
re-encoded each time and most become full shortly after (write overwritten).
`RUSTCRAFT_OPT_PROTO_LAZY` (default 1, `0`: old path; turned off for a while at the integration of batch
4, `tests_opt::fresh_world_fingerprint` failing one time out of two: the writer of a server dropped without
closing outlived its server and wrote the flushed protos at shutdown into the next new world
at the same path; the writer now waits for its writes when it is dropped, then 20 out of 20
identical with `PROTO_FLUSH_ASYNC` at 1 and at 0, as with `=0`): a proto is only written when it is forgotten from the table, at the
flush of the save (`save`, every parked dimension) and at closing; never if it
became full. `RUSTCRAFT_OPT_PROTO_FLUSH_ASYNC` (default 1; `0`: awaited): the automatic
save and the pause one request the flush without waiting for it (protos written right after
`level.dat`, at the next tick's readout), closing always waits.

Bench `flight_proto_writes` (ignored by default): straight flight of 40 rows of 9 chunks, cap
900, fast writer, 2 passes, LOADED machine.

| Path | protos encoded | NBT | batches, all cores (ms) | batches, 1 thread (ms) | final flush |
|---|---|---|---|---|---|
| per batch (`=0`) | 1,226 | 30 MB | 1,590; 1,562; 1,557; 1,594 | 5,626; 7,490; 5,485; 5,400 | — |
| lazy | 212 | 5 MB | 1,503; 1,533; 1,530; 1,715 | 5,068; 5,235; 4,989; 7,335 | 6 to 8 ms (1 thread: 76 to 111 ms) |

Main gain outside the generation thread: 25 MB less NBT to compress and write by
the writer over this flight (−83% of the proto writes), the only quantified gain. CPU of the
generation thread: within noise (before and after ranges overlap, loaded machine).
Flush wait (variant `PROTO_FLUSH_ASYNC=0`): the save waits for the end of the batch
in progress (~40 ms all cores, 135 to 190 ms on 1 thread) then the flush (76 to 111 ms on 1 thread), or even
the creation of the generator of a dimension that has just opened, i.e. a frozen tick of 4 to 6 ticks
on 1 thread at every automatic save during a flight, which writing at every batch
never did. Decision: no wait by default (no new blocking, protos on disk one
tick later, identical content, proven by test); the waiting variant stays behind
`=0`. To confirm on the scene bench (automatic save during a flight, 1 thread and all cores,
tick p99 and maximum, `PROTO_LAZY` 0 against 1, `PROTO_FLUSH_ASYNC` 0 against 1). GPU and
multi-core: no further relevance (encoding stays parallel per proto).
### Scoreboard sidebar kept by revision (I-07, 24/10/2026, loaded-machine micro-bench)

Bench `bench_sidebar` (client, ignored by default), one objective displayed, values 0..999, cost
per frame in µs, LOADED machine (3 passes; the selection varies from 15 to 33 µs between passes).
Switch `RUSTCRAFT_OPT_SIDEBARCACHE` (default 1; `=0`: full walk and sort at every frame).

| Entries | full (old path) | rebuild (selection of the 15) | kept (revision unchanged) | previous sort, lowercase allocated |
|---|---|---|---|---|
| 20 | 1.1-1.9 | 1.1-1.5 | 0.004 | 0.1 |
| 200 | 3.5-5.2 | 2.3-2.6 | 0.004 | 2.8-3.1 |
| 2,000 | 48-58 | 15-33 | 0.004-0.007 | 79-103 |

The revision changes per objective, slot or team packet, and per score or reset
of an objective placed in a sidebar slot (1, 3 to 18); the scores of the other
objectives do not touch it. The flattening of the 16 texts stays per frame (depends on the language,
replaced in place without a counter). Fidelity fix outside the switch: the game's case-insensitive
comparison (code points, simple mappings) instead of two string lowercasings, with
skipping of the common byte prefix (full sort at 2,000: 94 → 49 µs). Multi-core and GPU not applicable
(15 lines, 4 ns kept). Decision on the "synthetic board 2,000 entries" scene bench.
### Redstone dust: weak signal read on the requested side only (I-08, 24/10/2026, loaded-machine micro-bench)

`RUSTCRAFT_OPT_WIRESIDE` (default 1; 0 = full state recomputed then read on one side). The weak
signal of a dust toward a horizontal neighbour now only computes the requested side, its two
perpendiculars and, for a dot only, the opposite; it no longer writes any property.
Proof: `blocks::redstone::tests::wireside_matches_full_state` (240,000 sides, 3×3×3 neighbourhoods
drawn at random, fails if the dot rule is inverted or if the opposite side is forgotten).

| Measurement (1 core, calling thread) | old | new |
|---|---|---|
| answer for one side (25 dusts, line and comb), 4 passes | 520-650 ns | 87-105 ns |
| line of 15 dusts lit then cut, median (2 alternating passes) | 683; 602 µs | 586; 705 µs (noise) |

Profile `bench_ticks` (call counter): 0 calls on the 6 sites, `perf:redstone` included (the
scene has no dust: observer clocks, pistons, lamps). Abandon condition of the
mission (< 5% of the redstone tick) met for this scene; kept behind the switch until
a scene bench with dust (C6). Multi-core and GPU not applicable: isolated call, within
an ordered update cascade.
### Version data: name → id index and preparation (C4-M11, 24/10/2026, loaded machine)

Micro-bench `benches/load.rs` (`names`), 1 thread, LOADED machine; `RUSTCRAFT_OPT_NAMEINDEX=0` = old
path (linear search). Lazy index per list (position table, first occurrence
kept, answer checked against the list, linear fallback if the length changed).

| Lookup | linear | index | Note |
|---|---|---|---|
| `registry("minecraft:sound_event")` | 54 ns | 16 ns | 95 registries |
| id of a sound (52 names spread out) | 613 ns/name | 18 ns/name | ≈ 1,900 entries, every `play_sound` |
| block by name (48 names spread out) | 725 ns/name | 19 ns/name | 1,286 blocks |

Memory: one `u32` position table per list consulted (≈ 4 B × 1.15 per entry + control
bytes; < 100 KiB for the ≈ 7,000 entries of the 95 registries and the blocks), built at first
use only. Gain in game: only when the callers migrate (`registry_id`, `Registry::id`,
`block_id`, known registries `items()`…, `Registry::shared` to replace the `entries.clone()`);
nothing announced on `bench_ticks`.

Preparation (1st launch), `examples/prep_reads.rs` (one process per measurement, cold index),
3 passes: four folders of the base pack (117 JSONs) read by one jar opening each (old,
`RUSTCRAFT_OPT_PREPSINGLE=0`) 744 / 756 / 733 ms; through the shared jar index (C1-M12) 38 / 47 / 41 ms
(50 ms without parallel decompression). The index is then reused by the integrated server (same
process). `RUSTCRAFT_OPT_PREPOVERLAP`: these reads run during the 6 JVMs (finished at 255 ms
against 1.3 s after Java in the old path). `RUSTCRAFT_OPT_PREPPAR`: the ≈ 1,500 item
component reports read in parallel: full `compile` (`prep_reads compile`, 3 alternating
passes) 603 / 324 / 346 ms in series against 206 / 211 / 203 ms (1st serial pass cold). Full preparation
(`examples/prepare.rs`, empty cache in `target/`, `RUSTCRAFT_PREPLOG=1`): 12.5 s new against
9.8 s old on ONE pass each, dominated by Java under load (JVMs finished at 12.2 s against 8.0 s):
inconclusive, to redo on a quiet machine (fresh-install scene). A single JVM for the five
extractors (`PREPJVM1`): not done (see report C4-M11: extractor sources in the cache
fingerprint).
### Allocation-free lookups by name (C1-M03, 24/10/2026, loaded-machine micro-bench)

Bench `name_lookup_bench` (ignored by default, counting allocator): 10 tags per name (short and
full), 5 property reads/writes per state on all the states, 6 shape updates
on one state out of three; 2 alternating passes, LOADED machine. Default = new, `=0` = old.

| Mode | tags ns/op (passes) | alloc/op | properties ns/op | shapes ns/op | shapes alloc/op |
|---|---|---|---|---|---|
| old (all at 0) | 99.8; 51.2 | 1.30 | 9.28; 5.09 | 451; 232 | 4.6 |
| all at 1 | 14.0; ≈ 15 | 0 | 2.29; ≈ 2.3 | 35.2; ≈ 36 | 0 |
| `TAG_BORROW=0` | 50.8; 54.0 | 1.30 | 2.20; 2.28 | 34.3; 36.6 | 0 |
| `PROP_IDX=0` | 13.8; 16.2 | 0 | 4.80; 4.91 | 35.9; 38.9 | 0 |
| `SHAPE_KEYS=0` | 15.1; 27.6 | 0 | 2.33; 4.10 | 134; 220 | 0 |

Threads (all at 1 against old, op/µs): tags 1 thread 69.5 / 8.9, 32 threads 745 / 34.6 (the old one
caps out on the shared allocator); shapes 1 thread 23.8 / 2.0, 32 threads 291 / 10.4; properties
32 threads 3,863 / 1,886. GPU not applicable (point lookups, branches). 7×7 decoration
(`decor_perf`, best of 5, 2 passes): 34.7; 33.7 ms against 34.1; 34.0 ms (all at 0), within
noise, as expected (place_tree 0.09% of flight). The three leads kept at 1; decision on the scene bench
(flight) on a quiet machine.

Same pattern across the repository (B6, 24/10): the composite key on the stack goes through
`rc_data::ids` (`with_key`, `with_full_id`, same switch `RUSTCRAFT_OPT_TAG_BORROW`), which the
generator helpers now call. Sites converted: End city pieces, terracotta
bands and the palette's `intern` (lookup before allocation) in rc-worldgen; tags
of the adventure predicates, recipe book category, mob variants and sounds, spawn
egg, dimension key read in NBT in rc-sim; item tooltip (components), icons
of the recipe book tabs (a string allocated per item compared, at every frame),
command suggestions, stripped wood and waxed copper, component ids in the client.
Left: the `format!`s that build a kept string (NBT, list, field), those of the tests,
and those that transform the name (`replace`) before the lookup. No bench of its own: same
technique as the tags above (≈ 100 → 14 ns, 1 → 0 allocation per lookup), cold
sites except the recipe book icons; to check on the scene bench with the rest.

### Blocks drawn as entities, item entities, map names (fidelity G2-F05, 24/10/2026, loaded machine)

Fidelity fixes, no optimization lead of their own. Cost added, by construction:
- block carried, displayed, in a minecart, falling, pushed or transient: a lookup in a
  per-state table (special model or not, properties read once) and a color per tint
  group of the already baked mesh (read of the client facts); meshes unchanged (cache per state),
  color carried by the instance. Falling, pushed or transient block: one biome read
  (blur zoom) per block and per frame, without blending (the game only reads one).
- special models: same renders as the block entities (layers baked once), one instance
  per layer; End portals: 36 vertices per cube.
- map names: shaping kept per text (short list, NBT comparison), one instance
  per glyph; names of the held map in a separate range drawn with the hand camera.
- ominous spawner: same item group as the ground item (shared function, matrix
  product of the ground item unchanged bit for bit).
No hot spot (a few entities per scene): multi-core and GPU not applicable. No
switch (no competing lead). Correction (C2-M05, 03/11): wrong for the redstone scene, where the
biome zoom of the pushed blocks costs 2.10 % of the process on the main thread; see the C2-M05
entry. To check on the quiet-machine scene bench: entity
scenes (displays, minecarts, running pistons) before/after, 1600×900 screenshots.
### Placement, survival and shapes per family (G6-F06, 24/10/2026, loaded-machine micro-bench)

Fidelity fix; items touched: placement (one player click), shape updates of the
fixed families (copper chests, plants, pale moss, giant mushrooms), flammability
table (built once at startup).

- Flammability: the by-name rule (`format!` strings per wood attempt) becomes the table read
  from the game (190 blocks sorted, binary search) + colored families. Temporary bench (all
  blocks ×100, 3 passes, LOADED machine): old 739; 547; 383 ms, new 6.0; 4.6; 4.5 ms,
  i.e. ≈ 45 µs instead of 3.8 to 7.4 ms per table construction (×85). Kept (more accurate AND
  faster), no switch: the old result was wrong (wool stairs and slabs,
  chorus flowers…).
- Placement: cold paths (once per click). Leads reviewed (B1/B2): no allocation
  added except the `format!` of the wall variant name (sign, fan, once per placement,
  like `standing_or_wall`); stacking: 4 property lookups only if the held item
  is the clicked block; End rod: neighbour read only for this block. Multi-core and GPU not
  applicable (one click, world order dependencies).
- Shapes: copper chest = one tag lookup (binary search) per chest update;
  pale moss carpet = at most 2 neighbour reads cached per call (like the game);
  giant mushroom = one block comparison. Nothing on the families not concerned (dispatch by
  `Kind`, resolved once per block).

To measure on the quiet-machine scene bench with the series (no scene places blocks in a loop:
`perf:spawn` for startup).
### Special glint of compasses and clock, mannequin cape (fidelity G1-F08, 24/10/2026, no bench)

Special glint: the game's projection only depends on the vertex position in the model and on
its face (the pose cancels out), hence two placements at no per-frame cost on the processor side. Entities
(hand, ground, frames, carriers): redone in the vertex shader (6-case table, under a
flag of the light word; the other vertices only pay a bit test, and 2 more interpolated
components) — no vertex data added to the static meshes. Icons: vertices
already transformed on the processor, so coordinates computed when drawing the slot (5 divisions
per vertex, only for compasses and the clock); atlas vertex 40 → 48 bytes for
all the icons (slots redrawn only on change or animation: a few KB).
Lead ruled out without measurement: passing the coordinates in a uniform per slot (one slot = several
quads of different faces, impossible). Multi-core and separate GPU compute: not applicable (per-vertex
work already on the GPU).
Mannequin: profile re-read (small buffer, a few strings) once more per frame for the cape
(if the part is shown) and for the elytra (if worn); rare mannequins, cost not measurable
on the expected scene bench. To check on a quiet machine: scene with compasses in hand and in the
hotbar, and scene of mannequins with a cape.
Follow-up (review): item displays in interface or first-person context, glint scale
chosen by a sixth flag of the light word (set once per layer, on the processor);
on the shader side, one more bit test only for the vertices with a special glint. No measurable
cost expected; scene to check on a quiet machine: displays of enchanted compasses in
`gui` context.
### Entities: kind, sizes, layers and registries resolved once (C3-M11, 24/10/2026, loaded-machine micro-bench)

Bench `bench_entity_prepare` (ignored by default, `tests_c3m11.rs`): real entity renderer
(GPU without a window, data and game 26.3), scene of 92 kinds (equipped mobs, babies, named,
burning, invisible, items, projectiles) copied up to N entities, `prepare` alone, median in µs,
configurations alternated at each round, main thread (1 core). LOADED machine (other agents
compiling); 5,000 entities too noisy to conclude (± 8% drift between configurations).

| Configuration | 100 (73 drawn) | 1,000 (740) | 5,000 (3,832) |
|---|---|---|---|
| old (all at 0) | 91.0 | 919.5 | 9,664 |
| all at 1 | 76.3 (−16.2%) | 766.4 (−16.7%) | 8,692 (−10.1%) |
| `EQUIPFLAG` alone | −11.1% | −12.6% | −5.0% |
| `KINDINFO` alone | −0.7% | −2.0% | −0.9% |
| `SIZEONCE` alone | −1.5% | −2.8% | noise |
| `OBJEARLY` alone | −1.5% | −3.1% | noise |
| `LIGHTONCE` alone | −0.9% | −2.4% | noise |
| `REGIDX` + `ITEMFLAGS` | −1.4% | −2.6% | noise |
| all except `KINDINFO` / `SIZEONCE` / `OBJEARLY` / `LIGHTONCE` | −13.7 / −13.3 / −14.5 / −14.9% | −15.2 / −15.4 / −16.2 / −15.6% | noise |

- `EQUIPFLAG` (responsible for the `is_contained_in` 6% of push_entity): equipment layer and invisible
  silhouette as flags of the baked layer instead of 2 to 3 substring searches per layer
  and per entity; product by the layer offset skipped when it is the identity (same matrix up to
  the sign of zeros, no pixel changes: `identity_product_keeps_matrix`); water fluid
  mask of the swimming player computed once.
- `KINDINFO`: table by type id (name verified, otherwise recomputed from the name): visibility
  box, displays, always drawn, crystals, painting/orb/item, skin family (a single
  function called instead of the cascade of 5), animal variant, flames, humanoid poses,
  own light. Not converted (to do if a profile justifies it): `animate`, `pose_rotations`,
  `living_scale`, `state_bits`, `shaking`, the family functions, `within_render_distance`.
- `SIZEONCE`: sizes (base, full) computed once in the `prepare` list, re-read by
  the display distance, the eye probe, the flames and the pose height of the entity
  in progress; another entity (guardian beam, wither, dragon) keeps the direct computation.
- `OBJEARLY`: `push_object` tests the kind before the light probe, the size and the matrix
  (before: thrown away for every mob). `LIGHTONCE`: block probe of the eyes kept for the entity
  in progress and its exact position (item then living, 4 copies of the illusioner).
- `REGIDX`: item registry found by index (already hashed by C4-M11: gain = one hash per
  call); `ITEMFLAGS`: crossbow, bow, trident, spear as flags per item, name of the item of the
  player's arms borrowed instead of copied (one allocation per hand and per frame fewer).
- `VARIANTIDX` / `NAMECACHE` (`MetaMap`, kept at every write): bench `bench_meta_lookups`,
  2 passes: variant 5.4 → 1.1 ns per read; custom name (Dinnerbone, jeb_, Toast) 64 →
  0.7 ns and no more allocation per frame and per named mob.
- Ruled out for now (not tried): layer cache per entity (`LAYERCACHE`): it needs a
  version counter touched by every equipment and meta write, and a time key for
  wolf and bee; announced gain ≈ 0.3%, risk of silent omission. Multi-core not applicable here
  (`prepare` fills ordered lists on the main thread, ≈ 1 µs per entity); GPU not applicable.
- Proofs: golden snapshot of `prepare` (`prepare_snapshot_same_under_every_switch`, fingerprint
  `0xf3e23c99f7b86fb5` of the matrices, instances per pass, silhouettes, shadows, free geometry)
  identical all at 0, all at 1, each switch cut alone and each lead alone; tables against
  old predicates for each kind and each registry item; `MetaMap` index against
  search over 20,000 random writes. Eight sabotages, all detected.
- Decision: all at 1 by default; each lead stays behind its switch
  (`RUSTCRAFT_OPT_EQUIPFLAG`, `KINDINFO`, `SIZEONCE`, `OBJEARLY`, `LIGHTONCE`, `REGIDX`,
  `ITEMFLAGS`, `VARIANTIDX`, `NAMECACHE`) until the quiet-machine scene bench (crowd, village,
  crowd-1000, equipped-humanoids, plains12 as a control).
### Per-state tables for blocks and mobs (C6-M02, 24/10/2026, loaded-machine micro-bench)

Benches `rc-physics bench_collision_top` / `bench_is_free` (real data, world mixing all
states) and `rc-sim bench_tables_c6m02` (new world), ignored by default; 1 pass, LOADED machine
(other agents compiling). ns per call; default = new, `=0` = old. Switches:
`RUSTCRAFT_OPT_SHAPEREF` (0/1/2), `RUSTCRAFT_OPT_ISFREE` (0/1/2), `RUSTCRAFT_OPT_MOBSTATES`,
`RUSTCRAFT_OPT_TAG_BITS` (0/1/2), `RUSTCRAFT_OPT_BLOCKIDS`, `RUSTCRAFT_OPT_SKYDARK`. Separate fidelity
fix (fence gate kept as a support by its block kind, not by the tag).

| Lead (switch) | old | new | other approach |
|---|---|---|---|
| shape top (`SHAPEREF`, 1 thread) | 48.4 (copy into a `Vec`) | 3.2 (table, `=2` default) | 10.8 (borrowed shape, `=1`) |
| same 32 threads (wall time / calls of one thread) | 173 | 11.9 | 47.7 |
| `is_free` (`ISFREE`, 1 thread) | 621 | 269 (`=2` default: exit at first overlap + thread buffer) | 360 (`=1`, without thread buffer) |
| `is_free` 32 threads | 1,258 | 743 | 882 |
| box collection (common walk + thread buffer) | 560 | 523 | — |
| tag by name (`TAG_BITS`) | 16.2 | 11.3 (`=2`) | 12.1 (`=1`, bit set per tag) |
| named tag `TagId` (`TAG_BITS`) | 13.9 | 2.1 (`=2`, 128-bit word per block) | 9.1 (`=1`) |
| walkable / swimmable (`MOBSTATES`) | 62.7 / 42.9 | 1.3 / 1.3 | — |
| suffocation (`MOBSTATES`, collision table) | 40.1 | 1.9 | — |
| tag of a named block, loot (`BLOCKIDS`) | 350 | 18.6 | — |
| sky darkening (`SKYDARK`, exact key) | 5.8 | 2.9 | — |

Placement: all these reads are done on the tick thread (integrated server) or on the movement thread;
multi-threading only serves to check the absence of contention (the table-based top scales,
`is_free` level 2 a bit less regular than level 1 between 2 and 16 threads: to re-measure on a quiet
machine). GPU not applicable (reads of a few ns, branchy, called one by one). Memory: top
table 8 B × states (≈ 240 KB), mob bits 1 B × states (≈ 30 KB), tags ≈ 16 B × blocks
+ one bit set per tag (≈ 50 KB in total). Decision on the scene bench (village, crowd,
plains32, objects, zoo-end, forest, ocean, Nether, cave, building) on a quiet machine; all the
leads stay behind their switch.
### Templates, jigsaw assembly and placement (C1-M11, 24/10/2026, loaded-machine micro-bench)

Benches `templ_tests::bench_*` (rc-worldgen, ignored by default), LOADED machine (other agents
compiling), 3 alternating passes, best kept. Default = new, `=0` = old path.

Starts of a 128 × 128 chunk square, warm (seed 20261011), ms:

| Path | 1 thread | 2 | 4 | 8 | 16 | 32 |
|---|---|---|---|---|---|---|
| world, old | 358 | 215 | 130 | 219 | 289 | 312 |
| world, all new | 163 | 83 | 41 | 25 | 21 | 15 |
| world, `TEMPL_TABLE` alone | 329 | 178 | 127 | 81 | 75 | 68 |
| world, `ELEM_ONCE` alone | 302 | 169 | 109 | 92 | 75 | 63 |
| world, `JIGSAW_PRECALC` alone | 310 | 145 | 94 | 81 | 61 | 59 |
| world, `POOLS_TABLE` alone | 351 | 198 | 118 | 221 | 285 | 312 |
| world, `JIGSAW_IDX` alone | 385 | 196 | 135 | 229 | 271 | 311 |
| Nether (bastions), old / new | 103 / 103 | 52 / 51 | 27 / 25 | 14 / 13 | 8 / 8 | 7 / 6 |

The old path GOES BACKWARDS beyond 4 threads (templates cache lock taken at every junction
tried, plus a string allocated per call): × 20 at 32 threads, × 2.2 at 1 thread. Each lead alone
lifts part of the lock; the combination clearly wins over each one (B3). `POOLS_TABLE` and
`JIGSAW_IDX` alone: within noise (the templates lock dominates), kept at 1 (pools lock
removed, no element copy). Nether: nothing to gain (few junctions per start).
Internal multi-core of the assembly: not applicable (priority queue and single random draw, sequential by
nature); the parallelism is between starts, and that is what the lock broke. GPU not applicable
(branches, hash tables, sequential randomness).

Placement of the 24 largest templates (315,534 blocks) straddling a chunk, list
`ancient_city_generic_degradation`, 2 passes: old 2.83; 3.12 ms, all new 2.68; 2.32 ms,
`PROCESS_BOX` alone 2.68; 2.32 ms, `PROCS_TABLES` alone 2.81; 2.81, `PROCS_SHARED` alone 2.78; 2.80.
First version of `PROCESS_BOX` (chosen blocks copied into a list, dynamic iterator):
3.02 ms against 2.67, LOSING; rewritten as a direct visit (layers in y, range in x by
binary search, test in z): winning. The y slice alone is useless in game (a chunk's area
covers the whole height): it is the x/z range that wins. Full decoration of
60 structure areas: within noise (≈ 270 ms, the other decorations dominate).
`STATETRANS_FAST` (state transformation tables at startup): 105.9 against 117.5 ms.
`LOCKED_SET`: no measurable case (few sources placed per template), kept at 1 (hashed set
instead of a linear search).

Leads not done: background preloading of the ≈ 1,500 templates (`TEMPL_PRELOAD`, mission: only
if a hitch at the first village remains measurable — to see on the scene bench); `TBlock.nbt` shared
between palettes (only templates with several palettes clone, and placement clones anyway);
terrain → state table of the Nether fossil and marker list per template (cold sites). The
RwLock variants of the cache (formerly C5-M02) are superseded by the lock-free frozen table.
### Terrain draw lists and reached sections (C3-M06, 24/10/2026, loaded-machine micro-bench)

Construction extracted into `apps/client/src/render/lists.rs` (pure function, compared frame by
frame with the old one). Switches, default = new, `=0` = old:
`RUSTCRAFT_OPT_VISORDER` (order of the candidate columns kept by (walk, origin, version of the
columns), built only when the key repeats; rank of the regions read once per column
then counting sort instead of a sort that hashed in every comparison; translucent sections
of a column merged from both ends instead of a stable sort; radius of the walk kept
by (version of the columns, camera column)); `RUSTCRAFT_OPT_REACHGRID` (reached sections
as a bit grid over their box, built by the walk task instead of a hashed set
redone on the main thread); `RUSTCRAFT_OPT_ARGSKIP` (indirect and indexed commands: only the
range that differs from the buffer's content is rewritten, nothing if identical).

`[perf]` log: counters `us_rayon_image`, `us_atteintes_image`, `us_retri_image`,
`us_listes_image`, `us_envoi_commandes_image` (average µs per frame, frames without a rebuild
included), to read on the scene bench (plains32-rotation, flight, ocean).

Micro-bench `lists_bench` (synthetic world: 24 sections, 5 out of 8 reached at random, hence series
more fragmented than in game; 400 frames, 1 core, µs/frame, two launches):

| distance | rotation | walking 0.07 block/frame | flight 1.1 block/frame |
|---|---|---|---|
| 12 | 99.9 → 88.9; 117.6 → 104.8 (−11%) | 98.9 → 91.7; 105.4 → 98.2 (−7%) | 72.5 → 69.3; 92.1 → 72.3 |
| 32 | 1,265 → 1,143; 1,448 → 1,366 (−6 to −10%) | 1,776 → 1,624; 1,634 → 1,539 (−6 to −9%) | 1,462 → 1,380; 1,365 → 1,277 (−6%) |
| 64 | 7,276 → 5,384; 10,317 → 10,277 | 6,891 → 10,712; 9,327 → 9,948 | 6,411 → 6,342; 16,280 → 12,042 |

Distance 64: loaded-machine noise larger than the difference (same code, ±50% from one pass to
the next); to redo on a quiet machine. Most of the time remains the emission of the batches (common to both
paths). First version (full order redone at every origin change): +11% in flight at
distance 12, hence building at the second frame with the same key.

Reached sections (same bench): construction set / grid 52 / 29 µs at 12, 405 / 209 µs at
32, 1,944 / 592 µs at 64 (taken off the main thread through the grid); query 6.2–9.8 ns / 1.5–2.1 ns
(up to 8 queries per entity box, particle, block entity, leash). Memory: grid ≈ 13 KB
at 32 instead of a set of 63,000 entries (≈ 1 MB). ARGSKIP: no isolated bench (memory copy
against GPU write of a few KB), counter `ko_ecrits_gpu_image` on the scene bench.

Not done (reason): `FRUSTSIMD`, `FRUSTRANGE`, `LISTPAR` (only the columns straddling an edge
do the per-section test; the per-plane predicate is monotonic in height, so a binary
search would give the same decision, expected gain small); `REBIND` (≈ 30 bindings per frame,
no full-render bench); `TRANSADAPT`, `SECTIONFREE`, `TRANSROLL` (the per-viewpoint re-sort
`TRANSPOV` replaced the per-column rotation); RenderBundle, `GPUCULL`, `HIZ`, `VISGRID`,
`PARTVIS`, `VISPARTIAL` (low priority in the mission, to scope after the scene bench).
### Meshing: per-block loop, border and face removal by shapes (C2-M10, 24/10/2026, loaded-machine micro-bench)

Paths set in `MeshJob::cull` (`rc_mesh::cull::CullPaths`, read once; `=0` cuts, `=1`
turns on): A `RUSTCRAFT_OPT_MESHPROPS` (property word per state — covers, faces to remove,
air, fluid, leaves, sealed — stored in an 18³ grid parallel to the states one, filled in
the same loop; the per-block loop and the sealed-section test read everything there instead of 6 + 6
table lookups), B `RUSTCRAFT_OPT_MESHPALPROPS` (interior read through the section's palette,
table of 2^bits entries; OFF by default), C `RUSTCRAFT_OPT_MESHROWS` (rows of 16: only
the "candidate" blocks visited, same order; 16.9% of the non-empty blocks of the capture are:
83.1% exit by "buried" or without a face), D `RUSTCRAFT_OPT_MESHPILE` (border stacks of the
neighbouring columns read in one go, one division per stack), E `RUSTCRAFT_OPT_SHAPECULL` (game
rule by face shapes, see below). Default: A + C + D + E. B and C read A's grid:
`MESHPROPS=0` cuts them with it (`CullPaths::grid_cut`), otherwise the grid would stay on.
Ruled out by reading: 3×3 table of
`locate` (already direct through `RUSTCRAFT_OPT_TINTIDX`, 68 calls per section), unfolded planes (1.125
reads per cell, no double work), `plane()` of `wire.rs` (no longer exists). GPU ruled out (sending
the grids costs more than the test).

Interleaved micro-bench `tests/cull_bench.rs`, 1,248 columns, all threads, best of 5 then of 12
(very loaded machine: 1 thread at 2.0-2.8 s instead of 0.46 s, uninterpretable, ±25% differences without
consistency from one pass to the next): old 131.4 / 127.8 ms; A −10.8 / −6.2%; A+B −7.0 / −6.9%;
A+C −10.4 / −13.3%; A+B+C −8.8 / −9.8%; D alone −3.4 / −0.9%; A+C+D −16.2 / −13.9%; default
(A+C+D+E) −10.6 / −16.4%; all (with B) −4.9 / −12.3%. B loses against A+C in both passes:
off, kept behind its switch (C6). E without A (rule applied face by face) +15.0 / +11.2%:
only a trial combination. Memory: properties 2 × 143 KB, shapes per state 143 KB, grid
23 KB per thread.

Removal rule (E, fidelity fix A3): the game hides a face if the neighbour's face shape
is its full cube (solid state, or full slice whose grid has only one cell on the two axes of the
side), or by the "same block" rule, or if its own (non-empty) shape is included in the neighbour's
(non-empty) one. The old rule hid any face whose neighbour had a box covering the
side: faces between slabs, stairs, dirt paths… too many, and conversely the face of a block that
does not occlude (glass, leaves) hidden by the underside of a stair, which the game draws. 159 distinct
face shapes, all in sixteenths; inclusion table checked against a sampling
(24,964 pairs). Fluids, same root cause, under the same switch E: the game removes a fluid face
if the neighbour's face shape is the full cube (except the top of a fluid lower than 1),
or if the face's rectangle (side: full width over 0..h; bottom: full; top: full and
h = 1) is included in a non-empty shape of the neighbour; the waterlogged block hides its water's face
if its own shape covers the whole side (union of the boxes). The old rule asked for a single
box covering the side: water drawn behind a waterlogged stair, against the back of a
stair, and low water (h ≤ 0.5) against a bottom slab, too many. Coverage of each shape
precomputed (largest band 0..H included), no inclusion computed in the loop. Proof:
`fluid_faces_follow_shapes` (built scene, 3 faces removed, stored fingerprint) and unit cases of
`face_rule_matches_known_states`. Capture: 6,193,742 → 6,189,376 faces (−4,366 of which 5 fluid, −0.07%); to measure in village and
building (faces, GPU ms, VRAM). Proofs: `tests/cull_paths.rs` (16 combinations × 2 rules ×
default / opaque leaves / without occlusion against the stored fingerprint, reference version and
partial re-meshing); sabotages seen (7): rows without fluid, shifted palette, stack x/z swapped,
inclusion forgotten, full cube without the grid, sealed too wide, empty own face ignored (unit
trial). To measure on a quiet machine: `scenes.py` cave, plains32, flight, forest, village, building,
2 passes, each switch at 0 (`MESHPROPS=0` also cuts B and C) and `MESHPALPROPS=1`; 1 thread of the micro-bench to redo.
### Decoration: tree sets, veins, freeze, geodes, names (C1-M13, 24/10/2026, loaded machine)

Fidelity first: the tree position sets now also follow the buckets that a Java hash
set turns into a red-black tree (more than 8 elements, table of at least 64);
4 such buckets out of the 59 decor_ref cases, 4 out of 120 random 3×3 windows, no
different placed block observed. Proof against the JDK: `tests/jset_java`. No switch.

Bench `decor_ref decoration_profile` (61 cases, best of 3 per case, trial profile), LOADED
MACHINE (other agents compiling), everything else at default; "without X" = `RUSTCRAFT_OPT_X=0`:

| Mode | total ms | veins (high iron) | geode | freeze | jungle |
|---|---|---|---|---|---|
| all (2 passes) | 55.5; 55.0 | 3.06; 3.04 | 3.15; 3.16 | 1.18; 1.18 | 1.66; 1.63 |
| none (the 11 at 0, 2 passes) | 62.1; 64.0 | 4.88; 4.86 | 3.82; 3.76 | 1.32; 1.33 | 1.94; 2.01 |
| without `JSET_KEEP` | 57.4 | 3.10 | 3.18 | 1.21 | 1.94 |
| without `TREE_TAKE` | 55.2 | 3.04 | 3.12 | 1.17 | 1.63 |
| without `TREE_FACESKIP` | 64.5 (noise: everything goes up) | 3.33 | 3.48 | 1.35 | 2.17 |
| without `BERG_TRIG` | 58.8 | 3.14 | 3.16 | 1.26 | 1.71 |
| without `ORE_COL` | 58.4 | 3.13 | 3.20 | 1.22 | 1.68 |
| without `FREEZE_THRESHOLD` | 55.9 | 3.09 | 3.16 | 1.29 | 1.73 |
| without `FREEZE_COL` | 60.8 | 3.20 | 3.31 | 1.21 | 1.74 |
| without `GEODE_REUSE` | 55.2 | 3.02 | 3.76 | 1.17 | 1.63 |
| without `GEODE_ROWSKIP` | 58.0 | 3.12 | 3.16 | 1.22 | 1.68 |
| without `DECOR_IDS` | 57.3 | 3.11 | 3.19 | 1.21 | 1.73 |
| without `SINCOS` (C1-M02) | 79.1 (noise) | 5.51 | 4.97 | 1.63 | 2.25 |

Reading: set −12% on the decoration (≈ 0.61% of flight, hence invisible to the scenes); the
veins mainly gain through `SINCOS` (one shared sine-cosine instead of two); `GEODE_REUSE`
−16% on the geode; `JSET_KEEP` and `TREE_FACESKIP` visible on the jungle trees (1.63 against
1.94 and 2.17); `TREE_TAKE`, `GEODE_ROWSKIP`, `FREEZE_COL` and `DECOR_IDS` within noise on this
bench. Micro-bench `jset_java jset_keep_bench` (2000 runs, 1 thread): 592; 603 ms against 603;
744 ms. All kept at 1 (none loses), decision on the quiet-machine scene bench.
Threads and GPU: not applicable (per-chunk work, already spread across chunks by the generation;
short branchy loops). Not done: merging the two temperatures (decor and materials);
hygiene only (each keeps its own noises; bringing them together requires proving seeds and
order of the f32 operations), no gain expected;
`note_missing` and the table of the biomes present (cold paths, one allocation per chunk).
### Text: memoized lines, flat font table, copy-free templates (C4-M15, 24/10/2026, loaded-machine micro-bench)

Three switches, default = new, `=0` = old, read once:
- `RUSTCRAFT_OPT_TEXTCACHE`: a line placed twice identically (full characters and styles,
  position, color, shadow, background, pose, clipping, vertex format, font set, suspect-glyph
  filter, visual order) copies its memoized vertices bit for bit. Never for an
  obfuscated line (draw at every placement) nor with an object in the text (atlas slots per frame).
  Memoized at the second placement only (a text that changes every frame copies nothing);
  forgotten after 64 frames without placement, 4,096 lines at most.
- `RUSTCRAFT_OPT_FONTTABLE`: flat U+0000-U+02FF table per font (first glyph of each code
  point, resolved at the first request: a TrueType font only renders what is used), glyph
  lent without copy (no more atomic increment of the sheet per character), font resolved once
  per run of the same style, default font kept by rank. Measurement (`Metrics::advance`) included.
- `RUSTCRAFT_OPT_LANGPARTS` (rc-text): translation template split in bytes, lent (copied
  only if it contains `%%`), no template copy nor `Vec<char>` per translation.

Bench `text_bench` (rc-app, ignored by default): 1,000 styled lines with shadow, scale 3, 50 placements,
2 passes, LOADED machine (other agents compiling), µs for the 1,000 lines:

| Mode | ASCII placement | ASCII width | Unicode placement | Unicode width |
|---|---|---|---|---|
| old | 5,660; 5,277 | 923; 859 | 3,305; 3,723 | 604; 673 |
| font table | 4,499; 4,321 | 685; 721 | 3,801; 2,966 | 674; 505 |
| memoized text | 2,491; 3,299 | 729; 866 | 2,133; 1,735 | 826; 566 |
| both | 3,232; 3,160 | 749; 726 | 2,508; 2,117 | 701; 554 |

A copied line mainly costs the copy of its vertices (≈ 17 KB for 55 shadowed characters in
triangles): the gain grows with the 4-corner format (C4-M14). Bench `flatten_bench` (rc-text,
ns per flattening + styled characters, 2 passes): translated chat message 1,180; 1,805 ->
1,030; 1,675, enchanted tooltip line 961; 1,466 -> 542; 1,285. Placement: all on the main
thread (interface placement, sequential by nature: vertex order); multi-core and GPU
not applicable here (the GPU already draws, the layout is glyph-sorting work at a few
µs per line). Memory: flat table ≈ 768 × 112 B ≈ 86 KB per loaded font (4 fonts);
line memory bounded (4,096). Decision on the scene bench (pause, inventory, building,
plains12) on a quiet machine; no switch cut.

Not done (reason): compact style (`CharStyle` with an interned font and grouped events,
≈ 24 B) — changes the public type read at ≈ 70 sites in 32 files, cannot be cut behind a
run-time switch (a single type layout per executable); separate mission.
Memoized `tr()` translation and `lang_gen` counter: depend on that same project (language
keys), not done. Per-index color recomputed at copy (fades) and translation of moved
lines: not done, the key contains color and position (a line that fades or scrolls
is placed again). Merging width + drawing for centered text: not done.
### Item atlas: borrowed model, common glint clock, animated sprites at the tick (C4-M20, 24/10/2026, loaded-machine micro-bench)

Three switches (default 1, `=0` old), read at the creation of the atlas:
`RUSTCRAFT_OPT_ATLASBORROW` (baked model borrowed instead of a clone per layer and per draw, which
also re-unfolded the quads of a model stored in pixels; special quads, pass vertices and freed
slots in kept buffers), `RUSTCRAFT_OPT_GLINTCLOCK` (a common 4 ms beat instead of a
clock per slot), `RUSTCRAFT_OPT_ANIMDIRTY` (a slot with an animated sprite is only redrawn if one of
its sprites changed at the tick; the glint alone follows the beat). Proof `item_atlas::tests`: atlas rendered
on the real GPU at 8 imposed instants (enchanted sword, book and banner, compass, magma, aquatic
lantern, chest), interleaved ticks, each switch alone and combined against the old one: equal
bytes; one sabotage per lead makes it fail (≈ 30,000, 16,000 and 2,000 bytes of difference).

Atlas passes over 1 s at 2,400 fps, 14 slots appeared one per frame (`glint_clock_counts_passes`,
exact count, no noise): glints 1,922 → 252 passes/s (common clock); animated sprites alone
1,922 → 33 (ANIMDIRTY). Processor time of the redraws (`bench_redraw`, 8 slots all redrawn
per frame with one tick per frame, best of 3, 2 passes): old 11.3; 11.8 µs, borrow 9.75;
10.1, clock 11.3; 11.6 (no effect in this bench where everything is due at every frame), dirty 10.2;
10.3, all 9.46; 9.75 µs. Multi-core not applicable (≤ 14 slots per pass, ≈ 10 µs, main thread
holding the encoder); GPU: the gain is the number of passes (GPU time per pass to read through
`gpu_time` on the bench). Only difference: between two beats, ANIMDIRTY shows the new frame of an
animated sprite up to 4 ms earlier (early, imperceptible). Not done: glint computed in `gui.wgsl`
(depends on C4-M14, to decide after measurement). All three kept at 1; decision on the
`inventory-enchanted` / `inventory` scene bench on a quiet machine.

### Bench memory readout: one pass per report period (C5-M01, 03/11/2026, loaded-machine micro-bench)

Only runs under `RUSTCRAFT_PERFLOG` (bench), never in normal play. The integrated server walked its
chunks in batches of 64 at every tick and restarted a pass as soon as one ended (~11 ticks for 700
chunks), while the `[perf] ram` line reads the totals every 5 s: 10 passes out of 11 thrown away,
~13 % of the singleplayer thread in the bench profiles (`root_parts` on the kept compound), outside
the tick measurement but in the process CPU. Now (`RUSTCRAFT_OPT_MEMWALK_ONCE_PER_REPORT`, default 1,
`=0` old path) a pass starts every 100 ticks, the first at the first tick; batches of 64 kept;
totals published at the end of the last batch (the old path published one tick later) and the
light layers held for the dedup released at once. Not spread over 5 s: it would hold freed layers
and mix states 5 s apart. Variant `RUSTCRAFT_OPT_MEMWALK_ON_REPORT_REQUEST=1` (default 0): a pass
starts when the report asked for one; same 5 s rhythm, kept off since a whole pass costs well
under the 2 ms that would have justified it.

Micro-bench `cargo test --release -p rc-sim --lib -- --ignored --nocapture memory_walk_pass_cost`
(700 synthetic chunks of 24 sections, 1,000 ticks, 1 thread, loaded machine):

| Path | Walk per 100 ticks | Worst tick | Passes in 1,000 ticks |
|---|---|---|---|
| continuous (old) | 7.161 ms | 0.232 ms | 84 |
| one per 100 ticks | 0.926 ms | 0.101 ms | 10 |

Equivalence: same frozen world, the three rules publish the same totals, equal to stored ones
(`spaced_pass_publishes_the_same_totals_as_the_continuous_pass`); 100 ticks = one pass started and
published, no layer held between passes. Multi-core and GPU not applicable (≈ 1 ms every 5 s on
the singleplayer thread). To measure on a quiet machine: `samply` share of `root_parts` +
`nbt_heap` (expected ~0.5 % -> < 0.05 % of the process) and process CPU on plains12, plains32,
forest, cave, village, generation, crowd-1000; median tick unchanged (measured before the walk).
Same pattern left elsewhere: the generator recounts its whole map after every batch under
`RUSTCRAFT_PERFLOG` (plain counters, no NBT).

## Ruled out (to re-measure)

| Building block | Variant | Result | Why |
|---|---|---|---|
| Effect re-applied every tick (turtle helmet, G4-F09, loaded machine) | no attribute recomputation for an effect without a modifier | 4.0-4.2 µs against 5.3-5.7 µs per player tick | changes the result in an edge case: this recomputation also caps absorption after a `max_absorption` change by the equipment; to take up again if this capping moves to the attribute change (like the game) |
| Textured meshing, 1 core (bench 1,248 columns, 1.195 s) | light computed from the first access to a cache cell (a single generation) | 1.233 s (+3.2%) | many cells are only used for properties (shadow, permeability): reading their light costs more than the second generation |
| Textured meshing, 1 core | biome blend memoized per (kind, y cell of 4, x, z) | 1.195 s (±0%) | the tints are already kept per block; the blend is not a meshing cost (the profile counted the preparation check) |
| CFB8 decryption | pool for ≤ 64 KiB | 72 µs against 67 µs (64 KiB), 46 µs against 17 µs (16 KiB) | distribution cost higher than the computation; threshold set at 128 KiB |
| CFB8 decryption | 16 KiB pieces | ×2 at 256 KiB against ×6.6 with 4 KiB | worse distribution |
| Decoding of `data.bin` | states collected one by one in parallel | 1.77 ms against 0.86 ms on 1 thread | element-by-element collection too expensive; replaced by batches |
| CFB8 encryption (sending) | any parallelization | impossible | each byte depends on the previous encrypted one; ceiling 67 MiB/s |
| CFB8 encryption (sending, C5-M39, loaded machine) | sliding register view over a 16 + 256-byte window | 47.6 against 49.7 MiB/s (1 MiB, 1 core) | the 1-byte write then 16-byte read still cannot be forwarded; off behind `RUSTCRAFT_OPT_CFB8_SLIDING_WINDOW=1` until the quiet-machine scenes |
| Textured meshing | 18³ grid of the fluid type to skip water surrounded by water | 0.598 s against 0.591 s | the grid costs what it saves; the ≈ 190 ms of the fluids are in the visible surfaces |
| Section face communication | GPU (compute shader, label propagation, one group per section; RTX 5080) | 6.2 ms with mask upload, 1.8 ms resident data, against 1.9 ms CPU all cores (dilation) | the upload dominates and the CPU matches; to take up again if the sections live on the GPU (GPU meshing); result identical to the CPU on the 13,768 sections |
| CPU per frame after the fidelity fixes (translucent sorting with water, 32 textures, game depth) | A/B 74ec2cb against 35aa698, 3 passes | 0.56–0.59 → 0.62–0.63 ms | cost spread out, each cause within noise (±0.03 ms): translucent series, re-sorts cut, 16 textures tested one by one without a clear difference; kept because necessary for fidelity; translucent column list cached and series buffer reused (no measurable gain) |
| Textured meshing | "identical block" bits computed for all blocks, buried ones included | 0.738 s against 0.505 s | `block` reads for nothing on the buried blocks; computed after the burial test: 0.457 s |
| Chunk reception (decompression + decoding, 1,547 chunks, 32 t) | decompression buffer reused per thread instead of a new one per packet | 1.72 ms against 1.77 ms | −3%, i.e. 0.05 ms on a full loading; each pool thread would keep its largest packet (up to 8 MiB); CPU 1 t not measured separately (same difference expected), GPU not applicable (sequential zlib stream) |
| Presentation (test map) | DX12 instead of Vulkan | 2,260 fps, CPU 0.309 ms against 2,792 fps, 0.241 ms | Vulkan keeps the advantage; loop measurement: presentation 0.09 ms, acquisition 0.012 ms, winit loop ≈ 0.04 ms per frame |
| Face drawing (bench, A/B 3 passes) | indexed: 4 corners + index buffer [0,1,2,0,2,3] instead of 6 non-indexed vertices (lead A4) | GPU 0.181 → 0.212 ms, 2,515 → 2,168 fps | slower on RTX 5080: the instanced batches of 6 vertices do not benefit from the vertex cache, indexing adds the index read; to re-measure with a compact face format |
| Textured meshing with lighting (bench, 1,248 columns, 1 t) | (measurement) cost of smooth lighting + AO | without light 0.548 s, flat path alone 0.580 s, smooth 1.236 s (75 ms all cores) | samply profile: `quad_light` ≈ half of the meshing, mostly the ≈ 25 cache accesses per quad; to take up again: neighbourhood read once per block and not per face |
| Light at meshing | 20³ grid of levels filled in advance per section | 1.29 s against 1.236 s (direct reads) | fills 8,000 cells per section whereas only those near the faces are used |
| Light at meshing | lazy cache per cell (state, light), emptied by epoch number; full quads without weighting; neighbouring columns precomputed | 1.19–1.24 s | kept (exact: identical mesh fingerprint), gain within noise |
| Full sky (bench, A/B 3 passes, view toward the sky) | (measurement) glow, sun, moon, stars, "fancy" clouds range 64 | CPU +0.005 ms, GPU +0.004 ms | negligible cost; clouds re-meshed only at a cell change. Note: the "Bench" player was reoriented (yaw 0, pitch 0) on 11/10 around midnight; the absolute view figures of the before/after bench are not comparable |
| (Bench note) | bench world modified on 11/10 around 23:55 by `tools/testmap/lighting.sh`: area (−67..−57, 88..93, −193..−183) emptied then stone roof, torch, lanterns | — | the bench's spawn view differs from this point on; A/B always valid (old and new see the same scene), absolute figures not comparable with before |
| 5×5 biome blend | (measurement) blend cut | 1.46 s against 1.50 s | not a hot spot: 3% of the textured meshing at the time |
| Integrated server chunk tables (wave OPT1B, 19/10) | FxHash hashing (fixed seed) instead of foldhash for `chunks` and the per-tick sets | micro-bench `rc-sim/examples/hashing.rs` (1, 2, 4, 8, 16, 32 threads; radii 6, 12, 32): lookup −5 to −8%, player advance −10%; but tick bench (3 passes): B alone 3.89–4.01 s against base 3.91–4.01 s, A+B 3.81–3.92 s against A 3.74–3.87 s (+1.5%) | no gain at the tick (hashing is not the cost of `block_at`); foldhash kept |
| Chunk tables, other leads (same micro-bench) | 64-bit packed key hashed as is (identity); Fx of a packed key; toric 2^k window indexed directly | identity 10 to 90 times slower (hashbrown draws its tag from the 7 high bits, constant); packed Fx 2 to 3 times slower in lookup; 2^k window −15% in lookup, −40% in advance | identity and packed Fx losing; window: micro-bench gain only (hashing does not weigh at the tick, see previous line), rewrite of 107 uses of `chunks` not justified |
| Client column grid (`rc_world::ChunkGrid`, same micro-bench) | cell `x mod N` (N = 2R+3, not a power of 2: eviction like the game's cache) through Lemire's modulo instead of division | current grid 40% slower than foldhash in lookup (6.0 against 4.4 ns, neighbours 4.1 against 2.1 ns: two divisions per access); Lemire: 6.3 ns (no better) | kept as is; a power of 2 would change the eviction cell (difference from the game); to take up again if `environ::state`/`light` become hot |
| Light after block changes (finding OPT1B, tick bench) | (measurement) share of `rc_light::relight` in `flush_block_changes` | 55% of the total tick time; spikes of 7 to 24 ms every 5 (water) or 30 (lava) ticks, groups of 20 to 50 changes of a flow; tick median < 1 ms, mean ×2 to ×3 | outside the wave's files (crate `rc-light`): next lead of the tick (zone per group too high for the sky, zone arrays allocated per resolution, background thread like the game's engine) |
| Sound table (C4-M04, loaded machine) | baking of `SoundTable` into the resource cache | not implemented: table 7-25 ms (C4-M03: 15-94 ms), now read on the "audio-engine" thread (startup) or "resources" (F3+T); wait at the join 0.05-0.07 ms | under the 10 ms threshold on the path of the 1st frame: the cache would only shorten a background thread; `sounds.json` and the lists of the `.ogg` and `sounds.json` are in the cache snapshot (lists read before `finish` and files passed to `finish` at the first launch: guaranteed even if the "audio-engine" thread reads after the recording); to re-measure if the join waits on the scene bench |
| Audio device tracking (C4-M03, loaded machine) | `RUSTCRAFT_OPT_AUDIODEVIDS=1`: re-read the names only when the ids change | ids 0.06-0.08 ms against full list 0.29-3.5 ms per readout (1 per second, background thread) | kept at 0: gain off the main thread (≤ 0.35% of a core) and a device renamed without changing its id would no longer be seen; superseded by `AUDIO_DEVICE_EVENTS` (C4-M29), which only reads it when that one is 0 |
| Audio listener written on change (C4-M29, loaded machine) | `RUSTCRAFT_OPT_AUDIO_LISTENER_DIFF=1` | 24.0 / 25.7 ns per frame still, 34-39 moving, against 19.8 / 21.0 | kept at 0: comparing 9 components costs more than 10 plain stores on x86 |
| Audio poll rate limit (C4-M29, loaded machine) | `RUSTCRAFT_OPT_AUDIO_PUMP_RATE_LIMIT=1`: per-frame poll at most once per ms | 33-47 ns per frame against 19.8 / 21.0 | kept at 0: the clock read costs more than the poll it saves |
| Sound table sorted listing (C4-M29, loaded machine) | `RUSTCRAFT_OPT_SOUND_TABLE_SORTED_LISTING=1`: binary search instead of a set of the `.ogg` paths | load 14.3 / 14.6 / 18.6 ms against 11.6 / 12.6 / 16.3 | kept at 0: the set costs 0.12 ms; the real cost is the 3.2 ms listing, which needs a cache-aware existence query |
| Biome tint of block pieces (G2-F10) | without reusing the previous color (5 × 5 blend per piece) | 169 µs against 5.6 µs per break | 25 biome zooms per piece for the same color |
| Shared fluid flow (rendering + physics) | generic function instead of `&dyn Fn` | 1.500 s against 1.502 s (1 t) | no gain: the +1.4% overhead (1.480 → 1.502 s; 88.2 → 89.4 ms all cores) comes from the `blocks_fluid_flow` tag now being honoured, not from the call; keeps the generic version |
| Mob push (C6-M07) | 16³ section grid (`RUSTCRAFT_OPT_MOBGRID`) | **postponed, not measured** (not ruled out); prefiltered O(N²) residual: 22–27 ms per tick at 1,600 mobs, 0.06 ms at 100 | open lead, hottest remaining point of the push; not attempted here because the positions are written directly in ≈ 94 places and other mobs move during a mob's tick: a grid rebuilt once per tick does not see a mob that has entered the area since (re-checking the candidates is not enough); an exact grid requires intercepting every position write (candidates then re-sorted in list order: same result); dedicated mission to open; multi-core and GPU not applicable (sequential pushes, list order) |
| Mob dimensions (C6-M07) | dimensions cache (`RUSTCRAFT_OPT_DIMSCACHE`) | not implemented; a pass costs ≈ 16 ns per box (micro-bench) and the prefilter already avoids most of them | pose, age, size and species state are public fields written everywhere: a hand-invalidated cache can serve wrong values in production (safeguard in tests only), contrary to A1 |
| Mob lookup (C6-M07) | `ent_view`: one lookup instead of 3-4 | not attempted; measured bound of the gain (`bench_crowd`, loaded machine): 3 / 14 / 49 reads per id per tick at 100 / 400 / 1,600 mobs, 8.1 to 9.8 ns per read, i.e. at most 0.5 µs per tick at 1,600 (median tick 25.7 ms: 0.002%) | even removing all the reads, the gain stays under the tick noise; to take up again only if a profile shows these reads |

## Common bench, series 2 (C4-M01, 22/10)

`tools/bench/scenes.py` (help at the top of the file): non-measured warm-up launch per exe,
"cache to write" launches marked cold and excluded from the medians (`--cold` keeps them), one
`RUSTCRAFT_CACHE` folder per exe under `target/perf/cache` (no more eviction between exes by the
KEEP = 3 limit, nothing more on C:), exe order rotated at every pass, 3 passes by default,
`--matrix` (switches alone then combined in the same exe), `noise.md` (dispersion of the
passes per scene and "under the noise" verdict of an A/B), camera-centered pre-generation
(radius 16, `--pregen-radius 13 --pregen-center site` = old model), 40 new scenes
(cold startup, interface, chat, block entities, movement, rain, particles, mobs,
synchronization, generation alone, LAN, save), generic counters
`[perf] [ t s] counter <name> <value>`. samply profiles bounded to the measurement window
(`tools/window.py`, `--whole-profile` for the old behaviour).

| Item | Measurement | Decision |
|---|---|---|
| Artifact plains12 1.57 s (re-run of 19/10) | `table --label repasse --reanalyse`: colds excluded, meshed load plains12 1,347 / 1,416 / 1,344 ms (ref / v1 / v2) instead of 1,322 / 1,376 / 1,568; 1 cold launch excluded per exe, none elsewhere | exclusion by default; it was the first launch of a new exe (resource cache and driver shader cache cold), now measured separately (scenes cold-resources, first-launch, fresh-install) |
| samply profile of crowd (map of 19/10) | whole profile: pool 49.1% of the CPU, generation 0.5%; window 30-49 s: pool 0.9%, generation 0, main thread 95.6% (100.4 s → 19.4 s of CPU counted) | hot maps on the window alone; the old map overweighted the startup meshing and generation |
| Pre-generation, new scenes | to measure on a quiet machine: `prepare` to redo ("chunks generated" line expected at 0 in crowd and redstone; this 0 is only proof if the generation scene generates, the bench cache having to contain `generated/reports`), then pass of each new scene | — |
| mimalloc pages committed on demand (`MIMALLOC_PAGE_COMMIT_ON_DEMAND=1`, quiet-machine bench, `target/perf/ram-table.md`) | private memory: pause 1,666 → 931 MB, flight 2,376 → 1,612, cave 2,066 → 1,164; fps, p99 and CPU equal (within noise) | adopted: `mi_option_set` at the head of the client's `main` (integrated server = same process), `RUSTCRAFT_OPT_COMMITDEMAND` (default 1, `=0`: library default); a `MIMALLOC_PAGE_COMMIT_ON_DEMAND` set by the user wins |
| Stronghold rings computed at creation (`RUSTCRAFT_OPT_RINGS_WARM`, quiet-machine bench) | crowd generation −20%, flight p99 −10% | adopted: default moved to 1 (`=0`: laziness per stronghold) |

## Common bench, series 3 (X-M02, 03/11)

Scenes for the leads the 15 reference scenes do not see (generation, rendering, interface,
off-world screens, disk, integrated server, LAN, mobs and commands): 71 new scenes (65 → 136). The `perf` data
pack moved to `tools/bench/perf_pack.py` (one function per staged scene, plain text computed
from the two pads of the model, same text on every run), off-world fixtures to
`tools/bench/fixtures.py`. `--scenes` takes groups (`@reference` = the 15, `@reference-extended`
= the existing scenes missing from the reference table, `@generation`, `@first-use`, `@render`,
`@interface`, `@off-world`, `@disk`, `@server`, `@lan`, `@mobs`).

| Item | Content | Status |
|---|---|---|
| `RUSTCRAFT_GENLOG: "0"` of the fresh-world flights | the client tests the variable for presence only: "0" left the generation log on, so the before/after of that lead measured the same thing | key removed (the flights need the log); `--env=-VAR` / `--exe "path\|-VAR"` now removes a variable (the `=` is needed: argparse reads `--env -VAR` as an option), to price the log: `--exe a.exe --exe "a.exe\|-RUSTCRAFT_GENLOG"` on `@generation` (to measure, quiet machine, goal < 0.1%); test: no presence-only variable (read only through `env_flag!`) is ever set to an "off" value by a scene (sources scanned) |
| Staging commands typed in the chat | `pack-files` also writes every scene's chat commands (`<out>/chat-commands/<scene>.txt`, positions on the flat pad); `tests_perf_pack::scene_chat_commands_compile` compiles them at the singleplayer host's level 4. It found two silent faults: `publish false creative 25599` (since d239ed33, every LAN scene: 26.3 takes `publish [allowCommands] [port]`, no game mode, so the world was never published and no guest could join; no LAN figure was recorded with it) and `save-all` of `bee-nests-*` (a dedicated-server command, unknown in singleplayer) | fixed: `defaultgamemode creative` then `publish false 25599`; nests saved by `RUSTCRAFT_AUTOSAVE_TICKS=200`. Scene `signs-1` renamed `signs-chests`: `signs-1-<p>` A/B folders of `signs` were reread as passes of `signs-1` (test: every scene's folders read back to it) |
| Generation (C1) | `fresh-nether-rd16`, `fresh-swamp-rd16`, `fresh-taiga-rd16` (marked mushrooms, light post-processing; = `newworld-nether`/`newworld-taiga` of C5), `fresh-badlands-rd16` (eroded surface), starts located by `seed-model` (`fresh-paths.json` of the model: 192 blocks inside the biome); first uses `first-locate`, `first-place`, `first-bonemeal` (16 uses on a sapling), `first-ender-eye`, `first-nether-trip` | `seed-model` to redo (paths absent: scenes skipped with the reason) |
| Rendering (C3) | `maps-1` (900 distinct locked maps in item frames, map data written by `prepare`/`pack`: number in binary in a corner, no two alike), `hidden-1` (200 cows in closed houses and behind a ridge: a test casts the rays and checks that none is visible), `flight-rotation-32` (rotation at distance 32 then 4 teleports of 512 blocks), `ocean/forest/village-transparency` | ready |
| Interface (C4) | `survival-hud`, `hud-busy`, `inventory-enchanted-36`, `trade`, `tab-20`/`tab-80` (19/79 LAN guests, 4 teams, hearts), `chat-flood`, `creative-search`, `creative-tab`, `recipe-book`, `cmd-give`, `stats-screen`, `advancements`, `f3-open`, `team-crowd(-named)`, `signs-chests`, `displays-1`, `structure-show-air`, `packs-20`, `reload-f3t`; off-world `multiplayer` (6 fake status servers on 127.0.0.1 with favicons), `world-list` (20 worlds), `world-create`; disk `world-create-play` (seed 1234 typed), `open-from-list`, `long-flight-save` | ready; not done: recipe-book search typing (the book driver has no query) and `vsync-mouse` (needs SendInput in a focused local session); `tab-80` costs 79 guest processes (version data each): RAM to check first |
| Integrated server (C5) | `long-flight` (10 min), `hoppers-1`, `smelter-1`, `sculk-1`, `carts-1`, `items-merge-1`, `redstone-dense`, `tnt-cannon`; `autosave-1` = the existing `save` | ready |
| LAN (C5) | `crowd-1000-lan-1`, `tnt-lan-1` (guest brought to the host), `lan-4-spread` (guests 160 blocks apart), `lan-1-join` (window over the guest's join), `redstone-published` | ready; per-thread CPU through `--samply` + `tools/cpu_profile.py`, no bytes/packets counter in the log yet |
| Mobs and commands (C6) | `village-night`, `raid`, `crowd-1000-melee`, `crowd-1000-archers`, `pen-babies`, `crowd-villagers`, `trial-chamber` (chunks per save), `bee-nests-night/rain` (autosave every 10 s), `nether-fortress` (spawning on, everything killed each tick), `end-fight`, `swamp-bastion` (on the pad: frogs, axolotls, immune piglins and hoglins), `survival-inventory`, `survival-crafting`, `datapack`, `tnt-first-volley` | `trial-chamber` and `nether-fortress` need `prepare` again (views located there) |
| New readings | `tick max` (longest tick of the window: the server's 100-tick report has p99 = rank 99 of 100 = their maximum; the server line has no date: the report is dated by the client line before it when the next one follows within 1.5 s; across a longer gap (client stall) it counts only if the whole gap is inside the window), `tick p99 last minute` (NaN when the scene has no full minute after the start of its window: it would count loading ticks), chunks per save, resource reload, entities and block entities drawn, allocator and server chunk data (last value and growth over the window, `[perf] ram` line); `bruit.md` gains a "load of the passes" table ("differs" when a deterministic scene's load moved between passes) | ready |
| Safeguards | a scene is skipped, with the reason, when its view, located path or model is missing, or when an exe does not contain the name of a driver variable it needs (older version); placeholders `{view:NAME}` / `{site:NAME:dx:dy:dz}` fail explicitly on an unknown name; `crates/rc-sim/src/tests_perf_pack.rs` compiles every function of the pack with our command parser (sabotage: 3 invalid lines → 3 functions refused with the game's message, among them a `team join` with two members, which the game refuses) | ready |
| Micro-bench kit (`crates/rc-sim/src/bench_kit.rs`) | flat world with the bench's own pack, timed ticks, allocation calls with `--features memstats` (mimalloc behind a counter), `[bench] <bench> <case> <metric> <value>` lines tabled by `scenes.py micro` | first bench `bench_datapack_tick` (below) |
| `bench_datapack_tick` (loaded machine; per world, 40 warm-up ticks then 3 runs of 200 ticks, median µs per tick; 2 worlds per launch, one line each; allocation calls per tick with `memstats`, deterministic: ±0.1%; the case fails unless the macro of the `#tick` workload wrote 7 and `#blocks` ≥ 32 × 600, so it cannot time the crowd alone) | first launch (`scenes.py micro` of its lines): crowd of 1,000 animals alone 9,452 µs (best 7,795–10,896), 2,962 allocations; + `#tick` workload 21,929 µs (best 16,949–17,899), 45,036 allocations; + 256 repeating command blocks 110,533 µs (best 97,171–109,081), 216,605 allocations. Rerun 03/11 with the checks, per world: 12,616 / 8,463 µs; 27,876 / 21,018 µs; 108,822 / 95,561 µs | finding for the command missions (C6-M12, selectors): the `#tick` workload ≈ +8 ms and +42,000 allocations per tick, the command blocks ≈ +85 ms and ≈ 670 allocations each per tick; 4 of their 8 commands select entities by distance among 1,000 (lead: spatial lookup for distance-bounded selectors, no allocation per test); timings to settle on a quiet machine |

To measure on a quiet machine before the series (D1): 3 passes of `@reference-extended` and of
each new group on main, noise of every new scene written into the reference table; no decision
below that noise.

## Reference baseline

GPU time (C3-M01, 22/10/2026): all the GPU figures of this log prior to 22/10/2026
cover the WORLD pass ALONE. Since then, the readout covers the whole frame (timestamps in
the encoder on RTX 5080/Vulkan; fallback: start of the first pass, end of the last;
`RUSTCRAFT_OPT_GPUFRAME=0` = old readout) and `RUSTCRAFT_GPUPASSES=1` breaks it down by named pass.
Synthetic GPU bench (`gpu_time::tests`, 12 full-screen 1600 × 900 passes, loaded machine):
pixels identical in the 3 modes with and without breakdown; encoder total 0.561 ms, fallback
0.560 ms, world alone 0.047 ms; with breakdown total 0.591 ms for a sum of the passes of
0.557 ms (the 24 timestamps measure themselves: ≈ 2.5 µs per pass in the GPU total); wall
time per frame (300 frames, 3 passes): without 0.580–0.582 ms, total 0.579–0.586, fallback 0.582–0.585,
total + 12 passes 0.583–0.585: within noise. GPU reference of the scenes to redo (scene bench,
quiet machine).

Reference of the full client before optimization (development version of 19/10/2026, 12 singleplayer scenes, 3 passes):
bench `tools/bench/scenes.py`.

| Building block | 1 core | All cores |
|---|---|---|
| Textured meshing (models, 5×5 tints, fluids), 1,248 real columns | 1.50 s | 89.4 ms (×16.8) |

Textured meshing costs 8.8 times the wireframe (171 ms): next optimization target.

Light (C2-M01, 22/10/2026, loaded bench: other agents were compiling; to re-measure on a quiet machine).
Benches `rc-light` `relight` (trace replay, `RUSTCRAFT_LIGHTTRACE`) and `light_chunk` (capture,
1,248 columns), each verified first against the frozen copy `rc_light::reference`; traces outside the
repository in `%LOCALAPPDATA%\rustcraft-re\traces\lumiere` (recorded by `bench_ticks`).

| Measurement | Value |
|---|---|
| `bench_ticks` (2,000 ticks, 32 threads): share of the light | 345 relights, 4,488 changes, 2.9–3.0 s out of 6.2–6.3 s; sky 952 resolutions 1.23–1.41 s, block 1,614 resolutions 1.50–1.81 s (Nether: block alone, 0.86–1.25 s) |
| Relight, 1-thread replay, order A B B A (work, reference, reference, work) | surface 409 relights: work 3.61 / 3.47 s, reference 4.19 / 3.37 s; Nether 100: work 1.04 / 1.05 s, reference 0.86 / 0.90 s; synthetic 400: work 1.04 / 1.19 s, reference 0.83 / 1.17 s |
| Difference proper to the loaded bench (two passes of the same path) | up to 24% (surface reference 4.19 against 3.37 s; synthetic 0.83 against 1.17 s); the work/reference direction changes depending on the trace and the order, except Nether (work slower by 15 to 20% in both orders, cause not settled: the frozen copy has no counters nor `bounded` parameter and its machine code differs). First measurement (work always first, 4.50 / 4.82 s; 1.30 / 1.62 s; 1.29 / 1.50 s): effect of order and load, not a code difference |
| Rule for the `rc-light` missions | the frozen copy serves for equivalence only; a gain is judged against the old work path (`main`), in A B B A order, and on a quiet machine |
| Light of a new chunk, 64 columns 1 thread | 258–295 ms (4.0–4.6 ms/chunk) |
| Same, 256 columns, 1 / 2 / 4 / 8 / 16 / 32 threads | 1.09 s / 527 / 346 / 161 / 87 / 142 ms (16 threads = ×12.6; 32 threads slower, loaded bench) |
| Allocations | relight: 426 allocations / 1.3 MiB (synthetic), 839 / 3.4 MiB (surface), 5,634 / 5.8 MiB (Nether); chunk: 164 / 1.2 MiB |
| Difference from the official game (light of the capture's packets) | sky 11,357 cells out of 40.3 M (of which 8,548 within 2 cells of the edge), block 421 out of 15.7 M (298 at the edge); sheets present on one side only: 2,754 sky, 8,754 block |

Mesh equivalence proof (22/10/2026, base commit `08df4a1`): `rc-mesh`
`tests/digest.rs` compares against STORED fingerprints all the mesh fields (layers, starts,
fluids bit for bit, visibility; `rc_mesh::blocks::digest`) of the 1,248 columns, set up as in
game (world bottom −64, zoom seed of the capture, plains for the absent columns), for
each variant: default `f9da7433…`, blend 0 / 1 / 7, without occlusion, opaque leaves, without
sky + nether shading, absent columns as desert, partial re-meshing (= default). The capture
(size + fingerprint) and the cache (states, biomes) are checked first: another capture gives
"different data", not "mesh changed". The earlier fingerprints (`b049d4a72f419192`,
`2459d36d7f95f34f`) were partial (no starts, no visibility, no fluid texture; world
bottom 0, seed 0): they are no longer references. `RUSTCRAFT_OPT_REQUIRE=1`: missing data
= failure (all the tests and benches that go through `rc-testdata`).

Bench `rc-mesh` `benches/mesh.rs` (`blocks_*_columns_*`): since 22/10/2026 it sets up its tasks
as in game (world bottom −64 instead of 0, zoom seed of the capture instead of 0, real plains
instead of biome 0). The earlier figures of the textured meshing (1.528 → 0.890 s; sealed
sections 2.182 → 2.153 s; 1.50 s / 89.4 ms above) are not comparable: baseline to re-measure
on 08df4a1 with the new bench, quiet machine.

Animated textures (1,251 → 1,791 layers, 10 KiB buffer rewritten at the tick if a frame changes,
fade by two reads only for the interpolated textures): GPU time on the bench world,
spawn view, 3 alternating before/after passes: 0.78 ms/frame in both cases (corrected readout: a script divided by two);
resource loading 107–110 ms in both cases (layer mips computed in parallel).

## To measure (as soon as the building block exists)

- Fidelity G2-F12 (24/10), cost of the new code, decision: keep (no alternative lead
  cut, hence no switch). Client heightmaps tracked at every changed block
  (4 maps, rule of `rc_world::heightmap` shared with `rc-sim`, descent only when the
  top is broken: same cost as the weather tracking already measured negligible); to re-measure on the
  `building` bench (mass placing and breaking) on a quiet machine. Canonical vertex order: at
  baking only (result cached on disk), 6 faces × 4 vertices per element. Blocked
  servers list: read once per launch (ping or connection thread), 2 to 5 SHA-1 fingerprints
  per resolution. F3 occlusion traces: copy of the nodes only when the input is
  active; walk `Node` keeps 16 bytes (not `u16` in the padding).
- Translucent re-sort (imperceptible series, I-06, 24/10; decision on the scene bench: flight
  over the ocean and the forest, scripted camera path; counter
  `retris_translucides_image` of the `[perf]` log, sections re-sorted per frame, in both
  paths). Switches, default = new:
  `RUSTCRAFT_OPT_TRANSKEY` (sort of a section: `0` = stable sort that recomputes the distances at
  every comparison; `pairs` = distances computed once, sorted pairs (distance, index);
  default = 64-bit integer keys, distance stored like `total_cmp` in the high bits, index in the low
  bits; same order bit for bit, test `trans_key_orders_match_stable_sort`). Loaded-machine
  micro-bench (`trans_key_cost`, ns per quad, 1 thread / 4 / 32): sections of 8: old 19.1 / 4.8 / 2.5,
  pairs 11.0 / 3.3 / 1.5, keys 10.9 / 2.7 / 1.4; of 64: 28.1 / 7.7 / 2.4 against 13.5 / 3.9 / 1.6 and
  10.2 / 2.0 / 1.1; of 512: 27.0 → 16.0 → 10.2 (1 thread); of 4,096: 30.3 → 16.4 → 10.8 (1 thread).
  The threads only serve the bench: in game, the re-sort stays on the main thread (a few dozen
  sections per frame after TRANSPOV); GPU not done (per-section sort of a few dozen
  quads, sending the centers costs more than the sort).
  `RUSTCRAFT_OPT_TRANSPOV` (game rule per section, `docs/specs/visibility.md` "Translucent
  sorting": re-sort if the section's clamped viewpoint changes, or at block change
  if it is on the axis; columns within ±2 examined every frame, rotation of max(n / 8, 15);
  no re-sort in improved transparency; one write per run of contiguous sections). `=0`:
  old path (after each block, all the columns re-sorted whole by rotation, more
  often than the game). Fidelity: follows the game, the old path re-sorted more. Expected gain in flight
  (distant off-axis sections never re-sorted as long as the camera stays in the same section):
  to quantify on the scene bench, quiet machine.

- Mesh upload (series 2, C3-M02, 22/10, decision on the scene bench plains32 #0, cave,
  redstone, flight, building; `upload *` columns of `scenes.py`). Switches, default = new:
  `RUSTCRAFT_OPT_MESHPACK` (`0` = buffer reused then copied into the base; `main` = flattening
  at exact size on the main thread; default = flattening in the pool task and mesh
  freed on the pool), `RUSTCRAFT_OPT_MESHPREP` (GPU bytes prepared on the pool: the main thread
  only does `alloc_in` + `write_buffer`), `RUSTCRAFT_OPT_TRANSFREE` (free translucent column:
  centers never computed), `RUSTCRAFT_OPT_PARTIALINPLACE` (partial with the same counts:
  only the differing range rewritten in place if the GPU holds the base version),
  `RUSTCRAFT_OPT_UPLOADBUDGET` (+ `RUSTCRAFT_UPLOADBUDGET_MB`, 16: whole columns of the nearest
  ones up to the budget, partials and priority updates never delayed). Loaded-machine micro-bench (2,000 drawn
  columns, µs per column, 1 thread / 32 threads): old preparation 33–50 / 8.6, + short-circuit
  20–22 / 27; pool 26–40 / 12.8, + short-circuit 23–31 / 6.4; flattening + copy 33 / 11.8,
  exact 26–31 / 9.2. The expected gain is the move off the main thread (≈ 20–50 µs per
  column), not the total cost. In place: 846 draws out of 1,000 and 76 without any byte. Ruled out
  without measurement (reason): GPU copy from the old range (same buffer forbidden, the old range
  alive pushes the new one elsewhere); allocation per section (indirect commands multiplied);
  `write_buffer` from the pool (`alloc_in` may grow and submit: write lost); "changed size fitting in the allocation"
  variant not done (the arena keeps no per-column remainder: to take up again with C3-M04).

- GPU terrain formats (series 2, C3-M03, 22/10, decision on the scene bench plains32, plains12,
  forest, cave, ocean, flight, building: allocator VRAM and `RUSTCRAFT_VRAMDUMP`, GPU ms,
  upload MB/s). Switches, default = new, same image: `RUSTCRAFT_OPT_OFFSETFACES` (offset
  models — grass, flowers, ferns, bamboo — in 24 B faces: offset class in bits 28-31
  of the position, seed nibbles above a 20-bit baked face index, table
  15 × 3 × 16 f32 in binding 9; offset translucents always explicit),
  `RUSTCRAFT_OPT_WATERDOUBLE` (back face of water and lava = bit 31 of the texture; the
  shader swaps vertices 2 and 3 of a triangle seen from behind, which gives back exactly the old
  back triangle; side decided like the rasterization, by the sign of the determinant of the
  clip positions (x, y, w) of the three corners, never by the uniform's eye, which the
  walking bob shifts by up to 0.1 block from the true projection center; triangles
  stored in the order of the old path, front A then B, then back B then A, which matters
  for a folded quad whose one triangle is seen from the front and the other from behind), `RUSTCRAFT_OPT_FLUID96` (region fluids in 96 B, one color; only
  if all the offset faces fit in a compact face), `RUSTCRAFT_OPT_TRANSCOMPACT`
  (sorted translucents in 108 B: 8 unused light bytes removed, absolute column read through a
  16-bit slot, 512 KiB table). Proofs: `rc-mesh/tests/formats.rs` (1,248
  columns, 453,451 offset faces and 285,338 double faces unfolded bit for bit),
  `tests/digest.rs` (old fingerprints unchanged, compact 0x07ba7d1880fd68e2),
  `render::gpu_test` (RTX 5080 Vulkan: five views drawn by the real entry points, 0
  different values, blend by maximum then water alone as "last triangle winning"; views
  from above, underwater, flush with an offset block, eye 0.03 under the surface with a 0.1 bob, folded
  water quad; sabotages seen: order of additions, vertex swap, side decided by
  `cam.eye` (610,785 wrong values in a bobbed view), triangle order not carried over (12,838
  on the folded quad)). GPU cost of the side test (`water_side_cost`, water-rich column 2,000
  times per frame, water alone, best of 9, loaded machine): all clipped (vertices alone)
  old 2.86 ms against 1.74 ms (−39%, 6 vertices per quad instead of 12), and 1.74 ms without the
  test: its cost (4 matrix-vector products, 2 determinants) is not visible; from above 13.71
  against 13.73 ms (+0.1%), underwater 10.87 against 10.95 ms (+0.8%, identical fill,
  within the noise from one pass to the next: +0.4% at the previous one). To confirm in GPU ms on the
  ocean and plains32 scenes, each switch alone, quiet machine.
  Micro-bench (`format_bench`, bench 1,248 columns, GPU bytes faces + fluids + translucents):
  old 278.6 MB; offset alone 231.4; double face alone 242.0; both 194.9; + FLUID96
  194.3; + TRANSCOMPACT 185.8 (−33%). TRANSCOMPACT alone 264.5. Meshing 1 thread 2.5–5.4 s old
  against 2.5–3.6 s compact, 32 threads 193–276 against 193–206 ms (loaded machine, strong noise).
  Image comparison in the same launch: steps `meshflip` (inverts offset faces and
  double face, re-meshes everything; offset faces kept if FLUID96) and `cap:NAME` of
  `RUSTCRAFT_CMDTEST`. Only possible difference: the draw order at strictly equal depth
  between an offset face and a neighbouring face of the same layer (drawn before in the same
  draw instead of the fluid draw); to measure on the screenshots. Not done: full faces in
  16 B (`FACE16`), small shader costs (`TEXINV`, `FACEHALF`, `OITFUSE`).

- Callers of the server relight and faithful `light_update` (C2-M04, `crates/rc-sim/src/play.rs`, 24/10, loaded machine). Three switches (`play::LightOpts`, `=0`: old path): `SKYBIT` (shared with M03): the changes that touch the sky are picked up at write time (`write_raw`) and passed separately to `relight_sections_split`, a lamp no longer restarts the sky resolution; `LIGHTSECTIONS`: update limited to the sections the game signals (changed sections and edge neighbours, created sheets and their 26 neighbours, sky and block separated), null sheet in the empty mask (also in the chunk packet and in LAN); `SIMLIGHTMISC`: dimension type picked up once per dimension (relight, spawns under a ceiling, weather), implicit sheets looked up once per (column, section), consecutive duplicate of a position discarded (exact; the full deduplication is `LIGHTDEDUP` of M03, off). Proof `tests_light_m04`: same server light at every tick old/new and each switch alone, client rebuilt from the packets alone = server, game section rule, empty mask; 4 sabotages seen. Test scenario (slabs, glowstone in a corner, lamp, water on a roof, removals, 5 × 5 columns sent): sheet bytes sent 1,921,024 → 497,664 (−74%), updates 46 → 102 (more neighbouring columns, each smaller). Defect of the old path fixed: a sheet created around a change in an intact column was never sent. CPU gain of the filter: negligible (micro-bench below). Sheets shared through `Arc` in the singleplayer channel: already the case (no 2,048-byte copy). Accepted differences (A3): a cell written then returned to its value in the same relight is not signaled (zone engine); a sheet already written that returned to 0 goes in the empty mask (the game sends it as data, same zeros received). To measure on a quiet machine: `scenes.py` redstone (median/p99 tick, sky share of the relight), lan (bytes/s), cave, building, village, flight, plains12, plains32; each switch at 0.
- Singleplayer client, relight cut (C2-M05, `apps/client/src/main.rs`, `light_engine.rs`, `crates/rc-light/src/received.rs`, 24/10/2026, loaded machine). The integrated server sends the light of the changed sections BEFORE the blocks of the same tick; the client restarted the local recalculation on the same positions (≈ half of the process's relight in redstone, on the main thread). `RUSTCRAFT_OPT_SOLONORELIGHT` (default 1, `=0`: local recalculation of everything, validation of the client engine): in singleplayer (LAN host included), a change received from the server does not restart the recalculation if its section received a sheet in the frame (`rc_light::Received`, emptied after the frame's recalculation); otherwise (light arrived in the previous frame, nothing changed on the server side) recalculation kept: a section written by a local recalculation on a client-own state (piston replay, prediction) never waits for a correction the server would not send. Predictions, acknowledgements and piston replay always keep the recalculation; remote servers unchanged. `RUSTCRAFT_OPT_SKYBIT` (shared) on the client side: `relight_masked`, sky resolved around the changes that touch it only. `RUSTCRAFT_OPT_LIGHTNOCOPY` (default 1): light update without any different sheet = column left as is (neither `Arc::make_mut` of the column held by a meshing task, nor the weather counter); identical shared (`Arc`) sheet recognized without comparing its 2,048 bytes (the integrated server sends back its own `Arc`s); identical biomes (`chunks_biomes`): neither copy nor re-meshing. Already done, nothing to code: sections of the empty mask applied (network: `read_light` sets the null sheet; singleplayer: null sheet carried); diagnostic variables read once (macro `env_flag!`, C4-M02); a single recalculation per frame (changes of a frame already merged). Proofs `rc-sim::tests_light_m05` (real integrated server, slabs, glowstone, lamps by block then by torch, water, lava, removals, emitter placed and removed in the tick; mirror clients following the client's path): old, cut, cut + sky bit, and the three with the packets of a tick shared between two frames: sheets = server bit for bit at every tick (0 different cells over 225 ticks); client alone (received light ignored) `relight_masked` = `relight_sections` bit for bit; sabotages seen: received light ignored (≈ 10⁸ cells), received sections never noted (cut without effect), sky list emptied. `remesh::tests::touch_light_reports_real_changes`: true only if a sheet differs, same sections to re-mesh with and without the shortcut. Test setup: positions passed to the local recalculation 1,552 → 89 (−94%), recalculations 64 → 35, local recalculation time (micro-bench, 1 pass, loaded machine, calling thread + light reserve) 157.4 → 79.5 ms (−49%; the groups whose light did not follow in the frame remain); sky bit on the client side: 79.5 / 81.2 ms cut, 88.0 / 88.6 ms client alone, within noise (kept on, bit for bit, to settle on the scene bench); tick shared over two frames (light arrived in the previous frame): 1,552 → 1,480, the cautious rule keeps the recalculation. Finding: the client engine alone (without the server's light) differs from the server by the presence of the sheets (the server creates the missing sheets around a change, the client alone does not); with the received light, the old path is already bit for bit. Check for the bench: `RUSTCRAFT_SOLOLIGHTCHECK=1` (singleplayer) keeps the received light per column and writes, after the recalculation of each frame, the differences of the touched columns (a one-frame difference = packets of a tick over two frames; a lasting difference = defect). Not done (low lead): relight on the thread reserve with copies and join before the meshing (`RELIGHTBG`, multiplayer only, join in the same frame hence gain bounded by what `LIGHTGROUPS` already does); distance buckets of `schedule_meshing` (`MESHSCHED`: the current choice is already O(n) through `select_nth_unstable`, and its order among ties is not fixed, hence "same sequence" without a reference). To measure on a quiet machine: `scenes.py` redstone, cave, building, plains32, plains12, flight (frame p99, relight ms/s, main thread CPU, packet time), each switch at 0, and `RUSTCRAFT_SOLOLIGHTCHECK=1` on the 13 scenes (no lasting difference).
- Lightened relight (C2-M03, `crates/rc-light/src/fast.rs`): new resolution path, bit for bit the old `solve` (kept as is, taken when all the switches are at 0) except `LIGHTDEDUP`: the sorting of the changes changes the groups, hence the zones resolved, the repair of inconsistent data (sheets of the official server, saves) and the signaled sections; from step 0 on random data (`shipped_defaults_match_old` catches it). Gain ±5%, within noise: off by default (rule C3). Shipped configuration (all except `SKYMEMO` and `LIGHTDEDUP`) compared with the old path, sheets and sections, on consistent data, random and real tables. Buffers kept per thread (`ZONEBUF`): 8 MiB at most, buckets, memo and sections included (`retained_buffers_are_bounded`). Micro-bench `bench_relight` (1 core, loaded machine, best of 7, µs per relight) old → all on: plains 1,150 → 418 (−64%), cave 1,128 → 487 (−57%), building 1,450 → 536 (−63%), redstone with `relight_masked` 1,140 → 133 (−88%). Biggest contributions ("all except it"): `SKYSKIP`, `ZONEUNPACK`, `ZONESENT`, `ZONEONCE`, `ZONEBUF` (2 threads: 0.75 against 1.13 ms in plains). `SKYMEMO` loses alone (+2 to +7%) and in combination (`bench_ab`: −0.5 to −3.5% without it): off by default (`RUSTCRAFT_OPT_SKYMEMO=1`). `LIGHTQDIR` and `RELIGHT_OCCTAB`: ±5% depending on the scene, within noise: on (bit for bit), to settle on the scene bench. "All on" figures above: `LIGHTDEDUP` included. `SKYBIT` only acts through `relight_masked` (callers migrated by M04/M05). `tests_opt` fingerprints (new world, bench world) unchanged. To measure: `scenes.py` redstone, cave, building, plains12, plains32, forest, village, flight, lan (relight ms/s, median/p99 tick, frame p99), then each switch at 0 on the scene where it weighs.

- Multi-core relight (C2-M11, `crates/rc-light/src/fast/par.rs`, 22/10/2026, micro-bench `bench_par`, loaded machine: ±15% differences between two passes of the same path). Bit for bit the sequential path (sheets and sections touched, reserves of 1 and 32 threads, sabotages 8 to 11 caught). Switches, default = new except `LIGHTJOIN`: `RUSTCRAFT_OPT_LIGHTGROUPS` (groups scheduled by levels: a group waits for any earlier group whose written zone crosses its read zone, per kind of sheet; sky zone taken down to the bottom of the world; the groups of a level are computed at the same time then written in order): four scattered placements (`eloignes`) −29 to −40% outside the reserve (light reserve at 2, 4, 8 threads), −32 to −44% in reserve of 2 to 32 threads; a single group (plains, cave, building, water, slab, redstone): same path, no measurable difference. `RUSTCRAFT_OPT_LIGHTPARSOLVE` (zone ≥ 65,536 cells: reading in slices of 16 ranks, sky source heights by rows, propagation by synchronous levels with atomic maximum, tasks of 2,048 entries per level). The first figures (slab −15 to −19%, "threshold 0: +20 to +90%") had been taken with tasks of 64 entries (value of the tests, compiled into the bench): re-measured with the shipped value (the tests now force their 64 entries through `Opts`), outside the default light reserve: 16 × 16 slab (142,500 cells) −13 to −16%, reserve of 4 threads −33%, 16 threads −23%; zones of 43,000 to 53,000 cells at threshold 0 or 32k: plains and torches −18 to −21%, cave and water +12 to +18%, hence split depending on the scene: threshold kept at 65,536 (no trace loses), to settle on the scene bench. `RUSTCRAFT_OPT_LIGHTJOIN` (sky and block of a group at the same time, the block on another thread only if it has light to spread: estimated work ≥ 16,384): OFF by default (`=1` to turn it on): neutral on the micro-bench (±2%, cave +9% at re-measurement), and the synthetic trace of one group (where it alone acts) is slower on every pass turned on; the block weighs 40 µs against 280 for the sky. With `RUSTCRAFT_OPT_ZONEONCE=0`, `LIGHTJOIN` and `LIGHTGROUPS` keep the two zone reads (block then sky): the switch says what it measures. Light's own reserve (`RUSTCRAFT_LIGHT_THREADS`, default cores / 8, at least 2): the server thread or the main thread does not wait behind the meshing of the common reserve (low priority); with 32 threads it lost everywhere. Phases of a sky resolution (sequential, µs per group): plains 42,875 cells read 104, sources + seeds 125, propagation 125, write 19; slab 142,500 cells 357 / 394 / 256 / 78: write not parallelized (7%, gain bounded). Server background thread ruled out (spawns and plants read the light in the tick); GPU not applicable (zones of 40,000 to 150,000 cells, round trip longer than the resolution). Order of the `LightUpdate`s (hashed set, already non-deterministic): reported to C2-M04 (`play.rs`). Replay of the server's real traces (`benches/relight.rs -- --test`, `RUSTCRAFT_OPT_REQUIRE=1`, outputs = frozen copy and known fingerprints; one replay per pass, order off / on / on / off, ms per relight): surface (409 relights, 1,728 resolutions of each sheet: water makes several groups) 2.63 / 1.86 / 1.46 / 2.71 (−30 to −45%), `LIGHTGROUPS=0` alone 2.53: the gain comes from the groups; Nether (block alone) 3.54 / 2.79 / 2.35 / 3.52 (−21 to −33%), `LIGHTGROUPS=0` 3.60; synthetic (one group per relight) 0.72 / 0.96 / 0.79 / 0.72, within the noise of pass 2 (reference also slower). To measure: `scenes.py` redstone, cave, building, plains12, plains32, flight, lan, each switch at 0 on the scene where it weighs.

- Mesh biome tints (C2-M06, `crates/rc-mesh/src/blocks.rs` `Tint`, 23/10/2026, loaded machine). New reason compared with the lines "blend not a hot spot": the biome blur zoom (8 corners × 8 LCG steps per block, 25 blocks per tint at radius 2) has been added since. Context per mesh, buffers kept per thread (≤ 190 KiB: grid 72, sums 93, memos 20), zeroed per section, explicit fallback to the old computation outside the section and its one-block border; `mesh_reference` keeps the old computation. Switches (default = on, `=0` old path, read once, set in `MeshJob::tint`): `RUSTCRAFT_OPT_TINTUNI` (all the quarters reachable by the zoom for a single-biome section, except swamp grass: palette color without zoom or average), `RUSTCRAFT_OPT_TINTLATTICE` (corner offsets memoized per lattice point, same f64 operations in the same order), `RUSTCRAFT_OPT_TINTGRID` (zoomed biome per (layer, z, x) of the window, swamp grass per (x, z)), `RUSTCRAFT_OPT_TINTIDX` (neighbouring column by cell [dz + 1][dx + 1], also for reading the neighbouring states; biomes per quarter kept for the section), `RUSTCRAFT_OPT_TINTBOX` (separable sliding sums of a whole layer from its 32nd request; a number sets the threshold). Proofs: `digest.rs` 13 combinations × 5 variants = stored fingerprints, sample of 20 columns × radii 0 to 7 against the direct computation; unit tests (mixed biomes, absent column, neighbouring swamp, one biome, swamp everywhere; shuffled request order); sabotages shifted grid, y not folded, shifted sum window, swamp in the single zone, lattice memo not zeroed, all seen. Interleaved micro-bench `tests/tint_bench.rs` (full meshing, best of 7, 1 thread on 312 columns / all threads on 1,248), old → all: radius 2: 623 → 491 ms (−21%) and 145 → 116 ms (−20%); radius 7: 2,226 → 684 ms (−69%) and 368 → 136 ms (−63%). Alone (radius 2 / 7, 1 thread): grid −12 / −54%, single zone −10 / −46%, sums +13 / −44%, lattice +1 / −16%, direct access noise / −25%. Sliding sums: threshold 1 and 8 lose at radius 2 (+24 / +20% on 1 thread), 32 and 128 neutral, all win at radius 7: threshold 32 kept. Lattice alone loses on all threads at radius 7 (+31%, high noise: median +69%) but combined it is neutral: all kept on, to settle on the scene bench (C6). f64x8 variant of the zoom not done (the nearest corner depends on strict comparisons in order; expected gain small once the grid is in place). GPU evaluated and refused: the tint is read by the CPU mesher while building the faces, a round trip per section would cost more than the computation (full meshing ≈ 1.6 ms per column on 1 thread at radius 2, tints included). Memory: +190 KiB per meshing thread at most. To measure: `scenes.py` cave, forest, plains32, ocean, flight (pool ms/s, meshed/s), 2 alternating passes, each switch at 0.
- Incremental relight (C2-M14, `crates/rc-light/src/fast/inc.rs`, 23/10/2026, loaded machine): decrease then increase from the changed cells alone, in the manner of the game's engine (read decompiled: decrease and increase queues, neighbour emptied if darker than the emptied cell, brighter neighbour sent back toward it, emitters picked up, sky: 15s stored removed or placed along the stack). Bounds and margins = zone of the old path; a group whose zone crosses that of another group of the same call goes through the zone. Bit for bit the old path on consistent worlds (10,000 random steps, sheets and sections; limited sky `relight_masked`; emitter scene; sabotages 20 to 23 caught). NOT bit for bit on inconsistent sheets: the zone repairs its whole box, the incremental one (like the game) only touches what depends on the changes. Real traces, at equal input: synthetic (official server light) 20 relights out of 400 different, Nether 2 out of 100, surface (our server, sheets filled before the relight) 110 out of 409 (89,951 cells). Hence `RUSTCRAFT_OPT_LIGHTINC` OFF by default (`=1` turns it on), like `LIGHTDEDUP`, pending the user's decision (A3: closer to the game, which does not repair either, but changes the saved sheets). Consistent worlds with the game's real tables (the whole registry, one draw in four among the real emitters, 7,500 steps, scene of real emitters; sabotages 20, 21, 23 seen): bit for bit too, hence no difference proper to the real shapes, opacities or emissions hides in those of the traces. ABBA replay (`benches/relight.rs`, time of a whole trace, criterion median, single calling thread, light reserve for `LIGHTGROUPS`) zone / incremental / `LIGHTINCMAX=16`. First version (`target\c2m14-bench1.txt`): as soon as a group was isolated, all the groups of the call were resolved in series, zone groups included, and the parallel groups no longer served: Nether (many groups) 508 and 353 / 582 and 516 / 525 and 426 ms, incremental SLOWER on both passes (+15 and +46%, disjoint intervals at pass b: 317–400 against 470–567 ms), structural cause and not noise; synthetic 356 and 795 / 31 and 48 / 23 and 32 ms; surface 1,132 and 799 / 746 and 622 / 672 and 1,178 ms (pass b of 16: 867 to 1,523 ms); 16 then won on synthetic (−26 and −34%) and Nether (−10 and −17%), surface split. Fixed: the isolated sheets are resolved incrementally and removed, the rest follows the normal path (`LIGHTGROUPS` included); sky zone computed once and only for the groups that their block zone does not already exclude. Re-measured (`target\c2m14-bench2.txt`, loaded machine): synthetic 351 and 426 / 32 and 31 / 36 and 42 ms (÷ 11 to 14); surface 758 and 893 / 495 and 411 / 444 and 433 ms (−35 to −54%); Nether 248 and 302 / 311 and 266 / 270 and 265 ms (within the zone's own difference between its two passes, 248 to 302). Threshold `LIGHTINCMAX`: after the fix, 16 loses on synthetic (+13 and +37%), equal on Nether, split on surface; to settle on the quiet bench, no limit by default until then. GPU not applicable (a few hundred cells). RAM: per-thread buffers ≈ 10 B per zone cell (4.7 MiB for a full-height sky zone), bounded by `RETAIN_BYTES`. To measure: `scenes.py` redstone, building, cave, plains12 with `LIGHTINC=1` (tick p99), then settle the default.
- Smooth lighting and mesh occlusion (C2-M07, `crates/rc-mesh/src/light.rs`, 24/10/2026, loaded machine). Paths set in `MeshJob::light` (`LightPaths`, read once; default = D+A+B, best proven combination; C, E, F, G off until the quiet bench; `=1` turns on, `=0` cuts): A `RUSTCRAFT_OPT_AOCELL` (one cell read returns light and properties; block light read only if it is used), B `RUSTCRAFT_OPT_AOCORNERS` (corners of a face memoized per (direction, stuck) during the block: grass base + layer, stairs, slabs), C `RUSTCRAFT_OPT_AOFACETAB` (per baked face: plane, at the edge, full, carrying corner, weights f(i)·f(j) same f32 products; "stuck" stays dynamic through the cubic collision; weights shared bit for bit: 1.13 MB for all the faces, 9.96 MB without the sharing; built only if C is on: 80,011 faces in 9.0 ms at resource loading, best of 5, loaded machine), D `RUSTCRAFT_OPT_AOSCRATCH` (cells kept by the thread in `Scratch`, generation continued from one task to the next, allocation at first need: −128 KiB allocated per task), E `RUSTCRAFT_OPT_AOCELL8` (6-byte cell: u16 generation, absent light = 0xffff, state not kept: 48 KiB instead of 128), F `RUSTCRAFT_OPT_AOFLUID` (fluids read through the section's cache; flat path through the section's resolved layers with the state of the meshed block, never the cached light), G `RUSTCRAFT_OPT_AONEIGH` (lazy 3×3×3 neighbourhood, OFF by default). H (per-row masks) not done, GPU ruled out (arbitrary models, the game's truncations, read while building the faces). Proofs: `tests/light_paths.rs` (17 paths × default / without occlusion / without sky over the whole capture, against the old path and the stored fingerprint; built scene grass, stairs, slabs, leaves, water, magma, glowstone, lantern, torch; two tasks on a new thread and generation passing through its maximum, 16 B and 6 B); sabotages seen: generation reset to 1 per task, permuted weights, corner key without "stuck", flat path through the cached light, no flush at the maximum, sentinel not reset. Interleaved micro-bench `tests/light_bench.rs` (best of 7; 1 thread on 312 columns very noisy: medians +40% over the best), two passes, occlusion active, all threads on 1,248 columns: old 123.2 / 123.0 ms; D −2.6 / −6.1%; D+A −6.9 / −12.9%; D+A+B −16.7 / −15.1%; D+A+B+C −11.9 / −12.0%; +E −16.6 / −8.2%; +F −16.6 / −9.9%; +G −14.8 / −11.2%; default −17.8 / −11.6%. 1 thread (best): D+A+B −12.7 / −12.0%, old default (all except G) +5.4 / +14.0%, slower than the old path on both passes: default brought back to D+A+B (B3, C5). Occlusion off, all threads: old 63.9 ms, default −3.9%, everything under the noise. Alone, C, E, F, G cannot be distinguished from the noise (from −5 to +10%). Nothing thrown away: everything stays behind its switch until the quiet bench (C6). To measure: `scenes.py` cave, forest, plains32, flight (pool ms/s, meshed/s), 2 alternating passes, each switch at 0; settle C, E, F, G (`RUSTCRAFT_OPT_AOFACETAB=1`, `AOCELL8=1`, `AOFLUID=1`, `AONEIGH=1` against the default), 1 thread included.

- Name tags, glyphs kept (C3-M23, `apps/client/src/nametag.rs`, 24/10/2026, loaded machine). `RUSTCRAFT_OPT_TAGPERSIST` (default 1, `=0` old path): glyphs of each shaping kept in a reserve (storage buffer, 48 B per glyph) sent back only for the new shapings (tail of the reserve); per frame one 24 B reference per tag and per pass (anchor, light, first glyph, count), drawn as instances of 6 × (longest name of the draw) vertices, glyphs beyond the count degenerate; each pass is cut into contiguous draws (order kept) closed before their vertices exceed 2 × those of their glyphs, otherwise a single long name made every short tag pay (200 "CowN" + a 120-character name: 289,200 vertices computed in a single draw, 21,888 measured cut, old path 20,424; 6 draws per frame instead of 2); improved transparency: copy sorted solids first only if the original order is not already; forgotten glyphs counted, reserve compacted at the start of the frame when they exceed max(live, 4,096), hence reserve ≤ live + max(live, 4,096) glyphs, GPU buffer (power of 2) never reduced; axes, reserve and references (a single write instead of two) through the common staging. Proofs: `nametag::tests` (300 frames, 200 entities, names changed, sneaking, obfuscated, teams, background, transparency toggled: glyphs drawn per pass identical, re-read in a double of the GPU buffer; GPU captures of the normal and see-through passes, and of the full improved-transparency chain (opaques with the solids, three steps, pruning, composition, then see-through), byte-identical; omitted upload seen by both; vertices drawn ≤ 2 × those of the glyphs checked at every frame of the script, which contains an 88-character name, and exceeded (> 4 ×) without cutting). Micro-bench `nametag::tests::bench` (200 kept "CowN", 1 core, median of 1,000 frames): CPU 5.6 → 2.9 µs per frame (re-measured after cutting: 6.1 → 3.5 µs, loaded machine), bytes sent 190,832 → 9,632 per frame (÷ 19.8); GPU: vertices drawn 19,080 → 20,400 (homogeneous names, + 7%), at most 2 × the useful vertices by construction, not timed. Multi-core not applicable (2.9 µs). To measure: `scenes.py` name-tags (200 named cows), crowd: CPU ms/frame, GPU, bytes/frame, with `TAGPERSIST=0` as the reference.
- Missing GPU comparisons (user reminder of 11/10/2026: CPU 1 core / all cores / GPU for EVERY building block): textured meshing ("hidden" masks and section communication in a compute shader), per-section field-of-view test, section communication, animations (already GPU: measure the CPU variant by copying), physics block effects (tiny: note the reason if the GPU makes no sense). Visibility walk: no sensible GPU variant (each step depends on the queue order and the sections already reached).
- Pack stacking (overlays, filter, `.mcmeta`, fonts and block states read in each pack):
  hot path = resource loading only (cache snapshot afterwards). Bench of the examples
  `bake`/`font` on the jar alone, 4 alternating passes, loaded machine: old baking 580–702 ms,
  new 360–718 ms; `default` font old 67–109 ms, new 37–58 ms (noise dominant, no visible
  regression); baked faces identical (80,011). Without a filtering pack, `stack_list` keeps
  the old path (no sorted set); without an overlay, no extra read. To re-measure on a quiet
  machine: cold startup with a pack with overlays and filter, typing in the chat field.
  Follow-up (22/10): all the providers of a font are loaded before the options filter (like
  the game; `unifont_jp` also read with the Japanese option disabled), files validated in full,
  masked layers discarded. `default` font (asset index): old 27–34 ms, new 28–34 ms, 4 alternating
  passes, loaded machine, identical glyphs (114,432, advances compared). Cold path only (the
  cache keeps the fonts). Pack filter: `fancy-regex` (Java syntax) instead of `regex`; simple
  patterns go through the same automata engine, not measured on the bench (no filtering pack
  in the scenes). Particles per state (table `state_particles`, ≈ 30,000 × 4 B): baking 135 ms,
  identical faces.
- Section communication computation: +3 µs per section (0.457 → 0.544 s single-threaded on the bench) to optimize (filling by bit rows).

- GPU: meshing of full sections, culling, bulk lighting, terrain generation (singleplayer).
- Socket reading by 1 MiB blocks: in place; gain to measure online (offline is not quantified).
- `data.bin` strings in a single arena: gain bounded by the current 0.37 ms (mimalloc), to do with the next format rewrite.
- G4-F03 (goal AI and sensors, 22/10): no hot path touched, decision without a bench. Common item
  sensor (villager, allay, piglins): a single pass over the ground items, index sort
  instead of copying each wanted stack (one allocation fewer per item); partner search:
  non-combat conditions only after the cheap tests (species, love, panic); universal
  anger: one walk of the mob list per trigger (rare). Scene measurements to
  do on a quiet machine: populated village (villagers, cats, golems), breeding (cows and wolves in love),
  bastion (piglins); integrated server CPU expected identical within ± noise. Follow-up: the last hit
  received by the player is noted before the armor (one string copy per hit that lands, already paid
  before, nothing new); air and water positions in a single version (happy ghast and bee),
  one copy fewer, same number of draws.
- G2-F04 (dynamic item properties, 23/10): item model evaluation, bench
  `bench_evaluation` (26.3 jar, 6,632 evaluations per pass, 1 thread, loaded machine): old path
  32.3–32.4 ns / 0.101 alloc, new 31.4–31.8 ns / 0.033 alloc per evaluation; `layers()`
  flag 0 30.1–30.3 ns, flag 1 33.5–33.7 ns. Local time: text kept in the node and
  redone at most once per second (like the game), compared without copy (before: local date, three
  `format!` and three `replace` at every evaluation of a chest). Crossbow charge: id of the
  rocket read once (no registry lookup per evaluation). Chosen item of the bundle: content re-read
  only for the hovered slot that has a choice. Cast fishing rod: one walk of the
  entities per frame (local player's bobber), fishing players noted in the bobber pass
  already done. No lead cut; scenes to measure on a quiet machine: open inventory
  (bundles, chests), fishing in first and third person.
- G2-F03 (humanoid models, held items, carriers outside the HUD, 24/10): no new loop.
  Item carrier outside the HUD: environment on the stack (references; the frame's world is
  copied once per frame into the engine, 2-3 short strings); only the entity type (property
  not used by the base game) allocates. Hidden arms: parent chain recomputed only
  for a hidden part (rare: armor stand without arms). Piglins: pose read once per frame
  and per piglin (one tag read under a read lock if the offhand holds an
  item). Invisible illusioner: four body passes like the game (official cost), state of the
  copies in the entity (no table). Outline-only layers: instances moved, nothing
  copied. Nothing to parallelize or port to the GPU (a few matrices per entity). No
  switch (no performance lead); scenes to measure on a quiet machine: `crowd` (armed
  mobs), village (copper golems), bastion (piglins), raid (invisible illusioner).
  Follow-up (24/10): seed per carrier (id + context rank: addition and lookup
  in 10 names per held stack), item displays and projectiles carrying their stack (same
  environment on the stack), bob cut under freeze (a boolean): nothing measurable,
  no switch; scenes added: item displays (server map), Nether (compasses).
- G3-F03 (integrated server loop, data packs, 23/10): no hot tick path
  touched (held slot: one comparison; pause: one statistic per tick; save: computation of the
  interval once per save or rate change). World opening: the pack stack
  (`packs.rs`) costs nothing with the base pack alone (immediate exit, one read of the
  `datapacks` folder, which used to be read three times: functions, tags, advancements); with
  packs, the jar's files are taken without copy when the base pack is at the bottom of the stack
  (common case), only the tags of the added packs are rewritten. Tag resolution
  rewritten (refusal of missing required entries, like the game): bench
  `registries::tag_bench` (300 block tags of the jar, median of 9, loaded machine) 1.56 ms
  before, 1.59 ms after = noise; identical output verified by the bench and by
  `registries_match_capture`. Leads not attempted (gain bounded to ~2 ms per opening): resolution of the
  tag registries in parallel. Scene to measure on a quiet machine: singleplayer opening (time
  "singleplayer: opened in … ms" of the log), world without a pack then with trade_rebalance.
  Follow-up after review (23/10, loaded machine, medians of 9, 2 passes): the resolution now follows
  the game's dependency order (reference graph, cycles discarded, construction
  in order). Block tags alone 0.60 → 0.87 ms (graph denser in references);
  all the registry tags at opening 3.82 → 3.74 ms (the old path re-read the
  JSON of each cited tag); identical output on the jar (bench
  `opening_tag_passes_on_the_jar`). Target without dependency: cycle test skipped, and members of the
  cited tags read without copy; after/before ratio 1.41 without, 1.45 with (noise, two
  different passes), kept because it has no effect on the result. Walks added in vanilla
  at opening and at every `/reload`: tags of the other registries for the parser 0.28 ms,
  jar functions 0.11 ms. Leads not attempted (gain bounded to < 1 ms per opening): registries in
  parallel, memo of the cycle searches.
- Displayed frames per second and latency: to measure in front of the PC (the Remote Desktop session caps the display at 32 fps whereas the GPU computes a frame in 8.3 ms).

## Full client (milestone 1, wireframe)

Local 26.3 server, view distance 16 (1,089 columns), RTX 5080 Vulkan, 30-thread pool,
Remote Desktop session (presentation through RDP may cap the fps).

| Measurement | Value |
|---|---|
| Startup → entering the game (data in cache) | 363 ms |
| Startup → 1,057 columns received and meshed | ≈ 3 s (pace imposed by the server, official formula) |
| Meshing queue | permanently empty (the meshing follows the reception) |
| Exposed faces drawn | 24.98 M (caves included, no visibility sort) |
| Frames per second in wireframe | 32 (cap of the Remote Desktop session) |
| GPU time per frame (GPU timestamp, independent of the display) | 8.3 ms for 10.6 M faces in lines |

To do at milestone 2: full rendering, GPU visibility sort (frustum + occlusion), cave
visibility graph; re-measure outside RDP.

## Local network: fluid count of the sections (G3-F05, 24/10, loaded machine)

Each section sent to a guest now announces its number of fluid states (like the game).
Counted through the palette histogram (`Container::count_where`), with an exact exit when
no palette entry (nor the value 0 of the out-of-palette indices) is a fluid: the case of most
sections. Bench `rc-world/benches/container.rs` (criterion, loaded machine):

| Section (distinct states) | unfolding + filter | histogram | without fluid (fast exit) |
|---|---|---|---|
| 12 | 3.08 µs | 2.10 µs | 15.7 ns |
| 50 | 2.72 µs | 1.86 µs | 27.7 ns |
| 5,000 (direct, no palette) | 1.48 µs | 1.53 µs | not applicable (no palette) |

Decision: histogram + fast exit kept (≈ 100 times less per section without fluid, i.e.
< 1 µs per 24-section column instead of ≈ 50 µs). Identical result (test
`count_where_matches_unpacked_count`, unanswered predicate added). Leads not attempted (gain
bounded by the fast exit): count kept up to date at every block placement, per-state table in
bits. Multi-core and GPU not applicable (≈ 1 µs per column, on the server thread). To re-measure on a
quiet machine.

Player-to-player tracking (same mission): at most 8 × 7 pairs per tick, a distance test
and a lookup in a chunk set per pair; negligible, not measured separately.
Tracking range read by the passengers: immediate exit without a passenger (no allocation).
## G1-F15: singleplayer loading screen, symbolic links (24/10, loaded machine)

- Check of a world's links at opening (`rc_data::symlinks`, walk without following links,
  entry type read in the folder listing, no extra call per file): 3.65 ms for
  12,000 files (bench `bench_check_world`, best of 5). Multi-core not retained: nothing to gain
  under a millisecond per thousand files; to re-measure on a quiet machine on a real big world.
- Loading screen map (integrated server): Chebyshev distance in 2 passes over
  57 × 57 cells, redone only when the number of loaded or expected chunks changes, and
  only until the player enters (600 ticks at most). Client: one strip per run of cells
  of the same color on a line instead of 1,225 rectangles. Gains not quantified (loading
  scene to measure, quiet machine).
- New world (integration test `new_world_preparation_is_reported`): progress at 100% in
  1.2 s, view 2.
- Remaining fidelity difference: the initial chunks step is always empty. The game counts there the
  full chunks woken by the saved tickets (`minecraft:forced`, `minecraft:portal`
  in `chunk_tickets.dat`, all dimensions), so its bar starts at 10/(59 + N); ours starts
  at 10/59. Cause: our server does not load the forced chunks at startup (only at
  `/forceload add`); to handle with the residency of forced chunks.
- Deleting a world whose folder is itself a symbolic link: only the link is removed,
  without a walk (like the game); zero extra cost, one metadata read.

## G5-F03 — synchronized mob attributes (24/10, loaded machine)

- Re-sending the synchronizable attributes on change: fingerprint (base + modifiers of the 12
  modeled attributes, attributes kept) compared every tick for each tracked mob, without
  allocation. Bench `bench_attribute_fingerprint`: lookup by name 41.8 ns → direct fields
  15.3 ns per mob and per tick (kept; 1,000 tracked mobs ≈ 15 µs per tick).
- Review (fidelity): comparison made only at the tracking's send tick (1 in 3, 1 in 2 for
  the allay, or modified data), one fingerprint per attribute to send only the changed ones:
  16.4 ns per call (loaded machine), i.e. ≈ 5.5 ns per mob and per tick on average.
- Ruled out without measurement: "modified" flag set at every attribute write (dozens of
  sites write `base` directly: risk of omission, hence of a wrong client). To re-measure if the
  profile shows it.
- The rest of the batch (tag, egg, dyes, wolf armor, conversion, buckets, spawn
  point) is on rare events: off the hot path. Mixed color recipe: one
  recipe lookup per dyed birth.
## G6-F12: block entity tickers, conduit, containers (24/10, loaded machine)

- Common block entity tick test (border then tick range, `be_ticking`): 21.6 ns
  per call against 21.1 ns for the previous tick range alone (bench
  `bench_be_ticking_and_conduit_frame`, 10^6 calls, best of 5). The border test goes
  first (4 bounds, no table); furnaces, brewing stands, spawners and gateway cooldowns
  now also pay the tick range (21 ns per block entity and per tick), negligible. No other
  lead: nothing to gain under thirty nanoseconds (a cache per chunk and per tick
  would cost its construction at every tick for a few furnaces).
- Conduit: frame examination (27 water states + 42 tags, binary search) 0.77 µs,
  once every 40 ticks, i.e. ~19 ns per tick and per conduit. Target search: linear
  walk of the mobs once every 40 ticks and only for a complete conduit without a
  target; spatial index not retained (nothing measurable at this rate). Wet player: one sweep
  of the fluids of its box per examination. Multi-core and GPU not applicable (sub-microsecond work).
- Opening of the shulker box: one collision search on a half block per click.
## G4-F12: iron and copper golems (25/10, loaded machine)

- Village defence (start test, goal-selection tick): the line of sight the game requires costs
  rays; the villager scan is now skipped when the player is not a valid target (no observable
  effect: the sight cache only lives for one tick), and the reputation is read before the
  villager's sight ray. Bench `bench_defend_village_can_use` (8 villagers, 10^5 calls, 2
  passes): player out of range 12-13 ns per call against 1,934-2,199 ns without the skip;
  player nearby in the open (neutral reputation) 316-331 ns against 2,068-2,617 ns. Kept,
  without a switch (dead work removed, identical result by construction). Multi-core and GPU
  not applicable (one evaluation per golem and per goal tick).
- Door behaviour (villager, copper golem): a single code path; no more path copy at every
  call (one allocation fewer per walking villager and per tick). Copper golem: list of nearby
  living entities filled into the buffer of the previous measurement (no more allocation
  every 20 ticks for the seen living entities). Cost of a non-isolated call (below a
  microsecond).
- Shared spawn helper: one collision-box buffer for the whole descent instead of a `Vec` per
  cell; border test (4 bounds) before the descent, which spares the descent outside the
  border. Rare (spawning by attempts), not measured separately.
## G2-F08: singleplayer world, world pack, creation, list (24/10, loaded machine)

- "..." cut of the names in the list and of the packs (`rc_text::clamp`, cumulative width in a
  single pass instead of re-measuring the prefix at every character): 64 characters cut at 157,
  1,074 then 1,181 ns per call for the old method, 596 then 651 ns for the new one (bench
  `clamp_bench`, 2 passes, best of 5 × 200,000 calls): about 1.8 times faster, kept.
  Identical output (bench assertion, test `fit_matches_prefix_widths_and_clamp_adds_dots`).
  Multi-core and GPU not applicable (less than a microsecond per line, at drawing time).
- Backup question at opening (`rc_sim::packs::backup_question`: two gzip .dat, world packs
  walked): moved from the main thread to a thread, under the "Loading resources" screen that the game
  displays at that moment; the main thread no longer blocks. Worst case (pack
  in a folder of tens of thousands of files) not measured: to measure on a quiet machine.
- Check of a world's links: done as soon as "Play" (like the game, before any read), and
  redone at the actual opening as a fallback (the folder may change in the meantime); second
  walk of the order of 3.65 ms for 12,000 files (G1-F15 measurement), kept for safety.
- Free disk space (`rc_sim::storage::low_disk_space`): one system call at opening, on
  the main thread; not measured separately (negligible).
## G4-F16b: local monster effects (25/10, loaded machine)

- `entity_fx::mob_fx` (blaze, breeze, enderman, endermite, evoker, illusioner, phantom,
  ravager, warden): a few comparisons and draws per affected monster and per tick, on the
  main thread, inside the entity loop already walked (no second walk); no allocation
  except for a sound (empty vector otherwise). Rough measurement: 900,000 entity ticks +
  effects (test `ravager_stun_particles`) in under 0.1 s, i.e. < 110 ns per call, tick
  included. Leads not retained: grouping the draws by batch or with SIMD (a few monsters at a
  time, nothing measurable), multi-core and GPU not applicable (sub-microsecond work, output
  to the particle system of the main thread). Render shape of the ground block: comparison of
  the block name (12 cases) only for the breeze and the digging warden; per-state table not
  retained (construction at loading for a rare use). Scenes to re-measure on a calm machine:
  Nether (blazes), night (phantoms), ancient city (warden), trial chamber (breezes).
## G4-F16a: dragon client effects, creaking tearing itself down (25/10, loaded machine)

- Dragon (`entity_render/dragon.rs` `client_fx`, `App::tick_dragon_fx`): one pass over the
  entities per tick with a kind comparison (of the order of a nanosecond per entity), then
  a handful of operations for each dragon; no allocation (effects delivered by callback,
  sine and cosine from the game's table, already built). The only notable cost: the podium
  column re-read block by block (at most the world height, 384 reads) at every landing
  tick, a few seconds per fight; a cache of the "no leaves" height map would cost
  an update at every block change of the whole world for this single phase: not
  retained. Multi-core and GPU not applicable (a few microseconds per tick at most, effects emitted
  in the order of the random draws). Not measured on the bench: nothing on the hot path outside the dragon fight.
- Creaking: eye blinking in O(1) per tick (two floats of the free state), red overlay
  and tilt by a kind test; no measurable cost.
- Breath aim (revised): the dragon's head packet is discarded on reception (one kind comparison
  per head packet of a living entity, a few nanoseconds; filtering at the tick would cost a
  test per entity and per tick: rejected without measuring), the aim is read from the previous
  tick's head, already kept (nothing added per tick; the write at spawn from the previous
  version disappears). Integrated server: horizontal-movement test of the dragon (two
  subtractions, one comparison per dragon tick). Off the hot path: nothing to measure on the bench.

## G5-F02f: food finish sound, ravager trampling crops (25/10, loaded machine)

- Crop trampling by the ravager, one pass per ravager per tick (box shrunk by 1e-5, 3 x 4 x 3
  blocks). Bench `bench_ravager_trample_pass` (rc-sim, `--ignored`), 2 alternating passes, box
  without crops (common case): plain pass (one chunk looked up per block) 715 then 522 ns;
  per-column pre-scan (one chunk per column, exit as soon as there is no crop) 195 then 207 ns:
  about 3 times faster, kept by default (`RUSTCRAFT_OPT_RAVAGER_SCAN=0`: plain pass only, to
  re-measure on a quiet machine). Full field (placing + breaking 9 crops): 35 to 54 µs,
  dominated by the break (drops, updates), pre-scan difference within the noise.
  Identical result (the break keeps the game's x, y, z order; tests under both settings).
  Multi-core and GPU not applicable (under a microsecond, once per ravager per tick).
- Food finish sound: one more sound packet for the final bite, two random draws; the sound is
  looked up by name in the registry like other sounds (once per meal).

## G6-F09a: speleothems and active sulfur (25/10, loaded machine)

- Random tick of a speleothem with no possible growth (bench `bench_speleothem_random_tick_and_geyser`,
  best of 5 × 200,000): sulfur spike 15.3 ns, pointed dripstone 15.6 ns (two draws), against 21.7 ns
  for a stone (fall back to the copper tick): the chance test (0.0114) comes before any state
  read. Properties read by name (`vertical_direction`, `thickness`) only on this rare path
  and at shape time; per-state byte table not retained (nothing measurable). Feeder blocks
  (speleothem block, sulfur) resolved once in `Ids`. Multi-core and GPU not applicable.
- Continuous geyser (block entity, every tick): 106.5 ns with no mob, 1,972 ns with 200
  mobs outside the column (about 9 ns per mob: walk of the list, exact `Mob::touches`
  prefilter reused from the pushes). Lead noted, not done: spatial index of the mobs
  (none today) to read only the mobs in the column; to re-measure on a quiet machine on
  a real sulfur cave (number of active geysers unknown). Nausea and countdown:
  once every 10 and 20 ticks.
## G6-F09b: projectiles on plates, right to break on impact (25/10, loaded machine)

- Pass over the blocks traversed by a projectile: the block kind (table lookup per state) is
  now read for every cell and every projectile (before: arrows only) to route the plates;
  1 to 8 cells per step, no allocation; the update of the list copy and the recount of the
  entities only happen in a released plate (rare). Not measured separately: below the scene
  noise; to confirm on the quiet-machine bench (scene with arrows in flight). Lot 6
  integration: the projectile cells now come from the G5-F02e contact walk (section below),
  and the plate reads the box of the projectile being ticked (`ticking_proj`) instead of
  moving its list copy.
- Right to break on impact: the tag predicate is frozen as a `match` (no tag lookup at run
  time, test against the jar), the rule is read only when the block hit is a pot, a chorus
  flower or a speleothem. Multi-core and GPU not applicable (one-off event).

## G4-F07a: simulated lead and the wandering trader's llamas (25/10, loaded machine)

- Lead state boxed on the mob (`Option<Box<Leash>>`, 8 bytes per mob): a mob without a lead
  pays one test per tick; no global structure to keep up to date.
- Tick of a lead (bench `bench_leash_tick`, ignored by default, best of 5 × 2,000): taut
  (spring) 64 to 66 ns, slack near the holder 63 ns (2 passes). Holder found through the
  id index, knot through the decoration registry; reload by UUID through the UUID index.
  Multi-core and GPU not applicable (a few held animals, sub-microsecond work). The
  "animals held by X" sweeps (32-block cube) only serve clicks, snaps and knot removals:
  linear walk kept (nothing measurable at this rate).
- Fence knot: ordinary decoration filtered at write time (never saved, like the game);
  no separate list (selectors, `/kill` and `/data` see it with no extra code).
- Follow-up (lead zone read by the goals): home test added to the attack tests of the
  targeting goals, to the chase without sight, to the "go to a block" searches and to the
  bee goals. Without a home, it costs four `Option` tests (short-circuit before any
  position); no structure added. Lead tick unchanged: 64 ns taut and slack (bench above,
  loaded machine). Generic home in the field already present on every mob (no extra byte).

## G4-F07b: pig steered with a carrot, llama spit (25/10, loaded machine)

- Pig: one "carrot in hand?" test per tick and per pig (two inventory slots read), the boost
  and the wear only on click. No hot spot (no loop, no allocation); multi-core and GPU not
  applicable. Dismount: at most eight cells, once per dismount.
- Llama spit: same work per tick as before (one walk of the mobs); the margin and the entry
  point replace the distance to the centre (same cost); the "llama and passengers" list is
  built only until the spit has left (a few ticks), then no allocation at all. No bench: a
  few spits in flight at most, nothing measurable per scene.
- Rework (review): dismount chosen by species (horse family: sides; other animals, nautiluses
  included: neighbouring cells; the rest: top of the box), cost unchanged (one species test per
  dismount). Projectiles, spits, fangs and bolts born during a mob's tick: one boolean per
  entity, read and reset on the next tick (no allocation; one extra walk only while the game is
  frozen, to reset the flags).
- Second rework: the boolean becomes a birth mark (`mobs::Walk`, number of the entity walk,
  8 bytes per spit, fang, bolt, projectile and cloud) compared with the walk in progress; one
  integer comparison per entity and per tick, like reading the boolean. The two reset walks of
  the frozen game disappear (less work while frozen), no walk added, no allocation. Nothing
  measurable per scene: even with hundreds of arrows stuck in blocks, the work per arrow and
  per tick stays the read of one field, already done with the boolean; no bench, multi-core and
  GPU not applicable.

## G5-F16b: event 55, recipe book slot displays, "Biome builder" line (25/10, loaded machine)

- LAN broadcast of players' equipment: only the changed slots are sent (instead of all 6 at
  each change) and two hands exchanged go out as a 5-byte entity event instead of two stacks:
  fewer bytes on the network, same CPU work (the comparison of the 6 slots was already done).
  Multi-core and GPU not applicable (6 comparisons per player per tick).
- Recipe book, fuels: a scan of the item table (≈ 1,500 entries) then a sort of 347
  (group, identifier) pairs for each furnace ghost recipe, instead of 1,500 name suffix tests:
  not measured separately (rare event, of the order of ten µs). Value group precomputed when
  the data is prepared (≈ 30 values, linear search over 347 items, once). Per-recipe cache
  ruled out: nothing to gain on a click event.
- Dyeing demonstration: 16 stacks at most, one 4-byte allocation each; drawn with the variants
  menu as before (same number of resolutions).
- F3 "Biome builder" line: computed by the integrated server only when the entry is shown AND
  the feet block changes (6 density functions, /locate generator cache). First display:
  construction of the generator shared with /locate, 222 to 233 ms on the server thread (test,
  loaded machine), once per dimension; to re-measure on a quiet machine. Lead to measure:
  build this generator off the server thread.

## G6-F07: bone meal (25/10, loaded machine)

- New targets (`blocks/bone_meal.rs`): one player click or one dispenser shot, no hot path.
  Named tags read by `TagId` (3 added: corals, nylium, wall fans), no list allocated (tag
  members read in place for the draws). Bone meal in water: 128 attempts at most, like the
  game; multi-core and GPU not applicable (single piece of work, ordered by the world's random
  draws).
- Features placed by name loaded in every decorator (29 features + `grass_bonemeal`): decorator
  construction of a flat world 2.86 ms without, 2.88 ms with (best of 5, one pass each): under
  the noise, kept (without them, saplings and bone meal failed outside the biomes that cite
  them).
- Placing a feature in the live world (sapling tree, bone meal, `place feature`), 9 chunks of
  packed sections (bench `bench_g6f07_deco_copy_and_compare`, `--ignored`, best of 30 alternated
  rounds, a client building alongside): copy of the world into the region 1.34 ms cell by cell,
  0.11 ms by unpacking one pass per section (`unpack_into`); region / world comparison 3.90 ms
  by reading every cell of the world, 0.12 ms section against section (an identical section
  skipped by one slice comparison). A complete oak placement 3.30 ms after (about 8.3 ms before,
  by difference). Kept: same result (copied states identical, same cells written apart from the
  planted fungus; tests `w8b` and G6-F07). The comparison made before the replay of the breaks
  is also the fidelity fix (cells not touched by the feature are left to the world). The rest
  of the placement is not broken down (heightmaps of the 9 copies, biome conversion, the
  feature itself): a lead to measure on a quiet machine. Multi-core and GPU not applicable
  (under a tenth of a millisecond per chunk, order fixed by the world's random draws). No
  switch: the old read is not a lead worth keeping (slower on both measurements, same output).
  Two other ways to know which cells the feature changed, not built: a snapshot of the original
  copy (3.5 MB more per placement, one more copy) and a mask held by each write of the region
  (one more write on the hot path of the decoration of every generated chunk); at best they
  would save the 0.12 ms of the comparison, against a memory cost or a cost on generation: to
  measure on a quiet machine if feature placement becomes a hot spot.
- Client prediction (`apps/client/src/bone_meal_use.rs`): a few block reads per click (lichen:
  6 × 6 × 3 attempts at most; bamboo: 32 cells at most); face masks computed on demand from the
  single shape read (`rc_sim::shape_face_full`), no table; not measured separately (one click).
## G5-F02c: fall (wind charge, per-block rules, multiplier, sound, dust) (25/10, loaded machine)

- Player's impulse context: two fields in `Vitals` (impact point, grace), one decrement per
  tick, one test at landing. Clearing after each movement: immediate exit without a context
  (almost all movements: two comparisons); with a context, the only test that reads the world
  (liquid at the feet) is evaluated last. Not measured separately: a few nanoseconds per
  movement, out of reach of a scene bench.
- Block dust and sound: computed at landing only (one state read, one particle packet; the
  game sends the same packet, 40 to 375 shards drawn by the client). The particle registry is
  walked by name at each send (~100 entries, once per landing): off the hot path, kept.
  Multi-core and GPU not applicable (rare events, packet order detectable by the client).
- Block sound: the sound group lookup is shared between player and mobs
  (`block_fall_sound_at`), without any more copying than before (name cloned once per sound,
  as before).
- "New world" and "mechanisms" fingerprints changed by the sole key
  `current_impulse_context_reset_grace_time` of the player file (checked by removing it):
  intended behavior (the game always writes it).

## G5-F02a: crossed-block pass of players and mobs (25/10, loaded machine)

- Crossed-block pass (`rc_sim::inside_blocks`, player and mobs, every tick): bench
  `g5f02a_bench` (loaded machine, 2 runs, 200,000 passes): standing cow 96 to 101 ns,
  walking (0.2/tick) 201 to 208 ns; the old name-based "powder snow" scan that it
  replaces cost 31 to 32 ns (the pass also covers fire, cactus, campfire, water, lava, rain).
  Leads: state record read directly instead of the view (2 Arc): 106 -> 96..101 ns, kept;
  reused buffers (no allocation in steady state); rain tested only if the entity is
  burning. Multi-core and GPU not applicable (work of the order of 0.1 µs per entity,
  sequential effects). To re-measure on a quiet machine on the scenes with mobs.
  Cells already seen by the traversal (shared with ghast flight, the happy ghast and portals):
  the linear list first retained is quadratic for big boxes (4 × 4 × 4 ghast: hundreds of
  cells, not "a few dozen"). Replaced by a bit mask over the bounding box of the path (one-cell
  margin, fallback list beyond 2^18 cells or outside the box). Bench `g5f02a_bench` (loaded
  machine, 2,000 traversals): ghast path (14.3, 6.1, −11.7) 865 µs -> 18.7 µs; (5, 2, 6)
  299 -> 11.5 µs; 1 block 25.6 -> 2.8 µs; 0.2 block 9.6 -> 2.0 µs. Walking cow (small box),
  2 alternating runs: mask 277 / 233 ns, list 288 / 236 ns (noise). Identical output:
  `seen_mask_matches_linear_list` (3,000 paths, stored sum). The list stays behind
  `RUSTCRAFT_OPT_INSIDE_SEEN_LINEAR` until the quiet-machine bench. Starting hash set not
  re-measured (the mask avoids hashing; to compare on a quiet machine).
  Follow-up 2 (state specific to each LAN player, mob rotation): no in-game speed lead. A
  guest install swaps 4 more fields (movement list, position at the last pass, opened
  container, tracked equipment) among the sixty or so already swapped; the rotation of a mob
  by command does less than before (no more position copy, path stop or forced sync). Not
  measured: no bench scene has a guest or a rotation by command.
## G5-F02b: entity bodies (25/10, loaded machine, to re-measure when idle)

- Block stepped on (magma, redstone ore), every tick for every mob on the ground: effect cell
  reused from the last movement when the position and the support have not changed
  (`RUSTCRAFT_OPT_STEPON_CACHE`, default 1) instead of recomputing it (support read: fence,
  wall, fence gate by tag and by name). Bench `bench_tick_additions` (2 alternated passes,
  2,000,000 calls, zombie standing on stone with a support): 13.3 / 13.3 ns with the cache,
  65.6 / 59.4 ns without; kept. Equivalence: test `magma_and_ore_stepped_on` (reused cell =
  recomputed cell). The rest (state read, comparison of three identifiers) is the minimum.
- Size reading (refit after growth), twice per tick and per mob: 6.8 to 12 ns (two width ×
  height pairs, no allocation). The search for the free center (the game's shapes) only runs
  on growth (growing up, placement), a few times in a mob's life: multi-core and GPU not
  applicable.
- Team filter of the push: short-circuited as long as no team has a rule other than "always";
  otherwise a team lookup for each neighbor that touches. Review: "member → (team rank, rule)"
  index in the scoreboard (`CollisionIndex`, built on first use after a team change, members
  with a canonical UUID keyed by UUID: no UUID string recreated and no walk over the members),
  `RUSTCRAFT_OPT_TEAMIDX=0` for the old walk. Bench `bench_team_push` (loaded machine, 2
  alternated passes, 200,000 calls, mob among 20 neighbors outside any team, "never" team of
  300 members): 2,151 / 2,053 ns with the index, 18,146 / 19,146 ns with the walk (×8.8);
  kept. Without a team, push with no neighbor 11.3 to 11.6 ns in both cases. Memory: one
  entry per member, only when a rule is active. Equivalence: test `collision_rule_index` (index
  = linear search, first registered wins, index rebuilt after a leave and a rule change);
  fingerprints identical with the switch at 0 and at 1.
- Effect cell of the block stepped on, reused only in the tick of its computation (a mob
  without AI does not move but steps on its block every tick): 14.1 to 14.7 ns against 13.3 ns
  before (noise, one more integer comparison), same bench.
- Block step sound: one copy of the sound name per step (no more than one per block walked),
  like the existing fall sound; amethyst chime without allocation.
- Entity sound seed drawn from the sound generator (atomic SplitMix64) instead of the world
  random source: equivalent cost; world fingerprints changed (intended fix, see the commit).
  In tests, a fixed per-thread sequence (distinct from the test seeds of the other generators):
  sound packets no longer depend on the clock (protocol A1), test `sound_seeds_reproducible`.
- Dolphin jump and follow: direction of the yaw (`mth::facing`, a four-entry table) instead of
  the direction closest to the velocity (six dot products): slightly less computation,
  behavior fixed; fingerprints unchanged.
## G5-F02e: contacts with crossed blocks, enchanted boots (25/10, loaded machine)

- Walk over the cells swept by a mob (`rc_sim::contact`, one-axis segments, cells deduplicated
  in a reused vector, contact family read from a one-byte-per-block table built once): 78.6 ns
  per tick and per mob in open air, 79.1 ns in a cobweb (bench `bench_mob_contacts`, best of
  5 × 100,000); 500 mobs ≈ 40 µs per tick. No allocation per tick. Leads not retained for lack
  of gain at this scale: skipping the walk of a motionless mob (the changes of blocks around it
  would have to be tracked), spreading over several cores (the effects modify the world, game
  order imposed), GPU not applicable. To re-measure on a quiet machine (scene with many mobs).
- Player: one walk per tick (same costs); boots without Frost Walker or Soul Speed: immediate
  exit, no query to the world.
- Items and orbs (revision of 25/10): the deferral to the end of the list counted every item at
  its end-of-tick position (difference on the weighted plate). Replaced by a contact right after
  the movement of each item, the plate reading a copy of the positions (reused vector, filled
  once per tick: a 32-byte copy per item, no allocation in steady state). Projectiles: contact
  in the movement step, before the impact. Tripwire: swept box tested against its shape (six
  comparisons per segment, only in a tripwire cell). Mob bench after the revision: 77.3 ns in
  open air, 82.7 ns in a cobweb (noise of the loaded bench, same order as before). To
  re-measure on a quiet machine (scene with many items).
- Revision 2 (25/10 evening, loaded machine, best of 5 × 100,000, A/B alternated against HEAD):
  exact game walk (straight line for projectiles, per-axis segments otherwise, cap of 16
  iterations), fire and lava of projectiles on this walk. Correction alone, no lead: mob
  82-83 ns (before 73-74), arrow step 329-338 ns (before 224-226) in open air in the section
  of a row of plates, 546-552 ns right at the plates (before 393-394). Leads, each measured
  alone then combined:
  - contact family read from the table without copying the view (two atomic counters per
    cell): mob 75-76 ns, arrow 287 ns. Kept.
  - portals: ids held by the table instead of three lookups by name, block read without
    copying the view (also in the palette scan): 65 → 24 ns per call (arrows, falling blocks,
    TNT). Kept.
  - walk through static dispatch (generic) instead of dynamic visit functions: walk alone
    156 → 147 ns (6 cells). Kept.
  - dedupe of the walk by a bit grid over the envelope (instead of a search in the list): walk
    184 → 200-202 ns, loses. Kept behind `RUSTCRAFT_OPT_WALKGRID=1` (off by default) for the
    quiet-machine bench.
  - floor by integer truncation instead of `f64::floor`: loses (mob 82 ns against 72-74, walk
    183-188 against 163-168, 3 alternated passes). Discarded: a switch would cost on every
    call; to re-measure on a quiet machine.
  - skipping the empty: scan of the palettes of the crossed sections (plate, tripwire, fire,
    lava, wooden button families) before any walk. Empty section 44 ns against 254-260 without,
    rich section (about forty states) 102 against 286-292; section with plates: +10 ns (273
    against 262-264, 453-457 against 438-446). Kept, `RUSTCRAFT_OPT_CONTACTPAL=0` disables it.
  - same scan for mobs: empty section 30 ns against 74, rich section 58 against 75, but +20 to
    27 ns when the section has a contact block (open air near a cobweb 95-103 against 73-77, in
    the cobweb 99-105 against 75-79). Gain depends on the scene: kept off,
    `RUSTCRAFT_OPT_CONTACTPAL_MOBS=1` enables it, to be settled on the quiet-machine scenes.
  Total retained against HEAD (2 alternated passes): mob 74-75 ns against 75-78 in open air, 73
  against 76-78 in a cobweb; arrow in an empty section 45 ns against 222-225, rich section
  103-106 against 463-466; section with plates 268-271 against 224-228 and 449 against 390-398
  right at the plates (+14 to 19%: cost of the exact walk, 150 to 180 ns, that fidelity
  requires; the other leads gain nothing there). A single thread (game order imposed, effects
  on the world), GPU not applicable. No allocation per tick outside nearby portals. To
  re-measure on a quiet machine (bow shooting scenes, Nether).
- Player thorns, armor wear and protections (G5-F02d, `rc_sim::thorns`, `vitals.rs`): event code
  (one hit received), never in a per-tick loop. Thorns: six slots read, one random draw per
  enchanted item, no allocation; wear: one `equippable` component read per piece and per hit
  (JSON copy of a few fields, of the order of a microsecond, not measured separately); protection:
  a tag test up front that avoids walking the four pieces for `/kill` and the void.
  Multi-core and GPU not applicable (order of the random draws imposed, work of the order of one hit).

## G6-F08: nature ticks and turtle eggs (01/11, loaded machine)

Bench `bench_g6f08_scan_and_grass` (ignored; `--ignored bench_g6f08 --nocapture`), 2 passes each.

- Box scan (`Server::scan_box`, shared by the spread limit of vines and mushrooms and the
  eyeblossom wake-up, in the game's section/y/z/x order): one chunk lookup per section and direct
  container reads instead of one `get` (hash lookup) per cell. Spread limit of a vine, 9 × 3 × 9
  box, 10 % vines over 32 × 4 × 32: old walk 890-927 ns, scan 297-298 ns; lone vines in an empty
  sky band: 1,829-1,863 ns against 476-477 ns. Kept.
- Palette prefilter of the scan (skip a section whose palette cannot hold the block):
  `RUSTCRAFT_OPT_SCAN_PALETTE=0` cuts it. Dense case unchanged (296-333 ns off against 297-298
  on), sparse case 583-588 ns off against 476-477 on (−19 %). Kept on; to re-measure on a quiet
  machine (jungle, pale garden scenes).
- Grass, mycelium and nylium survival (`LightTables::dampening_into`: the face masks of the light
  engine, pair table of occluded edges, then the attenuation table) instead of the old plain
  attenuation read: 1.53-1.57 ns against 6.78-6.88 ns per test over the whole state table (the
  old path read the 24-byte state info twice). Faithful and faster: kept, no switch.
- Pointed dripstone fluid transfer (bench `bench_speleothem_random_tick_and_geyser`, top of a
  dry stalactite): 24.4-24.6 ns per random tick, stone 23.4-23.9 ns in the same runs (25/10,
  before the transfer: 15.6 against 21.7). 82 % of the draws leave on the first float compare;
  otherwise the block above, then the cell above the root (no walk from the top). Lead measured
  and discarded: reading the root directly instead of the walk of the shared helper, 24.4-24.8 ns
  against 24.4-24.6 (3 alternated passes, nothing measurable). Multi-core and GPU not
  applicable (one block per random tick, world random order).
- Not hot, not optimised further: turtle egg (rare random ticks), eyeblossom (day time compare
  first, scan only when it turns), copper (two property reads before the draw), pitcher crop and
  mushroom (crop draw, 1/25 draw before any scan). Redstone ore
  touch: 6 neighbour reads for the server's particle draws. Multi-core and GPU not applicable
  (world random order imposed, work of a few hundred nanoseconds per event); no allocation per
  tick (the eyeblossom wake-up list is allocated only when a flower turns).
## G1-F16: F3 and debug lines (01/11, loaded machine)

- Server values of the F3 screen in singleplayer (`rc_sim::debug_report`: server heightmaps,
  forced chunks, sprint, local difficulty, spawn census): the screen posts its request only while
  one of the five entries is shown, the server answers once per loop turn with the values of its
  last tick. Micro-bench (`tests_g1f16::bench_debug_report`, 2 passes): values 36 ns, loop turn
  without a request 3 ns, request + answer 47-48 ns; `Copy` report, no allocation per frame or per
  tick (dimension names copied into kept buffers). Leads not taken: per-frame direct reads of the
  server state (impossible without sharing the server's structures across threads, and the
  answer already costs less than 50 ns); multi-core and GPU not applicable (a few lookups).
  Census of the natural spawn pass kept in place (one name copy per tick into the same buffer).
- The game's `%.Nf` rounding (`tech_blocks::java_fixed`, half up on the shortest digits) for the
  reals of the F3 lines: 10 reals 1,116-1,143 ns against 1,168-1,255 ns for Rust's `{:.N}`
  (`debug_screen::tests::bench_java_rounding`, 2 passes): no cost, kept. Only while F3 is open.
- Adapter texts (vendor, API, driver) computed once at startup (`gpu_backend::GpuText`) instead
  of every frame of the F3 screen; Vulkan API version read through wgpu's backend once (one
  property read, no device call). No device refusal: the game's list only applies under MoltenVK,
  absent from our builds.
- Sounds line: two counters already kept up to date by the engine (no walk of the instances).
- Screen frequency label (`video::rate_label`): one integer division and, for a fractional rate,
  one `java_fixed` (about 110 ns per real in the bench above) per frame while the fps entry is
  shown; not measured separately.
- Fluid target: same walk as the block target, a second box per crossed block only for the fluid
  entries; one ray of 20 blocks per frame and per entry, as before (not measured separately: the
  walk is unchanged, the extra box test is of the order of ten ns). Shared walk (`pick::walk`,
  generic, no indirect call) for the crosshair and the F3 rays.
- To re-measure on a quiet machine: F3 open in singleplayer (frame time against main), scene bench
  `tools/bench/scenes.py` (F3 closed: no change expected, nothing is computed).
## G2-F11: inventory screens, recipe book, option menus (01/11, loaded machine)

Interface code driven by input events or by a screen tick; nothing in a world loop. Axes reviewed
(client CPU, regularity, allocations, network, RAM): no per-frame allocation added, no measurable
cost; scene bench not run (no client during the series, `menu`/`inventory` scenes to re-measure
on a quiet machine with the others).

- Recipe book: the open setting is sent only when the visibility changes (as before for the
  button; one more packet when the server changes the setting, which the game also sends back).
  Tab bounce: the check walks the tab collections only when the book shows or a book packet
  arrives (`retab`), then a sine per bouncing tab per frame. Shaped thumbnails: one offset pair
  per recipe instead of none (two comparisons). The vertically stretched tab icon reuses the
  item atlas cell (`Gui::item_stretched`, same quad as the uniform scale).
- Warning text area (`widgets::TextArea`): lines wrapped once per layout; each frame only the
  lines crossing the frame are walked (index range from the scroll, then the clip test), instead
  of all of them; no allocation per frame.
- Social interactions: chat log as a 1024-entry ring (`VecDeque`, one `None` per chat line, no
  allocation for system lines); the list is rebuilt only on layout (tab, search, size), with the
  senders gathered once into a hash set before the sort (instead of a log walk per comparison).
- Spectator menu: game modes read again per tick only while the menu is open (one pass over
  the player list, buffer reused).
- Multi-core and GPU not applicable (a few dozen elements per event, order imposed by the screen).
## G5-F05: orbs, ground items, display and interaction entities (01/11, loaded machine)

- Crossed-block pass of ground items and orbs (new, `rc_sim::inside_blocks`, every tick and per
  item or orb): bench `g5f05_bench` (loaded machine, 2 alternated runs, 200,000 passes), full
  pass 176 to 186 ns at rest on an open floor, 176 to 197 ns next to a cactus, 217 to 242 ns
  sliding. Leads:
  - skipping the empty: a non-burning item or orb can only be acted on by fire, soul fire,
    cactus and lava (water, powder snow and rain only put out, the campfire and the freezing are
    for the living); none of them in the cells of the envelope of its movements (at most 64
    cells, otherwise the full pass): nothing to do. 45 to 48 ns. Kept,
    `RUSTCRAFT_OPT_ITEM_INSIDE_FULL=1` gives the full pass back.
  - decision before taking the buffers out and copying the movements (read in place from the
    physics body): 45 to 48 -> 27 to 29 ns (all three cases). Kept (×6.5 against the full pass).
  - equivalence: `inert_item_shortcut_matches_full_pass` (720 starts around fire, cactus,
    lava, water, at rest, falling, burning: same state with and without the shortcut; 360
    skipped and sum of the outcomes stored).
  - not retained: remembering that a resting item's cells did not change (block-change
    tracking needed for two cell reads, about 20 ns); several cores (effects on the world in the
    game's order) and GPU not applicable.
- Periodic orb merging (new, one scan per orb every 20 ticks): whole list O(n) per scan; lead:
  candidates bucketed by identifier modulo 40 (the only ones that can merge), buckets rebuilt
  once per orb tick in a reused buffer. Bench at the game's rhythm (2 runs, a twentieth of the
  orbs scanning each tick, buckets rebuilt each tick): 20 orbs 0.02-0.03 µs whole list against
  0.04 µs buckets (loses), 100 orbs 0.38-0.39 against 0.17-0.28, 1,000 orbs 34.1 against
  3.3-4.4 (×8 to ×10). Kept from 40 orbs on (`ORB_BUCKETS_FROM`), whole list below;
  `RUSTCRAFT_OPT_ORB_MERGE_LINEAR=1` forces the whole list. Equivalence:
  `orb_merge_buckets_match_whole_list` (600 orbs in 2 × 2 blocks, 421 merged, same outcome per
  orb). Spatial grid not tried (residues already leave a few candidates per scan).
- Mending, interaction, marker, shears: event code (one pickup, one click, one break), no
  loop per tick; Mending reads the enchantment definitions of at most six equipped stacks.
- Display data: decoded when the entity is announced (summon, chunk sent, `/data`), never per
  tick. A matrix transformation goes through the game's SVD (5 Jacobi sweeps, 3 Givens steps:
  a few hundred float operations), bit-identical to the game (`svd_matches_the_game_bit_for_bit`,
  14 matrices computed by the game's code). Not cached: a few displays per chunk at most in the
  bench scenes; to measure on a map with thousands of displays (cache of the decomposition by
  entity, or the decomposed form written at save time like the game).
- Ground item and orb fire: one integer test per tick when not burning; the lava test (fluid
  scan of the box) only on the ticks where the burn point falls (every 20 ticks while burning).
- Quiet-machine bench to do: scene with many items (item farm) and orbs (XP farm), before/after.
## G5-F04: mace, thorns of mobs, equipment modifiers of mobs, enchantment effects (01/11, loaded machine)

- Equipment modifiers of mobs (`rc_sim::mobs::gear`, every tick of every mob, like the game):
  nothing worn and nothing seen = immediate exit (8 empty slots tested, no allocation, the
  previous state is not even allocated: `Option<Box<…>>`, 8 bytes per mob); worn equipment =
  8 stack comparisons, the modifiers recomputed only for a changed slot (allocation on change
  only). Bench `bench_g5f04_gear_and_post_attack` (5 passes, loaded machine): 2.6-2.8 ns per tick
  without equipment, 27-29 ns with full iron armor and a sword (500 armored mobs ≈ 14 µs per
  tick). Leads considered: tracking at the
  mutation sites instead of comparing (dozens of scattered writes to `equipment`, risk of a
  forgotten site: rejected for robustness, the comparison costs a few ns); hash of the slots
  (would cost as much as the comparison of short ids). Multi-core and GPU not applicable (part
  of the mob's own tick, already ordered).
- After-hit effects (`rc_sim::ench_fx`, event code: one landed hit), same bench: victim without
  equipment 4.8 ns; full unenchanted iron armor 302-316 ns with the first filter (the item's default
  component copied, key formatted), 155-161 ns once the default enchantments are read in place
  (kept: −49%); a thorns chestplate that does not trigger 552-563 → 418-432 ns (−23%). Lead not
  tried (shared `Items` API): a per-item flag "default enchantments non-empty" would avoid the
  global lock of the defaults cache for every piece (≈ 30 ns each), to measure on a quiet machine.
  Only the enchanted pieces
  are copied (component present), the definitions are read from the pack (`Arc`), the conditions
  go through the loot condition evaluator; Thorns of the player: same random draws as the former
  hand-written code (G5-F02d tests unchanged). Protection of mobs: immediate exit without
  equipment (the common case), otherwise one component read per piece (the former code read five
  enchantments per piece).
- Mace smash: event code (one hit), area knockback over the mob list once.
- Sweep: the weapon's bonus is computed per swept mob (one more enchantment read per target).
- Review (01/11): after-hit effects added on the dragon's wing and bite hits on mobs (one
  `post_attack` per mob struck, plus one copy of the dragon state per hit so that a thorns
  reaction sees it; event code, a few mobs per hit at most), on the player's hits on armor
  stands, minecarts, crystals and other local-network players (weapon only: no equipment walk),
  and on a projectile that hits its own shooter; the spin attack now goes through the player's
  melee path (one hit per spin). No hot path touched: the per-tick and per-hit costs above are
  unchanged (same bench, the mob melee path is identical). Multi-core and GPU not applicable
  (single events, ordered by the game).

## G3-F13: world tick order, ticking chunks of the entities (02/11, loaded machine)

Fidelity fix (phases of the world tick in the game's order, one block-entity list for every
family, entities frozen outside the "ticking entities" chunks); figures from the micro-bench
`bench_g3f13_entity_test_and_furnace_list` (3 passes, `cargo test --release -p rc-sim
bench_g3f13 -- --ignored --nocapture`), loaded machine (other agents compiling).

- Entity chunk test (`entity_ticks_at`, every entity other than a player and the mobs, which
  already had it: items, orbs, falling blocks and TNT, projectiles, clouds, fangs, spits, bolts,
  crystals, ominous item spawners): 8.2-8.5 ns per entity per tick in range, 6.6-7.3 ns out of
  range (distance tests first, the chunk table lookup last: one hash lookup). Lead considered: a
  per-tick set of the ticking chunks (O(loaded chunks) to build, more than the per-entity test
  for the usual entity counts); not tried.
- Furnaces and brewing stands moved from their own loops into the common block-entity list:
  idle furnace 42.1-42.9 ns per tick against 54.2-54.6 ns for the former loop (−22 %), idle
  brewing stand 55.7-56.3 ns against 82.0-83.0 ns (−32 %). Causes: no key copy per tick (former
  `Vec` of every position), no block-name string per brewing stand per tick, the chunk test of
  `be_ticking` (9 table lookups) reused while consecutive tickers stay in the same chunk, the
  border read once per walk, the shared-map families (campfires, jukeboxes, beacons, conduits,
  gateways, sulfur, and the four new ones) resolved by one lookup instead of the chain
  movers → creaking hearts → map. First version measured on the way: 107 ns (two chains of
  lookups, liveness reading the block and scanning the chunk's block entities), then 57-63 ns
  (one lookup, block still read): both discarded for the figures above. The liveness of the four
  new families reads nothing (their ticker leaves the map with their block entity). Bells: the
  copy of the recorded entities per tick removed (read in place). Spawner blocks: the living
  players read once per walk, only if a spawner ticks.
- Tick bench `bench_ticks` (5 sites, the `redstone` site fails: function `perf:redstone` missing
  from the current model world), before then after, not alternated, loaded machine: means
  2.103 / 1.024 / 2.110 / 0.763 / 1.873 ms before, 0.820-0.856 / 0.711-0.714 / 1.371-1.395 /
  0.543-0.544 / 1.377-1.398 ms after (two passes). The difference follows the machine's load
  (the "before" pass ran during other agents' builds), not this change: no gain is claimed; to
  measure on a quiet machine with `tools/bench/scenes.py`. Its "blocks and fluids" figure no
  longer includes the falling blocks and TNT (they tick with the entities now).
- Review follow-up: tickers of a loaded chunk in the order of the game's per-chunk table, every
  family in one walk over the chunk's block entities (before: five walks, then the furnaces and
  brewing stands in a sixth after the whole batch). Same bench: table order 3.0 µs for 256 block
  entities (12 ns each), whole registration of a chunk of 256 brewing stands 13.8-14.0 µs, once
  per chunk read or generated. Lead not taken: caching the order per chunk (computed once per
  load already). Portal passage of items, mobs and minecarts now skipped outside the ticking
  chunks (one chunk test per mob or minecart per tick, 7-8 ns; items reuse their tick's flag).
  A player's pearl asks for 25 chunks instead of 1: a set lookup per chunk per pearl per tick.
- Multi-core, GPU: not applicable (phase order and list order are observable).
## G6-F14: game master blocks, live templates, clone, locate poi, border (02/11, loaded machine)

- Border collision of the server's entities: the physics view builds the border at first need
  (lazy cell, no work for the light engine or block reads); bench
  `tests_g6f14::bench_border_in_entity_collision`: view alone 0.9 ns, view + border + edge test
  of an entity far from the edge 16.0 ns per movement (best of 5 over 1,000,000). Lead not taken:
  a border cached in the server (would save the build, a few ns) needs a resync at every change
  of the border (command, transition tick, tests that write the fields): kept lazy, to re-measure
  on a quiet machine against a movement of `rc_physics::collide::resolve`.
- Redstone, neighbour changes and scheduled ticks of the structure and test blocks: new arms of
  the existing `Kind` matches (block kind table), no name comparison on the hot path.
- Size detection of a structure block: sections skipped by their palette (`may_contain`) before
  any cell is read, then only the cells of the 161×161 area; command-time only.
- Live template placement (`place template`, structure block): one pass over the palette, block
  entities written once, edge shape pass on a bitset of the placed volume (faces only), sources
  locked in a hash set; command-time only (no per-tick cost). Multi-core and GPU not applicable:
  ordered writes into the live world, like the game.
- `clone` ticks: only the chunks of the source box are visited (`TickQueue::in_box`).
- `place structure`, `place jigsaw`, jigsaw block: the chunks of the start's box plus a one-chunk
  border are copied once into a grid, each chunk column is placed through a 3×3 window of that
  grid (disjoint borrows, no copy per chunk); lead not taken: one copy of the 3×3 neighbourhood
  per chunk (9 copies per chunk instead of about 1). Review round: the placements are no longer
  written back as a diff of the copies but replayed from a journal kept by the region
  (`Region::live_log`, 24 bytes per placement, freed after the command, `None` and one branch per
  write everywhere else), in their order and with their own flags (pool pieces without shape
  update, like the game); bench `tests_g6f14::bench_place_jigsaw` (outpost base plate, best of
  3): 13.1 ms, loaded machine; the old diff path is not kept (it was a fidelity defect, not a
  lead), to compare against `main` on a quiet machine.
  Command-time only. Biome tests of the structure generators go through one helper (`biome_ok`):
  a command placement skips the biome lookup altogether; world generation unchanged (equivalence
  tests of the starts and placements pass with their stored fingerprints).

## L9-STRUCT: features and structures replayed into the live world (02/11, loaded machine)

- Features of bone meal, saplings and `place feature` now replay the region's journal (the
  structure path's `Region::live_log`) with each write's own flags, breaks and direct section
  writes, instead of comparing the 9 copies with the world (fidelity fix). The comparison (0.12 to
  0.13 ms per placement, 9 chunks, section against section) is gone; the replay costs what the
  game's own writes cost (neighbour updates of the trees, flag 19). Bench
  `g6f07_tests::bench_g6f07_deco_copy_and_compare` (`--ignored`, best of 30, other agents
  compiling), main → this branch with `RUSTCRAFT_OPT_DECO_PRIME=0`: complete oak placement 3.17 to
  3.29 ms → 3.03 to 3.08 ms; `tests_g6f14::bench_place_jigsaw` 13.9 to 14.0 ms → 13.3 to 13.5 ms.
- Hot spot found by splitting the bench: the region copy of 9 chunks takes 2.2 to 2.6 ms of the
  3.1 ms oak placement, almost all of it in the six height maps, each walked from the top of the
  world down every column (6 × 256 × up to 384 cells per chunk); unpacking the sections is
  0.11 ms. Leads:
  - one walk per column for the six maps, started at the top of the highest section that is not
    all air (no map counts air), switch `RUSTCRAFT_OPT_DECO_PRIME` (default 1). Same heights
    (tests `prime_many_matches_prime_per_map`, random columns, and
    `deco_chunk_one_walk_matches_per_map`, built, flat and unloaded chunks). Alternated A/B, two
    passes each: region copy 2.22 / 2.58 ms → 0.20 / 0.21 ms; oak placement 3.03 / 3.08 ms →
    0.29 / 0.29 ms; bone meal on a grass field (up to 128 feature placements) 33.5 / 34.3 ms →
    2.38 / 2.39 ms (a single click took two thirds of a tick before); `place jigsaw` of the outpost
    base plate 13.3 / 13.5 ms → 1.75 / 1.75 ms. Kept, on by default.
  - region copies on several threads (B4): spawned threads in the bench, 9 chunks: 1 / 2 / 4 / 8
    threads 0.27-0.29 / 0.17-0.21 / 0.16-0.22 / 0.17-0.27 ms against 0.21 ms on the server thread
    alone: the spawn eats the gain at this size. Built for the grids of live structures (16 chunks
    or more, `RUSTCRAFT_THREADS` or the logical cores, `std::thread::scope`, not the rayon pool
    that the generation thread keeps busy), switch `RUSTCRAFT_OPT_DECO_PAR` (default 0); `place
    jigsaw` 1.76 / 1.95 ms on, 1.85 / 1.89 ms off: under the noise; kept off behind its switch
    until the quiet-machine bench (C6). Same copies in the same order (test
    `region_copies_on_threads_match_one_thread`, 2, 4 and 7 threads).
  - same pattern in world generation (B6): the final height maps of every generated chunk (and
    the generation maps of a flat chunk) were walked map by map from the top. One walk per column
    from the highest slice of 16 that is not all plain air (`DecoChunk::top_non_air`, one slice
    comparison each), switch `RUSTCRAFT_OPT_PRIME_ONE_WALK` (default 1). Bench
    `templ_tests::bench_prime_final` (25 chunks, best of 20, two runs): noise terrain 4.69 / 4.75 ms
    → 0.48 / 0.50 ms (0.19 → 0.02 ms per chunk), flat 6.64 / 6.73 ms → 0.55 / 0.57 ms. Same heights
    (test `generated_heightmaps_one_walk_match`, three seeds and flat, 25 chunks each; the stored
    fingerprints of generation pass). Kept, on by default; scene bench (chunk loading) to run on a
    quiet machine.
    Adversarial review (series 3, 03/11): slices of 4096 cells match the index layout, the start
    index and the stop at the bottom are exact, two maps sharing a bit are both filled; the one
    assumption is that no map counts an air state (true with the game's data: block tags are read
    from the jar only). The game's own priming also starts below the highest section that is not
    all air and never counts plain air, so the result is the game's. No defect found. New tests
    `decor_region::tests::air_states_count_for_no_height_map` (air, cave air and void air count
    for no map; it alone catches cave air counted for the surface maps, which the generated-chunk
    test misses because cave air lies underground) and
    `top_non_air_is_the_top_of_the_highest_filled_slice`; sabotages checked (top one slice too
    low, a map not released once found, cave air counted). `bench_prime_final` again, two runs,
    loaded machine (other agents compiling): noise 5.08 / 5.40 ms → 0.53 / 0.62 ms, flat 9.83 /
    9.38 ms → 0.82 / 0.97 ms, the same ratio as above (about 9 to 11 times).
  - GPU not applicable (a few hundred kilobytes copied and walked once, result needed at once by
    the server thread). Not built: reusing the 9 copies across the 128 attempts of a grass bone
    meal (the copies would have to follow the live side effects of each replay); lazy height
    maps (a branch on every height read of the decoration hot path).
- `place jigsaw` and the jigsaw block load the chunks under their pieces before copying them
  (disk, otherwise the generation thread is awaited, like the game's direct chunk access); only
  when the command reaches unloaded chunks. The wait of `load_now` (portal search too) now wakes on
  the generation thread's channel instead of polling every millisecond: same result, no busy loop.
- Live structures journal a barrier (24 bytes) before each block entity of a template; command
  time only, nothing on world generation (no journal there).
## G3-F08: compute, posteffect, fetchprofile, attribute reset, loot targets (02/11, loaded machine)

- Numeric providers (`rc_sim::loot`, every loot draw): the unchecked reading now carries the
  game's arithmetic error text (`Result<_, String>`, allocated only on an error) instead of a bare
  failure. Bench `bench_g3f08_providers_and_hooks` (ignored by default, 1,000,000 reads, 2 passes
  each): uniform 46.7-46.9 ns, binomial 46.7-46.8 ns, float uniform 45.6-49.0 ns, unchanged
  within noise after the change below. Lead tried and kept: the entries of an aggregate
  provider (`add`, `mul`, `min`, `max`, `avg`, `length`) are evaluated where they are instead of
  copying the JSON list at each reading: sum of three constants 92.5-94.4 ns → 56.4-70.5 ns
  (−25 to −40 %). Lead not tried (shared loot engine, G6-F17's ground): providers compiled once
  per table into a typed tree (no string lookups of `type`, `min`, `max` at each reading), to
  measure on a quiet machine with a loot-heavy scene. Multi-core and GPU not applicable (one draw
  per event, the order of the draws is the game's).
- Per-tick hooks: screen effects sent only when changed (one flag), profile answers polled only
  when a lookup is pending: 2.9 ns per tick together when idle (same bench, 10,000,000 calls).
- `/fetchprofile name|id`: network lookups on worker threads (never the server thread), cached
  10 minutes per name and per identifier (256 entries each, failures included) like the game.
  Review fix: one thread per lookup at first; now a pool started with the first lookup (one
  worker per logical core, one shared queue; the game reuses the idle threads of an unbounded
  I/O pool) and the cache lock is released during the network access (it was held, so lookups
  of different profiles waited on each other). Same bench, bursts of 2,000 `fetchprofile id`
  with instant fake services, 2 passes per version alternated: issuing on the server thread
  16.7-17.4 µs → 1.9-2.8 µs per command, whole burst answered 53.4-55.7 ms → 7.7-11.7 ms.
  Memory: the idle workers keep their stacks reserved (address space, a few committed pages
  each) instead of up to one transient thread per pending lookup. Answers still read at a
  following tick on the server thread: nothing observable changes but the order in which
  simultaneous lookups finish, which the network decides in the game as well.
  `/compute`, `/posteffect`, `/attribute`, `/loot`, `/enchant`, `/ride`, `/datapack`, `/spectate`:
  command code, no hot path. Attribute base reset builds one mob of the species to read its
  construction default (once per command). Player save: one key check (`post_effects`).
- Fidelity side effect on the simulation: zombies, husks, zombie villagers, drowned, spiders,
  cave spiders and endermen now move at their single-precision speed widened like the game
  (0.23F, 0.3F): no cost change, world fingerprints updated in `tests_opt.rs` (causes checked
  one by one).
## G4-F17: trading uses, typed input, tooltips, smoothing, timeline curves, saves, profiling (02/11, loaded machine)

Interface, save-folder and debug code driven by events, a screen or the F3+L recording; nothing
added to a world loop outside these. Axes reviewed (client CPU, integrated server CPU,
regularity, allocations, disk, RAM, startup, multi-core): figures below, the rest unchanged.

- Optimize World (`rc_sim::storage::optimize`, three passes like the game): micro-bench
  `storage::tests::bench_optimize` (8 region files of 1024 chunks, 2 passes, loaded machine).
  Without erasing nothing can change, so no chunk is read again: 1.1-1.2 ms against 70-100 ms for
  reading and inflating every chunk (what the old single pass did). With erasing, region files
  shared among threads (one file per thread, `RUSTCRAFT_THREADS`, default the logical cores):
  1 thread 359-371 ms, 2 threads 194-200 ms, 4 threads 118-212 ms, 8 threads 83-146 ms: kept
  multi-core. Leads not taken: parallel chunks inside one region file (writes to one file stay
  sequential, the inflate/deflate is per chunk and already spread by files); GPU not applicable
  (zlib and NBT). The entity and POI passes only read region headers (8 KiB per file).
- Named timeline curves (`rc_env::named_ease`, bit for bit with the game, values printed by the
  game itself): `ease_tests::bench_named_curves`, 30 curves x 10 000 points, 4.77-4.84 ns per call
  against 6.49-6.50 ns for the Bezier curve (2 passes). Sine and cosine of the game's table
  computed on demand from the entry index (same value as the table, no 256 KiB table kept);
  base-pack timelines never use them (no change in play).
- Trading: the take of the result reuses the prediction (one offer lookup, at most 2 payment
  checks); only on a click on the result slot. Not measured (a few comparisons per click).
- Typed text: the event check reads only the first character (was: every character).
- Tooltip effect durations: one atomic load of the tick rate per effect line (set once per
  client tick with the other tooltip context bits).
- Mouse smoothing, wheel accumulation, gliding roll: the game's sign (zero kept) replaces Rust's
  `signum`: one more comparison; no measurable cost.
- F3+L recording (only while it runs, 10 s): counters on the open path (at most 10 increments
  per frame, `&'static` names, one path string per new zone), one single-frame collection per
  frame like the game's per-tick profiler (rebuilt each frame; reusing its map would leave zero
  entries in the tree), working-set read per frame (`heap MiB`), processor times read at most
  every 501 ms (`NtQuerySystemInformation` / `/proc/stat`). Archive written once at the end.
- To re-measure on a quiet machine: `bench_optimize` and `bench_named_curves` (2 passes each);
  scene bench not concerned (nothing changed in the world loop).
## G1-F17: option labels and menus, near plane of the fluid camera test (02/11, loaded machine)

- Fluid-in-camera test (lava, powder snow): the near plane now follows the field-of-view option.
  Its half-height is computed once when the option is applied (`EnvState::set_fov_option`) and
  kept, instead of a tangent each frame (the former fixed 70° could be folded at compile time):
  micro-bench `environ::cache_tests::bench_near_half` (2 passes, 10 M calls) 2.71-2.72 ns per frame
  with the tangent against 0.56-0.57 ns with the kept value; kept. Third track, a table of the
  81 option angles (30..=110) read each frame: 0.56-0.57 ns (2 passes, rerun 02/11 with the
  tangent at 2.74-2.76 ns), no gain over the single kept value, which needs no table: dropped.
  In-wall overlay test: near plane now sized by the culling field of view (max of the frame's and
  the option, like the game); same tangent per frame as before, only its input changed, no cost
  to measure. Rest of the per-frame test
  unchanged (same neighbourhood cache, same five points). Multi-core and GPU not applicable (five
  block reads per frame).
- Option labels (brightness, fixed decimals through `tech_blocks::java_fixed`, about 110 ns per
  real, see G1-F16; composed OFF key): computed on layout and on slider moves only, never per
  frame; no measurable cost. Japanese glyph default: one system call at the first read, cached.
- To re-measure on a quiet machine with the other menu missions: `menu` scene of
  `tools/bench/scenes.py` (no change expected), screenshots of the video and accessibility
  screens (titles 2 px lower, labels).

## L9-TICK: pearl and fight tickets, dimension state, cauldrons in the crossed-block pass, packed block entities (02/11, loaded machine)

Fidelity fixes; figures from the existing micro-benches (`bench_g3f13_entity_test_and_furnace_list`,
`bench_mob_contacts`, `cargo test --release -p rc-sim <name> -- --ignored --nocapture`), loaded
machine (other agents compiling: the untouched furnace loop moved from 55 to 100 ns between two
runs), so only the order of magnitude is claimed.

- Entity chunk test (`entity_ticks_at`): the ticket test (pearls, dragon fight) only runs when
  the player's zone test fails, and costs a flag test plus an empty-list test without tickets:
  8.6-9.4 ns in range, 7.1-7.8 ns out of range, against 8.2-8.5 / 6.6-7.3 ns recorded by G3-F13
  (same bench, other load): no measurable change. Pearl tickets live in a small vector (one per
  chunk a pearl entered in the last 40 ticks): a linear scan beats a map at these sizes.
- Chunk tests of the world tick (`chunk_simulated`, `chunk_entity_ticking`,
  `chunk_simulated_now`): same ticket test appended after the existing ones.
- Dragon fight arena: the 19 × 19 chunk list is only built when one of them is missing (before:
  a 17 × 17 list allocated every tick of the fight, then filtered).
- Block-entity registration of a loaded chunk: one pass over each block entity's fields instead
  of one search per field read (six) now that `keepPacked` is read too: 256 block entities
  14.0-14.3 µs, against 15.5-16.8 µs for the first version (six searches + the new one) and
  13.8-14.0 µs recorded by G3-F13. Packed ones: promotion order computed once per chunk, linear
  in their count (bucket counts kept, recounted on growth).
- Crossed-block pass: cauldrons are three more block comparisons, after the existing ones, for
  the non-air cells only, and their 14-box shape is built only for a cauldron cell. The contact
  pass of the mobs no longer looks for cauldrons (smaller family mask for its palette pre-scan):
  `bench_mob_contacts` 74.6-75.5 ns per tick and per mob (78.6 ns in open air and 79.1 ns in a
  cobweb recorded by G5-F02e, other load): no measurable change.
- Generated chunks now resume their scheduled ticks and register their tickers (fidelity: never
  done before during the session), once per chunk arrival: the cost of `register_chunk_tickers`
  above per chunk, plus the ticks it schedules.
- Leads not taken: a per-tick set of the ticking chunks (would also hold the tickets; considered
  by G3-F13, more costly to build than the per-entity test at usual counts). Multi-core and GPU:
  not applicable (ticket expiry, promotion order and effect order are observable sequences).
- To re-measure on a quiet machine: `tools/bench/scenes.py` scenes with generated terrain
  (new chunks now register their tickers), and the dragon fight if a scene covers it.
## G3-F06: in-game tests (`/test`), relay of command successes, pack templates (02/11, loaded machine)

- Block-based tests read their structure every tick (start, accept, fail and log blocks). The game
  walks the structure cell by cell once per mode (three walks per tick, up to 48³ cells each).
  Kept: one walk for the four modes (nothing changes the blocks between the game's walks), the
  sections whose palette holds no test block skipped (`may_contain`), only the cells of the
  structure read in the others, then sorted back into the game's order (relative z, y, x).
  Bench `tests_g3f06::bench_test_block_search` (48³, a few test blocks, best of 5 × 20): one
  cell-by-cell walk 1.021 ms, section walk 0.054 ms (×19, per walk; the game makes three per tick).
  Equivalence: `test_block_search_matches_the_cellwise_reference` (four rotations, chunk and
  section borders). The cell walk stays behind `RUSTCRAFT_OPT_GT_SCAN=0` until the quiet-machine
  bench. Leads not taken: cache of the test block positions between ticks (blocks move: pistons,
  players; a rescan is needed every tick anyway, and the walk is already skip-dominated),
  multi-core and GPU (a few thousand cells, ordered results, below a thread start).
- Relay of command successes (`chat.type.admin`): the table of the game's non-relayed keys
  (97 sorted keys, binary search) is read only when an operator other than the source could
  receive the message: never for the host's own commands in singleplayer (the common case pays
  one rule read and one comparison). Command block output: one fewer rule read on success.
- Templates of the packs: looked up on demand (at most once per identifier, the absence
  remembered by the template cache, cleared on `/reload`), nothing read at opening; a world with
  the base pack only skips the pack listing. Lead not taken: stacking every pack `.nbt` at
  opening (startup and memory for every world, for files most worlds never ask for).
- Test runner: per tick, only the tests of the current batch are visited (indices, no copy of
  the tests); structure placement, clearing and barriers run once per test start (command or
  batch start), with the clearing's tick and block event removal limited to the box's chunks
  (`TickQueue::clear_box`). Not on any path when no test runs (one `Option` test per tick).
- To re-measure on a quiet machine: `bench_test_block_search` (2 passes, both walks); scene bench
  not concerned (no test runs in the scenes; the per-tick cost without a runner is one branch).
## G4-F08: monsters and entities (02/11, loaded machine)

- Opening shulker (server): the push now walks the mobs (species-bound prefilter kept), the
  primed TNT and falling blocks, the items and the orbs, and moves what it touches with its
  collisions (`mob_move`, `Body::shove`) instead of placing the mobs. Micro-bench
  `tests_g4f08::bench_g4f08_shulker_push_scan` (2 passes; 300 mobs, 1,000 items, 100 orbs, a slice
  touching nothing): former scan (mobs only) 3.76-3.83 µs, new 6.61-6.69 µs per push. Only while
  a shulker opens further (20 ticks per opening): about 60 µs per opening in such a crowd. Lead
  noted, not taken for this path alone: a spatial index of the items and orbs, which the piston push
  (`push_targets`, same full walk) would share. Multi-core, GPU: not applicable (a few boxes).
- Client side, the zones are built only for the shulkers opening further this tick; the player's
  collision boxes for the push are gathered only when a zone exists: nothing more per tick otherwise.
- Body rotation of a standing mob carrying a mob (client): the "first passenger is a mob" flag is
  refreshed before each entity tick. Two leads, same flags (checked by
  `entities::size_tests::standing_mob_carrying_a_mob_keeps_its_body`): walk of every entity 3.12-3.20
  µs per tick, list of the vehicles that received passengers 0.127-0.129 µs (bench
  `entities::size_tests::bench_mark_mob_riders`, 3,000 zombies, 10 vehicles, 2 passes; whole entity
  tick 138-141 µs). Kept: the list; `RUSTCRAFT_OPT_RIDERLIST=0` brings back the walk.
- Spawning and breeze dangers in one per-species test: same name tests as before, the fire tag read
  only for a species that is not fire immune, the "immune to" tag only for six species; no extra
  work on the spawning path, no bench. The new-world fingerprint changes (Nether entity chunks only:
  fire-immune species now spawn in fire cells like the game), see `tests_opt.rs`.
- Primed TNT and falling blocks written with the entity chunks: once per save, a dozen fields per
  entity. Warden targets: team names built only when a team exists, border test two comparisons.
- To re-measure on a quiet machine: the two micro-benches above; scene bench not concerned (no
  shulker, jockey or sulfur cube in the scenes).
- Resumption (fidelity): every mob push now scans the minecarts (the game pushes them, the cramming
  counts them, a burning cube hurts them). Bench `tests_g4f08::bench_g4f08_mob_push_with_minecarts`
  (2 passes, 20 neighbouring zombies): 1,508-1,509 ns per push without minecart, 1,694-1,758 ns
  with 100 minecarts that touch nothing, about 2 ns per minecart and per push (300 mobs, 100
  minecarts: about 60 µs per tick). Nothing allocated when no minecart touches (empty list).
  Leads noted, not taken at these counts: a spatial grid of the minecarts, or their boxes computed
  once per tick (a box is 6 additions from the position). Multi-core: not applicable (mutations
  in list order).
- The spawning danger test reads the seven "immune to" tags by `TagId` (one bit word per block,
  no name hashing; before: six tags read by name), `default_immune_to` included for the other
  species: one bit test more on the spawning path, no bench.
- The decorations in a shulker or piston push: nothing scanned while no decoration exists; the
  live list is walked (box of each) only when a shulker opens further or a block moves.
- Fingerprints: the three change for one cause only, the `PortalCooldown` now written by every mob
  (the game writes it for every entity); without that line the three come back identical.
## G6-F17: loot functions, providers and entries (02/11, loaded machine)

- Engine (`loot.rs`): each stack now goes through the entry's, the composites', the pool's and
  the table's modifiers as soon as it is made (one receiver per level, `Sink`) instead of being
  collected per entry then per roll: the draws of the functions interleave with those of nested
  tables like the game's, and the two vectors per roll are gone. The candidate buffer is kept
  from one roll and one pool to the next; weightless candidates are dropped while expanding (no
  second pass); a candidate stays 40 bytes (registry id of an expanded tag item instead of a
  string, composite modifiers as an optional boxed slice, absent in the base pack). Tables
  running: a `RefCell<Vec<usize>>` of data pointers in the context (push and pop per table run)
  replaces the depth counter.
- Equivalence (A1/A2): `loot::bench_tests::loot_bench` (ignored) fingerprints the 1447 tables of
  the base pack, 16 seeds each, fortune III pickaxe, luck 1, looting II: `af880b1baa83fb25` on
  389108f0 and on this branch.
- Time per draw (best of 7 x 5 000 draws, same bench, built before and after; loaded machine, 4
  alternating pairs). Quietest pair, before / after (first candidate layout, 72 bytes): stone
  442-450 / 434-435 ns, oak leaves 1304-1340 / 1329-1355, zombie 823 / 813-826, simple dungeon
  5168-5184 / 5273-5353, fishing 819-823 / 822-830. The dungeon's +2 % led to the 40-byte
  candidate and the kept buffer; the following pairs vary by up to 40 % between two passes of the
  same build (other compilations): no conclusion, nothing removed. To re-measure on a quiet
  machine: `loot_bench` on both builds, 2 alternating passes.
- Not applicable: multi-core and GPU (a draw is one chain on one generator, a few microseconds,
  main thread); caching resolved nested sets or tag item lists (none on the measured block,
  entity, chest and fishing tables).
- Value checks of `/execute if predicate` and advancements: one loot evaluator built per check
  (a few fields and a copy of the condition context). `/item modify`, `/loot`, chest loot: the
  world is lent to the draw by reference (`LootView`), nothing copied.
## L9-COMBAT: smash and wind reach, hurt trigger, stand attributes, summon, name cache (02/11, loaded machine)

- Wind Burst / wind charge bursts now reach armor stands, mannequins, what is hung, items, orbs,
  TNT, falling blocks, minecarts, crystals and the other players. Bench
  `tests_l9combat::bench_wind_burst_decorations` (50 bursts per figure, 2 passes): burst alone
  371-443 us (block rays, unchanged); with 400 armor stands around (68 in range) 825-903 us, of
  which the decoration pass 454-636 us: scan of the 400 entries 17-25 us, visible part of the 68
  stands 404-601 us (about 6 us each, 54 rays per stand box: the game's own computation, shared
  with every explosion). Without decorations the pass costs 0.01-0.12 us.
- Decoration pass, lead 1: one pass by registry index (chunk known) instead of a search by
  identifier per decoration (quadratic): scan 17-25 us against 43.6 us; kept. Lead 2: the push of
  a stand searches the registry once instead of three times (68 pushes): below the noise of the
  bench, kept (fewer searches, same result). Lead 3, distance tested before the box (painting
  boxes computed only in range): part of the 17-25 us scan, kept.
- Visible part of a box (`seen_percent`): the dominant cost for any entity in range, mobs
  included (pre-existing). Multi-core: not feasible as is (the world is read through the server,
  which is not shareable between threads); a lead for the explosion engine as a whole (a read-only
  block view shared by threads, or a cache of the cells crossed by the rays), to measure with the
  TNT scenes, not for this mission.
- Smash knockback: one pass over the living decorations and the other players per smash (rare);
  no measurable cost. `player_hurt_entity` from the mob's hurt: one lookup of the open criteria
  per hurt by the player; the projectile is copied only while such a criterion is open (none once
  the three vanilla advancements are done). Explosion packets: one per player only with other
  players of the local network (solo: same single event).
- `/attribute` on stands, `/summon` of stands and mannequins, the name cache: command and join
  paths, no measurable cost; the name cache is read once per world and written on each new entry
  (like the game). Spawn of a living decoration: synchronised attributes read from its compound
  (a few attributes, equipment parsed once).
- Multi-core and GPU not applicable to the rest (event paths, a few entities). To re-measure on a
  quiet machine: the bench above (2 passes), `fingerprint_*` tests unchanged.
## L10-COMBAT: explosion reach and direct entity, decoration sizes, sweep, normalization (02/11, loaded machine)

- Bench `tests_l10combat::bench_l10_combat` (200 calls per figure, 3 passes): new pass of the
  entities that never move (displays, markers, interactions kept as compounds, then clouds) 0.06-0.07
  us with nothing kept, 60.4-60.8 us with 4,000 markers out of range (15 ns per compound: type then
  position read; most real compounds are decorations, rejected on their type). New projectile pass
  (every projectile instead of the redirectable ones) 0.02-0.03 us empty, 1.32-1.38 us with 200 far
  arrows. Both run once per explosion or burst (a TNT burst alone costs hundreds of us).
- Lead discarded, chunk range of the still pass (only the chunks the explosion reaches): exact only
  if a kept compound always sits in the list of its position's chunk, which a teleport breaks (the
  compound stays in the list it was read in): it would miss a teleported display. Not kept; a
  registry of the pushable compounds (like the decorations') is the lead to measure if a world with
  thousands of markers shows it.
- Still armor stands now tracked every tick (weightless, markers, immovable mannequins, stands in
  water): 400 weightless stands with a helmet, deco tick 47.8-49.8 us against 48.4-48.5 us with the
  old early return (alternated, same build): no measurable cost. Their scale read every tick stays
  free: the attribute is only computed for a stand with a saved scale entry or a worn stack carrying
  modifiers or enchantments of its own (no item scales by default).
- Normalization sweep (33 sites through the common helper): same work, one threshold. Mannequin
  hurt, sweep over decorations and other players, LAN players in ordinary explosions: per event,
  a few entities; no hot spot. Multi-core and GPU not applicable. To re-measure on a quiet machine:
  the bench above (3 passes), TNT and creeper scenes.
- Review round (02/11, loaded machine, other agents compiling): still stands now cancel their
  speeds under 0.003 each tick (a map and a bit comparison of the saved motion; the compound is
  only rewritten when a value changes). Same bench, 2 passes: 400 weightless stands 56.5-56.6 us
  per deco tick (48 us earlier in the day, other load): the added work is a few ns per stand, the
  gap is attributed to the load and stays to re-measure on a quiet machine (no switch: the
  cancellation is the game's behaviour). The bobber now sends its velocity only when it changed
  (fewer packets at rest); the explosion author check (pvp rule, team) runs once per player in
  range of an explosion with a player author. No hot spot; multi-core and GPU not applicable.
## L9-CLIENT: screen limits (02/11, loaded machine)

- Wheel: discrete mode and sensitivity read once per wheel event (two option lookups, before any
  screen) and applied by one shared conversion; per event, never per frame: no cost to measure.
- Bundles: the stop-hover rule (announce "no item chosen" when the hovered slot changes away from
  a bundle) is checked every frame with a container or creative screen open, as the game does
  (a per-tick check would miss slots crossed between two ticks). One hit test per frame: the
  container screen's existing one; the creative screen's built a 54-entry list at each call,
  replaced by an iterator (same slot at every point, test `hit_test_without_a_list_finds_the_same_slot`).
  Micro-bench `creative_screen::tests::bench_creative_hit_test` (3 alternated passes, 200 000
  calls): with the list 61.2-62.1 ns per hit test, without 22.7-22.8 ns. Kept (no allocation per frame).
- Trail particle: sent with the particle type looked up by name like every other particle send
  (eyeblossom turns and creaking heart calls: rare, at most two per tick per calling heart); client
  tick shared with the vibration signal (`home_in`), same cost per particle. Not a hot spot.
- Checkbox label lines computed at layout (screen size change), never per frame. CSV column order
  computed once per archive. System locale read once (cached). Merchant result check: one offer
  lookup per click on the result slot.
- Multi-core and GPU not applicable (per-event or per-frame UI work of a few hundred
  nanoseconds at most). Scenes to re-measure on a quiet machine: `menu` and an in-game scene with
  the creative inventory open (no change expected beyond the removed allocation).
## L10-LOOT: value checks, set_contents order, removed components, pack tags (02/11, loaded machine)

- Equivalence (A1/A2): `loot::bench_tests::loot_bench` fingerprint of the 1447 base pack tables
  (16 seeds each) `af880b1baa83fb25` on 2b3094dd and on this branch, every per-table line
  identical (`RUSTCRAFT_LOOT_PRINT` files compared). The base pack has no bound-free value check
  and no set_contents with drawing children, so its draws do not move.
- Entry expansion: candidates are handed to a callback (a pool's roll pushes them into its kept
  buffer, set_contents creates each at once, its per-entry vector is gone). Three spellings
  built and benched against the old code in the same series (best of 7 x 5000 draws, `essai`
  profile, medians of alternated passes). Series A, 6 passes, old ns then generic `F: FnMut` /
  `&mut dyn FnMut`: stone 444 +0.6 / +2.0 %, oak leaves 1287 +0.2 / -1.0 %, zombie 821 +0.9 /
  +1.3 %, simple dungeon 5170 +0.9 / +0.9 %, fishing 821 +1.7 / +1.0 % (geometric mean +0.9 /
  +0.8 %). Series B, 6 passes, two-case enum (direct push for a pool, creation for contents) /
  `dyn`: stone +3.5 / +0.6 %, oak +2.0 / -1.4 %, zombie +4.1 / +2.4 %, dungeon +3.7 / +1.4 %,
  fishing +4.7 / +1.8 % (mean +3.6 / +1.0 %). Two earlier 4-pass series: generic +1.2 %, `dyn`
  +0.5 %. The enum, which does the least work, is the slowest: inlining and layout of the
  incremental 16-unit build, not the algorithm; old-code passes alone spread 1.5-2 %. Kept:
  `dyn` (one instance, best mean). C6 not met as written: the generic and enum spellings are
  compile-time shapes of the same code, they cannot sit behind a run-time switch (a branch
  would itself change the inlining being measured), and the gap (+0.1 % between generic and
  `dyn`) is inside the old code's own spread, so they were not kept behind a `cfg` either. To
  re-measure on a quiet machine with the full release profile, rebuild by hand: generic =
  `Loot::expand` takes `out: &mut F` with `F: FnMut(Cand<'v>, &mut dyn Rand)` (recursive calls
  unchanged); enum = `out` becomes a two-case target, a pool's candidate buffer (pushed
  directly) or the container-contents creator (stack made at once), matched where `push` is
  called.
- `Items::max_stack`: the removal (`!max_stack_size`) is read in the same pass over the patch as
  the value (one length test more per entry, no second scan); `damageable` answers a patch-free
  stack with one item lookup (before: two empty component lookups then the same item lookup).
- Pack tags (`DataPack::tag`): a registry folder's tags are all resolved on its first request with
  the shared `packs::resolve_tags` and kept (`Arc<[String]>` per tag, no copy per read); before,
  each read parsed-and-cached the raw file and walked it. Readers: nested loot tables, slot
  sources, set_instrument, goat horns (one per horn), bone meal features (one per bone meal).
  No measurable cost: none of them is on a per-tick path.
- Not applicable: multi-core and GPU (a draw is one chain on one generator, a few
  microseconds); the value-check change removes a draw, it adds no work.
- Review follow-up: wear brought back to 0 (set_damage, Mending) and the rest of a broken stack
  are written through the patch writer instead of a removal (same single lookup of the item's
  defaults); the composter reads `compostable` on the stack (one patch lookup before the same
  default lookup) for the click, the hopper and the farmer. Event-driven paths, nothing per
  tick: no measurement.

## L10-ENT: entity pushes, ground item fields, raids per dimension (02/11, loaded machine)

- Piston push (`push_targets`, per moving block and per tick): minecarts now walked (one box test
  each), and every decoration instead of the attached ones only (armor stands and mannequins are
  pushed). Micro-bench `tests_l10ent::bench_l10ent_piston_push_scan` (2 passes, 20,000 scans, a
  slice touching nothing among 20 armor stands, 20 mannequins, 10 frames and 100 minecarts):
  former decoration scan (attached only, looked up by identifier) 652-696 ns; new scan of every
  decoration: looked up by identifier with the kind read from the compound 2,142-2,233 ns, from
  the registry entry alone 1,900 ns, by identifier with the entry's kind alone 1,886-1,896 ns, both
  leads together 1,647-1,663 ns (kept; `RUSTCRAFT_OPT_DECOSCAN=0` brings back the identifier
  lookup and the compound's kind); minecart walk 141-142 ns for 100. Same decorations in the same
  order for both lookups: `tests_l10ent::push_scan_lookups_agree`. The same lookup-per-entry walk
  replaced by the entry's chunk in the cushion, frame-signal, knot, burst and survival loops of the
  decorations (same compound found, speed only; no bench).
- Leads noted, not taken at these counts: the position of each decoration cached in its registry
  entry (every write of `Pos` would have to refresh it; about 1 µs per push with 40 living
  decorations to win), a spatial grid of the minecarts shared with the mob push (G4-F08 lead).
  Multi-core and GPU: not applicable (a few boxes per push, mutations in list order, the cap of
  each entity depends on the pushes before it in the tick).
- Opening shulker: the minecart walk is skipped while the world has none (empty list); otherwise
  one box test per minecart per opening tick.
- Raids of the parked worlds, every tick like the game: an empty list costs a counter increment
  without visiting its world, 3 ns per tick; a world with raids is visited (`in_world`, two world
  swaps) 291-293 ns per tick per world (`tests_l10ent::bench_l10ent_parked_raids`, 2 passes, 20,000
  ticks, one inactive raid). Lead noted: tick an inactive raid of a parked world without the visit
  (only its counter and the difficulty and rule stops would run), not taken at one raid.
- Armor stands and mannequins without physics now run their tracking each tick (one interval test
  per tick, a packet every 60 ticks like every entity). Cramming: one addition per pushed entity.
- Ground items and orbs: one writer for the save and `/data` (the save's former writer and the
  `/data` one built the compound twice in two shapes), ten common fields more per entity per save;
  one reader for the chunk, the two summon paths and `/data`.
- Fingerprints: only the new world's changes, for two causes (items alone b36c37ff..., raids alone
  0947daba..., both reverted 700c26d9... again); mechanisms unchanged.
- To re-measure on a quiet machine: the two micro-benches above. Scene bench not concerned (no
  piston on an entity, no shulker, no parked raid in the scenes).

## L10-CLIENT: bundle choices per stack, particle options of `/particle` (02/11, loaded machine)

- Bundle choices: the single cell (one slot, one index) became a table per stack key (menu slot,
  or place in the creative grid's list), since the game keeps the choice on the stack. Micro-bench
  `bundle::tests::bench_bundle_choices` (3 passes, 4 million lookups): former cell 0.52-0.59 ns per
  lookup, table empty 1.14-1.33 ns, with 4 choices 1.91-2.09 ns; hovered-slot rule of a frame (mouse
  changing slot every frame, the worst case) 3.70-4.00 ns. Called for the bundles drawn in an open
  screen and once per frame for the rule: a few nanoseconds per frame, not a hot spot; an empty-table
  fast path (count in a cell) would save about 0.6 ns per drawn bundle, below anything a frame can
  show, not kept (more state for nothing measurable). No allocation per frame (the table only grows
  when a wheel notch chooses an item).
- Window resized with a screen open: one wheel reset, and for the creative screen one grid reload
  (the same as choosing the tab, already done on each tab change). Per event, never per frame.
- `/particle` options: decoded once at parse and once at execution, like before; the error list only
  allocates when a field fails. Geyser options now carried to the guests (4 or 8 bytes, as before,
  with the real values instead of zeros). Not a hot spot; multi-core and GPU not applicable.
- Scenes to re-measure on a quiet machine: `menu` and an in-game scene with the creative inventory
  open (saved hotbars tab with bundles); no change expected.
## L10-ORDER: one entity walk for every family, in the order of arrival (02/11, loaded machine)

Fidelity fix (the world's entities tick in the order they joined its list, every kind together:
`walk.rs`); figures from the micro-bench `bench_l10order_walk` (`cargo test --release -p rc-sim
bench_l10order -- --ignored --nocapture`): server ticks of a flat world with 1,000 entities,
test executables of `integ/lot9` (before) and of this branch (after) run alternately, three
rounds, minimum of 5 passes of 400 ticks per scene, loaded machine (other agents compiling).

- Design: a rank per entity (stamped when it enters its family's list, renewed when it comes
  back into a ticking chunk), each family list kept in rank order, a k-way merge of the lists by
  runs; the heads of the families of the walk that have entities are cached (one rank read per
  step, not one per family). No allocation in steady state (live position lists and item/orb
  placeholders reused); a family is re-sorted (stable sort) only on a tick where one of its
  entities came back.
- Five families interleaved (worst case: ranks alternate, one run per entity): 1,382-1,402 µs
  before, 1,430-1,447 µs after per server tick (+3.4 %, about 50 ns per entity). This is the
  price of the order itself: the walk changes family at every entity (code and data of five
  families in turn instead of 200 in a row), and items and orbs then tick one by one.
- Same entities summoned family after family (one run per family, the former order):
  1,369-1,411 µs before, 1,356-1,379 µs after: equal. Families alone (1,000 of one):
  items 889-914 / 889-906 µs, orbs 1,177-1,201 / 1,167-1,183, minecarts 4,325-4,391 /
  4,384-4,407 (+1 %), arrows stuck in the ground 417-421 / 431-437 (+3 %: a cheap tick, about
  15 ns of dispatch per entity; with the dispatch inlined, 440-450 / 443-445 on a noisier round:
  not resolved on this machine), pigs without AI 13.3-13.5 / 12.6-12.8 ms (−5 %, not explained,
  not claimed).
- Items and orbs, first version: ticked one by one out of their slot (a removed placeholder
  swapped in; item 360 bytes, orb 280, of which 184 of network tracker): +45 ns per item and
  +30 ns per orb (+5 %, +2.5 % on the families alone). Kept for short runs only; a run of at
  least an eighth of the list now ticks with the list taken out once and the plates' positions in
  `contact.live_items` / `live_orbs` (O(list) per run, O(1) per entity amortized): figures above.
  Equivalence test `item_runs_and_one_by_one_give_the_same_world` (plate states every tick,
  final positions; fails when the live positions are not updated).
- Merge, first version: the ranks of all 15 families re-read at each step: +3.5-5.6 % on the
  interleaved scene against the old code; cached heads: not distinguishable from it within this
  machine's noise, kept (strictly less work).
- Falling blocks and TNT: the copy ticked each tick no longer clones the physics body (one
  allocation per entity per tick removed): 1,000 primed TNT on the ground 988-1,000 µs before,
  975-994 after: below the noise.
- Leads not taken: an order buffer sorted at each walk (O(n log n), a scratch allocation above
  4 KB) instead of the merge (O(n + runs × families)): not measured, the merge is already within
  noise in the grouped case; items boxed in their list (8-byte moves) rejected (one more
  allocation per item, a pointer chase in the quadratic merge pass of the items); ranks in arrays
  beside each family list (no read through a mob's box): not tried, the mob's rank line is read by
  its visit anyway. To re-measure on a quiet machine: this bench (both scenes), then scenes
  `crowd`, `tnt`, `redstone`, `village` of `tools/bench/scenes.py`.
- Memory: 8 bytes per entity (rank), a removed flag per bolt and ominous item spawner, the list
  of entities out of the list (16 bytes each, only those outside a ticking chunk), two
  placeholders (item, orb). Multi-core, GPU: not applicable (the order is observable by the
  player and by a server, the walk is sequential by definition).
- Review fixes (02/11, loaded machine, same bench, one executable, switch alternated, two
  rounds): the item merge moved from after the walk into the item's own tick (same pairs
  tested, the list taken out once per due merge when ticked one by one); orbs born during the
  walk join the built merge buckets (O(1) per birth); entities back in a ticking chunk re-ranked
  by a pass at the chunk source's step (one emptiness test per tick when nothing is out, a
  binary search per entity out otherwise); player cooldowns counted after the walk (no cost).
  New switch `RUSTCRAFT_OPT_ITEM_RUNS=N` (default 8: a run of at least 1/N of the list ticks
  with the list taken out; 0: always one by one, the first version): default against 0,
  interleaved 1,455-1,474 / 1,455-1,481 µs (equal: its runs are short, one by one either way),
  items alone 931-944 / 937-948 (0 costs +0.5-1 %), orbs alone 1,171-1,193 / 1,202-1,217 (+2 %),
  grouped and TNT equal; `1000` (any run taken out): interleaved 1,532-1,656 (+5 %, the
  O(list) setup paid per one-entity run), items and orbs alone 929-933 and 1,164-1,177 (within
  1 % of the default). Default 8 kept; both other ways stay behind the switch for the
  quiet-machine scenes (`crowd`, `village`, `tnt`).
- Re-review fixes (02/11, quiet machine: no compilation during a measure; new scene
  `bench_l10order_rejoin`: 1,000 pigs with AI, 25 of them in a chunk that leaves the ticking
  chunks and comes back every other tick, so the mob list is re-sorted before every other walk;
  five passes of 400 ticks, two rounds per executable, executables alternated): the mob index is
  rebuilt at once after that re-sort instead of being marked stale (the walk's lookups and
  per-species queries then use it instead of scanning the list until the end of the walk; one
  rebuild per such tick either way, at the re-sort instead of after the mobs). Rebuilt
  7,311-7,488 µs per tick, marked stale 7,252-7,405, rebuilt again 7,250-7,462: equal within noise
  (pigs look few mobs up); kept, strictly less work where mobs look each other up. Lead not
  tried: re-indexing only the slots the re-sort moved (a stable sort can shift every slot after
  the first one that moved: O(list) as well). The base
  before all the review fixes read 7,664-8,090 µs earlier on the same scene: that gap is not
  attributed to these fixes (no work added to the walk), not claimed. The other fixes add no
  measurable work: the player's world part only moved (same calls, at its rank), the freeze
  test of a vehicle's passengers runs only when frozen, the plates also scan the server's item
  and orb lists during a series (empty unless something was born in it), a parked world is
  swept once per family after the player leaves (O(its entities), then a bit test per tick).

### `main.rs` cut into modules by concern (X-M01, 03/11/2026, pure move)

`apps/client/src/main.rs` (4,503 lines) became 549 lines plus eleven modules: `app_events`,
`app_meshing`, `app_frame`, `app_tick`, `app_camera`, `app_effects`, `app_menus`, `app_window`,
`startup`, `grid_world`, `app_entity_sets` (and `memstats` for the memory counter). Not one
statement changed: `tools/check_pure_move.py` compares the base file with the new ones item by
item (95 items identical, statement order included). No switch (nothing to switch), no
measurement taken: the cut is within one crate, so inlining and code generation do not depend
on the module of a function. Scenes to measure on a quiet machine: `plains12`, `crowd`, `title`,
2 alternating passes against the exe built before the cut; expected neutral (otherwise
`#[inline]` on the moved functions, noted here with the figures).
## C5-M02: borrowed block view (03/11, loaded machine)

The server's block view (`Server::view()`, formerly `v()`) cloned two `Arc`s on every call
(`Arc<Table>`, `Arc<VersionData>`: two atomic increments and two decrements), and `table()`
(formerly `tb()`) one; 1,237 call sites in `rc-sim`. The view is now
`View<'a> { table: &'a Table, data: &'a VersionData }` (`Copy`; a constant assertion fails to
compile if it stops being `Copy`), `table()` returns `&Table`. No switch: the old path cannot
coexist with the type (A/B = executable of `main` against the branch's).

- Callers: 53 borrow conflicts in 20 functions (a view held across a `&mut self` call or a
  random draw): the view is made again after the call. Three pure reads moved across a draw
  (vine tick, bamboo growth, frosted ice placement), none observable. Approach "hoist the view out
  of the random tick loop of `tick_chunk` and read the state flag directly": absorbed, the view
  in the loop is now two pointer copies and the same read.
- Micro-bench `bench_block_view` (`cargo test --release -p rc-sim bench_block_view -- --ignored
  --nocapture`, every state of 26.3 read 50 times, ns per view made and read; two passes):
  one thread, borrowed 4.41 / 1.11 ns, counted 10.28 / 7.98 ns (−5.9 to −6.9 ns per view); all
  threads sharing the counters (as the server and client threads share the version data),
  borrowed then counted, pass 2: 2 threads 1.12 / 53.7 ns, 4: 1.17 / 106, 8: 1.29 / 311, 16:
  1.50 / 636, 32 (every logical core): 2.86 / 1,319 (pass 1, counted: 28.9, 117, 316, 637,
  1,342 ns). GPU: not applicable (a pointer copy).
- Views made per server tick (`bench_ticks`, test counter): plains 3,849, forest 3,150,
  village 3,451, ocean 1,430, nether 6,167. Expected saving, uncontended: 23-27 µs per tick in
  plains (2-2.5 % of the mean tick), 8-10 µs in ocean (≈ 1.3 %), 37-43 µs in the Nether (≈ 2.5 %);
  more if another thread touches the same counters.
- `bench_ticks` A/B (executables of `main` 34e1be81 and of the branch, 4 passes each, both orders,
  then 4 more with `RAYON_NUM_THREADS=1`): inconclusive on this machine, the ranges overlap and
  the winner flips with the order (plains median 0.90-1.55 / 0.64-1.23 ms, nether 1.06-2.03 /
  1.14-2.13 ms; the Nether's spread follows the light time, code untouched here). The redstone site
  of `bench_ticks` cannot run on the current model world (no `perf:redstone` function). To settle
  on a quiet machine: `scenes.py` redstone, tnt, village, plains32, plains12, cave, forest (tick
  median and p99), 2 alternating passes.
- Proofs: whole `rc-sim` suite; `fingerprint_new_world` and `fingerprint_mechanisms` unchanged
  (both fail when a random tick is skipped in `tick_chunk`); bench world fingerprint
  2a5918d4… (740 files, model world d49ce0cb…) identical on `main` and the branch, it changes
  (89a3ba32…) under the same sabotage, but this model world has no recorded value yet (only its
  determinism is asserted). New `draw_order_tests` (11 scenarios, 64 seeds each: blocks left, value
  returned, next draw) with digests recorded on `main`: the world fingerprints miss an extra draw in
  the vine tick and the bamboo stage drawn every time, these digests catch both.
- Memory: none (the test executable is 0.5 MB smaller: no `Arc` clone and drop code at the sites).
## C7-M03: wgpu without OpenGL and WebGPU, criterion as a workspace dependency, duplicate crates (03/11, loaded machine)

Build-only change: no switch is possible, the comparison is two executables of the same commit
(`target\notes\before` / `after` of the mission worktree), built through the compilation queue
(trial profile: no LTO, 16 codegen units, rust-lld), other agents compiling.

- wgpu was built with its default features, among them `gles` (OpenGL backend: `glow`, EGL/WGL
  loaders, naga's GLSL output) and `webgpu` (compiles nothing on native targets), although the
  client only ever asks for Vulkan and DX12 (`gpu_backend.rs`, `order`). Now
  `default-features = false` with `std`, `parking_lot`, `dx12`, `vulkan`, `metal` (kept for a
  later macOS port) and `wgsl`; no other crate turns `gles` back on (`cargo tree -e features`:
  naga keeps `wgsl-in`, `hlsl-out`, `spv-out`, loses `glsl-out`).
- Proof: `gpu_backend::tests::only_the_tried_backends_are_compiled` reads the backends wgpu was
  compiled with (no GL, no browser WebGPU; Windows: exactly Vulkan + DX12). Sabotage (wgpu back
  to its default features): fails with `Backends(VULKAN | GL | DX12)`. Linker map of the old
  executable: 1,537 lines of `wgpu_hal::gles`, 325 of `glow`, 200 of `naga::back::glsl`; new one:
  0, 0, 0 (same patterns find 1,643 `wgpu_hal::dx12` lines and 1,130 `wgpu_hal::vulkan` lines).
  GPU tests were not using GL: they ask for a high-performance hardware adapter and skip
  without one, as before.

| Measure (trial profile, loaded machine) | Before | After | Difference |
|---|---|---|---|
| `.text` of `rustcraft.exe` | 36,019,014 B | 35,557,414 B | −461,600 B (−451 KiB, −1.3%) |
| `.rdata` / `.pdata` | 9,285,080 / 1,211,796 B | 9,141,960 / 1,193,820 B | −143,120 / −17,976 B |
| Executable file | 46,688,256 B | 46,062,592 B | −625,664 B (−611 KiB) |
| Packages in `Cargo.lock` | — | 9 removed | `gl_generator`, `glow`, `glutin_wgl_sys`, `khronos-egl`, `khronos_api`, `slotmap`, `termcolor`, `wgpu-core-deps-emscripten`, `xml-rs` |
| Crates in the client's tree (Windows, normal) | 260 | 255 | −5 |
| Compile time wgpu-hal → wgpu-core → wgpu (serial chain before rc-app) | 12.8 + 16.1 + 8.7 = 37.6 s | 7.2 + 9.1 + 4.4 = 20.7 s | −16.9 s |
| naga | 19.4 s | 20.6 s | noise (it lost only the GLSL writer) |
| OpenGL crates (glow, xml-rs, khronos, gl_generator, glutin_wgl_sys) | ≈ 4.0 s | 0 | −4.0 s |

To measure on a quiet machine: the full release (`RUSTCRAFT_FULL_RELEASE=1`: size of the final
executable, clean build time) and `scenes.py` title, 3 passes (startup, adapter
enumeration; expected neutral or better: the instance no longer loads the OpenGL backend).

- criterion: declared once in `[workspace.dependencies]`, `criterion.workspace = true` in the 6
  crates with benches (rc-data, rc-light, rc-mesh, rc-net, rc-proto-v777, rc-world). Same version
  and features as the 6 old lines, so cargo already built it once: maintenance gain only (one
  line to bump), no build time.
- Duplicate crates (`cargo tree -d`, Windows): bit-set 0.8/0.10 and bit-vec 0.8/0.9
  (fancy-regex 0.20 vs naga), getrandom 0.2/0.4 (ring 0.17, unicode_names2's build-time
  generator), hashbrown 0.16/0.17 (gpu-allocator under wgpu-hal), miniz_oxide 0.8/0.9 (png vs
  flate2), rand_core 0.6/0.10 (build-time generator), syn 2/3 (windows-implement,
  zerocopy-derive), windows-sys 0.52/0.61 (winit 0.30). No bump aligns any of them: every
  parent is at its latest stable release (fancy-regex 0.20.0, ring 0.17.14, png 0.18.1,
  unicode_names2 4.0.0, gpu-allocator through wgpu-hal 30.0.1; winit 0.31 is only a beta), and
  `cargo update --dry-run` only offers semver-compatible patches that remove none. Unchanged by
  this mission; to re-check at the next bump of each parent.
- Critical path for C7-M04 (same trial-profile build of the client, loaded machine, units
  partly served by the incremental cache): rc-worldgen 19.1 s → rc-sim 109.1 s → rc-app 43.6 s,
  (171.8 s summed, slightly overlapped by pipelining): the chain spans the whole 169.6 s wall
  time, rc-sim alone 64% of it; everything else runs beside it. Touching `rc-sim/src/lib.rs` without a change: rc-sim 6.1 s + rc-app 6.2 s. The full
  release profile (`RUSTCRAFT_FULL_RELEASE`) is not used by this series' agents: its timings
  are left to C7-M04 on a quiet machine.
## C2-M05: biome tint of moving blocks and main-thread world reads (03/11, loaded machine)

Hot spot: redstone scene, `zoom_corner` 2.10 % of the process on the main thread. Every pushed
block (piston block entity), falling block and transient block read the biome colours of its
block every frame, i.e. one full blur zoom (8 corners × 8 LCG steps), even for states with no
biome-tinted layer (stone, piston heads and bases, lily pad with its constant colour).
Switches (default on, `=0` old path, read once):
- `RUSTCRAFT_OPT_BLOCKTINTLAZY` (`EntOpt::block_tint_lazy`): the biome colours are read only
  when one of the drawn states (moved state, or head, or head + extended base) has a GRASS,
  FOLIAGE, DRY_FOLIAGE or WATER layer; otherwise a white placeholder that the colour lookup never
  reads for those states. Deterministic count: 1 → 0 zoom per untinted moving block per frame.
- `RUSTCRAFT_OPT_BIOMEZOOMMEMO`: `biome_full` (moving blocks, rain columns, particles, F3) reads
  the zoom from the mesher's per-thread memo (`rc_mesh::blocks::biome_zoom_cached`, now public;
  ≈ 196 KB once on the main thread, exact: key = seed + block). Micro-bench, 64 blocks re-read per
  frame, 20,000 frames, 2 alternating passes: 75.0 / 64.3 → 21.5 / 29.5 ns per read (≈ −65 %).
- `RUSTCRAFT_OPT_LIGHTPAIRONCE`: `light_pair` and `raw_brightness` look the column up once for
  both layers: 34.8 / 31.1 → 18.3 / 17.7 ns per block for the pair + brightness (≈ −45 %).
Combination: independent (laziness removes reads, the memo makes the remaining tinted ones
cheaper); all three on by default. Multi-core and GPU not applicable (a few dozen reads per
frame on the main thread, the result feeds the instance colours built on the CPU). Proofs:
`entity_render/tests_moving_block_tints.rs` (pushed, falling and transient grass block, oak
leaves, vine, leaf litter, water cauldron, sugar cane, stone, lily pad, sand, extended piston,
plus piston head and retracting base: same instances and matrices both ways, biome colour present
when tinted, no read when not; the four biome kinds covered), `environ/world_read_tests.rs`
(memo forward, backward and across two seeds, light reads with missing and holed layers and
absent columns; stored fingerprints), `BLOCKTINTLAZY` added to the entity snapshot of
`tests_c3m11.rs`. Sabotages seen failing: WATER left out of the laziness test, DRY_FOLIAGE left
out of the shared slot table, memo key without the seed, light layers swapped. (A memo key
without y is not caught: with y in the slot hash the stale hit practically never happens.)
To measure on the quiet machine: `scenes.py` redstone (frames/s, CPU ms per frame, p99; samply:
`zoom_corner` near 0), tnt and village (no effect expected), each switch at 0, and a 1600×900
redstone screenshot pair in one launch.

## C2-M08: section grids filled by rows, light bits in the property word, vector candidates, border rows (03/11, loaded machine)

Hot spot: `mesh_with` (cave 3.64 % self of the process, pool). Reading the code: the 18³ state and
property grids were filled one vertical stack per (x, z), so every write landed 324 cells (1.3 KB)
after the previous one; the section visibility walked the 4,096 states a second time for their
solid render flag; the candidate rows recomputed the surely hidden faces that the per-block loop
then computed again; a partial remesh decoded the whole neighbouring sections above and below
although only one row of each is read. Switches in `MeshJob::cull` (`rc_mesh::cull::CullPaths`,
default on, `=0` old path, read once):
- `RUSTCRAFT_OPT_MESHGRIDROWS` (`fill_rows`): interior copied one (y, z) row of 16 contiguous cells
  at a time, properties looked up in the same pass without the `OUTSIDE` test (identifiers are
  checked at reception; an out-of-table value reads as air either way); bottom and top layers
  copied 16 cells at a time from the rows of the sections below and above; the 68 ring stacks of
  the neighbouring columns unchanged (one function shared by both orders).
- `RUSTCRAFT_OPT_MESHPROPBITS` (`solid_from_props`): bits 29-31 of the property word (both rules)
  carry the light properties: solid render, closed for the smoothed light (solid render that
  dampens light, stored inverted so that word 0 — outside the columns — and an unknown state read
  as permeable, like the light table's default), dark shade. The visibility mask is gathered from
  the solid bit during the rows fill (in a pass over the grid in the stacks order) instead of
  `solid_bits(values)`; the path without the grid and the reference keep `solid_bits` as cross
  check. The grid with those bits is ready for the smoothed light (C2-M09 reads it).
- `RUSTCRAFT_OPT_MESHROWCANDSIMD` (`simd_candidates`): candidate rows computed on fixed 16-cell
  slices of the cell and its six neighbours, branch-free candidate test; AVX2 version chosen at
  run time (`is_x86_feature_detected`, the release stays portable), SSE2 otherwise. Assembly
  checked: 140 ymm instructions, mask built by `vpsllvd`, no call left (the `array::map` of the
  six neighbour slices stayed out of line, written out by hand). The surely hidden faces are kept
  per candidate (`Vec<u8>`) and read back by the per-block loop.
- `RUSTCRAFT_OPT_MESHEDGEROWS` (`border_rows_only`): in a partial remesh, a neighbour of a remeshed
  section that is itself outside the remesh decodes only its rows 0 and 15
  (`Container::unpack_range`, 512 values instead of 4,096); the border reads them (the old "empty
  neighbour → 0" rule kept for really empty sections); the light reads two rows away fall back
  to the column, as before for the sections that are not decoded.
Readability: the fill moved out of `mesh_with` (`fill_section_grids`, `fill_interior_rows`,
`fill_bottom_and_top_rows`, `fill_by_stacks`, `fill_ring_stack`), with `unfold_sections` and
`copy_reused_section` (`mesh_with` 410 → 334 lines); local names spelled out (`cell_props`,
`state_grid`, `candidates`, `cull_tables`, `light_view`, `group_faces_by_bucket`…).

Interleaved micro-bench `tests/cull_bench.rs`, 1,248 columns of the capture, best of 5, two passes,
loaded machine (CPU load 85-100 %), 1 thread, against the default without the four paths
(1,758.9 / 1,741.2 ms): rows fill alone −3.4 / −1.9 %; solid mask from the grid alone −2.5 /
−1.5 %; vector candidates alone −0.7 / −3.5 %; rows fill + vector −3.7 / −4.3 %; default without
the rows fill −4.1 / −4.8 %; default (all four) −5.7 / −3.8 %. All threads: ±20 % noise from one
pass to the next, not interpretable. Partial remesh of one section per column (best of 20):
border rows only −1.6 / −0.9 % on 1 thread (all threads ±1 %, noise). Every lead gains alone and
in combination on 1 thread: all four on. Multi-core: the work stays one section per pool task
(the pool already spreads the columns); GPU ruled out as for C2-M10 (sending the grids costs more
than the test). Memory: at most 4 KiB per pool thread for the kept faces of the candidates, and
2 KiB per pooled pair of border rows (a few per thread, 32 at most); the full boxes of the
neighbours are no longer taken in a partial remesh.

Proofs: `tests/cull_paths.rs` — 36 combinations per rule (the 16 of the grid paths with the four
paths off then on, and the default with each one off) × 3 settings against the stored
fingerprints and the reference version, partial remesh with the rows fill and the border rows
each off; `grid_fill_edge_cases_match_reference` (a column without neighbours, states outside the
table inside and on the borders of sections, blocks at the very bottom and top of the world, full
and partial, every combination); `property_light_bits_follow_the_light_table` (bits 29-31 equal to
the light table for every state, solid bit = the visibility's table, air without light bits);
`tests/partial.rs` `partial_remesh_reads_the_border_rows_of_its_neighbours` (covering blocks on the
neighbours' border rows, 24 columns × 2 rules × 3 paths); unit tests `lane_candidates_match_cell_by_cell`
(lanes and AVX2 against the cell-by-cell computation, candidates and surely hidden faces),
`solid_mask_of_props_matches_states`, `light_bits_follow_the_light_table`,
`candidate_bit_matches_candidate`, rc-world `unpack_range_matches_reference`. Sabotages seen
failing: solid bit inverted, border row shifted by one z, border rows read as air, a candidate
dropped in the vector version, `unpack_range` shifted by one cell.
To measure on the quiet machine: `scenes.py` cave, redstone, forest (meshing ms/s of the pool,
delay from placing a block to the image), 2 passes, each switch at 0; then the 1 thread of the
micro-bench again.
## C1-M08 (series 3): structure beard, only the elements that can contribute (03/11, loaded machine)

`structure_beard.rs`: the terrain adaptation around structures summed, for every point of the
chunk zone (16 × 16 columns, structure height + 48 blocks), the term of EVERY rigid piece and
junction of the chunk. Each element only reaches a small box (beard kernel offsets −12 to +11,
burial sphere of radius 6): outside it the term is exactly +0.0, no term is ever −0.0 and a point's
sum starts at +0.0, so skipping it leaves the sum bit for bit unchanged (test
`reach_is_exact_for_every_adaptation`: +0.0 everywhere outside, non-zero on every face).

Micro-bench `bench_beard` (`cargo test --release -p rc-worldgen --lib bench_beard -- --ignored
--nocapture`): 44 chunks of the test world's village (seed 20261011 around (−1408, 368), 43
elements per chunk), 32 chunks of an ancient city, stronghold, trail ruins and trial chambers
placed at chunk (10, 10); 1 thread, best of 5 to 9, 2 or 3 alternating passes per run, four runs,
LOADED machine (other agents compiling; the terrain step itself varied from 5.3 to 16 ms per
chunk between passes). Ranges below are over passes and runs, ms per chunk.

| Path (switch) | village | ancient city | stronghold | trail ruins | trial chambers |
|---|---|---|---|---|---|
| old, point by point | 1.08–1.57 | 0.49–0.91 | 1.37–2.82 | 0.45–0.87 | 4.21–4.31 |
| column lists alone (`COLUMN_LISTS`) | 0.77–1.95 | 0.38–0.63 | 0.25–0.62 | 0.17–0.54 | 2.32–4.32 |
| column buffer (`COLUMN_BUFFER`) | 0.15–0.22 | 0.095–0.22 | 0.033–0.061 | 0.033–0.058 | 0.34–0.54 |
| buffer + tables (`KERNEL_TABLES`), default | 0.066–0.10 | 0.079–0.18 | 0.023–0.061 | 0.015–0.027 | 0.18–0.22 |
| buffer + tables + lists | 0.069–0.12 | 0.078–0.19 | 0.021–0.054 | 0.016–0.030 | 0.19–0.26 |

- Default (buffer + tables) against old: village ×15, stronghold ×40 to ×60, trail ruins ×30,
  trial chambers ×20, ancient city ×6 (its box beards keep a computed term: the distance to the
  ground is bounded by the box height only). Terrain step of the village chunks (1 thread, beard
  included, last run): 5.65–6.73 ms with the old beard, 4.32–4.80 with the new one: the beard was
  about 20 % of the terrain of a village chunk. `Beard::new`: 0.0007 ms per chunk (not a hot spot).
- Column buffer: within a column, element after element over its own y range (≤ 24 blocks for
  a junction, a thin beard, a burial; box height + 22 for a box beard or an encapsulation), into a
  buffer of the column's zone that starts at +0.0, then added to the volume over the whole zone
  (the old path added +0.0 to every zone point, which turns a −0.0 density into +0.0). The y step
  of the filling volume is 1: other volumes keep the point-by-point path.
- Kernel tables (79 KB once per process): terms of junctions (24³), thin beards (12 × 24 × 12),
  burials (6 × 23 × 6) and encapsulations (12³), filled by calling the point-by-point terms on a
  one-block element at the origin, so with the very same operations; a column's y range is a run
  of a table (encapsulation: three runs, below, inside and above the box). Box beards: kernel row
  read once per column, logical shift in the inverse square root (bits of a positive value,
  `psrlq` in SSE2) and a `min(11)` that proves the kernel index in bounds; 2.34 → 1.0–1.4 ns per
  slot.
- Column lists (CSR of the elements per column, built once per chunk): alone, ×1.2 to ×5 against
  old; with the buffer, LOSES on the heaviest chunks (village 157–121 elements: 0.177/0.230 ms
  without, 0.216/0.264 with; trial chambers 304–233: 0.385/0.382 without, 0.428/0.423 with): the
  per-column bounding-box test of every element costs less than building the lists. Off by default
  (`RUSTCRAFT_OPT_BEARD_COLUMN_LISTS=1` enables it), for the quiet-machine scene bench.
- Rows in parallel (`par_chunks_mut` over z rows above 2¹⁶ column slots): one chunk at a time
  with 16 idle threads, trial chambers 0.207 → 0.132 ms; but in a batch (chunks already in
  parallel, the generation case) it costs: stronghold 0.052 → 0.108 ms, trail ruins 0.013 → 0.043
  at 16 threads, others within noise. After the two leads above, no structure chunk's beard holds
  the end of a batch (≤ 0.26 ms against a terrain of 4 ms and more). Off by default
  (`RUSTCRAFT_OPT_BEARD_PARALLEL=1` enables it). Thread counts 1, 2, 4, 8, 16, 32 measured (bench
  prints them all).
- GPU: ruled out (f32 square root and the f64 bit trick not guaranteed bit for bit in WGSL, the
  dedicated server has no GPU, and the remaining cost is about 0.1 ms per structure chunk).
- Equivalence: `beard_fingerprints_match_the_point_by_point_path` (stored FNV of the volume bits
  for each scene, taken with the code from before the mission: old path and environment paths),
  `beard_paths_match_bit_for_bit` (every combination of switches against the old path, chunk by
  chunk, rows forced onto the threads), `beard_paths_match_on_synthetic_bounds` (600 random
  beards of every adaptation across the reach bounds and the chunk edges, the zone cutting the
  reaches, plus a stepped volume). Each fails on these sabotages: direct add into the volume,
  first junction skipped, junction x margin 11 instead of 12, thin beard y bound −11 instead of −12,
  junction y bound +10 instead of +11, burial reach 4 instead of 5, column without element not
  added, encapsulation run below the box off by one, junctions summed before the rigid pieces.
- Memory: the tables (79 KB, once), per chunk the reached elements (≈ 40 B each) and one column
  buffer (≤ 1.5 KB); lists, when on, 4 B per (column, element). To re-measure on a quiet machine:
  scenes `generation`, `generation-1thread`, `fresh-village-rd16`.
## C4-M09: player list (Tab), sidebar, revisions of the player list (03/11, loaded machine)

Micro-benches of the client, real GPU and fonts of the installed game, LOADED machine (other
agents compiling: absolute figures vary by ×1.5 between launches, the ratios hold). The scenes
`tab-80`, `hud-busy` and `team-crowd` remain to be run on a quiet machine (`tools/bench/scenes.py`,
two alternating passes, reference executable), with each switch at 0 and at 1.

**Player list held open** (`tablist_bench`: 80 listed players, three teams with prefix, suffix and
color, own display names, spectators, header and footer on several lines, heads; the old code,
frozen in `tablist/tests/reference.rs`, and each switch drawn in turn on every frame so that a
drifting load weighs on all of them alike; 2,000 frames, two launches):

| Path | text scores, µs/frame | hearts, µs/frame | bytes allocated per frame (text / hearts) |
|---|---|---|---|
| old code | 106.2 / 157.4 | 142.4 / 201.6 | 297,597 / 271,998 |
| rows rebuilt every frame (`TAB_LIST_CACHE=0`, `TAB_SORT_KEY=0`) | 100.4 / 145.9 | 143.4 / 190.4 | 298,365 / 272,766 |
| sort keys alone (`TAB_LIST_CACHE=0`) | 95.4 / 137.8 | 137.9 / 185.0 | 300,413 / 274,814 |
| kept rows alone (`TAB_SORT_KEY=0`) | 31.4 / 58.0 | 79.0 / 114.3 | 0 / 0 |
| both (default) | 31.0 / 57.7 | 78.9 / 110.8 | 0 / 0 |

- Kept rows (`RUSTCRAFT_OPT_TAB_LIST_CACHE`, default 1): sort, names (display name or name
  formatted by the team), list scores flattened and measured, header and footer wrapped, built
  once and kept while their inputs hold: revision of the player list (every write except the
  latency alone), list revision of the scoreboard (score or reset of the objective in the list
  slot, that slot, objectives, teams), text context of the interface (language, fonts, bound
  keys), screen width, and header and footer compared by equality on every frame (the application
  assigns them directly). Latencies refreshed on their own revision; hearts, heads, presence in
  the world and placement stay per frame. The cleanup of the hearts and faces of departed players
  runs only after a change of the list or the arrival of a skin (it removed nothing in between).
  −70 % / −63 % (text), −45 % (hearts); allocations per frame 0 instead of ≈ 300 KB.
- Sort keys (`RUSTCRAFT_OPT_TAB_SORT_KEY`, default 1): team of each player looked up once
  instead of at each comparison that reaches it: −5 % of each rebuild; with kept rows, only on a
  change.
- A rebuild costs what the old frame cost (65 to 90 µs here): the frame of a change is no slower
  than before (p99 not worse), the others are cheaper. Memory: the kept rows (≈ 150 KB for 80
  players) live only while the list is open.
- Remaining with hearts (≈ 48-56 µs more than text): about 1,600 heart sprites looked up by name
  per frame, the subject of C4-M39 (resolved sprite ids extended to the hearts). Remaining with
  text (31 µs): 80 slots of fills, heads, two texts and a latency icon placed through the
  interface (text cache hashing). The face sprite names `dyn/tab_face/<i>` are built once
  (80 strings formatted per frame before).
- Leads not taken: one row rebuilt alone when a single score changes (a server updating a health
  objective of the list every tick rebuilds 20 times per second); multi-core and GPU not
  applicable (calls into the interface on the main thread, tens of µs).
- Proof `tab_list_paths_draw_identical_layers`: 200 frames, text then hearts, changes at fixed
  frames (one list score, a score of another objective, latencies alone, skin of a listed player
  then of an unknown one, language, narrower screen, team update and move, display name and
  spectator, 81st player, departure, unlisting, header, footer removed, bound key, fonts, heads
  hidden, list hidden for one frame, list slot emptied then filled, list order); layers equal to
  the byte to the old code for each switch alone and together, rebuilds counted (15 to 40). Ten
  sabotages fail at their frame (latency not refreshed, language, fonts or keys out of the
  context, list score or team not moving the list revision, header not compared, width out of the
  key, removal not moving the player revision, sort keys without the team).

**Sidebar styled lines kept** (`RUSTCRAFT_OPT_SIDEBAR_STYLED_CACHE`, default 1, acts with
`RUSTCRAFT_OPT_SIDEBARCACHE=1`): the 15 kept rows were flattened, styled and measured on every
frame (score width measured twice); title, lines, widths and panel width are now kept per
(rows, text context). `bench_sidebar_draw` (two passes): 14.25 / 8.80 µs per frame styled
every frame, 3.89 / 3.83 µs kept; 35,452 bytes allocated per frame, 0 kept. Proof
`sidebar_styled_cache_draws_identical_layers` (120 frames, score changes, language, fonts, other
local player; layers equal to the byte to the frozen old draw; context ignored: fails at frame 50).

**Borrowed score key** (`RUSTCRAFT_OPT_SCORE_KEY_BORROW`, default 1): list score per listed
player and "below name" line per entity and per frame looked up with a borrowed key hashed like
the owned `(String, String)` one (same table, same walk order, the sidebar ties unchanged).
`bench_score_lookup` (200 holders, two passes): 23.3 / 39.7 ns owned, 8.7 / 17.3 ns borrowed;
2 allocations (9 bytes) per lookup removed. Proof `borrowed_score_key_finds_the_same_scores`
(same hash with the table's hasher, same score found, 5,000 random events; swapped names: fails).
Same pattern elsewhere: `rc-sim` statistics (C6-M28), and two cold lookups (resource cache list
at startup, variant registry received) left as they are.

**Revisions and generations**: `players::Revision` (stamps from one counter for the process: a
list rebuilt after a disconnection never reuses one), revision and latency revision of the
player list, list revision of the scoreboard (proof `list_revision_moves_with_what_the_list_shows`:
whenever what the list shows changes over 20,000 random events, the revision moved; three
sabotages fail). `rc_text::Lang::generation` (unique per load), `rc_text::keybinds_generation`
(moves only when the keys really change), font identity of the interface unique in the process
(a new interface after a resource reload never reuses one), `Gui::text_context()`: the
generations planned by C4-M03, done here because the kept rows need them (proof
`language_and_key_generations`, two sabotages fail).

Axes not concerned: integrated server, network, disk, chunk loading, startup (client interface
code); GPU and VRAM: the same quads are sent (layers equal to the byte); energy: less CPU per
frame while the list or the sidebar is shown.
## C1-M09: `jmath` sine and cosine, exact product and series (03/11, loaded machine)

Micro-bench `rc-worldgen --test noise_opt sincos_paths_bench -- --ignored` (trial profile, machine
at 100% load: other agents compiling; best of 5), 10⁶ angles in ±10. Default = new, `=0` = old.
Switches: `RUSTCRAFT_OPT_TWO_PROD_FMA` (bodies compiled for the FMA instruction, chosen by runtime
detection), `RUSTCRAFT_OPT_TWO_PROD_SPLIT` (Veltkamp split and Dekker product, the fallback without
FMA; both at 0 = old library `fma` call), `RUSTCRAFT_OPT_SINCOS_ONE_SERIES` (sin or cos alone runs
only the Taylor series it returns), `RUSTCRAFT_OPT_SINCOS_INTERLEAVED` (both series advance in the
same loop, their two chains of dependent divisions overlap). Every path gives the same bits by
construction (the product error is exact, hence unique; each series does the same operations in
the same order): frozen digest recorded with the old code (all 32 switch combinations), 10⁷ random
angles in ±10⁵ plus edge angles against the old path, exhaustive `check_sincos`, `decor_ref` 0
differences.

| Lead (switch) | sin + cos separately, ms | `sincos`, ms | Note |
|---|---|---|---|
| old (library `fma`, both series one after the other) | 700 | 351 | — |
| split alone (`TWO_PROD_SPLIT`, FMA off) | 946 | 482 | +35%: here the library `fma` already runs the instruction (call only); the split is for processors without FMA, where the library emulates it (not measurable on this machine) |
| FMA compiled alone (`TWO_PROD_FMA`) | 633 | 328 | −10% / −7% |
| one series alone (`SINCOS_ONE_SERIES`) | 367 | 359 | −48% on sin, cos alone (they computed both series and kept one) |
| interleaved alone (`SINCOS_INTERLEAVED`) | 435 | 226 | −36% on `sincos` |
| one series + interleaved | 360 | 217 | — |
| split + both series switches (processor without FMA) | 500 | 306 | −29% / −13% against old |
| FMA + both series switches (default here) | 350 | 211 | −50% / −40% |

Threads (`sincos`, 4 × 10⁶ angles in ±10⁵, old → default, ms): 1 thread 1,466 → 855, 2: 701 → 462,
4: 369 → 240, 8: 186 → 129, 16: 101 → 76, 32: 63 → 61 (the 32-thread run shares the cores with the
compiling agents). Decoration (`decor_ref decoration_profile`, 61 cases, 2 alternating passes):
52.8 / 52.9 → 51.1 / 51.5 ms (−3%; `ore_iron_upper` 3.09 → 2.27 ms). Spawn search: about 850
sin/cos pairs, under 1 ms against the climate sampling, nothing to gain there. GPU: not relevant
(f64 not guaranteed in WGSL, and the calls are scattered single angles). Memory: none.
Wrong split constant (2²⁷ instead of 2²⁷ + 1) is not detected by the tests: it only alters the
last bit of an error term near 2⁻¹⁰⁶, below the final rounding; dropping the error term is detected.
Decision: all at 1 by default until the quiet-machine generation scene bench.
Still to do outside `jmath.rs`: the separate `sin(a)`, `cos(a)` pairs of `chunkgen.rs` (spawn search),
`structure.rs` and `tree.rs` can call `jmath::sincos` (one reduction, interleaved series: 217
against 360 ms per 10⁶).
## C5-M39: CFB8 encryption register, frames compressed in place, shareable frame (03/11, loaded machine)

Micro-benches (ignored tests, variants alternated inside each trial, minimum kept):
`cargo test --release -p rc-net cfb8_encrypt_bench -- --ignored --nocapture` and
`frame_encode_bench`. Scenes to measure on a quiet machine: `lan-4-spread`, `lan-2`
(CPU of the LAN writer threads), and the client on the official server online.

- Encryption (sending), 1 core, MiB/s for 16 B / 300 B / 8 KiB / 1 MiB packets: register shifted in
  memory (old) 49.3 / 49.7 / 49.0 / 49.7; register kept in an SSE2 vector register 55.7 / 56.7 /
  56.0 / 56.2 (+13 %, ADOPTED, `RUSTCRAFT_OPT_CFB8_VECTOR_REGISTER=0` for the old path); sliding
  view over a 16 + 256-byte window 46.8 / 48.0 / 47.6 / 47.6 (−3 to −5 %, kept OFF behind
  `RUSTCRAFT_OPT_CFB8_SLIDING_WINDOW=1`); bench-only `u128` register 50.9-51.6 (+3 %) and register
  rebuilt as an array 41.6-42.5 (−15 %). Cause: every in-memory path builds the AES input from
  several small writes then reads 16 bytes, which the processor cannot forward, on every byte of a
  chain that is sequential anyway; the vector register writes it with one 16-byte store. All
  cores (one 1 MiB stream per thread, i.e. one writer thread per LAN guest), total MiB/s, old ->
  vector: 1 t 47.4 -> 53.1, 2 t 94.5 -> 107.4, 4 t 186 -> 211, 8 t 368 -> 416, 16 t 679 -> 735,
  32 t 732 -> 840. GPU not applicable (byte-sequential chain of a few KB per tick). SSE2 is part of
  the x86-64 baseline: no runtime detection needed; other architectures keep the old path.
- Frame (unencrypted part), MiB/s of packet bytes, through `scratch` (old) -> written straight
  into the output (`RUSTCRAFT_OPT_FRAME_COMPRESS_IN_PLACE=0` for the old path): 40 B below the
  threshold 4,149 -> 6,432 (+55 %; 4,055 -> 6,381 on a second run); 2 KiB compressed 307 -> 307;
  40 KiB chunk-like 798 -> 799 (libdeflate dominates; a compressed content under 16 KiB still moves
  back by one byte for its 2-byte prefix). RAM: the encoder no longer keeps a scratch buffer as
  large as the largest packet ever sent (up to 8 MiB per connection) on the default path.
- `FrameEncoder::frame_compressed` (unencrypted frame, depends only on the threshold) and
  `FrameEncoder::encrypt` (per connection): base for compressing a LAN packet once for every guest
  (C5-M41/M42). No gain by itself.
- Not done: interleaving several AES streams on one core, gated on lan-4 showing encryption as
  the bottleneck of the writer threads (to measure first on the quiet machine).
## C4-M28: client block entities, state facts kept on the entity, ticking columns, block-change early exit (03/11, loaded machine)

`apps/client/src/block_entities.rs`. Micro-benches `bench_block_entity_tick` and
`bench_on_block_change` (`cargo test --release -p rc-app block_entities::switch_equivalence_tests::bench
-- --ignored --nocapture --test-threads=1`), two passes each, loaded machine (other agents
compiling): figures to re-take on a quiet machine with the scenes `plains32`, `redstone`, `village`
(expected gain under the scene noise: the audit put the whole block entity tick at 0.13 % of
`plains32`).

- `RUSTCRAFT_OPT_BLOCK_ENTITY_PROPS` (default on): the vault, trial spawner, campfire and potent
  sulfur ticks read `lit`, `signal_fire`, `facing`, `ominous`, the trial spawner state and the wet
  sulfur from the entity's properties (read once per state change, `Props` stays 24 bytes) instead
  of fetching the world state and collecting its properties into a `Vec` every tick. Mix of 256
  signs, 64 lit campfires, 16 active trial spawners: 6.0-6.3 µs per tick → 2.3 µs (full walk),
  4.6 → 1.06 µs (ticking columns); one allocation per vault and trial spawner per tick removed.
- `RUSTCRAFT_OPT_BLOCK_ENTITY_TICKER_LIST` (default on): columns without a ticking entity (signs,
  banners, pots, shelves, lecterns, statues…) are skipped (column list gathered again when
  `version` moves); moving pistons step from their own list, in creation order (the game's
  ticker-list order: fidelity gain for the order of the pushes, `pistons_push_in_creation_order`),
  `moving` = live entries of that list (exact). Mix above: 6.0 → 4.6 µs alone, 2.3 → 1.06 µs with
  the props; 512 chests (all ticking): 2.9 / 2.9 µs (no loss).
- `RUSTCRAFT_OPT_BLOCK_ENTITY_TICKER_ORDER=1` (default OFF, kept for the scenes): every ticking
  entity steps from a list in insertion order (the game's order for everything), one lookup in the
  two maps per entity: mix 2.2-2.7 µs (props on), 512 chests 9.0-9.8 µs against 2.9 µs: loses
  wherever most entities tick.
- `RUSTCRAFT_OPT_BLOCK_ENTITY_CHANGE_EARLY_EXIT` (default on): a block change whose new state
  carries no entity leaves at once (removal only if the block changes); an entity kept is updated
  with one property read, otherwise one is created (one read). 10^6 changes (70 % wire power,
  20 % stone/air, 10 % chests and signs): 38-45 ns → 18-25 ns per change.
- `RUSTCRAFT_OPT_CONDUIT_WATER_TABLE` (default on): water test of the conduit frame by a table per
  fluid instead of a substring search of the fluid name (27 cells every 40 ticks per conduit; not
  measured alone, negligible).
- Proofs: `switch_equivalence_tests.rs` (scripted world with every kind, 400 ticks, states changed
  through the block change, particles/sounds/steps bit for bit), random block changes against the
  eager path; 11 sabotages each caught by its test. Multi-core and GPU: not applicable (a few µs of
  serial work per tick).
## C4-M32: world list read in the background, backup and deletion side by side (03/11, loaded machine)

Menus only (nothing in a world loop). Micro-benches on a LOADED machine (other agents compiling),
trial profile; the quiet-machine scene bench (`world-list`, image max at the opening of
Singleplayer and at a cancelled return from Edit, `RUSTCRAFT_PERFLOG`) is still to do. Every lead
stays behind its switch (`=0`: old path).

- Reading of the list (`world_list::scan`, the game reads it in the background with one task per
  world). Micro-bench `world_list::tests::bench_world_scan` (worlds made of the test world's
  `level.dat`, a 64 × 64 icon and a `session.lock`, best of 5, 2 passes, warm cache only: emptying
  the system file cache needs administrator rights): one by one 0.23-0.24 ms for 1 world,
  4.6-5.3 ms for 20, 24-26 ms for 100, 30-50 ms for 200 (about 0.24 ms per world: icon decode,
  `level.dat` inflate, lock test, link checks). That was a frozen frame on the main thread at
  every opening and every return. Now:
  - `RUSTCRAFT_OPT_WORLD_LIST_ASYNC_SCAN` (on): read on a thread, the main thread pays 48-64 µs
    (thread start) whatever the count; "Loading worlds" row until the first reading (like the
    game), current list kept on a re-reading.
  - `RUSTCRAFT_OPT_WORLD_LIST_PARALLEL_READ` (on): one task per world on the shared pool; 20
    worlds 2.7-3.5 ms on 2 threads, 1.6-1.8 ms on 16; 100 worlds 5.9-6.6 ms on 16-32 threads; 200
    worlds 9.6-12.0 ms on 16-32 threads (÷3 to ÷5); 1 world: equal within noise (0.18-0.33 ms).
    Time until the list appears, not main-thread time.
  - `RUSTCRAFT_OPT_WORLD_LIST_NO_RELOAD_ON_CANCEL` (on): no reading back from the symbolic link
    notice or the re-creation alert (the game shows the same list again), nor when the backup
    question or the recovery screen gives way to the next step. Returns the game reads again
    (edit closed, deletion and its cancel, creation cancelled, opening abandoned) still read
    again, now in the background. Cache by modification date: ruled out (copied folders keep
    their date).
  - `RUSTCRAFT_OPT_WORLD_ICON_HEADER_FIRST` (on): the 24-byte PNG header read first, the rest only
    for 64 × 64; saves reading a wrong-size icon whole (any size up to the file), same open count
    otherwise. Not measured separately (inside the scan figures).
  - `RUSTCRAFT_OPT_QUICK_PLAY_LIGHT_SCAN` (on): quick play without a world name takes the last
    played world from `level.dat` alone, in parallel (no icon decode, no lock test).
- Row texts (`RUSTCRAFT_OPT_WORLD_LIST_ROW_TEXT_CACHE`, on): the name, "folder (date)" and
  information lines and their cut versions are built once per reading, like the game which builds
  them with the row, instead of every frame (the date read the time zone from the system for each
  row each frame). Micro-bench `bench_row_text` (15 visible rows, fixed-width metrics standing for
  the font, 2 passes): 74.4-74.8 µs per frame rebuilt, 0.07-0.11 µs kept.
- `level.dat` summary read (`storage::read_level_summary`, fidelity: the game skips
  `Data.Player` and `Data.WorldGenSettings`; 26.3 writes neither, so only older worlds read
  less): micro-bench `bench_level_summary_read` (player with 36 items and 800 recipes, best of
  200, 2 passes): whole read 68-71 µs, summary read 41-51 µs.
- Backup copy (`storage::backup`, `RUSTCRAFT_OPT_PARALLEL_BACKUP_ZIP`, on): every file
  compressed on its own thread into an archive of one entry, then copied raw in the walk order:
  the zip is byte for byte the old path's (test `backup_side_by_side_writes_the_old_zip`). Feeding
  the deflater in 8 KiB pieces (a stream read, to bound the memory) changes its output (one byte
  more on a 1 MiB file): ruled out, files read whole like the old path, at most 64 MiB of input
  waiting (`BACKUP_BYTES_IN_FLIGHT`, the writer's entry always passes). Micro-bench `bench_backup`
  (test world, 71 MB, 216 files, zip 8.1 MB, 2 passes): old path 150 / 232 ms, 1 thread 136 /
  207, 2 threads 80 / 166, 4 threads 53 / 142, 8 threads 38 / 106, 16 threads 39 / 84, 32 threads
  47 / 205 ms (÷2 to ÷4; 32 threads hit by the loaded machine). Memory: up to about twice the
  budget (128 MiB) instead of the largest file.
- Deletion (`storage::delete`, `RUSTCRAFT_OPT_PARALLEL_WORLD_DELETE`, on): the tree listed
  without entering links, its files removed on all threads, then the folders deepest first; the
  game deletes on its main thread too (frozen frame kept, shorter). Micro-bench `bench_delete`
  (640 files of 64 KiB in 10 folders, 2 passes): 1 thread 70 / 124 ms, 2 threads 42 / 77, 4
  threads 56 / 36, 8 threads 200 / 99, 16 threads 63 / 68, 32 threads 218 / 25 ms: too noisy here
  to settle; to re-measure on a quiet machine (and with Defender scanning the folder).
- Optimize World: already one region file per thread since G4-F17; now behind
  `RUSTCRAFT_OPT_PARALLEL_WORLD_OPTIMIZE` (`0`: one thread) with an equivalence test (1 against
  2, 4, 16 threads: same chunks read back, same counters and progress). The game works one
  dimension after the other and so do we (the progress bars fill in that order): dimensions not
  run together. `bench_optimize` this round: 1 thread 618-818 ms, 8 threads 97-222 ms.
- Leads not taken: icon slots by world instead of by row (one 16 KiB upload per row entering the
  view on a scroll, 32 slots for at most about 20 visible rows: nothing measurable); GPU (zlib,
  NBT, PNG, file system: not applicable).
- Fidelity fixes found on the way: worlds with only `level.dat_old` listed (unreadable, dated
  by that file); folder names compared in UTF-16 units; backup entries dated with the local time
  of the copy (were 1980-01-01).
- Differences with the game noted, not fixed here: a `level.dat` without `version` 19133/19132
  is unreadable for the game (shown normally here); a missing `LastPlayed` is 0 for the game
  (date of 1970 shown), -1 here (no date); `Version` present without `Name` gives the current
  version name in the game, "unknown" here; a failed access when opening Edit makes the game
  re-read the list (toast only here); the game's backup walks the folder in its listing order
  (entry order differs here, same content).
## C4-M37: invisible blocks of a structure block's save area, one GPU instance per cell (03/11, loaded machine)

Structure block in save mode with "show air": a small cube outline on every air, structure void,
barrier and light cell of the area (up to 48³ = 110,592 cells). Before: every frame, a state read
and four block-name comparisons per cell, then twelve debug segments per marked cell (near-plane
cut on the CPU) and their upload. After: one 16-byte instance per marked cell (absolute block,
kind) that `tech_cells.wgsl` widens into the twelve edges with the debug segments' own function
(same corners, colours, 2.5 px, cut at 0.1 block in front of the eye); the instances stay on the
GPU while the areas and the stamps of the columns they cover are unchanged. Micro-bench
`tech_blocks::cells_tests::bench_show_air_frames` (`cargo test --release -p rc-app bench_show_air
-- --ignored --nocapture`, RTX 5080 Vulkan, 48³ area of air seen from outside, median of 30
frames; GPU = submit and wait of the pass minus an empty pass, 512 × 512):

| Variant (switches) | CPU per frame (boxes + upload) | Sent per frame | GPU pass |
|---|---|---|---|
| Old: segments, names (`..._CELL_INSTANCES=0`, `..._MARKER_TABLE=0`) | 65.7 ms | 42.47 MB | 1.13 ms |
| Segments, byte table (`..._CELL_INSTANCES=0`) | 64.2 ms | 42.47 MB | 1.26 ms |
| Instances, walk every frame (`..._CELL_CACHE=0`) | 0.576 ms | 1.77 MB | 0.587 ms |
| Instances, cache (default) | 0.020 ms | 96 B | 0.586 ms |
| Instances, cache, parallel walk (`..._PARALLEL_WALK=1`) | 0.021 ms | 96 B | 0.540 ms |

- Memory: the old segment buffer grew to 64 MB of VRAM (power of two above 42.5 MB) and the
  segment lists kept 42 MB of RAM capacity after a show-air session; now a 2 MB instance buffer and
  1.77 MB of cells. Same vertex count (72 per cell either way); the GPU gain comes from 16 bytes
  read per cell instead of 384.
- Rebuild after a change in a covered column (block, light, biome or column packet), walk alone
  (`bench_show_air_walk`, random area): names 2.35 ms, byte table 1.40 ms on one thread; parallel
  walk 1.46 / 0.84 / 0.45 / 0.47 / 0.47 / 0.48 ms on 1 / 2 / 4 / 8 / 16 / 32 threads (no gain past
  4: small work). All-air area: 0.58 ms including the 1.77 MB upload. The parallel walk stays off
  by default: the walk only runs after a change, and the pool is the meshing one, whose queued
  jobs the main thread would wait behind (risk on p99 during chunk loading); to re-measure on a
  quiet machine with chunks loading.
- Equivalence: `cell_list_matches_the_old_outline_list` (list (cell, colour, growth) equal to the
  old algorithm, three areas, table and parallel on and off), `cached_cells_follow_100_changes`
  (blocks set, covered column removed and put back, far column replaced, frames without change:
  list equal to a fresh rebuild every frame, rebuild exactly when a covered column changed),
  `marker_table_matches_the_names` (every state), `instanced_cells_draw_the_segment_image_on_vulkan`
  and `..._on_dx12`: five views (outside, inside, eye inside a cube, edges behind the eye, eye 0.04
  block beside an edge crossing the cut plane), 0 pixel different on both backends (93 k to 455 k
  segments dropped behind the eye depending on the view). Sabotages seen: an edge corner changed
  in the shader (29,141 px), stamps left out of the key, light missing from the table, parallel
  slices in reverse order, cut plane at 0.11 (423 px on the edge view).
- Leads not taken: an 8-byte instance (offset in the area + area index): nothing gained per frame,
  the buffer is only rewritten after a change; incremental rebuild per column (re-walk only the
  column whose stamp changed, about a ninth of the area): not done, the rebuild is already an
  event of 0.6 to 1.4 ms; walk off the main thread: the markers would lag a few frames behind a
  block change while meshing jobs are queued. Error texts of the test instances cached per
  language: needs a language-change signal from the interface (about 1 µs per marker per frame,
  only while a failed test is shown). Raw block-entity data kept only for F3+I: not possible
  without changing what F3+I+Shift copies when operator rights are granted later.
- Also: the `test_instance` settings shared by the boxes and the beams (`Arc`, cloned twice
  before), the interactions no longer copy the 1,700 names of the sound registry (read from the
  version data when a patched `consumable` is decoded; same copy left in `commands.rs` for the
  command data).
- To re-measure on a quiet machine: scene `structure-show-air` of `tools/bench/scenes.py` (to add:
  48³ save area with show air, camera outside then inside), CPU of the boxes and upload, bytes
  sent, GPU time of the segment pass, p99 of the frame.
## C3-M31: size of the server → client events (03/11, loaded machine)

- Every `rc_client::Event` takes `size_of::<Event>()` bytes in the crossbeam channel of the
  singleplayer memory link (integrated server thread → main thread), in the network thread's
  channel and in bundles (`Vec<Event>`). Measured before: 256 bytes, set by the rare variants
  (resource pack push with its prompt ~248, `Misc` 200 through the server data MOTD, text
  components 176 in chat, action bar, kick, death, open screen, test block status; scores 176;
  particles 152; waypoints 96), while the frequent ones are small (entity packet 72, block update
  16, section update 36, sound 88).
- Change: the rare variants above 88 bytes are boxed (`Box<Component>` in chat, action bar,
  kick, death, open screen, pack push prompt, test block status; `Box<ScoreEvent>`,
  `Box<LevelParticles>`, `Box<Waypoint>`; the MOTD inside `Misc::ServerData`). `Event` is now
  96 bytes (the sound packet, kept inline, plus the tag); `Misc` at most 48. Tests
  `event_stays_small` / `frequent_payloads_stay_inline` assert the sizes (a sabotage variant of
  120 bytes makes `Event` 128 bytes and the test fail). No runtime switch: a type layout; the
  A/B is two executables (main against this branch).
- Micro-bench `channel_slot_size_bench` (ignored test in `rc-client`, the same entity move event
  padded to the old slot sizes, 2 M events, loaded machine): producer thread → consumer like
  the memory link, 96 B 40.0-42.8 ns per event, 160 B (particles left inline) 55.3-61.5 ns,
  256 B (before) 70.0-79.8 ns: about −45 % per event. One thread (all sent, then drained: the
  channel's peak memory) 98-136 / 150-172 / 209-224 ns; peak channel memory −62 % (slot size).
  Boxing a particle event costs one allocation and one free: 44.5 ns, against 15-20 ns saved on
  every event by the 96 B slot over the 160 B one: particles boxed (they are a small share of
  the server's events: commands, hits, sweeps, vibrations).
- Not done (outside this mission's files, noted for later): `rc_text::Component` is 176 bytes
  inline everywhere (chat lines, item names); shrinking it is the root fix. `SoundPacket` (88)
  and `Inventory` (88) set the 96 B floor: merging the sound's position and entity options into
  one source enum would bring `Event` to 80 bytes.
- To measure on the quiet machine: `crowd-1000`, `tnt` (event send and receive time,
  `perf_log::received`, channel RAM at peak).

## C4-M10: name tags and rare entity subsets kept incrementally (03/11, loaded machine)

Every frame walked the whole entity table five to seven times for subsets of a handful of
entities (name tags, glowing outlines, ghosts, guardians twice and once more for their beams,
fishing bobbers), collected three new player sets, and the name tag rules formatted two strings
per living mob as soon as a team existed; every tick walked every entity twice more for the
monster effects and rebuilt a set of N + 1 ids for the water splashes. Micro-benches of the
client (`bench_entity_sets`, `bench_splash_state`, ignored tests), 1,000 cows, 20 players with
a health line below the name, 10 always-visible names, one cow in ten glowing and one in twenty
invisible; LOADED machine (other agents compiling), one core (main-thread work of a few µs:
spreading it over threads costs more than the work; no GPU form).

| Lead (switch, default 1) | old | new | allocated per frame (old → new) |
|---|---|---|---|
| name tag pick, early rejection (`NAME_TAG_EARLY_REJECT`) | 7.9-8.1 µs | 3.0-3.1 µs | 2,448 → 1,792 B |
| … with every holder in a team (server case) | 91.7-93.7 µs | 4.6 µs | 2,448 → 1,792 B |
| … from the kept candidates (`ENTITY_SUBSETS`) | 7.9-8.1 µs | 1.1 µs (2.5 with teams) | 1,792 B |
| frame of tags, layouts kept (pick, anchors, light, keys) | 11.3 µs (97.3 teams) | 1.5 µs (2.9 teams) | 14,091 → 1,792 B |
| … "below name" keys rebuilt (`BELOW_NAME_KEY_CACHE=0`) | | 3.5 µs (4.9 teams) | 12,115 B |
| glowing outlines (`ENTITY_SUBSETS`) | 3.6-3.7 µs (10.5 teams) | 1.1-1.4 µs (8.3 teams) | 8,412 → 0 B |
| ghosts (`ENTITY_SUBSETS`) | 2.0-2.1 µs | 0.4 µs | 2,188 → 0 B |
| player sets (`PLAYER_SET_REUSE`) | 0.054 µs | 0.004 µs | 0 → 0 B (sets refilled in place) |
| 1,000 holder names (`HOLDER_NAME_ON_STACK`) | 173-201 µs | 31-32 µs | 96,000 → 0 B |
| splash checks per tick (`WET_FLAG_IN_ENTITY`) | 22.6-24.6 µs | 13.0-17.9 µs | 10,256 → 0 B |

- Early rejection (`RUSTCRAFT_OPT_NAME_TAG_EARLY_REJECT`; the `NAME_TAG_FILTER` lead of C3-M22 is
  the same): every tag rule needs a player, the aimed entity or an always-visible name (data 3),
  so the others leave before the render position, the frame item and the team lookups; the name
  is tested before the team rules (they only hide); frame item name, profile name and local team
  looked up once; the "below name" distance is the tag's own instead of a search of the list
  (O(k²): 40,000 comparisons per frame for the 200 named cows of `name-tags`).
- Holder names (`RUSTCRAFT_OPT_HOLDER_NAME_ON_STACK`): the UUID of a non-player holder written in a
  fixed 36-byte buffer, a player's profile name borrowed; the ghost set no longer copies team
  names. Applies to the push rule of the player physics too (`player_env.rs`).
- Kept subsets (`RUSTCRAFT_OPT_ENTITY_SUBSETS`; `ENTITY_FLAG_INDEXES` of C3-M22): glowing (0x40),
  invisible (0x20) and name-tag candidates (players, data 3) noted by `Entities::apply` on spawn and
  on data 0 or 3. They are supersets: entities are also removed elsewhere than by `apply` (picked
  up items, lightning), so every reader checks the entity again and each tick drops the stale ids.
  The local player is read directly. Two tags at the same distance fall back to the full walk:
  their order is the table's, which only the walk knows (identical to the byte in a launch).
- Rare kinds (`RUSTCRAFT_OPT_RARE_KIND_LISTS`): guardians and elder guardians (eyes, then beams),
  fishing bobbers, monsters with client effects (`mob_fx`, illusioner copies), firework rockets
  (those attached to the local player, counted every tick by `player_env.rs`) listed at spawn.
  Their order is the list's: eyes and bobber owners go into tables, beams are opaque quads and
  fishing lines black opaque lines (order invisible); the monster effects draw client-side
  randomness in list order, which the decision of 19/10 allows (the table order was already
  random per launch: the hasher is seeded per table).
- "Below name" keys (`RUSTCRAFT_OPT_BELOW_NAME_KEY_CACHE`, acts with `NAME_TAG_EARLY_REJECT=1`): the
  cache key of the line kept per entity (UUID and kind checked) while its stamp holds: new
  `Scoreboard::below_name_revision` (score or reset of the slot 2 objective, that slot,
  objectives; not `version`, which only follows teams and would miss a score change), player
  list revision (profile names), language generation, bound keys. Mannequin lines (their own data)
  are built every frame. A frame without the tag forgets its key.
- Player sets (`RUSTCRAFT_OPT_PLAYER_SET_REUSE`): spectators, upside-down and eared players refilled
  in place, only when (player list revision, local player, local spectator mode) changes; the
  stamp lives in the entity renderer, so a new renderer refills them.
- Water contact on the entity (`RUSTCRAFT_OPT_WET_FLAG_IN_ENTITY`): `Entity::was_wet` and
  `Entities::local_was_wet` replace the thread table and its per-tick set. Difference of the old
  path, faithful now: an entity removed and spawned again under the same id between two checks
  inherited the removed one's state (a splash on its first check); the game makes a new entity,
  which never splashes on its first tick. The old table also survived a world change.
- Proofs: `name_tags_match_the_old_code` (400 frames of a scene of every holder kind, teams with
  the four name-tag rules, frame items named, spectator and hidden interface, entities removed by
  `apply` and elsewhere, ids spawned again with another kind, local player respawned, equal
  distances; tags equal in order, origin, text, anchor, light and discretion for the walk and the
  candidates; three sabotages fail), `below_line_keys_are_kept_while_their_stamp_holds` (built once,
  rebuilt on score, profile, language, another entity under the id, return in range),
  `below_name_revision_moves_with_the_below_lines` (20,000 events), `kept_subsets_match_the_walk_of_every_entity`,
  `player_sets_match_the_old_code`, `splash_state_on_the_entity_matches_the_table`,
  `uuid_text_writes_the_old_form`, `switches_follow_their_variable`; ten sabotages verified in all.
- Same pattern elsewhere: the client block entity tick walked every entity twice for the same
  player list (positions, then UUIDs and positions): one walk, same order (`app_tick.rs`).
- Memory: `Entity` + 1 byte (`was_wet`), seven small id sets in `Entities` (members only), the kept
  keys (live tags with a line). Axes not concerned: integrated server, network, disk, chunk loading,
  startup, GPU and VRAM (the same tags, outlines and quads are drawn).
- Not done: the entity kind as a number instead of a string (242 comparisons; mission of its
  own); the main `entity_fx` pass (bees, wolves, pandas are not rare) and the dragon pass; a
  UUID → id index for the Tab list's "in world" test (called only for "Dinnerbone"/"Grumm" rows).
- To measure on the quiet machine, each switch at 0 then all at 1: `crowd-1000`, `crowd`,
  `team-crowd` and `team-crowd-named` (the 1,000 mobs in one team, then with visible names),
  `name-tags`, `glowing`, `forest`, `plains32`, `tnt`, `village` (CPU ms, p99).
## C6-F03: kept entity compounds, dirty chunk and refile on chunk change (03/11, loaded machine)

- Fidelity first: a change of a kept compound (`/data merge`, `/tag`, `/tp`, the motion reset
  after `/tp`) now marks its chunk for the next save (it was lost at reload), and a compound that
  changes chunk (`/tp`, `/data merge` of `Pos`, an armor stand that falls or is pushed across a
  border) moves to the list of its new chunk, as the game files an entity under the chunk of its
  position. Clients see it leave or enter the chunks they were sent (removal, announcement).
  Proof: `tests_kept_entity_chunks` (5 tests, all fail with the former code).
- Cost of the refile check in the decorations' edit (every move of an armor stand or a
  mannequin): `tests_kept_entity_chunks::bench_decoration_edit_chunk_check` (2 passes, 200,000
  edits): edit 66-68 ns, of which 7.1 ns for the check (position read back, chunk compared). The
  whole-map search runs only when a border is crossed. Lead noted, not taken: check only in the
  three edits that write `Pos` (saves the 7 ns on the other edits, at the price of a second edit
  function to keep in step).
- `kill` of a kept compound marked every entity chunk of the dimension for the next save; it now
  marks only the chunk that held it (`RUSTCRAFT_OPT_KILL_REWRITES_ONE_CHUNK=0`: the former way).
  Deterministic count on the test world (25 entity chunks read): the save after the kill writes
  1 entity chunk instead of 25; the entities on disk are the same for both paths
  (`tests_kept_entity_chunks::kill_marks_only_its_chunk_same_disk_contents`). The scene bench
  kills no kept compound: no scene to measure.
- Multi-core and GPU: not applicable (one list search per command, mutations in order).
- Remaining difference, noted: a compound teleported into a chunk whose entities were never read
  stays in its former list (the game reads the destination file at save time and merges; ours
  would overwrite it), and stays selectable (the game stops tracking it).
## C1-M07: state transformation tables off the critical path (03/11, loaded machine)

The five tables (quarter, half, three-quarter turn; left-right and front-back mirror) of every
block state were rewritten as text, state by state (≈ 30,000 states × 5, a `Vec` and three
`String`s per property, a map of the sides, a formatted value), inside `WorldData::load`: in
"generator ready" and at every world (re)opening. Switches, default = new, `=0` = old:
- `RUSTCRAFT_OPT_COMPOSE_ROTATIONS`: half turn = quarter ∘ quarter, three quarters = quarter ∘
  half; mirrored stairs read the half-turn table instead of turning again.
- `RUSTCRAFT_OPT_TRANSFORMS_PER_BLOCK`: rules worked out once per block (new value index for
  each value index; the sides take the value of the side that turns onto them; mirrored stairs
  by their facing and shape), then applied to its states by index.
- `RUSTCRAFT_OPT_PARALLEL_TRANSFORMS=1` (default OFF): blocks and states across the rayon pool,
  only on the inline build (a build in the background cell stays sequential: its waiters block
  inside the cell).
- `RUSTCRAFT_OPT_TRANSFORMS_IN_BACKGROUND`: `0` inline, `rayon` a pool task, otherwise a dedicated
  thread (default); the first use waits for the build, or does it if it has not started.
- `RUSTCRAFT_OPT_KEEP_TRANSFORMS`: tables (≈ 0.6 MB) kept by the process by jar and version, a
  world opened again skips the build although its generation data are rebuilt.

Proof: `every_table_build_matches_old_tables` compares the 16 combinations of the four build leads
with the old build state by state, and the old build with the stored fingerprint;
`state_tables_off_the_critical_path` checks the three places of the build and the kept tables.
Seven sabotages fail them (front-back mirror composed from left-right and a half turn, ascending
rail not turned, hinge not swapped, axis turned by a half turn, per-block rules read from the
partly rewritten state, a side keeping its own value in the per-block rule, kept tables not
stored). `STATETRANS_FAST` now only copies blocks without an oriented property; its name is left
to the end-of-wave renaming (proposed: `SKIP_UNORIENTED_BLOCKS`).

| Tables, 1 thread (ms, best of 5) | |
|---|---|
| old | 93.5 |
| blocks without an oriented property copied (`STATETRANS_FAST`, main) | 89.0 |
| composed rotations alone / + copy | 50.4 / 48.6 |
| per block alone / + copy | 3.6 / 3.5 |
| composed + per block (default) | 1.97 |

Parallel build, rayon pool of 1, 2, 4, 8, 16, 32 threads: 2.06, 1.13, 0.70, 0.79, 0.87, 1.28 ms:
less than 10 ms remain after the two first leads, so it stays off (−1.3 ms at best, inline path
only). Processor tables built alongside: 0.05 ms. Generation data (`examples/worlddata`, load
from the jar index, min of 7, 2 passes): main 91.6 / 99.7 ms; composed + per block, inline 14.3 /
13.3; + background thread 11.4 / 11.6; + kept 12.7 / 11.5 (the remaining ≈ 2 ms build is in the
background already: within noise here, kept for the reopening). Background while the stronghold
rings occupy the rayon pool (as in the generator start): call returns in 0.01-0.05 ms instead of
2.3-3.5; dedicated thread and pool task indistinguishable (tables ready 43-44 / 41-43 ms, the rings
finishing first). GPU: not applicable (≈ 2 ms of branches over text values, once). To settle on
the quiet-machine scene bench: "generator ready", first batch and in-game times of `generation`
(dedicated thread against `rayon` against `0`), `plains12` and `crowd` as controls.
Lead not done: read the tables from the official game through the extractor (each state as the
game itself turns and mirrors it; a data change for ≈ 2 ms left).

## C5-M08: region files, read outside the lock, one open file per region, header words, section count in the check (03/11, loaded machine)

Region reads and writes (`rc-sim/src/region.rs`, `world.rs`) and the block read of a section
(`chunk.rs`). Micro-benches on a LOADED machine (other agents compiling, load between 8 and
45 %), trial profile, 2 or 3 passes; the quiet-machine scene bench (`plains32`, `plains12`,
`generation`, `autosave-1`, `forest`, `village`) is still to do. Every lead stays behind its
switch (`=0`: old path, read once).

Before: a chunk read made six system calls under the region lock (seek, 4-byte slot word, file
length, seek, 5-byte head, stream) and inflated the stream still under the lock, so the tasks of
one region and the write thread waited behind every inflate; every task of 4 chunks reopened its
region file (path canonicalised, file opened, length, 8 KiB header read, 1,024 slots checked).
A write rewrote the 8 KiB header, rebuilt the sector map from the 1,024 slots and checked the
`.mcc` file on disk.

Read switches (proofs `region::tests::damaged_streams_read_the_same_for_every_plan`,
`inflate_matches_the_old_decoder`, `concurrent_reader_never_sees_invalid_locations`,
`world::tests::load_everything_same_for_every_plan`, `load_split_same_for_every_plan`,
`damaged_regions_load_the_same`, `kept_files_see_rewritten_chunks`):
- `RUSTCRAFT_OPT_REGION_POSITIONED_READS`: slot word and sectors read at their offset, the whole
  run of sectors in one call bounded by the end of the file (like the game), the length asked
  again only when a slot or its sectors go past the end known so far (files only grow while
  open). 2 calls per chunk instead of 6; readers share the region lock (read lock), the writer
  holds it alone.
- `RUSTCRAFT_OPT_REGION_INFLATE_UNLOCKED`: the stream is inflated after the lock is released.
- `RUSTCRAFT_OPT_REGION_LIBDEFLATE_READ`: zlib streams inflated by libdeflate (state and output
  buffer kept per thread, buffer doubled until the result fits, kept up to 8 MiB); any refusal
  (damaged, cut, wrong checksum, preset dictionary) goes back to flate2, so the outcome and its
  error text stay the old ones (49,486 streams compared: the 17,689 of the test world and damaged
  copies; 15,752 of them need the fall back to keep the old error). gzip (type 1) stays on flate2.
- `RUSTCRAFT_OPT_REGION_SHARED_HANDLE`: each region file opened once per load and shared by all
  its tasks (no file cursor with positioned reads); `load_everything` opens every file first,
  then reads every chunk as its own task (a full region no longer keeps one thread busy).
- `RUSTCRAFT_OPT_REGION_HANDLE_CACHE`: those files kept by the `Dimension` between loads (shared
  by its clones, at most 256 like the game, a missing file never remembered), closed when the
  world closes (`Server::close`) so that the folder can be deleted, renamed or recreated.
- Unused region locks are forgotten once 1,024 are known (the table kept one entry per file ever
  opened).

Micro-benches (`--ignored`, model world `target/perf/model`, overworld, 20,373 chunks):
- `region::tests::bench_region_inflate` (one thread, 23,714 streams, 68 MiB → 485 MiB): flate2
  15.5-16.3 µs per stream, libdeflate 10.2-11.5 µs (×1.4-1.5, 1,260-1,320 → 1,780-2,010 MiB/s).
- `world::tests::bench_load_slices` (the loader's way: slices of 32 chunks, tasks of 4, the three
  folders, 41 × 41 chunks): old 136-286 ms whatever the thread count; all 54-69 ms on 1 to
  16 threads in the quieter runs (125-134 ms in the noisy one), 60-92 ms on 32 threads (old
  163-220). Alone (32 threads): positioned reads 119-154, shared handle 103-148, handle cache
  95-133, inflate unlocked and libdeflate within the noise. The cache brings "all" from 74-78
  to 60-63 ms (one open less per region and folder and slice).
- `world::tests::bench_load_everything` (every chunk, 1 to 32 threads): too noisy on this
  machine to rank the leads alone; together: 32 threads 288-306 → 220-225 ms (two quieter runs),
  16 threads 340-351 → 248-249 ms, 1 thread 1,516-2,669 → 1,260-1,487 ms.
- `world::tests::bench_load_stages` (one thread): NBT parse 1.0-1.7 s, chunk parse 1.1-2.4 s
  (0.6-0.8 s of it is the clone of the root made by the bench), inflate 0.36-0.81 s, file reads
  (with flate2) 0.48-1.47 s, palette lookups (`state_of`) 104 ms for 1.44 million entries
  (72 ns each, about 4 % of a one-core load): the per-block property table of the mission is
  not done (NBT parse and inflate dominate), to re-measure on the quiet machine.
- `storage::tests::bench_optimize` (Optimize World, 8 region files of 1,024 chunks, every
  region switch at 0 then at 1, two runs, machine at 45-100 %): reading every chunk one by one
  88-90 → 30-41 ms; optimise with erasing (read, rewrite) 1 thread 606-829 → 398-448 ms,
  2 threads 324-426 → 221-273, 4 threads 164-199 → 121-138, 8 threads 94-107 → 68-74 ms (one
  run at 177 ms with the machine at 100 %).

Section count (`RUSTCRAFT_OPT_SECTION_ONE_PASS_COUNT`, proof `world::tests::section_count_same_both_ways`:
16 palette shapes and damages, then the 17,676 chunks of the test world parsed both ways; a
damaged section read as empty and an air count instead of the solid count are seen): the indices
are checked and the non-empty blocks counted in the same walk (one flag per palette entry), no
walk at all when the palette has `2^bits` entries (no index can fall outside) and is all air or
has none. `bench_section_count` (220,656 block sections of the model world, one thread, quieter
runs): 324-397 → 192-199 ms (−40 %, about 0.6 µs per section). The mission's histogram is not
needed: a flag per entry gives the count with one table read per block.

Write switches (proofs `region::tests::writes_same_bytes_for_every_plan`: every one of the 8
combinations leaves the same files byte for byte, region and `.mcc`, before and after closing,
from a damaged start (two slots sharing a sector, an invalid slot, a `.mcc` to remove), with
the clock fixed; the sabotages of each switch are seen; `crash_before_header_keeps_the_old_version`:
header of before put back after a write, the old version read back intact for every plan, an
allocation that frees the old sectors first is seen; `write_compressed_matches_write`):
- `RUSTCRAFT_OPT_REGION_SECTOR_BITMAP`: sectors in use counted per sector (number of slots
  covering it) and kept up to date at every slot change, instead of rebuilt from the 1,024 slots.
  Same placement as the old code in every case, overlapping slots included (see
  `docs/quirks-to-reproduce.md`: the game's bit set frees shared sectors; not reproduced, to
  decide).
- `RUSTCRAFT_OPT_REGION_HEADER_WORDS`: only the chunk's two header words written (slot,
  timestamp), sectors and words written at their offset; the whole header when a word in memory
  differs from the file (invalid slot cleared at opening, truncated header, failed write). The
  game rewrites the 8 KiB at every chunk (`docs/specs/anvil-world.md` § 3.3). 8 KiB → 8 bytes
  of header per chunk (a full save of 533 chunks: 4.2 MiB → 4.2 KiB handed to the system).
- `RUSTCRAFT_OPT_REGION_EXTERNAL_LIST`: the region's `.mcc` streams listed once, at the first
  write that needs it, instead of one file check per chunk written.
- `RegionFile::write_compressed` takes a stream already compressed (for the save pipeline,
  C5-M09).
- `region::tests::bench_region_write` (4,096 writes of streams already compressed, one thread):
  old 18.3-19.3 µs per chunk, sector map 14.9-15.9, header words 18.1-19.4 (no gain alone: the
  8 KiB land in the same two cached pages), `.mcc` list 11.9-12.7 (the file check costs about
  6 µs on Windows), all 8.2-8.7 (×2.2). With the others on, the header words still take
  0.8-0.9 µs off (all but the header words 9.4 against 8.6 in the same run, machine at 90 %).
  In a real save the zlib compression (hundreds of µs per chunk, on the write thread)
  dominates: −10 µs per chunk written.
- Header once per batch (`RUSTCRAFT_OPT_REGION_HEADER_PER_BATCH` of the mission): not done. With
  the header words, what is left is two 4-byte writes per chunk (about 1 ms for a full save of
  533 chunks), under the 2-10 ms threshold the mission set; it would also need the batch end
  from the write thread (`save.rs`, C5-M09) and reserve the freed sectors until the header is
  written (different placement from the game).
- Not done: a seqlock instead of the read lock (the lock now covers two reads of cached data,
  the inflate and the NBT parse are outside it); the 5 bytes reserved at the head of the
  compressed stream (one copy of the stream per write, about 1 µs).

Axes: integrated server CPU (chunk loading, generation reads of proto-chunks, Optimize World,
write thread), regularity (the write thread no longer waits for an inflate; the loader's tasks
of one region inflate side by side), disk (system calls per chunk read 6 → 2, per region opened
by a task about 7 → 0 with the cache; per write 5 → 3 calls and 8 KiB → 8 bytes of header), RAM
(one libdeflate state and an output buffer per pool thread, as large as the largest chunk it
inflated, at least 64 KiB: about 2-6 MB on 32 threads; at most 256 kept files per dimension
with their 8 KiB header copy), allocations (no 8 KiB header per task, inflate buffer reused).
GPU, client, network, VRAM: not concerned (files, zlib, NBT). Placement: pool threads, nothing
on the main thread.

Found on the way (not changed): the game keeps a slot starting exactly at the end of the file
(reads it as absent) where we drop it; the game frees shared sectors of overlapping slots;
`rc-assets/src/cache.rs` inflates the baked asset cache with flate2 at startup (libdeflate lead
for another mission).

To measure on the quiet machine: the six scenes of the mission, `bench_load_everything`,
`bench_load_slices`, `bench_region_inflate`, `bench_region_write`, `bench_section_count`,
`storage::tests::bench_optimize` and `tests_opt::loading::bench_chunk_sending`, with each switch
at 0 for the A/B.
## C5-M40: client network thread dispatch, lazy packet log, small client copies, block list snapshot (03/11, loaded machine)

Micro-benches (ignored tests, 3 trials, loaded machine): `cargo test --release -p rc-client
packet_dispatch_bench network_thread_handle_bench memory_send_bench hidden_rows_bench -- --ignored
--nocapture`, `cargo test --release -p rc-sim entity_lookup_bench -- --ignored --nocapture`.
Scenes to measure on the quiet machine: `lan-1` (network thread of the guest), `plains12`, `pause`;
the social screen during an online connection (frame p99 while the block list is re-read).

- Dispatch (`RUSTCRAFT_OPT_CLIENT_PACKET_TABLE`, ADOPTED, `=0` compares as before): every stage of
  the network thread decides on the packet identifier alone, so the decision is a function of
  (phase, identifier). `dispatch.rs` lists each phase's packets in the former comparison order;
  a 256-entry table built once from that list answers in one index, and an identifier outside it
  (malformed) still goes through the comparisons. `run` is cut into one handler per phase and,
  in play, one per group (session, world, chat, HUD, containers, entities). Decision only, the
  39 318 play identifiers of the reference capture: comparisons 27.6-34.7 ns per packet, table
  0.71-1.28 ns; one stream per thread, M packets/s at 1/2/4/8/16/32 threads: comparisons 33 / 62 /
  58 / 193 / 395 / 552, table 1,407 / 274 (noise) / 1,111 / 9,920 / 12,227 / 15,269. The
  comparison path scans the ordered list; the old code made the same comparisons plus five calls
  into the sub-modules, so the gain is a lower bound. Whole `handle` on the capture's light
  packets (no chunk or light): comparisons 159-221 ns, table 132-180 ns (noisy, one trial
  inverted). GPU not applicable (a per-packet decision in stream order); one network thread per
  connection, so no splitting across cores.
- Equivalence: the reference capture then a synthetic tail (cookies, resource packs, a bundle,
  identifiers 0xF0, 300 and -1, HUD, extra, misc, entity and system chat packets, a configuration
  round trip, the server's disconnection) replayed through `handle` with the table and with the
  comparisons, and through the code before this change (former `run` over a loopback socket,
  compressed frames, heavy packets on the pool): 39,413 events, 128 replies, same digests, frozen
  in `conn_replay_tests.rs`. Sabotages (title shown as subtitle in a shared handler, two entity
  packets swapped in the table) make the tests fail.
- Packet log detail lines (`RUSTCRAFT_OPT_LAZY_PACKET_LOG`, ADOPTED): the text of a detail line
  (bundle closing, cookies, resource packs) is only built when `RUSTCRAFT_PROTOLOG` is on; before,
  one `format!` per bundle even with the log off. No bench of its own (one allocation per bundle).
- In-memory send to the integrated server (`RUSTCRAFT_OPT_MOVE_SENT_PACKET`, ADOPTED): the packet
  buffer is handed over and replaced by a fresh 64-byte one instead of being copied: 34-byte
  packets to a consumer thread, copy 63.3-91.3 ns, move 34.2-71.4 ns per packet. Bytes received
  identical (test, sizes 0 to 300).
- Tick time samples (F3+2 graph): stored as `[i64; 4]` instead of one `Vec` per sample, the
  remote server's samples read into a fixed array (one allocation less per tick, none per remote
  sample). No runtime switch: a storage type; the A/B is two executables. Difference: a sample of
  fewer than four values (never sent by the official server for tick times, and never drawn) no
  longer takes one of the 1,024 places, which only matters if the graph stops reading for 51 s.
- Hidden players on the social screen (`RUSTCRAFT_OPT_HIDDEN_ROW_LOOKUP`, kept OFF, `=1` to try):
  one locked lookup per visible row instead of one copy of the list per frame, 8 rows: nobody
  hidden (the usual case) copy 4-6 ns, per row 26 ns (loses); 1 hidden 35-38 / 26-27; 20 hidden
  113-150 / 102-114 ns per frame.
- Block list on the social screen (`RUSTCRAFT_OPT_BLOCKLIST_SNAPSHOT`, ADOPTED): the drawn rows
  read the last known list without waiting for a refresh nor fetching it on the main thread (a
  missing list is fetched in the background, ending on the same list); hiding a message still
  waits for the refresh, like the game. Test: with a 300 ms refresh in progress the row reading
  returns at once with the old list, the waiting reading returns the new one after 300 ms, then
  both agree. Before, every visible row of every frame could wait for the HTTP request.
- Entity by network identifier in the integrated server (`RUSTCRAFT_OPT_DIRECT_ENTITY_LOOKUP`,
  ADOPTED): the entity lists searched in the order `all_ents` joins them, instead of building and
  sorting that list (its sort is stable: same entity found, duplicates included); 1,001 entities:
  9.0-10.5 µs -> 0.45-0.49 µs per lookup (entity tag query, spectator click, middle click).
- Not done here: the server-side identifier chains (`input.rs`, `life.rs`, `solo_packets.rs`,
  `lan/mod.rs`), mission "Table id -> handler"; the other `all_ents()` searches by UUID or kind
  in `rc-sim` (`commands/ents.rs`, `select.rs`), where the sort order can matter.
## C2-M20: terrain shader, atlas texel size in the uniform, shared footprint, lightmap buffer (03/11, loaded machine)

Two `terrain.wgsl` variants (`terrain_shader.rs`, `//#if` blocks, read once at startup, `=0`:
old path), GPU RTX 5080 Vulkan, micro-benches on a loaded machine (other agents compiling).

| Lead | Variant | Result (loaded machine) | Decision |
|---|---|---|---|
| Texel size in the uniform (`RUSTCRAFT_OPT_ATLASTEXELUNIFORM`) | 1 / side of the two square block arrays computed once on the CPU and carried in the free half of `fog_sky` (camera slot stays 256 B; the other shaders declare that slot as a `vec4` they never read past `.x`); the screen footprint `sqrt(dx² + dy²)` computed once per fragment and shared by the two frames of an animated texture | fragments: 2 texture size queries → 0 and 6 divisions → 4 written in the code each terrain fragment entry reaches (naga count, `atlas_texel_uniform_removes_size_queries_and_divisions`; per blended fragment the old path runs them twice). Texture colour identical to the bit (`atlas_texel_uniform_samples_identically_on_gpu`: 3 views, RGSS off and on, 95-142 k pixels each, 55-142 k blended toward a next frame, 35-82 k of the large array; 3 sabotages seen; a CPU texel size halved makes 367,330 values differ). `texel_sampling_cost` (200 draws, best of 7): 5.54 → 5.54 ms (RGSS off), 5.59 → 5.57 ms (RGSS on); this bench writes two Rgba32Float targets with max blending and is bound by that write, not by the shader | kept at 1 (exact, fewer instructions); fragment ms to measure on the scenes (plains32, forest, ocean, `RUSTCRAFT_GPUPASSES=1`, `RUSTCRAFT_GPUPROBE=fs`), and on an integrated GPU where the size query is a sampler message |
| Lightmap from a buffer (`RUSTCRAFT_OPT_LIGHTMAPBUFFER`) | the 256 lightmap colours also written to a storage buffer (1 KiB per lightmap change, 20 per second, only under the switch), bilinear filter by hand in the vertices: 4 loads, 3 `mix` instead of one texture sample. The weights (multiples of 1/16) are exact in any sampler; only the blend arithmetic differs | tint (vertex colour × lightmap) within 0.002 / 255 of the sampler's (41-116 k of the values differ in the low bits; bound of the test 0.25 / 255, a quarter of an output step); fog, uv, fade, layer identical to the bit (`lightmap_buffer_matches_the_sampler_on_gpu`, both quad formats, 3 views; 2 sabotages seen, a coordinate /17 instead of /16 gives a gap of 79 / 255). `shader_variants_cost`: out of view 2.53 → 2.52 ms and 2.28 → 2.29 ms, from above 10.94 → 10.87 and 10.78 → 10.89 ms: noise (±1 %) | kept at 1 (default = new, no loss measured); ninth storage buffer of the terrain vertices, so switched off by `ShaderOpts::within_limits` on a GPU limited to 8 per stage (the camera layout then keeps its old bindings); decision at the scene bench: if the vertex ms do not drop, set it to 0 |

- Not done: the 1600 × 900 screenshots of one launch with each switch (no client may run during
  this series); the scene bench (quiet machine).
- Same pattern elsewhere, outside this mission's files: `entity.wgsl`, `entity_oit.wgsl` and
  `gui.wgsl` query `textureDimensions` per vertex or fragment on sheets that grow at run time
  (a uniform would need rewriting on growth); `block_text`, `leash`, `entity`, `nametag` sample
  the lightmap per vertex like the terrain (same buffer if the scene bench keeps it).
## C6-M27: loot engine, borrowed context, table read once, sequences without allocation (03/11, loaded machine)

- Three block loot draws (a block mined by the world, by the player, blown up) were the same code
  three times: properties copied as owned strings, block entity cloned, table name written with
  `format!`, the table looked up by name twice (its sequence, then the draw), the sequence name
  copied, the sequence map keyed by a fresh `String` at each of its two lookups. Now one shared draw
  (`block_loot.rs`, B8): table resolved once per block id (`DataPack::block_loot_table`, cache
  emptied with the pack), properties lent by state (`BlockProps::State`, read from the version data
  only when a condition or `copy_state` asks), block entity lent, the table at hand drawn
  (`Loot::drops_of_table`; chests and brushes through `drops_of_looked_up_table`). Switch `RUSTCRAFT_OPT_LOOT_BORROWED_CONTEXT`
  (default 1; `0`: the former draws and engine paths, kept in `block_loot.rs` and next to the new
  ones). Sequences (`rng.rs`): looked up first, the name copied only at creation; switch
  `RUSTCRAFT_OPT_RANDOM_SEQUENCE_LOOKUP_FIRST`.
- Engine (same switch): `table_bonus` chances and the enchantment ids of an item predicate read in
  place; stacks split as the table makes them (no second list); the first candidate of a roll and
  the first four running tables kept inline; `set_random_potion` without cloning the potion
  registry, stew effects borrowed, the attribute removal key a constant. Tag entries walk the tag's
  names without a `Vec<String>` (simplification, no switch: same names in the same order; an
  expanded tag entry of the base pack, the creeper's music discs when a skeleton kills it, is
  frozen on the former walk at `ab047108cfc08943`, 64 deaths, 12 discs reached).
  Blown-up blocks no longer copy their name; the player's mining lends it.
- Equivalence (A1/A2): every block table of the base pack (1201) for up to 7 states and 4 tools
  with the server's block tags, frozen before the change at `651fc71b4e039258` (properties listed and
  lent give it); `loot_bench`'s `af880b1baa83fb25` now asserted by a non-ignored test; the shared draw
  against the three former ones on a test table (block tag and state conditions, silk touch or fortune
  test, table bonus, explosion pool, stacks of 40 to 100 split, copied state, a named barrel): same
  stacks, same saved sequences, same level random. Sabotages caught: candidate order inverted (every
  table fingerprint), generator not put back (shared draw test), table bonus index shifted (block
  table fingerprint), sequence salt shifted (sequence lookup test), tag members walked backwards
  (creeper fingerprint; the older disc test still passed). The loot tests pass with the two
  switches in each of their four combinations; with a switch at 0 the allocation test only asks
  the shared draw for no more than the former one (stone mined, former → shared: both at 1,
  5 → 2; context 1 and lookup 0, 7 → 4; context 0 and lookup 1, 10 → 7; both at 0, 12 → 9).
- Allocations per removal (deterministic, counting allocator of the tests), former draw with the old
  engine → shared draw: stone mined 12 → 2 (the list and the stack's name), blown 6 → 0.4; oak leaves
  mined 24.5 → 0.2, blown 14.9 → 0.1; named barrel mined 33 → 12, blown 21.5 → 2.8 (the rest is the
  stack's components). Time per removal (`block_loot_bench`, best of 7 × 5 000, one thread, loaded
  machine, pass 1): stone mined 740 → 546 ns, blown 407 → 294; leaves mined 1 718 → 1 326, blown
  1 216 → 961; barrel mined 1 006 → 652, blown 503 → 245. Pass 2 the same within 2 %, except a
  load spike on the all-new stone cells (863-911 ns, its leaves and barrel cells matching pass 1).
  Leads alone (stone mined by the world, both passes): borrowed context only 494-504 ns (4
  allocations, the sequence's name copied twice), sequence lookup only 566-586 ns (7): the
  combination keeps both. Engine alone (`loot_bench`): stone 476-489 → 439-442 ns, leaves
  1 358-1 381 → 1 228-1 245, zombie, dungeon chest and fishing within noise.
- Multi-core and GPU: not applicable (one draw of a few hundred ns on the server thread, in the
  order of the world's random). Not done (other files): the explosion's `default_state("minecraft:air")`
  is a linear scan of the block names per blown block; the block tag closure scans the block list
  (C6-M36); `Stack.id` is a `String` (one allocation per stack); a new sequence's MD5 allocates.
- To measure on the quiet machine: `tnt-premiere-salve` (expected invisible in the scene: the draw
  is about 1 % of an explosion).
## C7-M10: reproducible release builds and rehearsal of the full profile (03/11, loaded machine)

- Before: two builds of one commit never gave the same bytes. The Windows linker wrote the link time into the
  executable header and debug directory; the executable held 1,180 absolute dependency paths
  (`C:\Users\<name>\.cargo\registry\src\...`, one per panic location) and the user name with them; the Windows
  archive came from `Compress-Archive` and the Linux one from a bare `tar` (dates, order, owner, system bytes of
  the archive entries).
- Change (no runtime switch: build configuration, packaging and CI): `.cargo/config.toml` passes `/Brepro` to
  the Windows linker (the rustc default `/PDBALTPATH:%_PDB%` already cuts the `.pdb` path to its file name: seen
  on the executable before any change); `tools/release/hide_paths.py config` writes a per-machine cargo config
  that remaps the cargo home to `/cargo` and the checkout to `/src` (`cargo build --config <file>`), and its
  `check` command fails if the cargo home, the checkout or the home directory is still inside a binary;
  `tools/release/pack.py` builds the zip and the tar.gz (commit date as `SOURCE_DATE_EPOCH`, sorted entries,
  fixed owner, permissions, zip "created by" byte, gzip name and date); `release.yml` uses them;
  `reproducibility.yml` (manual start) builds the commit twice per system with the full profile, packages like
  the release and compares executables and archives byte for byte.
- `trim-paths` of the cargo profile is not stable in the pinned cargo 1.99.0 ("feature `trim-paths` is
  required"): the remap flags are used instead. They cannot be committed (they hold the user's home path), hence
  the generated file; cargo joins it with the committed flags (rustc line: `-C link-arg=/Brepro
  --remap-path-prefix=... --remap-path-prefix=...`). `RUSTFLAGS` in the environment would replace the committed
  flags: nothing in the repository sets it.
- Side effect on messages: our own files are given to rustc as relative paths, so `file!()` and panic locations of
  our code are unchanged (`apps/client/src/...`); a panic inside a dependency now reads
  `/cargo/registry/src/index.crates.io-.../<crate>/src/<file>.rs` in the crash report and on stderr. The crash
  report test uses hand-written locations; run with the remap on: `crash_report::tests` ok.
- Measures, profile "essai" through the queue (no LTO, 16 units, incremental off for the A/B), loaded machine:
  - Baseline executable (rust-lld, incremental): 46,113,280 bytes, 1,180 machine paths, header stamp = link time.
  - Same settings with and without the remap (MSVC `link.exe`, as the published build): file 47,310,848 →
    47,295,488 bytes (−15,360, −0.03 %), `.rdata` 9,366,016 → 9,350,656 (−0.16 %); machine paths 1,173 → 0.
    1,173 strings × 13 characters saved (a 3-letter user name: `C:\Users\<name>\.cargo` → `/cargo`); on a runner
    (`C:\Users\runneradmin\.cargo`, 8 characters longer) about 25 KB. Memory effect negligible: the benefit is
    that no user name is in the executable.
  - Equivalence, MSVC `link.exe`: build A in the worktree and build B from a copy of the same commit at another
    path (other checkout root, other target folder, other time, in parallel): IDENTICAL, sha256
    `099c4e80a97b11ea837208bfeba3e8e12d4ec36461c56d84fadc52acdc596cd8`, 0 machine paths in both.
  - Sabotage: test program, `link.exe` without `/Brepro` (`RUSTFLAGS` replaces the committed flags), two links
    2 s apart: 20 bytes differ (header stamp = real time); with `/Brepro` 0 bytes differ. `hide_paths.py check`
    on the baseline executable: exit 1 (1,180 × cargo home, 1,180 × home); on the new one: exit 0.
  - Archives: `pack.py windows` 18,481,990 bytes in 1.9 s against `Compress-Archive` 19,351,651 bytes in 1.0 s
    (−4.5 %, level 9 fixed); the same sha256 from two working directories 3 s apart; Linux tar.gz 18,483,831
    bytes. 14 unit tests (`tools/release/test_release.py`, run in the CI `tools` job); each guard failed on
    a sabotage: the zip date, the zip system byte, the zip and tar order, the gzip date, the tar entry date,
    the committed `/Brepro`, the rehearsal's build command, the path spellings and the generated config's table.
  - Tests with the remap and `link.exe` on: `rc-app` 780 passed, 0 failed, 41 ignored (including
    `crash_report::tests::report_follows_the_game_layout`); `clippy -D warnings --all-targets -p rc-app` clean.
- Finding for the linker mission: with `rust-lld` (the quick local profile; a candidate for the release) `/Brepro`
  is not enough. Builds A and B differ in exactly 20 bytes (header stamp, the stamps of the debug directory and
  the repro hash; the code is identical) because lld writes a PDB whose GUID is a hash of its content. On a test
  program `/DEBUG:NONE` makes `rust-lld` identical (0 bytes) while `/timestamp:0` still leaves 8 bytes (the PDB
  GUID). A release linked by `rust-lld` must write no PDB, or keep `link.exe`. Builds A and B sat at two paths;
  the review asked for the same-path case, measured since (profile "essai", rust-lld, `/Brepro` and remap,
  incremental off, `apps/client/src/main.rs` touched between builds so that only the game crate is compiled
  and linked again): builds 2, 3 and 4 in the same folder differ two by two in the same 20 bytes, code
  identical. Cause seen in the PDB: rustc hands the linker `target\release\deps\rustcXXXXXX\symbols.o`, in a
  folder with a random name made for every link, and lld writes that path into the PDB it hashes. The rule
  stands at the same path. (Build 1, the first after a rebuild of the dependencies, differs from the others
  in 769 bytes spread over `.text` and `.rdata`; not explained, outside the conclusion: the earlier A/B with
  `link.exe`, two complete builds from scratch, was identical.)
- Not measured here (no CI run from the worktree, the full profile is forbidden through the queue): the full
  profile (thin LTO, one unit) on the real runners, the Linux executable (ELF, `strip`, tar.gz), the duration
  of the jobs. To do with the first manual start of `Reproducibility` before the next tag (4 builds); if the
  Linux executables differ, `cmp -l` offsets are printed by the compare job.
- Review follow-up (Linux and the rehearsal): the full profile has debug info, so the `cc` crate compiles the C
  dependencies (mimalloc, libdeflate, ring) with `-g`, and `cc` 1.7.0 does not translate rustc's
  `--remap-path-prefix` (it reads only the unstable trim-paths variables). Their debug info would name the
  runner's cargo home and target folder, the path check on the unstripped Linux executable would fail the tag
  after the empty release is created, and the GNU build id (a hash of the whole file, kept by `strip`) would
  change with the paths. Measured with gcc 16.1 (MinGW, same flags as `cc`: absolute source and `-I`, `-g`):
  one small object names its folder 4 times and two folders give two different objects; with
  `-ffile-prefix-map=<folder>=/src`, 0 times and identical objects. `hide_paths.py config` now also writes
  `CFLAGS = -ffile-prefix-map=...` (not on Windows: cl.exe puts its debug info in the `.pdb`), and the path
  check reads the shipped file (Linux: after `strip`). The rehearsal builds its second copy at another checkout
  folder (`elsewhere/rustcraft`) with another cargo home, and compares the shipped files: before, both copies sat
  at the same path, so a path left in both was invisible. Not run: the Linux build itself (no Linux here).
## C6-M37A: mob list and mob position behind a small API (03/11, loaded machine)

Refactoring, no switch: prerequisite of the exact spatial index of the mobs (C5-M18), which has to
follow every mob that changes cell, also one moved by another system during the entity walk. The
slots of `Mobs::list` are private to `mobs/list.rs` (`MobList`: `get`, `get_mut`, `take`, `put`,
`try_put`, `iter`, `iter_mut`, `slots` read-only; `Mobs::push`, `remove_slot`, `drop_empty_slots`,
`sort_by_rank` and `of_kinds_mut` written in the same file), so every mutation goes through a few
functions the compiler checks; a private field of `Mobs` would have stayed open to every submodule
of `mobs`. `Mob::pos` is private, read with the inline `pos()`, written only by `set_pos` (`add_pos`,
`set_pos_axis`, `add_pos_axis` go through it). About 600 list sites and 1,383 position sites moved.
Hook points for the grid, not filled here (nothing would read them yet): `put`, `get_mut`,
`iter_mut`, `push`, `remove_slot`, the compaction and the sort, and `set_pos`.

Proof: whole rc-sim suite with the bench model world and `RUSTCRAFT_OPT_REQUIRE=1`, results equal
to before (1357, plus the 2 list API tests), tests_opt fingerprints unchanged; sabotages (`put`
appending, `set_pos` rounding to f32) caught by the list API test and the fingerprints.

`tests_opt::bench_crowd`, tick median (ms), 8 alternating passes per variant, loaded machine (other
agents compiling: medians of a variant spread over ×1.5 to ×1.9, p99 up to 138 ms):

| Mobs | old, best pass | new, best pass | old, median of passes | new, median of passes |
|---|---|---|---|---|
| 100 | 0.551 | 0.570 (+3.4%) | 0.889 | 0.717 |
| 400 | 2.582 | 2.688 (+4.1%) | 4.619 | 3.809 |
| 1,600 | 19.714 | 19.236 (−2.4%) | 29.406 | 36.700 |

No difference above the noise in either direction; the accessors are one-line inline functions
over the same vector. To settle on the quiet-machine scene bench: `crowd-1000`, `crowd`, `village`
(tick median and p99, 2 alternating passes). Not applicable: multi-core, GPU, memory (same
structures, `MobList` is the vector itself).
## C1-M01 (series 3): measuring generation, prerequisite of the zone (03/11, loaded machine)

Measurement only, no switch: nothing generated, written or relit changes.

**Whole-length profiles.** `python -I tools/hot_map.py target/perf/avant-optim-main-profils <out>
target/perf/ref-main/rustcraft.pdb --whole-profile` over the main bench profiles. The windowed v3
map sees rc_worldgen at 0.0 % because generation ends before the window; over the whole profile it
is 11.3 % of the process CPU in `generation` (crowd 4.6, dropped-items 5.0, equipped-humanoids 5.0,
redstone 4.9, tnt 4.6, crowd-1000 4.2; pregenerated scenes ≈ 2 % or less). `generation`, inclusive:
terrain 9.75 % (column generation 8.70, structure references 13 % of terrain), stronghold ring
search 1.93, full light of the new chunks 1.09, decoration only 0.41, conversion to a full chunk
0.24, structure starts NBT 0.14, proto NBT 0.12, generator setup 0.16. Self time: surface material
1.81, density volume 1.33, light propagation 0.98, Perlin (y precomputed, AVX2) 0.90, Perlin 512
table 0.90, biome distances 0.53, aquifer window 0.49. In `crowd` the ring search (1.76 %) is the
largest worldgen item. No `generation-1thread` profile exists in that folder: to record.

**Per-phase clocks of a batch** (`crates/rc-sim/src/generation_timing.rs`, under `RUSTCRAFT_GENLOG`,
which the bench launches with). One dated line per batch, `[perf] [t s] generation batch N: …`
(dated from the client's `main` like its other `[perf]` lines): wall ms of load, biomes, terrain,
bookkeeping, decoration, post-processing, light and conversion, relight, sending, proto-chunks,
eviction (consecutive laps: they add up to the batch), the single-threaded share (bookkeeping,
post-processing, sending, eviction), wall ms and region count of each decoration class, light CPU
(process-wide `rc_light::stats`) of the post-processing (mushroom chunks), new and relit chunks.
Generator setup: one line `generator <dim> setup ms:` with game data, dimension generator,
structures, stronghold rings, then the block-state and light tables built after "generator ready"
(that reading is unchanged). `tools/bench/scenes.py` turns them into `gen:<phase> ms/chunk`,
`gen:single-threaded %` and `gen setup:<step> ms` columns; `@generation-zone` groups the zone's
scenes (generation, generation-1thread, fresh village / ocean / mountain rd16, crowd) for the
quiet-machine samply pass. Cost: 3.0 µs per batch with the log on (laps, nine classes, the line
built; ≈ 0.01 % of a 37 ms batch), 0.03 µs off (`bench_batch_clock_cost`).

Proof: `batch_timing_changes_nothing` generates the same walk timed and untimed (eager writes
without cap; lazy writes, cap 16, saves) and compares every full chunk, light, relight count and
proto-chunk; every timed batch must add up and count the light of its new and relit chunks in its
own phases. Sabotages caught: relights not sent when timed (7 chunks differ); the light lap set
before the light loop (light of 9 new chunks outside its phase); the bench parser without `-` in
phase names (`proto-chunks` missing).

Micro-bench `bench_batch_phases_by_threads` (test build: every chunk lit twice by the scan check,
so light reads about double; loaded machine, 12 rows of 24 chunks, ms per chunk):

| threads | total | terrain | biomes | decoration | post-processing | light and conversion | single-threaded |
|---|---|---|---|---|---|---|---|
| 1 | 20.72 | 13.39 | 1.33 | 2.42 | 0.46 | 3.12 | 2.2 % |
| 2 | 9.42 | 5.92 | 0.57 | 1.11 | 0.40 | 1.41 | 4.2 % |
| 4 | 5.22 | 3.07 | 0.29 | 0.73 | 0.38 | 0.75 | 7.3 % |
| 8 | 2.95 | 1.57 | 0.16 | 0.46 | 0.35 | 0.40 | 11.8 % |
| 16 | 2.81 | 1.42 | 0.12 | 0.54 | 0.38 | 0.34 | 13.5 % |
| 32 | 3.19 | 1.45 | 0.18 | 0.61 | 0.47 | 0.44 | 14.8 % |

Reading (to confirm on the quiet machine, other agents were compiling): past 8 threads nothing
shortens. The single-threaded post-processing stays at ≈ 0.4 ms per chunk whatever the thread
count: it is the light of the chunks with marked mushrooms (1.1-1.9 ms each, up to 4 per 9-chunk
batch, up to 7.6 ms of a 45 ms batch in `batch_timing_changes_nothing`), run one after the other on
the generation thread. Decoration can only use as many threads as a class has regions (1 to 4 in
a 9-chunk batch, about 8 in a 24-chunk one), and grows past 8 threads. Leads for the zone's
missions: light the mushroom chunks of a batch in parallel before their post-processing (only once
proved that the post-processing of one target never changes blocks the light of another reads; the
loop runs them in order today), and give
decoration more regions per class (larger batches or overlapping the classes of two batches).
Not done here (client and samply forbidden during the series): the samply pass of
`@generation-zone` and samply on the terrain, density and biome micro-benches at 1..32 threads.
## C2-M01: counters that settle the light and meshing leads, redstone light trace (03/11, loaded machine)

Measurement mission: no gain claimed, no `RUSTCRAFT_OPT_*` switch (nothing changes a result).
The new readings are generic `[perf] counter <name> <value>` lines (column `c:<name>` of
`tools/bench/scenes.py`, mean over the window), written with `RUSTCRAFT_PERFLOG`:
- Light queues (`rc_light::stats::queues`, zone resolutions; the incremental path is not
  counted): per kind of light and per level, the entries the seeding placed and the entries each
  level held when its turn came. Free: an entry pushed always lands on a lower level (opacity ≥ 1),
  so the bucket lengths read at each level boundary are the count; nothing is added to the
  propagation loop. Kept only while the perf log is on (one cached flag per relight).
  `light_{block,sky}_seeded_per_s`, `…_seeded_at_15_per_s`, `…_pushed_per_s`, `…_cells_per_s`,
  `…_entries_per_cell`, and a `light queue entries per s, levels 15 to 1` line.
- Relights per caller: the client times its own (`light_engine`), the integrated server's are the
  rest of the process totals: `client_relights_per_s`, `client_relight_changes_per_s`,
  `client_relight_ms_per_s`, `server_relights_per_s`, `server_relight_ms_per_s`.
- Client block changes that touch the light, by origin: `light_changes_{server_light_received,
  server,piston_event,piston_finish,prediction}_per_s` (`server_light_received`: skipped by
  `SOLONORELIGHT`, looked at only with the log on).
- Meshing: `mesh_{whole_column,partial_with_base,partial_without_base}_{jobs_per_s,ms_per_job}`,
  base misses by cause `mesh_base_miss_{evicted,never_kept}_per_s` (evicted by the cache budget,
  or the last mesh was a whole column), `mesh_cache_fluid_mb` (27 words per fluid face against 6),
  `mesh_live_columns` (kept plus evicted bases: the cache that would lose none),
  `mesh_partials_{reached,unreached}_per_s` (column reached or not by the last visibility walk).
- Uploads: `upload_in_place_{sent,whole}_mb_per_s` (bytes written in place against a whole upload of
  the same columns), `upload_in_place_refused_{version,other}_per_s` (the GPU no longer holds the
  base version, or another reason).
- Visibility test of the frame (entities, particles, block entities): calls per frame by verdict
  `visibility_{outside,unchecked,one_section_reached,one_section_hidden,straddling_reached,
  straddling_hidden}_per_frame` (plain cells flushed when the frame's test is dropped).
- Column copies by writer (`ChunkGrid::get_mut_by`, a copy happens when a meshing task still
  holds the column): `column_copies_{blocks,light_update,biomes,relight,other}_per_s`.

Redstone light trace for `benches/relight.rs`: the `redstone` site of `rc-sim`
`tests_opt::bench_ticks` never ran (`cmd` demands a success, and a function without `return`
reports none, like the game: this was the "function missing" noted above); fixed, plus
`RUSTCRAFT_BENCH_SITES` to keep some sites. `RUSTCRAFT_BENCH_SITES=redstone RUSTCRAFT_LIGHTTRACE=…`
gives `bench_ticks_redstone.rclt` (57 MB, 297 relights, 80,228 changes, fingerprints in `KNOWN`).

Deterministic counts (relight bench, queue counting forced on the equivalence replay), per relight:

| trace | sky seeded (at 15) | sky pushed | sky cells | sky entries / cell | block seeded | block pushed | block entries / cell |
|---|---|---|---|---|---|---|---|
| synthetic | 17,355 (16,291) | 8,242 | 38,410 | 0.666 | 36 | 659 | 0.033 |
| surface | 1,832 (1,515) | 1,776 | 184,700 | 0.020 | 1,516 | 9,082 | 0.061 |
| Nether | 0 | 0 | 0 | – | 20,368 | 96,830 | 0.344 |
| redstone | 77,583 (74,636) | 31,145 | 171,255 | 0.635 | 540 | 29,079 | 0.175 |

Left unspread at level 1 (pushed for nothing): redstone sky 2,568, block 4,076; Nether block 11,144.
Decisions they settle:
- C2-M02 (seed only the sky frontier): confirmed for redstone, 108,700 sky entries per relight of
  which 69 % are level-15 seeds; the surface trace has only 3,600.
- C2-M19 (dense propagation): entries per cell 0.02 to 0.67, block at most 0.34; the sky share
  drops with C2-M02, so entries ≪ cells everywhere: the lead is very likely without object (to
  re-check with these counters after C2-M02).
- C2-M12 (incremental light): `LIGHTINC` gives the reference result on all 297 redstone relights
  (0 differ at equal input, against 110 of 409 on the surface trace). Relight bench A B B A on the
  redstone trace, 1 thread, LOADED machine (whole replay): work 1.235 / 2.696 s, counted 0.992 /
  0.816 s, `LIGHTINC` 0.689 / 0.437 s, `LIGHTINCMAX=16` 1.164 / 1.241 s, reference 3.855 / 5.520 s.
  The two passes of the same path differ by ×2.2: nothing is concluded; `LIGHTINC` was faster on
  both passes, to confirm on the quiet machine. The counters' cost does not show (`counted` below
  `work` on both passes).
- Meshing, uploads, visibility and copies: the values come from the scene bench (client only).
Proofs: `fast/tests/queue_counters.rs` (exact counts of a torch in air, 4 d² + 2 cells per distance
shell; sheets and touched sections identical with the counting on and off, sequential and 1 or
8 threads; sabotage 27 — counting that drops the level-14 seeds — caught; a manual sabotage
dropping level 1 from the totals was seen failing), `remesh::tests::miss_tracking_keeps_the_same_cache`
(same bases kept with and without the bookkeeping, kept plus evicted = columns whose last mesh is a
live one within the budget, fluid bytes = 27 words per fluid face; seen failing when a received
mesh does not clear its eviction mark), `remesh::tests::fluid_bytes_from_the_starts`,
`render::frustum::tests` (100,000 boxes: same answer as the test of before, every verdict drawn;
seen failing when the reached test reads one section column only), `app_meshing::tests`
(origins relight like the boolean of before; light positions unchanged), `rc-world`
`copies_counted_per_writer`; whole suites of `rc-light`, `rc-world`, `rc-sim` and `rc-app`.
To run on the quiet machine (2 alternating passes, `main` and this branch): `scenes.py` redstone,
cave, plains12, nether, generation, tnt, forest, plains32, flight, ocean, then redstone, cave,
plains12, nether, generation with `--env=RUSTCRAFT_OPT_LIGHTINC=1` (upper bound of C2-M12), and the
relight bench A B B A on the four traces (noise below 1 % before judging a light mission).
## C1-M18: jigsaw assembly, failed elements and expansion heights not worked out again (03/11, loaded machine)

`jigsaw.rs`: the assembly (one 200-line function) is cut into short steps (`Assembly`: grow a
piece, fill a junction, try an element, first fit, expansion height, place) with five work
savings behind their switches, default = new, `=0` = old path:
`RUSTCRAFT_OPT_JIGSAW_FAILED_ELEMENT_MEMO` (a pool lists an element once per unit of weight; a
second trial of the same element at the same junction meets the same free space, so it fails
again: only its draws are replayed, the shuffle of the four rotations then, per rotation, a shuffle
of its junction count over a list of unit values, rejection loop included);
`RUSTCRAFT_OPT_JIGSAW_EXPAND_MEMO` (expansion height of the village trick per (element, rotation)
for the start: two pool lookups per junction of every rotation trial before);
`RUSTCRAFT_OPT_JIGSAW_REUSED_BUFFERS` (candidate and junction order buffers kept from one trial to
the next, candidates shuffled in place); `RUSTCRAFT_OPT_JIGSAW_SPATIAL_GRID` (removed boxes of a
free space indexed by 16 × 16 columns past 100 boxes); `RUSTCRAFT_OPT_LAZY_POOLS` (pool files kept
as bytes, shared with the data, parsed at their first lookup). `RUSTCRAFT_OPT_JIGSAW_RECENT_FIRST`
(free space test from the most recent box) is OFF by default (`=1` enables it).

Work counters (tests only, counted in a pass of their own: counting during the timed passes cost
up to 80 % on a village), 60 assemblies anywhere per structure (every biome accepted), all new:
a plains village did 100,835 pool lookups, 3,738 element trials of which 1,944 repeats at the same
junction, 7,843 free space tests over 32 boxes on average (197 at most), 222 ground reads
(0.085 ms in all: not a lead); trial chambers 4,046 trials of which 3,400 repeats (84 %).

Bench `templ_tests::bench_jigsaw_assembly_per_structure` (`cargo test --release -p rc-worldgen
--lib bench_jigsaw_assembly_per_structure -- --ignored --nocapture`), 1 thread, paths
interleaved start by start (a load peak falls on all of them), best of 3 per start, ms per
assembly, mean (worst):

| Structure | old | C1-M11 (series 2 default) | all new | pool lookups, real trials (C1-M11 → new) |
|---|---|---|---|---|
| plains village | 7.58 (14.7) | 3.97 (7.79) | 0.83 (1.65) | 100,835 → 1,633; 3,738 → 1,794 |
| desert village | 3.66 (11.2) | 1.89 (5.93) | 0.41 (1.15) | 49,691 → 1,062 |
| savanna village | 5.53 (11.6) | 2.73 (5.77) | 0.66 (1.39) | 57,926 → 1,240 |
| snowy village | 7.11 (18.5) | 3.44 (7.15) | 0.74 (1.66) | 81,214 → 1,360 |
| taiga village | 9.44 (29.7) | 3.87 (9.25) | 0.82 (2.83) | 86,308 → 1,558 |
| pillager outpost | 0.174 | 0.057 | 0.041 | |
| trial chambers | 15.9 (35.8) | 5.44 (11.3) | 1.56 (3.79) | 4,046 → 646 trials |
| ancient city | 1.29 | 0.455 | 0.260 | |
| bastion remnant | 0.265 | 0.119 | 0.083 | |

Each saving alone over C1-M11 (plains village): failed-element memo 1.82, expansion memo 1.74,
reused buffers 3.49, spatial grid 3.92, recent first 3.99, lazy pools 3.91 (no effect on the
assembly: the noise). All new but one (reference: all but the lazy pools, 0.81, since "all new"
is timed right after the slowest path): without the failed-element memo 1.35, without the
expansion memo 1.60, without the buffers 1.04, without the grid 0.81, without recent first 0.80.
Trial chambers: without the failed-element memo 4.79 against 1.42-1.56.

- Recent first: deterministic count of the boxes compared, plains village 69,288 → 110,347
  (+59 %: its trials mostly meet the oldest, central boxes), trial chambers 30,361 → 14,056,
  bastion 5,099 → 4,298; time within noise everywhere. Off by default, kept for the quiet-machine
  bench.
- Spatial grid: compared boxes −7 % (plains 69,288 → 64,129), time within noise (the grid only
  exists past 100 boxes, and a village holds 32 on average). Kept on; to settle on the scene bench.
- Lazy pools: generator start reads the pools in 4.65 ms (every file parsed) against 0.03 ms
  (bytes kept); a world only parses the pools of the structures it meets, at their first lookup
  (single cell per pool, as before). Memory: the parsed text of the unused pools is no longer
  kept (the bytes are the data's, shared).
- Starts of the 128 × 128 chunk square (`bench_jigsaw_starts`, seed 20261011; 15 trial chambers
  are its only jigsaw starts), ms for 1, 2, 4, 8, 16, 32 threads: old 356, 200, 114, 224, 291, 298;
  C1-M11 172, 87, 46, 26, 25, 18; all new 99, 50, 29, 17, 12, 11. Trial chambers start 17.7 ms
  (old) → 1.6 ms. Nether square (13 bastions among 493 starts): within noise.
- Placement: 1 core by nature (single random draw, priority queue); the parallelism stays between
  starts (series 2). GPU: not applicable (branches, hash tables, sequential randomness).
- Equivalence: `jigsaw_assembly_anywhere_every_path_matches_old` (500 villages, 500 outposts,
  trial chambers and ancient cities, 100 bastions: pieces and the next draw of the start's random
  source after each assembly = stored fingerprints taken with the code from before the mission,
  then every path, one thread per path: each saving alone, the memo over the three junction views,
  all new but each saving); `jigsaw_starts_every_path_matches_old` (same paths on the squares of 3
  seeds, main world and Nether); `starts_match_stored_values` (EXPECTED_STARTS and
  EXPECTED_STARTS_NBT); `lazy_pools_match_pools_read_at_start`. Sabotages, each detected: memo
  kept between junctions, replay without the rotation shuffle, expansion memo keyed without the
  element, grid missing its last cell, recent-first walk skipping a box, candidate buffer not
  cleared, lazy pool without its fallback. (An expansion memo keyed without the rotation is not
  detected, and rightly: a rotation turns the box and the junctions together.)
- To re-measure on a quiet machine: scenes `fresh-village-rd16` (peak at the start chunk of a
  village) and `generation` ("generator ready" for the lazy pools).
## C6-M02 (series 3): mob path search, per-state path types, memo, kept buffers (03/11, loaded machine)

The ground evaluator asked the path type of a single block ≈ 13,600 times per search (village
draws below), each one reading the block again for every fact (air, name, three tags, fluid,
`lit`/`open`, walkability: up to 8 chunk-map lookups) and comparing about twelve names; the 24
neighbours of every walkable cell were read again for each neighbouring cell; every search
allocated ≈ 400 times (three hash maps, the node arena, the heap, a `Vec` per footprint type).
Switches, default = new, `=0` = old path (`SearchOptions`, read once):
- `RUSTCRAFT_OPT_PATH_TYPE_TABLE` (0, 1, 2 default): 1 the block read once and the rules applied to
  its state; 2 a byte per state (`PathRules`, built with `MobStates` at server opening, 1.06 ms,
  ≈ 30 KB): verdict of the state-only rules plus which tags come first (speleothems, then
  fences and walls), the tags read afterwards through the tag bits, so the table holds after a
  reload.
- `RUSTCRAFT_OPT_PATH_MEMO` (0, 1, 2 default): block types and static cell types kept per position
  for one search (the search only reads the world); 1 a hash map, 2 a direct-mapped table of
  4,096 slots per memo (key checked, collisions evict, slots of an older search told apart by a
  stamp: nothing to clear).
- `RUSTCRAFT_OPT_PATH_SCRATCH`: node arena, maps, heap and lists kept per thread between searches
  (cleared, capacity kept), footprint types as a 27-bit set walked upwards (the order of the
  game's sorted set).
- `RUSTCRAFT_OPT_PATH_REGION`: the 7 × 7 chunks around the mob looked up once, when first read.
- `RUSTCRAFT_OPT_PATH_PALETTE`: a walkable cell (and a flyer's cell) skips its 24 neighbour reads
  when none of the sections they touch has a dangerous state in its palette (damaging, fire,
  lava, water, damage-cautious; verdict kept per section for the search; only inside a search).
- `RUSTCRAFT_OPT_RANDOM_POS_VIEW`: the mob view of the penalty tests built once per random position
  draw (ten tries) instead of once per test, in every ten-try draw that tests penalties
  (villager, wild animal, goal, guardian, piglin and allay rules), a tamed pet's teleport to its
  owner, and once per goat ram or long-jump choice (built again after each path request of the
  long jump, which changes the mob's navigation).
- `RUSTCRAFT_OPT_PATH_KEEP`: the movement helpers (towards a point, an entity) keep the current
  path without the copy the path request made only to compare and drop it.
- Without switch: the two A* (one target, several targets) are one (`find_path_with`; with one
  target the set search is the single-target search); the dead distance at the end of the
  single-target search is gone; `fly_type` shares the neighbour scan of the ground cells; the
  amphibious and flyer water tests compare the block id, not its name; the evaluator is named
  `NodeEvaluator` and its footprint type `path_type_for_mob`; cell and section keys are one `u64`
  packed relative to the mob's block (21 bits per axis) instead of three `i32`, distinct within any
  search's reach (`relative_keys_are_distinct_within_a_search_reach`), so every row of the table
  below, `old` and `SCRATCH=0` included, hashes the packed keys.

Proofs (`mobs::nav::search_tests`, village of the bench model world, `RUSTCRAFT_OPT_WORLD`): 2,000
searches drawn with a fixed seed (20 bodies and evaluators: walkers 1×1 to 3×3, doors, flyers,
amphibian, swimmer, warden, linked creaking, penalties set along the way, one to four targets)
and 2,000 static types against the fingerprint recorded on the code before the series
(`a68d8505…`); every lead alone, pairs and all together, twice each (kept buffers reused), each
memo hit computed again and compared; every state written in the world, old rules by position
against the state rules and the table at the three `TAG_BITS` levels, before and after a reload
that changes fences, walls and speleothems; a wall built across 200 paths between two searches;
1,000 mobs × 15 random position draws with and without the shared view, each shared draw building
at most one view per ten-try loop; a block of every path type alone beside a cell, palette shortcut
against the neighbour scan; 300 mobs × 4 movement
requests kept against the old copy. Eight sabotages each fail at least one proof (type set walked
downwards, memo kept across searches, table ignoring speleothems, palette forgetting water, shared
view with the flyer flag flipped, two chunks of the region on one slot, kept path not restarted,
20-bit keys); two weaker first sabotages were not caught and were replaced (view off by one block:
only a heart-linked creaking or a fence under a flyer reads it; region slots transposed: still a
one-to-one slot map, so not a fault).

Micro-bench `bench_path_search` (same 2,000 searches; `villagers`: the 200 villager searches),
LOADED machine, 3 passes × 3 runs, median (min–max), µs per search; allocations and block types
computed per search are deterministic:

| Lead | all | villagers | allocations | block types computed |
|---|---|---|---|---|
| old | 543 (506–662) | 453 (431–502) | 400 | 13,564 |
| rules on the state read once (`TYPE_TABLE=1`) | 382–1,125 (noisy) | 321 (314–351) | 400 | 13,564 |
| per-state table (`TYPE_TABLE=2`) | 472 (270–901) | 217 (216–233) | 400 | 13,564 |
| memo, hash map / slots | 267 / 200 | 160 / 141 | 417 / 402 | 991 / 1,053 |
| kept buffers (`SCRATCH`) | 680 (497–860) | 426 (425–463) | 3.4 | 13,564 |
| region (`REGION`) | 630 (500–826) | 426 (425–445) | 400 | 13,564 |
| palette (`PALETTE`) | 332 (295–486) | 260 (245–298) | 402 | 7,006 |
| table + memo | 153 (130–234) | 116 (115–121) | 402 | 1,053 |
| table + region | 253 (226–440) | 180 (179–188) | 400 | 13,564 |
| all but memo | 154 (142–154) | 115 (114–123) | 3.4 | 7,006 |
| all but palette | 113 (107–115) | 97 (95–160) | 3.4 | 1,053 |
| all, memo as hash map | 120 (102–217) | 88 (87–89) | 3.4 | 807 |
| all (default) | 96 (92–102) | 102 (87–116) | 3.4 | 835 |

One block type (`bench_block_type_lookup`, 1 M random cells around the village, ns): old rules 52,
state read once 36, table 21.6, table + region 17.2. A hit of a level-wide direct-mapped cache
like the game's costs 1.4 ns, but such a cache is only exact if every block write, chunk load and
unload and tag reload reaches it (dozens of write sites in rc-sim); the per-search memo already
leaves ≈ 835 types computed per search (≈ 15 µs of the 96): lead not implemented, figure kept
here. The kept buffers and the region gain nothing alone on this loaded machine (their share is
inside the noise) but remove 397 of the 400 allocations and a chunk-map lookup per read; the memo
as hash map against slots is undecided (each wins one set). Placement: the searches run on the
server tick thread, synchronously (the mob uses its path in the same tick; their number and draws
are the game's); the server is not shared between threads (its sky cache is a `Cell`), so no
multi-thread run; GPU not applicable (branchy A* over a few hundred nodes). Memory: table ≈ 30 KB
per server, per thread two memo tables of 64 KB plus maps sized by the largest search. Startup:
+1.06 ms at server opening (table). To settle on the quiet-machine scene bench: village,
village-night, crowd-villagers, crowd, crowd-1000 (tick median and p99), each switch alone at 0.

Fidelity gaps found while reading the game (not fixed here, separate fidelity lot): the game keeps
a level-wide path type cache invalidated only by block updates sent to clients (not by writes
without that flag, chunks below block-ticking, chunk loads or tag reloads), so it can serve a stale
type where Rustcraft recomputes; its node map is keyed by a 32-bit hash that merges cells 128
blocks apart in x or z (or 256 in y), reachable by searches of follow range 64 and more; its block
rules read the trapdoor, fire and campfire block tags and the water and lava fluid tags where
Rustcraft compares names (identical with the vanilla data, different under a data pack that edits
those tags).
## C6-M24: scoreboard and teams, indexed holders, one lookup per write, entity removal without formatting (03/11, loaded machine)

Before: the holders sat in a list searched linearly (a command write searched the holder up to
four times: read, open, write, packet; the save and the start of tracking searched every holder
again, O(H²)); every access copied the objective name; the team of a holder was a scan of every
team's members (each decorated name, `team=`, friendly fire); the push index was rebuilt after
every join or leave; every entity leaving the world formatted its UUID (two `format!`) and
searched it as soon as a single score existed. Switches, default = new, `=0` = old (read once,
`BoardPaths`):
- `RUSTCRAFT_OPT_SCOREHOLDERMAP`: holders in a hash table (`score_holders.rs`); the order a player
  or a save sees still comes from the game's table replayed (`HashOrder`, keys only).
- `RUSTCRAFT_OPT_SCOREONELOOKUP`: `players set/add/remove` and the criteria find each score they
  write once (`update_score`, `update_criteria` without the list of objective names);
  `operation` opens each source then reads and writes its target again (`open_score`, `get`,
  `write_score`: three holder lookups per source instead of five, the gain of its row comes mostly from
  the hash table); the objective and display state once per command; no write copies the name
  of an existing score.
- `RUSTCRAFT_OPT_TEAMMEMBERINDEX`: member → team rank kept by `join_team`, `leave_team`,
  `add_team`, `remove_team` and the read (`team_members.rs`, by UUID for canonical UUID names);
  the push rules are a view of it. "First registered wins" kept (a member in two teams, from a
  save, leaves to its next team).
- `RUSTCRAFT_OPT_SCOREUUIDSET`: canonical UUID holders in a set; a removed entity whose UUID names
  no holder and no member costs one lookup, nothing formatted.

Proof: `scores_equivalence_tests` replays 10,000 random operations (every command shape, resets,
objectives, slots, filled teams, joins, leaves, removals by name and by UUID, a save re-read every
1,000) on the 16 combinations; each operation's results, the queued packets, the save flag and,
every 25 operations, the save bytes, the holder listing, and the team, push rule, scores and
decorated name of every holder and member, give a digest frozen from the board before the change
(55,292 lines). `scores_world_equivalence_tests` plays a command script on a flat world on the
former paths, on each switch alone and on all (30 pigs joined by `execute as @e run team join t
@s`, scores by selectors, operation, store, feedback, kills, then 1,000 markers killed while one
fake player holds a score): packets, chat, save and the team of every pig identical. Five
sabotages fail them (see the commit). `canonical_uuid` (no allocation) = the parser's UUID kept
only when its text is canonical, on 1,000 random UUIDs and their altered forms.

| Board alone, ns per operation (allocations), loaded machine | former | each switch alone | all |
|---|---|---|---|
| `players add`, 1,000 UUID holders, objective displayed | 6,794 (4) | map 177, one lookup 1,127 (2) | 47 (2) |
| `players add`, 10,000 holders | 43,407 (4) | map 151, one lookup 11,822 (2) | 54 (2) |
| `operation += #c`, 1,000 / 10,000 holders | 5,177 / 59,983 (5) | map 227 / 265 | 90 / 99 (2) |
| save, per holder, 1,000 / 10,000 | 1,330 / 18,923 | map 182 / 466 | 164 / 233 |
| start of tracking, per holder, 1,000 / 10,000 | 1,141 / 14,620 | map 95 / 266 | 56 / 150 |
| entity removed, 1 fake player score | 135 (5) | UUID set 1.5 (0) | 1.7 (0) |
| team of 300 members + 300 others | 420 | member index 131 | 58 |
| decorated name, same | 512 | member index 193 | 88 |
| join then push lookup, 300 members | 91,076 (2,133) | member index 1,789 (7) | 795 (7) |
| temporary fake player set then reset, beside 1,000 / 10,000 holders (pass 2) | 1,762 / 19,244 (8) | map 256 / 143 | 147 / 141 (7) |

Second pass, machine more loaded (the former paths ran ≈ 1.5-2× slower): same ratios, `players
add` 10,000 holders 88,767 → 58 ns, removal 255 → 1.6 ns, join then push 162,948 → 899 ns. The
member index alone first made a removal slower when no team exists (377 against 135 ns: the UUID
text read for nothing); an empty index now answers at once (165 against 255 ns in pass 2). The
set-then-reset pair costs its allocations (both holder names, the score's objective name, three
packet strings): hashing in the replayed order table is not visible, `hash_order.rs` unchanged.
Same cause elsewhere (a UUID formatted to find a team): `team_of_uuid` serves the warden's
target check (the target's name is no longer built when the warden has no team), the evoker's vex
and the mace smash (attacker's team looked up once, not per bystander); a displayed entity name
formats its UUID once for the team and the insertion instead of twice. Review pass: the player's
sweep (mobs, armor stands and mannequins; the player's team looked up once, the stands' team
tested after the area test), the `team` entity predicate (mobs, projectiles, any entity), the
`team=` selector filter (`ent_team`), `/teammsg` and the kill criteria (`teamkill.<colour>` finds
the victim's team by UUID; its name is formatted only for `killedByTeam.<colour>`) no longer format
a UUID to find a team (`team_by_uuid_tests`: no UUID formatted by the team test, results as
before). The `scores=` selector filter is left to C6-M13. The save of 10,000 holders goes from 189 ms to 2.3 ms (an
autosave spike). The two remaining allocations of a write are those of the packet (holder and
objective names); the eight of a saved score are its NBT. Memory: a holder costs ≈ 56-110 bytes in the hash table instead of
48-96 in the list, plus ≈ 18-36 per UUID holder in the set and ≈ 36 per member in the index
(≈ 80 KB for 1,000 holders and 300 members). Multi-core and GPU: not applicable (state of the
server thread, mutations in command order). Discarded, as planned: objectives interned as `u32`
(would change the objective order of `players list` and of the save), `Arc<str>` names (≈ 72 KB
more for 1,000 holders), a member index rebuilt on demand (O(N²) when joins and reads alternate).
Not done: `players enable` still looks its score up twice; `execute store … score` asks the
objective state once per target (it lives in `exec.rs`); leaving a team still removes the member
from the team's ordered list (O(team size)). To settle on the quiet-machine scene bench:
`datapack` (2 passes, old exe as reference).
## C7-M05: shared Java arithmetic, one sine table, floors without the C library (03/11, loaded machine)

Module `rc_data::java_math`, shared by the client and the integrated server (same process: one copy).
Micro-benches `cargo test --release -p rc-data --lib java_math -- --ignored --nocapture` (trial
profile, machine loaded by other agents compiling, best of 5 or 7; three runs, the third quoted).

- Sine table. Before: seven copies. Physics and carver: a 256 KB table each, filled by the C library
  at first use; decorations: one 256 KB table per decorator (surface, Nether, End), filled at world
  opening; environment curves, fluid mesh, sky and command coordinates: the entry recomputed by the C
  library at every call. Now one table, computed by the compiler (`const fn`: quarter-turn reduction
  with π/2 in three pieces, then Taylor series to r^17 / r^18), stored once in the executable (256 KB,
  read-only, paged in on demand). Same bits as the game's virtual machine (FNV-1a hash of the table
  dumped by the launcher's Java 25.0.1, intrinsic on or off) and, on Windows, as each of the four old
  formulas (`i / scale`, `i * 2π / 65536`, `i * π * 2 / 65536`, library entry): the test fails if one
  entry moves by one unit of the last bit. On other systems the old tables followed the local C
  library; the frozen one is the same everywhere. Compile time: rc-data builds in under 5 s.
- Switches, read once (first call): `RUSTCRAFT_OPT_SHARED_SIN_TABLE=0` computes each entry per call
  (old path of the four recomputing copies), `RUSTCRAFT_OPT_FROZEN_SIN_TABLE=0` fills the shared table
  with the C library at first use (old path of the three table copies, now shared).

| sin + cos, 10⁶ calls | 1 thread, ms | 32 threads (10⁶ each), ms |
|---|---|---|
| entry recomputed per call (old env, fluid, sky, commands) | 20.5 | 36.6 |
| table filled at first use (old physics, carver, decorations) | 1.36 | 5.23 |
| frozen table, read directly | 1.11 | 3.65 |
| frozen table through the switches (default) | 1.39 | — |

  Filling one table with the C library: 0.28 ms; before, up to five were filled (physics, carver, three
  decorators: about 1.4 ms, half of it at world opening) and held 1.25 MB of heap; now nothing at
  startup and no heap. The switch check costs 0.14 ns per read (1.39 against 1.11 ms): the same as the
  old OnceLock tables. GPU: not relevant (scattered single angles).
- Fidelity fix (fluid mesh): the old copy multiplied the float angle by the table scale in single
  precision; the game widens the angle and multiplies in double. 163 of the float angles in [−7, 7]
  sampled one in 97 land on another entry. The flowing texture's rotation changes by one table step
  (≈ 10⁻⁴ rad, a few ten-thousandths of a texel) on those angles. The reference capture has such
  flows: every rc-mesh fingerprint changed and was replaced; with the old single-precision product
  (and the shared table) all the previous values passed again, so the table itself moved nothing.
  Still different from the game in that line (outside this mission): the angle comes from the library
  `atan2` and the quarter turn is taken off in double, where the game uses its tabulated `atan2`
  (`rc-sim mobs::mth::atan2`) and subtracts in single precision.
- Floors and Java rounding: one shared set instead of the copies of worldgen (`floor_i_java`,
  `floor_small`, `floor_f_java`, `floor_l_java`, `round_half_up`), of the data command (`floor_i32`),
  of enchanting, enchantment effects, projectiles, font and mip levels (five copies of the game's
  float rounding, one of its double rounding). Two copies rounded through a single-precision sum
  (wrong for 0.49999997 and odd integers in [2²³, 2²⁴[, values those callers never reach); one
  truncated its long to an int instead of saturating. Proof: every float (2³²) and each widened to a
  double, against the library and the game's rules; 10⁸ random doubles and the limits (±0, NaN
  quiet and signaling, ±∞, subnormals, ±2³¹, ±2⁵², ±2⁶³); sabotages (truncation without correction,
  correction without saturation, floor without the sign copied back, rounding through a float sum)
  detected. 10⁷ values in ±3·10⁷, ms:

| Operation | C library (old) | Shared | Variant measured, not kept |
|---|---|---|---|
| floor to i32 | 18.1 | 6.4 (1.5·2⁵² shift) | truncation + saturating correction 12.8 |
| floor to i64 | 13.3 | 6.1 (shift, integer read in the bits) | truncation + saturating correction 10.6 |
| floor of a float to i32 | 13.7 | 7.3 (widened, shift) | old worldgen single-precision truncation 11.1 |
| floor f64 → f64 (sign of −0 kept) | 12.5 | 7.4 (±2⁵² shift + copysign) | truncation through a long 13.0 |
| ceil f64 → f64 | 12.5 | 8.3 | — |
| Java rounding of a float | 13.8 (float sum + library floor) | 8.6 | — |
| Java rounding of a double | 14.6 | 15.5 | cold code (enchanting), formula kept for clarity |

  The saturating `as` conversions of x86-64 cost extra tests: the shift reads the integer without
  them. First version of the double rounding (integer select with `saturating_add`): 86.8 ms,
  replaced. Floor to i32 without a branch (`select_unpredictable` between the shift and the
  saturated truncation): 14.8 ms in range against 6.7 with the branch (15.3 against 32.4 with 28%
  of the values beyond ±2³¹, which no caller reaches): branch kept.
- Switch out of the call path (`noise_opt noise_micro_bench`, 10⁷ in ±3·10⁷): the generator's
  scattered floors (2-D simplex, aquifer, surface rules, cell tops, clay bands, scattered ores)
  read `RUSTCRAFT_OPT_FLOOR_JAVA` at every call: 8.44 ms against 6.67 for the shared floor called
  directly (library 15.9). The switch is now read once by the hot caller and kept: per volume in
  the noise kernel (`Paths`), per evaluation context (`sampler::Opts`: aquifer, surface rules, cell
  tops, clay bands, the `floor`, `ceil`, `round` and `truncate` density functions), per simplex
  noise (at construction), per scattered ore. `=0` restores every old library call until the
  quiet-machine bench. `math::floor_switch_paths_agree` proves both paths give the same bits
  (one float in 997, halves, limits; it fails if the old path truncates or rounds like Rust);
  `sampler_opt` runs the `floor_java` variant alone on the real generator, but its surface levels
  and cell tops are positive there, so it cannot see a negative-value difference on its own.
- Collision walk (`collide::scan`, shared by `resolve`, `support_of` and the free-space
  tests): its six floors use the shared one (`RUSTCRAFT_OPT_FLOOR_JAVA=0` restores the library call);
  hot map v3: floor under 3% of `collect_in` (below its fourth callee), well under the scene bench's
  noise. Not done: the hot rc-sim functions of the mission (`mob_move`, `tick_living`, `ai_step`),
  after the server missions on those files; the other ~1,900 library roundings, none hot in the map.
- Fidelity fix (commands): the duration argument and the server's tick count rounded with Rust's
  `round` (halves away from zero) where the game rounds halves up: `/time add -0.5` (or `-0.5t`)
  was refused with "-1" below the minimum 0, the game accepts it (0 ticks); a refused `-1.5`
  reported -2 instead of -1. Both now use the shared rounding. Daylight detector: its rounding
  too (negative results are clamped to 0, nothing changes there), and its cosine came from the C
  library (single precision) where the game reads its sine table: on 3·10⁶ (sky level, angle)
  pairs, 369 signals change by one (`daylight_power_reads_the_sine_table`). The 42 other
  `.round() as <integer>` outside tests and the float `.round()` calls were reviewed: positive
  inputs only (statistics in cm and tenths of a heart, delays, costs, velocity packing, UI sizes),
  where Rust's rounding equals the game's, or not the game's
  rounding (font outlines in 26.6 fixed point, our own sine reduction, tests).
- To measure on the quiet machine: `title` (startup), `ocean` (fluids), `generation` (carver,
  decorations, noise floors), `crowd-1000`, `tnt`, `dropped-items` (collision walks); memstats at
  world opening (−1 MB of heap expected).

## C5-M29: decorations, explosion without copying every compound, one lookup per tick (07/11, loaded machine)

- Explosion pass over the decorations (`deco_explosion`): the former path copied every
  decoration's compound (equipment, framed items) before any reach test, far ones included. Now
  each compound is read in place (`deco_blast_effect`) and only one the blast breaks is copied
  (armor stand smashed, something attached killed); `RUSTCRAFT_OPT_DECO_EXPLOSION_BORROW=0`
  restores the copy. Micro-bench `decoration::switch_equivalence_tests::bench_explosion_among_decorations`
  (64 blasts, 500 fixed frames holding items + 200 equipped armor stands, 2 runs of 2 passes): blast
  out of reach of everything 323-394 us → 17-23 us; blast by a mob with `mob_griefing` off among
  the frames (in reach, nothing broken) 436-452 us → 137-144 us. The distance is now tested before
  the box (a painting's box reads its variant's size). Multi-core and GPU: not applicable (the hits
  mutate the server in registry order).
- Exposure only when the outcome reads it: an armor stand smashed and something attached killed
  by an allowed hit are decided before the eye height and the rays cast towards the box (pure
  reads, so the same events); only a decoration left standing or a refused hit casts them. Bench
  `bench_explosion_breaking_everything` (ordinary blast, no author, breaking 40 frames holding
  items + 20 equipped armor stands, rebuilt before each of 32 blasts, 3 passes): copy of every
  compound 270-283 us per blast; read in place with the exposure first (former order) 272-274 us
  (the rays dominate, the copies did not); exposure last 103-109 us. No switch of its own: the
  order only skips work, and `RUSTCRAFT_OPT_DECO_EXPLOSION_BORROW=0` restores the former path.
- One reach reckoning (`deco_blast_reach`, `deco_blast_exposure`, `deco_knockback_resistance`)
  shared by the explosion and the wind burst (`deco_blast_push`), which had their own copy of
  the distance, box, eye, direction and resistance. The explosion scene now ends with wind
  bursts, so its stored fingerprint covers both (taken before the change, unchanged after).
- Lookups by registry index (`RUSTCRAFT_OPT_DECO_SINGLE_LOOKUP`): the edit of a falling or pushed
  armor stand, a mannequin's countdowns, a push from a blast and the survival check reach the
  compound from the registry entry instead of walking the registry for the identifier (a walk per
  edit: quadratic when many stands fall or are pushed at once). Remembered slot
  (`RUSTCRAFT_OPT_DECO_SLOT_HINT`): each entry keeps the index of its compound in its chunk's
  entity list, checked by identifier before use (a removal or a move shifts it: the list is then
  scanned as before, no purge needed). Micro-bench `bench_decoration_ticks_with_falling_stands`
  (500 frames + 100 armor stands falling, decoration walk alone, 40 ticks, 6 passes, very noisy):
  old 150-215 us per tick, index alone 112-177, slot alone 132-217, both 110-152 (never above
  old in a pass). Both kept on. Not taken: a cached position per entry (the lookups no longer
  dominate once both leads are on, and it would need a refresh on every `Pos` write).
- Wind burst bench `tests_l9combat::bench_wind_burst_decorations` (400 stands, 68 in reach), new
  / old switches, 2 alternating passes: deco pass 585 / 455 then 447 / 533 us: within noise, the
  visible part of the 68 boxes (403-533 us) dominates; the scan stays 16-18 us.
- Blast pass over the still compounds (`blast_push_still`): the distance tested before the type
  (`RUSTCRAFT_OPT_STILL_PUSH_DISTANCE_FIRST=0` restores the type first). Bench
  `bench_still_push_distance_first` (2,000 markers + 500 frames, 200 calls, 2 runs of 2 passes):
  nothing in reach 45-89 → 23-36 us; near a corner of the markers 50-81 → 26-41 us; among the
  frames only (the case where the distance first does more work) 43-58 → 24-29 us. Kept on.
- Minecart explosion pass: readability only (a minecart is taken out of the list only once in
  reach), same order of calls.
- Equivalence (A2): `explosion_paths_agree`, `tick_lookup_paths_agree`, `still_push_paths_agree`
  run each scene in its own thread on every path (all old, all new, each lead alone) and compare
  every event, the ground items and each chunk's encoded entity list, plus a stored fingerprint
  (Windows). Seen failing on sabotage: reach taken as the radius (explosion), remembered slot used
  without its identifier check (explosion and ticks), still reach halved (still pass), an
  attached kill decided before the reach test (explosion), and for the lookups by index: survival
  check always true, edit applied to the next entry (ticks), blast push given to the next entry
  (explosion).
- To measure on the quiet machine: scenes `tnt` and `dropped-items` (2 alternating passes, main
  before the series and this branch with the four switches at 0).

