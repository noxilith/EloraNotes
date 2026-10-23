#!/usr/bin/env python3
"""Build every icon file EloraNotes ships from one source image.

    scripts/make-icons.py [source]

`source` defaults to `assets/logo-tile.svg`, then `assets/logo.png`. The image is padded to a
square and resized with ImageMagick, then written to `assets/icons/`:

    elora-notes-16.png … elora-notes-1024.png   Linux/Flatpak/AppImage
    elora-notes.ico                             Windows executable + setup wizard
    elora-notes.icns                            macOS app bundle
    window-icon-256.rgba                        raw pixels embedded in the window itself

The raw file is what the running window hands to the window manager, so the taskbar and
titlebar show the same mark without pulling an image decoder into the app.
"""

from __future__ import annotations

import argparse
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "icons"
SIZES = (16, 24, 32, 48, 64, 128, 256, 512, 1024)

# ICNS chunk types that accept PNG data, smallest first.
ICNS_TYPES = {
    16: b"icp4",
    32: b"icp5",
    64: b"icp6",
    128: b"ic07",
    256: b"ic08",
    512: b"ic09",
    1024: b"ic10",
}


def run(*args: str) -> bytes:
    result = subprocess.run(args, check=False, capture_output=True)
    if result.returncode != 0:
        raise SystemExit(
            f"{' '.join(args)} failed:\n{result.stderr.decode(errors='replace')}"
        )
    return result.stdout


def find_source(explicit: str | None) -> Path:
    if explicit:
        path = Path(explicit)
        if not path.is_file():
            raise SystemExit(f"no such file: {path}")
        return path
    for candidate in ("assets/logo-tile.svg", "assets/logo.png"):
        path = ROOT / candidate
        if path.is_file():
            return path
    raise SystemExit(
        "no logo found: put the artwork at assets/logo.svg or assets/logo.png"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", nargs="?", help="logo image (svg or raster)")
    parser.add_argument(
        "--size",
        type=int,
        default=1024,
        help="edge length of the square canvas before downscaling (default 1024)",
    )
    args = parser.parse_args()

    source = find_source(args.source)
    OUT.mkdir(parents=True, exist_ok=True)

    # Pad to a square canvas so wide or tall artwork keeps its proportions, then use that
    # single square as the base for every size.
    geometry = f"{args.size}x{args.size}"
    base = OUT / ".icon-square.png"
    # `-background` has to come before the input: it is the colour the SVG is rasterised onto,
    # and once the file has been read the settings are no longer applied to it. Given afterwards,
    # the transparent areas around the tile were painted opaque white, which is what a taskbar
    # then drew as a white square around the icon.
    run(
        "magick",
        "-background",
        "none",
        str(source),
        "-gravity",
        "center",
        "-resize",
        f"{geometry}>",
        "-extent",
        geometry,
        str(base),
    )

    written = []
    for size in SIZES:
        target = OUT / f"elora-notes-{size}.png"
        run("magick", str(base), "-filter", "Lanczos", "-resize", f"{size}x{size}", str(target))
        written.append(target)

    # ImageMagick writes a multi-resolution .ico from several PNG inputs.
    run(
        "magick",
        *[str(OUT / f"elora-notes-{size}.png") for size in (16, 32, 48, 64, 128, 256)],
        str(OUT / "elora-notes.ico"),
    )

    chunks = b""
    for size, kind in ICNS_TYPES.items():
        data = (OUT / f"elora-notes-{size}.png").read_bytes()
        chunks += kind + struct.pack(">I", len(data) + 8) + data
    body = b"icns" + struct.pack(">I", len(chunks) + 8) + chunks
    (OUT / "elora-notes.icns").write_bytes(body)

    # Raw RGBA for `egui::IconData`; the window code only needs the bytes and the edge length.
    raw = run("magick", str(OUT / "elora-notes-256.png"), "-depth", "8", "rgba:-")
    expected = 256 * 256 * 4
    if len(raw) != expected:
        raise SystemExit(f"expected {expected} bytes of RGBA, got {len(raw)}")
    (OUT / "window-icon-256.rgba").write_bytes(raw)

    base.unlink()
    print(f"wrote {len(written)} PNGs, elora-notes.ico, elora-notes.icns and window-icon-256.rgba")
    print(f"  from {source.relative_to(ROOT) if source.is_relative_to(ROOT) else source}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
