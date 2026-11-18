# Entities (x 0..40, z 140..150), without AI so they stay in place: animals, monsters,
# armor stand, boat, minecart on a rail.
summon minecraft:cow 2 -59 142 {NoAI:1b,PersistenceRequired:1b}
summon minecraft:sheep 6 -59 142 {NoAI:1b,PersistenceRequired:1b,Color:14}
summon minecraft:pig 10 -59 142 {NoAI:1b,PersistenceRequired:1b}
summon minecraft:chicken 14 -59 142 {NoAI:1b,PersistenceRequired:1b}
summon minecraft:zombie 18 -59 142 {NoAI:1b,PersistenceRequired:1b}
summon minecraft:skeleton 22 -59 142 {NoAI:1b,PersistenceRequired:1b}
summon minecraft:creeper 26 -59 142 {NoAI:1b,PersistenceRequired:1b}
summon minecraft:villager 30 -59 142 {NoAI:1b,PersistenceRequired:1b}
summon minecraft:horse 34 -59 142 {NoAI:1b,PersistenceRequired:1b}
summon minecraft:armor_stand 2 -59 148 {ShowArms:1b}
summon minecraft:oak_boat 6 -59 148
fill 10 -59 148 16 -59 148 minecraft:rail[shape=east_west]
summon minecraft:minecart 12 -59 148
