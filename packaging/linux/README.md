# Linux package

What goes into the Linux archive, next to the `rustcraft` binary (`cargo build --release -p rc-app`):

- `rustcraft.sh`: the program to choose as **Java executable** in the official launcher. It puts
  Rustcraft's worlds, options and server list in `~/.local/share/rustcraft` and its cache in
  `~/.cache/rustcraft` (otherwise they would land in `/tmp` and worlds would be lost on reboot),
  finds the launcher's Java for the one-time data preparation, then starts `rustcraft` with the
  launcher's arguments. Every location can be overridden with `RUSTCRAFT_GAME_DIR`,
  `RUSTCRAFT_CACHE`, `RUSTCRAFT_MC_DIR` and `RUSTCRAFT_JAVA`.
- `INSTALL.txt`: installation steps and what to check, for players.
- `LICENSE-MIT` and `LICENSE-APACHE` from the repository root.

Requirements: x86-64, glibc 2.35 or newer, a Vulkan driver (there is no OpenGL fallback), ALSA
(PulseAudio and PipeWire provide it), and for copy and paste `wl-clipboard` on Wayland or `xclip` /
`xsel` on X11. Links open with `xdg-open`.
