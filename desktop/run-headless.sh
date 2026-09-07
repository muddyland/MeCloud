#!/bin/bash
# Run the client on a virtual display and screenshot it.
# For verifying the GUI in a container with no desktop of its own.
set -u
export DISPLAY=:99

# Trust the demo CA, if one was mounted, so the embedded webview can load a
# server whose certificate is self-signed for the test environment.
if [ -f /certs/ca.pem ] && [ "$(id -u)" = "0" ]; then
  cp /certs/ca.pem /usr/local/share/ca-certificates/demo-ca.crt
  update-ca-certificates >/dev/null 2>&1
fi

Xvfb :99 -screen 0 1280x900x24 >/tmp/xvfb.log 2>&1 &
sleep 3
openbox >/tmp/openbox.log 2>&1 &
sleep 2

# A session bus, so the tray icon has somewhere to register.
if command -v dbus-launch >/dev/null 2>&1; then
  eval "$(dbus-launch --sh-syntax)"
fi

export GDK_BACKEND=x11
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1

"$@" >/tmp/app.log 2>&1 &
APP=$!
sleep "${SHOT_DELAY:-8}"

# Optional scripted interaction, e.g. typing an address and pressing Connect.
if [ -n "${SHOT_SCRIPT:-}" ]; then
  eval "$SHOT_SCRIPT"
fi

import -window root /shots/"${SHOT_NAME:-app}".png 2>/dev/null \
  || xwd -root -silent | convert xwd:- /shots/"${SHOT_NAME:-app}".png
echo "--- app log ---"; tail -25 /tmp/app.log
kill $APP 2>/dev/null
