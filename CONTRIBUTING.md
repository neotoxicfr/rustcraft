# Contributing to Rustcraft

Thank you for wanting to help. Rustcraft aims to be an exact replacement for the official client,
much faster: every contribution is judged on those two points, fidelity and measurement.

## The clean-room rule

This is the only non-negotiable rule.

- **Nothing from Mojang enters the repository**: no code (even decompiled, even rewritten line by
  line), no texture, sound, model, font or translation, and no data extracted from the game (tables,
  reports, jar). These are read at run time from the player's own installation.
- The game's behaviour is described **in our own words** in `docs/specs/`, and the code is then
  written from the spec. No official class or method name in the code, the comments or the commit
  messages.
- Network captures, logs and world files are for local tests and stay out of the repository (see
  `.gitignore`).

A contribution that breaks this rule is refused, even if it is correct.

## Fidelity

- A behaviour is checked against the official game: local 26.3 server, compared block by block,
  tick by tick or bit by bit depending on the subject (see `docs/test-map.md` and the `*_ref.rs`
  tests).
- What still differs from the game is written down, with the reason, in the relevant spec.
- An interface change is only finished once it has been seen on screen: attach a screenshot to the
  PR.

## Performance

- Measure before claiming anything. An optimisation comes with its before and after numbers, the
  method (bench or real session, number of runs) and the machine.
- Results go in `docs/perf.md`, including the variants that were dropped: they will be measured
  again on other machines.
- For each building block, compare at least one core, all cores and, when it makes sense, the GPU.
- No per-frame allocation in steady state in the hot paths.

## Before opening a PR

```
cargo fmt
cargo clippy --release --workspace
cargo test --release --workspace
```

Clippy must report no warning in the files you touched. Most tests compare with the official game's
data: install 26.3 with the official launcher (or point `RUSTCRAFT_MC_DIR` to a game folder), then
prepare the data once with `cargo run --release -p rc-data --example prepare -- 26.3`. The project
is developed on Windows; on Linux you also need the
ALSA and udev development packages, and code specific to one system goes behind `#[cfg(...)]` with a
version for the others. The CI runs the same clippy and tests on Windows and Linux for every PR.

## Style

- Code, comments and documentation in English. Commit
  messages in English too, in the
  [Conventional Commits](https://www.conventionalcommits.org/) format (`feat(render): …`,
  `fix(net): …`, `perf(mesh): …`), with a body that explains the scenario and the reason.
- Follow the neighbouring code: same comment density, same names, same idioms.
- One PR = one subject. Large PRs are split into steps that each compile.

## Reporting a problem

Open an issue with the matching template. For a difference with the official game, describe what the
game does, what Rustcraft does, and how to reproduce it (server, version, commands). For a security
flaw, do not open a public issue: follow [SECURITY.md](SECURITY.md).

## Releases (maintainers)

Pushing a tag `vX.Y.Z` on `main` runs the Release workflow: it builds Windows and Linux and
publishes the release with an empty body. The notes are written by hand, by theme (what changes for
the player), then added with `gh release edit vX.Y.Z --notes-file <file>`.

By taking part, you accept the [code of conduct](CODE_OF_CONDUCT.md).
