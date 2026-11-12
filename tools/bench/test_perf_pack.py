"""Tests of the `perf` data pack of the bench (`python -I tools/bench/test_perf_pack.py`): quantities
of each staged scene, same text on every call, hidden cows really hidden from the camera, signs
near the camera, distinct maps. The game-side check (every function compiles with our command
parser) is `crates/rc-sim/src/tests_perf_pack.rs`."""
import math
import os
import re
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import nbt  # noqa: E402
import perf_pack  # noqa: E402
import scenes  # noqa: E402

SITES = {"crowd": (0, 64, 0), "redstone": (0, 64, 0)}
F = perf_pack.pack_functions(SITES)


def count(name, word):
    return F[name].count(word)


FILL = re.compile(r"^fill (-?\d+) (-?\d+) (-?\d+) (-?\d+) (-?\d+) (-?\d+) minecraft:(\S+)( hollow)?$", re.M)
SUMMON_COW = re.compile(r"^summon minecraft:cow (-?[\d.]+) (-?[\d.]+) (-?[\d.]+)", re.M)


def solid_voxels(text):
    """Blocks set by the `fill … stone [hollow]` lines of a function."""
    out = set()
    for m in FILL.finditer(text):
        x0, y0, z0, x1, y1, z1 = map(int, m.groups()[:6])
        for x in range(x0, x1 + 1):
            for y in range(y0, y1 + 1):
                for z in range(z0, z1 + 1):
                    shell = x in (x0, x1) or y in (y0, y1) or z in (z0, z1)
                    if m.group(7) == "stone" and (shell or not m.group(8)):
                        out.add((x, y, z))
    return out


def hidden(eye, target, solid, step=0.05):
    """True if the straight line from `eye` to `target` crosses a solid block."""
    n = int(math.dist(eye, target) / step)
    for i in range(1, n):
        p = [e + (t - e) * i / n for e, t in zip(eye, target)]
        if tuple(math.floor(c) for c in p) in solid:
            return True
    return False


class Quantities(unittest.TestCase):
    def test_server_scenes(self):
        self.assertEqual(count("hoppers", "minecraft:hopper["), 200)
        self.assertEqual(count("hoppers_feed", "summon minecraft:item "), 500)
        self.assertIn("schedule function perf:hoppers_feed 200t", F["hoppers_feed"])
        self.assertEqual(count("furnaces", "minecraft:furnace["), 500)
        self.assertEqual((count("sculk", "sculk_sensor"), count("sculk", "summon minecraft:cow")), (100, 50))
        self.assertEqual((count("minecarts", "summon minecraft:minecart "), count("minecarts", "hopper_minecart")), (2000, 100))
        self.assertEqual(count("items_merge", "summon minecraft:item "), 2000)
        self.assertNotIn("PickupDelay:32767", F["items_merge"])  # mergeable
        self.assertEqual(count("redstone_dense", "observer["), 30)
        self.assertEqual(count("tnt_cannon_fire", "summon minecraft:tnt"), 72)
        self.assertEqual(count("tnt_once", "summon minecraft:tnt"), 64)
        self.assertNotIn("schedule", F["tnt_once"])

    def test_redstone_rows_stand_on_stone(self):
        """No grass under the clocks (it turns into dirt at a random tick and shifts a row's
        phase): each of the 16 rows' grass becomes stone before any clock is placed."""
        text = F["redstone"].splitlines()
        floor = [line for line in text if line.endswith("minecraft:stone replace minecraft:grass_block")]
        self.assertEqual(len(floor), 16)
        first_clock = next(i for i, line in enumerate(text) if line.startswith("setblock"))
        self.assertTrue(all(text.index(line) < first_clock for line in floor))
        self.assertEqual(count("redstone", "minecraft:observer["), 256)

    def test_mob_scenes(self):
        self.assertEqual((count("melee", "minecraft:zombie"), count("melee", "minecraft:iron_golem")), (100, 20))
        self.assertEqual(count("melee", "resistance"), 120)
        self.assertEqual(count("archers", "minecraft:skeleton"), 30)
        self.assertEqual((count("pen_babies", "summon"), count("pen_babies", "Age:-24000")), (1000, 333))
        self.assertEqual((count("villagers", "minecraft:villager"), count("villagers", "summon minecraft:cow")), (100, 100))
        self.assertEqual((count("villagers", "half=lower"), sum(count("villagers", f"minecraft:{j}") for j in perf_pack.JOB_SITES)), (20, 20))
        self.assertEqual((count("bee_nests", "bee_nest["), count("bee_nests", "entity_data")), (30, 90))
        self.assertEqual(count("end_fight", "enderman"), 20)
        self.assertEqual([count("swamp_bastion", k) for k in ("frog", "axolotl", "piglin", "hoglin")], [100, 20, 20, 4])
        self.assertEqual(count("village_night", "minecraft:zombie"), 10)
        rings = [math.hypot(dx, dz) for dx, dz in perf_pack.ring(10, 40, 2)]
        self.assertTrue(all(39.5 <= r <= 58.5 for r in rings), rings)
        self.assertEqual(count("datapack", "repeating_command_block"), 256)
        self.assertIn("$scoreboard players set #macro perf_dp $(value)", F["datapack_macro"])

    def test_interface_scenes(self):
        stacks = [int(l.rsplit(" ", 1)[1]) for l in F["survival_hud"].splitlines() if " with minecraft:" in l and "armor" not in l]
        self.assertEqual((len(stacks), min(stacks), max(stacks)), (36, 2, 64))
        self.assertEqual(count("enchanted_36", "item replace"), 36)
        self.assertEqual(count("hud_busy", "bossbar add"), 3)
        self.assertEqual(count("hud_busy", "scoreboard players set Line"), 15)
        self.assertEqual(count("tab_list", "team join"), 79)
        self.assertEqual(count("trade", "enchanted_book"), 8)
        self.assertEqual(count("displays", "summon"), 50)
        self.assertEqual(count("displays", "perf_interp"), 15)
        self.assertIn("schedule function perf:displays_swap_a", F["displays_swap_b"])
        self.assertIn("sizeX:48,sizeY:48,sizeZ:48,showair:1b", F["structure_air"])
        pickups = re.findall(r'id:"minecraft:(\w+)",count:64', F["survival_pickups"])
        self.assertEqual(len(set(pickups)), 64)
        self.assertEqual(count("survival_inventory", "filled_map[map_id="), 13)

    def test_signs_glow_within_16_blocks(self):
        self.assertEqual((count("signs_chests", "minecraft:chest"), count("signs_chests", "oak_sign")), (400, 200))
        self.assertEqual(count("signs_chests", "has_glowing_text:1b"), 50)
        x, y, z = SITES["crowd"]
        chests = perf_pack.grid(400, 20, 2, x - 19, z - 19)
        fiftieth = perf_pack.nearest_to_crowd_camera(x, y, z, chests)[49]
        cam = (x + 0.5, y + 9 + 1.62, z - 21.5)
        self.assertLess(math.dist(cam, (fiftieth[0] + 0.5, y + 2.5, fiftieth[1] + 0.5)), 16)

    def test_map_wall(self):
        ids = [int(i) for i in re.findall(r'"minecraft:map_id":(\d+)', F["maps_wall"])]
        self.assertEqual(sorted(ids), list(range(900)))
        self.assertEqual(perf_pack.MAP_COUNT, 900)

    def test_same_text_on_every_call(self):
        self.assertEqual(perf_pack.pack_functions(SITES), F)
        self.assertEqual(set(F), set(perf_pack.FUNCTIONS) | {"tick"})

    def test_called_functions_exist(self):
        called = set(re.findall(r"function perf:(\w+)", "".join(F.values())))
        self.assertEqual(called - set(F), set())


class Hidden(unittest.TestCase):
    def test_cows_hidden_from_the_camera(self):
        sc = scenes.SCENES["hidden-1"]
        x, y, z = SITES["crowd"]
        eye = (x + sc["d"][0], y + sc["d"][1] + 1.62, z + sc["d"][2])
        solid = solid_voxels(F["hidden_cows"])
        cows = [tuple(map(float, m.groups())) for m in SUMMON_COW.finditer(F["hidden_cows"])]
        self.assertEqual(len(cows), 200)
        seen = [c for c in cows if not hidden(eye, (c[0], c[1] + 0.7, c[2]), solid)]
        self.assertEqual(seen, [])
        # The check sees what is in the open: a point in front of the camera.
        self.assertFalse(hidden(eye, (x + 0.5, y + 1.7, z - 20.0), solid))


class Maps(unittest.TestCase):
    def test_colours_distinct_and_valid(self):
        sample = list(range(0, 62)) + [243, 244, 245, 487, 488, 899]
        colours = {n: perf_pack.map_colors(n) for n in sample}
        self.assertEqual(len(set(colours.values())), len(sample))
        for n, c in colours.items():
            self.assertEqual(len(c), 16384)
            self.assertTrue(all(1 <= b // 4 <= 61 for b in c[::97]))
            self.assertEqual(sum((c[k] // 4 == perf_pack.MAP_BLACK) << k for k in range(10)), n)

    def test_map_files(self):
        base = os.path.join(scenes.ROOT, "target", "bench-tests")
        os.makedirs(base, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=base) as t:
            perf_pack.write_maps(t, 3)
            d = os.path.join(t, "data", "minecraft", "maps")
            self.assertEqual(sorted(os.listdir(d)), ["0.dat", "1.dat", "2.dat", "last_id.dat"])
            m = nbt.read_dat(os.path.join(d, "2.dat"))
            self.assertEqual(m["data"]["colors"][1], perf_pack.map_colors(2))
            self.assertEqual((m["data"]["locked"][1], m["data"]["scale"][1]), (1, 0))
            self.assertEqual(nbt.read_dat(os.path.join(d, "last_id.dat"))["data"]["map"][1], 2)


if __name__ == "__main__":
    unittest.main()
