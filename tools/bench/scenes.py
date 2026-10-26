"""Reference scene bench (measurements of the optimisation, `docs/perf.md`).

Two steps, a quiet machine is mandatory for the second:

    python tools/bench/scenes.py prepare --java <java.exe> --server-jar <server.jar> [--seed 20261011]
                                        [--pregen-radius 16] [--pregen-center camera|site]
    python tools/bench/scenes.py pack        (rewrites the `perf` pack of the existing model)
    python tools/bench/scenes.py run --exe <rustcraft.exe> [--exe <other.exe>] [--scenes plains12,forest] [--passes 3]
                                    [--label name] [--env VAR=value] [--matrix VAR=value]… [--samply]
                                    [--warmup 0|1] [--cold] [--dry-run] [--sound-device NAME]
                                    [--connect connect.exe] [--cache-source FOLDER]
    python tools/bench/scenes.py table --label name [--reanalyse] [--cold]

`prepare` builds the model world with a temporary official server (127.0.0.1 only, stopped at
the end): fixed seed, locations found with `locate` (plains, dark forest, village, deep ocean,
lush caves, Nether), zones generated in advance with tickets (radius 33 chunks around the
plains; elsewhere `--pregen-radius`, 16 ≥ distance 12 + 2, centred on the chunk of the scene's
CAMERA; `--pregen-radius 13 --pregen-center site` = old model), time set to noon, weather and
spawning switched off, two flat pads (crowd, redstone); a `perf` data pack (staging functions,
flight, rotation and walking by teleporting every tick). Output: `target/perf/model/` and
`target/perf/scenes.json`.

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
check).

Readings: frame time (median, 99th percentile), CPU and GPU per frame, columns meshed, light,
memory, VRAM, integrated server ticks, startup (milestones `[startup] <step> at N ms`, audio,
world opening, generator) and loading, generation (ms/chunk, chunks/s, first batch), saves, LAN
guests, and any generic counter `[perf] [ t s] counter <name> <value>` (mean over the window,
column `c:<name>`). Output: `target/perf/<label>/resultats.json`, Markdown table (median of the
passes and min–max spread) and `bruit.md` (spread of the passes of one exe per scene; an A/B whose
difference is within that noise does not conclude). With `--samply`, `window.json` bounds the
profile to the measurement window (`tools/window.py`).
"""
import argparse
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
import nbt  # noqa: E402

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


def mobs_function(x, y, z):
    """100 mobs (25 of each kind) in a 10 × 10 grid in a 24 × 24 pen."""
    kinds = ["cow", "sheep", "pig", "chicken"]
    lines = [
        f"fill {x - 12} {y + 1} {z - 12} {x + 12} {y + 1} {z + 12} minecraft:oak_fence hollow",
    ]
    for i in range(100):
        gx, gz = i % 10, i // 10
        lines.append(f"summon minecraft:{kinds[i % 4]} {x - 9 + gx * 2}.5 {y + 1} {z - 9 + gz * 2}.5 {{PersistenceRequired:1b}}")
    return "\n".join(lines) + "\n"


def redstone_function(x, y, z):
    """128 clocks of observers facing each other: each one lights a lamp and pushes a block."""
    lines = []
    for row in range(16):
        for col in range(8):
            bx, bz = x - 20 + col * 6, z - 16 + row * 2
            lines += [
                f"setblock {bx - 1} {y + 1} {bz} minecraft:redstone_lamp",
                f"setblock {bx + 2} {y + 1} {bz} minecraft:sticky_piston[facing=east]",
                f"setblock {bx + 3} {y + 1} {bz} minecraft:slime_block",
                f"setblock {bx} {y + 1} {bz} minecraft:observer[facing=east]",
                f"setblock {bx + 1} {y + 1} {bz} minecraft:observer[facing=west]",
            ]
    return "\n".join(lines) + "\n"


def grid(n, cols, step, x0, z0):
    """`n` positions (x, z) in a grid of `cols` columns at step `step` from (x0, z0)."""
    return [(x0 + (i % cols) * step, z0 + (i // cols) * step) for i in range(n)]


KEEP_MOB = "PersistenceRequired:1b,NoAI:1b"


def mobs1000_function(x, y, z):
    """1,000 mobs (4 kinds) in a 32 × 32 grid at a step of 1.4 in a 46 × 46 pen."""
    kinds = ["cow", "sheep", "pig", "chicken"]
    lines = [f"fill {x - 23} {y + 1} {z - 23} {x + 23} {y + 1} {z + 23} minecraft:oak_fence hollow"]
    for i, (gx, gz) in enumerate(grid(1000, 32, 1.4, x - 21.7, z - 21.7)):
        lines.append(f"summon minecraft:{kinds[i % 4]} {gx:.2f} {y + 1} {gz:.2f} {{PersistenceRequired:1b}}")
    return "\n".join(lines) + "\n"


def armor(head="diamond_helmet", hand="iron_sword"):
    pieces = {"head": head, "chest": "iron_chestplate", "legs": "golden_leggings", "feet": "chainmail_boots", "mainhand": hand}
    return "equipment:{" + ",".join(f'{k}:{{id:"minecraft:{v}",count:1}}' for k, v in pieces.items()) + "}"


def humanoids_function(x, y, z):
    """160 equipped humanoids (full armour and weapon): zombies, skeletons, armor stands, mannequins."""
    kinds = [("zombie", "iron_sword"), ("skeleton", "bow"), ("armor_stand", "iron_sword"), ("mannequin", "iron_sword")]
    lines = []
    for i, (gx, gz) in enumerate(grid(160, 16, 2.5, x - 19.5, z - 12.5)):
        kind, hand = kinds[i % 4]
        lines.append(f"summon minecraft:{kind} {gx} {y + 1} {gz} {{{KEEP_MOB},Rotation:[180.0f,0.0f],{armor(hand=hand)}}}")
    return "\n".join(lines) + "\n"


def keyframes_function(x, y, z):
    """≈ 200 mobs animated by keyframes: camels, breezes, wardens (without AI, in place)."""
    kinds = ["camel", "breeze", "warden"]
    return "".join(f"summon minecraft:{kinds[i % 3]} {gx} {y + 1} {gz} {{{KEEP_MOB}}}\n"
                   for i, (gx, gz) in enumerate(grid(201, 15, 3, x - 20.5, z - 20.5)))


def items_function(x, y, z):
    """500 dropped items (never picked up nor despawned), 100 item frames placed on the ground, 100 filled shelves."""
    items = ["diamond", "apple", "iron_sword", "oak_log", "redstone", "compass", "golden_apple", "stone"]
    lines = []
    for i, (gx, gz) in enumerate(grid(500, 25, 1.5, x - 18.0, z - 20.0)):
        lines.append(f'summon minecraft:item {gx} {y + 1} {gz} {{Item:{{id:"minecraft:{items[i % 8]}",count:1}},PickupDelay:32767,Age:-32768}}')
    for i, (gx, gz) in enumerate(grid(100, 25, 1, x - 12, z + 12)):
        lines.append(f'summon minecraft:item_frame {gx} {y + 1} {gz} {{Facing:1b,Fixed:1b,Item:{{id:"minecraft:{items[i % 8]}",count:1}}}}')
    for i, (gx, gz) in enumerate(grid(100, 25, 1, x - 12, z + 16)):
        slots = ",".join(f'{{Slot:{s}b,id:"minecraft:{items[(i + s) % 8]}",count:1}}' for s in range(3))
        lines.append(f"setblock {gx} {y + 1} {gz} minecraft:oak_shelf[facing=north]{{Items:[{slots}]}}")
    return "\n".join(lines) + "\n"


def glowing_function(x, y, z):
    """100 glowing cows (outline of the entities that glow)."""
    return "".join(f"summon minecraft:cow {gx} {y + 1} {gz} {{{KEEP_MOB},Glowing:1b}}\n" for gx, gz in grid(100, 10, 2, x - 9.5, z - 9.5))


def names_function(x, y, z):
    """200 named cows, name always visible (name tags)."""
    return "".join(f'summon minecraft:cow {gx} {y + 1} {gz} {{{KEEP_MOB},CustomName:"Cow {i}",CustomNameVisible:1b}}\n'
                   for i, (gx, gz) in enumerate(grid(200, 20, 2, x - 19.5, z - 19.5)))


def signs_function(x, y, z):
    """50 glowing-text signs (every other one with a scrambled `§k` line), facing the camera,
    10 fixed text displays and 10 turned toward the camera."""
    lines = []
    for i, (gx, gz) in enumerate(grid(50, 10, 2, x - 9, z - 6)):
        second = '"§kscramble§r"' if i % 2 == 0 else f'"sign {i}"'
        lines.append(f'setblock {gx} {y + 1} {gz} minecraft:oak_sign[rotation=8]'
                     f'{{front_text:{{has_glowing_text:1b,color:"yellow",messages:["Rustcraft",{second},"line 3","line 4"]}}}}')
    for i, (gx, gz) in enumerate(grid(20, 10, 2, x - 9, z + 6)):
        mode = "fixed" if i < 10 else "center"
        lines.append(f'summon minecraft:text_display {gx} {y + 3} {gz} {{billboard:"{mode}",Rotation:[180.0f,0.0f],text:"Display {i}"}}')
    return "\n".join(lines) + "\n"


def block_entities_function(x, y, z):
    """Block entities: 2,016 chests (42 × 48), 500 signs on top, 200 spawners, 50 beacons
    on a layer of iron (the pad is 48 × 48)."""
    return "\n".join([
        f"fill {x - 21} {y + 1} {z - 24} {x + 20} {y + 1} {z + 23} minecraft:chest[facing=north]",
        f"fill {x - 21} {y + 2} {z - 24} {x - 2} {y + 2} {z} minecraft:oak_sign[rotation=8]",
        f"fill {x} {y + 2} {z - 24} {x + 19} {y + 2} {z - 15} minecraft:spawner",
        f"fill {x} {y + 2} {z + 19} {x + 9} {y + 2} {z + 23} minecraft:iron_block",
        f"fill {x} {y + 3} {z + 19} {x + 9} {y + 3} {z + 23} minecraft:beacon",
    ]) + "\n"


def tnt_function(x, y, z):
    """64 primed TNT (4 s fuse) in an 8 × 8 grid at a step of 3, primed again every 5 s."""
    return "".join(f"summon minecraft:tnt {gx} {y + 1} {gz} {{fuse:80s}}\n" for gx, gz in grid(64, 8, 3, x - 10.5, z - 10.5)) \
        + "schedule function perf:tnt 100t\n"


def fireworks_function(x, y, z):
    """16 firework rockets (large ball, trail, twinkle) every second."""
    rocket = ('{LifeTime:20,FireworksItem:{id:"minecraft:firework_rocket",count:1,components:{"minecraft:fireworks":'
              '{flight_duration:1b,explosions:[{shape:"large_ball",colors:[I;16711680,65280,255],has_trail:1b,has_twinkle:1b}]}}}}')
    lines = [f"summon minecraft:firework_rocket {gx} {y + 2} {gz} {rocket}" for gx, gz in grid(16, 4, 6, x - 9, z - 9)]
    lines.append("schedule function perf:fireworks 20t")
    return "\n".join(lines) + "\n"


# Pack lines run every tick: flight (2 blocks/tick), slow flight of the fresh-world paths (0.8 block/tick =
# 16 blocks/s, pitch kept), rotation (4°/tick), walking (0.22 block/tick).
TICK_FUNCTION = (
    "execute as @a[tag=perf_fly] at @s rotated ~ 0 run tp @s ^ ^ ^2\n"
    "execute as @a[tag=perf_glide] at @s rotated ~ 0 run tp @s ^ ^ ^0.8\n"
    "execute as @a[tag=perf_turn] at @s run tp @s ~ ~ ~ ~4 ~\n"
    "execute as @a[tag=perf_walk] at @s rotated ~ 0 run tp @s ^ ^ ^0.22\n"
)


def pack_functions(sites):
    """Functions of the `perf` pack: name -> text."""
    f, r = sites["crowd"], sites["redstone"]
    return {
        "tick": TICK_FUNCTION,
        "mobs": mobs_function(*f),
        "redstone": redstone_function(*r),
        "mobs1000": mobs1000_function(*f),
        "humanoids": humanoids_function(*f),
        "keyframes": keyframes_function(*f),
        "items": items_function(*f),
        "glowing": glowing_function(*f),
        "names": names_function(*f),
        "signs": signs_function(*f),
        "tnt": tnt_function(*f),
        "fireworks": fireworks_function(*f),
        "block_entities": block_entities_function(*r),
    }


def write_pack(world, sites):
    pack = os.path.join(world, "datapacks", "perf")
    fn = os.path.join(pack, "data", "perf", "function")
    tags = os.path.join(pack, "data", "minecraft", "tags", "function")
    os.makedirs(fn, exist_ok=True)
    os.makedirs(tags, exist_ok=True)

    def w(path, text):
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            f.write(text)

    w(os.path.join(pack, "pack.mcmeta"), json.dumps({"pack": {"description": "perf bench", "pack_format": 121, "min_format": 121, "max_format": 121}}))
    w(os.path.join(tags, "tick.json"), json.dumps({"values": ["perf:tick"]}))
    for name, text in pack_functions(sites).items():
        w(os.path.join(fn, f"{name}.mcfunction"), text)


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
        zones.append((ne if k == "nether" else ow, (x + dx) >> 4, (z + dz) >> 4, radius))
    return zones


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
    **{f"fps{n}": {"at": "plains", **STANDARD, "opts": {"maxFps": n}} for n in (60, 120, 144)},
    # Generation alone: world without regions, all cores then a single thread.
    "generation": {"at": "plains", "rd": 12, "secs": 40, "from": 20, "world": "fresh"},
    "generation-1thread": {"at": "plains", "rd": 12, "secs": 60, "from": 40, "world": "fresh", "env": {"RUSTCRAFT_THREADS": "1"}},
    # Integrated server: local network (host + 1, 2, 4 windowless guests), close-together saves.
    **{f"lan-{n}": {"at": "plains", "rd": 12, "secs": 50, "from": 25, "guests": n,
                    "cmd": cmd_of((60, "publish false creative 25599")), "env": {"RUSTCRAFT_LAN_AUTH": "0"}} for n in (1, 2, 4)},
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
for _n, (_a, _b, _pitch) in FRESH_PATHS.items():
    _yaw = 0.0 if _b[2] != _a[2] else -90.0
    for _rd in (8, 16, 32):
        SCENES[f"fresh-{_n}-rd{_rd}"] = {
            "pos": list(_a), "rot": [_yaw, _pitch], "model": "model-1234", "world": "fresh", "rd": _rd,
            "env": {"RUSTCRAFT_GENLOG": "0"},
            "secs": 40, "from": 22, "cmd": cmd_of((20, "time set noon"), (380, "tag @s add perf_glide"),
                                                  (240, "tag @s remove perf_glide")),
        }
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
    """Viewpoint of a scene: view from `scenes.json`, site + offset (`site`, `d`, `rot`), or
    given position (`pos`, `rot`)."""
    if "pos" in sc:
        return {"dim": "minecraft:overworld", "pos": sc["pos"], "rot": sc["rot"]}
    if "site" in sc:
        x, y, z = info["sites"][sc["site"]]
        return {"dim": "minecraft:overworld", "pos": [x + sc["d"][0], y + sc["d"][1], z + sc["d"][2]], "rot": sc["rot"]}
    return info["scenes"][sc["at"]]


def offline_uuid(name):
    h = bytearray(hashlib.md5(f"OfflinePlayer:{name}".encode()).digest())
    h[6] = (h[6] & 0x0F) | 0x30
    h[8] = (h[8] & 0x3F) | 0x80
    return str(uuidlib.UUID(bytes=bytes(h)))


def stage(run_dir, view, rd, extra_opts=None, fresh_world=False, model=None):
    """Fresh game folder: copy of the model, player placed, options fixed. `fresh_world`: without
    the regions (terrain, entities, points of interest) of all dimensions, everything is generated."""
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
        "playerGameType": (nbt.INT, 1),
        "abilities": {
            "flying": (nbt.BYTE, 1), "mayfly": (nbt.BYTE, 1), "instabuild": (nbt.BYTE, 1),
            "invulnerable": (nbt.BYTE, 1), "mayBuild": (nbt.BYTE, 1),
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
FIRST = re.compile(r"first frame: (\d+) ms")
GEN = re.compile(r"singleplayer: generated (\d+) chunks in ([\d.]+) ms")
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


def med(v):
    return statistics.median(v) if v else float("nan")


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
    pl = [p for p in (PERFL.search(l) for l in lines) if p]
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
    rv = [r for r in (RECV.search(l) for l in lines) if r and float(r.group(1)) <= 5.0]
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
    c = [x for x in (LOADED.search(l) for l in lines) if x]
    m["load_ms"] = float(c[0].group(1)) if c else float("nan")
    st = [x for x in (STABLE.search(l) for l in lines) if x]
    m["stable_ms"] = float(st[0].group(1)) if st else float("nan")
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
    sv = [float(x.group(2)) for x in (SAVE.search(l) for l in lines) if x]
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
              "RUSTCRAFT_AUTOSAVE_TICKS", "RUSTCRAFT_LAN_AUTH")


def launch_env(a, sc, out, game, cache_dir, own):
    env = dict(os.environ)
    env.update({
        "RUSTCRAFT_BACKGROUND": "1", "RUSTCRAFT_LAN_BIND": "127.0.0.1", "RUSTCRAFT_GAME_DIR": game,
        "RUSTCRAFT_SOLO": "perf", "RUSTCRAFT_CAPTURE": f"{out}/fin.bmp;{sc['secs']}",
        "RUSTCRAFT_PERFLOG": f"{out}/images.txt", "RUSTCRAFT_VSYNC": "0", "RUSTCRAFT_GENLOG": "1",
        "RUSTCRAFT_CACHE": cache_dir,
        # Special dates frozen: an Oct 31 (Halloween heads) or a Dec 24-26 would change the world
        # or the frame being measured.
        "RUSTCRAFT_HALLOWEEN": "0", "RUSTCRAFT_CHRISTMAS": "0",
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
    # `--env` for all, then `--exe path|VAR=val|VAR2=val`: variables specific to that exe.
    for kv in a.env + own:
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


def run_one(a, name, sc, info, pass_no, exe_no=0, warm=False):
    exe, own = split_exe(a.exe[exe_no])
    tag = f"{name}-{pass_no}" + (f"-{exe_no}" if len(a.exe) > 1 else "")
    out = os.path.join(PERF, a.label, f"warmup-{exe_no}" if warm else tag)
    game = os.path.join(PERF, "game")
    opts = dict(sc.get("opts", {}))
    if sc.get("sound"):
        opts["soundDevice"] = json.dumps(a.sound_device)
    if a.fullscreen:
        opts["fullscreen"] = "true"
    model = os.path.join(PERF, sc["model"]) if "model" in sc else None
    stage(game, view_of(sc, info), sc["rd"], opts, sc.get("world") == "fresh", model)
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
        with open(log, "w", encoding="utf-8") as f:
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
    ("load_ms", "meshed load ms", ".0f"), ("stable_ms", "last column ms", ".0f"),
    ("last_mesh_ms", "last mesh ms", ".0f"), ("receive_load_ms", "receive 0-5 s ms", ".0f"),
    ("audio_ready_ms", "audio ready ms", ".0f"), ("audio_joined_ms", "audio joined ms", ".0f"),
    ("audio_wait_ms", "audio wait ms", ".0f"), ("open_ms", "world opening ms", ".0f"),
    ("generator_ms", "generator ready ms", ".0f"), ("generated", "chunks generated", ".0f"),
    ("gen_ms_per_chunk", "generation ms/chunk", ".2f"), ("gen_chunks_s", "generation chunks/s", ".0f"),
    ("gen_first_batch_ms", "generation 1st batch ms", ".1f"), ("saves", "saves", ".0f"),
    ("save_snapshot_ms", "save snapshot med. ms", ".2f"), ("save_snapshot_max_ms", "save snapshot max ms", ".2f"),
    ("closing_total_ms", "closing save ms", ".1f"), ("guests_joined", "guests joined", ".0f"),
    ("guest_join_ms", "guest join ms", ".0f"), ("heap_mb", "heap MB", ".1f"),
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


def skip_reason(a, sc):
    if sc.get("sound") and not a.sound_device:
        return "no --sound-device"
    if sc.get("guests") and not (a.connect and os.path.exists(a.connect)):
        return "no --connect (compiled `connect` example of rc-client)"
    return None


def run(a):
    info = load_info()
    a.exe = expand_matrix(a.exe, a.matrix)
    names = a.scenes.split(",") if a.scenes else list(SCENES)
    unknown = [n for n in names if n not in SCENES]
    if unknown:
        raise SystemExit(f"unknown scenes: {unknown}")
    kept_names = []
    for n in names:
        why = skip_reason(a, SCENES[n])
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
    dyn = sorted({k for runs in res.values() for r in runs for k in r if k.startswith(("startup:", "c:"))})
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
    """Rewrites the `perf` pack of the existing model (new scenes without redoing `prepare`)."""
    info = load_info()
    write_pack(os.path.join(PERF, "model"), {k: tuple(v) for k, v in info["sites"].items()})
    print("perf pack rewritten:", ", ".join(pack_functions(info["sites"])))


def main():
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
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
    r = sub.add_parser("run")
    r.add_argument("--exe", action="append", required=True, help="repeatable: alternating A/B; path|VAR=val for a setting of its own")
    r.add_argument("--scenes", default="")
    r.add_argument("--passes", type=int, default=3)
    r.add_argument("--label", default="reference")
    r.add_argument("--env", action="append", default=[], help="VAR=value added to every launch")
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
    a = ap.parse_args()
    if a.cmd == "prepare":
        prepare(a)
    elif a.cmd == "seed-model":
        prepare_seed(a)
    elif a.cmd == "pack":
        pack(a)
    elif a.cmd == "run":
        run(a)
    else:
        res = reanalyse(a.label) if a.reanalyse else upgrade_results(
            json.load(open(os.path.join(PERF, a.label, "resultats.json"), encoding="utf-8")))
        print(table(res, a.cold))
        print(noise_report(res, a.cold))


if __name__ == "__main__":
    main()
