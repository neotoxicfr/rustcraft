"""Reference scene bench (measurements of the optimisation, `docs/perf.md`).

Two steps, a quiet machine is mandatory for the second:

    python tools/bench/scenes.py prepare --java <java.exe> --server-jar <server.jar> [--seed 20261011]
                                        [--pregen-radius 16] [--pregen-center camera|site]
    python tools/bench/scenes.py pack        (rewrites the `perf` pack and the maps of the existing model)
    python tools/bench/scenes.py pack-files <out>   (pack and scene chat commands for the flat world of `rc-sim` tests_perf_pack)
    python tools/bench/scenes.py micro <pass1.txt> [<pass2.txt>…]   (table of `[bench]` micro-bench lines)
    python tools/bench/scenes.py run --exe <rustcraft.exe> [--exe <other.exe>] [--scenes plains12,@server] [--passes 3]
                                    [--label name] [--env VAR=value] [--matrix VAR=value]… [--samply]
                                    [--warmup 0|1] [--cold] [--dry-run] [--sound-device NAME]
                                    [--connect connect.exe] [--cache-source FOLDER]
    python tools/bench/scenes.py table --label name [--reanalyse] [--cold]

`prepare` builds the model world with a temporary official server (127.0.0.1 only, stopped at
the end): fixed seed, locations found with `locate` (plains, dark forest, village, deep ocean,
lush caves, Nether, trial chambers, Nether fortress), zones generated in advance with tickets (radius 33 chunks around the
plains; elsewhere `--pregen-radius`, 16 ≥ distance 12 + 2, centred on the chunk of the scene's
CAMERA; `--pregen-radius 13 --pregen-center site` = old model), time set to noon, weather and
spawning switched off, two flat pads (crowd, redstone); a `perf` data pack (`perf_pack.py`:
staging functions, flight, rotation and walking by teleporting every tick) and 900 locked maps.
Output: `target/perf/model/` and `target/perf/scenes.json`. `seed-model` also locates the
fresh-world flights of the swamp, badlands, old-growth taiga and Nether (`fresh-paths.json`).

`run` launches each scene `--passes` times (3 by default: 2 give no median) in rotation
(A B C A B C…, executables order rotated on each pass) on a fresh copy of the model (player placed
through its data file, creative flying, options fixed), client in the background without a
console, `RUSTCRAFT_PERFLOG` and the end capture to quit. Before pass 1, a short unmeasured
warm-up launch per executable (`--warmup 0` to skip it) fills the resource cache and the driver's
shader cache. Each executable has its own `RUSTCRAFT_CACHE` folder
(`target/perf/cache/<name>-<hash>`, version data copied from `--cache-source`, nothing is written on
C:): the limit of caches kept per folder (`KEEP` of rc-assets) no longer applies whatever the number
of `--exe`. A launch whose log says "cache to write" is marked cold and left out of the medians
(`--cold` includes it), except in the scenes made for that (cold-resources, first-launch,
fresh-install). `--matrix VAR=val` (repeatable) launches each `--exe` without a switch, with each
one alone, then all together. `--dry-run` prepares each launch without starting anything (scene
check). `--scenes` takes names and groups (`@reference`, `@server`, `@interface`… see
`SCENE_GROUPS`). `--env=-VAR` and `--exe "path|-VAR"` remove a variable (the client tests
some only for presence: `=0` would leave them on); the `=` is needed, `--env -VAR` is read as
an unknown option. A scene whose view, located path or model is
missing, or that drives the client through a variable the exe does not contain (older version),
is skipped with the reason.

Readings: frame time (median, 99th percentile), CPU and GPU per frame, columns meshed, light,
memory, VRAM, integrated server ticks, startup (milestones `[startup] <step> at N ms`, audio,
world opening, generator) and loading, generation (ms/chunk, chunks/s, first batch), saves, LAN
guests, longest tick of the window, entities drawn, allocator and server chunk data over the
window, and any generic counter `[perf] [ t s] counter <name> <value>` (mean over the window,
column `c:<name>`). Output: `target/perf/<label>/resultats.json`, Markdown table (median of the
passes and min–max spread) and `bruit.md` (spread of the passes of one exe per scene; an A/B whose
difference is within that noise does not conclude). With `--samply`, `window.json` bounds the
profile to the measurement window (`tools/window.py`).
"""
import argparse
import contextlib
import hashlib
import json
import os
import re
import shutil
import socket
import statistics
import struct
import subprocess
import sys
import time
import uuid as uuidlib

sys.path.insert(0, os.path.dirname(__file__))
import fixtures  # noqa: E402
import nbt  # noqa: E402
# The `perf` data pack (one function per staged scene) and the map data.
from perf_pack import pack_functions, write_maps, write_pack  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
# `RUSTCRAFT_PERF_DIR`: bench folder of another copy of the repository (worktree that rereads a past bench).
PERF = os.environ.get("RUSTCRAFT_PERF_DIR") or os.path.join(ROOT, "target", "perf")
NAME = "Perf"
RCON_PORT, RCON_PASS, SRV_PORT = 25598, "perf-bench-733", 25597
NO_WINDOW = 0x08000000

# --- RCON ---------------------------------------------------------------------------------


class Rcon:
    def __init__(self, port, password):
        self.s = socket.create_connection(("127.0.0.1", port), timeout=60)
        if self._packet(3, password)[0] == -1:
            raise SystemExit("RCON password refused")

    def _recv(self, n):
        buf = b""
        while len(buf) < n:
            part = self.s.recv(n - len(buf))
            if not part:
                raise ConnectionError("RCON closed")
            buf += part
        return buf

    def _packet(self, kind, body):
        data = struct.pack("<ii", 7, kind) + body.encode("utf-8") + b"\0\0"
        self.s.sendall(struct.pack("<i", len(data)) + data)
        size = struct.unpack("<i", self._recv(4))[0]
        rid, _ = struct.unpack("<ii", self._recv(8))
        return rid, self._recv(size - 8)[:-2].decode("utf-8", "replace")

    def cmd(self, c):
        return self._packet(2, c)[1]


# --- Preparing the model world ------------------------------------------------------------

COORD = re.compile(r"\[(-?\d+), (~|-?\d+), (-?\d+)\]")


def locate(r, what, dim="minecraft:overworld"):
    out = r.cmd(f"execute in {dim} run locate {what}")
    m = COORD.search(out)
    if not m:
        raise SystemExit(f"locate {what} : {out}")
    y = None if m.group(2) == "~" else int(m.group(2))
    return int(m.group(1)), y, int(m.group(3))


AWAY = re.compile(r"\((\d+) blocks? away\)")
# Game yaw (0 = south, +z) -> horizontal direction.
DIRS = {0.0: (0, 1), 90.0: (-1, 0), 180.0: (0, -1), 270.0: (1, 0)}


def inside(r, x, z, biome):
    out = r.cmd(f"execute positioned {x} 64 {z} run locate biome {biome}")
    m = AWAY.search(out)
    return m is not None and int(m.group(1)) <= 24


def find_view(r, biome, reach=(0, 64, 128, 192)):
    """Point of the biome from which the view (one of the 4 yaws) stays in the biome over `reach` blocks."""
    for ox, oz in [(0, 0), (1600, 0), (-1600, 0), (0, 1600), (0, -1600), (1600, 1600), (-1600, -1600), (3200, 0), (0, 3200)]:
        m = COORD.search(r.cmd(f"execute positioned {ox} 64 {oz} run locate biome {biome}"))
        if not m:
            continue
        x, z = int(m.group(1)), int(m.group(3))
        for yaw, (dx, dz) in DIRS.items():
            if all(inside(r, x + dx * d, z + dz * d, biome) for d in reach):
                return x, z, yaw
    raise SystemExit(f"no view entirely inside {biome}")


def forceload(r, dim, cx, cz, radius, add=True):
    """Forced-loading tickets on the square of radius `radius` chunks (blocks of 16 × 16)."""
    verb = "add" if add else "remove"
    for x0 in range(cx - radius, cx + radius + 1, 16):
        for z0 in range(cz - radius, cz + radius + 1, 16):
            x1, z1 = min(x0 + 15, cx + radius), min(z0 + 15, cz + radius)
            out = r.cmd(f"execute in {dim} run forceload {verb} {x0 * 16} {z0 * 16} {x1 * 16} {z1 * 16}")
            if "rror" in out or "Unknown" in out:
                print(f"  forceload: {out}")


def wait_loaded(r, dim, cx, cz, radius, timeout=900):
    """Waits for the corners and the centre of the zone to be loaded (generated)."""
    pts = [(cx, cz)] + [(cx + dx * radius, cz + dz * radius) for dx in (-1, 1) for dz in (-1, 1)]
    t0 = time.time()
    while time.time() - t0 < timeout:
        ok = all("passed" in r.cmd(f"execute in {dim} if loaded {x * 16} 0 {z * 16}") for x, z in pts)
        if ok:
            return time.time() - t0
        time.sleep(2)
    raise SystemExit(f"zone {dim} ({cx}, {cz}) not loaded after {timeout} s")


def surface(r, x, z, heightmap="motion_blocking", dim="minecraft:overworld"):
    r.cmd("kill @e[type=marker,tag=perfh]")
    r.cmd(f"execute in {dim} positioned {x} 0 {z} positioned over {heightmap} run summon marker ~ ~ ~ {{Tags:[\"perfh\"]}}")
    out = r.cmd("data get entity @e[type=marker,tag=perfh,limit=1] Pos[1]")
    r.cmd("kill @e[type=marker,tag=perfh]")
    m = re.search(r"(-?\d+(?:\.\d+)?)d", out)
    if not m:
        raise SystemExit(f"height at ({x}, {z}): {out}")
    return int(float(m.group(1)))


def air_pocket(r, dim, x, z, ys):
    """First y of `ys` with 4 blocks of air above and in front (toward +z): free viewpoint."""
    for y in ys:
        if all("passed" in r.cmd(f"execute in {dim} if block {x} {y + dy} {z + dz} minecraft:air") for dy in range(0, 3) for dz in (0, 3, 6)):
            return y
    return None


def pad(r, x, y, z, half=24):
    """Flat grass pad of (2 half)² blocks, air over the 24 blocks above."""
    for x0 in range(x - half, x + half, 16):
        for z0 in range(z - half, z + half, 16):
            x1, z1 = min(x0 + 15, x + half - 1), min(z0 + 15, z + half - 1)
            r.cmd(f"fill {x0} {y - 3} {z0} {x1} {y - 1} {z1} minecraft:dirt")
            r.cmd(f"fill {x0} {y} {z0} {x1} {y} {z1} minecraft:grass_block")
            r.cmd(f"fill {x0} {y + 1} {z0} {x1} {y + 12} {z1} minecraft:air")
            r.cmd(f"fill {x0} {y + 13} {z0} {x1} {y + 24} {z1} minecraft:air")


def pregen_zones(found, sites_xz, radius=16, center="camera"):
    """Zones generated in advance: (dimension, chunk x, chunk z, radius in chunks).

    Plains: radius 33 (distance 32 + 1). Elsewhere, `radius` around the chunk of the scene's
    camera (position set further down in `prepare`, same offsets) or, `center="site"`, around the
    point found (old model: radius 13, the crowd and redstone cameras were 22-26 blocks away).
    """
    ow, ne = "minecraft:overworld", "minecraft:the_nether"
    px, _, pz = found["plains"]
    sites = {k: (v[0], v[2]) for k, v in found.items() if k != "plains"}
    sites.update(sites_xz)
    # Horizontal offset of the camera from the scene's point (see `prepare`).
    cam = {"village": (0, -20), "crowd": (0, -22), "redstone": (0, -26)}
    zones = [(ow, px >> 4, pz >> 4, 33)]
    for k, (x, z) in sites.items():
        dx, dz = cam.get(k, (0, 0)) if center == "camera" else (0, 0)
        zones.append((ne if k in NETHER_VIEWS else ow, (x + dx) >> 4, (z + dz) >> 4, radius))
    return zones


# Located views that are in the Nether (the others are in the overworld).
NETHER_VIEWS = {"nether", "fortress"}


def underground_view(r, dim, x, z, ys, fallback_y):
    """Camera in the first air pocket of `ys` at (x, z), or in a pocket dug at `fallback_y`."""
    y = air_pocket(r, dim, x, z, ys)
    if y is None:
        y = fallback_y
        r.cmd(f"execute in {dim} run fill {x - 2} {y} {z - 1} {x + 2} {y + 3} {z + 8} minecraft:air")
    return [x + 0.5, y + 0.0, z + 0.5]


def prepare(a):
    srv = os.path.join(PERF, "server")
    if os.path.exists(srv):
        shutil.rmtree(srv)
    os.makedirs(srv)
    shutil.copy(a.server_jar, os.path.join(srv, "server.jar"))
    props = {
        "server-ip": "127.0.0.1", "server-port": SRV_PORT, "enable-rcon": "true", "rcon.port": RCON_PORT,
        "rcon.password": RCON_PASS, "level-seed": a.seed, "level-name": "world", "online-mode": "false",
        "gamemode": "creative", "spawn-protection": 0, "view-distance": 10, "simulation-distance": 10,
        "white-list": "true", "enforce-whitelist": "true", "max-tick-time": -1, "sync-chunk-writes": "false",
        "pause-when-empty-seconds": 0, "enable-status": "false",
    }
    with open(os.path.join(srv, "server.properties"), "w", encoding="utf-8", newline="\n") as f:
        f.writelines(f"{k}={v}\n" for k, v in props.items())
    with open(os.path.join(srv, "eula.txt"), "w", encoding="utf-8", newline="\n") as f:
        f.write("eula=true\n")
    log = open(os.path.join(srv, "console.log"), "w", encoding="utf-8")
    p = subprocess.Popen([a.java, "-Xmx6G", "-jar", "server.jar", "nogui"], cwd=srv, stdin=subprocess.PIPE,
                         stdout=log, stderr=subprocess.STDOUT, creationflags=NO_WINDOW)
    try:
        r = None
        for _ in range(300):
            time.sleep(1)
            try:
                r = Rcon(RCON_PORT, RCON_PASS)
                break
            except OSError:
                if p.poll() is not None:
                    raise SystemExit("the server stopped (see console.log)")
        if r is None:
            raise SystemExit("RCON unreachable")
        print("server ready")
        for c in ["gamerule advance_time false", "gamerule doDaylightCycle false", "gamerule advance_weather false",
                  "gamerule doWeatherCycle false", "gamerule spawn_mobs false", "gamerule doMobSpawning false",
                  "gamerule spawn_patrols false", "gamerule spawn_wandering_traders false", "time set 6000", "weather clear"]:
            print(f"  {c} : {r.cmd(c)[:80]}")
        ow, ne = "minecraft:overworld", "minecraft:the_nether"
        views = {"plains": find_view(r, "minecraft:plains"), "forest": find_view(r, "minecraft:dark_forest")}
        print("views:", views)
        found = {
            "plains": (views["plains"][0], None, views["plains"][1]),
            "forest": (views["forest"][0], None, views["forest"][1]),
            "village": locate(r, "structure #minecraft:village"),
            "ocean": locate(r, "biome minecraft:deep_ocean"),
            "cave": locate(r, "biome minecraft:lush_caves"),
            "nether": locate(r, "biome minecraft:nether_wastes", ne),
            # Series 3: spawners and vaults of a trial chamber, natural spawning of a fortress.
            "trial": locate(r, "structure minecraft:trial_chambers"),
            "fortress": locate(r, "structure minecraft:fortress", ne),
        }
        print("locations:", found)
        px, _, pz = found["plains"]
        sites_xz = {"crowd": (px + 640, pz), "redstone": (px + 640, pz + 640)}
        # Centred on the camera, radius ≥ distance 12 + 2 (the 2026-10-19 model, centred on the site
        # at radius 13, left chunks to generate at the start of crowd and redstone).
        zones = pregen_zones(found, sites_xz, a.pregen_radius, a.pregen_center)
        t0 = time.time()
        for z in zones:
            forceload(r, *z)
        for z in zones:
            wait_loaded(r, *z)
        print(f"zones generated in {time.time() - t0:.0f} s")
        scenes = {}
        # Plains and forest: view that stays inside the biome, 6 (3) blocks above the ground (leaves).
        h = surface(r, px, pz)
        scenes["plains"] = {"dim": ow, "pos": [px + 0.5, h + 6, pz + 0.5], "rot": [views["plains"][2], 15.0]}
        x, _, z = found["forest"]
        scenes["forest"] = {"dim": ow, "pos": [x + 0.5, surface(r, x, z) + 3, z + 0.5], "rot": [views["forest"][2], 10.0]}
        x, _, z = found["village"]
        scenes["village"] = {"dim": ow, "pos": [x + 0.5, surface(r, x, z, "motion_blocking_no_leaves") + 14, z - 20 + 0.5], "rot": [0.0, 30.0]}
        x, _, z = found["ocean"]
        scenes["ocean"] = {"dim": ow, "pos": [x + 0.5, 72.0, z + 0.5], "rot": [0.0, 10.0]}
        x, y, z = found["cave"]
        gy = air_pocket(r, ow, x, z, [y + d for d in (0, 1, -1, 2, -2, 3, -3, 4, -4, 5, -5, 6, -6, 8, -8, 10, -10, 12, -12)])
        if gy is None:
            gy = y
            r.cmd(f"fill {x - 2} {y} {z - 1} {x + 2} {y + 3} {z + 8} minecraft:cave_air")
        scenes["cave"] = {"dim": ow, "pos": [x + 0.5, gy + 0.0, z + 0.5], "rot": [0.0, 0.0]}
        x, _, z = found["nether"]
        ny = air_pocket(r, ne, x, z, list(range(34, 110)))
        if ny is None:
            ny = 64
            r.cmd(f"execute in {ne} run fill {x - 2} {ny} {z - 1} {x + 2} {ny + 3} {z + 8} minecraft:air")
        scenes["nether"] = {"dim": ne, "pos": [x + 0.5, ny + 0.0, z + 0.5], "rot": [0.0, 5.0]}
        x, _, z = found["trial"]
        scenes["trial"] = {"dim": ow, "pos": underground_view(r, ow, x, z, list(range(-40, -10)) + list(range(-56, -40)), -30),
                           "rot": [0.0, 10.0]}
        x, _, z = found["fortress"]
        scenes["fortress"] = {"dim": ne, "pos": underground_view(r, ne, x, z, list(range(64, 90)) + list(range(40, 64)), 70),
                              "rot": [0.0, 10.0]}
        sites = {}
        for k, (x, z) in sites_xz.items():
            y = surface(r, x, z, "motion_blocking_no_leaves")
            pad(r, x, y, z)
            sites[k] = (x, y, z)
        scenes["crowd"] = {"dim": ow, "pos": [sites["crowd"][0] + 0.5, sites["crowd"][1] + 9, sites["crowd"][2] - 22 + 0.5], "rot": [0.0, 25.0]}
        scenes["redstone"] = {"dim": ow, "pos": [sites["redstone"][0] + 0.5, sites["redstone"][1] + 10, sites["redstone"][2] - 26 + 0.5], "rot": [0.0, 25.0]}
        for z in zones:
            forceload(r, *z, add=False)
        r.cmd("execute in minecraft:overworld run forceload remove all")
        r.cmd(f"execute in {ne} run forceload remove all")
        print("save:", r.cmd("save-all flush")[:80])
        r.cmd("stop")
        p.wait(timeout=180)
    finally:
        if p.poll() is None:
            p.kill()
        log.close()
    world = os.path.join(srv, "world")
    model = os.path.join(PERF, "model")
    if os.path.exists(model):
        shutil.rmtree(model)
    shutil.copytree(world, model, ignore=shutil.ignore_patterns("session.lock"))
    write_pack(model, sites)
    write_maps(model)
    level = nbt.read_dat(os.path.join(model, "level.dat"))
    d = level["Data"]
    d["allowCommands"] = (nbt.BYTE, 1)
    d["GameType"] = (nbt.INT, 1)
    dp = d.get("DataPacks")
    if isinstance(dp, dict) and isinstance(dp.get("Enabled"), nbt.List):
        if not any(v[1] == "file/perf" for v in dp["Enabled"].items):
            dp["Enabled"].items.append((nbt.STRING, "file/perf"))
    nbt.write_dat(os.path.join(model, "level.dat"), level)
    with open(os.path.join(PERF, "scenes.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump({"seed": a.seed, "found": found, "sites": sites, "scenes": scenes}, f, indent=1)
    print("model ready:", model)


# Fresh-world paths located by `seed-model` (file of the model): biome -> located start.
FRESH_PATHS_FILE = "fresh-paths.json"
LOCATED_FRESH_BIOMES = {
    # Mushrooms marked for the light post-processing (swamp, Nether wastes, old-growth taiga),
    # eroded surface (badlands).
    "swamp": "minecraft:swamp", "badlands": "minecraft:badlands", "taiga": "minecraft:old_growth_spruce_taiga",
}


def locate_fresh_paths(r):
    """Start of a 192-block flight that stays inside each biome of `LOCATED_FRESH_BIOMES`
    (20 blocks above the ground), and a Nether wastes flight at y 72 (through the caverns)."""
    ow, ne = "minecraft:overworld", "minecraft:the_nether"
    paths = {}
    for name, biome in LOCATED_FRESH_BIOMES.items():
        x, z, yaw = find_view(r, biome)
        forceload(r, ow, x >> 4, z >> 4, 0)
        wait_loaded(r, ow, x >> 4, z >> 4, 0)
        y = surface(r, x, z, "motion_blocking_no_leaves")
        forceload(r, ow, x >> 4, z >> 4, 0, add=False)
        paths[name] = {"dim": ow, "pos": [x + 0.5, y + 20.0, z + 0.5], "rot": [yaw, 25.0]}
    x, _, z = locate(r, "biome minecraft:nether_wastes", ne)
    paths["nether"] = {"dim": ne, "pos": [x + 0.5, 72.0, z + 0.5], "rot": [0.0, 10.0]}
    return paths


def prepare_seed(a):
    """Model `a.name` for a given seed, game default settings (creative, spawning and day cycle
    active): the official server creates the world then stops; the scenes with `world: fresh`
    then generate all of it (fresh-world flights, seed 1234)."""
    srv = os.path.join(PERF, f"server-{a.seed}")
    if os.path.exists(srv):
        shutil.rmtree(srv)
    os.makedirs(srv)
    shutil.copy(a.server_jar, os.path.join(srv, "server.jar"))
    props = {
        "server-ip": "127.0.0.1", "server-port": SRV_PORT, "enable-rcon": "true", "rcon.port": RCON_PORT,
        "rcon.password": RCON_PASS, "level-seed": a.seed, "level-name": "world", "online-mode": "false",
        "gamemode": "creative", "white-list": "true", "enforce-whitelist": "true", "enable-status": "false",
    }
    with open(os.path.join(srv, "server.properties"), "w", encoding="utf-8", newline="\n") as f:
        f.writelines(f"{k}={v}\n" for k, v in props.items())
    with open(os.path.join(srv, "eula.txt"), "w", encoding="utf-8", newline="\n") as f:
        f.write("eula=true\n")
    log = open(os.path.join(srv, "console.log"), "w", encoding="utf-8")
    p = subprocess.Popen([a.java, "-Xmx4G", "-jar", "server.jar", "nogui"], cwd=srv, stdin=subprocess.PIPE,
                         stdout=log, stderr=subprocess.STDOUT, creationflags=NO_WINDOW)
    try:
        r = None
        for _ in range(300):
            time.sleep(1)
            try:
                r = Rcon(RCON_PORT, RCON_PASS)
                break
            except OSError:
                if p.poll() is not None:
                    raise SystemExit("the server stopped (see console.log)")
        if r is None:
            raise SystemExit("RCON unreachable")
        paths = locate_fresh_paths(r)
        print("fresh-world paths:", paths)
        print("save:", r.cmd("save-all flush")[:80])
        r.cmd("stop")
        p.wait(timeout=180)
    finally:
        if p.poll() is None:
            p.kill()
        log.close()
    model = os.path.join(PERF, a.name)
    if os.path.exists(model):
        shutil.rmtree(model)
    shutil.copytree(os.path.join(srv, "world"), model,
                    ignore=shutil.ignore_patterns("session.lock", "region", "entities", "poi"))
    with open(os.path.join(model, FRESH_PATHS_FILE), "w", encoding="utf-8", newline="\n") as f:
        json.dump(paths, f, indent=1)
    write_pack(model, {"crowd": (0, 64, 0), "redstone": (0, 64, 0)})
    level = nbt.read_dat(os.path.join(model, "level.dat"))
    d = level["Data"]
    d["allowCommands"] = (nbt.BYTE, 1)
    d["GameType"] = (nbt.INT, 1)
    dp = d.get("DataPacks")
    if isinstance(dp, dict) and isinstance(dp.get("Enabled"), nbt.List):
        if not any(v[1] == "file/perf" for v in dp["Enabled"].items):
            dp["Enabled"].items.append((nbt.STRING, "file/perf"))
    nbt.write_dat(os.path.join(model, "level.dat"), level)
    print("model ready:", model)


# --- Scenes -------------------------------------------------------------------------------

# Each scene: viewpoint (key of scenes.json), render distance, duration, start of the
# measurement window, optional `RUSTCRAFT_CMDTEST` / `RUSTCRAFT_KEYTEST` steps.
SCENES = {
    "plains12": {"at": "plains", "rd": 12, "secs": 45, "from": 25},
    "plains32": {"at": "plains", "rd": 32, "secs": 70, "from": 45},
    "forest": {"at": "forest", "rd": 12, "secs": 45, "from": 25},
    "village": {"at": "village", "rd": 12, "secs": 45, "from": 25},
    "nether": {"at": "nether", "rd": 12, "secs": 45, "from": 25},
    "ocean": {"at": "ocean", "rd": 12, "secs": 45, "from": 25},
    "cave": {"at": "cave", "rd": 12, "secs": 45, "from": 25},
    "crowd": {"at": "crowd", "rd": 12, "secs": 50, "from": 30, "cmd": "w100|/|tfunction perf:mobs|enter"},
    "redstone": {"at": "redstone", "rd": 12, "secs": 50, "from": 30, "cmd": "w100|/|tfunction perf:redstone|enter"},
    "inventory": {"at": "plains", "rd": 12, "secs": 45, "from": 25, "keys": "300:+:key.keyboard.e,302:-:key.keyboard.e"},
    "pause": {"at": "plains", "rd": 12, "secs": 45, "from": 25, "menu": "pause"},
    "flight": {"at": "plains", "rd": 12, "secs": 50, "from": 22, "cmd": "w300|/|ttag @s add perf_fly|enter"},
    # Building: blocks placed and dug at 5 s, straddling column and section edges
    # (light, water, glass, leaves, hole down to the underground); end frame compared between two
    # versions of the meshing (must be identical).
    "building": {"at": "plains", "rd": 12, "secs": 30, "from": 20, "cmd": "|".join([
        "w100", "/", "tfill ~-3 78 ~-17 ~3 81 ~-15 glowstone", "enter",
        "w10", "/", "tfill ~-8 72 ~-20 ~-4 76 ~-20 glass", "enter",
        "w10", "/", "tsetblock ~2 80 ~-14 water", "enter",
        "w10", "/", "tfill ~4 73 ~-12 ~8 73 ~-10 oak_leaves[persistent=true]", "enter",
        "w10", "/", "tfill ~-1 60 ~-24 ~1 75 ~-24 air", "enter",
        "w10", "/", "tfill ~-9 70 ~-13 ~-5 70 ~-9 sea_lantern", "enter",
    ])},
}


def cmd_of(*steps):
    """`RUSTCRAFT_CMDTEST` script: each command preceded by its wait in ticks."""
    out = []
    for wait, command in steps:
        out += [f"w{wait}", "/", f"t{command}", "enter"]
    return "|".join(out)


FUNCTION = lambda name, wait=100: cmd_of((wait, f"function perf:{name}"))  # noqa: E731
RAIN = (100, "weather rain")
FLIGHT = (300, "tag @s add perf_fly")
STANDARD = {"rd": 12, "secs": 45, "from": 25}
CROWD = {"at": "crowd", "rd": 12, "secs": 50, "from": 30}
# 26.3 syntax: `publish [allowCommands] [port]`, no game mode: guests join in the default one.
PUBLISH = ((60, "defaultgamemode creative"), (1, "publish false 25599"))

# Scenes of optimisation series 2. Extra keys: `cache` (cold:
# empty resource cache; first: fresh exe + empty cache; fresh: fresh exe + empty cache folder,
# data.bin redone), `world` (fresh: copy without regions, everything is generated), `opts`
# (options.txt), `env`, `site` + `d` (camera = site point + offset), `title` (title screen, no
# world), `guests` (windowless LAN guests), `sound` (named audio device, `--sound-device`).
SCENES.update({
    # Startup: deliberately cold launches (never left out of the medians).
    "cold-resources": {"at": "plains", **STANDARD, "cache": "cold"},
    "first-launch": {"at": "plains", **STANDARD, "cache": "first"},
    "fresh-install": {"at": "plains", "rd": 12, "secs": 60, "from": 40, "cache": "fresh"},
    "named-sound-device": {"at": "plains", **STANDARD, "sound": True},
    # Interface.
    "pause-enchanted": {"at": "plains", **STANDARD, "menu": "pause",
                        "cmd": cmd_of((100, "give @s minecraft:diamond_sword[enchantments={sharpness:1}]"))},
    "inventory-enchanted": {"at": "plains", **STANDARD, "env": {"RUSTCRAFT_CREATIVETEST": "search:enchanted"}},
    "title": {"at": "plains", "rd": 12, "secs": 30, "from": 12, "title": True, "menu": "title",
              "env": {"RUSTCRAFT_MENUCLICKS": "~560:m:0:0,quit"}},
    # Chat: one message per second over the whole scene, then 100 messages in a row (one per 3 ticks).
    "chat-spread": {"at": "plains", "rd": 12, "secs": 50, "from": 25,
                    "cmd": "w100|" + "|".join(f"o|tmessage {i}|enter|w17" for i in range(48))},
    "chat-burst": {"at": "plains", "rd": 12, "secs": 50, "from": 25,
                   "cmd": "w440|" + "|".join(f"o|tburst {i}|enter" for i in range(100))},
    # Block entities and texts.
    "signs": {**CROWD, "cmd": FUNCTION("signs")},
    "block-entities": {"at": "redstone", "rd": 12, "secs": 50, "from": 30, "cmd": FUNCTION("block_entities")},
    # Movement and weather.
    # Rotation alone over the whole scene: a single regime in the window (walking is ocean-walk).
    "plains32-rotation": {"at": "plains", "rd": 32, "secs": 80, "from": 45, "cmd": cmd_of((100, "tag @s add perf_turn"))},
    # The camera turned every frame by the client itself, as a steady mouse (90 degrees per second):
    # the server's teleports above turn it 20 times a second, so per-frame view costs hide between.
    "plains32-mouse-turn": {"at": "plains", "rd": 32, "secs": 80, "from": 45, "env": {"RUSTCRAFT_BENCH_TURN": "90"}},
    "ocean-walk": {"at": "ocean", **STANDARD, "cmd": cmd_of((100, "tag @s add perf_walk"))},
    "rain": {"at": "plains", **STANDARD, "cmd": cmd_of(RAIN)},
    "rain-forest": {"at": "forest", **STANDARD, "cmd": cmd_of(RAIN)},
    "rain-redstone": {"at": "redstone", "rd": 12, "secs": 50, "from": 30, "cmd": cmd_of(RAIN, (10, "function perf:redstone"))},
    "rain-flight": {"at": "plains", "rd": 12, "secs": 50, "from": 22, "cmd": cmd_of((100, "weather rain"), (200, "tag @s add perf_fly"))},
    "flight-clouds128": {"at": "plains", "rd": 12, "secs": 50, "from": 22, "cmd": cmd_of(FLIGHT), "opts": {"cloudRange": 128}},
    # Particles and explosions.
    **{f"particles-{n}": {"at": "plains", **STANDARD, "env": {"RUSTCRAFT_PARTICLES_BENCH": str(n)}} for n in (1000, 4000, 16000)},
    # Priming around 27 s (entering the game ≈ 1 s + 520 ticks): 1st explosion around 31 s, then every 5 s.
    "tnt": {**CROWD, "cmd": FUNCTION("tnt", 520)},
    # 1,728 TNT blocks set off at 27 s: the chain runs inside the measured window (30 to 50 s).
    "tnt-heap": {**CROWD, "cmd": FUNCTION("tnt_heap", 520)},
    "fireworks": {**CROWD, "cmd": FUNCTION("fireworks")},
    # Mobs.
    "crowd-close-up": {"site": "crowd", "d": [0.5, 2.0, -6.5], "rot": [0.0, 10.0], "rd": 12, "secs": 50, "from": 30,
                       "cmd": FUNCTION("mobs")},
    "crowd-1000": {**CROWD, "cmd": FUNCTION("mobs1000")},
    "equipped-humanoids": {**CROWD, "cmd": FUNCTION("humanoids")},
    "mobs-keyframes": {**CROWD, "cmd": FUNCTION("keyframes")},
    "dropped-items": {**CROWD, "cmd": FUNCTION("items")},
    "glowing": {**CROWD, "cmd": FUNCTION("glowing")},
    "name-tags": {**CROWD, "cmd": FUNCTION("names")},
    # Frame: vertical sync, frame-rate caps.
    "vsync": {"at": "plains", **STANDARD, "env": {"RUSTCRAFT_VSYNC": "1"}, "opts": {"enableVsync": "true"}},
    # `RUSTCRAFT_FPSLIMIT=1`: a launch with a capture ignores the frame limit otherwise, and these
    # scenes ran unlimited. Read the frames per second against the limit asked (60.0 for 60).
    **{f"fps{n}": {"at": "plains", **STANDARD, "env": {"RUSTCRAFT_FPSLIMIT": "1"}, "opts": {"maxFps": n}}
       for n in (60, 120, 144)},
    # Generation alone: world without regions, all cores then a single thread.
    "generation": {"at": "plains", "rd": 12, "secs": 40, "from": 20, "world": "fresh"},
    "generation-1thread": {"at": "plains", "rd": 12, "secs": 60, "from": 40, "world": "fresh", "env": {"RUSTCRAFT_THREADS": "1"}},
    # Integrated server: local network (host + 1, 2, 4 windowless guests), close-together saves.
    **{f"lan-{n}": {"at": "plains", "rd": 12, "secs": 50, "from": 25, "guests": n,
                    "cmd": cmd_of(*PUBLISH), "env": {"RUSTCRAFT_LAN_AUTH": "0"}} for n in (1, 2, 4)},
    "save": {"at": "plains", "rd": 12, "secs": 60, "from": 22, "cmd": cmd_of(FLIGHT),
             "env": {"RUSTCRAFT_AUTOSAVE_TICKS": "200"}},
})
# Fresh-world flight paths (seed 1234, terrain generated during the flight): start, end
# (192 blocks along +z or +x), pitch; tp at the start, noon, 20 s of loading, 12 s of flight
# at 16 blocks/s. The flight covers 20 to 32 s after entering the game (`[perf] in game at … ms`
# of the log): a compared analysis realigns its window on that rather than on `from`.
FRESH_PATHS = {
    "village": ((-960.0, 125.0, 80.0), (-960.0, 125.0, 272.0), 35.0),
    "forest": ((-1100.0, 120.0, -560.0), (-1100.0, 120.0, -368.0), 25.0),
    "ocean": ((-300.0, 85.0, -420.0), (-300.0, 85.0, -228.0), 10.0),
    "mountain": ((560.0, 260.0, 860.0), (752.0, 260.0, 860.0), 30.0),
}
FRESH_FLIGHT = cmd_of((20, "time set noon"), (380, "tag @s add perf_glide"), (240, "tag @s remove perf_glide"))
for _n, (_a, _b, _pitch) in FRESH_PATHS.items():
    _yaw = 0.0 if _b[2] != _a[2] else -90.0
    for _rd in (8, 16, 32):
        # Generation log kept on (launch default `RUSTCRAFT_GENLOG=1`): the generation figures of
        # the flight come from it. A `RUSTCRAFT_GENLOG: "0"` here used to leave it on all the
        # same (the client only tests that the variable exists); `--exe a.exe|-RUSTCRAFT_GENLOG`
        # removes it to price the log.
        SCENES[f"fresh-{_n}-rd{_rd}"] = {
            "pos": list(_a), "rot": [_yaw, _pitch], "model": "model-1234", "world": "fresh", "rd": _rd,
            "secs": 40, "from": 22, "cmd": FRESH_FLIGHT,
        }

# Scenes of optimisation series 3. Extra keys: `fresh_path` (start located by `seed-model`,
# file `fresh-paths.json` of the model), `dim` (with `pos`), `rot` (with `at`: camera turned),
# `survival` (player in survival mode, no flight), `servers` (`servers.dat` + as many fake
# status servers), `worlds` (extra worlds in the list), `packs` (zipped resource packs).
# Text values may name a position of `scenes.json`: `{view:NAME}` = "x y z" of the camera,
# `{site:NAME:dx:dy:dz}` = "x,y,z" of a pad block (the `RUSTCRAFT_INVTEST` format).
LAN = {"RUSTCRAFT_LAN_AUTH": "0"}
# Standing on the crowd pad 2.5 blocks north of the block at (site x, y + 1, z - 18).
ON_CROWD_PAD = {"site": "crowd", "d": [0.5, 1.0, -20.5], "rd": 12, "secs": 50, "from": 30}
CHEST_ON_PAD = "{site:crowd:0:1:-18}"


def right_clicks(first_tick, n, every=10):
    """`RUSTCRAFT_KEYTEST` script: `n` right clicks (press, release a tick later)."""
    return ",".join(f"{t}:+:key.mouse.right,{t + 1}:-:key.mouse.right" for t in range(first_tick, first_tick + n * every, every))


def crafting_clicks(rounds=4):
    """`RUSTCRAFT_INVCLICKS` of the crafting table: for each hotbar slot, take its logs, put
    them in the grid, shift-click the result (slot:button:mode, 0 = pick up, 1 = quick move)."""
    return ",".join(f"{37 + k}:0:0,1:0:0,0:0:1" for _ in range(rounds) for k in range(9))


# Title-screen clicks (GUI units at 1600 × 900, scale 3: 533 × 300).
TITLE_SINGLEPLAYER, TITLE_MULTIPLAYER = "266:133", "266:157"
WORLD_LIST_FIRST, WORLD_LIST_CREATE = "266:69", "345:258"
CREATE_WORLD_TAB, CREATE_SEED_FIELD, CREATE_BUTTON = "266:12", "265:96", "187:284"
PAUSE_ADVANCEMENTS, PAUSE_STATISTICS = "213:117", "319:117"
OFF_WORLD = {"at": "plains", "rd": 12, "title": True, "menu": "title"}

SCENES.update({
    # Generation (C1): fresh worlds rich in marked mushrooms or eroded surface; first uses.
    **{f"fresh-{b}-rd16": {"fresh_path": b, "model": "model-1234", "world": "fresh", "rd": 16, "secs": 40, "from": 22,
                           "cmd": FRESH_FLIGHT} for b in ("nether", "swamp", "badlands", "taiga")},
    "first-locate": {"at": "plains", **STANDARD, "cmd": cmd_of((600, "locate structure #minecraft:village"))},
    # `/locate biome` of a rare biome (longest tick of the window): pool idle, then during a flight
    # (generation running on the pool).
    "locate-biome": {"at": "plains", **STANDARD, "cmd": cmd_of((600, "locate biome minecraft:mushroom_fields"))},
    "flight-locate-biome": {"at": "plains", "rd": 12, "secs": 50, "from": 22,
                            "cmd": cmd_of(FLIGHT, (300, "locate biome minecraft:mushroom_fields"))},
    "first-place": {"at": "plains", **STANDARD, "cmd": cmd_of((600, "execute at @s positioned ~ ~ ~8 positioned over "
                                                                    "motion_blocking_no_leaves run place feature minecraft:oak"))},
    # Looking straight down at a sapling on a floating grass block, 16 uses of bone meal.
    "first-bonemeal": {"at": "plains", "rot": [0.0, 90.0], **STANDARD, "keys": right_clicks(600, 16),
                       "cmd": cmd_of((100, "execute at @s run setblock ~ ~-3 ~ minecraft:grass_block"),
                                     (1, "execute at @s run setblock ~ ~-2 ~ minecraft:oak_sapling"),
                                     (1, "item replace entity @s weapon.mainhand with minecraft:bone_meal 64"))},
    "first-ender-eye": {"at": "plains", **STANDARD, "keys": right_clicks(600, 1),
                        "cmd": cmd_of((100, "item replace entity @s weapon.mainhand with minecraft:ender_eye 16"))},
    "first-nether-trip": {"at": "plains", "rd": 12, "secs": 50, "from": 25,
                          "cmd": cmd_of((600, "execute in minecraft:the_nether run tp @s {view:nether}"),
                                        (200, "execute in minecraft:overworld run tp @s {view:plains}"))},
    # Rendering (C3): culling, maps, arenas, improved transparency.
    "maps-1": {"site": "crowd", "d": [0.5, 11.0, -14.5], "rot": [0.0, 0.0], "rd": 12, "secs": 50, "from": 30,
               "cmd": FUNCTION("maps_wall")},
    "hidden-1": {"site": "crowd", "d": [0.5, 6.0, -23.5], "rot": [0.0, 15.0], "rd": 12, "secs": 50, "from": 30,
                 "cmd": FUNCTION("hidden_cows")},
    "flight-rotation-32": {"at": "plains", "rd": 32, "secs": 90, "from": 25, "cmd": cmd_of(
        (100, "tag @s add perf_turn"), (800, "tp @s ~512 ~ ~"), (200, "tp @s ~-512 ~ ~"), (200, "tp @s ~ ~ ~512"),
        (200, "tp @s ~ ~ ~-512"))},
    **{f"{v}-transparency": {"at": v, **STANDARD, "opts": {"improvedTransparency": "true"}} for v in ("ocean", "forest", "village")},
    # Interface (C4).
    "survival-hud": {"at": "ocean", **STANDARD, "survival": True, "cmd": FUNCTION("survival_hud")},
    "hud-busy": {"at": "plains", **STANDARD, "cmd": FUNCTION("hud_busy")},
    "inventory-enchanted-36": {"at": "plains", **STANDARD, "cmd": FUNCTION("enchanted_36"),
                               "env": {"RUSTCRAFT_CREATIVETEST": "inventory"}},
    # Right click on the librarian at 21 s, cursor held over the first offer.
    "trade": {**ON_CROWD_PAD, "rot": [0.0, 0.0], "cmd": FUNCTION("trade"), "keys": right_clicks(420, 1),
              "env": {"RUSTCRAFT_INVMOUSE": "177,95"}},
    **{f"tab-{n + 1}": {"at": "plains", "rd": 12, "secs": 55, "from": 30, "guests": n,
                        "cmd": cmd_of(*PUBLISH, (440, "function perf:tab_list")), "env": {**LAN, "RUSTCRAFT_TABTEST": "1"}}
       for n in (19, 79)},
    "chat-flood": {"at": "plains", **STANDARD, "cmd": FUNCTION("chat_flood")},
    "creative-search": {"at": "plains", **STANDARD, "env": {"RUSTCRAFT_CREATIVETEST": "search:diamond"}},
    "creative-tab": {"at": "plains", **STANDARD, "env": {"RUSTCRAFT_CREATIVETEST": "building_blocks"}},
    # Saved hotbars tab: its rows are kept between ticks (C4-M21).
    "creative-hotbars": {"at": "plains", **STANDARD, "env": {"RUSTCRAFT_CREATIVETEST": "hotbar"}},
    "recipe-book": {**ON_CROWD_PAD, "rot": [0.0, 10.0], "survival": True, "cmd": cmd_of((100, "recipe give @s *")),
                    "env": {"RUSTCRAFT_INVTEST": "e", "RUSTCRAFT_BOOKTEST": "open"}},
    # Command typed without Enter: the item suggestions stay open.
    "cmd-give": {"at": "plains", **STANDARD, "cmd": "w40|/|tgive @s |w200"},
    "stats-screen": {"at": "plains", **STANDARD, "menu": "pause", "env": {"RUSTCRAFT_MENUCLICKS": PAUSE_STATISTICS}},
    "advancements": {"at": "plains", **STANDARD, "menu": "pause", "env": {"RUSTCRAFT_MENUCLICKS": PAUSE_ADVANCEMENTS}},
    "f3-open": {"at": "plains", **STANDARD, "env": {"RUSTCRAFT_DEBUGTEST": "f3"}},
    "team-crowd": {**CROWD, "cmd": FUNCTION("team_crowd")},
    "team-crowd-named": {**CROWD, "cmd": FUNCTION("team_crowd_named")},
    "signs-chests": {**CROWD, "cmd": FUNCTION("signs_chests")},
    "displays-1": {**CROWD, "cmd": FUNCTION("displays")},
    "structure-show-air": {**CROWD, "cmd": FUNCTION("structure_air")},
    "packs-20": {"at": "plains", "rd": 12, "secs": 50, "from": 24, "packs": 20, "env": {"RUSTCRAFT_PACKSCREEN": "26"}},
    "reload-f3t": {"at": "plains", "rd": 12, "secs": 50, "from": 24, "packs": 20, "env": {"RUSTCRAFT_RELOADTEST": "28"}},
    # Outside a world (title screen driven by clicks; `~N:` = N ticks before the next step).
    "multiplayer": {**OFF_WORLD, "secs": 50, "from": 20, "servers": 6,
                    "env": {"RUSTCRAFT_MENUCLICKS": f"~200:m:0:0,~800:{TITLE_MULTIPLAYER},quit"}},
    "world-list": {**OFF_WORLD, "secs": 50, "from": 20, "worlds": 20,
                   "env": {"RUSTCRAFT_MENUCLICKS": f"~200:m:0:0,~800:{TITLE_SINGLEPLAYER},quit"}},
    "world-create": {**OFF_WORLD, "secs": 50, "from": 20,
                     "env": {"RUSTCRAFT_MENUCLICKS": f"~200:m:0:0,~60:{TITLE_SINGLEPLAYER},~800:{WORLD_LIST_CREATE},quit"}},
    # Disk: new world (seed 1234) up to the game, world opened from the list, long flight then save.
    "world-create-play": {**OFF_WORLD, "secs": 75, "from": 40, "env": {"RUSTCRAFT_MENUCLICKS": ",".join([
        "~200:m:0:0", f"~60:{TITLE_SINGLEPLAYER}", f"~60:{WORLD_LIST_CREATE}", f"~20:{CREATE_WORLD_TAB}",
        f"~10:{CREATE_SEED_FIELD}", "~10:t:1234", f"~1200:{CREATE_BUTTON}", "quit"])}},
    "open-from-list": {**OFF_WORLD, "secs": 60, "from": 30, "env": {"RUSTCRAFT_MENUCLICKS": ",".join([
        "~200:m:0:0", f"~40:{TITLE_SINGLEPLAYER}", f"~1000:dc:{WORLD_LIST_FIRST}", "quit"])}},
    "long-flight-save": {"at": "plains", "rd": 12, "secs": 200, "from": 22, "cmd": cmd_of(FLIGHT),
                         "env": {"RUSTCRAFT_AUTOSAVE_TICKS": "600"}},
    # Integrated server (C5): long flight (server memory over time), block entities and entities.
    "long-flight": {"at": "plains", "rd": 12, "secs": 620, "from": 22, "cmd": cmd_of(FLIGHT)},
    "hoppers-1": {**CROWD, "cmd": FUNCTION("hoppers")},
    "smelter-1": {**CROWD, "cmd": FUNCTION("furnaces")},
    "sculk-1": {**CROWD, "cmd": FUNCTION("sculk")},
    "carts-1": {**CROWD, "cmd": FUNCTION("minecarts")},
    "items-merge-1": {**CROWD, "cmd": FUNCTION("items_merge")},
    "redstone-dense": {"at": "redstone", "rd": 12, "secs": 50, "from": 30, "cmd": FUNCTION("redstone_dense")},
    "tnt-cannon": {**CROWD, "cmd": FUNCTION("tnt_cannon")},
    # Local network (C5): guests brought to the host (`tp Invite0 @s`) or spread out, join in the window.
    "crowd-1000-lan-1": {**CROWD, "guests": 1, "env": LAN,
                         "cmd": cmd_of(*PUBLISH, (40, "function perf:mobs1000"), (340, "tp Invite0 @s"))},
    # TNT primed at 520 ticks, as in `tnt`.
    "tnt-lan-1": {**CROWD, "guests": 1, "env": LAN, "cmd": cmd_of(*PUBLISH, (340, "tp Invite0 @s"), (120, "function perf:tnt"))},
    "lan-4-spread": {"at": "plains", "rd": 12, "secs": 50, "from": 25, "guests": 4, "env": LAN, "cmd": cmd_of(
        *PUBLISH, (340, "execute at @s run tp Invite0 ~160 ~ ~"), (1, "execute at @s run tp Invite1 ~-160 ~ ~"),
        (1, "execute at @s run tp Invite2 ~ ~ ~160"), (1, "execute at @s run tp Invite3 ~ ~ ~-160"))},
    "lan-1-join": {"at": "plains", "rd": 12, "secs": 50, "from": 10, "guests": 1, "env": LAN, "cmd": cmd_of(*PUBLISH)},
    "redstone-published": {"at": "redstone", "rd": 12, "secs": 50, "from": 30,
                           "cmd": cmd_of(*PUBLISH, (40, "function perf:redstone"))},
    # Mobs, villages, commands (C6).
    "village-night": {"at": "village", "rd": 12, "secs": 50, "from": 30,
                      "cmd": cmd_of((100, "time set midnight"), (1, "function perf:village_night"))},
    # Bad omen at 5 s, raid 30 s later, first wave after its 15 s countdown.
    "raid": {"at": "village", "rd": 12, "secs": 110, "from": 55, "cmd": FUNCTION("raid")},
    "crowd-1000-melee": {**CROWD, "cmd": FUNCTION("melee")},
    "crowd-1000-archers": {**ON_CROWD_PAD, "rot": [0.0, 10.0], "survival": True, "cmd": FUNCTION("archers")},
    "pen-babies": {**CROWD, "cmd": FUNCTION("pen_babies")},
    "crowd-villagers": {**CROWD, "cmd": FUNCTION("villagers")},
    # Out of the spawners' reach (60 blocks up) from 1 s to 26 s, then back inside; save every 10 s.
    "trial-chamber": {"at": "trial", "rd": 12, "secs": 60, "from": 15, "survival": True,
                      "env": {"RUSTCRAFT_AUTOSAVE_TICKS": "200"},
                      "cmd": cmd_of((20, "effect give @s minecraft:resistance infinite 4 true"), (1, "tp @s ~ ~60 ~"),
                                    (500, "tp @s ~ ~-60 ~"))},
    # Nests saved every 10 s (`save-all` is a dedicated-server command: refused in singleplayer).
    **{f"bee-nests-{k}": {**CROWD, "env": {"RUSTCRAFT_AUTOSAVE_TICKS": "200"},
                          "cmd": cmd_of((100, setup), (1, "function perf:bee_nests"))}
       for k, setup in (("night", "time set midnight"), ("rain", "weather rain"))},
    # Natural spawning on, every mob killed each tick (the cap never fills).
    "nether-fortress": {"at": "fortress", "rd": 12, "secs": 50, "from": 30,
                        "cmd": cmd_of((100, "gamerule spawn_mobs true"), (1, "tag @s add perf_reap"))},
    "end-fight": {"pos": [0.5, 90.0, -60.5], "dim": "minecraft:the_end", "rot": [0.0, 15.0], "rd": 12, "secs": 60,
                  "from": 35, "cmd": FUNCTION("end_fight", 200)},
    "swamp-bastion": {**CROWD, "cmd": FUNCTION("swamp_bastion")},
    "survival-inventory": {**ON_CROWD_PAD, "rot": [0.0, 30.0], "survival": True, "cmd": FUNCTION("survival_inventory"),
                           "env": {"RUSTCRAFT_INVTEST": CHEST_ON_PAD}},
    "survival-crafting": {**ON_CROWD_PAD, "rot": [0.0, 30.0], "survival": True, "cmd": FUNCTION("crafting"),
                          "env": {"RUSTCRAFT_INVTEST": CHEST_ON_PAD, "RUSTCRAFT_INVCLICKS": crafting_clicks()}},
    "datapack": {**CROWD, "cmd": FUNCTION("datapack")},
    "tnt-first-volley": {**CROWD, "cmd": FUNCTION("tnt_once", 520)},
    # The `tnt` volleys while a world without regions generates at render distance 32: the
    # explosions' helper threads share the cores with generation (read the tick p99 and max).
    "tnt-during-generation": {**CROWD, "rd": 32, "world": "fresh", "cmd": FUNCTION("tnt", 520)},
})

# Named lists for `--scenes @name` (C3-M01: the extended reference pass is `reference-extended`).
SCENE_GROUPS = {
    "reference": ["plains12", "plains32", "forest", "village", "nether", "ocean", "cave", "crowd", "redstone",
                  "crowd-1000", "dropped-items", "equipped-humanoids", "generation", "title", "tnt"],
    "reference-extended": ["plains32-rotation", "plains32-mouse-turn", "flight", "flight-clouds128", "rain", "rain-forest", "rain-redstone",
                           "rain-flight", "fireworks", "particles-1000", "particles-4000", "particles-16000",
                           "mobs-keyframes", "name-tags", "glowing", "signs", "block-entities", "crowd-close-up", "pause",
                           "inventory", "pause-enchanted", "building", "vsync"],
    "generation": ["fresh-nether-rd16", "fresh-swamp-rd16", "fresh-badlands-rd16", "fresh-taiga-rd16",
                   "fresh-village-rd16", "fresh-forest-rd16", "fresh-ocean-rd16", "fresh-mountain-rd16"],
    # Generation measured end to end (profile with `--samply`, then `tools/hot_map.py`): the
    # 1-thread run gives clean self times, the fresh flights steady generation inside the window.
    "generation-zone": ["generation", "generation-1thread", "fresh-village-rd16", "fresh-ocean-rd16",
                        "fresh-mountain-rd16", "crowd"],
    "first-use": ["first-locate", "first-place", "first-bonemeal", "first-ender-eye", "first-nether-trip",
                  "locate-biome", "flight-locate-biome"],
    "render": ["maps-1", "hidden-1", "flight-rotation-32", "ocean-transparency", "forest-transparency",
               "village-transparency"],
    "interface": ["survival-hud", "hud-busy", "inventory-enchanted-36", "trade", "tab-20", "tab-80", "chat-flood",
                  "creative-search", "creative-tab", "creative-hotbars", "recipe-book", "cmd-give", "stats-screen",
                  "advancements", "f3-open", "team-crowd", "team-crowd-named", "signs-chests", "displays-1",
                  "structure-show-air", "packs-20", "reload-f3t"],
    "off-world": ["multiplayer", "world-list", "world-create"],
    "disk": ["world-create-play", "open-from-list", "long-flight-save", "save"],
    "server": ["save", "flight", "long-flight", "hoppers-1", "smelter-1", "sculk-1", "carts-1", "items-merge-1",
               "redstone-dense", "tnt-cannon"],
    "lan": ["crowd-1000-lan-1", "tnt-lan-1", "lan-4-spread", "lan-1-join", "redstone-published", "lan-1", "lan-2", "lan-4"],
    "mobs": ["village-night", "raid", "crowd-1000-melee", "crowd-1000-archers", "pen-babies", "crowd-villagers",
             "trial-chamber", "bee-nests-night", "bee-nests-rain", "nether-fortress", "end-fight", "swamp-bastion",
             "survival-inventory", "survival-crafting", "datapack", "tnt-first-volley"],
}


def expand_scene_names(spec):
    """`--scenes` list: names and `@group`s, in order, each scene once."""
    out = []
    for token in (t for t in spec.split(",") if t):
        if token.startswith("@"):
            if token[1:] not in SCENE_GROUPS:
                raise SystemExit(f"unknown scene group {token}: {', '.join(sorted(SCENE_GROUPS))}")
            names = SCENE_GROUPS[token[1:]]
        else:
            names = [token]
        out += [n for n in names if n not in out]
    return out


COLD_SCENES = {n for n, sc in SCENES.items() if sc.get("cache")}
LAN_PORT = 25599

# Names used before the English renaming, so that data written by older versions of the tool
# stays readable: view keys of `scenes.json`, then scene and metric keys of `resultats.json`.
OLD_VIEW_KEYS = {"plaine": "plains", "foret": "forest", "grotte": "cave", "foule": "crowd"}
OLD_SCENES = {
    "plaine12": "plains12", "plaine32": "plains32", "foret": "forest", "grotte": "cave", "foule": "crowd",
    "inventaire": "inventory", "vol": "flight", "chantier": "building", "ressources-froides": "cold-resources",
    "premier-lancement": "first-launch", "installation-neuve": "fresh-install", "son-nomme": "named-sound-device",
    "pause-enchante": "pause-enchanted", "inventaire-enchante": "inventory-enchanted", "titre": "title",
    "chat-etale": "chat-spread", "chat-rafale": "chat-burst", "panneaux": "signs", "blocs-entites": "block-entities",
    "plaine32-rotation": "plains32-rotation", "ocean-mouvement": "ocean-walk", "pluie": "rain",
    "pluie-foret": "rain-forest", "pluie-redstone": "rain-redstone", "pluie-vol": "rain-flight",
    "vol-nuages128": "flight-clouds128", "particules-1000": "particles-1000", "particules-4000": "particles-4000",
    "particules-16000": "particles-16000", "feux": "fireworks", "foule-gros-plan": "crowd-close-up",
    "foule-1000": "crowd-1000", "humanoides-equipes": "equipped-humanoids", "mobs-images-cles": "mobs-keyframes",
    "objets-au-sol": "dropped-items", "lueur": "glowing", "noms": "name-tags", "synchro": "vsync",
    "generation-1fil": "generation-1thread", "sauvegarde": "save",
}
OLD_METRICS = {
    "colonnes": "columns", "maillees_s": "meshed_s", "maillees_s_max": "meshed_s_max",
    "lumiere_troncons_s": "light_chunks_s", "lumiere_ms_par_troncon": "light_ms_per_chunk",
    "lumiere_troncons_total": "light_chunks_total", "recalculs_s": "relights_s", "recalcul_ms_s": "relight_ms_s",
    "prive_mo": "private_mb", "ws_mo": "ws_mb", "dernier_maillage_s": "last_mesh_s", "maillage_ms_s": "meshing_ms_s",
    "parcours_s": "walks_s", "parcours_ms_s": "walks_ms_s", "cpu_processus_ms_s": "process_cpu_ms_s",
    "reception_charge_ms": "receive_load_ms", "cache_mo": "cache_mb", "sans_base_s": "without_base_s",
    "envoi_ms_s": "upload_ms_s", "envoi_recus_ms_s": "upload_received_ms_s", "envoi_gpu_ms_s": "upload_gpu_ms_s",
    "envoi_colonnes_s": "upload_columns_s", "envoi_mo_s": "upload_mb_s", "sur_place_s": "in_place_s",
    "prep_pool_ms_s": "pool_prep_ms_s", "vram_alloue_mo": "vram_allocated_mb", "vram_reserve_mo": "vram_reserved_mb",
    "tick_moy_ms": "tick_mean_ms", "charge_ms": "load_ms", "en_jeu_ms": "in_game_ms",
    "premiere_image_ms": "first_frame_ms", "generes": "generated", "gen_ms_par_troncon": "gen_ms_per_chunk",
    "gen_premier_lot_ms": "gen_first_batch_ms", "gen_troncons_s": "gen_chunks_s", "tas_mo": "heap_mb",
    "dernier_maillage_ms": "last_mesh_ms", "froid": "cold", "audio_pret_ms": "audio_ready_ms",
    "audio_joint_ms": "audio_joined_ms", "audio_attente_ms": "audio_wait_ms", "ouverture_ms": "open_ms",
    "generateur_ms": "generator_ms", "fermeture_total_ms": "closing_total_ms", "sauvegardes": "saves",
    "save_instantane_ms": "save_snapshot_ms", "save_instantane_max_ms": "save_snapshot_max_ms",
    "images_s": "frames_s", "image_med_ms": "frame_med_ms", "image_p99_ms": "frame_p99_ms",
    "image_p999_ms": "frame_p999_ms", "image_max_ms": "frame_max_ms", "invites_entres": "guests_joined",
    "invite_entree_ms": "guest_join_ms",
}


def load_info():
    """`scenes.json` of the model, the old French view keys mapped to the current ones."""
    with open(os.path.join(PERF, "scenes.json"), encoding="utf-8") as f:
        info = json.load(f)
    for part in ("found", "sites", "scenes"):
        info[part] = {OLD_VIEW_KEYS.get(k, k): v for k, v in info[part].items()}
    return info


def upgrade_results(res):
    """Results saved before the English renaming: scene and metric keys mapped to the current ones."""
    out = {}
    for name, runs in res.items():
        if name == "exe":
            out[name] = runs
            continue
        scene, sep, rest = name.partition("#")
        out[OLD_SCENES.get(scene, scene) + sep + rest] = [
            {"startup:" + k[4:] if k.startswith("dem:") else OLD_METRICS.get(k, k): v for k, v in r.items()}
            for r in runs]
    return out


def view_of(sc, info):
    """Viewpoint of a scene: view from `scenes.json` (`at`, camera turned by `rot`), site + offset
    (`site`, `d`, `rot`), given position (`pos`, `rot`, `dim`), or start located by `seed-model`
    (`fresh_path`)."""
    if "fresh_path" in sc:
        return fresh_paths(sc["model"])[sc["fresh_path"]]
    if "pos" in sc:
        return {"dim": sc.get("dim", "minecraft:overworld"), "pos": sc["pos"], "rot": sc["rot"]}
    if "site" in sc:
        x, y, z = info["sites"][sc["site"]]
        return {"dim": "minecraft:overworld", "pos": [x + sc["d"][0], y + sc["d"][1], z + sc["d"][2]], "rot": sc["rot"]}
    view = info["scenes"][sc["at"]]
    return {**view, "rot": sc["rot"]} if "rot" in sc else view


def fresh_paths(model):
    """Located fresh-world starts of a model (`seed-model`), {} if the model has none."""
    path = os.path.join(PERF, model, FRESH_PATHS_FILE)
    if not os.path.exists(path):
        return {}
    with open(path, encoding="utf-8") as f:
        return json.load(f)


PLACEHOLDER = re.compile(r"\{(view|site):([a-z0-9_-]+)((?::-?\d+){3})?\}")


def resolve_positions(text, info):
    """`{view:NAME}` -> "x y z" of that camera, `{site:NAME:dx:dy:dz}` -> "x,y,z" of the pad
    block at that offset; an unknown name stops the bench (explicit failure)."""
    def sub(m):
        kind, name, offset = m.group(1), m.group(2), m.group(3)
        table = info["scenes" if kind == "view" else "sites"]
        if name not in table:
            raise SystemExit(f"{m.group(0)}: no {kind} {name} in scenes.json (run `prepare` again)")
        if kind == "view":
            return " ".join(f"{v:.2f}" for v in table[name]["pos"])
        d = [int(v) for v in offset[1:].split(":")] if offset else [0, 0, 0]
        return ",".join(str(int(c) + dc) for c, dc in zip(table[name], d))
    return PLACEHOLDER.sub(sub, text)


def scene_with_positions(sc, info):
    """Copy of the scene whose command, keys and variables have their positions resolved."""
    out = dict(sc)
    for k in ("cmd", "keys"):
        if k in out:
            out[k] = resolve_positions(out[k], info)
    out["env"] = {k: resolve_positions(v, info) for k, v in sc.get("env", {}).items()}
    return out


def offline_uuid(name):
    h = bytearray(hashlib.md5(f"OfflinePlayer:{name}".encode()).digest())
    h[6] = (h[6] & 0x0F) | 0x30
    h[8] = (h[8] & 0x3F) | 0x80
    return str(uuidlib.UUID(bytes=bytes(h)))


def stage(run_dir, view, rd, extra_opts=None, fresh_world=False, model=None, survival=False):
    """Fresh game folder: copy of the model, player placed, options fixed. `fresh_world`: without
    the regions (terrain, entities, points of interest) of all dimensions, everything is generated.
    `survival`: player in survival mode (no flight, no instant building, not invulnerable)."""
    if os.path.exists(run_dir):
        shutil.rmtree(run_dir)
    world = os.path.join(run_dir, "saves", "perf")
    ignore = shutil.ignore_patterns("region", "entities", "poi") if fresh_world else None
    shutil.copytree(model or os.path.join(PERF, "model"), world, ignore=ignore)
    pdir = os.path.join(world, "players", "data")
    os.makedirs(pdir, exist_ok=True)
    player = {
        "DataVersion": (nbt.INT, 5023),
        "Pos": nbt.List(nbt.DOUBLE, [(nbt.DOUBLE, float(v)) for v in view["pos"]]),
        "Rotation": nbt.List(nbt.FLOAT, [(nbt.FLOAT, float(v)) for v in view["rot"]]),
        "Motion": nbt.List(nbt.DOUBLE, [(nbt.DOUBLE, 0.0)] * 3),
        "Dimension": (nbt.STRING, view["dim"]),
        "playerGameType": (nbt.INT, 0 if survival else 1),
        "abilities": {
            **{k: (nbt.BYTE, 0 if survival else 1) for k in ("flying", "mayfly", "instabuild", "invulnerable")},
            "mayBuild": (nbt.BYTE, 1),
            "flySpeed": (nbt.FLOAT, 0.05), "walkSpeed": (nbt.FLOAT, 0.1),
        },
    }
    nbt.write_dat(os.path.join(pdir, offline_uuid(NAME) + ".dat"), player)
    # "Custom" preset with the values of "fancy" (game default), except the scene's render
    # distance (a named preset would force it: 16 for "fancy").
    fancy = {
        "graphicsPreset": '"custom"', "biomeBlendRadius": 2, "prioritizeChunkUpdates": 1, "ao": "true",
        "renderClouds": '"true"', "particles": 0, "mipmapLevels": 4, "entityShadows": "true",
        "entityDistanceScaling": 1.0, "menuBackgroundBlurriness": 5, "cloudRange": 64, "cutoutLeaves": "true",
        "improvedTransparency": "false", "weatherRadius": 10, "maxAnisotropyBit": 1, "textureFiltering": 1,
    }
    opts = {
        **fancy, "renderDistance": rd, "simulationDistance": 12, "maxFps": 260, "enableVsync": "false",
        "fullscreen": "false", "onboardAccessibility": "false", "tutorialStep": "none",
        "skipMultiplayerWarning": "true", "joinedFirstServer": "true", "soundCategory_master": "0.0",
        "pauseOnLostFocus": "false", **(extra_opts or {}),
    }
    with open(os.path.join(run_dir, "options.txt"), "w", encoding="utf-8", newline="\n") as f:
        f.writelines(f"{k}:{v}\n" for k, v in opts.items())


# Log line formats written by the client; the bench parses them, so they change together.
STAT = re.compile(r"\[\s*([\d.]+) s\] (\d+) fps, CPU ([\d.]+) ms/frame, GPU ([\d.]+|\?) ms/frame, (\d+) columns loaded.*?(\d+) meshes in progress")
ALLOC = re.compile(r"GPU allocator: ([\d.]+) MB allocated, ([\d.]+) MB reserved")
PERFL = re.compile(
    r"\[perf\] \[\s*([\d.]+) s\] frames (\d+), median ([\d.]+) ms, p99 ([\d.]+) ms, max ([\d.]+) ms; columns meshed ([\d.]+)/s; "
    r"light: (\d+) chunks in ([\d.]+) ms, (\d+) relights \((\d+) changes\) in ([\d.]+) ms; private memory ([\d.]+) MB, "
    r"working set ([\d.]+) MB; last mesh at ([\d.]+) s; (\d+) columns"
)
WORK = re.compile(
    r"\[perf\] \[\s*([\d.]+) s\] work: sections meshed ([\d.]+)/s in ([\d.]+) ms/s; walks ([\d.]+)/s in ([\d.]+) ms/s; "
    r"process CPU ([\d.]+) ms/s"
)
RECV = re.compile(r"\[perf\] \[\s*([\d.]+) s\] work: .*receive ([\d.]+) ms/s, ([\d.]+) events/s")
STABLE = re.compile(r"\[perf\] initial load meshed at \d+ ms .*stable since (\d+) ms")
MCACHE = re.compile(r"\[perf\] \[\s*([\d.]+) s\] work: .*mesh cache ([\d.]+) MB")
BASES = re.compile(r"\[perf\] \[\s*([\d.]+) s\] work: .*bases ([\d.]+)/s, without base ([\d.]+)/s")
UPLOAD = re.compile(r"\[perf\] \[\s*([\d.]+) s\] work: .*mesh upload ([\d.]+) ms/s \(received ([\d.]+) ms/s, GPU ([\d.]+) ms/s, ([\d.]+) columns/s, ([\d.]+) MB/s, in place ([\d.]+)/s\); pool preparation ([\d.]+) ms/s")
TICK = re.compile(r"\[perf\] server: tick median ([\d.]+) ms, p95 ([\d.]+) ms, p99 ([\d.]+) ms, mean ([\d.]+) ms")
INGAME = re.compile(r"\[perf\] in game at (\d+) ms")
LOADED = re.compile(r"\[perf\] initial load meshed at (\d+) ms")
# Loading landmarks (client `perf_log.rs`, integrated server `server.rs`).
WORLD_ENTERED = re.compile(r"\[perf\] world entered at (\d+) ms")
FIRST_CHUNK = re.compile(r"\[perf\] first chunk received at (\d+) ms")
CAUGHT_UP = re.compile(r"\[perf\] meshing caught up at (\d+) ms")
SERVER_FIRST_CHUNK = re.compile(r"singleplayer: first chunk sent (\d+) ms after opening")
SERVER_QUEUE_EMPTY = re.compile(r"singleplayer: chunk queue empty (\d+) ms after opening")
FIRST = re.compile(r"first frame: (\d+) ms")
GEN = re.compile(r"singleplayer: generated (\d+) chunks in ([\d.]+) ms")
# Phases of a generation batch and steps of the generator setup (`rc-sim/src/generation_timing.rs`).
GEN_BATCH = re.compile(r"generation batch \d+: (\d+) chunks, ([\d.]+) ms, \d+ threads; phases ms: (.*?); "
                       r"single-threaded ([\d.]+) ms")
GEN_SETUP = re.compile(r"singleplayer: generator minecraft:overworld setup ms: (.*)")
NAMED_MS = re.compile(r"([a-z][a-z ,-]*?) ([\d.]+)(?:, |$)")
HEAP = re.compile(r"Rust heap: ([\d.]+) MB")
# Cache of the resources baked at startup (`resources.rs`): "to write" = cold launch.
COLD = re.compile(r"resources: \d+ pack\(s\), cache (valid|to write)")
STARTUP = re.compile(r"\[startup\] (.+?) at ([\d.]+) ms")
AUDIO_READY = re.compile(r"audio: ready in ([\d.]+) ms")
AUDIO_JOINED = re.compile(r"audio: engine joined ([\d.]+) ms after its launch, wait ([\d.]+) ms")
OPENING = re.compile(r"singleplayer: opened in ([\d.]+) ms")
GEN_READY = re.compile(r"singleplayer: generator minecraft:overworld ready in ([\d.]+) ms")
SAVE = re.compile(r"singleplayer: save: (\d+) chunks, snapshot ([\d.]+) ms")
SAVE_END = re.compile(r"singleplayer: closing save: (\d+) chunks, snapshot ([\d.]+) ms, total ([\d.]+) ms")
# Generic counter of a mission: `[perf] [  12.0 s] counter <name> <value>` (column `c:<name>`).
COUNTER = re.compile(r"\[perf\] \[\s*([\d.]+) s\] counter (\S+) (-?[\d.]+)")
GUEST = re.compile(r"in game \((\d+) ms\)")
# Any dated line (`[  12.0 s]`): the server's tick report has no date of its own.
DATED = re.compile(r"\[\s*([\d.]+) s\]")
ENTITIES = re.compile(r"\[\s*([\d.]+) s\] .*?(\d+) entities drawn, block entities (\d+) of which (\d+) drawn")
RAM = re.compile(r"\[perf\] \[\s*([\d.]+) s\] ram: private [\d.]+ MB, allocator ([\d.]+) MB committed")
SERVER_RAM = re.compile(r"server: (\d+) chunks, sections ([\d.]+) MB, light ([\d.]+) MB \(\d+ layers\), heightmaps ([\d.]+) MB, "
                        r"block entities ([\d.]+) MB, kept compound ([\d.]+) MB")
RELOAD = re.compile(r"resource reload: applied in ([\d.]+) ms")


def med(v):
    return statistics.median(v) if v else float("nan")


# The client writes a dated line about every second; a longer gap means it stalled.
DATED_LINE_GAP_S = 1.5


def dated_ticks(lines):
    """Server tick reports with the dated lines around them: (date before, date after or None,
    p99). The server line carries no date of its own. A report covers the last 100 ticks, and its
    p99 (rank 99 of 100) is their maximum."""
    out, before, undated = [], None, []
    for line in lines:
        tick = TICK.search(line)
        if tick:
            if before is not None:  # a report before any date is loading: left out
                undated.append(float(tick.group(3)))
            continue
        d = DATED.search(line)
        if d:
            after = float(d.group(1))
            out += [(before, after, p99) for p99 in undated]
            before, undated = after, []
    return out + [(before, None, p99) for p99 in undated]


def report_inside(before, after, inside):
    """Whether a report written between two dated lines lies in a range. Dated by the line
    before when the next one follows within `DATED_LINE_GAP_S`; across a longer gap (client
    stall) or after the last line, its time is unknown, so it counts only if the whole gap does."""
    if after is not None and after - before <= DATED_LINE_GAP_S:
        return inside(before)
    return after is not None and inside(before) and inside(after)


def timed_tick_metrics(lines, win, start, secs):
    """Longest tick of the window (first uses, volleys, saves) and median p99 of the last
    minute (end of a long flight; NaN when the scene has no full minute after `start`, it would
    count loading ticks)."""
    ticks = dated_ticks(lines)
    in_window = [p99 for b, a, p99 in ticks if report_inside(b, a, win)]
    last_minute = lambda t: secs - 61 <= t <= secs - 1  # noqa: E731
    p99_last_minute = med([p99 for b, a, p99 in ticks if report_inside(b, a, last_minute)])
    return {"tick_max_ms": max(in_window) if in_window else float("nan"),
            "tick_p99_last_minute_ms": p99_last_minute if secs >= start + 61 else float("nan")}


def load_metrics(lines, win):
    """What the frame holds (must not move between two passes of one exe): entities and block
    entities drawn, block entities loaded (medians of the window)."""
    e = [x for x in (ENTITIES.search(l) for l in lines) if x and win(float(x.group(1)))]
    return {"entities_drawn": med([int(x.group(2)) for x in e]), "block_entities": med([int(x.group(3)) for x in e]),
            "block_entities_drawn": med([int(x.group(4)) for x in e])}


def memory_metrics(lines, win):
    """Allocator and integrated-server chunk data (`[perf] ram` line, every 5 s): last value of
    the window and growth over it."""
    alloc, server = [], []
    for line in lines:
        r = RAM.search(line)
        if not r or not win(float(r.group(1))):
            continue
        alloc.append(float(r.group(2)))
        s = SERVER_RAM.search(line)
        if s:
            server.append((int(s.group(1)), sum(float(s.group(i)) for i in range(2, 7))))
    nan = float("nan")
    return {
        "allocator_mb": alloc[-1] if alloc else nan, "allocator_growth_mb": alloc[-1] - alloc[0] if alloc else nan,
        "server_chunks": server[-1][0] if server else nan, "server_mem_mb": server[-1][1] if server else nan,
        "server_mem_growth_mb": server[-1][1] - server[0][1] if server else nan,
    }


def generation_phase_metrics(lines):
    """Split of the generation batches (every batch, like `gen_ms_per_chunk`): ms per chunk of
    each phase, share of the single-threaded phases (what more threads cannot shorten), and the
    steps of the first overworld generator setup."""
    phases, single, chunks, total = {}, 0.0, 0, 0.0
    for b in (GEN_BATCH.search(l) for l in lines):
        if not b:
            continue
        chunks += int(b.group(1))
        total += float(b.group(2))
        single += float(b.group(4))
        for name, ms in NAMED_MS.findall(b.group(3)):
            phases[name] = phases.get(name, 0.0) + float(ms)
    m = {f"gen:{name} ms/chunk": ms / chunks for name, ms in phases.items()} if chunks else {}
    if total > 0:
        m["gen:single-threaded %"] = 100.0 * single / total
    setup = next((s for s in (GEN_SETUP.search(l) for l in lines) if s), None)
    for name, ms in NAMED_MS.findall(setup.group(1)) if setup else []:
        m[f"gen setup:{name} ms"] = float(ms)
    return m


def lines_since_world_entry(lines):
    """Lines after the first `[perf] world entered`, or all of them without a world (title
    screen). The per-second blocks of the loading screen before it carry the world preparation
    (spawn light, events); the whole-run sums leave them out, as when only worlds were logged."""
    entry = next((i for i, l in enumerate(lines) if WORLD_ENTERED.search(l)), None)
    return lines if entry is None else lines[entry + 1:]


def loading_landmarks(lines):
    """Loading landmarks in ms (NaN when absent). `meshing_after_last_column_ms`: meshing still
    needed once the count of columns to mesh stopped changing (negative when it had already
    caught up, e.g. a count that dropped)."""
    def first(rx):
        x = next((x for x in (rx.search(l) for l in lines) if x), None)
        return float(x.group(1)) if x else float("nan")

    m = {"first_chunk_ms": first(FIRST_CHUNK), "caught_up_ms": first(CAUGHT_UP), "stable_ms": first(STABLE),
         "server_first_chunk_ms": first(SERVER_FIRST_CHUNK), "server_queue_empty_ms": first(SERVER_QUEUE_EMPTY)}
    m["meshing_after_last_column_ms"] = m["caught_up_ms"] - m["stable_ms"]
    return m


def analyse(log_path, frames_path, frm, secs):
    with open(log_path, encoding="utf-8", errors="replace") as f:
        lines = f.read().splitlines()
    m = {}
    win = lambda t: frm <= t <= secs - 1  # noqa: E731
    st = [s for s in (STAT.search(l) for l in lines) if s]
    sw = [s for s in st if win(float(s.group(1)))]
    m["cpu_ms"] = med([float(s.group(3)) for s in sw])
    m["gpu_ms"] = med([float(s.group(4)) for s in sw if s.group(4) != "?"])
    m["columns"] = med([int(s.group(5)) for s in sw])
    world = lines_since_world_entry(lines)
    pl = [p for p in (PERFL.search(l) for l in world) if p]
    pw = [p for p in pl if win(float(p.group(1)))]
    span = max(1e-9, len(pw))
    m["meshed_s"] = sum(float(p.group(6)) for p in pw) / span
    m["meshed_s_max"] = max((float(p.group(6)) for p in pl), default=0.0)
    m["light_chunks_s"] = sum(int(p.group(7)) for p in pw) / span
    m["light_ms_per_chunk"] = (sum(float(p.group(8)) for p in pl) / max(1, sum(int(p.group(7)) for p in pl)))
    m["light_chunks_total"] = sum(int(p.group(7)) for p in pl)
    m["relights_s"] = sum(int(p.group(9)) for p in pw) / span
    m["relight_ms_s"] = sum(float(p.group(11)) for p in pw) / span
    m["private_mb"] = med([float(p.group(12)) for p in pw])
    m["ws_mb"] = med([float(p.group(13)) for p in pw])
    m["last_mesh_s"] = float(pl[-1].group(14)) if pl else float("nan")
    # Pool work on re-meshing and the walk, process CPU (per second, window).
    wk = [w for w in (WORK.search(l) for l in lines) if w and win(float(w.group(1)))]
    for i, k in enumerate(("sections_s", "meshing_ms_s", "walks_s", "walks_ms_s", "process_cpu_ms_s"), 2):
        m[k] = sum(float(w.group(i)) for w in wk) / len(wk) if wk else float("nan")
    # Receiving events on the render thread during loading (first 5 seconds).
    rv = [r for r in (RECV.search(l) for l in world) if r and float(r.group(1)) <= 5.0]
    m["receive_load_ms"] = sum(float(r.group(2)) for r in rv) if rv else float("nan")
    mc = [c for c in (MCACHE.search(l) for l in lines) if c and win(float(c.group(1)))]
    m["cache_mb"] = med([float(c.group(2)) for c in mc])
    bs = [b for b in (BASES.search(l) for l in lines) if b and win(float(b.group(1)))]
    m["bases_s"] = sum(float(b.group(2)) for b in bs) / len(bs) if bs else float("nan")
    m["without_base_s"] = sum(float(b.group(3)) for b in bs) / len(bs) if bs else float("nan")
    ev = [e for e in (UPLOAD.search(l) for l in lines) if e and win(float(e.group(1)))]
    for i, k in enumerate(("upload_ms_s", "upload_received_ms_s", "upload_gpu_ms_s", "upload_columns_s", "upload_mb_s", "in_place_s", "pool_prep_ms_s"), 2):
        m[k] = sum(float(e.group(i)) for e in ev) / len(ev) if ev else float("nan")
    al = [a for a in (ALLOC.search(l) for l in lines) if a]
    m["vram_allocated_mb"] = med([float(a.group(1)) for a in al[-10:]])
    m["vram_reserved_mb"] = med([float(a.group(2)) for a in al[-10:]])
    tk = [t for t in (TICK.search(l) for l in lines) if t]
    tkw = tk[len(tk) // 2:] if tk else []
    m["tick_med_ms"] = med([float(t.group(1)) for t in tkw])
    m["tick_p99_ms"] = med([float(t.group(3)) for t in tkw])
    m["tick_mean_ms"] = med([float(t.group(4)) for t in tkw])
    m.update(timed_tick_metrics(lines, win, frm, secs))
    m.update(load_metrics(lines, win))
    m.update(memory_metrics(lines, win))
    c = [x for x in (LOADED.search(l) for l in lines) if x]
    m["load_ms"] = float(c[0].group(1)) if c else float("nan")
    m.update(loading_landmarks(lines))
    g = [x for x in (INGAME.search(l) for l in lines) if x]
    m["in_game_ms"] = float(g[0].group(1)) if g else float("nan")
    f = [x for x in (FIRST.search(l) for l in lines) if x]
    m["first_frame_ms"] = float(f[0].group(1)) if f else float("nan")
    ge = [x for x in (GEN.search(l) for l in lines) if x]
    m["generated"] = sum(int(x.group(1)) for x in ge)
    m["gen_ms_per_chunk"] = sum(float(x.group(2)) for x in ge) / max(1, m["generated"])
    m["gen_first_batch_ms"] = float(ge[0].group(2)) if ge else float("nan")
    gen_ms = sum(float(x.group(2)) for x in ge)
    m["gen_chunks_s"] = m["generated"] * 1e3 / gen_ms if gen_ms > 0 else float("nan")
    m.update(generation_phase_metrics(lines))
    ta = [x for x in (HEAP.search(l) for l in lines) if x]
    m["heap_mb"] = med([float(x.group(1)) for x in ta[-10:]])
    m["last_mesh_ms"] = m["last_mesh_s"] * 1e3
    cold = [x for x in (COLD.search(l) for l in lines) if x]
    m["cold"] = float(cold[0].group(1) == "to write") if cold else float("nan")

    def first(rx, group=1):
        x = next((x for x in (rx.search(l) for l in lines) if x), None)
        return float(x.group(group)) if x else float("nan")

    m["audio_ready_ms"] = first(AUDIO_READY)
    m["audio_joined_ms"] = first(AUDIO_JOINED)
    m["audio_wait_ms"] = first(AUDIO_JOINED, 2)
    m["open_ms"] = first(OPENING)
    m["generator_ms"] = first(GEN_READY)
    m["closing_total_ms"] = first(SAVE_END, 3)
    for x in (STARTUP.search(l) for l in lines):
        if x:
            m.setdefault(f"startup:{x.group(1)}", float(x.group(2)))
    m["reload_ms"] = first(RELOAD)
    saves = [x for x in (SAVE.search(l) for l in lines) if x]
    sv = [float(x.group(2)) for x in saves]
    m["save_chunks_med"] = med([int(x.group(1)) for x in saves])
    m["saves"] = len(sv)
    m["save_snapshot_ms"] = med(sv)
    m["save_snapshot_max_ms"] = max(sv) if sv else float("nan")
    counters = {}
    for x in (COUNTER.search(l) for l in lines):
        if x and win(float(x.group(1))):
            counters.setdefault(x.group(2), []).append(float(x.group(3)))
    for k, v in counters.items():
        m[f"c:{k}"] = sum(v) / len(v)
    # Frames: interval of each frame in the window.
    ft = []
    if os.path.exists(frames_path):
        with open(frames_path, encoding="utf-8") as f:
            for l in f:
                parts = l.split()
                if len(parts) == 2 and win(float(parts[0]) / 1e3):
                    ft.append(float(parts[1]))
    ft.sort()
    if ft:
        m["frames_s"] = len(ft) / max(1e-9, sum(ft) / 1e3)
        m["frame_med_ms"] = ft[len(ft) // 2]
        m["frame_p99_ms"] = ft[min(len(ft) - 1, int(len(ft) * 0.99))]
        m["frame_p999_ms"] = ft[min(len(ft) - 1, int(len(ft) * 0.999))]
        m["frame_max_ms"] = ft[-1]
    return m


def cache_source(a):
    """Version data to copy: `--cache-source`, otherwise the player's cache."""
    root = (getattr(a, "cache_source", None) or os.environ.get("RUSTCRAFT_CACHE")
            or os.path.join(os.environ.get("LOCALAPPDATA", ""), "rustcraft", "cache"))
    return os.path.join(root, "26.3")


def seed_cache(dest, src):
    """`RUSTCRAFT_CACHE` folder filled with the version data (data.bin, commands, extractions),
    without the baked-resource caches (`assets-*.bin`, specific to each exe)."""
    d = os.path.join(dest, "26.3")
    os.makedirs(d, exist_ok=True)
    if not os.path.exists(os.path.join(src, "data.bin")):
        raise SystemExit(f"version data missing from {src}: start the client once or pass --cache-source")
    # Whole tree: `generated/reports` (biome parameters, item components) is read by the
    # integrated server; without it generation is disabled. Files already present are kept.
    def keep(s, t):
        if not os.path.exists(t):
            shutil.copy2(s, t)
    shutil.copytree(src, d, ignore=shutil.ignore_patterns("assets-*", "*.tmp"), dirs_exist_ok=True, copy_function=keep)
    return dest


def exe_cache_dir(exe):
    """Cache folder specific to an exe (hashed absolute path): the `KEEP` limit of the resource
    cache (3 per folder) can no longer evict another bench exe's cache."""
    p = os.path.abspath(exe)
    stem = os.path.splitext(os.path.basename(p))[0]
    return os.path.join(PERF, "cache", f"{stem}-{hashlib.sha1(p.lower().encode()).hexdigest()[:8]}")


def split_exe(spec):
    """`path|VAR=val|…` -> (path, [VAR=val…])."""
    exe, *own = spec.split("|")
    return exe, own


# Test variables cleared before each launch (only the scene sets them).
SCENE_VARS = ("RUSTCRAFT_CMDTEST", "RUSTCRAFT_KEYTEST", "RUSTCRAFT_MENUTEST", "RUSTCRAFT_MENUCLICKS",
              "RUSTCRAFT_CREATIVETEST", "RUSTCRAFT_PARTICLES_BENCH", "RUSTCRAFT_THREADS",
              "RUSTCRAFT_AUTOSAVE_TICKS", "RUSTCRAFT_LAN_AUTH", "RUSTCRAFT_INVTEST", "RUSTCRAFT_INVCLICKS",
              "RUSTCRAFT_INVMOUSE", "RUSTCRAFT_BOOKTEST", "RUSTCRAFT_TABTEST", "RUSTCRAFT_DEBUGTEST",
              "RUSTCRAFT_PACKSCREEN", "RUSTCRAFT_RELOADTEST", "RUSTCRAFT_BENCH_TURN", "RUSTCRAFT_FPSLIMIT")
# Variables of every launch (with the folders of the launch, set in `launch_env`).
FIXED_ENV = {
    "RUSTCRAFT_BACKGROUND": "1", "RUSTCRAFT_LAN_BIND": "127.0.0.1", "RUSTCRAFT_SOLO": "perf",
    "RUSTCRAFT_VSYNC": "0", "RUSTCRAFT_GENLOG": "1",
    # Special dates frozen: an Oct 31 (Halloween heads) or a Dec 24-26 would change the world
    # or the frame being measured.
    "RUSTCRAFT_HALLOWEEN": "0", "RUSTCRAFT_CHRISTMAS": "0",
}


def scene_drivers(sc):
    """`RUSTCRAFT_*` variables the scene drives the client with (the exe must read them)."""
    implied = {"menu": "RUSTCRAFT_MENUTEST", "cmd": "RUSTCRAFT_CMDTEST", "keys": "RUSTCRAFT_KEYTEST"}
    return sorted({v for k, v in implied.items() if k in sc} | set(sc.get("env", {})))


def launch_env(a, sc, out, game, cache_dir, own):
    env = dict(os.environ)
    env.update(FIXED_ENV)
    env.update({
        "RUSTCRAFT_GAME_DIR": game, "RUSTCRAFT_CAPTURE": f"{out}/fin.bmp;{sc['secs']}",
        "RUSTCRAFT_PERFLOG": f"{out}/images.txt", "RUSTCRAFT_CACHE": cache_dir,
    })
    for k in SCENE_VARS:
        env.pop(k, None)
    if sc.get("title"):
        env.pop("RUSTCRAFT_SOLO")
    if "menu" in sc:
        env["RUSTCRAFT_MENUTEST"] = sc["menu"]
    if "cmd" in sc:
        env["RUSTCRAFT_CMDTEST"] = sc["cmd"]
    if "keys" in sc:
        env["RUSTCRAFT_KEYTEST"] = sc["keys"]
    env.update(sc.get("env", {}))
    # `--env` for all, then `--exe path|VAR=val|VAR2=val`: variables specific to that exe;
    # `-VAR` removes one (a variable the client only tests for presence cannot be set to "off").
    for kv in a.env + own:
        if kv.startswith("-") and "=" not in kv:
            env.pop(kv[1:], None)
            continue
        k, v = kv.split("=", 1)
        env[k] = v
    return env


def start_guests(a, sc, out, env):
    """Windowless LAN guests (`connect` example of rc-client), launched once the host has published."""
    time.sleep(sc.get("guests_after", 12))
    procs = []
    for k in range(sc["guests"]):
        f = open(os.path.join(out, f"invite-{k}.log"), "w", encoding="utf-8")
        cmd = [os.path.abspath(a.connect), "127.0.0.1", str(LAN_PORT), str(sc["secs"] - 14), f"Invite{k}"]
        procs.append((subprocess.Popen(cmd, env=env, stdout=f, stderr=subprocess.STDOUT, creationflags=NO_WINDOW), f))
    return procs


def guest_stats(out, n):
    joined = []
    for k in range(n):
        p = os.path.join(out, f"invite-{k}.log")
        x = GUEST.search(open(p, encoding="utf-8", errors="replace").read()) if os.path.exists(p) else None
        if x:
            joined.append(float(x.group(1)))
    return {"guests_joined": len(joined), "guest_join_ms": med(joined)}


# Warm-up launch (unmeasured): fills the resource cache and the driver's shader cache.
WARMUP = {"at": "plains", "rd": 12, "secs": 15, "from": 5}


def stage_scene(a, sc, info, game):
    """Game folder of a launch: model copy, player, options, then the off-world fixtures."""
    opts = dict(sc.get("opts", {}))
    if sc.get("sound"):
        opts["soundDevice"] = json.dumps(a.sound_device)
    if a.fullscreen:
        opts["fullscreen"] = "true"
    model = os.path.join(PERF, sc.get("model", "model"))
    stage(game, view_of(sc, info), sc["rd"], opts, sc.get("world") == "fresh", model, sc.get("survival", False))
    if sc.get("servers"):
        fixtures.write_servers(game, sc["servers"])
    if sc.get("worlds"):
        fixtures.write_worlds(game, model, sc["worlds"])
    if sc.get("packs"):
        fixtures.write_resource_packs(game, sc["packs"])


def run_one(a, name, sc, info, pass_no, exe_no=0, warm=False):
    exe, own = split_exe(a.exe[exe_no])
    tag = f"{name}-{pass_no}" + (f"-{exe_no}" if len(a.exe) > 1 else "")
    out = os.path.join(PERF, a.label, f"warmup-{exe_no}" if warm else tag)
    game = os.path.join(PERF, "game")
    sc = scene_with_positions(sc, info)
    stage_scene(a, sc, info, game)
    os.makedirs(out, exist_ok=True)
    # Measurement window, reread by `tools/window.py` to bound the samply profile.
    with open(os.path.join(out, "window.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump({"from": sc["from"], "to": sc["secs"] - 1}, f)
    kind, copy = sc.get("cache"), None
    if kind:
        cache_dir = os.path.join(PERF, "cold-cache")
        if os.path.exists(cache_dir):
            shutil.rmtree(cache_dir)
        if kind == "fresh":
            os.makedirs(cache_dir)  # data.bin redone by the client (launcher's Java, ≈ 4.6 s)
        else:
            seed_cache(cache_dir, cache_source(a))
        if kind in ("first", "fresh"):
            # Fresh exe (other name, other date): fresh resource-cache key and driver shader cache.
            ndir = os.path.join(PERF, "fresh-exe")
            os.makedirs(ndir, exist_ok=True)
            copy = os.path.join(ndir, f"{os.path.splitext(os.path.basename(exe))[0]}-{uuidlib.uuid4().hex[:8]}.exe")
            shutil.copy(exe, copy)
            exe = copy
    else:
        cache_dir = seed_cache(exe_cache_dir(exe), cache_source(a))
    env = launch_env(a, sc, out, game, cache_dir, own)
    w, h = a.size.split("x")
    cmd = [os.path.abspath(exe), "--username", NAME, "--width", w, "--height", h]
    if a.samply and not warm:
        # CPU profile of the whole process (`tools/cpu_profile.py`, PROFIL_FIL=all|<thread name>), bounded to
        # the measurement window by `window.json` (`--whole-profile` for the whole profile).
        cmd = ["samply", "record", "--save-only", "--unstable-presymbolicate", "-o", os.path.join(out, "profile.json.gz"), "--"] + cmd
    try:
        if a.dry_run:
            shown = ", ".join(f"{k}={env[k]}" for k in sorted(env) if k.startswith("RUSTCRAFT_"))
            print(f"  dry run {'warm-up' if warm else tag}: {' '.join(cmd)}\n    {shown}")
            return {}
        log = os.path.join(out, "client.log")
        status = fixtures.StatusServers(sc["servers"]) if sc.get("servers") else contextlib.nullcontext()
        with open(log, "w", encoding="utf-8") as f, status:
            p = subprocess.Popen(cmd, env=env, stdout=f, stderr=subprocess.STDOUT, creationflags=NO_WINDOW)
            guests = start_guests(a, sc, out, env) if sc.get("guests") and not warm else []
            try:
                p.wait(timeout=sc["secs"] + 90)
            except subprocess.TimeoutExpired:
                p.kill()
                print(f"  {name}: timed out, client stopped")
            for g, gl in guests:
                try:
                    g.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    g.kill()
                gl.close()
    finally:
        if copy:
            try:
                os.remove(copy)
            except OSError:
                pass
    m = analyse(log, os.path.join(out, "images.txt"), sc["from"], sc["secs"])
    if sc.get("guests"):
        m.update(guest_stats(out, sc["guests"]))
    return m


KEYS = [
    ("frames_s", "fps", ".0f"), ("frame_med_ms", "frame med. ms", ".3f"), ("frame_p99_ms", "frame p99 ms", ".3f"),
    ("cpu_ms", "CPU ms", ".3f"), ("gpu_ms", "GPU ms", ".3f"), ("columns", "columns", ".0f"),
    ("meshed_s", "meshed/s (window)", ".1f"), ("meshed_s_max", "meshed/s max", ".0f"),
    ("sections_s", "sections meshed/s", ".0f"), ("meshing_ms_s", "meshing ms/s (pool)", ".1f"),
    ("walks_s", "walks/s", ".1f"), ("walks_ms_s", "walks ms/s (pool)", ".2f"),
    ("process_cpu_ms_s", "process CPU ms/s", ".0f"), ("cache_mb", "mesh cache MB", ".1f"),
    ("bases_s", "partials with base/s", ".1f"), ("without_base_s", "partials without base/s", ".1f"),
    ("upload_ms_s", "upload ms/s", ".1f"), ("upload_received_ms_s", "upload received ms/s", ".1f"), ("upload_gpu_ms_s", "upload GPU ms/s", ".1f"),
    ("upload_columns_s", "upload columns/s", ".0f"), ("upload_mb_s", "upload MB/s", ".1f"), ("in_place_s", "in place/s", ".1f"),
    ("pool_prep_ms_s", "pool preparation ms/s", ".1f"),
    ("light_chunks_s", "light chunks/s", ".1f"), ("light_ms_per_chunk", "light ms/chunk", ".3f"),
    ("relights_s", "light relights/s", ".1f"), ("relight_ms_s", "relight ms/s", ".2f"),
    ("tick_med_ms", "tick med. ms", ".3f"), ("tick_p99_ms", "tick p99 ms", ".3f"),
    ("private_mb", "private MB", ".0f"), ("ws_mb", "WS MB", ".0f"), ("vram_allocated_mb", "VRAM MB", ".1f"),
    ("first_frame_ms", "1st frame ms", ".0f"), ("in_game_ms", "in game ms", ".0f"),
    ("first_chunk_ms", "first chunk received ms", ".0f"), ("caught_up_ms", "meshing caught up ms", ".0f"),
    ("load_ms", "meshed load ms", ".0f"), ("stable_ms", "meshable columns stable since ms", ".0f"),
    ("meshing_after_last_column_ms", "meshing after last column ms", ".0f"),
    ("server_first_chunk_ms", "server first chunk sent ms", ".0f"),
    ("server_queue_empty_ms", "server chunk queue empty ms", ".0f"),
    ("last_mesh_ms", "last mesh ms", ".0f"), ("receive_load_ms", "receive 0-5 s ms", ".0f"),
    ("audio_ready_ms", "audio ready ms", ".0f"), ("audio_joined_ms", "audio joined ms", ".0f"),
    ("audio_wait_ms", "audio wait ms", ".0f"), ("open_ms", "world opening ms", ".0f"),
    ("generator_ms", "generator ready ms", ".0f"), ("generated", "chunks generated", ".0f"),
    ("gen_ms_per_chunk", "generation ms/chunk", ".2f"), ("gen_chunks_s", "generation chunks/s", ".0f"),
    ("gen_first_batch_ms", "generation 1st batch ms", ".1f"), ("saves", "saves", ".0f"),
    ("save_snapshot_ms", "save snapshot med. ms", ".2f"), ("save_snapshot_max_ms", "save snapshot max ms", ".2f"),
    ("closing_total_ms", "closing save ms", ".1f"), ("guests_joined", "guests joined", ".0f"),
    ("guest_join_ms", "guest join ms", ".0f"), ("heap_mb", "heap MB", ".1f"),
    ("tick_max_ms", "tick max ms", ".3f"), ("tick_p99_last_minute_ms", "tick p99 last minute ms", ".3f"),
    ("save_chunks_med", "chunks per save", ".0f"), ("reload_ms", "resource reload ms", ".0f"),
    ("entities_drawn", "entities drawn", ".0f"), ("block_entities", "block entities", ".0f"),
    ("block_entities_drawn", "block entities drawn", ".0f"), ("allocator_mb", "allocator MB", ".0f"),
    ("allocator_growth_mb", "allocator growth MB", ".1f"), ("server_chunks", "server chunks", ".0f"),
    ("server_mem_mb", "server chunk data MB", ".1f"), ("server_mem_growth_mb", "server chunk data growth MB", ".1f"),
]


def expand_matrix(exes, matrix):
    """`--matrix`: each exe without a switch, with each one alone, then all together."""
    if not matrix:
        return list(exes)
    out = []
    for e in exes:
        out += [e] + [f"{e}|{kv}" for kv in matrix]
        if len(matrix) > 1:
            out.append(e + "|" + "|".join(matrix))
    return out


def rotated(n, pass_no):
    """Order of the exes at pass `pass_no` (1, 2…): rotated by one notch on each pass."""
    k = (pass_no - 1) % max(1, n)
    return list(range(n))[k:] + list(range(n))[:k]


def missing_data(sc, info):
    """What the scene needs that the model lacks (view, located path, model), or None."""
    if "at" in sc and sc["at"] not in info["scenes"]:
        return f"view {sc['at']} missing from scenes.json (run `prepare` again)"
    model = sc.get("model", "model")
    if not os.path.isdir(os.path.join(PERF, model)):
        return f"model {model} missing (run `seed-model` or `prepare`)"
    if "fresh_path" in sc and sc["fresh_path"] not in fresh_paths(model):
        return f"path {sc['fresh_path']} missing from {model}/{FRESH_PATHS_FILE} (run `seed-model` again)"
    return None


_EXE_LACKS = {}


def exe_lacks(exe):
    """Driver variables of the scenes whose name the exe does not contain, so that it cannot
    read them (an older version): read once per exe."""
    key = os.path.abspath(exe).lower()
    if key not in _EXE_LACKS:
        wanted = {v for sc in SCENES.values() for v in scene_drivers(sc)}
        with open(exe, "rb") as f:
            data = f.read()
        _EXE_LACKS[key] = {v for v in wanted if v.encode() not in data}
    return _EXE_LACKS[key]


def skip_reason(a, sc, info=None):
    if sc.get("sound") and not a.sound_device:
        return "no --sound-device"
    if sc.get("guests") and not (a.connect and os.path.exists(a.connect)):
        return "no --connect (compiled `connect` example of rc-client)"
    if info is not None and (why := missing_data(sc, info)):
        return why
    for spec in a.exe:
        exe = split_exe(spec)[0]
        if os.path.exists(exe) and (lacking := [v for v in scene_drivers(sc) if v in exe_lacks(exe)]):
            return f"{exe} does not read {', '.join(lacking)} (driver missing from that version)"
    return None


def run(a):
    info = load_info()
    a.exe = expand_matrix(a.exe, a.matrix)
    names = expand_scene_names(a.scenes) if a.scenes else list(SCENES)
    unknown = [n for n in names if n not in SCENES]
    if unknown:
        raise SystemExit(f"unknown scenes: {unknown}")
    kept_names = []
    for n in names:
        why = skip_reason(a, SCENES[n], info)
        if why:
            print(f"scene {n} skipped: {why}")
        else:
            kept_names.append(n)
    names = kept_names
    if a.passes < 3:
        print(f"warning: {a.passes} pass(es), 3 are needed for a median and a spread")
    if a.warmup:
        seen = set()
        for i, spec in enumerate(a.exe):
            k = os.path.abspath(split_exe(spec)[0]).lower()
            if k not in seen:
                seen.add(k)
                m = run_one(a, "warmup", WARMUP, info, 0, i, warm=True)
                if m:
                    print(f"warm-up {split_exe(spec)[0]}: cache {'to write' if m.get('cold') == 1 else 'valid'}", flush=True)
    key = lambda n, i: n if len(a.exe) == 1 else f"{n}#{i}"  # noqa: E731
    res = {key(n, i): [] for n in names for i in range(len(a.exe))}
    for pno in range(1, a.passes + 1):
        for n in names:
            for i in rotated(len(a.exe), pno):
                m = run_one(a, n, SCENES[n], info, pno, i)
                if not m:
                    continue
                res[key(n, i)].append(m)
                print(f"pass {pno} {key(n, i)}: fps {m.get('frames_s', 0):.0f}, frame p99 {m.get('frame_p99_ms', 0):.3f} ms, "
                      f"CPU {m['cpu_ms']:.3f}, GPU {m['gpu_ms']:.3f}, tick {m['tick_med_ms']:.3f} ms"
                      + (" (cold)" if m.get("cold") == 1 else ""), flush=True)
    if a.dry_run:
        return
    base = os.path.join(PERF, a.label)
    os.makedirs(base, exist_ok=True)
    with open(os.path.join(base, "resultats.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump({"exe": a.exe, **res}, f, indent=1)
    print(table(res, a.cold))
    report = noise_report(res, a.cold)
    with open(os.path.join(base, "bruit.md"), "w", encoding="utf-8", newline="\n") as f:
        f.write(report + "\n")
    print(report)


def kept(name, runs, include_cold=False):
    """Launches counted: cold ones ("cache to write") are left out, except with `--cold` and in the
    scenes made to measure a cold startup."""
    if include_cold or name.split("#")[0] in COLD_SCENES:
        return runs
    return [r for r in runs if r.get("cold") != 1]


def table(res, include_cold=False):
    res = {k: v for k, v in res.items() if k != "exe"}
    rows = ["| measurement | " + " | ".join(res) + " |", "|---|" + "---|" * len(res)]
    dyn = sorted({k for runs in res.values() for r in runs for k in r if k.startswith(("startup:", "c:", "gen:", "gen setup:"))})
    for k, label, fmt in KEYS + [(k, k, ".1f") for k in dyn]:
        cells = []
        for n, runs in res.items():
            v = [r[k] for r in kept(n, runs, include_cold) if k in r and r[k] == r[k]]
            if not v:
                cells.append("—")
                continue
            cells.append(f"{med(v):{fmt}} ({min(v):{fmt}}–{max(v):{fmt}})" if len(v) > 1 else f"{v[0]:{fmt}}")
        rows.append(f"| {label} | " + " | ".join(cells) + " |")
    rows.append("| cold runs excluded | " + " | ".join(
        str(len(runs) - len(kept(n, runs, include_cold))) for n, runs in res.items()) + " |")
    return "\n".join(rows)


NOISE_KEYS = [("frames_s", "fps"), ("frame_p99_ms", "frame p99"), ("cpu_ms", "CPU"), ("gpu_ms", "GPU"),
              ("tick_med_ms", "tick med."), ("process_cpu_ms_s", "process CPU")]


def spread(v):
    """Noise of a series of passes: (max − min) / median, in %."""
    m = med(v)
    return (max(v) - min(v)) / abs(m) * 100 if len(v) > 1 and m == m and m != 0 else float("nan")


def noise_of(a, b):
    """Noise of a comparison: the larger of the two, unknown (NaN) if either is
    (`max` with a NaN depends on the order of its arguments)."""
    sa, sb = spread(a), spread(b)
    return max(sa, sb) if sa == sa and sb == sb else float("nan")


def noise_report(res, include_cold=False):
    """Noise per scene (passes of one exe) and, with several exes, difference of each exe from the first
    (#0): "within noise" when the difference does not exceed the noise of the more scattered of the two."""
    res = {k: v for k, v in res.items() if k != "exe"}
    vals = {n: {k: [r[k] for r in kept(n, runs, include_cold) if k in r and r[k] == r[k]] for k, _ in NOISE_KEYS}
            for n, runs in res.items()}
    out = ["## Noise per scene: (max − min) / median of the passes", "",
           "| scene | " + " | ".join(l for _, l in NOISE_KEYS) + " |", "|---|" + "---|" * len(NOISE_KEYS)]
    for n, kv in vals.items():
        out.append(f"| {n} | " + " | ".join(f"{spread(kv[k]):.1f} %" if len(kv[k]) > 1 else "—" for k, _ in NOISE_KEYS) + " |")
    pairs = [(n, n.split("#")[0] + "#0") for n in vals if "#" in n and not n.endswith("#0")]
    if pairs:
        out += ["", "## Difference to exe #0 (medians) and verdict", "",
                "| scene | " + " | ".join(l for _, l in NOISE_KEYS) + " |", "|---|" + "---|" * len(NOISE_KEYS)]
        for n, ref in pairs:
            cells = []
            for k, _ in NOISE_KEYS:
                a, b = vals.get(ref, {}).get(k, []), vals[n][k]
                if not a or not b or med(a) == 0:
                    cells.append("—")
                    continue
                d = (med(b) - med(a)) / abs(med(a)) * 100
                noise = noise_of(a, b)
                # Unknown noise (fewer than 2 passes kept): no conclusion possible.
                verdict = "noise unknown" if noise != noise else "within noise" if abs(d) <= noise else "above"
                cells.append(f"{d:+.1f} % ({verdict})")
            out.append(f"| {n} | " + " | ".join(cells) + " |")
    return "\n".join(out + ["", load_report(res, include_cold)])


LOAD_KEYS = [("entities_drawn", "entities drawn"), ("block_entities", "block entities"), ("columns", "columns")]


def load_report(res, include_cold=False):
    """Load of each pass (medians of the window): a deterministic scene gives the same values on
    every pass of one exe; "differs" flags a scene whose load moved (wandering mobs, deaths,
    a slower load), whose timings then compare different work."""
    res = {k: v for k, v in res.items() if k != "exe"}
    out = ["## Load of the passes (identical on a deterministic scene)", "",
           "| scene | " + " | ".join(l for _, l in LOAD_KEYS) + " |", "|---|" + "---|" * len(LOAD_KEYS)]
    for n, runs in res.items():
        cells = []
        for k, _ in LOAD_KEYS:
            v = [r[k] for r in kept(n, runs, include_cold) if k in r and r[k] == r[k]]
            if not v:
                cells.append("—")
            elif min(v) == max(v):
                cells.append(f"{v[0]:.0f}")
            else:
                cells.append(f"{min(v):.0f}–{max(v):.0f} (differs)")
        out.append(f"| {n} | " + " | ".join(cells) + " |")
    return "\n".join(out)


def parse_run_dir(d):
    """`scene-pass[-exe]` -> (scene, pass, exe or None), or None (warm-up, other folder).
    Single-exe form tried first: `crowd-1000-1` is pass 1 of `crowd-1000`, not
    pass 1000 of `crowd`."""
    parts = d.rsplit("-", 1)
    if len(parts) == 2 and parts[0] in SCENES and parts[1].isdigit():
        return parts[0], int(parts[1]), None
    parts = d.rsplit("-", 2)
    if len(parts) == 3 and parts[0] in SCENES and parts[1].isdigit() and parts[2].isdigit():
        return parts[0], int(parts[1]), int(parts[2])
    return None


def reanalyse(label):
    """Rereads the logs of a bench already run (`target/perf/<label>/<scene>-<pass>[-<exe>]`)
    with the current analysis (cold mark included)."""
    base = os.path.join(PERF, label)
    runs = [r + (d,) for d in sorted(os.listdir(base))
            if (r := parse_run_dir(d)) and os.path.exists(os.path.join(base, d, "client.log"))]
    multi = any(e is not None for _, _, e, _ in runs)
    res = {}
    for n, _, e, d in sorted(runs, key=lambda t: (t[0], t[2] or 0, t[1])):
        sc = SCENES[n]
        m = analyse(os.path.join(base, d, "client.log"), os.path.join(base, d, "images.txt"), sc["from"], sc["secs"])
        res.setdefault(f"{n}#{e}" if multi else n, []).append(m)
    return res


def pack(_a):
    """Rewrites the `perf` pack and the maps of the existing model (new scenes without redoing `prepare`)."""
    info = load_info()
    model = os.path.join(PERF, "model")
    write_pack(model, {k: tuple(v) for k, v in info["sites"].items()})
    write_maps(model)
    print("perf pack rewritten:", ", ".join(pack_functions(info["sites"])))


# Pads of the flat test world of rc-sim (`tests_perf_pack.rs`): grass at y -61, chunks -2..=9 × -2..=2.
FLAT_SITES = {"crowd": (64, -61, 8), "redstone": (64, -61, 8)}


def chat_commands(script):
    """Commands a `RUSTCRAFT_CMDTEST` script types in the chat: the `t` steps between `/` and
    `enter`. A command edited by other keys (completion, erasing) is left out: its final text is
    not in the script."""
    out, typed = [], None
    for step in script.split("|"):
        if step == "/":
            typed = ""
        elif typed is not None and step.startswith("t") and step != "tab":
            typed += step[1:]
        elif typed is not None and step == "enter":
            out.append(typed)
            typed = None
        elif typed is not None and not step.startswith("w"):
            typed = None
    return out


def flat_world_positions(texts):
    """Stand-in `scenes.json` positions for the flat test world: every view and pad named in
    `texts` sits on the crowd pad, so placeholders resolve to coordinates the parser accepts."""
    x, y, z = FLAT_SITES["crowd"]
    names = {(kind, name) for t in texts for kind, name, _ in PLACEHOLDER.findall(t)}
    return {"scenes": {n: {"pos": [x + 0.5, y + 1.0, z + 0.5]} for k, n in names if k == "view"},
            "sites": {n: FLAT_SITES.get(n, FLAT_SITES["crowd"]) for k, n in names if k == "site"}}


def write_chat_commands(out):
    """`<out>/chat-commands/<scene>.txt`: the staging commands each scene types in the chat, one
    per line, so that a typo or a command our parser refuses fails a test instead of leaving the
    scene measuring an empty view."""
    scripts = {n: sc["cmd"] for n, sc in SCENES.items() if "cmd" in sc}
    info = flat_world_positions(scripts.values())
    folder = os.path.join(out, "chat-commands")
    os.makedirs(folder, exist_ok=True)
    for name, script in scripts.items():
        lines = chat_commands(resolve_positions(script, info))
        with open(os.path.join(folder, f"{name}.txt"), "w", encoding="utf-8", newline="\n") as f:
            f.write("".join(line + "\n" for line in lines))
    return len(scripts)


def pack_files(a):
    """The `perf` pack for the flat test world, in `<out>/datapacks/perf`, and the chat commands
    of the scenes in `<out>/chat-commands`: read by `crates/rc-sim/src/tests_perf_pack.rs`
    (`RUSTCRAFT_PERF_PACK=<out>/datapacks/perf`)."""
    write_pack(a.out, FLAT_SITES)
    write_chat_commands(a.out)
    print(os.path.join(os.path.abspath(a.out), "datapacks", "perf"))


BENCH_LINE = re.compile(r"\[bench\] (\S+) (\S+) (\S+) (-?[\d.]+(?:e-?\d+)?)")


def micro_results(paths):
    """`[bench] <bench> <case> <metric> <value>` lines of micro-bench outputs (one file per pass):
    (bench, case, metric) -> values in file order."""
    out = {}
    for p in paths:
        with open(p, encoding="utf-8", errors="replace") as f:
            for line in f:
                if m := BENCH_LINE.search(line):
                    out.setdefault(m.groups()[:3], []).append(float(m.group(4)))
    return out


def micro_number(v):
    return f"{v:.1f}" if abs(v) >= 100 else f"{v:.4g}"


def micro_table(res):
    """Median (min–max) and noise (max − min) / median of each micro-bench measure."""
    rows = ["| bench | case | metric | median (min–max) | noise | n |", "|---|---|---|---|---|---|"]
    for (bench, case, metric), v in sorted(res.items()):
        noise = spread(v)
        rows.append(f"| {bench} | {case} | {metric} | {micro_number(med(v))} ({micro_number(min(v))}–{micro_number(max(v))}) | "
                    + (f"{noise:.1f} %" if noise == noise else "—") + f" | {len(v)} |")
    return "\n".join(rows)


def micro(a):
    print(micro_table(micro_results(a.files)))


def build_parser():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("prepare")
    p.add_argument("--java", required=True)
    p.add_argument("--server-jar", required=True)
    p.add_argument("--seed", default="20261011")
    p.add_argument("--pregen-radius", type=int, default=16, help="radius in chunks outside the plains (old: 13)")
    p.add_argument("--pregen-center", choices=["camera", "site"], default="camera", help="old: site")
    g = sub.add_parser("seed-model", help="model of a seed, default game settings (fresh-world scenes)")
    g.add_argument("--java", required=True)
    g.add_argument("--server-jar", required=True)
    g.add_argument("--seed", default="1234")
    g.add_argument("--name", default="model-1234")
    sub.add_parser("pack")
    pf = sub.add_parser("pack-files", help="perf pack for the flat test world of rc-sim (function compile check)")
    pf.add_argument("out")
    mi = sub.add_parser("micro", help="table of `[bench]` lines of micro-bench outputs (one file per pass)")
    mi.add_argument("files", nargs="+")
    r = sub.add_parser("run")
    r.add_argument("--exe", action="append", required=True, help="repeatable: alternating A/B; path|VAR=val for a setting of its own")
    r.add_argument("--scenes", default="")
    r.add_argument("--passes", type=int, default=3)
    r.add_argument("--label", default="reference")
    r.add_argument("--env", action="append", default=[],
                   help="VAR=value added to every launch; --env=-VAR removes VAR (the `=` is needed: "
                        "`--env -VAR` reads -VAR as an option)")
    r.add_argument("--matrix", action="append", default=[], help="VAR=value: switch alone then combined, same exe")
    r.add_argument("--samply", action="store_true", help="records a samply profile per launch")
    r.add_argument("--warmup", type=int, choices=[0, 1], default=1, help="unmeasured warm-up launch per exe")
    r.add_argument("--cold", action="store_true", help="also counts the cold-cache launches")
    r.add_argument("--dry-run", action="store_true", help="prepares each launch without starting anything")
    r.add_argument("--size", default="1600x900", help="window WxH (e.g. 3840x2160)")
    r.add_argument("--fullscreen", action="store_true", help="fullscreen (options.txt fullscreen:true)")
    r.add_argument("--sound-device", default="", help="named audio device (scene named-sound-device)")
    r.add_argument("--connect", default="", help="`connect` example of rc-client (scenes lan-N)")
    r.add_argument("--cache-source", default="", help="cache to copy data.bin from (default: the player's)")
    t = sub.add_parser("table")
    t.add_argument("--label", default="reference")
    t.add_argument("--reanalyse", action="store_true", help="rereads the logs with the current analysis")
    t.add_argument("--cold", action="store_true", help="also counts the cold-cache launches")
    return ap


def main():
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    a = build_parser().parse_args()
    if a.cmd == "prepare":
        prepare(a)
    elif a.cmd == "seed-model":
        prepare_seed(a)
    elif a.cmd == "pack":
        pack(a)
    elif a.cmd == "pack-files":
        pack_files(a)
    elif a.cmd == "micro":
        micro(a)
    elif a.cmd == "run":
        run(a)
    else:
        res = reanalyse(a.label) if a.reanalyse else upgrade_results(
            json.load(open(os.path.join(PERF, a.label, "resultats.json"), encoding="utf-8")))
        print(table(res, a.cold))
        print(noise_report(res, a.cold))


if __name__ == "__main__":
    main()
