# On-screen player list (Tab key, 26.3)

Source: reading of the decompiled client (outside the repository), reworded. Data: `players.md`.

## 1. Display

- Visible while the "player list" key (Tab) is held, outside text input (screen
  open) and with the HUD visible. On a local server (singleplayer), only if more than one player is listed
  or there is a "list" display objective.
- Drawn in a new layer, after the chat.
- `tab_list` packet: header then footer (NBT text components); a component whose text
  is empty is not displayed.

## 2. Players

- "Listed" entries only, sorted by: list order descending, spectators last,
  team name, profile name case-insensitively; at most 80.
- Name: the entry's display name if it exists, otherwise the profile name formatted by the team; in
  italics for a spectator.

## 3. Layout (interface units)

- Rows = number of players; while rows > 20: columns + 1, rows = ⌈players / columns⌉.
- Head (8 × 8, + 1) only if the server is in online mode (`online_mode` of the login
  packet).
- Cell width = min(columns × (head + largest name + score width + 13),
  screen width − 50) / columns; start x = width / 2 − (cell × columns +
  (columns − 1) × 5) / 2; start y = 10. Integer divisions.
- Width of the bands = max(width of the cells, header and footer lines). Header and footer
  wrapped to screen width − 50, centered, 9 per line, on a 50% black background
  (0x80000000) from x = w/2 − width/2 − 1 to w/2 + width/2 + 1, y − 1 to y + lines × 9;
  1 pixel gap after the header and before the footer.
- Players band: same background, height rows × 9. Each cell: 0x20FFFFFF background (default color when the
  text background is reserved for the chat) 8 high; name with shadow, white
  (0x90FFFFFF for a spectator); latency icon 10 × 8 at x + cell width − 11.
- Latency: < 0 unknown, < 150 five bars, < 300 four, < 600 three, < 1000 two, otherwise one
  (sprites `icon/ping_*`).
- Score of the "list" objective (number, or hearts for a hearts objective, with blinking):
  not tracked for now (scoreboard to do).
