#!/usr/bin/env python3
"""Bakes Ko-fi's brand symbol into the two alpha masks the footer button tints.

`assets/kofi-symbol.svg` is Ko-fi's own symbol, as supplied by them. It is drawn with three
inks: a dark structure (`#202020` — the cup's outline and the handle loop), a white face, and the
orange heart (`#FF5A16`). The app paints the structure in the sidebar's icon colour and the heart
in the theme accent, and lets the white face show the panel through, so only the first and the
last of those need a mask. Each mask is written as one alpha byte per pixel (white ink), which
`src/app.rs` turns into a texture at startup.

Regenerating needs `rsvg-convert` and ImageMagick; building or running the app needs neither.
"""

import subprocess
import sys

# Four times the box the footer draws the mark in, so it stays sharp on a HiDPI screen.
WIDTH, HEIGHT = 96, 77
SOURCE = "assets/kofi-symbol.svg"
OUT = "assets/icons/"
# The two colours the symbol is drawn with, as its own SVG gives them.
STRUCTURE = 0x20
HEART = (255, 90, 22)


def render() -> bytes:
    """The symbol as straight RGBA, at the mask size."""
    png = "/tmp/kofi-mark-render.png"
    subprocess.run(
        ["rsvg-convert", "-w", str(WIDTH), "-h", str(HEIGHT), SOURCE, "-o", png],
        check=True,
    )
    return subprocess.run(
        ["magick", png, "-depth", "8", "rgba:-"], check=True, capture_output=True
    ).stdout


def masks(rgba: bytes):
    """Splits the render into (structure, heart) alpha, one byte per pixel each."""
    structure = bytearray(WIDTH * HEIGHT)
    heart = bytearray(WIDTH * HEIGHT)
    for index in range(WIDTH * HEIGHT):
        r, g, b, a = rgba[index * 4 : index * 4 + 4]
        if a == 0:
            continue
        # How much of this pixel the artwork covers, from the render's own alpha.
        covered = a / 255.0
        # Orange is the heart: its green is far below the white face's, and it is a long way
        # from grey. Anything neutral is the dark structure, whose coverage is how far its
        # brightness has come up from #202020 towards white.
        if r - b > 60:
            orange = (255 - g) / (255 - HEART[1])
            heart[index] = round(255 * min(1.0, max(0.0, orange)) * covered)
        else:
            dark = (255 - r) / (255 - STRUCTURE)
            structure[index] = round(255 * min(1.0, max(0.0, dark)) * covered)
    return structure, heart


def summarise(name: str, alpha: bytearray) -> None:
    """Prints what was baked, so a bad render is obvious without opening the file."""
    covered = [i for i, value in enumerate(alpha) if value > 8]
    if not covered:
        sys.exit(f"{name}: nothing was drawn")
    xs = [i % WIDTH for i in covered]
    ys = [i // WIDTH for i in covered]
    print(
        f"{name}: {len(covered)} pixels covered, "
        f"x {min(xs)}..{max(xs)}, y {min(ys)}..{max(ys)}"
    )


def main() -> None:
    structure, heart = masks(render())
    for name, alpha in (("kofi-cup", structure), ("kofi-heart", heart)):
        summarise(name, alpha)
        path = f"{OUT}{name}.alpha"
        with open(path, "wb") as file:
            file.write(alpha)
        print(f"wrote {path} ({WIDTH}x{HEIGHT} alpha)")


if __name__ == "__main__":
    main()
