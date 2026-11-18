# 8x8 friction plates along x (z 0..7): ice, packed ice, blue ice,
# soul sand, honey block, slime block, dirt (reference), soul soil.
fill 0 -60 0 7 -60 7 minecraft:ice
fill 10 -60 0 17 -60 7 minecraft:packed_ice
fill 20 -60 0 27 -60 7 minecraft:blue_ice
fill 30 -60 0 37 -60 7 minecraft:soul_sand
fill 40 -60 0 47 -60 7 minecraft:honey_block
fill 50 -60 0 57 -60 7 minecraft:slime_block
fill 60 -60 0 67 -60 7 minecraft:dirt
fill 70 -60 0 77 -60 7 minecraft:soul_soil
# Fall tower above the slime (bounce) and bed (damped bounce).
fill 52 -59 3 52 -50 3 minecraft:oak_planks
setblock 54 -59 10 minecraft:red_bed[facing=south,part=foot]
setblock 54 -59 11 minecraft:red_bed[facing=south,part=head]
