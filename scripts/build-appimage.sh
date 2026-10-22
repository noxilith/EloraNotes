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
# DT_RUNPATH only covers libraries the executable itself opens; the copy in usr/lib is put on
# the loader's search path so the libraries opened at run time are found too.
LD_LIBRARY_PATH="$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export LD_LIBRARY_PATH
exec "$HERE/usr/bin/elora" "$@"
EOF
chmod +x "$appdir/AppRun"

echo "collecting shared libraries"
ARCH=x86_64 "$linuxdeploy" --appimage-extract-and-run \
  --appdir "$appdir" \
  --executable "$appdir/usr/bin/elora" \
  --desktop-file "$appdir/$app_id.desktop" \
  --icon-file "$appdir/$app_id.png"

# The window backend opens the Wayland, X11 and keyboard libraries with dlopen instead of
# linking against them, so linuxdeploy cannot see them and the image only works on hosts that
# happen to have them in the loader's search path. Distributions such as NixOS do not, and the
# app then exits with "NoWaylandLib" before a window ever appears. Copy them in, together with
# whatever they need in turn.
ldconfig_cache=$(ldconfig -p 2>/dev/null || true)
find_lib() {
  local name=$1 dir hit
  for dir in /usr/lib/x86_64-linux-gnu /lib/x86_64-linux-gnu /usr/lib64 /usr/lib /lib64 /lib; do
    if [[ -e $dir/$name ]]; then printf '%s\n' "$dir/$name"; return 0; fi
  done
  hit=$(awk -v name="$name" '$1 == name { print $NF; exit }' <<<"$ldconfig_cache")
  if [[ -n $hit ]]; then printf '%s\n' "$hit"; return 0; fi
  return 1
}
copy_lib() {
  local path=$1 name dep dep_path
  name=$(basename "$path")
  if [[ -e $appdir/usr/lib/$name ]]; then return 0; fi
  cp -L "$path" "$appdir/usr/lib/$name"
  while read -r dep; do
    # The glibc core and the C++ runtime have to match the host.
    case $dep in
      libc.so.* | libm.so.* | libdl.so.* | libpthread.so.* | librt.so.* | ld-linux*) continue ;;
      libgcc_s.so.* | libstdc++.so.*) continue ;;
    esac
    if dep_path=$(find_lib "$dep"); then
      copy_lib "$dep_path"
    else
      echo "warning: $dep (needed by $name) not found; the host has to provide it" >&2
    fi
  done < <(readelf -d "$path" | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p')
}
mkdir -p "$appdir/usr/lib"
for lib in libwayland-client.so.0 libwayland-cursor.so.0 libwayland-egl.so.1 \
  libxkbcommon.so.0 libxkbcommon-x11.so.0 \
  libX11.so.6 libX11-xcb.so.1 libXcursor.so.1 libXrandr.so.2 libXi.so.6 libXext.so.6; do
  if path=$(find_lib "$lib"); then
    copy_lib "$path"
  else
    echo "warning: $lib not found; the host has to provide it" >&2
  fi
done
mkdir -p "$outdir"
# appimagetool changes into the unpacked AppImage while it builds, so the output has to be
# spelled out as an absolute path.
outdir=$(cd "$outdir" && pwd)
echo "packing $appimage"
ARCH=x86_64 "$appimagetool" --appimage-extract-and-run \
  --no-appstream \
  "$appdir" "$outdir/$appimage"

echo "$outdir/$appimage"
