#!/bin/sh
# Entry point of the Flatpak sandbox.
#
# /app is read-only, so the app cannot replace itself there. This launcher seeds a copy of the
# bundled binary in the writable data folder (granted by --filesystem=xdg-data/EloraNotes:create)
# and runs that instead: the sandbox then supplies only the runtime while the app updates itself
# exactly like a portable build. The bundled copy is only used when no copy is in place yet, and
# as a fallback when the data folder cannot be written to.
set -eu

data="${XDG_DATA_HOME:-$HOME/.local/share}/EloraNotes"
app="$data/elora"
bundled=/app/lib/eloranotes/elora

if [ ! -x "$app" ]; then
    if ! { mkdir -p "$data" &&
        cp "$bundled" "$app.tmp" &&
        chmod 755 "$app.tmp" &&
        mv -f "$app.tmp" "$app"; }; then
        # No writable data folder: run the read-only copy. Updates are offered but cannot be
        # installed by the app itself, which is what it reports in its update prompt.
        rm -f "$app.tmp"
        exec "$bundled" "$@"
    fi
fi

exec "$app" "$@"
