  # Small Dungeons
  Soplo clear sub 3
  flameofwar 2:42 dungeonbreaker
  

## Running it

1. `scripts/setup-room-data.sh` copies the room data from Evensmallerdungeonsdata into `src/room_data` (pass a path to an existing checkout, or it clones one).
2. `cargo run` starts the server on `127.0.0.1:4972`. It speaks 1.8.9, so a 1.8.9 client can join that address directly.
3. For newer clients (26.1.x to 26.3), also run `proxy/run-proxy.sh` (needs Java 21+). It downloads ViaProxy and listens on `localhost:25565`; join that. `BIND=0.0.0.0:25565` lets other machines join.

`python3 tools/smoke_26.py` logs a bare 26.1 client in through the proxy and checks it reaches play and gets chunks.

`python3 tools/breaker_test_18.py` checks the Dungeonbreaker against a running server: a break, the charge count in the lore, the block coming back after ~11 s, and the 21st hit in a burst being refused.
