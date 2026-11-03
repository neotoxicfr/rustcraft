# Reproducible builds

Building the same commit twice gives the same executable and the same release archives, byte for byte,
so anyone can check that a published file comes from the published source.

## What makes a build differ, and what fixes it

| Source of difference | Fix |
|---|---|
| Build time written by the Windows linker into the executable header and debug directory | `-C link-arg=/Brepro` in `.cargo/config.toml` (the linker writes a hash of the content instead) |
| Absolute path of the `.pdb` in the debug directory | rustc already cuts it down to the file name (`/PDBALTPATH:%_PDB%`) |
| Absolute paths of the dependencies' sources (`C:\Users\<name>\.cargo\registry\...`) in every panic message | `--remap-path-prefix` generated per machine by `tools/release/hide_paths.py config`, passed to cargo with `--config` |
| Dates, order, owner and "created by" bytes of the archive entries | `tools/release/pack.py`, with the commit date as `SOURCE_DATE_EPOCH` |
| The toolchain, the dependencies, the actions | `rust-toolchain.toml`, `--locked`, actions pinned to a commit |

`trim-paths` of the cargo profile would replace the second generated file but is not stable in the pinned
toolchain (1.99.0: "feature `trim-paths` is required"); switch to it when it is, and delete the script's
`config` command.

## The linker matters

The published Windows build is linked by MSVC `link.exe`, which gives the same bytes twice with `/Brepro`.
`rust-lld` (used by the quick local profile) does not: it writes a PDB whose GUID is a hash of the PDB
content, and two builds of the same commit then differ in 20 bytes (the header stamp, the stamps of the
debug directory and the repro hash, all hash values; the code is identical; measured on the game). On a
test program, `/DEBUG:NONE` makes `rust-lld` reproducible too, while `/timestamp:0` still leaves the PDB
GUID. A release linked by `rust-lld` must therefore not write a PDB.

## Build the executable the way a release does

```
python -I tools/release/hide_paths.py config target/hide-paths.toml
cargo build --locked --release -p rc-app --config target/hide-paths.toml
python -I tools/release/hide_paths.py check target/release/rustcraft.exe
```

`check` fails when the cargo home, the checkout or the home directory (the user name) is still written in
the binary. The result does not depend on the folder of the checkout or on the user.

## Check a release

The executable inside `rustcraft-X.Y.Z-windows-x64.zip` / `rustcraft-X.Y.Z-linux-x64.tar.gz` is the one to
compare: build the tag as above (same OS, same pinned toolchain) and compare the sha256 of your
`rustcraft` (Linux: `strip` it first, the archive holds the stripped file) with the file in the archive.
The archive itself is also reproducible when rebuilt with the same Python and zlib; its compressed bytes can
differ with another zlib, the files inside never do.

## Rehearsal

The workflow `Reproducibility` (manual start, Actions tab) builds the selected commit twice per system
with the full release profile, packages both like `release.yml`, and fails if an executable or an archive
differs. It is also the only place besides a tag where the full profile (LTO, one code generation unit, and later the
profile-guided step) is compiled in CI, so a break of that profile is seen before the tag. It is not run on
every tag: four full builds.
