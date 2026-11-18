#!/bin/sh
# Rustcraft Linux launcher script: to be chosen as the "Java executable" of the 26.3 installation in the
# official launcher. It puts Rustcraft's data in lasting folders (otherwise it would go to /tmp and
# worlds would be lost on reboot), then starts the game with the same arguments.
HERE=$(dirname "$(readlink -f "$0")")
DATA=${XDG_DATA_HOME:-$HOME/.local/share}/rustcraft
export RUSTCRAFT_GAME_DIR=${RUSTCRAFT_GAME_DIR:-$DATA}
export RUSTCRAFT_CACHE=${RUSTCRAFT_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/rustcraft}
export RUSTCRAFT_MC_DIR=${RUSTCRAFT_MC_DIR:-$HOME/.minecraft}
# The official launcher's Java (used only once, to prepare the version's data).
if [ -z "$RUSTCRAFT_JAVA" ]; then
    for j in "$RUSTCRAFT_MC_DIR"/runtime/*/linux/*/bin/java; do
        [ -x "$j" ] && RUSTCRAFT_JAVA=$j
    done
    [ -n "$RUSTCRAFT_JAVA" ] && export RUSTCRAFT_JAVA
fi
mkdir -p "$RUSTCRAFT_GAME_DIR" "$RUSTCRAFT_CACHE"
exec "$HERE/rustcraft" "$@"
