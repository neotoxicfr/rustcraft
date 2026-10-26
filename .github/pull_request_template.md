## What and why

<!-- The scenario: what was missing or wrong, what the PR changes. -->

## Verified

<!-- How: comparison with the official game, tests, screenshot for the interface. -->

## Measurements

<!-- For an optimisation or a hot path: before / after, method, machine. Otherwise: "not applicable". -->

## Checklist

- [ ] No Mojang code or resource added (clean room)
- [ ] `cargo fmt`, `cargo clippy --release --workspace` without warnings in my files
- [ ] `cargo test --release --workspace`
- [ ] Spec in `docs/specs/` updated if the described behaviour changes
- [ ] Results added to `docs/perf.md` if this is an optimisation
