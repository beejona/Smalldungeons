  # Small Dungeons
  Soplo clear sub 3
  flameofwar 2:42 dungeonbreaker
  

## Quick install (Windows)

Open PowerShell and paste:

```powershell
irm https://raw.githubusercontent.com/beejona/Smalldungeons/main/install.ps1 | iex
```

It installs everything into `%LOCALAPPDATA%\Rustclear`, adds a **Rustclear** shortcut to the desktop and Start menu, and starts it. Then join `localhost:25565` from Minecraft 26.1 to 26.3 (or `127.0.0.1:4972` from 1.8.9). Next time, just use the shortcut. Running the command again updates it. The server needs about 8 GB of free memory.

If the repo has a published release with `Rustclear-windows.zip`, the installer downloads that and needs nothing else. Otherwise it builds from source and installs Git, Rust and the C++ build tools with winget if they're missing (a one-time download of a few GB, and the first build takes several minutes). Java 21 and ViaProxy are fetched on first start if needed.

## Quick install (macOS and Linux)

Open Terminal and paste:

```bash
curl -fsSL https://raw.githubusercontent.com/beejona/Smalldungeons/main/install.sh | bash
```

On a Mac it installs into `~/Library/Application Support/Rustclear`, puts a **Rustclear.command** on the Desktop and in `~/Applications` (double-click it to start), and starts it. Join the same addresses as above. Works on Apple Silicon and Intel. If the repo has a published release with `Rustclear-macos.tar.gz` it downloads that; otherwise it installs the Xcode command line tools (click "Install" when asked) and Rust, then builds from source. On Linux it always builds from source into `~/.local/share/rustclear` and needs `git` and a C compiler already installed. Java 21 and ViaProxy are fetched on first start if needed.

## Prebuilt downloads

Run the **Release bundles** workflow from the Actions tab to build `Rustclear-windows.zip` and `Rustclear-macos.tar.gz`. Ticking "draft_release" attaches them to a draft release, which stays private until it's published; once published, both installers use it instead of building.

## Running it

1. `scripts/setup-room-data.sh` copies the room data from Evensmallerdungeonsdata into `src/room_data` (pass a path to an existing checkout, or it clones one).
2. `cargo run` starts the server on `127.0.0.1:4972`. It speaks 1.8.9, so a 1.8.9 client can join that address directly.
3. For newer clients (26.1.x to 26.3), also run `proxy/run-proxy.sh` (needs Java 21+). It downloads ViaProxy and listens on `localhost:25565`; join that. `BIND=0.0.0.0:25565` lets other machines join.

`python3 tools/smoke_26.py` logs a bare 26.1 client in through the proxy and checks it reaches play and gets chunks.

`python3 tools/breaker_test_18.py` checks the Dungeonbreaker against a running server: a break, the charge count in the lore, the block coming back after ~11 s, and the 21st hit in a burst being refused.

## Dungeon settings and the Rustclear mod

Players need the Rustclear mod (a separate Fabric mod) to stay on the server: on join the server sends a hello on `rustclear:hello`, and a client that doesn't answer on `rustclear:settings` within 10 s is kicked. Set `RUSTCLEAR_REQUIRE_MOD=0` to let plain clients in (needed for `tools/smoke_26.py` and `tools/breaker_test_18.py`).

The Windows installer sets up the server and proxy but not the mod, so players also need the mod jar in `.minecraft/mods` with Fabric Loader and Fabric API for 26.1.2. To play without it, set `RUSTCLEAR_REQUIRE_MOD=0` before starting the server.

The mod chooses settings for the next dungeon: rooms that must spawn, the puzzles that may spawn, and a range for the total secrets. Leaving all three on random gives a normal run, and only those count for the leaderboard (`Dungeon::counts_for_leaderboard`). `src/dungeon/settings.rs` has the message formats and `src/dungeon/generation.rs` the planner that picks a recorded layout and its rooms to meet the settings, or explains why none can.

`python3 tools/settings_test_18.py` checks the handshake against a running server: the kick without the mod, a custom dungeon, refused settings, and a ranked random dungeon.

## Leaderboard

A run counts when the blood door opens with S+ score (300 or more) on a fully random dungeon: no custom settings, no layout chosen on the command line, not practice mode, and no score-changing `/dscore`. The time is the server's own tick count from the dungeon start. Runs are kept in `leaderboard.json` in the server's working directory and ranked per party size, one entry per party. The mod asks for a board on the `rustclear:board` channel (see `src/server/leaderboard.rs` for the messages); `python3 tools/leaderboard_test_18.py` checks that protocol against a server started next to a copy of `tools/leaderboard_fixture.json` named `leaderboard.json`.
