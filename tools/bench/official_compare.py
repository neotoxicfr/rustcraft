"""Compare Rustcraft with the official Minecraft 26.3 client on the same flights (Windows only).

Both games open the same fresh world (seed 1234, no regions: terrain is generated while flying), with the
same options.txt and the same small data pack that drives the camera: it holds the player at the start of
the path, sets noon, flies 192 blocks at 16 blocks per second from 20 s to 32 s after joining, then stops.
Frame times come from PresentMon (the same external capture for both games, time between presents), memory
and CPU time from the process counters, sampled 4 times a second. Only the flight is measured.

With `--mode load` the player stays at the start of the path (sun, weather fixed, no clouds) and the game
window is captured from the screen 10 times a second: the time until the view stops changing is how long
the terrain around the player takes to load and appear, measured the same way for both games.

    python tools/bench/official_compare.py run --model <seed model dir> --rustcraft <rustcraft.exe>
        --java <java.exe> --presentmon <PresentMon.exe> --out <dir> [--distances 16,32] [--passes 3]
        [--mode flight|load]
    python tools/bench/official_compare.py table --out <dir>

The model is the world built by `scenes.py` for seed 1234 (a world folder with level.dat and no regions).
The official client is started like the launcher does (its version JSON, its libraries, the launcher's
default JVM options) from the player's own .minecraft; nothing is downloaded.
"""
import argparse, csv, ctypes, datetime, hashlib, json, os, re, shutil, statistics, struct, subprocess, sys, time, uuid
from ctypes import wintypes

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import nbt  # noqa: E402

MC = os.path.join(os.environ.get("APPDATA", ""), ".minecraft")
VERSION = "26.3"
NAME = "Perf"
# Same paths as the scenes of scenes.py: start, end (192 blocks along +z or +x), pitch.
PATHS = {
    "village": ((-960.0, 125.0, 80.0), (-960.0, 125.0, 272.0), 35.0),
    "forest": ((-1100.0, 120.0, -560.0), (-1100.0, 120.0, -368.0), 25.0),
    "ocean": ((-300.0, 85.0, -420.0), (-300.0, 85.0, -228.0), 10.0),
    "mountain": ((560.0, 260.0, 860.0), (752.0, 260.0, 860.0), 30.0),
}
FLY_FROM, FLY_FOR, RUN_FOR = 20.0, 12.0, 34.0
# Loading runs: camera pitch (ground and horizon in view), end once the picture has not changed for
# LOAD_QUIET seconds (at least LOAD_MIN after joining, at most LOAD_MAX), captures of CAP_W×CAP_H
# every CAP_EVERY seconds; a pixel has changed when its luma differs by more than CHANGE_LEVEL.
LOAD_PITCH, LOAD_QUIET, LOAD_MIN, LOAD_MAX = 15.0, 20.0, 10.0, 150.0
CAP_W, CAP_H, CAP_EVERY, CHANGE_LEVEL = 320, 180, 0.1, 24
# The official game's defaults for the "detailed" preset, except the render distance (a named preset
# would force 16), unlimited frame rate, no VSync, master volume 0, no pause without focus.
OPTIONS = {
    "graphicsPreset": '"custom"', "biomeBlendRadius": 2, "prioritizeChunkUpdates": 1, "ao": "true",
    "renderClouds": '"true"', "particles": 0, "mipmapLevels": 4, "entityShadows": "true",
    "entityDistanceScaling": 1.0, "menuBackgroundBlurriness": 5, "cloudRange": 64, "cutoutLeaves": "true",
    "improvedTransparency": "false", "weatherRadius": 10, "maxAnisotropyBit": 1, "textureFiltering": 1,
    "simulationDistance": 12, "maxFps": 260, "enableVsync": "false", "fullscreen": "false",
    "onboardAccessibility": "false", "tutorialStep": "none", "skipMultiplayerWarning": "true",
    "joinedFirstServer": "true", "soundCategory_master": "0.0", "pauseOnLostFocus": "false",
    # Without input, both games otherwise cap the frame rate after a while.
    "inactivityFpsLimit": '"minimized"',
}


def offline_uuid(name):
    h = bytearray(hashlib.md5(f"OfflinePlayer:{name}".encode()).digest())
    h[6] = (h[6] & 0x0F) | 0x30
    h[8] = (h[8] & 0x3F) | 0x80
    return str(uuid.UUID(bytes=bytes(h)))


def stage(game_dir, model, scene, rd, load=False):
    """Fresh game folder: the model world without regions, options.txt, and the flight data pack.
    `load`: the player stays at the start of the path, looking ahead, sun and weather fixed, no clouds
    (nothing on screen moves but the terrain that loads)."""
    if os.path.exists(game_dir):
        shutil.rmtree(game_dir)
    world = os.path.join(game_dir, "saves", "perf")
    shutil.copytree(model, world, ignore=shutil.ignore_patterns("region", "entities", "poi", "session.lock"))
    # `version` first: without it the official game upgrades the file as a very old one and drops newer keys.
    version = nbt.read_dat(os.path.join(world, "level.dat"))["Data"]["DataVersion"][1]
    with open(os.path.join(game_dir, "options.txt"), "w", encoding="utf-8", newline="\n") as f:
        # No clouds, no vignette: the vignette fades in over about 8 s after joining, like the light.
        clouds = {"renderClouds": '"false"', "vignette": "false"} if load else {}
        f.writelines(f"{k}:{v}\n" for k, v in {"version": version, **OPTIONS, **clouds, "renderDistance": rd}.items())
    (x, y, z), end, pitch = PATHS[scene]
    yaw = 0.0 if end[2] != z else -90.0
    if load:
        pitch = LOAD_PITCH
    pack = os.path.join(world, "datapacks", "flight")
    fn = os.path.join(pack, "data", "flight", "function")
    tags = os.path.join(pack, "data", "minecraft", "tags", "function")
    os.makedirs(fn)
    os.makedirs(tags)
    files = {
        os.path.join(pack, "pack.mcmeta"): json.dumps({"pack": {"description": "flight bench", "pack_format": 121, "min_format": 121, "max_format": 121}}),
        os.path.join(tags, "tick.json"): json.dumps({"values": ["flight:tick"]}),
        os.path.join(fn, "tick.mcfunction"): (
            "execute as @a[tag=!flight_init] run function flight:init\n"
            f"execute as @a[tag=flight_hold] run tp @s {x} {y} {z} {yaw} {pitch}\n"
            "execute as @a[tag=flight_fly] at @s rotated ~ 0 run tp @s ^ ^ ^0.8\n"),
        os.path.join(fn, "init.mcfunction"): (
            "tag @s add flight_init\ntag @s add flight_hold\n"
            + ("" if load else "time set noon\n")
            + ("" if load else
               f"schedule function flight:start {int(FLY_FROM * 20)}t\n"
               f"schedule function flight:stop {int((FLY_FROM + FLY_FOR) * 20)}t\n")),
        os.path.join(fn, "start.mcfunction"): "tag @a remove flight_hold\ntag @a add flight_fly\n",
        os.path.join(fn, "stop.mcfunction"): "tag @a remove flight_fly\n",
    }
    for path, text in files.items():
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            f.write(text)
    level = nbt.read_dat(os.path.join(world, "level.dat"))
    dp = level["Data"].get("DataPacks")
    if isinstance(dp, dict) and isinstance(dp.get("Enabled"), nbt.List):
        if not any(v[1] == "file/flight" for v in dp["Enabled"].items):
            dp["Enabled"].items.append((nbt.STRING, "file/flight"))
    nbt.write_dat(os.path.join(world, "level.dat"), level)
    if load:
        # Noon already when the world opens, time and weather frozen: a `time set` at joining makes the
        # light change for several seconds, which the capture would count as loading.
        saved = os.path.join(world, "data", "minecraft")
        clocks = nbt.read_dat(os.path.join(saved, "world_clocks.dat"))
        clocks["data"]["minecraft:overworld"]["total_ticks"] = (nbt.LONG, 6000)
        nbt.write_dat(os.path.join(saved, "world_clocks.dat"), clocks)
        rules = nbt.read_dat(os.path.join(saved, "game_rules.dat"))
        rules["data"].update({"minecraft:advance_time": (nbt.BYTE, 0), "minecraft:advance_weather": (nbt.BYTE, 0)})
        nbt.write_dat(os.path.join(saved, "game_rules.dat"), rules)


def allowed(rules, features):
    if not rules:
        return True
    ok = False
    for r in rules:
        os_ok = r.get("os", {}).get("name") in (None, "windows") and r.get("os", {}).get("arch") in (None, "x86_64")
        feat_ok = all(features.get(k, False) == v for k, v in r.get("features", {}).items())
        if os_ok and feat_ok:
            ok = r.get("action") == "allow"
    return ok


def official_command(java, game_dir, size):
    """The launcher's command line for the official client, with its default JVM options."""
    v = json.load(open(os.path.join(MC, "versions", VERSION, f"{VERSION}.json"), encoding="utf-8"))
    cp = [os.path.join(MC, "libraries", lib["downloads"]["artifact"]["path"]) for lib in v["libraries"]
          if allowed(lib.get("rules"), {}) and "artifact" in lib.get("downloads", {})]
    cp.append(os.path.join(MC, "versions", VERSION, f"{VERSION}.jar"))
    natives = os.path.join(game_dir, "natives")
    w, h = size.split("x")
    values = {
        "auth_player_name": NAME, "version_name": VERSION, "game_directory": game_dir,
        "assets_root": os.path.join(MC, "assets"), "assets_index_name": v["assetIndex"]["id"],
        "auth_uuid": offline_uuid(NAME), "auth_access_token": "0", "clientid": "", "auth_xuid": "",
        "version_type": "release", "resolution_width": w, "resolution_height": h,
        "quickPlaySingleplayer": "perf", "natives_directory": natives, "launcher_name": "rustcraft-bench",
        "launcher_version": "1", "classpath": os.pathsep.join(cp),
    }
    features = {"has_custom_resolution": True, "is_quick_play_singleplayer": True}

    def expand(args):
        out = []
        for a in args:
            if isinstance(a, dict):
                if allowed(a.get("rules"), features):
                    out += [a["value"]] if isinstance(a["value"], str) else a["value"]
            else:
                out.append(a)
        for k, val in values.items():
            out = [x.replace("${" + k + "}", val) for x in out]
        return out

    launcher_jvm = ["-Xmx2G", "-XX:+UnlockExperimentalVMOptions", "-XX:+UseG1GC", "-XX:G1NewSizePercent=20",
                    "-XX:G1ReservePercent=20", "-XX:MaxGCPauseMillis=50", "-XX:G1HeapRegionSize=32M"]
    return [java] + launcher_jvm + expand(v["arguments"]["jvm"]) + [v["mainClass"]] + expand(v["arguments"]["game"])


class Counters(ctypes.Structure):
    _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD), ("PeakWorkingSetSize", ctypes.c_size_t),
                ("WorkingSetSize", ctypes.c_size_t), ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPagedPoolUsage", ctypes.c_size_t), ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t), ("PagefileUsage", ctypes.c_size_t),
                ("PeakPagefileUsage", ctypes.c_size_t), ("PrivateUsage", ctypes.c_size_t)]


def sample(pid):
    """Working set and private bytes (MB), and CPU time used so far (s), of a process."""
    k = ctypes.windll.kernel32
    h = k.OpenProcess(0x1000 | 0x0010, False, pid)
    if not h:
        return None
    try:
        c = Counters()
        c.cb = ctypes.sizeof(c)
        ctypes.windll.psapi.GetProcessMemoryInfo(h, ctypes.byref(c), c.cb)
        t = [wintypes.FILETIME() for _ in range(4)]
        k.GetProcessTimes(h, *[ctypes.byref(x) for x in t])
        cpu = sum((x.dwHighDateTime << 32 | x.dwLowDateTime) for x in t[2:]) / 1e7
        return c.WorkingSetSize / 2**20, c.PrivateUsage / 2**20, cpu
    finally:
        k.CloseHandle(h)


class BitmapInfo(ctypes.Structure):
    _fields_ = [("biSize", wintypes.DWORD), ("biWidth", wintypes.LONG), ("biHeight", wintypes.LONG),
                ("biPlanes", wintypes.WORD), ("biBitCount", wintypes.WORD), ("biCompression", wintypes.DWORD),
                ("biSizeImage", wintypes.DWORD), ("biXPelsPerMeter", wintypes.LONG), ("biYPelsPerMeter", wintypes.LONG),
                ("biClrUsed", wintypes.DWORD), ("biClrImportant", wintypes.DWORD), ("colors", wintypes.DWORD)]


class Screen:
    """Small grey copies (CAP_W×CAP_H luma bytes) of a process's game window, read from the screen."""

    def __init__(self, pid):
        self.pid, self.hwnd = pid, None
        u, g = ctypes.windll.user32, ctypes.windll.gdi32
        u.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
        u.IsWindowVisible.argtypes = [wintypes.HWND]
        u.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
        u.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
        u.GetDC.argtypes, u.GetDC.restype = [wintypes.HWND], wintypes.HDC
        g.CreateCompatibleDC.argtypes, g.CreateCompatibleDC.restype = [wintypes.HDC], wintypes.HDC
        g.CreateCompatibleBitmap.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int]
        g.CreateCompatibleBitmap.restype = wintypes.HBITMAP
        g.SelectObject.argtypes, g.SelectObject.restype = [wintypes.HDC, wintypes.HGDIOBJ], wintypes.HGDIOBJ
        g.SetStretchBltMode.argtypes = [wintypes.HDC, ctypes.c_int]
        g.StretchBlt.argtypes = [wintypes.HDC] + [ctypes.c_int] * 4 + [wintypes.HDC] + [ctypes.c_int] * 4 + [wintypes.DWORD]
        g.GetDIBits.argtypes = [wintypes.HDC, wintypes.HBITMAP, wintypes.UINT, wintypes.UINT, ctypes.c_void_p,
                                ctypes.POINTER(BitmapInfo), wintypes.UINT]
        self.u, self.g = u, g
        self.screen = u.GetDC(None)
        self.mem = g.CreateCompatibleDC(self.screen)
        # GetDIBits wants its bitmap outside any DC: a 1×1 bitmap takes its place while reading.
        self.bmp, self.spare = (g.CreateCompatibleBitmap(self.screen, w, h) for w, h in ((CAP_W, CAP_H), (1, 1)))
        g.SelectObject(self.mem, self.bmp)
        g.SetStretchBltMode(self.mem, 4)  # HALFTONE: averages the pixels it shrinks
        self.info = BitmapInfo(ctypes.sizeof(BitmapInfo) - 4, CAP_W, -CAP_H, 1, 32, 0)
        self.buf = ctypes.create_string_buffer(CAP_W * CAP_H * 4)

    def find(self):
        best = []

        def each(hwnd, _):
            p = wintypes.DWORD()
            self.u.GetWindowThreadProcessId(hwnd, ctypes.byref(p))
            if p.value == self.pid and self.u.IsWindowVisible(hwnd):
                r = wintypes.RECT()
                self.u.GetClientRect(hwnd, ctypes.byref(r))
                best.append((r.right * r.bottom, hwnd))
            return True

        self.u.EnumWindows(ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)(each), 0)
        self.hwnd = max(best)[1] if best and max(best)[0] > 0 else None

    def grab(self):
        """Luma of the client area, or None while the window is not there."""
        if self.hwnd is None:
            self.find()
            if self.hwnd is None:
                return None
        r, p = wintypes.RECT(), wintypes.POINT(0, 0)
        if not self.u.GetClientRect(self.hwnd, ctypes.byref(r)) or r.right == 0:
            self.hwnd = None
            return None
        self.u.ClientToScreen(self.hwnd, ctypes.byref(p))
        self.g.StretchBlt(self.mem, 0, 0, CAP_W, CAP_H, self.screen, p.x, p.y, r.right, r.bottom, 0x00CC0020)
        self.g.SelectObject(self.mem, self.spare)
        self.g.GetDIBits(self.screen, self.bmp, 0, CAP_H, self.buf, ctypes.byref(self.info), 0)
        self.g.SelectObject(self.mem, self.bmp)
        raw = self.buf.raw
        return bytes((b * 29 + g * 150 + r * 77) >> 8 for b, g, r in zip(raw[0::4], raw[1::4], raw[2::4]))


# Pixels compared: the interface is left out (toasts in the top right corner, chat in the bottom left,
# hotbar at the bottom).
KEEP = [y * CAP_W + x for y in range(CAP_H) for x in range(CAP_W)
        if y < CAP_H * 0.88 and not (x >= CAP_W * 0.6 and y < CAP_H * 0.25) and not (x < CAP_W * 0.75 and y >= CAP_H * 0.7)]


def changed(x, y):
    """Fraction of the compared pixels whose luma differs by more than CHANGE_LEVEL."""
    return sum(1 for i in KEEP if abs(x[i] - y[i]) > CHANGE_LEVEL) / len(KEEP)


def run_one(a, game, scene, rd, out):
    game_dir = os.path.join(a.out, "game", game)
    stage(game_dir, a.model, scene, rd, load=a.mode == "load")
    if a.fullscreen:
        path = os.path.join(game_dir, "options.txt")
        text = open(path, encoding="utf-8").read().replace("fullscreen:false\n", "fullscreen:true\n")
        open(path, "w", encoding="utf-8", newline="\n").write(text)
    if game == "official" and a.official_backend:
        # The official game draws with OpenGL by default; 26.3 can also use Vulkan.
        with open(os.path.join(game_dir, "options.txt"), "a", encoding="utf-8", newline="\n") as f:
            f.write(f'preferredGraphicsBackend:"{a.official_backend}"\n')
    os.makedirs(out, exist_ok=True)
    log = open(os.path.join(out, "game.log"), "w", encoding="utf-8")
    env = dict(os.environ)
    if game == "rustcraft":
        # The performance log is what prints the "in game" line used to place the flight window.
        env.update({"RUSTCRAFT_GAME_DIR": game_dir, "RUSTCRAFT_SOLO": "perf", "RUSTCRAFT_VSYNC": "0",
                    "RUSTCRAFT_LAN_BIND": "127.0.0.1", "RUSTCRAFT_PERFLOG": os.path.join(out, "frames-internal.txt")})
        w, h = a.size.split("x")
        cmd = [a.rustcraft, "--username", NAME, "--width", w, "--height", h]
        joined_file, joined_marks = os.path.join(out, "game.log"), ("en jeu à", "in game at")
    else:
        cmd = official_command(a.java, game_dir, a.size)
        joined_file, joined_marks = os.path.join(game_dir, "logs", "latest.log"), (f"{NAME} joined the game",)
    started = time.time()
    p = subprocess.Popen(cmd, cwd=game_dir, env=env, stdout=log, stderr=subprocess.STDOUT)
    load = a.mode == "load"
    pm = None
    if not load:
        # Only the time between presents is used. With the default 2,048 presents in flight PresentMon loses
        # presents above about 2,000 a second (gaps of hundreds of ms that the game's own frame log does not
        # have). Display tracking stays: Vulkan and OpenGL presents are only seen through it.
        pm = subprocess.Popen([a.presentmon, "--process_id", str(p.pid), "--output_file", os.path.join(out, "frames.csv"),
                               "--date_time", "--terminate_on_proc_exit", "--no_console_stats", "--stop_existing_session",
                               "--no_track_gpu", "--no_track_input", "--set_circular_buffer_size", "65536"],
                              stdout=open(os.path.join(out, "presentmon.log"), "w"), stderr=subprocess.STDOUT)
    screen = Screen(p.pid) if load else None
    shots = open(os.path.join(out, "frames.bin"), "wb") if load else None
    # Distant terrain can load a little at a time: each capture is compared with the one 5 s earlier.
    joined, samples, recent, last_change = None, [], [], None
    while time.time() - started < 300:
        time.sleep(0.1)
        if p.poll() is not None:
            break
        if joined is None and os.path.exists(joined_file):
            with open(joined_file, encoding="utf-8", errors="replace") as f:
                if any(m in f.read() for m in joined_marks):
                    joined = last_change = time.time()
        s = sample(p.pid)
        if s and joined is not None:
            samples.append((time.time() - joined, *s))
        if load:
            shot, now = screen.grab(), time.time()
            if shot is not None:
                shots.write(struct.pack("<d", now) + shot)
                recent.append((now, shot))
                while len(recent) > 1 and recent[1][0] <= now - 5.0:
                    recent.pop(0)
                if joined is not None and recent[0][0] <= now - 5.0 and changed(recent[0][1], shot) > 0.001:
                    last_change = now
            if joined is not None and now - joined > LOAD_MIN and (now - last_change > LOAD_QUIET or now - joined > LOAD_MAX):
                break
        elif joined is not None and time.time() - joined > RUN_FOR:
            break
    ended = time.time()
    p.kill()
    p.wait()
    if shots:
        shots.close()
    if pm:
        try:
            pm.wait(timeout=5)
        except subprocess.TimeoutExpired:
            # PresentMon does not always notice the exit: stop its session, which flushes the file.
            subprocess.run([a.presentmon, "--terminate_existing_session"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            pm.wait(timeout=30)
    log.close()
    meta = {"game": game, "scene": scene, "rd": rd, "mode": a.mode, "started": started, "joined": joined, "ended": ended,
            "startup_to_join_s": (joined - started) if joined else None, "samples": samples}
    json.dump(meta, open(os.path.join(out, "run.json"), "w", encoding="utf-8"))
    return meta


def png(path, luma):
    """Grey PNG of a capture, to look at the frames the figures point to."""
    import zlib
    rows = b"".join(b"\0" + luma[y * CAP_W:(y + 1) * CAP_W] for y in range(CAP_H))
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))  # noqa: E731
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", CAP_W, CAP_H, 8, 0, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def analyse_load(out, meta):
    """Loading run: seconds after joining until the picture is within 5 % and 0.2 % of its final state
    (share of pixels that still differ from the last capture), the same way for both games; for
    Rustcraft also its own mark (every loaded column meshed)."""
    size = CAP_W * CAP_H
    data = open(os.path.join(out, "frames.bin"), "rb").read()
    shots = [(struct.unpack_from("<d", data, i)[0], data[i + 8:i + 8 + size]) for i in range(0, len(data), 8 + size)]
    shots = [(t - meta["joined"], s) for t, s in shots if t >= meta["joined"]]
    if len(shots) < 2:
        return None
    final = shots[-1][1]
    diffs = [(t, changed(s, final)) for t, s in shots]

    def settled(limit):
        late = [i for i, (_, d) in enumerate(diffs) if d > limit]
        return diffs[late[-1] + 1][0] if late else diffs[0][0]

    most, done = settled(0.05), settled(0.002)
    for name, t in (("most", most), ("done", done), ("final", diffs[-1][0])):
        png(os.path.join(out, f"{name}.png"), min(shots, key=lambda x: abs(x[0] - t))[1])
    internal = None
    text = open(os.path.join(out, "game.log"), encoding="utf-8", errors="replace").read()
    # Builds from before the English translation print the French lines.
    m1 = re.search(r"\[perf\] (?:in game at|en jeu à) (\d+) ms", text)
    m2 = re.search(r"\[perf\] (?:initial load meshed at|chargement initial maillé à) (\d+) ms", text)
    if m1 and m2:
        internal = (int(m2.group(1)) - int(m1.group(1))) / 1000.0
    return {"game": meta["game"], "scene": meta["scene"], "rd": meta["rd"], "visible_95_s": most, "visible_all_s": done,
            "rustcraft_mark_s": internal if internal is not None else float("nan"),
            "start_to_all_s": done + meta["startup_to_join_s"], "watched_s": diffs[-1][0]}


def analyse(out):
    """Frame figures over the flight window, computed the same way for both games."""
    meta = json.load(open(os.path.join(out, "run.json"), encoding="utf-8"))
    if not meta["joined"]:
        return None
    if meta.get("mode") == "load":
        return analyse_load(out, meta)
    t0, t1 = meta["joined"] + FLY_FROM + 0.5, meta["joined"] + FLY_FROM + FLY_FOR - 0.5
    frames = []
    path = os.path.join(out, "frames.csv")
    if os.path.exists(path):
        for row in csv.DictReader(open(path, encoding="utf-8", errors="replace")):
            when = row.get("CPUStartDateTime") or row.get("CPUStartTime")
            try:
                ts = datetime.datetime.fromisoformat(when.replace(" ", "T")[:26]).timestamp()
                frames.append((ts, float(row["MsBetweenPresents"])))
            except (TypeError, ValueError, KeyError, AttributeError):
                continue
    if not frames:
        return None
    # PresentMon's date can be shifted by whole time-zone steps: the game renders until it is killed, so
    # the last frame is at the end time, and the shift is that difference rounded to a quarter of an hour.
    skew = round((max(f[0] for f in frames) - meta["ended"]) / 900.0) * 900.0
    ft = [ms for ts, ms in frames if t0 <= ts - skew <= t1]
    if not ft:
        return None
    s = sorted(ft)
    n = len(s)
    slow = lambda frac: 1000.0 / statistics.mean(s[-max(1, int(n * frac)):])  # noqa: E731
    win = [x for x in meta["samples"] if FLY_FROM + 0.5 <= x[0] <= FLY_FROM + FLY_FOR - 0.5]
    cpu = (win[-1][3] - win[0][3]) / (win[-1][0] - win[0][0]) if len(win) > 1 else float("nan")
    return {"game": meta["game"], "scene": meta["scene"], "rd": meta["rd"], "frames": n,
            "avg_fps": n / (sum(s) / 1000.0), "low_1pct_fps": slow(0.01), "low_01pct_fps": slow(0.001),
            "p50_ms": s[n // 2], "p99_ms": s[min(n - 1, int(n * 0.99))], "max_ms": s[-1],
            "working_set_mb": statistics.median(x[1] for x in win) if win else float("nan"),
            "working_set_peak_mb": max((x[1] for x in meta["samples"]), default=float("nan")),
            "cpu_cores": cpu, "startup_to_join_s": meta["startup_to_join_s"]}


def table(out_dir):
    rows = []
    for d in sorted(os.listdir(os.path.join(out_dir, "runs"))):
        r = analyse(os.path.join(out_dir, "runs", d))
        if r:
            rows.append(r)
    if rows and "visible_all_s" in rows[0]:
        keys = ["visible_95_s", "visible_all_s", "rustcraft_mark_s", "start_to_all_s", "watched_s"]
    else:
        keys = ["avg_fps", "low_1pct_fps", "low_01pct_fps", "p50_ms", "p99_ms", "max_ms", "working_set_mb",
                "working_set_peak_mb", "cpu_cores", "startup_to_join_s"]
    print("| scene | RD | game | runs | " + " | ".join(keys) + " |")
    print("|---|---|---|---|" + "---|" * len(keys))
    groups = {}
    for r in rows:
        groups.setdefault((r["scene"], r["rd"], r["game"]), []).append(r)
    for (scene, rd, game), rs in sorted(groups.items()):
        vals = " | ".join(f"{statistics.mean(r[k] for r in rs):.2f}" for k in keys)
        print(f"| {scene} | {rd} | {game} | {len(rs)} | {vals} |")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--model", required=True)
    r.add_argument("--rustcraft", required=True)
    r.add_argument("--java", required=True)
    r.add_argument("--presentmon", required=True)
    r.add_argument("--out", required=True)
    r.add_argument("--distances", default="16,32")
    r.add_argument("--scenes", default=",".join(PATHS))
    r.add_argument("--passes", type=int, default=3)
    r.add_argument("--size", default="1920x1080")
    r.add_argument("--games", default="rustcraft,official")
    r.add_argument("--official-backend", default="", help="opengl or vulkan (default: the game's own choice)")
    r.add_argument("--fullscreen", action="store_true", help="both games in full screen at the display's own size")
    r.add_argument("--mode", choices=["flight", "load"], default="flight",
                   help="flight: frame times while flying; load: time until the view has finished loading")
    t = sub.add_parser("table")
    t.add_argument("--out", required=True)
    a = ap.parse_args()
    if a.cmd == "table":
        table(a.out)
        return
    os.makedirs(os.path.join(a.out, "runs"), exist_ok=True)
    for game in a.games.split(","):
        # One unmeasured warm-up launch per game (shader caches, JIT, file cache).
        run_one(a, game, "village", 16, os.path.join(a.out, "warmup", game))
        for p in range(1, a.passes + 1):
            for rd in [int(x) for x in a.distances.split(",")]:
                for scene in a.scenes.split(","):
                    tag = f"{game}-{scene}-rd{rd}-{p}"
                    m = run_one(a, game, scene, rd, os.path.join(a.out, "runs", tag))
                    print(f"{tag}: joined after {m['startup_to_join_s'] or float('nan'):.1f} s", flush=True)
    table(a.out)


if __name__ == "__main__":
    main()
