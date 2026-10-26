# Rustcraft

**Minecraft: Java Edition 26.3 rewritten in Rust: the client and its integrated server, built to play the same game, faster.**

![Rust 1.99](https://img.shields.io/badge/Rust-1.99-orange?logo=rust)
![Minecraft Java 26.3](https://img.shields.io/badge/Minecraft_Java-26.3-62B47A)
![Protocol 777](https://img.shields.io/badge/protocol-777-blue)
![wgpu: Vulkan or DX12](https://img.shields.io/badge/render-Vulkan_%7C_DX12-AC162C?logo=vulkan)
![Windows and Linux](https://img.shields.io/badge/platform-Windows_%7C_Linux-0078D6)

[Version française](README.fr.md)

Rustcraft replaces the game's Java client with a native program. You start it from the **official
launcher**, with your own account, and you join the same servers (vanilla, Paper…) or play your own
worlds in singleplayer. The game itself is meant to be unchanged: same rules, same world generation,
same protocol. What changes is the engine underneath: no JVM, no garbage collector, every CPU core
busy, and a modern GPU pipeline.

> [!WARNING]
> Unofficial and **work in progress**. Multiplayer and singleplayer are playable, but parts of the
> game are still missing or not yet checked against the official one. Read [Status](#status) before
> you rely on it, and keep backups of worlds you care about.

## Status

The project follows one rule: wherever the official game has a behaviour, the official game is the
specification. A full audit of the game's code turned every known difference into a mission; the
numbers below come from that list.

| Area | Where it stands |
|---|---|
| Fidelity to the official game | **53 of 83** audit missions fixed and merged, 8 more implemented and under review |
| Performance work | **47 of 125** planned optimisations done, each one measured |
| Tests | about **2,200** automated tests, many of them compared bit for bit with the official game |

### What works well

- **Multiplayer**, offline and online: Microsoft account through the official launcher, encryption,
  compression, signed chat, resource packs sent by servers, LAN.
- **Singleplayer** on an integrated server written in Rust: create, open, play and save worlds in
  the official format.
- **World generation** identical to the official game for the same seed (noise, biomes, terrain and
  decoration are checked against it by reference tests).
- **Lighting** identical to the official game, and the **physics** of the player and of mounts.
- **Redstone**, with the official game's quirks reproduced on purpose (quasi-connectivity, update
  order), and the bugs Mojang keeps listed in [docs/quirks-to-reproduce.md](docs/quirks-to-reproduce.md).
- **Mobs and villagers** (AI, trading, breeding, raids), blocks and block entities, items,
  enchantments, effects, commands with autocompletion.
- **Interface**: title screen, server list, HUD, inventories, creative inventory, recipe book, chat,
  player list, scoreboards, boss bars, F3 screen, options and `options.txt` shared with the official
  game; sounds, music and particles.

### Work in progress

Implemented and waiting to be merged: leads and saddled pigs, entity contacts (fire, rain, magma,
berries, honey, falls, armour wear), bone meal, sulfur blocks and projectiles, golems, block entity
and mob effects on the client.

Still on the fidelity list, by priority:

- combat and offensive enchantments; experience orbs, dropped items and display entities;
- nature ticks and turtle eggs; technical blocks (structure, jigsaw, test), `/place`, `/clone`, `/locate`;
- some commands (`/loot`, `/enchant`, attributes…), data pack tags and `/reload`, saving, `/schedule` and `/data`;
- mob spawning and spawn eggs, navigation and maps, the locator bar;
- the F3 and debug screens, inventory screens and option menus, small client effects.

Implemented but **not yet systematically compared** with the official game: fluid flow, entity
movement and collisions, the damage chain, hunger, generic menu clicks, attribute lines in tooltips.

### Known gaps

- **macOS** is not supported yet (planned last).
- **Linux** builds and runs, but has had very little real play. It needs a GPU with a Vulkan driver:
  there is no OpenGL fallback, so older integrated GPUs fall back to software rendering.
- No **narrator**, no **shader packs**, and resource packs that replace the game's core shaders are
  not supported.
- Worlds saved by **older versions** of the game are not converted; open them with 26.3 first.
- The **dedicated server** is planned, after the client.
- Not planned: Realms, telemetry, RCON and the JSON-RPC management protocol, and signing in to a
  Microsoft account inside Rustcraft (the official launcher does it; Rustcraft never sees your
  password).

Every difference found so far is listed with its cause in [docs/gaps-audit2.md](docs/gaps-audit2.md),
[docs/gaps-client.md](docs/gaps-client.md) and [docs/gaps-protocol.md](docs/gaps-protocol.md).

## Performance

Rustcraft against the official 26.3 client, on the same machine (Ryzen 9 7950X, 32 GB, RTX 5080,
Windows 11), in a 1920×1080 window with the same `options.txt` (VSync off, frame rate unlimited).
Both games open the same new world (seed 1234) and fly 192 blocks at 16 blocks per second over
terrain generated on the way, in four places: a village, a forest, the ocean and mountains. Each
flight is run three times; the tables give the range of the four places.

| Render distance 16 | Official client | Rustcraft |
|---|---|---|
| Frames per second, average | 1,389 to 1,712 | **2,601 to 3,197** |
| Slowest 1 % of frames | 328 to 348 per second | **895 to 1,310 per second** |
| Longest frame | 10.8 to 15.1 ms | **2.4 to 3.4 ms** |
| CPU cores busy | 3.1 to 4.3 | **1.5 to 1.7** |
| Memory (working set) | 2,190 to 2,390 MB | **1,360 to 1,590 MB** |
| From start to in the world | 11.9 to 12.8 s | **0.9 to 1.2 s** |

| Render distance 32 | Official client | Rustcraft |
|---|---|---|
| Frames per second, average | 863 to 1,454 | **1,849 to 2,633** |
| Slowest 1 % of frames | 101 to 195 per second | **460 to 785 per second** |
| Longest frame | 95 to 258 ms | **3.7 to 9.9 ms** |
| CPU cores busy | 6.6 to 8.0 | **2.0 to 2.3** |
| Memory (working set) | 2,800 to 2,890 MB | **1,980 to 2,480 MB** |
| From start to in the world | 11.9 to 13.0 s | **0.8 to 3.1 s** |

Arriving in the world: the player stays where they joined and the game window is captured ten times a
second; the view is complete once the picture stops changing (interface, vignette and clouds left
out, the same way for both games).

| View complete, new world | Official client | Rustcraft |
|---|---|---|
| After joining, render distance 16 | 8.0 to 15.3 s | **0.9 to 1.4 s** |
| After joining, render distance 32 | 22.4 to 35.5 s | **2.6 to 4.3 s** |
| From start, render distance 16 | 20.0 to 27.2 s | **1.9 to 2.2 s** |
| From start, render distance 32 | 35.1 to 48.0 s | **4.8 to 6.3 s** |

Frame times come from [PresentMon](https://github.com/GameTechDev/PresentMon) for both games, memory
and CPU time from the process counters. The official client runs on the launcher's own Java, under
another file name, because the graphics driver's profile for `java.exe` holds it at the display's
refresh rate. The tool is [tools/bench/official_compare.py](tools/bench/official_compare.py). Each
optimisation is also measured on its own, before and after, in the [measurement log](docs/perf.md).

What makes the difference:

- **Everything in parallel**: generation, lighting, meshing and network decoding spread over every
  core, without waiting for each other.
- **A modern GPU pipeline**: one face buffer, merged indirect draws, visibility sorted by section,
  no allocation per frame in steady state.
- **Instant start**: models, textures, fonts and sounds are baked once into a binary cache.
- **Same results**: an optimisation is kept only if its result is identical to the old path, checked
  by a test that fails when the optimisation is sabotaged.

## Install

You need:

- **Minecraft: Java Edition** (the game bought on your Microsoft account) and the **official launcher**;
- **Windows 10 or 11** (64-bit) with a GPU that supports **Vulkan** or **DirectX 12**, or **Linux**
  (x86-64, glibc 2.35 or newer) with a **Vulkan** driver.

### 1. Prepare the official game

In the launcher, start the official **26.3** release **once**, then close it. The launcher downloads
the game's files; Rustcraft reads them on your machine.

### 2. Get Rustcraft

Download it from the [Releases](https://github.com/neotoxicfr/rustcraft/releases) page when a
release matches your version, or [build it](#build). Put it in a folder that stays put, for example
`%LOCALAPPDATA%\rustcraft\bin\rustcraft.exe` on Windows, or `~/.local/share/rustcraft/bin/` on Linux
(keep `rustcraft` and `rustcraft.sh` together there).

### 3. Add it to the launcher

1. In the launcher, open **Installations**, then **New installation**.
2. **Name**: `Rustcraft`. **Version**: `release 26.3`.
3. Open **More options**. Under **Java executable**, choose `rustcraft.exe` on Windows, or
   `rustcraft.sh` on Linux.
4. Click **Create**, then **Play**.

The launcher starts Rustcraft instead of Java and passes it your name, your id and your session
token, as it does for the official game; the Java options it adds are ignored. The very first start
prepares the version's data: it downloads the official server from Mojang's servers, checks its
fingerprint, and extracts the game's tables with the launcher's Java. That takes a few minutes, once.

Rustcraft shares the official game folder for the server list, `options.txt` and resource packs. On
Linux, worlds and the cache go to `~/.local/share/rustcraft` and `~/.cache/rustcraft`.

## Build

You need Rust (the version is pinned in `rust-toolchain.toml` and installed by `rustup`) and, on
Windows, the Visual Studio C++ build tools.

```
git clone https://github.com/neotoxicfr/rustcraft.git
cd rustcraft
cargo build --release -p rc-app
```

The program is `target/release/rustcraft.exe` (`rustcraft` on Linux, where the build also needs the
ALSA and udev development packages). The Linux launch script is in [packaging/linux](packaging/linux).

## Repository layout

| Crate | What it owns |
|---|---|
| `rc-app` ([`apps/client`](apps/client)) | the client: window, rendering, interface, game loop |
| `rc-sim`, `rc-light` | the integrated server's simulation, lighting |
| `rc-worldgen` | world generation |
| `rc-net`, `rc-proto`, `rc-proto-v777`, `rc-client` | networking, encryption, protocol 777 |
| `rc-world`, `rc-mesh`, `rc-physics`, `rc-env` | world data, meshing, physics, environment |
| `rc-assets`, `rc-data` | resources and game data read from the player's own installation |
| `rc-text`, `rc-audio`, `rc-commands` | text and translations, sound, command tree |

The documentation is in [`docs/`](docs): [architecture](docs/architecture.md), the
[measurement log](docs/perf.md), the [optimisation protocol](docs/optimization-protocol.md), and
59 specifications of the game's behaviour written in our own words in [`docs/specs/`](docs/specs).

## Legal

**NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR MICROSOFT.**

- "Minecraft" is a trademark of Mojang Synergies AB and Microsoft; the name is only used to say
  which game Rustcraft works with.
- **You must own Minecraft: Java Edition.** Rustcraft only works with an official installation and
  a genuine account.
- **This repository contains and distributes nothing from Mojang**: no code, textures, sounds,
  models or game data. Everything is read on your machine from your official installation; the
  official server is downloaded by your machine from Mojang's servers, as the launcher does, and its
  fingerprint is checked.
- No line of Mojang's code is in this repository. The game's behaviour is described in our own words
  in [`docs/specs/`](docs/specs), then implemented from those descriptions.
- Your use of the game stays subject to the [Minecraft EULA](https://www.minecraft.net/eula) and the
  [usage guidelines](https://www.minecraft.net/usage-guidelines).
- **Your account**: Rustcraft never asks for your password. It receives a session token from the
  official launcher, keeps it in memory only, never writes it to a log, and sends it only to Mojang's
  session services.
- **Servers**: Rustcraft presents itself to servers as the official client (`vanilla`) so it is
  treated the same way. It is still a third-party client: some servers forbid them, respect their rules.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) first,
especially the clean-room rule: nothing from Mojang enters the repository. To report a
vulnerability, follow [SECURITY.md](SECURITY.md).

## Licence

Rustcraft's code is dual-licensed, at your choice, under [Apache 2.0](LICENSE-APACHE) or
[MIT](LICENSE-MIT). Unless stated otherwise, any contribution submitted for inclusion is licensed
under both, without additional terms. The licence covers this repository's code only, not the game
Minecraft or its resources, which remain Mojang's property.
