# Redstone (x 80..115, z -2..15): wire shapes, repeaters, comparators, lamps, and the known
# "bug" behaviours: quasi-connectivity, BUD switch, observer clock
# that keeps blocks moving permanently.
setblock 80 -59 0 minecraft:redstone_wire
fill 82 -59 0 86 -59 0 minecraft:redstone_wire
fill 88 -59 0 92 -59 0 minecraft:redstone_wire
fill 90 -59 -2 90 -59 2 minecraft:redstone_wire
setblock 94 -59 0 minecraft:redstone_block
fill 95 -59 0 99 -59 0 minecraft:redstone_wire
setblock 100 -59 0 minecraft:redstone_lamp
setblock 80 -59 4 minecraft:redstone_block
setblock 81 -59 4 minecraft:repeater[facing=west,delay=4]
setblock 82 -59 4 minecraft:redstone_lamp
setblock 84 -59 4 minecraft:redstone_block
setblock 85 -59 4 minecraft:comparator[facing=west]
setblock 86 -59 4 minecraft:redstone_lamp
# Clock of observers facing each other that drives a sticky piston (moving blocks).
setblock 82 -59 10 minecraft:sticky_piston[facing=east]
setblock 83 -59 10 minecraft:slime_block
setblock 80 -59 10 minecraft:observer[facing=east]
setblock 81 -59 10 minecraft:observer[facing=west]
# Quasi-connectivity: the lever powers the block diagonally above the piston.
setblock 90 -59 10 minecraft:piston[facing=up]
setblock 91 -58 10 minecraft:stone
setblock 91 -57 10 minecraft:lever[face=floor,facing=north]
# BUD switch: sticky piston next to a redstone block diagonally.
setblock 96 -59 10 minecraft:sticky_piston[facing=up]
setblock 97 -58 10 minecraft:redstone_block
# Rails powered by a lever, trapdoor with a lever.
setblock 104 -59 10 minecraft:lever[face=floor,facing=north,powered=true]
fill 105 -59 10 112 -59 10 minecraft:powered_rail[shape=east_west]
setblock 104 -59 14 minecraft:oak_trapdoor[facing=north,open=false]
setblock 105 -59 14 minecraft:lever[face=floor,facing=north]
