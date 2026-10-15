#!/bin/bash
# Rustclear one-step installer for macOS (Apple Silicon and Intel) and Linux. In Terminal:
#   curl -fsSL https://raw.githubusercontent.com/beejona/Smalldungeons/main/install.sh | bash
#
# Installs to ~/Library/Application Support/Rustclear on macOS (~/.local/share/rustclear on Linux)
# and, on macOS, adds a double-clickable Rustclear.command to ~/Applications and the Desktop.
# If this repo has a published release with Rustclear-macos.tar.gz, it downloads that (no compilers
# needed). Otherwise it builds from source, installing the Xcode command line tools and Rust where
# they're missing. Running it again updates an existing install.
#
# Env overrides: RUSTCLEAR_DIR (install folder), RUSTCLEAR_FROM_SOURCE=1 (skip the release check),
# RUSTCLEAR_REPO (owner/name, default beejona/Smalldungeons), RUSTCLEAR_BRANCH (default main),
# RUSTCLEAR_DATA_REPO (git URL of the room data), RUSTCLEAR_NO_START=1 (don't start it afterwards).
set -euo pipefail

# Everything is in main so a download cut off halfway through can't run half a script.
main() {
  local os repo branch data_repo dir asset_url=""
  os="$(uname -s)"
  repo="${RUSTCLEAR_REPO:-beejona/Smalldungeons}"
  branch="${RUSTCLEAR_BRANCH:-main}"
  data_repo="${RUSTCLEAR_DATA_REPO:-https://github.com/Moo-ody/Evensmallerdungeonsdata.git}"
  if [ "$os" = Darwin ]; then
    dir="${RUSTCLEAR_DIR:-$HOME/Library/Application Support/Rustclear}"
  else
    dir="${RUSTCLEAR_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/rustclear}"
  fi
  mkdir -p "$dir"

  if [ "$os" = Darwin ] && [ -z "${RUSTCLEAR_FROM_SOURCE:-}" ]; then
    asset_url="$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" 2>/dev/null \
      | grep -o '"browser_download_url": *"[^"]*Rustclear-macos[^"]*\.tar\.gz"' \
      | head -n1 | sed 's/.*"\(https[^"]*\)"/\1/' || true)"
  fi

  if [ -n "$asset_url" ]; then
    echo "Downloading $asset_url..."
    curl -fsSL "$asset_url" | tar -xz -C "$dir"
  else
    echo 'No prebuilt release found, so building Rustclear from source.'
    install_build_tools "$os"
    sync_repo "https://github.com/$repo.git" "$branch" "$dir/source"
    sync_repo "$data_repo" main "$dir/data"
    rm -rf "$dir/source/src/room_data"
    cp -R "$dir/data/room_data" "$dir/source/src/room_data"
    echo 'Compiling (the first build takes several minutes)...'
    (cd "$dir/source" && cargo build --release)
    cp "$dir/source/target/release/RustClear" "$dir/"
    cp "$dir/source/launcher/start.sh" "$dir/"
  fi
  chmod +x "$dir/RustClear" "$dir/start.sh"

  if [ "$os" = Darwin ]; then
    mkdir -p "$HOME/Applications"
    for place in "$HOME/Applications" "$HOME/Desktop"; do
      [ -d "$place" ] || continue
      printf '#!/bin/bash\nexec "%s/start.sh"\n' "$dir" > "$place/Rustclear.command"
      chmod +x "$place/Rustclear.command"
    done
  fi

  echo
  echo "Rustclear is installed in $dir."
  if [ "$os" = Darwin ]; then
    echo 'Start it any time by double-clicking Rustclear.command on your Desktop or in ~/Applications.'
  else
    echo "Start it any time with: \"$dir/start.sh\""
  fi
  if [ -z "${RUSTCLEAR_NO_START:-}" ]; then
    # stdin is this script when piped from curl, so give the launcher the terminal instead.
    if [ -r /dev/tty ]; then "$dir/start.sh" < /dev/tty; else "$dir/start.sh"; fi
  fi
}

install_build_tools() {
  if [ "$1" = Darwin ] && ! xcode-select -p >/dev/null 2>&1; then
    echo 'Installing the Xcode command line tools. Click "Install" in the window that opens.'
    xcode-select --install 2>/dev/null || true
    until xcode-select -p >/dev/null 2>&1; do sleep 5; done
  fi
  for tool in git cc; do
    if ! command -v "$tool" >/dev/null; then
      echo "$tool is missing. Install it with your package manager (for example build-essential and git), then run this again." >&2
      exit 1
    fi
  done
  export PATH="$HOME/.cargo/bin:$PATH"
  if ! command -v cargo >/dev/null; then
    echo 'Installing Rust...'
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal
  fi
}

sync_repo() {
  local url="$1" branch="$2" path="$3"
  if [ -d "$path/.git" ]; then
    git -C "$path" fetch --depth 1 origin "$branch"
    git -C "$path" reset --hard FETCH_HEAD
  else
    git clone --depth 1 --branch "$branch" "$url" "$path"
  fi
}

main "$@"
