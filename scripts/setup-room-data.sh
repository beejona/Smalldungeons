#!/usr/bin/env bash
# Copies room data from Evensmallerdungeonsdata into src/room_data (gitignored), which the
# server embeds at build time. Usage: scripts/setup-room-data.sh [path-to-Evensmallerdungeonsdata]
# With no path, clones the data repo next to this one if it isn't there yet.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
data="${1:-$root/../Evensmallerdungeonsdata}"
if [ ! -d "$data/room_data" ]; then
  git clone --depth 1 "${DATA_REPO:-https://github.com/Moo-ody/Evensmallerdungeonsdata.git}" "$data"
fi
rm -rf "$root/src/room_data"
cp -r "$data/room_data" "$root/src/room_data"
echo "room data copied from $data"
