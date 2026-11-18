"""Builds the shipped executable once per toolchain setting, all from one commit, to measure the
toolchain leads of docs/perf.md (section C7-M04): fat against thin LTO, more codegen units for the
crates on the critical path of the build, a lower opt-level for large cold crates, the rust-lld
linker, `panic = "abort"`. Quiet machine only: the build time and the memory peak are results.

    python -I tools/bench/build_matrix.py list
    python -I tools/bench/build_matrix.py build target/matrix [--variant NAME ...] [--dry-run]
    python -I tools/bench/build_matrix.py test target/matrix --variant NAME [--package P] [-- cargo test arguments]

Tick micro-bench of a variant, built like the shipped executable (protocol B11):
`test target/matrix --variant fat-lto --package rc-sim -- bench_ticks -- --ignored --nocapture`.
`panic = "abort"` cannot be measured that way: cargo builds every test target and all its
dependencies with unwinding, whatever the profile says, so `test` refuses such a variant; that
lead is measured on the executable only (scenes).

Run it with cargo itself, not through another wrapper: it sets what a run needs (temporary
folder `<out>/tmp`, so the copies of the test worlds never land on the system drive; the LAN
server of the tests bound to 127.0.0.1; `SOURCE_DATE_EPOCH` from the commit; the path-hiding
config of the release, `<out>/hide-paths.toml`).

`build` makes a clean `cargo build --profile dist -p rc-app` per variant, all in the same folder
`<out>/target` (emptied before each build, so that the paths written into the executable are the
same for every variant), copies the executable, its PDB and cargo's timing page to `<out>/exes/`, and records the build
time, the memory peak of the build processes, the size of `.text` and the sha256 in
`<out>/matrix.json`. `test` runs the whole test suite built with the same settings (equivalence:
every stored fingerprint must still match); run it twice, the second time with
`RUSTCRAFT_OPT_NOISE_AVX2=0`, so that both noise paths are covered. Then compare the scenes with
`tools/bench/scenes.py run --exe <out>/exes/shipped.exe --exe <out>/exes/<variant>.exe`.

The variant settings are passed as `--config`, and the profile variables of the trial build
(`CARGO_PROFILE_*`, `RUSTFLAGS`, a linker or target folder from the environment) are removed
from cargo's environment: the variants differ from one another by their settings only. They are
not byte-identical to the published file, whose target folder is `target/dist`.
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HIDE_PATHS = os.path.join(ROOT, "tools", "release", "hide_paths.py")
PACKAGE = "rc-app"
PROGRAM = "rustcraft"

# Large crates that run outside the frame and the tick: shader translation at startup, TLS and
# HTTP for the downloads, regular expressions, archives, font tables. Never rc-commands (command
# blocks and /test run every tick) nor wgpu / wgpu-core (they encode every frame).
COLD_CRATES = ("naga", "rustls", "ureq", "regex", "zip", "skrifa")
# The crates whose compilation is the end of the critical path of a clean build.
CRITICAL_PATH_CRATES = ("rc-sim", "rc-app")


def per_package(crates, key, value):
    return [f"profile.dist.package.{crate}.{key}={value}" for crate in crates]


# Variant name -> `--config` entries on top of the committed profile.
VARIANTS = {
    # Cargo.toml as committed. Built twice it gives the same bytes (reproducible build), so a
    # second copy only measures the noise of the runs.
    "shipped": [],
    # Same code, other layout: an extra metadata salt changes every symbol hash, so the order of
    # the generic instances in each object and every address. Its distance to `shipped` is the
    # code-layout noise (about 1 to 2 %); a toolchain lead is concluded only beyond it.
    "shipped-layout-salt": ["target.'cfg(all())'.rustflags=['-C', 'metadata=layout-salt']"],
    "fat-lto": ['profile.dist.lto="fat"'],
    "fat-lto-panic-abort": ['profile.dist.lto="fat"', 'profile.dist.panic="abort"'],
    "critical-path-units-4": per_package(CRITICAL_PATH_CRATES, "codegen-units", 4),
    "critical-path-units-8": per_package(CRITICAL_PATH_CRATES, "codegen-units", 8),
    "critical-path-units-16": per_package(CRITICAL_PATH_CRATES, "codegen-units", 16),
    "cold-crates-opt-2": per_package(COLD_CRATES, "opt-level", 2),
    "cold-crates-opt-size": per_package(COLD_CRATES, "opt-level", '"s"'),
    # Windows only: Linux links with lld already.
    "rust-lld": ['target.x86_64-pc-windows-msvc.linker="rust-lld"'],
}

# Environment that would change the profile under a variant's feet (the compilation queue sets
# the first ones for its trial profile).
FOREIGN_BUILD_SETTINGS = re.compile(
    r"^(CARGO_PROFILE_|CARGO_TARGET_.*_(LINKER|RUSTFLAGS|RUNNER)$|CARGO_TARGET_DIR$|CARGO_BUILD_"
    r"|CARGO_INCREMENTAL$|CARGO_ENCODED_RUSTFLAGS$|RUSTFLAGS$|RUSTC_WRAPPER$|RUSTC_WORKSPACE_WRAPPER$)"
)

# Process names counted in the memory peak of a build.
BUILD_PROCESSES = {"cargo", "rustc", "link", "rust-lld", "lld-link", "cl", "cc", "ld", "build-script-build"}


def build_environment(environ):
    return {k: v for k, v in environ.items() if not FOREIGN_BUILD_SETTINGS.match(k.upper())}


def cargo_environment(environ, out, source_date_epoch):
    """The environment of every cargo run of the matrix: the trial profile removed, and what the
    tests need to stay on this machine and in the output folder."""
    env = build_environment(environ)
    # The tests copy their worlds under the system temporary folder: keep those copies here,
    # never on the system drive.
    tmp = os.path.abspath(os.path.join(out, "tmp"))
    os.makedirs(tmp, exist_ok=True)
    env["TMP"] = env["TEMP"] = env["TMPDIR"] = tmp
    # The LAN server of the tests listens on this machine only: no firewall prompt per test program.
    env.setdefault("RUSTCRAFT_LAN_BIND", "127.0.0.1")
    # Like the release workflows: the C code of the dependencies writes this date, not the clock.
    env["SOURCE_DATE_EPOCH"] = source_date_epoch
    return env


def panic_abort_settings(variant):
    return [entry for entry in VARIANTS[variant] if re.search(r'panic\s*=\s*"abort"', entry)]


def cargo_command(cargo, subcommand, variant, target_dir, hide_paths_config, extra=(), packages=()):
    """`build`: the executable. `test`: the given packages, or the whole workspace."""
    command = [*cargo, subcommand, "--locked", "--profile", "dist", "--target-dir", target_dir]
    if subcommand == "build":
        command += ["-p", PACKAGE]
    else:
        command += [arg for p in packages for arg in ("-p", p)] or ["--workspace"]
        command.append("--no-fail-fast")
    command += ["--config", hide_paths_config]
    for entry in VARIANTS[variant]:
        command += ["--config", entry]
    if subcommand == "build":
        command.append("--timings")
    return command + list(extra)


def text_section_size(path):
    """Size of the code (`.text` virtual size, without the file alignment) of a PE executable;
    None for another format."""
    with open(path, "rb") as f:
        head = f.read(4096)
    if head[:2] != b"MZ":
        return None
    pe = struct.unpack_from("<I", head, 0x3C)[0]
    if head[pe:pe + 4] != b"PE\0\0":
        return None
    sections, = struct.unpack_from("<H", head, pe + 6)
    optional_header_size, = struct.unpack_from("<H", head, pe + 20)
    table = pe + 24 + optional_header_size
    for i in range(sections):
        name, virtual_size = struct.unpack_from("<8sI", head, table + 40 * i)
        if name.rstrip(b"\0") == b".text":
            return virtual_size
    return None


def kilobytes(field):
    # tasklist writes "123,456 K" or "123 456 K" depending on the locale.
    digits = re.sub(r"\D", "", field)
    return int(digits) if digits else 0


def build_memory_bytes():
    """Memory now held by the compilers, linkers and build scripts."""
    return windows_working_sets() if os.name == "nt" else linux_resident_sets()


def windows_working_sets():
    listing = subprocess.run(["tasklist", "/fo", "csv", "/nh"], capture_output=True, text=True).stdout
    total = 0
    for line in listing.splitlines():
        fields = [f.strip('"') for f in line.split('","')]
        if len(fields) >= 5 and os.path.splitext(fields[0])[0].lower() in BUILD_PROCESSES:
            total += kilobytes(fields[4]) * 1024
    return total


def linux_resident_sets():
    total = 0
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            with open(f"/proc/{pid}/status") as f:
                status = dict(line.split(":", 1) for line in f if ":" in line)
        except OSError:
            continue
        if status.get("Name", "").strip() in BUILD_PROCESSES and "VmRSS" in status:
            total += kilobytes(status["VmRSS"]) * 1024
    return total


def run_measured(command, env):
    """Runs the build, sampling the memory of the build processes twice a second."""
    peak = [0]
    done = threading.Event()

    def sample():
        while not done.wait(0.5):
            peak[0] = max(peak[0], build_memory_bytes())

    sampler = threading.Thread(target=sample, daemon=True)
    start = time.perf_counter()
    sampler.start()
    code = subprocess.run(command, cwd=ROOT, env=env).returncode
    done.set()
    sampler.join()
    return code, time.perf_counter() - start, peak[0]


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True).stdout.strip()


def commit():
    return git("rev-parse", "HEAD") + ("-dirty" if git("status", "--porcelain") else "")


def keep_outputs(target_dir, exes_dir, variant):
    """Copies the executable, its PDB and the timing page out of the variant's target folder."""
    suffix = ".exe" if os.name == "nt" else ""
    built = os.path.join(target_dir, "dist", PROGRAM + suffix)
    os.makedirs(exes_dir, exist_ok=True)
    exe = os.path.join(exes_dir, variant + suffix)
    shutil.copy2(built, exe)
    pdb = os.path.join(target_dir, "dist", PROGRAM + ".pdb")
    if os.path.exists(pdb):
        shutil.copy2(pdb, os.path.join(exes_dir, variant + ".pdb"))
    timings = os.path.join(target_dir, "cargo-timings", "cargo-timing.html")
    if os.path.exists(timings):
        shutil.copy2(timings, os.path.join(exes_dir, variant + "-timings.html"))
    return exe


def record(out, variant, entry):
    path = os.path.join(out, "matrix.json")
    results = {}
    if os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            results = json.load(f)
    results[variant] = entry
    with open(path, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=1, sort_keys=True)


def target_dir_of(out):
    # One folder for every variant: a folder named after the variant would change the length of
    # the paths written into the executable (build-script outputs), and so its layout.
    return os.path.abspath(os.path.join(out, "target"))


def prepare_output(out):
    """Environment and path-hiding config shared by every cargo run of the matrix."""
    os.makedirs(out, exist_ok=True)
    hide_paths_config = os.path.abspath(os.path.join(out, "hide-paths.toml"))
    subprocess.run([sys.executable, "-I", HIDE_PATHS, "config", hide_paths_config], check=True)
    return cargo_environment(os.environ, out, git("log", "-1", "--format=%ct")), hide_paths_config


def build(args):
    env, hide_paths_config = prepare_output(args.out)
    target_dir = target_dir_of(args.out)
    for variant in args.variant or list(VARIANTS):
        command = cargo_command(args.cargo, "build", variant, target_dir, hide_paths_config)
        print("[matrix]", variant, ":", subprocess.list2cmdline(command), flush=True)
        if args.dry_run:
            continue
        if os.path.exists(target_dir):
            shutil.rmtree(target_dir)  # a clean build: its time is a result
        code, seconds, peak = run_measured(command, env)
        if code != 0:
            sys.exit(f"[matrix] {variant}: cargo failed ({code})")
        exe = keep_outputs(target_dir, os.path.join(args.out, "exes"), variant)
        entry = {"settings": VARIANTS[variant], "commit": commit(), "build_seconds": round(seconds, 1),
                 "memory_peak_bytes": peak, "text_bytes": text_section_size(exe),
                 "file_bytes": os.path.getsize(exe), "sha256": sha256(exe), "exe": exe}
        record(args.out, variant, entry)
        print("[matrix]", variant, json.dumps(entry), flush=True)
        if args.delete_target:
            shutil.rmtree(target_dir)


def test(args):
    if not os.environ.get("RUSTCRAFT_SOLO_WORLD"):
        # Without it the tests on the test world stop at once and pass: nothing would be proven.
        sys.exit("[matrix] set RUSTCRAFT_SOLO_WORLD to the pristine test world first")
    if panic_abort_settings(args.variant):
        # Cargo builds test targets and all their dependencies with unwinding whatever the profile
        # says: the figure would be the one of the same variant without `panic = "abort"`.
        sys.exit(f"[matrix] {args.variant}: panic = \"abort\" is ignored by cargo test; "
                 "measure it on the executable (scenes)")
    env, hide_paths_config = prepare_output(args.out)
    command = cargo_command(args.cargo, "test", args.variant, target_dir_of(args.out), hide_paths_config,
                            args.rest, args.package or ())
    print("[matrix]", subprocess.list2cmdline(command), flush=True)
    if not args.dry_run:
        sys.exit(subprocess.run(command, cwd=ROOT, env=env).returncode)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--cargo", default="cargo", help="cargo command (split on spaces)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list")
    b = sub.add_parser("build")
    b.add_argument("out")
    b.add_argument("--variant", action="append", choices=list(VARIANTS))
    b.add_argument("--dry-run", action="store_true")
    b.add_argument("--delete-target", action="store_true", help="remove the variant's target folder afterwards")
    t = sub.add_parser("test")
    t.add_argument("out")
    t.add_argument("--variant", required=True, choices=list(VARIANTS))
    t.add_argument("--package", action="append", help="only this package (repeatable); default: the workspace")
    t.add_argument("--dry-run", action="store_true")
    # Everything after the first `--` goes to `cargo test` as written (argparse would read it).
    argv = sys.argv[1:]
    split = argv.index("--") if "--" in argv else len(argv)
    args = ap.parse_args(argv[:split])
    args.rest = argv[split + 1:]
    args.cargo = args.cargo.split()
    if args.cmd == "list":
        for name, settings in VARIANTS.items():
            print(f"{name:24} {' '.join(settings) or '(Cargo.toml as committed)'}")
    elif args.cmd == "build":
        build(args)
    else:
        test(args)


if __name__ == "__main__":
    main()
