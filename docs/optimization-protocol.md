# Optimization protocol (mandatory, every item)

Single source of the optimization rules requested by the user. Every optimization mission
reads it IN FULL and hands back, at the end, this list checked off (item by item, with the figure
or the reason for a non-applicable item). The reviewer checks every item; an empty or unproven item blocks
the merge. Add any new rule here, never only in a message or a memory.

Scope (19/10): ALL new code, including a fidelity fix or an added feature,
goes through this protocol before being delivered; there is no more code "to optimize later".

Switches (22/10): an environment variable, `RUSTCRAFT_OPT_*` included, is read only
through `rc_data::env_str!` / `env_flag!` / `env_os!` (one read per site, `crates/rc-data/src/env_flags.rs`);
`clippy.toml` rejects `std::env::var` and `var_os` elsewhere, and `set_var` / `remove_var` everywhere.
A `RUSTCRAFT_OPT_*` switch is read at its use site by `rc_data::opt_switch!("RUSTCRAFT_OPT_X")`
(on by default, `=0` = old path) or `opt_switch!("RUSTCRAFT_OPT_X", default_off)` (`=1` turns it
on): the final `bool` is cached, never `env_flag!` (presence only) nor a value computed before its cache.

## A. What does not move

- [ ] A1. Identical observable result: server, network and save bit for bit; rendering within
  tolerances (imperceptible rounding, measured difference); the quirks of game 26.3 remain.
  Nothing that we prove or measure depends on the clock: test seeds (`rng::test_seed`),
  special dates frozen in tests and on the bench (`RUSTCRAFT_HALLOWEEN`, `RUSTCRAFT_CHRISTMAS`).
- [ ] A2. AUTOMATIC equivalence proof (test that fails if the result changes, expected value
  stored, explicit failure if the data is missing); check that it fails on a sabotage.
- [ ] A3. Difference faithful to the official game reported as such, with its cause.

## B. Search everything

- [ ] B1. The 15 axes reviewed for every point touched:
  client CPU, integrated server CPU, GPU, regularity (p99, stutter), latency, RAM, VRAM,
  allocations, disk, network, startup, chunk loading, energy, scale, placement.
- [ ] B2. Several leads of different natures per hot spot: avoid the work (cache,
  incremental, skip the empty), another algorithm or equivalent maths, memory layout /
  allocations / SIMD, multi-core, GPU, ideas read in the mods (Sodium, Lithium, FerriteCore…,
  reading only, never copying).
- [ ] B3. Each lead measured ALONE, then the COMBINATIONS; keep the best proven combination.
- [ ] B4. Placement, 4 measurements: CPU 1 core, CPU all cores (1 thread then powers of 2 up to the
  number of logical cores; thread count never fixed: `available_parallelism`,
  `RUSTCRAFT_THREADS`), GPU (wgpu compute shader) when feasible, main or background thread.
- [ ] B5. Read the code, not only the profiles: start from the hot map cards
  (`tools/hot_map.py`), then read every touched file in full.
- [ ] B6. Root cause everywhere: every pattern found is searched for and fixed across the whole repository.
- [ ] B7. Simplify the code when possible without changing the behaviour, and keep it readable by a
  human (user request, 03/11): clear English names that say what the thing does (functions, types,
  variables, files, tests, benches, `RUSTCRAFT_OPT_*` switches), never a cryptic abbreviation or a
  lot or mission number in a name; short functions, split when they grow; a short comment saying
  WHY before every non-obvious optimisation trick. At equal gain, the simpler version wins.
- [ ] B8. Share the work: the same data walked or computed several times (two passes over the
  same list, the same neighbour search, the same lookup in several systems) becomes one pass or
  one shared structure (a spatial index shared by collisions, AI, explosions and pushes; one cache
  read by several callers). In singleplayer, client and integrated server live in one process:
  what both hold or compute (registries, block-state tables, biome data, chunk contents, light)
  is shared or handed over without a copy, and a packet is not encoded then decoded when a direct
  hand-over gives the same result (the network path stays for LAN guests).
- [ ] B9. Transfers, at every boundary crossed: CPU to GPU (batched uploads, partial updates,
  persistent or staging buffers, compact vertex formats, no readback that stalls), thread to
  thread (batched messages, no copy of large data, bounded queues), disk (batched writes,
  buffered reads, compression level), network (batching, compression threshold, deltas). Count
  what crosses per frame or per tick, before and after.
- [ ] B10. Memory footprint and sharing: identical immutable data stored once (interning,
  deduplicated palettes and block-state maps, shared sections, copy-on-write), compact types
  (smaller integers, rare variants boxed, `size_of` of the hot structs asserted in a test),
  buffers and pools reused instead of per-frame allocations, arenas for short-lived trees.
- [ ] B11. Toolchain leads, measured like any other: fat LTO against thin, `panic = "abort"`,
  profile-guided optimisation trained on the bench scenes, a faster hasher for internal maps
  whose keys no outsider controls, cold/inline hints on measured paths. CPU-specific code only
  behind runtime detection: the release runs on any x86-64. Measurement rule (06/11): every
  build or micro-bench of a toolchain lead (LTO, codegen units, opt-level, linker, panic strategy,
  PGO) is built with the settings of the shipped executable: `--profile dist` with
  `RUSTCRAFT_FULL_RELEASE=1` in `cargo-file.ps1` (`cargo test --profile dist` for a micro-bench),
  the lead's own setting passed as `--config profile.dist.<key>=<value>`; never with the trial
  profile: without LTO and with 16 codegen units it measures a program nobody ships.
  `tools/bench/build_matrix.py` builds the variants of one commit that way. Two builds with the
  same settings are byte-identical (reproducible build), so their distance is run noise only; the
  code-layout noise (about 1 to 2 %) is the distance to the `shipped-layout-salt` variant (same
  code, other order and addresses), and a toolchain lead is concluded only beyond it.
- [ ] B12. Spread the work in time: a budget per frame and per tick, amortised work (a little
  each frame rather than a spike), priorities (what the camera sees first), background threads
  for what does not need the main one. Check p99 and stutter, not only the median.
- [ ] B13. Rendering leads, within the rendering tolerance of A1: culling (frustum, an occlusion
  graph like Sodium's), instancing of entities and particles, GPU-driven draws (indirect,
  compute culling), overdraw and translucency cost, shader cost (ALU, texture samples), pipeline
  cache, frame pacing and input latency.

## C. Decide

- [ ] C1. No gain thrown away: every measurable gain is kept, even 1%.
- [ ] C2. A few hundred MB of extra memory are acceptable for a big CPU, GPU or
  frames/s gain, but every memory increase is investigated to the end (it may hide a gain).
- [ ] C3. A lead is only discarded if it gains nothing measurable or changes the result; its
  figure is noted in `docs/perf.md` so it can be re-measured later.
- [ ] C4. Decision with figures in `docs/perf.md` for every point (including when the GPU or
  multi-core loses).
- [ ] C5. No regression on another axis or another scene (otherwise fix it before delivering).
- [ ] C6. No lead is removed or cut off for good on a measurement taken on a loaded
  machine: it stays behind its switch until the quiet-machine bench.

## D. Measure cleanly

- [ ] D1. Bench `tools/bench/scenes.py` before/after, all the scenes concerned, at least 2 alternating
  passes; reference = `main` before the series AND the old code (reference exe).
- [ ] D2. Quiet machine: no compilation or other client during a measurement (compilation queue).
- [ ] D3. Rendering: 1600×900 screenshots before/after compared.
- [ ] D4. Before/after figures in the commit message.
- [ ] D5. On a loaded machine, a deterministic count (instructions or cache misses, for example
  cachegrind under WSL) may back up a timing; it never replaces the quiet-machine bench.

## E. Deliver

- [ ] E1. Adversarial review by a second agent before the merge.
- [ ] E2. Clippy without warnings and all the repository's tests.
- [ ] E3. List of this protocol checked off in the mission's final report.
