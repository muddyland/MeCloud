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
install -m 0644 desktop/file-manager/mecloud_extension.py "$APP/mecloud_extension.py"

cat > "$APP/install.sh" <<'INNER'
#!/bin/sh
# Install the MeCloud desktop client for the current user. No root needed.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
ICON_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/128x128/apps"

# ── Check the runtime libraries before installing anything ──────────────────
#
# Tauri renders in the system's WebKitGTK rather than bundling a browser, which
# is why this binary is 4.6 MB — but it means three libraries have to be
# present. Without this check the first symptom is the dynamic linker's
# "cannot open shared object file", which tells the user nothing about what to
# install.
missing=""
if command -v ldd >/dev/null 2>&1; then
    missing="$(ldd "$HERE/mecloud-desktop" 2>/dev/null | awk '/not found/ {print $1}' | sort -u)"
fi

if [ -n "$missing" ]; then
    echo "MeCloud needs some system libraries that are not installed:" >&2
    echo "$missing" | sed 's/^/  /' >&2
    echo >&2

    # Name the packages for the distribution actually in front of us.
    distro=""
    [ -r /etc/os-release ] && . /etc/os-release && distro="${ID:-} ${ID_LIKE:-}"
    case "$distro" in
      *debian*|*ubuntu*)
        echo "  sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0 libayatana-appindicator3-1" >&2 ;;
      *fedora*|*rhel*)
        echo "  sudo dnf install webkit2gtk4.1 gtk3 libappindicator-gtk3" >&2 ;;
      *arch*)
        echo "  sudo pacman -S webkit2gtk-4.1 gtk3 libappindicator-gtk3" >&2 ;;
      *suse*)
        echo "  sudo zypper install libwebkit2gtk-4_1-0 gtk3 libayatana-appindicator3-1" >&2 ;;
      *)
        echo "  Install WebKitGTK 4.1, GTK 3 and libayatana-appindicator3 with your" >&2
        echo "  package manager." >&2 ;;
    esac

    case "$missing" in
      *webkit2gtk-4.1*)
        echo >&2
        echo "If your distribution only offers webkit2gtk 4.0, it is too old for this" >&2
        echo "build. 4.1 is present from Debian 12, Ubuntu 22.04 and Fedora 36 onward." >&2 ;;
    esac
    echo >&2
    echo "Nothing was installed. Run this script again afterwards." >&2
    exit 1
fi

mkdir -p "$BIN_DIR" "$ICON_DIR"
install -m 0755 "$HERE/mecloud-desktop" "$BIN_DIR/mecloud-desktop"
install -m 0644 "$HERE/mecloud-desktop.png" "$ICON_DIR/mecloud-desktop.png"

echo "Installed to $BIN_DIR/mecloud-desktop"

# ── File manager integration ────────────────────────────────────────────────
#
# Installed only where the corresponding <manager>-python bridge is already
# present. Creating the directory for a file manager that is not installed
# would leave dead files behind for no benefit.
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
installed_for=""
for fm in nautilus nemo caja; do
    if [ -d "$DATA_DIR/$fm-python" ] \
       || [ -d "/usr/share/$fm-python" ] \
       || [ -d "/usr/lib/$fm/extensions-3.0" ] \
       || python3 -c "import gi; gi.require_version('$(printf '%s' "$fm" | sed 's/^./\U&/')', '3.0')" 2>/dev/null; then
        target="$DATA_DIR/$fm-python/extensions"
        mkdir -p "$target"
        install -m 0644 "$HERE/mecloud_extension.py" "$target/mecloud_extension.py"
        installed_for="$installed_for $fm"
    fi
done

if [ -n "$installed_for" ]; then
    echo "File manager integration installed for:$installed_for"
    echo "  Restart the file manager to pick it up (e.g. nautilus -q)."
else
    echo "No supported file manager found — skipping the badge integration."
    echo "  Install python3-nautilus (or the nemo/caja equivalent) and re-run"
    echo "  this script to add sync badges and the right-click menu."
fi
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

# ── Manifest ────────────────────────────────────────────────────────────────
# What the server needs to answer "is there an update, and did I get the file
# intact?" without unpacking anything. The digests are what the client checks
# before it replaces its own binary.
{
  printf '{\n  "version": "%s",\n  "artifacts": {\n' "$VERSION"
  first=1
  for f in "$OUT"/mecloud-desktop-*.tar.gz "$OUT"/mecloud-desktop-*.zip; do
    [ -f "$f" ] || continue
    [ "$first" = 1 ] || printf ',\n'
    first=0
    printf '    "%s": { "sha256": "%s", "size": %s }' \
      "$(basename "$f")" \
      "$(sha256sum "$f" | cut -d' ' -f1)" \
      "$(wc -c < "$f" | tr -d ' ')"
  done
  printf '\n  }\n}\n'
} > "$OUT/manifest.json"

ls -l "$OUT"
