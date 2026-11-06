"""Build matrix tests (`python -I tools/bench/test_build_matrix.py`): every variant builds the
shipped profile plus its own settings only, the trial profile of the shell never reaches cargo,
the named crates exist, the hot crates keep their optimisation, `.text` is read from the PE
section table, memory figures are read in any locale. Expected values are stored here."""
import json
import os
import struct
import sys
import tempfile
import tomllib
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import build_matrix  # noqa: E402


def config_entries(command):
    return [command[i + 1] for i, arg in enumerate(command) if arg == "--config"]


def minimal_pe(sections):
    """DOS stub pointing at a PE header with an empty optional header area and the given
    (name, virtual size) sections."""
    pe_offset = 0x80
    optional_header_size = 0xF0
    head = bytearray(pe_offset)
    head[:2] = b"MZ"
    struct.pack_into("<I", head, 0x3C, pe_offset)
    head += b"PE\0\0" + struct.pack("<HHIIIHH", 0x8664, len(sections), 0, 0, 0, optional_header_size, 0x22)
    head += bytes(optional_header_size)
    for name, virtual_size in sections:
        head += struct.pack("<8sI", name, virtual_size) + bytes(28)
    return bytes(head)


class Variants(unittest.TestCase):
    def test_every_variant_is_the_shipped_build_plus_its_own_settings(self):
        for variant, settings in build_matrix.VARIANTS.items():
            command = build_matrix.cargo_command(["cargo"], "build", variant, "T")
            self.assertEqual(command[:8], ["cargo", "build", "--locked", "--profile", "dist", "--target-dir", "T", "-p"], variant)
            self.assertEqual(command[8], "rc-app", variant)
            self.assertEqual(config_entries(command), settings, variant)
            self.assertIn("--timings", command, variant)

    def test_tests_cover_the_whole_workspace_with_the_same_settings(self):
        command = build_matrix.cargo_command(["cargo"], "test", "fat-lto", "T", ["--", "--ignored"])
        self.assertEqual(command[:7], ["cargo", "test", "--locked", "--profile", "dist", "--target-dir", "T"])
        self.assertIn("--workspace", command)
        self.assertNotIn("-p", command)
        self.assertEqual(config_entries(command), ['profile.dist.lto="fat"'])
        self.assertEqual(command[-2:], ["--", "--ignored"])
        bench = build_matrix.cargo_command(["cargo"], "test", "shipped", "T", ["bench_ticks"], ["rc-sim"])
        self.assertEqual(bench[7:], ["-p", "rc-sim", "--no-fail-fast", "bench_ticks"])

    def test_settings_are_valid_cargo_config(self):
        for variant, settings in build_matrix.VARIANTS.items():
            for entry in settings:
                tomllib.loads(entry)  # raises on a malformed entry
        salt = tomllib.loads(build_matrix.VARIANTS["shipped-layout-salt"][0])
        self.assertEqual(salt["target"]["cfg(all())"]["rustflags"], ["-C", "metadata=layout-salt"])
        units = tomllib.loads(build_matrix.VARIANTS["critical-path-units-8"][0])
        self.assertEqual(units["profile"]["dist"]["package"]["rc-sim"]["codegen-units"], 8)
        size = tomllib.loads(build_matrix.VARIANTS["cold-crates-opt-size"][0])
        self.assertEqual(size["profile"]["dist"]["package"]["naga"]["opt-level"], "s")

    def test_named_crates_are_in_the_lockfile(self):
        # A misspelt package in a profile override is only a warning: the variant would build the
        # shipped profile under another name.
        with open(os.path.join(build_matrix.ROOT, "Cargo.lock"), "rb") as f:
            names = {package["name"] for package in tomllib.load(f)["package"]}
        for crate in build_matrix.COLD_CRATES + build_matrix.CRITICAL_PATH_CRATES:
            self.assertIn(crate, names)

    def test_hot_crates_keep_full_optimisation(self):
        for crate in ("rc-commands", "wgpu", "wgpu-core"):
            self.assertNotIn(crate, build_matrix.COLD_CRATES)


class Environment(unittest.TestCase):
    def test_trial_profile_of_the_shell_never_reaches_cargo(self):
        shell = {
            "CARGO_PROFILE_RELEASE_LTO": "false",
            "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16",
            "cargo_profile_release_debug": "false",
            "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER": "rust-lld",
            "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS": "-Copt-level=1",
            "CARGO_TARGET_DIR": "elsewhere",
            "CARGO_BUILD_JOBS": "2",
            "CARGO_INCREMENTAL": "1",
            "RUSTFLAGS": "-Ctarget-cpu=native",
            "CARGO_ENCODED_RUSTFLAGS": "-Ctarget-cpu=native",
            "RUSTC_WRAPPER": "sccache",
            "PATH": "p",
            "CARGO_HOME": "h",
            "RUSTCRAFT_SOLO_WORLD": "w",
            "RUSTCRAFT_OPT_NOISE_AVX2": "0",
        }
        self.assertEqual(
            build_matrix.build_environment(shell),
            {"PATH": "p", "CARGO_HOME": "h", "RUSTCRAFT_SOLO_WORLD": "w", "RUSTCRAFT_OPT_NOISE_AVX2": "0"},
        )

    def test_tests_refuse_to_run_without_the_test_world(self):
        args = mock.Mock(cargo=["cargo"], variant="shipped", out="o", rest=[], dry_run=True)
        with mock.patch.dict(os.environ, {"PATH": os.environ.get("PATH", "")}, clear=True):
            with self.assertRaises(SystemExit):
                build_matrix.test(args)


class Measures(unittest.TestCase):
    def test_text_size_comes_from_the_section_table(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "a.exe")
            with open(path, "wb") as f:
                f.write(minimal_pe([(b".rdata", 999), (b".text", 123_456), (b".pdata", 7)]))
            self.assertEqual(build_matrix.text_section_size(path), 123_456)
            with open(path, "wb") as f:
                f.write(b"\x7fELF" + bytes(60))
            self.assertIsNone(build_matrix.text_section_size(path))

    @unittest.skipUnless(os.name == "nt", "PE executables only")
    def test_text_size_of_a_real_executable(self):
        size = build_matrix.text_section_size(sys.executable)
        self.assertIsInstance(size, int)
        self.assertGreater(size, 0)
        self.assertLess(size, os.path.getsize(sys.executable))

    def test_memory_figures_in_any_locale(self):
        self.assertEqual(build_matrix.kilobytes("123,456 K"), 123_456)
        self.assertEqual(build_matrix.kilobytes("123 456 Ko"), 123_456)
        self.assertEqual(build_matrix.kilobytes("  52044 kB"), 52_044)
        self.assertEqual(build_matrix.kilobytes("N/A"), 0)

    def test_results_of_separate_runs_accumulate(self):
        with tempfile.TemporaryDirectory() as tmp:
            build_matrix.record(tmp, "shipped", {"build_seconds": 1.0})
            build_matrix.record(tmp, "fat-lto", {"build_seconds": 2.0})
            build_matrix.record(tmp, "shipped", {"build_seconds": 3.0})
            with open(os.path.join(tmp, "matrix.json"), encoding="utf-8") as f:
                self.assertEqual(
                    json.load(f),
                    {"fat-lto": {"build_seconds": 2.0}, "shipped": {"build_seconds": 3.0}},
                )


if __name__ == "__main__":
    unittest.main()
