#!/usr/bin/env python3
"""Writes ios/App/Acknowledgements.txt: the open-source code the phone app is built from.

Run after the core's dependencies change (needs cargo). The list is read from the real
dependency tree for the iPhone, so it names what is in the app and nothing that is only used
to build it.
"""
import os
import subprocess
import sys

here = os.path.dirname(os.path.abspath(__file__))
core = os.path.join(here, "..", "core")
out = os.path.join(here, "..", "App", "Acknowledgements.txt")

tree = subprocess.run(
    [
        "cargo", "tree", "--locked", "--target", "aarch64-apple-ios", "-e", "normal",
        "--prefix", "none", "--format", "{p}|{l}|{r}",
    ],
    cwd=core, capture_output=True, text=True, check=True,
).stdout

found = {}
for line in tree.splitlines():
    parts = line.split("|")
    if len(parts) != 3:
        continue
    package, license_, repository = (p.strip() for p in parts)
    name, _, version = package.partition(" v")
    version = version.split(" ")[0]
    if name in ("elora-mobile-core", "elora-notes"):
        continue
    found[(name, version)] = (license_ or "see the project", repository)

lines = [
    "EloraNotes",
    "Copyright (C) noxilith. Released under the GNU General Public License, version 3 or later.",
    "Source code: https://github.com/noxilith/EloraNotes",
    "",
    "EloraNotes is built on the open-source software below. Each is used under the licence",
    "named beside it; the full text is in that project's repository.",
    "",
]
lines += [
    "Fonts and icons",
    "",
    "Ubuntu Light - Ubuntu Font Licence 1.0 - https://design.ubuntu.com/font",
    "Hack - MIT / Bitstream Vera licence - https://sourcefoundry.org/hack/",
    "DejaVu Sans Bold - Bitstream Vera / DejaVu licence - https://dejavu-fonts.github.io",
    "Tabler Icons - MIT - https://tabler.io/icons",
    "",
    "Rust libraries",
    "",
]
for (name, version), (license_, repository) in sorted(found.items()):
    lines.append(f"{name} {version}")
    lines.append(f"  {license_}")
    if repository:
        lines.append(f"  {repository}")
    lines.append("")
open(out, "w").write("\n".join(lines))
print(f"{len(found)} packages written to {out}")
