# Conversion of older saved data (`rc-datafix`)

Every file the game saves carries the data version it was written with (`DataVersion`; the
options file keeps it as a `version:` line). When the game reads a file of an older version, it
first brings it to its own version, then reads it as current data. Rustcraft does the same with the
`rc-datafix` crate, which knows nothing of the server: it works on the NBT tree.

## Versions

- Current data version: 5023 (26.3).
- A file without `DataVersion` takes the default of its reader: -1 for a chunk, an entity chunk,
  a player and `level.dat`; 1945 for a points of interest chunk; 1343 for the hotbars, statistics,
  advancements and the world's saved data; 500 for a structure template; 0 for the options; 4649
  for the debug screen profile. Any integer width is read, cut to 32 bits.
- Data of a newer version is read as it is (the world list offers a backup first).
- Data older than the oldest version the conversion covers (`OLDEST_CONVERTIBLE`, 4899 for now)
  is left untouched (`Outcome::TooOld`); see "Too old" below.

## World list and file moves

The game moved world files around in its versions 4772, 4773 and 4899. A world whose `level.dat` is
older than the last move (`requires_file_fixing`: data version below 4899, every version before the
first move counting as 0) shows "Upgrade and Play", offers a backup and shows its version in
italics; any newer world shows "Play" and has its data converted as it opens, with editing and
re-creation available. Rustcraft does not move files yet: such a world keeps its play, edit and
re-create buttons off, with one extra tooltip line.

## The chain

A rule changes one kind of data from one version (with a step number for some versions: `4996.1`).
The chain lists the rules in the game's order: by version, then step, then the order the game gives
them within a step. Converting a file applies, one after the other, every rule newer than the file's
version, then writes `DataVersion` = 5023. Each rule is applied everywhere its kind of data lies in
the file, outermost first (a rule on item stacks reaches the stacks in a shulker box item in a chest
minecart ridden by a zombie in a chunk).

Rules above 4899, all written: 4996 pot decorations (list of four item ids to four sides, missing
or non-text entry a brick, an empty id keeps the list), 4996.1 decorated pot sherds (same; a pot
without sherds gets four bricks), 4997 terrain blending data (removed, then recreated in the
overworld: sections -4 to 20 past the biome step, 0 to 16 for an under-zero regeneration towards
such a step), 5001 debug profile entry renamed, 5002 signs and hanging signs allow operator
features, 5006 block state fields `Name` / `Properties` renamed `id` / `properties`, 5007 swing
animation split into attack and interact animations, 5008 explorer maps become their own items
(their old name key dropped) and the map colour component goes, 5009 blending heights as floats
(non-numbers and the largest double become the largest float), 5012 explorer map items renamed,
5013 the old terrain steps merged (carvers to terrain, noise and surface back to biomes, with the
blocks, heights and blending of a chunk stopped in between dropped), 5014 the inline block
transformer component goes, 5016 an empty map named with the old buried treasure key takes the new
item key.

## The map of nested data

For each kind (chunk, entity, block entity, item stack, data components, text, villager trade,
particle, spawner, scoreboard objective and team, world generation settings, identifiers...), the
map says which fields hold which other kind, with the fields an entity or block entity gets from its
identifier. It changes with the versions; a rule reads its input with the map in force just before
its own version. Differences in the versions covered: 4996 (pot decorations as four item stacks;
`can_break` and `can_place_on` back to the layout with a `predicates` list: the game's own map of
the components starts again from an older one there), 4996.1 (the pot's sherds), 4997 and 5000
(new entities).

## Reading as the game reads

Before any rule, the data is read with the map of its own version, as the game's conversion does:

- typed identifiers (item, block, entity, biome, recipe, game event names, an entity's or block
  entity's `id`, typed keys of statistics, dimension names) get the `minecraft:` namespace when they
  have none and are valid identifiers; block states inside are left as written;
- an item stack written as a bare identifier becomes `{id, count: 1}`;
- a scoreboard criterion `type:id` (dotted identifiers) is unpacked, its id visited, then packed
  again with the canonical dots;
- an optional field whose value does not read as its kind is left out of the whole conversion,
  kept as it is. For example, one entity of unknown identifier in a chunk's `entities` list leaves
  that entire list unconverted (the other entities included), the rest of the chunk converting; a
  block entity of unknown identifier is kept as it is, the others converted. Lists need every
  element to read, required fields must be present, choices need a known key.

## Where it is applied

- `level.dat` (`Data` compound) when the world opens; the server refuses only a world needing
  file moves (or older than the chain).
- Players (`players/data`), at their first read.
- Chunks, at every read, before anything else (the status is never looked at before the version):
  a converted chunk is marked to be written back at the next save. The generation thread converts
  the chunks it rereads the same way; a converted proto-chunk is written back converted.
- Entity chunks and points of interest, as they are read.
- "Optimize World": every chunk, entity chunk and points of interest chunk of an older version is
  converted and rewritten ("converted"); one too old is "skipped".

## Too old

Data older than the chain covers is never misread:

- a chunk is skipped (`Skip::OldVersion`) before its status is read: neither "unfinished" (which
  would have it generated again over the player's terrain) nor empty; it is never rewritten;
- the generation never makes a chunk whose making would read or write such a chunk (one in its
  3 × 3, or next to a neighbour still to decorate); it reports it refused and the server leaves it
  empty;
- an entity chunk is not read, and its file is never written over;
- a points of interest chunk is read as absent (the loaded chunk's blocks give the points again,
  their claims lost);
- a player file is read as it is (as before the conversion existed).

Each older version the chain gains lowers `OLDEST_CONVERTIBLE`, with its rules and map changes.

## Known differences

- A rule receives no chunk generator identifier (the game gives it; no rule above 4899 reads it).
- A converted chunk is written back at the next save even when nothing else changes in it (the
  game keeps the old version on the disk until something does); the data read are the same.

## Proof

`crates/rc-datafix/src/tests/oracle.rs`: inputs written for each kind and version, converted by the
official game's converter (`tools/extractor/datafix_oracle`, an extractor: it calls the game's own
conversion through its API on the player's installed jar) and stored in
`crates/rc-datafix/tests/oracle/`; our conversion must give the same tree (compound field order
aside, which the game does not keep). These files are the game's conversion of inputs we write
ourselves, kept as reference outputs for this comparison, like `tools/bench/test_data/`.
