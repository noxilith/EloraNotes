#!/usr/bin/env bash
# Build the portable Linux build: EloraNotes-<version>-linux-x86_64.AppImage
#
#   scripts/build-appimage.sh 0.7.0 [target/release/elora] [dist]
#
# linuxdeploy copies the shared libraries the binary needs into the AppDir (except the
# glibc core and the GPU driver, which must come from the host) and appimagetool packs it.
# Both tools are downloaded into .tools/ on first use; set LINUXDEPLOY / APPIMAGETOOL to use
# copies you already have. They are AppImages themselves, so they run through
# --appimage-extract-and-run and do not need FUSE (which CI containers lack).
set -euo pipefail

version=${1:?usage: build-appimage.sh <version> [binary] [outdir]}
binary=${2:-target/release/elora}
outdir=${3:-dist}
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

app_id=io.github.noxilith.EloraNotes
appimage="EloraNotes-$version-linux-x86_64.AppImage"

if [[ ! -x $binary ]]; then
  echo "missing release binary: $binary (run cargo build --release first)" >&2
  exit 1
fi
if [[ ! -f assets/icons/elora-notes-256.png ]]; then
  echo "missing icons: run scripts/make-icons.py first" >&2
  exit 1
fi

tools=${ELORA_TOOLS_DIR:-$root/.tools}
mkdir -p "$tools"
fetch() {
  local name=$1 url=$2
  if [[ ! -x $tools/$name ]]; then
    echo "fetching $name" >&2
    curl -fsSL --retry 3 -o "$tools/$name" "$url"
    chmod +x "$tools/$name"
  fi
  printf '%s\n' "$tools/$name"
}

linuxdeploy=${LINUXDEPLOY:-$(fetch linuxdeploy-x86_64.AppImage \
  https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage)}
appimagetool=${APPIMAGETOOL:-$(fetch appimagetool-x86_64.AppImage \
  https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage)}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir="$work/EloraNotes.AppDir"
mkdir -p "$appdir/usr/bin" \
  "$appdir/usr/share/applications" \
  "$appdir/usr/share/metainfo" \
  "$appdir/usr/share/icons/hicolor/256x256/apps"

install -m755 "$binary" "$appdir/usr/bin/elora"

# linuxdeploy wants the desktop file and icon at the AppDir root; the copies under
# usr/share are what a desktop actually reads once the image is unpacked.
install -m644 "packaging/linux/$app_id.desktop" "$appdir/$app_id.desktop"
install -m644 "packaging/linux/$app_id.desktop" "$appdir/usr/share/applications/$app_id.desktop"
install -m644 packaging/linux/elora-notes.metainfo.xml \
  "$appdir/usr/share/metainfo/$app_id.metainfo.xml"
install -m644 assets/icons/elora-notes-256.png "$appdir/$app_id.png"
install -m644 assets/icons/elora-notes-256.png \
  "$appdir/usr/share/icons/hicolor/256x256/apps/$app_id.png"

# AppRun is a tiny launcher; the "$@" forwarding keeps `elora --dir <path>` working when the
# AppImage is started from a shell.
cat > "$appdir/AppRun" <<'EOF'
#!/bin/sh
HERE=$(dirname "$(readlink -f "$0")")
exec "$HERE/usr/bin/elora" "$@"
EOF
chmod +x "$appdir/AppRun"

echo "collecting shared libraries"
ARCH=x86_64 "$linuxdeploy" --appimage-extract-and-run \
  --appdir "$appdir" \
  --executable "$appdir/usr/bin/elora" \
  --desktop-file "$appdir/$app_id.desktop" \
  --icon-file "$appdir/$app_id.png"

mkdir -p "$outdir"
# appimagetool changes into the unpacked AppImage while it builds, so the output has to be
# spelled out as an absolute path.
outdir=$(cd "$outdir" && pwd)
echo "packing $appimage"
ARCH=x86_64 "$appimagetool" --appimage-extract-and-run \
  --no-appstream \
  "$appdir" "$outdir/$appimage"

echo "$outdir/$appimage"
