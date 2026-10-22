#!/usr/bin/env bash
# Package the macOS build:
#
#   EloraNotes-<version>-macos-arm64.zip   portable: unzip anywhere and run the .app
#   EloraNotes-<version>-macos-arm64.dmg   installer: open, drag EloraNotes into Applications
#
#   scripts/build-macos-app.sh 0.7.0 [target/release/elora] [dist]
#
# The bundle is signed ad-hoc, which keeps macOS from calling it damaged but does not replace
# a Developer ID signature; without notarisation the first launch still needs
# right-click > Open (or System Settings > Privacy & Security > Open Anyway).
set -euo pipefail

version=${1:?usage: build-macos-app.sh <version> [binary] [outdir]}
binary=${2:-target/release/elora}
outdir=${3:-dist}
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

if [[ ! -x $binary ]]; then
  echo "missing release binary: $binary (run cargo build --release first)" >&2
  exit 1
fi
if [[ ! -f assets/icons/elora-notes.icns ]]; then
  echo "missing icons: run scripts/make-icons.py first" >&2
  exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

app="$work/EloraNotes.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m755 "$binary" "$app/Contents/MacOS/elora"
install -m644 assets/icons/elora-notes.icns "$app/Contents/Resources/EloraNotes.icns"
sed "s/__VERSION__/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"

# Ad-hoc signature: no certificate needed, and it covers the bundle so Gatekeeper reports a
# known unsigned developer instead of a broken app.
codesign --force --deep --sign - "$app" >/dev/null 2>&1 || echo "warning: ad-hoc codesign failed"

mkdir -p "$outdir"
portable="$outdir/EloraNotes-$version-macos-arm64.zip"
rm -f "$portable"
ditto -c -k --keepParent "$app" "$portable"

dmg_root="$work/dmg"
mkdir -p "$dmg_root"
cp -R "$app" "$dmg_root/EloraNotes.app"
ln -s /Applications "$dmg_root/Applications"
dmg="$outdir/EloraNotes-$version-macos-arm64.dmg"
rm -f "$dmg"
hdiutil create -quiet -volname "EloraNotes" -srcfolder "$dmg_root" -ov -format UDZO "$dmg"

echo "$portable"
echo "$dmg"
