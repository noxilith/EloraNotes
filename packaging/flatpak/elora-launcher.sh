#!/bin/sh
# Entry point of the Flatpak sandbox.
#
# /app is read-only, so the app cannot replace itself there. This launcher keeps a copy of the
# bundled binary in the writable data folder (granted by --filesystem=xdg-data/EloraNotes:create)
# and runs that instead: the sandbox then supplies only the runtime while the app updates itself
# exactly like a portable build. The bundled copy is used as-is when the data folder cannot be
# written to.
#
# Installing a newer bundle has to refresh that copy, otherwise the flatpak would keep starting
# the version it was first installed with. The bundle's version comes from a plain file next to
# the binary, and the copy records the bundle version it came from in a stamp beside it. A copy
# the app has updated for itself keeps its stamp, so installing the same (or an older) bundle
# never downgrades it.
set -eu

data="${XDG_DATA_HOME:-$HOME/.local/share}/EloraNotes"
app="$data/elora"
stamp="$data/.bundle-version"
bundled=/app/lib/eloranotes/elora
bundle_version=$(cat /app/lib/eloranotes/version 2>/dev/null || true)

# newer <a> <b> is true when version a is greater than version b.
newer() {
    [ "$1" != "$2" ] || return 1
    [ "$1" = "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" ]
}

refresh=0
if [ ! -x "$app" ]; then
    refresh=1
elif [ -n "$bundle_version" ]; then
    seeded=$(cat "$stamp" 2>/dev/null || true)
    # A copy without a stamp was put there before this launcher tracked versions.
    if [ -z "$seeded" ] || newer "$bundle_version" "$seeded"; then
        refresh=1
    fi
fi

if [ "$refresh" = 1 ]; then
    if { mkdir -p "$data" &&
        cp "$bundled" "$app.tmp" &&
        chmod 755 "$app.tmp" &&
        mv -f "$app.tmp" "$app"; }; then
        # Failing to record the version only costs another copy on the next start.
        if [ -n "$bundle_version" ]; then
            { printf '%s\n' "$bundle_version" > "$stamp.tmp" &&
                mv -f "$stamp.tmp" "$stamp"; } || rm -f "$stamp.tmp"
        fi
    else
        # No writable data folder: run the read-only copy. Updates are offered but cannot be
        # installed by the app itself, which is what it reports in its update prompt.
        rm -f "$app.tmp" "$stamp.tmp"
        exec "$bundled" "$@"
    fi
fi

exec "$app" "$@"
