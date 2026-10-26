# Version facts extractor

Queries the player's official server jar and writes, outside the repository, the per-block-state
facts the client needs and that neither the server nor the official reports provide: collision,
outline, occlusion and support shapes (deduplicated), light emission and dampening, flags (air,
liquid, skylight passing through, replaceable…), fluid, and per block: friction, speed and jump
factors.

Launch (Git Bash; the launcher's Java contains the compiler, no prior compilation):

```bash
S="$LOCALAPPDATA/rustcraft-re/26.3/server"
CP="$(cygpath -w "$S/versions/26.3/server-26.3.jar")"
while IFS= read -r j; do CP="$CP;$(cygpath -w "$j")"; done < <(find "$S/libraries" -name "*.jar")
"$JAVA" -cp "$CP" tools/extractor/Extractor.java "$LOCALAPPDATA/rustcraft-re/26.3/facts.json"
```

The server jar must have been run once (it then unpacks `versions/` and `libraries/`).
Bench: 3.7 s, 1.1 MB for 26.3. Check: id ranges and default state of each block identical to the
`blocks.json` report (0 differences over 1,286 blocks).

The names called here are the game's public API, as for a mod: no code is copied.
`rc-data` will run this extraction automatically on the first start of a version.
