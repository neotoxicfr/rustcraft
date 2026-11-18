# Steps and shapes (z 12..20): step-up, support, fences, walls, snow.
fill 0 -59 12 3 -59 12 minecraft:oak_slab[type=bottom]
fill 0 -59 14 3 -59 14 minecraft:stone_stairs[facing=south]
fill 0 -59 16 3 -59 16 minecraft:white_carpet
fill 0 -60 18 3 -60 18 minecraft:dirt_path
fill 6 -59 12 9 -59 12 minecraft:snow[layers=1]
fill 6 -59 13 9 -59 13 minecraft:snow[layers=4]
fill 6 -59 14 9 -59 14 minecraft:snow[layers=7]
fill 6 -59 15 9 -59 15 minecraft:snow[layers=8]
fill 12 -59 12 16 -59 12 minecraft:oak_fence
fill 12 -59 15 16 -59 15 minecraft:cobblestone_wall
setblock 18 -59 15 minecraft:oak_fence_gate[facing=north]
fill 20 -60 12 23 -60 12 minecraft:farmland
setblock 26 -59 13 minecraft:stone_slab[type=bottom]
setblock 26 -59 14 minecraft:stone_slab[type=top]
setblock 26 -58 15 minecraft:stone_slab[type=top]
setblock 30 -59 12 minecraft:stone_stairs[facing=south]
setblock 30 -58 13 minecraft:stone_stairs[facing=south]
setblock 30 -57 14 minecraft:stone_stairs[facing=south]
# One-block walkway above a pit (stopping at the edge while sneaking).
fill 34 -62 12 44 -60 20 minecraft:air
fill 34 -60 16 44 -60 16 minecraft:stone
