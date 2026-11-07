"""The `perf` data pack of the scene bench: one function per staged scene, written into the
model world by `scenes.py prepare` / `scenes.py pack`, plus the map data the map scenes show.

Every function is plain text computed from the two flat pads of the model (`crowd`, `redstone`:
grass at `y`, 48 x 48 blocks around the site, 24 blocks of air above): same text on every run,
fixed positions, no randomness of ours. One invalid line makes the game refuse the WHOLE
function, so `crates/rc-sim/src/tests_perf_pack.rs` compiles the pack with our command parser
(`python -I tools/bench/scenes.py pack-files <dir>` first).
"""
import json
import math
import os

import nbt

# --- Shared pieces --------------------------------------------------------------------------


def grid(n, cols, step, x0, z0):
    """`n` positions (x, z) in a grid of `cols` columns at step `step` from (x0, z0)."""
    return [(x0 + (i % cols) * step, z0 + (i // cols) * step) for i in range(n)]


KEEP_MOB = "PersistenceRequired:1b,NoAI:1b"
PERSIST = "PersistenceRequired:1b"
# Resistance V cancels all damage (fights and arrows go on forever, counts stay fixed).
RESIST = 'active_effects:[{id:"minecraft:resistance",amplifier:4b,duration:-1,show_particles:0b}]'
# Helmet that never breaks: an undead keeps it and never burns in daylight.
SUN_HELMET = 'equipment:{head:{id:"minecraft:leather_helmet",count:1,components:{"minecraft:unbreakable":{}}}}'
FARM_ANIMALS = ["cow", "sheep", "pig", "chicken"]
# 64 distinct common items (pickups, hotbars, hoppers).
ITEMS_64 = [
    "stone", "cobblestone", "dirt", "oak_log", "birch_log", "spruce_log", "oak_planks", "glass", "sand", "gravel",
    "iron_ingot", "gold_ingot", "diamond", "emerald", "redstone", "lapis_lazuli", "coal", "charcoal", "quartz",
    "copper_ingot", "apple", "bread", "carrot", "potato", "wheat", "wheat_seeds", "beetroot", "melon_slice",
    "pumpkin", "sugar_cane", "bone", "string", "feather", "gunpowder", "leather", "flint", "clay_ball", "brick",
    "paper", "book", "stick", "torch", "ladder", "chest", "furnace", "crafting_table", "white_wool", "red_wool",
    "blue_wool", "yellow_wool", "green_wool", "black_wool", "oak_sapling", "birch_sapling", "dandelion", "poppy",
    "cactus", "kelp", "slime_ball", "ender_pearl", "blaze_rod", "glowstone_dust", "snowball", "egg",
]
ENCHANTED = [
    "diamond_sword[enchantments={sharpness:5,unbreaking:3}]", "diamond_pickaxe[enchantments={efficiency:5,fortune:3}]",
    "diamond_axe[enchantments={efficiency:4}]", "bow[enchantments={power:5,infinity:1}]",
    "diamond_helmet[enchantments={protection:4}]", "diamond_chestplate[enchantments={protection:4}]",
    "diamond_leggings[enchantments={protection:4}]", "diamond_boots[enchantments={feather_falling:4}]",
    "enchanted_book[stored_enchantments={mending:1}]", "trident[enchantments={loyalty:3}]",
    "iron_shovel[enchantments={silk_touch:1}]", "fishing_rod[enchantments={lure:3}]",
]


def lines(*parts):
    """Function text: one command per line, final newline."""
    return "\n".join(parts) + "\n"


def mob_grid(kind, n, cols, step, x0, z0, y, nbt_text, start=0):
    """`summon` lines of `n` mobs of `kind` (a name or a list cycled by index)."""
    out = []
    tail = f" {{{nbt_text}}}" if nbt_text else ""
    for i, (gx, gz) in enumerate(grid(n, cols, step, x0, z0), start):
        k = kind[i % len(kind)] if isinstance(kind, list) else kind
        out.append(f"summon minecraft:{k} {gx:.2f} {y} {gz:.2f}{tail}")
    return out


def fence_pen(x, y, z, half=23):
    return f"fill {x - half} {y + 1} {z - half} {x + half} {y + 1} {z + half} minecraft:oak_fence hollow"


# --- Scenes of series 2 (unchanged) ---------------------------------------------------------


def mobs_function(x, y, z):
    """100 mobs (25 of each kind) in a 10 × 10 grid in a 24 × 24 pen."""
    out = [f"fill {x - 12} {y + 1} {z - 12} {x + 12} {y + 1} {z + 12} minecraft:oak_fence hollow"]
    for i in range(100):
        gx, gz = i % 10, i // 10
        out.append(f"summon minecraft:{FARM_ANIMALS[i % 4]} {x - 9 + gx * 2}.5 {y + 1} {z - 9 + gz * 2}.5 {{PersistenceRequired:1b}}")
    return lines(*out)


def redstone_function(x, y, z):
    """128 clocks of observers facing each other: each one lights a lamp and pushes a block."""
    out = []
    for row in range(16):
        for col in range(8):
            bx, bz = x - 20 + col * 6, z - 16 + row * 2
            out += [
                f"setblock {bx - 1} {y + 1} {bz} minecraft:redstone_lamp",
                f"setblock {bx + 2} {y + 1} {bz} minecraft:sticky_piston[facing=east]",
                f"setblock {bx + 3} {y + 1} {bz} minecraft:slime_block",
                f"setblock {bx} {y + 1} {bz} minecraft:observer[facing=east]",
                f"setblock {bx + 1} {y + 1} {bz} minecraft:observer[facing=west]",
            ]
    return lines(*out)


def mobs1000_function(x, y, z):
    """1,000 mobs (4 kinds) in a 32 × 32 grid at a step of 1.4 in a 46 × 46 pen."""
    out = [fence_pen(x, y, z)]
    for i, (gx, gz) in enumerate(grid(1000, 32, 1.4, x - 21.7, z - 21.7)):
        out.append(f"summon minecraft:{FARM_ANIMALS[i % 4]} {gx:.2f} {y + 1} {gz:.2f} {{PersistenceRequired:1b}}")
    return lines(*out)


def armor(head="diamond_helmet", hand="iron_sword"):
    pieces = {"head": head, "chest": "iron_chestplate", "legs": "golden_leggings", "feet": "chainmail_boots", "mainhand": hand}
    return "equipment:{" + ",".join(f'{k}:{{id:"minecraft:{v}",count:1}}' for k, v in pieces.items()) + "}"


def humanoids_function(x, y, z):
    """160 equipped humanoids (full armour and weapon): zombies, skeletons, armor stands, mannequins."""
    kinds = [("zombie", "iron_sword"), ("skeleton", "bow"), ("armor_stand", "iron_sword"), ("mannequin", "iron_sword")]
    out = []
    for i, (gx, gz) in enumerate(grid(160, 16, 2.5, x - 19.5, z - 12.5)):
        kind, hand = kinds[i % 4]
        out.append(f"summon minecraft:{kind} {gx} {y + 1} {gz} {{{KEEP_MOB},Rotation:[180.0f,0.0f],{armor(hand=hand)}}}")
    return lines(*out)


def keyframes_function(x, y, z):
    """≈ 200 mobs animated by keyframes: camels, breezes, wardens (without AI, in place)."""
    kinds = ["camel", "breeze", "warden"]
    return "".join(f"summon minecraft:{kinds[i % 3]} {gx} {y + 1} {gz} {{{KEEP_MOB}}}\n"
                   for i, (gx, gz) in enumerate(grid(201, 15, 3, x - 20.5, z - 20.5)))


def items_function(x, y, z):
    """500 dropped items (never picked up nor despawned), 100 item frames placed on the ground, 100 filled shelves."""
    items = ["diamond", "apple", "iron_sword", "oak_log", "redstone", "compass", "golden_apple", "stone"]
    out = []
    for i, (gx, gz) in enumerate(grid(500, 25, 1.5, x - 18.0, z - 20.0)):
        out.append(f'summon minecraft:item {gx} {y + 1} {gz} {{Item:{{id:"minecraft:{items[i % 8]}",count:1}},PickupDelay:32767,Age:-32768}}')
    for i, (gx, gz) in enumerate(grid(100, 25, 1, x - 12, z + 12)):
        out.append(f'summon minecraft:item_frame {gx} {y + 1} {gz} {{Facing:1b,Fixed:1b,Item:{{id:"minecraft:{items[i % 8]}",count:1}}}}')
    for i, (gx, gz) in enumerate(grid(100, 25, 1, x - 12, z + 16)):
        slots = ",".join(f'{{Slot:{s}b,id:"minecraft:{items[(i + s) % 8]}",count:1}}' for s in range(3))
        out.append(f"setblock {gx} {y + 1} {gz} minecraft:oak_shelf[facing=north]{{Items:[{slots}]}}")
    return lines(*out)


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
    out = []
    for i, (gx, gz) in enumerate(grid(50, 10, 2, x - 9, z - 6)):
        second = '"§kscramble§r"' if i % 2 == 0 else f'"sign {i}"'
        out.append(f'setblock {gx} {y + 1} {gz} minecraft:oak_sign[rotation=8]'
                   f'{{front_text:{{has_glowing_text:1b,color:"yellow",messages:["Rustcraft",{second},"line 3","line 4"]}}}}')
    for i, (gx, gz) in enumerate(grid(20, 10, 2, x - 9, z + 6)):
        mode = "fixed" if i < 10 else "center"
        out.append(f'summon minecraft:text_display {gx} {y + 3} {gz} {{billboard:"{mode}",Rotation:[180.0f,0.0f],text:"Display {i}"}}')
    return lines(*out)


def block_entities_function(x, y, z):
    """Block entities: 2,016 chests (42 × 48), 500 signs on top, 200 spawners, 50 beacons
    on a layer of iron (the pad is 48 × 48)."""
    return lines(
        f"fill {x - 21} {y + 1} {z - 24} {x + 20} {y + 1} {z + 23} minecraft:chest[facing=north]",
        f"fill {x - 21} {y + 2} {z - 24} {x - 2} {y + 2} {z} minecraft:oak_sign[rotation=8]",
        f"fill {x} {y + 2} {z - 24} {x + 19} {y + 2} {z - 15} minecraft:spawner",
        f"fill {x} {y + 2} {z + 19} {x + 9} {y + 2} {z + 23} minecraft:iron_block",
        f"fill {x} {y + 3} {z + 19} {x + 9} {y + 3} {z + 23} minecraft:beacon",
    )


def primed_tnt(x, y, z):
    """64 primed TNT (4 s fuse) in an 8 × 8 grid at a step of 3."""
    return [f"summon minecraft:tnt {gx} {y + 1} {gz} {{fuse:80s}}" for gx, gz in grid(64, 8, 3, x - 10.5, z - 10.5)]


def tnt_function(x, y, z):
    """64 primed TNT, primed again every 5 s."""
    return lines(*primed_tnt(x, y, z), "schedule function perf:tnt 100t")


def tnt_once_function(x, y, z):
    """A single volley of the 64 TNT, on intact ground (cost of the first volley)."""
    return lines(*primed_tnt(x, y, z))


def tnt_heap_function(x, y, z):
    """A heap of 12 × 12 × 12 TNT blocks (1,728) set off from the top: the chain reaction throws
    primed TNT and dropped blocks everywhere, the case that separates two clients."""
    return lines(f"fill {x - 6} {y + 1} {z - 6} {x + 5} {y + 12} {z + 5} minecraft:tnt",
                 f"setblock {x} {y + 13} {z} minecraft:redstone_block")


def fireworks_function(x, y, z):
    """16 firework rockets (large ball, trail, twinkle) every second."""
    rocket = ('{LifeTime:20,FireworksItem:{id:"minecraft:firework_rocket",count:1,components:{"minecraft:fireworks":'
              '{flight_duration:1b,explosions:[{shape:"large_ball",colors:[I;16711680,65280,255],has_trail:1b,has_twinkle:1b}]}}}}')
    out = [f"summon minecraft:firework_rocket {gx} {y + 2} {gz} {rocket}" for gx, gz in grid(16, 4, 6, x - 9, z - 9)]
    return lines(*out, "schedule function perf:fireworks 20t")


# --- Server scenes (blocks and entities that tick) ------------------------------------------


def hoppers_function(x, y, z):
    """200 hoppers: 40 rings of 4 passing their items round forever, 40 collectors under the
    500 items `hoppers_feed` drops every 10 s, each pouring into a chest."""
    out = []
    ring = [(0, 0, "east"), (1, 0, "south"), (1, 1, "west"), (0, 1, "north")]
    for i, (bx, bz) in enumerate(grid(40, 10, 4, x - 20, z - 20)):
        for k, (dx, dz, facing) in enumerate(ring):
            item = ITEMS_64[(i * 4 + k) % 64]
            out.append(f'setblock {bx + dx} {y + 1} {bz + dz} minecraft:hopper[facing={facing}]'
                       f'{{Items:[{{Slot:0b,id:"minecraft:{item}",count:16}}]}}')
    for bx, bz in collector_spots(x, z):
        out += [f"setblock {bx} {y + 1} {bz} minecraft:chest[facing=north]",
                f"setblock {bx} {y + 2} {bz} minecraft:hopper[facing=down]"]
    return lines(*out, "function perf:hoppers_feed")


def collector_spots(x, z):
    return grid(40, 10, 4, x - 20, z + 2)


def hoppers_feed_function(x, y, z):
    out = []
    for i in range(500):
        bx, bz = collector_spots(x, z)[i % 40]
        out.append(f'summon minecraft:item {bx + 0.5} {y + 3.2 + (i // 40) * 0.25:.2f} {bz + 0.5} '
                   f'{{Item:{{id:"minecraft:{ITEMS_64[i % 8]}",count:1}},Motion:[0.0d,0.0d,0.0d]}}')
    return lines(*out, "schedule function perf:hoppers_feed 200t")


def furnaces_function(x, y, z):
    """500 lit furnaces (25 × 20, side by side), 64 raw iron and 64 coal each: ≈ 10 min of smelting."""
    items = '{Items:[{Slot:0b,id:"minecraft:raw_iron",count:64},{Slot:1b,id:"minecraft:coal",count:64}]}'
    return lines(*(f"setblock {gx} {y + 1} {gz} minecraft:furnace[facing=north]{items}"
                   for gx, gz in grid(500, 25, 1, x - 12, z - 10)))


def sculk_function(x, y, z):
    """100 sculk sensors (10 × 10, step 4) among 50 cows that wander (AI on) inside a pen."""
    out = [fence_pen(x, y, z)]
    out += [f"setblock {gx} {y + 1} {gz} minecraft:sculk_sensor" for gx, gz in grid(100, 10, 4, x - 18, z - 18)]
    out += mob_grid("cow", 50, 10, 4, x - 16, z - 16, y + 1, PERSIST)
    return lines(*out)


def minecarts_function(x, y, z):
    """1,000 minecarts spread out, 1,000 stacked in a 1 × 1 shaft (they push each other
    without end) and 100 hopper minecarts."""
    out = mob_grid("minecart", 1000, 40, 1.15, x - 23, z - 23, y + 1, "")
    sx, sz = x + 12, z + 16
    out += [f"fill {sx - 1} {y + 1} {sz - 1} {sx + 1} {y + 3} {sz + 1} minecraft:stone",
            f"fill {sx} {y + 1} {sz} {sx} {y + 3} {sz} minecraft:air"]
    out += [f"summon minecraft:minecart {sx + 0.5} {y + 1} {sz + 0.5}"] * 1000
    out += mob_grid("hopper_minecart", 100, 20, 1.2, x - 22, z + 8, y + 1, "")
    return lines(*out)


def items_merge_function(x, y, z):
    """2,000 items of 4 kinds falling in layers (they merge as they land), dropped again every 5 s."""
    kinds = ["cobblestone", "dirt", "oak_log", "wheat_seeds"]
    out = ["kill @e[type=minecraft:item,tag=perf_merge]"]
    for i, (gx, gz) in enumerate(grid(2000, 50, 0.4, x - 10, z - 8)):
        out.append(f'summon minecraft:item {gx:.2f} {y + 6 + i % 5} {gz:.2f} '
                   f'{{Item:{{id:"minecraft:{kinds[i % 4]}",count:1}},Tags:["perf_merge"],Motion:[0.0d,0.0d,0.0d]}}')
    return lines(*out, "schedule function perf:items_merge 100t")


def redstone_dense_function(x, y, z):
    """Wire-heavy circuit: 15 rows of 43 dust over lamps, a repeater every 15, a comparator and
    a lamp at the end, each row driven by an observer clock placed last."""
    out = []
    for r in range(15):
        bz = z - 21 + r * 3
        out += [f"fill {x - 21} {y + 1} {bz} {x + 21} {y + 1} {bz} minecraft:stone"]
        out += [f"setblock {bx} {y + 1} {bz} minecraft:redstone_lamp" for bx in range(x - 21, x + 22, 2)]
        out += [f"fill {x - 21} {y + 2} {bz} {x + 21} {y + 2} {bz} minecraft:redstone_wire[east=side,west=side]",
                f"setblock {x - 7} {y + 2} {bz} minecraft:repeater[facing=west]",
                f"setblock {x + 8} {y + 2} {bz} minecraft:repeater[facing=west]",
                f"setblock {x + 22} {y + 2} {bz} minecraft:comparator[facing=west]",
                f"setblock {x + 23} {y + 2} {bz} minecraft:redstone_lamp",
                f"setblock {x - 24} {y + 2} {bz} minecraft:observer[facing=east]",
                f"setblock {x - 23} {y + 2} {bz} minecraft:observer[facing=west]",
                f"setblock {x - 22} {y + 2} {bz} minecraft:redstone_wire[east=side,west=side]"]
    return lines(*out)


def tnt_cannon_function(x, y, z):
    """16 observer clocks pushing slime blocks that carry items, and 8 water cannons (charge of
    8 TNT in a water block, projectile TNT above) fired every 3 s by `tnt_cannon_fire`."""
    out = []
    for i, (bx, bz) in enumerate(grid(16, 4, 10, x - 20, z - 20)):
        out += [f"setblock {bx + 2} {y + 1} {bz} minecraft:sticky_piston[facing=east]",
                f"setblock {bx + 3} {y + 1} {bz} minecraft:slime_block",
                f'summon minecraft:item {bx + 3.5} {y + 2.1} {bz + 0.5} '
                f'{{Item:{{id:"minecraft:{ITEMS_64[i]}",count:1}},PickupDelay:32767,Age:-32768}}',
                f"setblock {bx} {y + 1} {bz} minecraft:observer[facing=east]",
                f"setblock {bx + 1} {y + 1} {bz} minecraft:observer[facing=west]"]
    for bx, bz in cannon_spots(x, z):
        out += [f"fill {bx - 1} {y + 1} {bz - 1} {bx + 1} {y + 2} {bz + 1} minecraft:stone",
                f"setblock {bx} {y + 2} {bz} minecraft:water"]
    return lines(*out, "function perf:tnt_cannon_fire")


def cannon_spots(x, z):
    return grid(8, 8, 5, x - 18, z + 12)


def tnt_cannon_fire_function(x, y, z):
    out = []
    for bx, bz in cannon_spots(x, z):
        out += [f"summon minecraft:tnt {bx + 0.5} {y + 2} {bz + 0.5} {{fuse:20s}}"] * 8
        out.append(f"summon minecraft:tnt {bx + 0.5} {y + 3} {bz + 0.5} {{fuse:40s}}")
    return lines(*out, "schedule function perf:tnt_cannon_fire 60t")


# --- Mobs and villages (run as the player, positions relative to it) --------------------------


def ring(n, r0, dr):
    """`n` offsets (dx, dz) on a ring, radius r0 + k·dr, whole blocks."""
    return [(round((r0 + k * dr) * math.cos(k * 2 * math.pi / n)), round((r0 + k * dr) * math.sin(k * 2 * math.pi / n)))
            for k in range(n)]


def village_night_function(_x, _y, _z):
    """10 zombies at 40 to 58 blocks around the player, on the ground (night set by the scene)."""
    return lines(*(f"execute at @s positioned ~{dx} ~ ~{dz} positioned over motion_blocking_no_leaves "
                   f"run summon minecraft:zombie ~ ~ ~ {{{PERSIST}}}" for dx, dz in ring(10, 40, 2)))


def raid_function(_x, _y, _z):
    """Hard difficulty, player moved to the village centre (20 blocks south of the camera, 3 above
    the ground), Bad Omen V: the raid omen starts the raid 30 s later."""
    return lines("difficulty hard",
                 "execute at @s positioned ~ ~ ~20 positioned over motion_blocking_no_leaves run tp @s ~ ~3 ~",
                 "effect give @s minecraft:bad_omen 6000 4")


def melee_function(x, y, z):
    """crowd-1000 + 100 zombies (sun helmet) against 20 iron golems, all with Resistance V."""
    out = ["function perf:mobs1000"]
    out += mob_grid("zombie", 100, 10, 4, x - 18.5, z - 18.5, y + 1, f"{PERSIST},{RESIST},{SUN_HELMET}")
    out += mob_grid("iron_golem", 20, 5, 8, x - 16, z - 12, y + 1, f"{PERSIST},{RESIST}")
    return lines(*out)


def archers_function(x, y, z):
    """crowd-1000 + 30 skeletons under a stone roof 8 to 12 blocks from the player, who is in
    survival with Resistance V (arrows shot, stuck, picked up again)."""
    out = ["function perf:mobs1000", "effect give @s minecraft:resistance infinite 4 true",
           "effect give @s minecraft:saturation infinite 0 true",
           f"fill {x - 16} {y + 4} {z - 13} {x + 15} {y + 4} {z - 9} minecraft:stone"]
    out += mob_grid("skeleton", 30, 15, 2, x - 14.5, z - 12.5, y + 1,
                    f'{PERSIST},equipment:{{mainhand:{{id:"minecraft:bow",count:1}}}}')
    return lines(*out)


def pen_babies_function(x, y, z):
    """1,000 farm animals with AI, one in three a baby, in a pen; wheat in the player's hand
    (tempt); `pen_love` puts the adults in love 20 s later."""
    out = [fence_pen(x, y, z), "item replace entity @s weapon.mainhand with minecraft:wheat 64"]
    for i, (gx, gz) in enumerate(grid(1000, 32, 1.4, x - 21.7, z - 21.7)):
        extra = "Age:-24000" if i % 3 == 2 else 'Tags:["perf_adult"]'
        out.append(f"summon minecraft:{FARM_ANIMALS[i % 4]} {gx:.2f} {y + 1} {gz:.2f} {{{PERSIST},{extra}}}")
    return lines(*out, "schedule function perf:pen_love 400t")


def pen_love_function(_x, _y, _z):
    return lines("execute as @e[tag=perf_adult] run data merge entity @s {InLove:600}")


JOB_SITES = ["composter", "lectern", "barrel", "smoker", "blast_furnace", "cartography_table", "fletching_table",
             "loom", "grindstone", "stonecutter"]


def villagers_function(x, y, z):
    """100 villagers and 100 cows (AI on) in a pen with 20 doors and 20 job sites."""
    out = [fence_pen(x, y, z)]
    for k in range(20):
        dx = -20 + (k % 10) * 4
        dz = -23 if k < 10 else 23
        facing = "south" if k < 10 else "north"
        out += [f"setblock {x + dx} {y + 1} {z + dz} minecraft:oak_door[half=lower,facing={facing}]",
                f"setblock {x + dx} {y + 2} {z + dz} minecraft:oak_door[half=upper,facing={facing}]"]
    out += [f"setblock {gx} {y + 1} {gz} minecraft:{JOB_SITES[i % 10]}" for i, (gx, gz) in enumerate(grid(20, 10, 4, x - 18, z - 2))]
    villager = f'{PERSIST},VillagerData:{{type:"minecraft:plains",profession:"minecraft:none",level:1}}'
    out += mob_grid("villager", 100, 10, 2, x - 19.5, z - 19.5, y + 1, villager)
    out += mob_grid("cow", 100, 10, 2, x + 0.5, z + 2.5, y + 1, PERSIST)
    return lines(*out)


def bee_nests_function(x, y, z):
    """30 bee nests on oak logs, 3 bees inside each (the scene sets night or daytime rain)."""
    bee = '{entity_data:{id:"minecraft:bee"},min_ticks_in_hive:600,ticks_in_hive:0}'
    out = []
    for gx, gz in grid(30, 10, 4, x - 18, z - 4):
        out += [f"setblock {gx} {y + 1} {gz} minecraft:oak_log",
                f"setblock {gx} {y + 2} {gz} minecraft:bee_nest[facing=north]{{bees:[{bee},{bee},{bee}]}}"]
    return lines(*out)


def end_fight_function(_x, _y, _z):
    """20 endermen on the main End island, 20 to 39 blocks from its centre (dragon and its 10
    crystals are the island's own)."""
    return lines(*(f"execute in minecraft:the_end positioned {dx} 0 {dz} positioned over motion_blocking_no_leaves "
                   f"run summon minecraft:enderman ~ ~ ~ {{{PERSIST}}}" for dx, dz in ring(20, 20, 1)))


def swamp_bastion_function(x, y, z):
    """West: a 20 × 20 pool with 100 frogs and 20 axolotls. East: 20 piglins (golden swords) and
    4 hoglins, immune to the overworld conversion."""
    out = [f"fill {x - 22} {y - 2} {z - 10} {x - 3} {y} {z + 9} minecraft:water"]
    out += mob_grid("frog", 100, 10, 2, x - 21.5, z - 9.5, y + 1, PERSIST)
    out += mob_grid("axolotl", 20, 5, 4, x - 20.5, z - 8.5, y - 1, PERSIST)
    piglin = f'{PERSIST},IsImmuneToZombification:1b,equipment:{{mainhand:{{id:"minecraft:golden_sword",count:1}}}}'
    out += mob_grid("piglin", 20, 5, 3, x + 5.5, z - 6.5, y + 1, piglin)
    out += mob_grid("hoglin", 4, 4, 4, x + 6.5, z + 8.5, y + 1, f"{PERSIST},IsImmuneToZombification:1b")
    return lines(*out)


def survival_inventory_function(x, y, z):
    """Survival player 2.5 blocks from a chest fed by a hopper (supply chest of 27 × 64 above):
    the 27 slots of the main inventory full of enchanted items and maps, 64 different stacks
    dropped at the player every 10 s, hotbar emptied every 2 s (9 pickups per 2 s)."""
    out = ["tag @s add perf_survivor", "effect give @s minecraft:resistance infinite 4 true",
           "effect give @s minecraft:saturation infinite 0 true"]
    for k in range(27):
        item = f"filled_map[map_id={k // 2}]" if k % 2 else ENCHANTED[k % len(ENCHANTED)]
        out.append(f"item replace entity @s inventory.{k} with minecraft:{item}")
    supply = ",".join(f'{{Slot:{s}b,id:"minecraft:cobblestone",count:64}}' for s in range(27))
    out += [f"setblock {x} {y + 1} {z - 18} minecraft:chest[facing=north]",
            f"setblock {x} {y + 2} {z - 18} minecraft:hopper[facing=down]",
            f"setblock {x} {y + 3} {z - 18} minecraft:chest[facing=north]{{Items:[{supply}]}}"]
    return lines(*out, "function perf:survival_pickups", "function perf:survival_hotbar_clear")


def survival_pickups_function(_x, _y, _z):
    out = ["kill @e[type=minecraft:item,tag=perf_pickup]"]
    out += [f'execute at @a[tag=perf_survivor] run summon minecraft:item ~ ~0.5 ~ '
            f'{{Item:{{id:"minecraft:{item}",count:64}},PickupDelay:10,Tags:["perf_pickup"]}}' for item in ITEMS_64]
    return lines(*out, "schedule function perf:survival_pickups 200t")


def survival_hotbar_clear_function(_x, _y, _z):
    out = [f"item replace entity @a[tag=perf_survivor] hotbar.{k} with minecraft:air" for k in range(9)]
    return lines(*out, "schedule function perf:survival_hotbar_clear 40t")


def crafting_function(x, y, z):
    """Survival player in front of a crafting table, 9 stacks of logs in the hotbar, refilled
    (and the planks cleared) every 15 s by `crafting_refill`."""
    out = ["tag @s add perf_crafter", "effect give @s minecraft:saturation infinite 0 true",
           f"setblock {x} {y + 1} {z - 18} minecraft:crafting_table"]
    return lines(*out, "function perf:crafting_refill")


def crafting_refill_function(_x, _y, _z):
    out = ["clear @a[tag=perf_crafter] minecraft:oak_planks"]
    out += [f"item replace entity @a[tag=perf_crafter] hotbar.{k} with minecraft:oak_log 64" for k in range(9)]
    return lines(*out, "schedule function perf:crafting_refill 300t")


# Commands of the 256 repeating command blocks of the datapack scene (cycled).
COMMAND_BLOCK_COMMANDS = [
    "scoreboard players add #blocks perf_dp 1",
    "execute if entity @e[type=minecraft:cow,distance=..12] run scoreboard players add #near perf_dp 1",
    "execute if block ~ ~1 ~ minecraft:dirt run scoreboard players add #dirt perf_dp 1",
    "execute store result score #count perf_dp if entity @e[type=minecraft:sheep,distance=..16]",
    "execute if score #blocks perf_dp matches 1000000.. run scoreboard players set #blocks perf_dp 0",
    "data modify storage perf:dp last set value 1",
    "execute as @e[type=minecraft:pig,distance=..8,limit=1,sort=nearest] run scoreboard players add @s perf_dp 1",
    "execute unless entity @e[type=minecraft:item,distance=..4] run scoreboard players add #clean perf_dp 1",
]


def datapack_function(x, y, z):
    """crowd-1000, a realistic `#tick` workload (`datapack_tick`, on while the player carries
    `perf_datapack`) and 256 repeating command blocks under the pad (dirt layer)."""
    out = ["function perf:mobs1000", "gamerule command_block_output false",
           "scoreboard objectives add perf_dp dummy", "data modify storage perf:dp args set value {value:7}"]
    for i, (gx, gz) in enumerate(grid(256, 16, 2, x - 16, z - 16)):
        command = COMMAND_BLOCK_COMMANDS[i % len(COMMAND_BLOCK_COMMANDS)]
        out.append(f"setblock {gx} {y - 2} {gz} minecraft:repeating_command_block{{auto:1b,TrackOutput:0b,Command:'{command}'}}")
    return lines(*out, "tag @s add perf_datapack")


def datapack_tick_function(_x, _y, _z):
    return lines(
        "execute as @e[type=minecraft:cow,distance=..64] at @s run tp @s ~ ~ ~ ~2 ~",
        "scoreboard players add @e[type=minecraft:pig,distance=..64] perf_dp 1",
        "execute as @e[type=minecraft:sheep,limit=50,sort=nearest] if data entity @s {Color:0b} "
        "run scoreboard players add #white perf_dp 1",
        "execute if score #white perf_dp matches 1000.. run scoreboard players reset #white perf_dp",
        "effect give @e[type=minecraft:chicken,distance=..64] minecraft:speed 2 0 true",
        "kill @e[type=minecraft:item,distance=..64]",
        "particle minecraft:happy_villager ~ ~ ~ 4 1 4 0 8 normal",
        "function perf:datapack_macro with storage perf:dp args",
    )


def datapack_macro_function(_x, _y, _z):
    return lines("$scoreboard players set #macro perf_dp $(value)")


# --- Interface scenes ----------------------------------------------------------------------


def survival_hud_function(_x, _y, _z):
    """Survival HUD under water: 7 effects (absorption, water breathing…), diamond armour,
    hotbar and inventory stacks of 2 to 64, 30 levels."""
    out = [f"effect give @s minecraft:{e} infinite {a} {h}" for e, a, h in (
        ("resistance", 4, "true"), ("water_breathing", 0, "false"), ("absorption", 1, "false"), ("speed", 0, "false"),
        ("haste", 1, "false"), ("strength", 0, "false"), ("regeneration", 0, "false"), ("saturation", 0, "true"))]
    out += [f"item replace entity @s armor.{s} with minecraft:diamond_{p}"
            for s, p in (("head", "helmet"), ("chest", "chestplate"), ("legs", "leggings"), ("feet", "boots"))]
    for k in range(36):
        slot = f"hotbar.{k}" if k < 9 else f"inventory.{k - 9}"
        out.append(f"item replace entity @s {slot} with minecraft:{ITEMS_64[k]} {2 + (k * 62) // 35}")
    return lines(*out, "xp add @s 30 levels", "tp @s ~ 50 ~")


def hud_busy_function(_x, _y, _z):
    """3 boss bars, a 15-line sidebar, then `hud_busy_loop` every second: title, subtitle and
    action bar with a counter, bar value and one sidebar line follow it."""
    out = []
    for k, color in enumerate(("red", "blue", "green"), 1):
        out += [f'bossbar add perf:busy{k} "Busy bar {k}"', f"bossbar set perf:busy{k} players @a",
                f"bossbar set perf:busy{k} color {color}", f"bossbar set perf:busy{k} max 100000"]
    out += ['scoreboard objectives add perf_side dummy "Busy sidebar"', "scoreboard objectives setdisplay sidebar perf_side",
            "scoreboard objectives add perf_clock dummy"]
    out += [f"scoreboard players set Line{k:02d} perf_side {16 - k}" for k in range(1, 16)]
    return lines(*out, "function perf:hud_busy_loop")


def hud_busy_loop_function(_x, _y, _z):
    counter = '{score:{name:"#t",objective:"perf_clock"}}'
    return lines(
        "scoreboard players add #t perf_clock 1",
        "title @a times 0 30 0",
        f'title @a title [{{text:"Title "}},{counter}]',
        'title @a subtitle {text:"subtitle",color:"aqua"}',
        f'title @a actionbar [{{text:"Action "}},{counter}]',
        "execute store result bossbar perf:busy1 value run scoreboard players get #t perf_clock",
        "scoreboard players operation Line01 perf_side = #t perf_clock",
        "schedule function perf:hud_busy_loop 20t",
    )


def enchanted_36_function(_x, _y, _z):
    """36 enchanted pieces: hotbar and the 27 slots of the main inventory."""
    out = []
    for k in range(36):
        slot = f"hotbar.{k}" if k < 9 else f"inventory.{k - 9}"
        out.append(f"item replace entity @s {slot} with minecraft:{ENCHANTED[k % len(ENCHANTED)]}")
    return lines(*out)


def trade_function(x, y, z):
    """A level 5 librarian (no AI) 2 blocks in front of the player with 8 offers; emeralds and
    books in the player's hotbar."""
    books = ["mending", "unbreaking", "efficiency", "sharpness", "protection", "silk_touch", "fortune", "looting"]
    offers = []
    for k, enchantment in enumerate(books):
        book = f'{{id:"minecraft:enchanted_book",count:1,components:{{"minecraft:stored_enchantments":{{{enchantment}:1}}}}}}'
        offers.append(f'{{buy:{{id:"minecraft:emerald",count:{5 + k}}},buyB:{{id:"minecraft:book",count:1}},sell:{book},'
                      "maxUses:12,uses:0,rewardExp:1b,xp:5,priceMultiplier:0.2f,specialPrice:0,demand:0}")
    recipes = ",".join(offers)
    villager = (f'{KEEP_MOB},Rotation:[180.0f,0.0f],VillagerData:{{type:"minecraft:plains",profession:"minecraft:librarian",level:5}},'
                f"Offers:{{Recipes:[{recipes}]}}")
    return lines(f"summon minecraft:villager {x + 0.5} {y + 1} {z - 17.5} {{{villager}}}",
                 "item replace entity @s hotbar.1 with minecraft:emerald 64",
                 "item replace entity @s hotbar.2 with minecraft:book 64")


def tab_list_function(_x, _y, _z, guests=79):
    """4 coloured teams over the LAN guests (`Invite0`…, one member per `team join`: the
    argument takes a single holder), hearts objective in the player list."""
    teams = ("red", "blue", "green", "gold")
    out = [c for color in teams for c in (f"team add perf_{color}", f"team modify perf_{color} color {color}")]
    out += [f"team join perf_{teams[i % len(teams)]} Invite{i}" for i in range(guests)]
    return lines(*out, "scoreboard objectives add perf_hp health", "scoreboard objectives setdisplay list perf_hp",
                 "scoreboard objectives modify perf_hp rendertype hearts")


def chat_flood_function(_x, _y, _z):
    return lines("scoreboard objectives add perf_chat dummy", "tag @s add perf_chat")


def chat_flood_tick_function(_x, _y, _z):
    return lines("scoreboard players add #n perf_chat 1",
                 'tellraw @s [{text:"flood line ",color:"gold"},{score:{name:"#n",objective:"perf_chat"}}]')


def team_crowd_function(_x, _y, _z):
    """crowd-1000 in one team (team checks on every name and outline)."""
    return lines("function perf:mobs1000", "team add perf_crowd",
                 "team join perf_crowd @e[type=!minecraft:player,distance=..96]")


def team_crowd_named_function(_x, _y, _z):
    """team-crowd with a visible name on each of the 1,000 mobs."""
    return lines("function perf:team_crowd",
                 'execute as @e[type=!minecraft:player,distance=..96] run data merge entity @s {CustomName:"Mob",CustomNameVisible:1b}')


def nearest_to_crowd_camera(x, y, z, spots):
    """`spots` (x, z) sorted by distance to the crowd camera (site + (0.5, 9, -21.5)), top at y + 2."""
    cam = (x + 0.5, y + 9 + 1.62, z - 21.5)
    return sorted(spots, key=lambda p: (math.dist(cam, (p[0] + 0.5, y + 2.5, p[1] + 0.5)), p))


def signs_chests_function(x, y, z):
    """400 chests (20 × 20, step 2) and 200 signs on top: the 50 nearest the camera (< 16 blocks)
    with glowing text."""
    chests = grid(400, 20, 2, x - 19, z - 19)
    out = [f"setblock {gx} {y + 1} {gz} minecraft:chest[facing=north]" for gx, gz in chests]
    for i, (gx, gz) in enumerate(nearest_to_crowd_camera(x, y, z, chests)[:200]):
        glow = "1b" if i < 50 else "0b"
        out.append(f'setblock {gx} {y + 2} {gz} minecraft:oak_sign[rotation=8]'
                   f'{{front_text:{{has_glowing_text:{glow},color:"yellow",messages:["Chest","{i}","",""]}}}}')
    return lines(*out)


TRANSFORM = "transformation:{{left_rotation:[0f,{r}f,0f,{w}f],right_rotation:[0f,0f,0f,1f],translation:[0f,{t}f,0f],scale:[{s}f,{s}f,{s}f]}}"
IDENTITY = TRANSFORM.format(r=0, w=1, t=0, s=1)
TURNED = TRANSFORM.format(r=0.7071, w=0.7071, t=0.5, s=1.5)


def displays_function(x, y, z):
    """50 displays (20 block, 20 item, 10 text), 15 of them interpolating between two poses
    every 2 s (`displays_swap_a` / `_b`)."""
    out = []
    for i, (gx, gz) in enumerate(grid(50, 10, 3, x - 13.5, z - 10.5)):
        tag = ',Tags:["perf_interp"]' if i % 10 < 3 else ""
        if i < 20:
            body = f'block_state:{{Name:"minecraft:{["stone", "glass", "oak_log", "diamond_block"][i % 4]}"}}'
            kind = "block_display"
        elif i < 40:
            body = f'item:{{id:"minecraft:{ITEMS_64[i]}",count:1}}'
            kind = "item_display"
        else:
            body = f'text:"Display {i}",billboard:"center"'
            kind = "text_display"
        out.append(f"summon minecraft:{kind} {gx} {y + 2} {gz} {{{body},{IDENTITY}{tag}}}")
    return lines(*out, "function perf:displays_swap_a")


def displays_swap_function(pose, other):
    return lambda _x, _y, _z: lines(
        f"execute as @e[tag=perf_interp] run data merge entity @s {{start_interpolation:0,interpolation_duration:40,{pose}}}",
        f"schedule function perf:{other} 40t")


def structure_air_function(x, y, z):
    """Structure block over the whole pad, 48 × 48 × 48, air shown."""
    return lines(f'setblock {x - 24} {y} {z - 24} minecraft:structure_block[mode=save]'
                 '{mode:"SAVE",name:"perf:air",posX:0,posY:1,posZ:0,sizeX:48,sizeY:48,sizeZ:48,showair:1b,showboundingbox:1b}')


# --- Rendering scenes (culling, maps) -------------------------------------------------------

MAP_WALL = (45, 20)  # columns, rows: 900 maps


def maps_wall_function(x, y, z):
    """900 item frames holding the 900 filled maps written by `write_maps`, on a stone wall 31
    blocks south of the crowd camera."""
    cols, rows = MAP_WALL
    out = [f"fill {x - 22} {y + 1} {z + 10} {x + 22} {y + rows} {z + 10} minecraft:stone"]
    for n in range(cols * rows):
        bx, by = x - 22 + n % cols, y + 1 + n // cols
        out.append(f"summon minecraft:item_frame {bx} {by} {z + 9} "
                   f'{{Facing:2b,Fixed:1b,Item:{{id:"minecraft:filled_map",count:1,components:{{"minecraft:map_id":{n}}}}}}}')
    return lines(*out)


def hidden_cows_function(x, y, z):
    """200 cows the camera cannot see: 100 inside 10 closed stone houses, 100 behind a ridge
    higher than the camera."""
    out = []
    for k in range(10):
        bx = x - 23 + 5 * k
        out.append(f"fill {bx} {y + 1} {z - 18} {bx + 3} {y + 4} {z - 13} minecraft:stone hollow")
        out += [f"summon minecraft:cow {bx + 1.5 + (i % 2)} {y + 2} {z - 16.5 + (i // 2) * 0.6:.2f} {{{KEEP_MOB}}}"
                for i in range(10)]
    out.append(f"fill {x - 23} {y + 1} {z - 10} {x + 23} {y + 14} {z - 6} minecraft:stone")
    out += mob_grid("cow", 100, 10, 2, x - 9.5, z - 1.5, y + 1, KEEP_MOB)
    return lines(*out)


# Lines run every tick: flight (2 blocks/tick), slow flight of the fresh-world paths (0.8 block/tick =
# 16 blocks/s, pitch kept), rotation (4°/tick), walking (0.22 block/tick); then the scene workloads
# switched on by a player tag (nothing runs without it: a selector over the one player).
TICK_FUNCTION = lines(
    "execute as @a[tag=perf_fly] at @s rotated ~ 0 run tp @s ^ ^ ^2",
    "execute as @a[tag=perf_glide] at @s rotated ~ 0 run tp @s ^ ^ ^0.8",
    "execute as @a[tag=perf_turn] at @s run tp @s ~ ~ ~ ~4 ~",
    "execute as @a[tag=perf_walk] at @s rotated ~ 0 run tp @s ^ ^ ^0.22",
    "execute as @a[tag=perf_chat] run function perf:chat_flood_tick",
    "execute as @a[tag=perf_reap] at @s run kill @e[type=!minecraft:player,distance=..128]",
    # Run at the player: a tick function otherwise runs at the world spawn, far from the crowd.
    "execute as @a[tag=perf_datapack] at @s run function perf:datapack_tick",
)

# Function name -> (site, builder(x, y, z)). Site `crowd` or `redstone` (pads of the model).
FUNCTIONS = {
    "mobs": ("crowd", mobs_function), "redstone": ("redstone", redstone_function),
    "mobs1000": ("crowd", mobs1000_function), "humanoids": ("crowd", humanoids_function),
    "keyframes": ("crowd", keyframes_function), "items": ("crowd", items_function),
    "glowing": ("crowd", glowing_function), "names": ("crowd", names_function), "signs": ("crowd", signs_function),
    "tnt": ("crowd", tnt_function), "tnt_once": ("crowd", tnt_once_function), "fireworks": ("crowd", fireworks_function),
    "tnt_heap": ("crowd", tnt_heap_function),
    "block_entities": ("redstone", block_entities_function),
    "hoppers": ("crowd", hoppers_function), "hoppers_feed": ("crowd", hoppers_feed_function),
    "furnaces": ("crowd", furnaces_function), "sculk": ("crowd", sculk_function),
    "minecarts": ("crowd", minecarts_function), "items_merge": ("crowd", items_merge_function),
    "redstone_dense": ("redstone", redstone_dense_function), "tnt_cannon": ("crowd", tnt_cannon_function),
    "tnt_cannon_fire": ("crowd", tnt_cannon_fire_function),
    "village_night": ("crowd", village_night_function), "raid": ("crowd", raid_function),
    "melee": ("crowd", melee_function), "archers": ("crowd", archers_function),
    "pen_babies": ("crowd", pen_babies_function), "pen_love": ("crowd", pen_love_function),
    "villagers": ("crowd", villagers_function), "bee_nests": ("crowd", bee_nests_function),
    "end_fight": ("crowd", end_fight_function), "swamp_bastion": ("crowd", swamp_bastion_function),
    "survival_inventory": ("crowd", survival_inventory_function),
    "survival_pickups": ("crowd", survival_pickups_function),
    "survival_hotbar_clear": ("crowd", survival_hotbar_clear_function),
    "crafting": ("crowd", crafting_function), "crafting_refill": ("crowd", crafting_refill_function),
    "datapack": ("crowd", datapack_function), "datapack_tick": ("crowd", datapack_tick_function),
    "datapack_macro": ("crowd", datapack_macro_function),
    "survival_hud": ("crowd", survival_hud_function), "hud_busy": ("crowd", hud_busy_function),
    "hud_busy_loop": ("crowd", hud_busy_loop_function), "enchanted_36": ("crowd", enchanted_36_function),
    "trade": ("crowd", trade_function), "tab_list": ("crowd", tab_list_function),
    "chat_flood": ("crowd", chat_flood_function), "chat_flood_tick": ("crowd", chat_flood_tick_function),
    "team_crowd": ("crowd", team_crowd_function), "team_crowd_named": ("crowd", team_crowd_named_function),
    "signs_chests": ("crowd", signs_chests_function), "displays": ("crowd", displays_function),
    "displays_swap_a": ("crowd", displays_swap_function(TURNED, "displays_swap_b")),
    "displays_swap_b": ("crowd", displays_swap_function(IDENTITY, "displays_swap_a")),
    "structure_air": ("crowd", structure_air_function), "maps_wall": ("crowd", maps_wall_function),
    "hidden_cows": ("crowd", hidden_cows_function),
}


def pack_functions(sites):
    """Functions of the `perf` pack: name -> text."""
    out = {"tick": TICK_FUNCTION}
    for name, (site, build) in FUNCTIONS.items():
        out[name] = build(*sites[site])
    return out


def write_text(path, text):
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def write_pack(world, sites):
    """The `perf` pack in `world/datapacks/perf` (functions, `#minecraft:tick` tag)."""
    pack = os.path.join(world, "datapacks", "perf")
    fn = os.path.join(pack, "data", "perf", "function")
    tags = os.path.join(pack, "data", "minecraft", "tags", "function")
    os.makedirs(fn, exist_ok=True)
    os.makedirs(tags, exist_ok=True)
    write_text(os.path.join(pack, "pack.mcmeta"),
               json.dumps({"pack": {"description": "perf bench", "pack_format": 121, "min_format": 121, "max_format": 121}}))
    write_text(os.path.join(tags, "tick.json"), json.dumps({"values": ["perf:tick"]}))
    for name, text in pack_functions(sites).items():
        write_text(os.path.join(fn, f"{name}.mcfunction"), text)


# --- Map data -------------------------------------------------------------------------------

MAP_COUNT = MAP_WALL[0] * MAP_WALL[1]
DATA_VERSION = 5023


MAP_BLACK, MAP_WHITE = 29, 8  # base colours of the number written in the corner


def map_colors(n):
    """128 × 128 colour bytes of map `n`: 8 × 8 tiles of base colours 1-61 in 4 shades arranged
    by `n`, and `n` written in binary (black / white, 10 pixels by 4) in the top-left corner, so
    that no two of the 900 maps are alike."""
    rows = []
    for ty in range(8):
        bases = [1 + (n * 7 + tx * 3 + ty * 5) % 61 for tx in range(8)]
        # The 4 shade phases of a tile row, built once (a row is one of them).
        phases = [bytes(bases[px >> 4] * 4 + ((px + s) & 3) for px in range(128)) for s in range(4)]
        rows += [phases[(ty * 16 + r + n) & 3] for r in range(16)]
    corner = bytes((MAP_BLACK if n >> k & 1 else MAP_WHITE) * 4 + 2 for k in range(10))
    rows[:4] = [corner + row[10:] for row in rows[:4]]
    return b"".join(rows)


def write_maps(world, count=MAP_COUNT):
    """Locked filled maps `0` … `count - 1` (`data/minecraft/maps/<n>.dat`) and the id counter."""
    maps = os.path.join(world, "data", "minecraft", "maps")
    os.makedirs(maps, exist_ok=True)
    for n in range(count):
        data = {
            "dimension": (nbt.STRING, "minecraft:overworld"), "xCenter": (nbt.INT, 0), "zCenter": (nbt.INT, 0),
            "scale": (nbt.BYTE, 0), "colors": (nbt.BYTES, map_colors(n)), "trackingPosition": (nbt.BYTE, 0),
            "unlimitedTracking": (nbt.BYTE, 0), "locked": (nbt.BYTE, 1),
            "banners": nbt.List(nbt.COMPOUND, []), "frames": nbt.List(nbt.COMPOUND, []),
        }
        nbt.write_dat(os.path.join(maps, f"{n}.dat"), {"data": data, "DataVersion": (nbt.INT, DATA_VERSION)})
    nbt.write_dat(os.path.join(maps, "last_id.dat"),
                  {"data": {"map": (nbt.INT, count - 1)}, "DataVersion": (nbt.INT, DATA_VERSION)})
