# Reference ground: grass at y = -60, walking at y = -59.
# Zone plan: docs/test-map.md.
forceload add -32 -32 159 175
gamerule advance_time false
gamerule advance_weather false
gamerule spawn_mobs false
gamerule random_tick_speed 0
time set 6000
setworldspawn 0 -59 -8
function rustcraft:zone_friction
function rustcraft:zone_steps
function rustcraft:zone_climb
function rustcraft:zone_slow
function rustcraft:zone_fluids
function rustcraft:zone_render
function rustcraft:zone_biomes
function rustcraft:zone_redstone
function rustcraft:zone_entities
scoreboard players set #map rustcraft 1
say Rustcraft test map built.
