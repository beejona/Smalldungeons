#!/usr/bin/env bash
# Lets 26.1.x-26.3 clients (and anything else Via supports) join the 1.8 server through ViaProxy.
# Start the server first (it listens on 127.0.0.1:4972), then this; join localhost:25565.
# BIND=0.0.0.0:25565 to accept other machines. The server is offline mode, so anyone who can
# reach that address can join under any name.
set -euo pipefail
dir="$(cd "$(dirname "$0")" && pwd)"
version="${VIAPROXY_VERSION:-3.4.14}"
jar="$dir/ViaProxy-$version.jar"
if [ ! -f "$jar" ]; then
  curl -fL -o "$jar.part" "https://github.com/ViaVersion/ViaProxy/releases/download/v$version/ViaProxy-$version.jar"
  mv "$jar.part" "$jar"
fi
cd "$dir"
exec java -jar "$jar" cli \
  --target-address "${TARGET:-127.0.0.1:4972}" \
  --target-version 1.8.x \
  --bind-address "${BIND:-127.0.0.1:25565}" \
  --auth-method NONE \
  --chat-signing false
