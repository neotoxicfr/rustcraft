"""Release tool tests (`python -I tools/release/test_release.py`): archives identical byte for byte
whatever the date, the order and the place of the input files; path hiding writes a valid cargo
config (C compiler included on Linux) and finds the paths left in a binary; the committed linker
flag, the release/rehearsal build command, the commit date set before every build (local recipe
included), the check of the shipped file and the rehearsal's second place stay as they must be. Expected values are stored here."""
import gzip
import io
import os
import struct
import sys
import tarfile
import tempfile
import time
import tomllib
import unittest
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import hide_paths  # noqa: E402
import pack  # noqa: E402

EPOCH = 1_767_225_600  # 2026-01-01 00:00:00 UTC
PROGRAM = bytes(range(256)) * 40  # compressible but not trivial


def sample_entries():
    return [
        ("rustcraft.exe", 0o755, PROGRAM),
        ("INSTALL.txt", 0o644, b"install\n"),
        ("LICENSE-MIT", 0o644, b"mit\n"),
        ("LICENSE-APACHE", 0o644, b"apache\n"),
    ]


def read_file(path):
    with open(path, "rb") as f:
        return f.read()


def read_text(path):
    return read_file(path).decode("utf-8")


def written(writer, entries, epoch=EPOCH):
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "archive")
        writer(path, entries, epoch)
        return read_file(path)


def write_file(path, data):
    with open(path, "wb") as f:
        f.write(data)


class Archives(unittest.TestCase):
    def test_zip_content_does_not_depend_on_input_order(self):
        entries = sample_entries()
        self.assertEqual(written(pack.write_zip, entries), written(pack.write_zip, entries[::-1]))

    def test_tar_gz_content_does_not_depend_on_input_order(self):
        entries = sample_entries()
        self.assertEqual(written(pack.write_tar_gz, entries), written(pack.write_tar_gz, entries[::-1]))

    def test_zip_fields_are_fixed(self):
        with zipfile.ZipFile(io.BytesIO(written(pack.write_zip, sample_entries()))) as zf:
            infos = zf.infolist()
            self.assertEqual([i.filename for i in infos], ["INSTALL.txt", "LICENSE-APACHE", "LICENSE-MIT", "rustcraft.exe"])
            for info in infos:
                self.assertEqual(info.date_time, (2026, 1, 1, 0, 0, 0))
                self.assertEqual(info.create_system, 3, "must not depend on the system that builds the archive")
                self.assertEqual(info.compress_type, zipfile.ZIP_DEFLATED)
            self.assertEqual(infos[3].external_attr >> 16, 0o100755)
            self.assertEqual(infos[0].external_attr >> 16, 0o100644)
            self.assertEqual(zf.read("rustcraft.exe"), PROGRAM)

    def test_tar_gz_fields_are_fixed(self):
        raw = written(pack.write_tar_gz, sample_entries())
        # gzip header: magic, method, flags (no file name), date, extra flags, system.
        magic, method, flags, mtime, _extra, system = struct.unpack("<HBBIBB", raw[:10])
        self.assertEqual((magic, method, flags, mtime, system), (0x8B1F, 8, 0, 0, 255))
        with tarfile.open(fileobj=io.BytesIO(gzip.decompress(raw))) as tar:
            members = tar.getmembers()
            self.assertEqual([m.name for m in members], ["INSTALL.txt", "LICENSE-APACHE", "LICENSE-MIT", "rustcraft.exe"])
            for m in members:
                self.assertEqual((m.mtime, m.uid, m.gid, m.uname, m.gname), (EPOCH, 0, 0, "", ""))
            self.assertEqual(members[3].mode, 0o755)
            self.assertEqual(tar.extractfile("rustcraft.exe").read(), PROGRAM)

    def test_another_commit_date_changes_the_archive(self):
        for writer in (pack.write_zip, pack.write_tar_gz):
            self.assertNotEqual(written(writer, sample_entries()), written(writer, sample_entries(), EPOCH + 86_400))

    def test_pack_ignores_file_dates_and_output_place(self):
        # The same program file packed twice, its date and the output folder changed in between.
        with tempfile.TemporaryDirectory() as tmp:
            program = os.path.join(tmp, "rustcraft.exe")
            write_file(program, PROGRAM)
            first = pack.pack("windows", "1.2.3", program, os.path.join(tmp, "a"), EPOCH)
            os.utime(program, (time.time() + 5000, time.time() + 5000))
            second = pack.pack("windows", "1.2.3", program, os.path.join(tmp, "b", "c"), EPOCH)
            self.assertEqual(os.path.basename(first), "rustcraft-1.2.3-windows-x64.zip")
            self.assertEqual(read_file(first), read_file(second))
            linux = pack.pack("linux", "1.2.3", program, os.path.join(tmp, "d"), EPOCH)
            self.assertEqual(os.path.basename(linux), "rustcraft-1.2.3-linux-x64.tar.gz")
            with tarfile.open(linux) as tar:
                self.assertEqual(sorted(tar.getnames()), ["INSTALL.txt", "LICENSE-APACHE", "LICENSE-MIT", "rustcraft", "rustcraft.sh"])
                self.assertEqual(tar.getmember("rustcraft.sh").mode, 0o755)

    def test_commit_date_is_required(self):
        for environ in ({}, {"SOURCE_DATE_EPOCH": ""}, {"SOURCE_DATE_EPOCH": "yesterday"}):
            with self.assertRaises(SystemExit):
                pack.source_date_epoch(environ)
        self.assertEqual(pack.source_date_epoch({"SOURCE_DATE_EPOCH": "1767225600"}), EPOCH)


class Workflows(unittest.TestCase):
    """The pieces that live in files nobody runs on a developer's machine."""

    def test_windows_link_keeps_the_content_hash_stamp(self):
        config = tomllib.loads(read_text(os.path.join(pack.ROOT, ".cargo", "config.toml")))
        flags = config["target"]["x86_64-pc-windows-msvc"]["rustflags"]
        self.assertIn("link-arg=/Brepro", flags)

    def test_rehearsal_builds_the_way_the_release_does(self):
        def build_lines(name):
            text = read_text(os.path.join(pack.ROOT, ".github", "workflows", name))
            return [line.strip() for line in text.splitlines() if "cargo build" in line and "run:" in line]

        release = build_lines("release.yml")
        self.assertEqual(len(release), 1)
        self.assertEqual(release, build_lines("reproducibility.yml"))
        self.assertIn("hide-paths.toml", release[0])
        self.assertIn("--locked", release[0])

    def test_path_check_reads_the_shipped_linux_file(self):
        # The unstripped Linux executable holds debug info that is never published.
        for name in ("release.yml", "reproducibility.yml"):
            text = read_text(os.path.join(pack.ROOT, ".github", "workflows", name))
            self.assertIn("exe=pkg/rustcraft\n", text, name)
            self.assertLess(text.index("strip pkg/rustcraft"), text.index("hide_paths.py check"), name)

    def test_commit_date_is_set_before_every_build(self):
        # On Linux gcc writes the clock into mimalloc's "built on" line unless SOURCE_DATE_EPOCH is set,
        # so the local recipe and both workflows must set it before cargo builds the C code.
        def date_comes_before_build(text):
            date = text.find("SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)")
            return date != -1 and date < text.index("cargo build --locked --release -p rc-app")

        for name in ("release.yml", "reproducibility.yml"):
            text = read_text(os.path.join(pack.ROOT, ".github", "workflows", name))
            self.assertTrue(date_comes_before_build(text), name)
        doc = read_text(os.path.join(pack.ROOT, "docs", "reproducible-builds.md"))
        recipe = doc[doc.index("## Build the executable the way a release does"):]
        self.assertTrue(date_comes_before_build(recipe), "docs/reproducible-builds.md")

    def test_rehearsal_builds_twice_at_different_paths(self):
        text = read_text(os.path.join(pack.ROOT, ".github", "workflows", "reproducibility.yml"))
        self.assertIn("root: .\n", text)
        self.assertIn("root: elsewhere/rustcraft\n", text)
        self.assertIn("path: ${{ matrix.root }}\n", text)
        self.assertIn("working-directory: ${{ matrix.root }}\n", text)
        self.assertIn('echo "CARGO_HOME=$home" >> "$GITHUB_ENV"', text)


class HidePaths(unittest.TestCase):
    def test_config_is_valid_toml_with_both_remaps(self):
        flags = hide_paths.remap_flags(r"C:\Users\someone\.cargo", r"D:\a\work")
        config = tomllib.loads(hide_paths.config_text(flags))
        self.assertEqual(
            config["target"]["cfg(all())"]["rustflags"],
            [r"--remap-path-prefix=C:\Users\someone\.cargo=/cargo", r"--remap-path-prefix=D:\a\work=/src"],
        )

    def test_config_refuses_a_path_it_cannot_quote(self):
        with self.assertRaises(SystemExit):
            hide_paths.config_text(["--remap-path-prefix=/home/o'brien=/src"])

    def test_c_compiler_gets_the_same_renames_on_linux(self):
        c_flags = hide_paths.c_remap_flags("/home/runner/.cargo", "/home/runner/work/r", os_name="posix")
        config = tomllib.loads(hide_paths.config_text(hide_paths.remap_flags("/home/runner/.cargo", "/w"), c_flags))
        self.assertEqual(
            config["env"]["CFLAGS"],
            "-ffile-prefix-map=/home/runner/.cargo=/cargo -ffile-prefix-map=/home/runner/work/r=/src",
        )

    def test_no_c_flags_on_windows(self):
        self.assertEqual(hide_paths.c_remap_flags(r"C:\Users\some one\.cargo", r"D:\a", os_name="nt"), [])
        self.assertNotIn("[env]", hide_paths.config_text(hide_paths.remap_flags(r"C:\x", r"D:\a"), []))

    def test_c_flags_refuse_a_path_with_a_space(self):
        with self.assertRaises(SystemExit):
            hide_paths.config_text([], hide_paths.c_remap_flags("/home/some one/.cargo", "/w", os_name="posix"))

    def test_cargo_home_follows_the_environment(self):
        self.assertEqual(hide_paths.cargo_home({"CARGO_HOME": "/opt/cargo"}), "/opt/cargo")

    def test_leaks_are_found_with_either_separator(self):
        data = b"xx C:\\Users\\someone\\.cargo\\registry\\src\\a.rs yy C:/Users/someone/.cargo/registry/b.rs zz"
        self.assertEqual(hide_paths.find_leaks(data, [r"C:\Users\someone\.cargo", r"D:\other"]), {r"C:\Users\someone\.cargo": 2})

    def test_clean_binary_has_no_leak(self):
        self.assertEqual(hide_paths.find_leaks(b"/cargo/registry/src/x /src/crates/a.rs", [r"C:\Users\someone"]), {})


if __name__ == "__main__":
    unittest.main()
