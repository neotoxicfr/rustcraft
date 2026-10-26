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
- [ ] B7. Simplify the code when possible without changing the behaviour.

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

## E. Deliver

- [ ] E1. Adversarial review by a second agent before the merge.
- [ ] E2. Clippy without warnings and all the repository's tests.
- [ ] E3. List of this protocol checked off in the mission's final report.
