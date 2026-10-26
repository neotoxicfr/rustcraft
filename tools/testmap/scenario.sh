#!/usr/bin/env bash
# Replays a scenario on the test server: starts the client with a per-tick trace, sends the
# file's RCON commands (one per line, "sleep N" to wait), closes the client, then prints the
# server's movement complaints and the trace summary.
#
#   tools/testmap/scenario.sh tools/testmap/scenarios/effects.txt [seconds of auto-walk]
set -e
SCEN="$1"
WALK="${2:-0}"
T="$LOCALAPPDATA/rustcraft-re/26.3/server-test"
OUT="${TMPDIR:-/tmp}/rustcraft-scenario"
mkdir -p "$OUT"
LOG="$T/console.log"
before=$(wc -l < "$LOG")
cd "$(dirname "$0")/../.."
env RUSTCRAFT_TRACE="$OUT/trace.txt" $([ "$WALK" != 0 ] && echo RUSTCRAFT_AUTOWALK=$WALK) \
    ./target/release/rustcraft.exe 127.0.0.1 25570 Rustcraft > "$OUT/client.log" 2>&1 &
sleep 8
mapfile -t CMDS < <(grep -v '^\s*\(#\|$\)' "$SCEN")
python tools/testmap/rcon.py "$T" "${CMDS[@]}" | grep -A1 '^> execute' || true
taskkill //IM rustcraft.exe //F > /dev/null
echo "--- server ---"
tail -n +$((before + 1)) "$LOG" | grep -i "moved\|kick\|disconnect\|warn\|error" || echo "no complaints"
echo "--- trace ($OUT/trace.txt) ---"
python tools/testmap/trace.py "$OUT/trace.txt"
