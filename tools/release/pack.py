"""Release archives that are identical byte for byte when built twice from the same commit.

    SOURCE_DATE_EPOCH=<unix seconds> python -I tools/release/pack.py <windows|linux> <version> <exe> <out dir>

`SOURCE_DATE_EPOCH` is the commit date (`git log -1 --format=%ct`). Everything that normally varies
between two runs is fixed here: the date of every entry, the order of the entries, the owner and
the permissions, the header fields that depend on the operating system (zip "created by" byte,
gzip name/date/OS bytes). PowerShell's Compress-Archive and a bare `tar` fix none of these.

The files are stored at the root of the archive (no directory entries), as in earlier releases.
"""
import gzip
import io
import os
import sys
import tarfile
import time
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

EXECUTABLE = 0o755
PLAIN_FILE = 0o644
COMPRESSION_LEVEL = 9  # fixed: the default can differ between library versions

# platform -> (archive suffix, name of the program in the archive, [(source file, mode), ...])
PLATFORMS = {
    "windows": (
        "windows-x64.zip",
        "rustcraft.exe",
        [
            ("packaging/windows/INSTALL.txt", PLAIN_FILE),
            ("LICENSE-MIT", PLAIN_FILE),
            ("LICENSE-APACHE", PLAIN_FILE),
        ],
    ),
    "linux": (
        "linux-x64.tar.gz",
        "rustcraft",
        [
            ("packaging/linux/rustcraft.sh", EXECUTABLE),
            ("packaging/linux/INSTALL.txt", PLAIN_FILE),
            ("LICENSE-MIT", PLAIN_FILE),
            ("LICENSE-APACHE", PLAIN_FILE),
        ],
    ),
}


def source_date_epoch(environ=os.environ):
    """The fixed date of every entry; refuses to guess when the variable is missing."""
    raw = environ.get("SOURCE_DATE_EPOCH")
    if raw is None or not raw.isdigit():
        raise SystemExit("SOURCE_DATE_EPOCH must be the commit date in seconds (git log -1 --format=%ct)")
    return int(raw)


def read_entries(program_name, program_mode, program_path, others):
    """[(name in the archive, mode, bytes)]; the writers store them sorted by name."""
    files = [(program_name, program_mode, program_path)]
    files += [(os.path.basename(src), mode, os.path.join(ROOT, src)) for src, mode in others]
    entries = []
    for name, mode, path in files:
        with open(path, "rb") as f:
            entries.append((name, mode, f.read()))
    return entries


def write_zip(path, entries, epoch):
    stamp = time.gmtime(epoch)[:6]  # UTC: the local time zone must not change the bytes
    with zipfile.ZipFile(path, "w") as zf:
        for name, mode, data in sorted(entries, key=lambda e: e[0]):
            info = zipfile.ZipInfo(name, date_time=stamp)
            info.create_system = 3  # "Unix" everywhere: the default is 0 when run on Windows
            info.external_attr = (0o100000 | mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            zf.writestr(info, data, compresslevel=COMPRESSION_LEVEL)


def write_tar_gz(path, entries, epoch):
    with open(path, "wb") as raw:
        # Empty file name and date 0 keep the gzip header free of the archive name and the clock.
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, compresslevel=COMPRESSION_LEVEL, mtime=0) as gz:
            with tarfile.open(fileobj=gz, mode="w", format=tarfile.GNU_FORMAT) as tar:
                for name, mode, data in sorted(entries, key=lambda e: e[0]):
                    info = tarfile.TarInfo(name)
                    info.size = len(data)
                    info.mode = mode
                    info.mtime = epoch
                    info.uid = info.gid = 0
                    info.uname = info.gname = ""
                    tar.addfile(info, io.BytesIO(data))


def pack(platform, version, program_path, out_dir, epoch):
    suffix, program_name, others = PLATFORMS[platform]
    entries = read_entries(program_name, EXECUTABLE, program_path, others)
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, f"rustcraft-{version}-{suffix}")
    (write_zip if suffix.endswith(".zip") else write_tar_gz)(path, entries, epoch)
    return path


def main(argv):
    if len(argv) != 5 or argv[1] not in PLATFORMS:
        raise SystemExit(__doc__)
    _, platform, version, program_path, out_dir = argv
    print(pack(platform, version.lstrip("v"), program_path, out_dir, source_date_epoch()))


if __name__ == "__main__":
    main(sys.argv)
