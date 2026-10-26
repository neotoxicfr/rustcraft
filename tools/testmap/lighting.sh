#!/usr/bin/env bash
# Client light engine against the server (bench server, player "Bench", absolute coordinates
# near its starting point; adapt if the bench changes).
# 1) cleared zone, reference light; 2) client A applies the changes with its engine and
# writes its light; 3) fresh client B receives the server's light. A must equal B, B differ from 1.
S="${TMPDIR:-/tmp}/rustcraft-light"; mkdir -p "$S"
B="$LOCALAPPDATA/rustcraft-re/26.3/server"
cd /d/Rustcraft
R() { python tools/testmap/rcon.py "$B" "$@" > /dev/null; }
run() { RUSTCRAFT_LIGHTDUMP="$S/$1.bin" RUSTCRAFT_CAPTURE="$S/$1.bmp;$2" ./target/release/rustcraft.exe 127.0.0.1 25565 Bench > "$S/$1.log" 2>&1; }
R "time set midnight" "fill -67 88 -193 -57 93 -183 minecraft:air"
sleep 1
run light_base 7
run light_ours 16 &
sleep 6
R "setblock -64 88 -188 minecraft:torch"
sleep 1
R "setblock -62 89 -188 minecraft:glowstone"
sleep 1
R "fill -66 91 -190 -60 91 -185 minecraft:stone"
sleep 1
R "setblock -62 89 -188 minecraft:air"
sleep 1
R "setblock -65 88 -186 minecraft:sea_lantern"
sleep 1
R "setblock -63 91 -187 minecraft:air"
wait
sleep 1
run light_server 7
R "time set noon"
python - "$S" <<'EOF'
import sys
S = sys.argv[1]
def load(n): return open(f"{S}/{n}.bin", 'rb').read()
def diff(a, b):
    return sum(1 for i in range(min(len(a), len(b))) for sh in (0, 4) if (a[i] >> sh) & 15 != (b[i] >> sh) & 15)
o, s, b = load('light_ours'), load('light_server'), load('light_base')
print('sizes', len(o), len(s), len(b))
print('client engine / server:', diff(o, s))
print('server / reference (must be > 0):', diff(s, b))
EOF
