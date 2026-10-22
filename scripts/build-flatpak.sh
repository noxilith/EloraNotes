#!/usr/bin/env bash
# Build the Linux installer build: EloraNotes-<version>-linux-x86_64.flatpak
#
#   scripts/build-flatpak.sh 0.7.0 [target/release/elora] [dist]
#
# The bundle installs a launcher plus a read-only copy of the binary; the launcher runs the app
# from $XDG_DATA_HOME/EloraNotes, which the sandbox keeps writable, so the app can still replace
# itself when it updates.
#
# Requires flatpak-builder and the org.freedesktop Platform/Sdk for the runtime named in the
# manifest:
#   flatpak install --noninteractive flathub org.freedesktop.Platform//25.08 org.freedesktop.Sdk//25.08
set -euo pipefail

version=${1:?usage: build-flatpak.sh <version> [binary] [outdir]}
binary=${2:-target/release/elora}
outdir=${3:-dist}
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

app_id=io.github.noxilith.EloraNotes
manifest_src=packaging/flatpak/$app_id.yml

if [[ ! -x $binary ]]; then
  echo "missing release binary: $binary (run cargo build --release first)" >&2
  exit 1
fi
if [[ ! -f assets/icons/elora-notes-512.png ]]; then
  echo "missing icons: run scripts/make-icons.py first" >&2
  exit 1
fi
command -v flatpak-builder >/dev/null || { echo "flatpak-builder is not installed" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# flatpak-builder resolves `path:` sources relative to the manifest, so the manifest and the
# files it installs are staged together.
stage="$work/src"
mkdir -p "$stage"
cp "$manifest_src" "$stage/"
install -m755 "$binary" "$stage/elora"
# Read by the launcher to decide whether an installed copy is out of date.
printf '%s\n' "$version" > "$stage/version"
install -m755 packaging/flatpak/elora-launcher.sh "$stage/elora-launcher"
install -m644 packaging/linux/$app_id.desktop "$stage/$app_id.desktop"
# Software centres and `flatpak info` read the version from the newest AppStream <release>
# entry, and the repository only gets that entry once the release notes are written. Derive it
# here instead, so an installed bundle always reports the version it was built from.
today=$(date -u +%Y-%m-%d)
if grep -q "<release version=\"$version\"" packaging/linux/elora-notes.metainfo.xml; then
  install -m644 packaging/linux/elora-notes.metainfo.xml "$stage/elora-notes.metainfo.xml"
else
  awk -v version="$version" -v date="$today" '
    { print }
    !added && /<releases>/ {
      printf "    <release version=\"%s\" date=\"%s\">\n", version, date
      printf "      <description>\n        <p>See the release notes for %s on GitHub.</p>\n      </description>\n", version
      print "    </release>"
      added = 1
    }
  ' packaging/linux/elora-notes.metainfo.xml > "$stage/elora-notes.metainfo.xml"
fi
install -m644 assets/icons/elora-notes-256.png "$stage/$app_id-256.png"
install -m644 assets/icons/elora-notes-512.png "$stage/$app_id-512.png"

repo="$work/repo"
build="$work/build"
# --disable-rofiles-fuse keeps this working inside CI containers without /dev/fuse.
# flatpak-builder insists that its state directory lives on the same filesystem as the build
# directory, and /tmp is a separate one on CI runners.
flatpak-builder --force-clean --disable-rofiles-fuse \
  --state-dir="$work/state" --repo="$repo" "$build" "$stage/$app_id.yml"

mkdir -p "$outdir"
bundle="$outdir/EloraNotes-$version-linux-x86_64.flatpak"
flatpak build-bundle "$repo" "$bundle" "$app_id" \
  --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo \
  --arch=x86_64

echo "$bundle"
