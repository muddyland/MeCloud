"""MeCloud integration for GTK file managers.

Draws a sync badge on files in the sync folder, and adds MeCloud entries to the
right-click menu. One file covers Nautilus (GNOME), Nemo (Cinnamon) and Caja
(MATE), because all three expose the same extension API through
`<manager>-python` — only the module name differs.

Deliberately dependency-free and defensive. This runs *inside the file
manager's* process: an exception here is the user's file manager misbehaving,
and a blocking call here is their file manager hanging. So every socket
operation has a short timeout, every failure degrades to "no badge", and
nothing is ever raised out of a callback.

Install to whichever of these exists:

    ~/.local/share/nautilus-python/extensions/
    ~/.local/share/nemo-python/extensions/
    ~/.local/share/caja-python/extensions/
"""

import os
import socket
import threading
from urllib.parse import unquote, urlparse

# ── Bind to whichever file manager is loading us ────────────────────────────
#
# The APIs are identical; only the namespace differs. Importing in this order
# and stopping at the first success is what makes one file serve all three.
_MANAGER = None
# The toolkit that goes with it, so the emblem names can be checked against
# the very icon theme this process will draw with. Nautilus 4.0 is a GTK4
# process; everything else here is GTK3, and asking for the wrong one raises.
_GTK = None
for _name in ("Nautilus", "Nemo", "Caja"):
    try:
        import gi

        gi.require_version(_name, "4.0" if _name == "Nautilus" else "3.0")
        _MANAGER = __import__("gi.repository", fromlist=[_name])
        _FM = getattr(_MANAGER, _name)
        _GTK = "4.0" if _name == "Nautilus" else "3.0"
        break
    except (ImportError, ValueError):
        try:
            import gi

            gi.require_version(_name, "3.0")
            _MANAGER = __import__("gi.repository", fromlist=[_name])
            _FM = getattr(_MANAGER, _name)
            _GTK = "3.0"
            break
        except (ImportError, ValueError):
            continue

from gi.repository import GObject  # noqa: E402

SOCKET_TIMEOUT = 0.5           # never block the file manager for longer
CACHE_SECONDS = 2.0            # the badge does not need to be to the second

# Emblems, in the order they are worth trying.
#
# The client's own first, because the theme can no longer be relied on for
# these. Adwaita now ships five legacy emblems — synchronizing, shared,
# readonly, symbolic-link, unreadable — and `emblem-default`, the green tick,
# is not among them. `add_emblem` with a name the theme does not have is not an
# error: nothing is drawn, which is exactly the bug this fixes. The same had
# happened to `emblem-important`, so the error badge was invisible too.
#
# The theme names stay as fallbacks for an extension installed without the
# icons, and for themes that do still carry them.
EMBLEMS = {
    "SYNCED": ("mecloud-synced", "emblem-default", "object-select-symbolic"),
    "SYNCING": ("mecloud-syncing", "emblem-synchronizing", "content-loading-symbolic"),
    "CLOUD": ("mecloud-cloud", "weather-overcast-symbolic", "network-server-symbolic"),
    "ERROR": ("mecloud-error", "emblem-important", "dialog-warning-symbolic"),
    # IGNORED and NOP get nothing: a badge saying "we are not involved" is
    # noise on every file the user has.
}

_RESOLVED = {}


def _icon_theme():
    """The icon theme this file manager draws with, or None.

    None is a perfectly good answer — a headless check, a display we cannot
    reach — and the caller falls back to the client's own emblem, which is the
    one the installer put there.
    """
    if _GTK is None:
        return None
    try:
        gi.require_version("Gtk", _GTK)
        from gi.repository import Gtk

        if _GTK == "3.0":
            return Gtk.IconTheme.get_default()

        from gi.repository import Gdk

        display = Gdk.Display.get_default()
        return Gtk.IconTheme.get_for_display(display) if display else None
    except Exception:
        # Inside someone else's process: an icon theme is never worth raising
        # over.
        return None


def _emblem_for(status):
    """The first emblem for this status the icon theme can actually draw.

    Cached: this is asked once per visible file, and the answer cannot change
    without the theme changing.
    """
    if status not in EMBLEMS:
        return None
    if status in _RESOLVED:
        return _RESOLVED[status]

    candidates = EMBLEMS[status]
    theme = _icon_theme()
    chosen = candidates[0]
    if theme is not None:
        for name in candidates:
            try:
                if theme.has_icon(name):
                    chosen = name
                    break
            except Exception:
                break
    _RESOLVED[status] = chosen
    return chosen


def _socket_path():
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if not runtime:
        runtime = "/tmp/mecloud-%d" % os.getuid()
    return os.path.join(runtime, "mecloud", "socket")


class _Client:
    """A very small client for the MeCloud socket.

    A fresh connection per exchange rather than a kept one: the client may be
    restarted, updated or not running at all, and a stale socket handle inside
    the file manager would need reconnection logic that is not worth the risk
    of getting wrong in someone else's process.
    """

    def __init__(self):
        self._lock = threading.Lock()
        self._cache = {}

    def ask(self, request):
        path = _socket_path()
        if not os.path.exists(path):
            return None
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
                sock.settimeout(SOCKET_TIMEOUT)
                sock.connect(path)
                sock.sendall((request + "\n").encode("utf-8"))
                data = sock.recv(4096)
            return data.decode("utf-8", "replace").strip() or None
        except (OSError, socket.timeout):
            # Client not running, or busy. No badge is the right answer.
            return None

    def status(self, filename):
        import time

        now = time.monotonic()
        with self._lock:
            cached = self._cache.get(filename)
            if cached and now - cached[0] < CACHE_SECONDS:
                return cached[1]

        reply = self.ask("RETRIEVE_FILE_STATUS:" + filename)
        status = None
        if reply and reply.startswith("STATUS:"):
            # STATUS:<state>:<path> — the path may contain colons, the state
            # may not, so split only twice.
            parts = reply.split(":", 2)
            if len(parts) == 3:
                status = parts[1]

        with self._lock:
            self._cache[filename] = (now, status)
            # Bounded, so browsing a huge tree cannot grow this without limit.
            if len(self._cache) > 4096:
                self._cache.clear()
        return status

    def tell(self, command, filename):
        # Fire and forget: the client opens a window, and the file manager has
        # nothing useful to do with the answer.
        threading.Thread(
            target=self.ask, args=("%s:%s" % (command, filename),), daemon=True
        ).start()


_CLIENT = _Client()


def _path_of(item):
    """The filesystem path of a file manager item, or None."""
    try:
        uri = item.get_uri()
    except Exception:
        return None
    parsed = urlparse(uri)
    if parsed.scheme != "file":
        return None            # a remote location is not ours to badge
    return unquote(parsed.path)


class MeCloudExtension(GObject.GObject, _FM.InfoProvider, _FM.MenuProvider):
    """Badges and menu entries."""

    def update_file_info(self, item):
        path = _path_of(item)
        if not path:
            return
        status = _CLIENT.status(path)
        emblem = _emblem_for(status)
        if emblem:
            item.add_emblem(emblem)

    def get_file_items(self, *args):
        # The signature changed between versions: older managers pass
        # (window, files), newer ones just (files). Taking *args and picking
        # the list out is what lets one file serve every version of all three.
        files = next((a for a in reversed(args) if isinstance(a, list)), [])
        if len(files) != 1:
            return []          # one file at a time keeps the actions unambiguous

        path = _path_of(files[0])
        if not path:
            return []
        status = _CLIENT.status(path)
        if status in (None, "NOP"):
            return []          # not in the sync folder, or the client is not running

        share = _FM.MenuItem(
            name="MeCloud::share",
            label="Share via email…",
            tip="Start a message with this file attached",
        )
        share.connect("activate", lambda _menu: _CLIENT.tell("SHARE", path))

        show = _FM.MenuItem(
            name="MeCloud::open",
            label="Show in MeCloud",
            tip="Open this file in MeCloud",
        )
        show.connect("activate", lambda _menu: _CLIENT.tell("OPEN", path))

        top = _FM.MenuItem(name="MeCloud::menu", label="MeCloud")
        submenu = _FM.Menu()
        submenu.append_item(share)
        submenu.append_item(show)
        top.set_submenu(submenu)
        return [top]

    def get_background_items(self, *args):
        return []
