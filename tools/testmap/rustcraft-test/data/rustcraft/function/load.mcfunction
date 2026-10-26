# Builds the map only once (marker: score of #map).
scoreboard objectives add rustcraft dummy
execute unless score #map rustcraft matches 1 run function rustcraft:build
