"""Bench tests (`python -I tools/bench/test_scenes.py`): log analysis (real and new formats),
exclusion of cold launches, camera-centred pre-generation, scene consistency, profile
window, compatibility with data written before the English renaming. Expected values are
stored here; explicit failure if the data is missing."""
import json
import math
import os
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
        views = {"plains", "forest", "village", "ocean", "cave", "nether", "crowd", "redstone"}
        steps = {"/", "o", "tab", "stab", "up", "down", "left", "right", "esc", "enter", "bs"}
        for n, sc in scenes.SCENES.items():
            self.assertTrue(sc.get("at") in views or sc.get("site") in ("crowd", "redstone") or "pos" in sc, n)
            self.assertLess(sc["from"], sc["secs"] - 5, n)
            for s in sc.get("cmd", "").split("|") if sc.get("cmd") else []:
                self.assertTrue(s in steps or s[:1] in "wt", f"{n}: step {s}")
                if s.startswith("tfunction perf:"):
                    self.assertIn(s[len("tfunction perf:"):], funcs, n)
        self.assertEqual(scenes.COLD_SCENES, {"cold-resources", "first-launch", "fresh-install"})

    def test_function_quantities(self):
        f = scenes.pack_functions({"crowd": (0, 64, 0), "redstone": (0, 64, 0)})
        self.assertTrue(all(isinstance(t, str) for t in f.values()))
        self.assertEqual([l.split("tag=")[1][:9] for l in f["tick"].splitlines()], ["perf_fly]", "perf_glid", "perf_turn", "perf_walk"])
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
