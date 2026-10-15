#!/bin/bash
# Starts Rustclear on macOS or Linux: the server, then ViaProxy so 26.x clients can join.
# Fetches a portable Java 21 and ViaProxy on first run if they aren't there.
# Works from an install folder (RustClear next to this file) or from a source checkout
# (target/release/RustClear). Sticks to bash 3.2, which is what macOS ships.
#
# Env overrides: VIAPROXY_VERSION, BIND (default 127.0.0.1:25565; 0.0.0.0:25565 lets other
# machines join, and the server is offline mode, so anyone who reaches it can join under any name).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
via_version="${VIAPROXY_VERSION:-3.4.14}"
bind="${BIND:-127.0.0.1:25565}"
server_host=127.0.0.1
server_port=4972

port_open() { (exec 3<>"/dev/tcp/$1/$2") 2>/dev/null; }

find_server() {
  for p in "$here/RustClear" "$here/../target/release/RustClear"; do
    if [ -x "$p" ]; then echo "$p"; return; fi
  done
}

java_major() { "$1" -version 2>&1 | sed -n 's/.*version "\([0-9]*\).*/\1/p' | head -n1; }

find_java() {
  local portable
  portable="$(find "$here/java" -type f -path '*/bin/java' 2>/dev/null | head -n1 || true)"
  if [ -n "$portable" ]; then echo "$portable"; return; fi
  if [ "$(uname -s)" = Darwin ]; then
    # /usr/bin/java on a Mac without Java is a stub that pops up a dialog, so ask java_home instead.
    local home
    if home="$(/usr/libexec/java_home -v 21+ 2>/dev/null)"; then echo "$home/bin/java"; return; fi
  elif command -v java >/dev/null && [ "$(java_major java)" -ge 21 ] 2>/dev/null; then
    command -v java; return
  fi
  local os arch
  case "$(uname -s)" in Darwin) os=mac ;; *) os=linux ;; esac
  case "$(uname -m)" in arm64|aarch64) arch=aarch64 ;; *) arch=x64 ;; esac
  echo 'Downloading Java 21 (one time, about 45 MB)...' >&2
  mkdir -p "$here/java"
  curl -fsSL "https://api.adoptium.net/v3/binary/latest/21/ga/$os/$arch/jre/hotspot/normal/eclipse" | tar -xz -C "$here/java"
  find "$here/java" -type f -path '*/bin/java' | head -n1
}

find_viaproxy() {
  local jar="$here/proxy/ViaProxy-$via_version.jar"
  if [ ! -f "$jar" ]; then
    mkdir -p "$here/proxy"
    echo "Downloading ViaProxy $via_version (one time)..." >&2
    curl -fsSL -o "$jar.part" "https://github.com/ViaVersion/ViaProxy/releases/download/v$via_version/ViaProxy-$via_version.jar"
    mv "$jar.part" "$jar"
  fi
  echo "$jar"
}

server="$(find_server)"
if [ -z "$server" ]; then
  echo 'RustClear not found. Run install.sh first (see the README).' >&2
  exit 1
fi
java="$(find_java)"
jar="$(find_viaproxy)"

server_pid=""
proxy_pid=""
stop() {
  for pid in $proxy_pid $server_pid; do kill "$pid" 2>/dev/null || true; done
}
trap stop EXIT
trap 'exit 130' INT TERM HUP

if port_open "$server_host" "$server_port"; then
  echo "Something is already listening on port $server_port; using it as the server."
else
  echo 'Starting the Rustclear server (it needs about 8 GB of memory and up to a minute)...'
  (cd "$(dirname "$server")" && exec "$server") > "$here/server.log" 2>&1 &
  server_pid=$!
  deadline=$(( $(date +%s) + 240 ))
  until port_open "$server_host" "$server_port"; do
    if ! kill -0 "$server_pid" 2>/dev/null; then
      echo "The server closed before it was ready. The end of $here/server.log:" >&2
      tail -n 20 "$here/server.log" >&2
      exit 1
    fi
    if [ "$(date +%s)" -gt "$deadline" ]; then
      echo 'The server did not start listening within 4 minutes.' >&2
      exit 1
    fi
    sleep 2
  done
fi

echo 'Starting ViaProxy for 26.x clients...'
(cd "$here/proxy" && exec "$java" -jar "$jar" cli \
  --target-address "$server_host:$server_port" \
  --target-version 1.8.x \
  --bind-address "$bind" \
  --auth-method NONE \
  --chat-signing false) > "$here/proxy.log" 2>&1 &
proxy_pid=$!
join_port="${bind##*:}"
deadline=$(( $(date +%s) + 60 ))
until port_open 127.0.0.1 "$join_port"; do
  if ! kill -0 "$proxy_pid" 2>/dev/null || [ "$(date +%s)" -gt "$deadline" ]; then
    echo "ViaProxy did not start (see $here/proxy.log). 1.8.9 clients can still join the server directly." >&2
    break
  fi
  sleep 1
done

echo
echo 'Rustclear is running.'
echo "  Minecraft 26.1 to 26.3: join localhost:$join_port"
echo "  Minecraft 1.8.9:        join $server_host:$server_port"
echo "  Logs: $here/server.log and $here/proxy.log"
echo
read -r -p 'Press Enter here (or close this window) to stop Rustclear. ' _ || true
