#!/bin/sh
# Package the built Linux client, and archive the source for the platforms we
# cannot cross-compile to.
#
# Run from the repository root with the release binary already built.
# Usage: desktop/package-linux.sh <out-dir>
set -eu

OUT="${1:-/out}"
BIN="desktop/src-tauri/target/release/mecloud-desktop"
VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' desktop/src-tauri/Cargo.toml | head -1)"

[ -f "$BIN" ] || { echo "no binary at $BIN — build it first" >&2; exit 1; }
mkdir -p "$OUT"

# ── Linux tarball ───────────────────────────────────────────────────────────
# A tarball rather than a .deb or AppImage: it needs no packaging toolchain in
# the build image, installs per-user with no root, and works the same on any
# distribution that has the three runtime libraries.
STAGE="$(mktemp -d)"
APP="$STAGE/mecloud-desktop-$VERSION"
mkdir -p "$APP"

install -m 0755 "$BIN" "$APP/mecloud-desktop"
install -m 0644 desktop/src-tauri/icons/128x128.png "$APP/mecloud-desktop.png"
install -m 0644 desktop/README.md "$APP/README.md"

cat > "$APP/install.sh" <<'INNER'
#!/bin/sh
# Install the MeCloud desktop client for the current user. No root needed.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
ICON_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/128x128/apps"

mkdir -p "$BIN_DIR" "$ICON_DIR"
install -m 0755 "$HERE/mecloud-desktop" "$BIN_DIR/mecloud-desktop"
install -m 0644 "$HERE/mecloud-desktop.png" "$ICON_DIR/mecloud-desktop.png"

echo "Installed to $BIN_DIR/mecloud-desktop"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "Note: $BIN_DIR is not on your PATH." ;;
esac
echo "Run it once to connect to your server; it will register itself as the"
echo "mail handler if you leave that box ticked."
INNER
chmod 0755 "$APP/install.sh"

tar -czf "$OUT/mecloud-desktop-linux-x86_64.tar.gz" -C "$STAGE" "mecloud-desktop-$VERSION"
rm -rf "$STAGE"

# ── Source archive ──────────────────────────────────────────────────────────
# What a user needs to build the client for Windows or macOS themselves, and
# nothing else: no build outputs, no cargo cache.
# The archive root is `mecloud-desktop/` with src-tauri directly inside it, and
# no version in the directory name: the build instructions tell people to run
# `cd mecloud-desktop/src-tauri`, and that has to be true verbatim on a machine
# where globbing a version number is not an option. The version is in the file
# name instead.
SRC="$(mktemp -d)"
mkdir -p "$SRC/mecloud-desktop"
tar -cf - \
  --exclude='target' \
  --exclude='.cargo-cache' \
  --exclude='*.tar.gz' \
  --exclude='*.zip' \
  -C desktop . \
  | tar -xf - -C "$SRC/mecloud-desktop"

(cd "$SRC" && zip -qr "$OUT/mecloud-desktop-source.zip" "mecloud-desktop")
rm -rf "$SRC"

ls -l "$OUT"
