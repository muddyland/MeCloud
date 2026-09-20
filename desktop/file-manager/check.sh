#!/bin/sh
# Load the file manager extension the way a file manager does, and check it
# behaves.
#
# It runs inside a container with a *real* nautilus-python, because the things
# worth checking are exactly the ones a mock would paper over: that the module
# binds to the manager's API at all, that the emblem names it asks for are
# names the icon theme can actually draw, and that a client which is not
# running costs the file manager nothing.
#
# Usage: desktop/file-manager/check.sh
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"

docker run --rm -v "$HERE":/work:ro debian:trixie-slim sh -c '
set -eu
apt-get update -qq >/dev/null
# gir1.2-gtk-4.0 is not needed by the extension; the check uses it to open a
# display, which the file manager has already done by the time we are loaded.
apt-get install -y -qq python3-nautilus adwaita-icon-theme xvfb gir1.2-gtk-4.0 >/dev/null 2>&1

mkdir -p "$HOME/.local/share/icons/hicolor/scalable/emblems"
cp /work/icons/*.svg "$HOME/.local/share/icons/hicolor/scalable/emblems/"
gtk-update-icon-cache -q -t -f "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

cat > /check.py <<"PY"
import sys
sys.path.insert(0, "/work")

import gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk
Gtk.init()                       # the file manager did this long before us

import mecloud_extension as m

assert m._FM is not None, "bound to no file manager API"
assert m._GTK == "4.0", m._GTK

theme = m._icon_theme()
assert theme is not None, "no icon theme, with a display open"

# The bug this guards: an emblem the theme cannot draw is not an error. The
# file manager draws nothing, which is indistinguishable from a badge that has
# stopped working -- and is what `emblem-default` had quietly become.
for status in ("SYNCED", "SYNCING", "CLOUD", "ERROR"):
    name = m._emblem_for(status)
    assert theme.has_icon(name), "%s resolved to %s, which the theme cannot draw" % (status, name)
    print("%-8s -> %s" % (status, name))

# A badge meaning "we have no opinion" is noise on every file the user owns.
for status in ("IGNORED", "NOP", None):
    assert m._emblem_for(status) is None, status

class Item:
    """One file, and a file manager that fails the check if it is badged."""
    def __init__(self, uri):
        self.uri = uri
    def get_uri(self):
        return self.uri
    def add_emblem(self, name):
        raise AssertionError("badged %s with no client running" % self.uri)

# Nothing here is running a client, so nothing may be drawn and nothing may be
# raised: this code runs inside someone else is process.
m.MeCloudExtension().update_file_info(Item("file:///tmp/nothing.txt"))
m.MeCloudExtension().update_file_info(Item("sftp://elsewhere/notes.txt"))
assert m.MeCloudExtension().get_file_items(None, [Item("file:///tmp/nothing.txt")]) == []
print("no client running: no badge, no menu, no exception")
PY
xvfb-run -a python3 /check.py
'
echo "file manager extension: ok"
