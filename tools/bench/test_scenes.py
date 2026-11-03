"""Bench tests (`python -I tools/bench/test_scenes.py`): log analysis (real and new formats),
exclusion of cold launches, camera-centred pre-generation, scene consistency, profile
window, compatibility with data written before the English renaming. Expected values are
stored here; explicit failure if the data is missing."""
import argparse
import json
import math
import os
import re
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.dirname(HERE))
import window  # noqa: E402
import scenes  # noqa: E402

DATA = os.path.join(HERE, "test_data")


def data(name):
    p = os.path.join(DATA, name)
    if not os.path.exists(p):
        raise AssertionError(f"test data missing: {p}")
    return p


def analyse(log, images="absent.txt"):
    return scenes.analyse(data(log), os.path.join(DATA, images), 25, 45)


class Logs(unittest.TestCase):
    def test_real_logs_cold_and_warm(self):
        # Rerun of 2026-10-19, plains12 of the reference exe: pass 1 (cache to write) and pass 2.
        cold, warm = analyse("plains12-cold.log"), analyse("plains12-warm.log")
        self.assertEqual(cold["cold"], 1.0)
        self.assertEqual(warm["cold"], 0.0)
        expected = {
            "load_ms": (1296.0, 1347.0), "first_frame_ms": (679.0, 710.0), "in_game_ms": (808.0, 885.0),
            "audio_ready_ms": (26.0, 394.0), "generator_ms": (474.0, 513.0), "closing_total_ms": (94.9, 91.8),
            "cpu_ms": (0.107, 0.106), "gpu_ms": (0.105, 0.104), "last_mesh_ms": (44700.0, 44800.0),
        }
        for k, (c, w) in expected.items():
            self.assertAlmostEqual(cold[k], c, places=6, msg=k)
            self.assertAlmostEqual(warm[k], w, places=6, msg=k)

    def test_new_formats(self):
        m = analyse("new.log", "images.txt")
        self.assertEqual(m["cold"], 0.0)
        self.assertEqual(m["startup:data"], 12.0)  # first milestone of that name
        self.assertEqual(m["startup:resources"], 70.0)
        self.assertEqual((m["audio_ready_ms"], m["audio_joined_ms"], m["audio_wait_ms"]), (22.0, 3.0, 0.0))
        self.assertEqual((m["open_ms"], m["generator_ms"]), (318.0, 450.0))
        self.assertEqual((m["generated"], m["gen_first_batch_ms"]), (64, 40.0))
        self.assertAlmostEqual(m["gen_ms_per_chunk"], 120.0 / 64)
        self.assertAlmostEqual(m["gen_chunks_s"], 64e3 / 120.0)
        self.assertEqual(m["c:write_buffer"], 40.0)  # 26 and 27 s only (window 25-44 s)
        self.assertEqual((m["saves"], m["save_snapshot_ms"], m["save_snapshot_max_ms"]), (3, 25.0, 30.0))
        self.assertEqual(m["closing_total_ms"], 94.9)
        self.assertAlmostEqual(m["frames_s"], 3 / 0.012)
        self.assertEqual((m["frame_med_ms"], m["frame_max_ms"]), (4.0, 6.0))

    def test_absent_stay_empty(self):
        m = analyse("plains12-warm.log")
        for k in ("audio_joined_ms", "open_ms", "save_snapshot_ms", "gen_first_batch_ms"):
            self.assertTrue(math.isnan(m[k]), k)
        self.assertFalse(any(k.startswith(("startup:", "c:")) for k in m))


class Table(unittest.TestCase):
    def setUp(self):
        self.cold, self.warm = analyse("plains12-cold.log"), analyse("plains12-warm.log")

    def row(self, text, label):
        line = next(l for l in text.splitlines() if l.startswith(f"| {label} |"))
        return [c.strip() for c in line.strip("|").split("|")][1:]

    def test_cold_excluded_by_default(self):
        res = {"plains12#0": [self.cold, self.warm]}
        self.assertEqual(self.row(scenes.table(res), "meshed load ms"), ["1347"])
        self.assertEqual(self.row(scenes.table(res), "cold runs excluded"), ["1"])
        self.assertEqual(self.row(scenes.table(res, True), "meshed load ms"), ["1322 (1296–1347)"])

    def test_cold_scenes_keep_everything(self):
        res = {"cold-resources": [self.cold], "fresh-install#1": [self.cold]}
        self.assertEqual(self.row(scenes.table(res), "meshed load ms"), ["1296", "1296"])

    def test_noise_and_verdict(self):
        a = [{"frames_s": v} for v in (100.0, 104.0, 102.0)]
        b = [{"frames_s": v} for v in (103.0, 105.0, 104.0)]
        c = [{"frames_s": v} for v in (120.0, 121.0, 122.0)]
        rep = scenes.noise_report({"s#0": a, "s#1": b, "s#2": c})
        self.assertIn("| s#0 | 3.9 % |", rep)
        self.assertIn("+2.0 % (within noise)", rep)
        self.assertIn("+18.6 % (above)", rep)
        rep = scenes.noise_report({"s#0": a[:1], "s#1": c[:1]})
        self.assertIn("(noise unknown)", rep)

    def test_noise_unknown_in_both_orders(self):
        three = [{"frames_s": v} for v in (1000.0, 1010.0, 1020.0)]
        one = [{"frames_s": 1015.0}]
        for ref, var in ((three, one), (one, three)):
            rep = scenes.noise_report({"s#0": ref, "s#1": var})
            self.assertIn("(noise unknown)", rep)
            self.assertNotIn("(within noise)", rep)
        self.assertTrue(math.isnan(scenes.noise_of([1000.0, 1010.0, 1020.0], [1015.0])))
        self.assertTrue(math.isnan(scenes.noise_of([1015.0], [1000.0, 1010.0, 1020.0])))
        self.assertAlmostEqual(scenes.noise_of([100.0, 102.0], [100.0, 110.0]), 10.0 / 105.0 * 100)

    def test_cache_filled_with_subfolders(self):
        base = os.path.join(scenes.ROOT, "target", "bench-tests")
        os.makedirs(base, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=base) as t:
            src, dest = os.path.join(t, "src"), os.path.join(t, "dest")
            os.makedirs(os.path.join(src, "generated", "reports", "biome_parameters", "minecraft"))
            for f in ("data.bin", "assets-0123.bin", "x.tmp",
                      os.path.join("generated", "reports", "biome_parameters", "minecraft", "overworld.json")):
                open(os.path.join(src, f), "w").close()
            scenes.seed_cache(dest, src)
            d = os.path.join(dest, "26.3")
            self.assertTrue(os.path.isfile(os.path.join(d, "generated", "reports", "biome_parameters", "minecraft", "overworld.json")))
            self.assertTrue(os.path.isfile(os.path.join(d, "data.bin")))
            self.assertFalse(os.path.exists(os.path.join(d, "assets-0123.bin")))
            self.assertFalse(os.path.exists(os.path.join(d, "x.tmp")))
            # File already present: kept as is (a second pass copies nothing).
            with open(os.path.join(d, "data.bin"), "w") as f:
                f.write("kept")
            scenes.seed_cache(dest, src)
            with open(os.path.join(d, "data.bin")) as f:
                self.assertEqual(f.read(), "kept")

    def test_folders_reread(self):
        self.assertEqual(scenes.parse_run_dir("crowd-1000-1"), ("crowd-1000", 1, None))
        self.assertEqual(scenes.parse_run_dir("crowd-1000-2-1"), ("crowd-1000", 2, 1))
        self.assertEqual(scenes.parse_run_dir("crowd-2-1"), ("crowd", 2, 1))
        self.assertEqual(scenes.parse_run_dir("lan-1-2"), ("lan-1", 2, None))
        self.assertEqual(scenes.parse_run_dir("plains12-2-1"), ("plains12", 2, 1))
        self.assertEqual(scenes.parse_run_dir("plains12-3"), ("plains12", 3, None))
        self.assertEqual(scenes.parse_run_dir("particles-1000-1"), ("particles-1000", 1, None))
        self.assertEqual(scenes.parse_run_dir("particles-1000-2-0"), ("particles-1000", 2, 0))
        self.assertIsNone(scenes.parse_run_dir("warmup-0"))


class Scenes(unittest.TestCase):
    FOUND = {"plains": (100, None, 200), "forest": (1000, None, 1000), "village": (-500, 70, 300),
             "ocean": (3000, None, -3000), "cave": (50, 20, 50), "nether": (16, 64, 16)}
    SITES = {"crowd": (740, 200), "redstone": (740, 840)}

    def test_camera_centred_pregeneration(self):
        z = {(d, x, zz): r for d, x, zz, r in scenes.pregen_zones(self.FOUND, self.SITES)}
        ow = "minecraft:overworld"
        self.assertEqual(z[(ow, 100 >> 4, 200 >> 4)], 33)
        self.assertEqual(z[(ow, 740 >> 4, (200 - 22) >> 4)], 16)  # crowd camera, 22 blocks north
        self.assertEqual(z[(ow, 740 >> 4, (840 - 26) >> 4)], 16)
        self.assertEqual(z[(ow, -500 >> 4, (300 - 20) >> 4)], 16)
        self.assertEqual(z[("minecraft:the_nether", 1, 1)], 16)
        old = scenes.pregen_zones(self.FOUND, self.SITES, 13, "site")
        self.assertIn((ow, 740 >> 4, 200 >> 4, 13), old)
        self.assertEqual(len(old), len(z))

    def test_tnt_explodes_in_the_window(self):
        sc = scenes.SCENES["tnt"]
        wait = int(sc["cmd"].split("|")[0][1:])
        for load in (0.9, 1.5):  # entering the game, measured in game
            boom = load + wait / 20 + 4  # 80-tick fuse
            self.assertGreater(boom, sc["from"], load)
            self.assertLess(boom, sc["from"] + 5, load)  # at least 3 volleys in the window
        f = scenes.pack_functions({"crowd": (740, 64, 200), "redstone": (740, 64, 840)})
        self.assertIn("schedule function perf:tnt 100t", f["tnt"])

    def test_rotation_alone_in_the_window(self):
        self.assertNotIn("perf_walk", scenes.SCENES["plains32-rotation"]["cmd"])
        self.assertNotIn("remove perf_turn", scenes.SCENES["plains32-rotation"]["cmd"])

    def test_radius_covers_the_distance(self):
        for n, sc in scenes.SCENES.items():
            if sc.get("world") != "fresh" and sc.get("at", sc.get("site")) != "plains":
                self.assertGreaterEqual(16, sc["rd"] + 2, n)

    def test_consistent_scenes(self):
        funcs = scenes.pack_functions({"crowd": (740, 64, 200), "redstone": (740, 64, 840)})
        views = {"plains", "forest", "village", "ocean", "cave", "nether", "crowd", "redstone", "trial", "fortress"}
        steps = {"/", "o", "tab", "stab", "up", "down", "left", "right", "esc", "enter", "bs"}
        located = set(scenes.LOCATED_FRESH_BIOMES) | {"nether"}
        for n, sc in scenes.SCENES.items():
            self.assertTrue(sc.get("at") in views or sc.get("site") in ("crowd", "redstone") or "pos" in sc
                            or sc.get("fresh_path") in located, n)
            self.assertLess(sc["from"], sc["secs"] - 5, n)
            for s in sc.get("cmd", "").split("|") if sc.get("cmd") else []:
                self.assertTrue(s in steps or s[:1] in "wt", f"{n}: step {s}")
                if s.startswith("tfunction perf:"):
                    self.assertIn(s[len("tfunction perf:"):], funcs, n)
        self.assertEqual(scenes.COLD_SCENES, {"cold-resources", "first-launch", "fresh-install"})

    def test_function_quantities(self):
        f = scenes.pack_functions({"crowd": (0, 64, 0), "redstone": (0, 64, 0)})
        self.assertTrue(all(isinstance(t, str) for t in f.values()))
        self.assertEqual([l.split("tag=")[1][:9] for l in f["tick"].splitlines()],
                         ["perf_fly]", "perf_glid", "perf_turn", "perf_walk", "perf_chat", "perf_reap", "perf_data"])
        count = lambda name, word: f[name].count(word)  # noqa: E731
        self.assertEqual(count("mobs", "summon"), 100)
        self.assertEqual(count("mobs1000", "summon"), 1000)
        self.assertEqual(count("names", "CustomNameVisible"), 200)
        self.assertEqual(count("glowing", "Glowing"), 100)
        self.assertEqual(count("humanoids", "summon"), 160)
        self.assertEqual(count("keyframes", "summon"), 201)
        self.assertEqual((count("items", "minecraft:item "), count("items", "item_frame"), count("items", "oak_shelf")), (500, 100, 100))
        self.assertEqual((count("signs", "oak_sign"), count("signs", "text_display")), (50, 20))
        self.assertEqual(count("signs", "§k"), 25)
        self.assertEqual(count("tnt", "summon"), 64)
        self.assertIn("schedule function perf:fireworks", f["fireworks"])
        # Chests: 42 × 48 = 2016, signs 20 × 25, spawners 20 × 10, beacons 10 × 5.
        self.assertIn("fill -21 65 -24 20 65 23 minecraft:chest", f["block_entities"])
        self.assertIn("fill -21 66 -24 -2 66 0 minecraft:oak_sign", f["block_entities"])
        self.assertIn("fill 0 66 -24 19 66 -15 minecraft:spawner", f["block_entities"])
        self.assertIn("fill 0 67 19 9 67 23 minecraft:beacon", f["block_entities"])

    def test_matrix_and_order(self):
        self.assertEqual(scenes.expand_matrix(["a.exe"], ["X=0", "Y=0"]), ["a.exe", "a.exe|X=0", "a.exe|Y=0", "a.exe|X=0|Y=0"])
        self.assertEqual(scenes.expand_matrix(["a.exe", "b.exe"], []), ["a.exe", "b.exe"])
        self.assertEqual([scenes.rotated(3, p) for p in (1, 2, 3, 4)], [[0, 1, 2], [1, 2, 0], [2, 0, 1], [0, 1, 2]])

    def test_view_by_site(self):
        info = {"sites": {"crowd": [740, 64, 200]}, "scenes": {}}
        v = scenes.view_of(scenes.SCENES["crowd-close-up"], info)
        self.assertEqual(v["pos"], [740.5, 66.0, 193.5])


RUST_READ = re.compile(r'env_(flag|str|os)!\(\s*"(RUSTCRAFT_[A-Z0-9_]+)"|read_os\("(RUSTCRAFT_[A-Z0-9_]+)"\)')
OFF_VALUES = {"0", "false", "off", ""}


def rust_env_reads():
    """`RUSTCRAFT_*` variables read by the Rust sources: name -> {"flag"} (presence only) and/or
    {"value"}. Explicit failure if no source is found."""
    reads = {}
    for top in ("apps", "crates"):
        for d, dirs, files in os.walk(os.path.join(scenes.ROOT, top)):
            dirs[:] = [x for x in dirs if x != "target"]
            for f in (f for f in files if f.endswith(".rs")):
                with open(os.path.join(d, f), encoding="utf-8", errors="replace") as fh:
                    for m in RUST_READ.finditer(fh.read()):
                        kind = "flag" if m.group(1) == "flag" else "value"
                        reads.setdefault(m.group(2) or m.group(3), set()).add(kind)
    if len(reads) < 50:
        raise AssertionError(f"Rust sources not found under {scenes.ROOT}")
    return reads


def bench_dir():
    base = os.path.join(scenes.ROOT, "target", "bench-tests")
    os.makedirs(base, exist_ok=True)
    return tempfile.TemporaryDirectory(dir=base)


class Drivers(unittest.TestCase):
    """The variables the scenes drive the client with are the ones it reads."""

    @classmethod
    def setUpClass(cls):
        cls.reads = rust_env_reads()

    def test_presence_flags_never_set_off(self):
        # The client tests some variables for presence only: "0" leaves them ON (the trap of the
        # fresh-world flights, `RUSTCRAFT_GENLOG: "0"`).
        self.assertEqual(self.reads["RUSTCRAFT_GENLOG"], {"flag"})
        envs = [("launch", scenes.FIXED_ENV)] + [(n, sc.get("env", {})) for n, sc in scenes.SCENES.items()]
        traps = [(n, k, v) for n, env in envs for k, v in env.items()
                 if self.reads.get(k) == {"flag"} and v.strip().lower() in OFF_VALUES]
        self.assertEqual(traps, [])

    def test_every_driver_is_read(self):
        unread = sorted({(n, v) for n, sc in scenes.SCENES.items() for v in scenes.scene_drivers(sc) if v not in self.reads})
        self.assertEqual(unread, [])
        self.assertEqual(set(scenes.SCENE_VARS) - set(self.reads), set())

    def test_menu_names_exist(self):
        with open(os.path.join(scenes.ROOT, "apps", "client", "src", "menus.rs"), encoding="utf-8") as f:
            text = f.read()
        body = text[text.index("pub fn test_path"):]
        names = set(re.findall(r'"(\w+)" =>', body[:body.index("_ => return None")])) | {"title"}
        self.assertIn("pause", names)
        for n, sc in scenes.SCENES.items():
            if "menu" in sc:
                self.assertIn(sc["menu"], names, n)

    def test_exe_without_a_driver_is_skipped(self):
        with bench_dir() as t:
            exe = os.path.join(t, "old.exe")
            with open(exe, "wb") as f:
                f.write(b"\0RUSTCRAFT_CMDTEST\0RUSTCRAFT_MENUTEST\0RUSTCRAFT_MENUCLICKS\0")
            a = argparse.Namespace(exe=[exe], sound_device="", connect="")
            scenes._EXE_LACKS.clear()
            self.assertIsNone(scenes.skip_reason(a, scenes.SCENES["first-locate"]))
            self.assertIn("does not read RUSTCRAFT_DEBUGTEST", scenes.skip_reason(a, scenes.SCENES["f3-open"]))
            scenes._EXE_LACKS.clear()

    def test_missing_view_or_path_is_skipped(self):
        a = argparse.Namespace(exe=[], sound_device="", connect="")
        with bench_dir() as t:
            os.makedirs(os.path.join(t, "model"))
            os.makedirs(os.path.join(t, "model-1234"))
            saved, scenes.PERF = scenes.PERF, t
            try:
                info = {"scenes": {"plains": {}}, "sites": {}}
                self.assertIn("view trial missing", scenes.skip_reason(a, scenes.SCENES["trial-chamber"], info))
                self.assertIsNone(scenes.skip_reason(a, scenes.SCENES["first-locate"], info))
                self.assertIn("path swamp missing", scenes.skip_reason(a, scenes.SCENES["fresh-swamp-rd16"], info))
                with open(os.path.join(t, "model-1234", scenes.FRESH_PATHS_FILE), "w") as f:
                    json.dump({"swamp": {"dim": "minecraft:overworld", "pos": [1.5, 90.0, 2.5], "rot": [90.0, 25.0]}}, f)
                self.assertIsNone(scenes.skip_reason(a, scenes.SCENES["fresh-swamp-rd16"], info))
                self.assertEqual(scenes.view_of(scenes.SCENES["fresh-swamp-rd16"], info)["pos"], [1.5, 90.0, 2.5])
            finally:
                scenes.PERF = saved


class Positions(unittest.TestCase):
    INFO = {"scenes": {"nether": {"dim": "minecraft:the_nether", "pos": [1.5, 64.0, -2.5], "rot": [0.0, 5.0]},
                       "plains": {"dim": "minecraft:overworld", "pos": [10.5, 70.0, 20.5], "rot": [90.0, 15.0]}},
            "sites": {"crowd": [740, 64, 200]}}

    def test_placeholders(self):
        self.assertEqual(scenes.resolve_positions("tp @s {view:nether}", self.INFO), "tp @s 1.50 64.00 -2.50")
        self.assertEqual(scenes.resolve_positions("{site:crowd:0:1:-18}", self.INFO), "740,65,182")
        self.assertEqual(scenes.resolve_positions("{site:crowd}", self.INFO), "740,64,200")
        with self.assertRaises(SystemExit):
            scenes.resolve_positions("{view:trial}", self.INFO)

    def test_scene_placeholders_name_known_positions(self):
        known = {"view": {"plains", "forest", "village", "ocean", "cave", "nether", "crowd", "redstone", "trial", "fortress"},
                 "site": {"crowd", "redstone"}}
        for n, sc in scenes.SCENES.items():
            for text in [sc.get("cmd", ""), sc.get("keys", "")] + list(sc.get("env", {}).values()):
                for m in scenes.PLACEHOLDER.finditer(text):
                    self.assertIn(m.group(2), known[m.group(1)], n)

    def test_resolved_copy(self):
        sc = scenes.SCENES["first-nether-trip"]
        out = scenes.scene_with_positions(sc, self.INFO)
        self.assertIn("tp @s 1.50 64.00 -2.50", out["cmd"])
        self.assertIn("{view:nether}", sc["cmd"])  # the scene itself is untouched
        inv = scenes.scene_with_positions(scenes.SCENES["survival-inventory"], self.INFO)
        self.assertEqual(inv["env"]["RUSTCRAFT_INVTEST"], "740,65,182")

    def test_views(self):
        self.assertEqual(scenes.view_of(scenes.SCENES["end-fight"], self.INFO)["dim"], "minecraft:the_end")
        v = scenes.view_of(scenes.SCENES["first-bonemeal"], self.INFO)
        self.assertEqual((v["pos"], v["rot"]), ([10.5, 70.0, 20.5], [0.0, 90.0]))
        self.assertEqual(self.INFO["scenes"]["plains"]["rot"], [90.0, 15.0])

    def test_survival_player(self):
        with bench_dir() as t:
            model, game = os.path.join(t, "model"), os.path.join(t, "game")
            os.makedirs(model)
            view = {"dim": "minecraft:overworld", "pos": [0.5, 65.0, 0.5], "rot": [0.0, 0.0]}
            for survival, mode in ((True, 0), (False, 1)):
                scenes.stage(game, view, 12, model=model, survival=survival)
                p = scenes.nbt.read_dat(os.path.join(game, "saves", "perf", "players", "data", scenes.offline_uuid(scenes.NAME) + ".dat"))
                self.assertEqual(p["playerGameType"][1], mode)
                self.assertEqual(p["abilities"]["flying"][1], mode)
                self.assertEqual(p["abilities"]["mayBuild"][1], 1)


class TimedAnalysis(unittest.TestCase):
    TICK = "[perf] server: tick median 1.000 ms, p95 2.000 ms, p99 {} ms, mean 1.100 ms"

    def test_tick_reports_dated_by_the_line_before(self):
        t = self.TICK.format
        lines = [t("50.0"), "[  24.0 s] 100 fps", t("3.0"), "[  25.0 s] x", t("9.5"), "[  26.0 s] x",
                 "[  30.0 s] x", t("7.0"), "[  34.0 s] x",  # client stall inside the window
                 "[  42.0 s] x", t("80.0"), "[  48.0 s] x",  # stall across the end of the window
                 t("4.0")]
        self.assertEqual(scenes.dated_ticks(lines),
                         [(24.0, 25.0, 3.0), (25.0, 26.0, 9.5), (30.0, 34.0, 7.0), (42.0, 48.0, 80.0), (48.0, None, 4.0)])
        window = lambda s: 25 <= s <= 44  # noqa: E731
        m = scenes.timed_tick_metrics(lines, window, 25, 45)
        # The 50 ms before any date (loading) and the 80 ms of unknown time are left out.
        self.assertEqual(m["tick_max_ms"], 9.5)
        self.assertTrue(math.isnan(m["tick_p99_last_minute_ms"]))  # 45 s scene: no full minute after 25 s
        long = scenes.timed_tick_metrics(lines, window, 25, 90)
        self.assertEqual(long["tick_p99_last_minute_ms"], 43.5)  # 7.0 and 80.0, both gaps within 29..89 s
        self.assertTrue(math.isnan(scenes.timed_tick_metrics(lines, lambda s: s > 60, 25, 70)["tick_max_ms"]))

    def test_load_and_memory(self):
        ram = ("[perf] [{t:6.1f} s] ram: private 900 MB, allocator {a} MB committed (peak 700), outside the allocator 300 MB; "
               "server: {c} chunks, sections 10.0 MB, light 2.0 MB (5 layers), heightmaps 1.0 MB, block entities 0.5 MB, "
               "kept compound 3.5 MB (x 1.0), frozen entities 0 (0.0 MB); pending: generator outputs 0, write batches 0")
        stat = "[{t:6.1f} s] 100 fps, CPU 0.1 ms/frame, 12 entities drawn, block entities {b} of which 3 drawn"
        lines = [ram.format(t=20.0, a=100, c=5), ram.format(t=26.0, a=500, c=400), ram.format(t=31.0, a=600, c=450),
                 stat.format(t=26.0, b=40), stat.format(t=27.0, b=40), stat.format(t=10.0, b=99)]
        win = lambda t: 25 <= t <= 44  # noqa: E731
        mem = scenes.memory_metrics(lines, win)
        self.assertEqual((mem["allocator_mb"], mem["allocator_growth_mb"], mem["server_chunks"]), (600.0, 100.0, 450))
        self.assertAlmostEqual(mem["server_mem_mb"], 17.0)
        self.assertAlmostEqual(mem["server_mem_growth_mb"], 0.0)
        load = scenes.load_metrics(lines, win)
        self.assertEqual((load["entities_drawn"], load["block_entities"], load["block_entities_drawn"]), (12, 40, 3))

    def test_new_measures_shown(self):
        m = analyse("new.log", "images.txt")
        shown = {k for k, _, _ in scenes.KEYS}
        for k in ("tick_max_ms", "save_chunks_med", "reload_ms", "entities_drawn", "allocator_mb", "server_mem_growth_mb"):
            self.assertIn(k, m)
            self.assertIn(k, shown)
        self.assertEqual(m["save_chunks_med"], 250)  # saves of 240, 250 and 260 chunks in the stored log

    def test_load_report_flags_moving_load(self):
        rep = scenes.noise_report({"a": [{"entities_drawn": 12.0}, {"entities_drawn": 12.0}],
                                   "b": [{"entities_drawn": 12.0}, {"entities_drawn": 15.0}]})
        self.assertIn("| a | 12 |", rep)
        self.assertIn("| b | 12–15 (differs) |", rep)


class Launch(unittest.TestCase):
    def test_remove_and_fixed_variables(self):
        a = argparse.Namespace(env=["-RUSTCRAFT_GENLOG", "X_PERF_TEST=1"])
        env = scenes.launch_env(a, {"secs": 45, "env": {"RUSTCRAFT_TABTEST": "1"}}, "out", "game", "cache", ["-RUSTCRAFT_VSYNC"])
        self.assertNotIn("RUSTCRAFT_GENLOG", env)
        self.assertNotIn("RUSTCRAFT_VSYNC", env)
        self.assertEqual((env["X_PERF_TEST"], env["RUSTCRAFT_TABTEST"], env["RUSTCRAFT_HALLOWEEN"]), ("1", "1", "0"))
        env = scenes.launch_env(argparse.Namespace(env=[]), {"secs": 45}, "out", "game", "cache", [])
        self.assertEqual(env["RUSTCRAFT_GENLOG"], "1")

    def test_removal_written_as_documented(self):
        # Through the real parser: the documented spelling must reach launch_env.
        a = scenes.build_parser().parse_args(["run", "--exe", "a.exe|-RUSTCRAFT_VSYNC", "--env=-RUSTCRAFT_GENLOG"])
        self.assertEqual((a.env, a.exe), (["-RUSTCRAFT_GENLOG"], ["a.exe|-RUSTCRAFT_VSYNC"]))
        self.assertNotIn("RUSTCRAFT_GENLOG", scenes.launch_env(a, {"secs": 45}, "out", "game", "cache", []))
        self.assertIn("--env=-VAR", scenes.__doc__)

    def test_groups(self):
        for g, names in scenes.SCENE_GROUPS.items():
            self.assertEqual([n for n in names if n not in scenes.SCENES], [], g)
        self.assertEqual(len(scenes.SCENE_GROUPS["reference"]), 15)
        names = scenes.expand_scene_names("@first-use,first-locate,plains12,@first-use")
        self.assertEqual(names, scenes.SCENE_GROUPS["first-use"] + ["plains12"])
        with self.assertRaises(SystemExit):
            scenes.expand_scene_names("@nothing")

    def test_mission_scenes_exist(self):
        # Scenes asked by the series-3 plan (renamed: autosave-1 is `save`, newworld-taiga and
        # newworld-nether are `fresh-taiga-rd16` and `fresh-nether-rd16`).
        asked = ["save", "flight", "long-flight", "hoppers-1", "smelter-1", "sculk-1", "carts-1", "items-merge-1",
                 "redstone-dense", "tnt-cannon", "fresh-taiga-rd16", "fresh-nether-rd16", "fresh-swamp-rd16",
                 "fresh-badlands-rd16", "crowd-1000-lan-1", "tnt-lan-1", "lan-4-spread", "redstone-published",
                 "maps-1", "hidden-1", "flight-rotation-32", "ocean-transparency", "survival-hud", "hud-busy", "trade",
                 "tab-80", "chat-flood", "recipe-book", "cmd-give", "stats-screen", "advancements", "f3-open",
                 "team-crowd", "signs-chests", "displays-1", "structure-show-air", "reload-f3t", "multiplayer",
                 "world-list", "world-create", "packs-20", "long-flight-save", "village-night", "raid",
                 "crowd-1000-melee", "crowd-1000-archers", "pen-babies", "crowd-villagers", "trial-chamber",
                 "bee-nests-night", "nether-fortress", "end-fight", "swamp-bastion", "survival-inventory", "datapack",
                 "tnt-first-volley", "first-locate", "first-bonemeal", "first-ender-eye", "first-nether-trip"]
        self.assertEqual([n for n in asked if n not in scenes.SCENES], [])

    def test_scripted_inputs(self):
        clicks = scenes.crafting_clicks().split(",")
        self.assertEqual(len(clicks), 108)
        self.assertTrue(all(len(c.split(":")) == 3 and all(p.isdigit() for p in c.split(":")) for c in clicks))
        self.assertEqual(clicks[:3], ["37:0:0", "1:0:0", "0:0:1"])
        self.assertEqual(scenes.right_clicks(600, 2),
                         "600:+:key.mouse.right,601:-:key.mouse.right,610:+:key.mouse.right,611:-:key.mouse.right")
        self.assertEqual(scenes.SCENES["cmd-give"]["cmd"], "w40|/|tgive @s |w200")

    def test_clicks_inside_the_interface(self):
        # 1600 × 900 at scale 3: 533 × 300 interface units.
        point = re.compile(r"(?:^|:)(\d+):(\d+)$")
        for n, sc in scenes.SCENES.items():
            for step in sc.get("env", {}).get("RUSTCRAFT_MENUCLICKS", "").split(","):
                m = point.search(step)
                if m and not step.split(":")[-3:-2] == ["t"]:
                    self.assertTrue(0 <= int(m.group(1)) < 533 and 0 <= int(m.group(2)) < 300, f"{n}: {step}")

    def test_folders_of_new_scenes(self):
        self.assertEqual(scenes.parse_run_dir("tab-20-1"), ("tab-20", 1, None))
        self.assertEqual(scenes.parse_run_dir("maps-1-2-0"), ("maps-1", 2, 0))
        self.assertEqual(scenes.parse_run_dir("crowd-1000-lan-1-3"), ("crowd-1000-lan-1", 3, None))

    def test_chat_commands_of_a_script(self):
        script = "w100|/|tfunction perf:x|enter|w5|/|tgi|tab|enter|o|thello|enter|/|ttp @s |t~ ~1 ~|enter"
        self.assertEqual(scenes.chat_commands(script), ["function perf:x", "tp @s ~ ~1 ~"])

    def test_chat_commands_written_with_positions(self):
        with tempfile.TemporaryDirectory(dir=os.path.join(scenes.ROOT, "target")) as out:
            self.assertEqual(scenes.write_chat_commands(out), sum("cmd" in sc for sc in scenes.SCENES.values()))
            with open(os.path.join(out, "chat-commands", "first-nether-trip.txt"), encoding="utf-8") as f:
                self.assertEqual(f.read().splitlines()[0], "execute in minecraft:the_nether run tp @s 64.50 -60.00 8.50")
            for name in os.listdir(os.path.join(out, "chat-commands")):
                with open(os.path.join(out, "chat-commands", name), encoding="utf-8") as f:
                    self.assertIsNone(scenes.PLACEHOLDER.search(f.read()), name)

    def test_every_run_folder_reads_back_to_its_scene(self):
        # A scene named `<other scene>-<pass number>` makes `<other>-<p>-<e>` ambiguous: the
        # A/B folder of one scene would be reread as a single-exe pass of the other.
        for n in scenes.SCENES:
            for p in range(1, 10):
                self.assertEqual(scenes.parse_run_dir(f"{n}-{p}"), (n, p, None), n)
                for e in range(10):
                    self.assertEqual(scenes.parse_run_dir(f"{n}-{p}-{e}"), (n, p, e), n)


class Micro(unittest.TestCase):
    def test_bench_lines(self):
        with bench_dir() as t:
            paths = []
            for k, v in enumerate(("120.5", "130.5")):
                paths.append(os.path.join(t, f"pass{k}.txt"))
                with open(paths[-1], "w", encoding="utf-8") as f:
                    f.write(f"running 1 test\n[bench] datapack_tick crowd us_per_tick {v}\n"
                            "[bench] datapack_tick crowd allocs_per_tick 3\nok\n")
            res = scenes.micro_results(paths)
        self.assertEqual(res[("datapack_tick", "crowd", "us_per_tick")], [120.5, 130.5])
        table = scenes.micro_table(res)
        self.assertIn("| datapack_tick | crowd | us_per_tick | 125.5 (120.5–130.5) | 8.0 % | 2 |", table)


class OldData(unittest.TestCase):
    """Data written before the English renaming stays readable."""

    def test_old_scenes_json(self):
        old = {"seed": "1", "found": {"plaine": [1, None, 2], "foret": [3, None, 4], "village": [5, 70, 6],
                                      "grotte": [7, 8, 9]},
               "sites": {"foule": [10, 64, 11], "redstone": [12, 64, 13]},
               "scenes": {"plaine": {"dim": "d", "pos": [0, 0, 0], "rot": [0, 0]}, "foule": {"dim": "d"}, "ocean": {}}}
        base = os.path.join(scenes.ROOT, "target", "bench-tests")
        os.makedirs(base, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=base) as t:
            with open(os.path.join(t, "scenes.json"), "w", encoding="utf-8") as f:
                json.dump(old, f)
            saved, scenes.PERF = scenes.PERF, t
            try:
                info = scenes.load_info()
            finally:
                scenes.PERF = saved
        self.assertEqual(set(info["found"]), {"plains", "forest", "village", "cave"})
        self.assertEqual(set(info["sites"]), {"crowd", "redstone"})
        self.assertEqual(set(info["scenes"]), {"plains", "crowd", "ocean"})
        self.assertEqual(info["sites"]["crowd"], [10, 64, 11])
        # The pack functions take the old sites once mapped.
        scenes.pack_functions({k: tuple(v) for k, v in info["sites"].items()})

    def test_old_results(self):
        old = {"exe": ["a.exe"], "plaine12#0": [{"images_s": 100.0, "froid": 1.0, "dem:data": 12.0, "c:x": 3.0}],
               "particules-1000": [{"cpu_ms": 1.0, "invites_entres": 2}]}
        up = scenes.upgrade_results(old)
        self.assertEqual(up["exe"], ["a.exe"])
        self.assertEqual(up["plains12#0"], [{"frames_s": 100.0, "cold": 1.0, "startup:data": 12.0, "c:x": 3.0}])
        self.assertEqual(up["particles-1000"], [{"cpu_ms": 1.0, "guests_joined": 2}])
        # Every old scene name maps to a current scene, every metric to a metric the analysis writes.
        self.assertTrue(set(scenes.OLD_SCENES.values()) <= set(scenes.SCENES))
        written = set(analyse("plains12-cold.log")) | set(analyse("new.log", "images.txt"))
        written |= {"guests_joined", "guest_join_ms"}
        self.assertTrue(set(scenes.OLD_METRICS.values()) <= written, set(scenes.OLD_METRICS.values()) - written)


class Window(unittest.TestCase):
    def test_profile_window(self):
        # samply times counted from the start of the recording: the window starts at the process start.
        th = {"processStartupTime": 2000.0,
              "samples": {"length": 4, "time": [2500.0, 26500.0, 28000.0, 50100.0], "threadCPUDelta": [5, 7, 9, 11]}}
        self.assertEqual(window.weights(th, (25000.0, 44000.0)), [0, 0, 9, 0])
        self.assertEqual(window.weights(th, None), [5, 7, 9, 11])
        th2 = {"processStartupTime": 0.0, "samples": {"length": 2, "timeDeltas": [1000.0, 25500.0], "threadCPUDelta": [3, 4]}}
        self.assertEqual(window.weights(th2, (25000.0, 44000.0)), [0, 4])
        self.assertEqual(window.strip_flag(["x", "--whole-profile", "y"]), (["x", "y"], True))


if __name__ == "__main__":
    unittest.main()
