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
Switch `RUSTCRAFT_OPT_ENVONCE=0`: re-read at every call (A/B in the same executable).

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
`RUSTCRAFT_OPT_SPLINE_STACK`, default OFF (`=1` enables) until the quiet-machine bench. Equivalence:
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
  a few atomic loads per frame, on 1 or 2 streams.
- lewton against symphonia 0.5.5 (`examples/vorbis_compare.rs`, 552 sounds, 248 M samples):
  symphonia 10.7 ns per sample against 18.6 (×1.74), but PCM identical for only 56 sounds,
  different length for 206 (end of stream), 109,822 samples at ±1. Ruled out (output
  changed). Neither is the game's decoder (stb_vorbis through LWJGL): if one day we
  compare with the game, choose the one that resembles it most, and only then speed.

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
switch (no competing lead). To check on the quiet-machine scene bench: entity
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
| Audio device tracking (C4-M03, loaded machine) | `RUSTCRAFT_OPT_AUDIODEVIDS=1`: re-read the names only when the ids change | ids 0.06-0.08 ms against full list 0.29-3.5 ms per readout (1 per second, background thread) | kept at 0: gain off the main thread (≤ 0.35% of a core) and a device renamed without changing its id would no longer be seen |
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
