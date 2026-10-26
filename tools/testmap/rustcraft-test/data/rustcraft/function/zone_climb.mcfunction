# Climbables (z 24..30): ladder, vines, scaffolding, twisting vines, trapdoor on a ladder.
fill 0 -59 24 0 -50 24 minecraft:stone
fill 0 -59 25 0 -50 25 minecraft:ladder[facing=south]
fill 4 -59 24 4 -50 24 minecraft:stone
fill 4 -59 25 4 -50 25 minecraft:vine[north=true]
fill 8 -59 25 8 -50 25 minecraft:scaffolding[distance=0,bottom=false]
fill 12 -59 25 12 -50 25 minecraft:twisting_vines_plant
setblock 12 -49 25 minecraft:twisting_vines
fill 16 -59 24 16 -55 24 minecraft:stone
fill 16 -59 25 16 -56 25 minecraft:ladder[facing=south]
setblock 16 -55 25 minecraft:oak_trapdoor[facing=south,open=true]
