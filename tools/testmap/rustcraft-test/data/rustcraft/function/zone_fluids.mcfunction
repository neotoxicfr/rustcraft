# Fluids (z 44..64): pool with a waterfall (current), bubble columns, lava under glass,
# free flow: source at the centre of an 11-block plateau (no hole within 4 blocks,
# otherwise the water only flows toward that hole) that spills over the edges in waterfalls.
fill 0 -62 44 15 -60 55 minecraft:water
fill 0 -59 43 0 -55 43 minecraft:stone
setblock 0 -55 44 minecraft:water
fill 20 -61 44 26 -52 50 minecraft:glass hollow
fill 21 -60 45 25 -53 49 minecraft:water
setblock 22 -61 47 minecraft:soul_sand
setblock 24 -61 47 minecraft:magma_block
fill 22 -60 47 22 -53 47 minecraft:bubble_column[drag=false]
fill 24 -60 47 24 -53 47 minecraft:bubble_column[drag=true]
fill 30 -61 44 36 -58 50 minecraft:glass hollow
fill 31 -60 45 35 -60 49 minecraft:lava
fill 38 -59 44 48 -59 56 minecraft:stone
setblock 43 -58 50 minecraft:water
