#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Add Nix store Rust toolchain and graphical runtime libraries when running on NixOS without a dev shell
for p in /nix/store/*-cargo-1.*/bin /nix/store/*-rustc-wrapper-1.*/bin /nix/store/*-pkg-config-wrapper-*/bin; do
    if [ -d "$p" ]; then
        export PATH="$p:$PATH"
    fi
done

LIB_DIRS=""
for lib in \
    /nix/store/*-wayland-1.*/lib \
    /nix/store/*-libxkbcommon-1.*/lib \
    /nix/store/*-libGL-*/lib \
    /nix/store/*-libglvnd-*/lib \
    /nix/store/*-vulkan-loader-*/lib \
    /nix/store/*-libX11-*/lib \
    /nix/store/*-libXcursor-*/lib \
    /nix/store/*-libXi-*/lib \
    /nix/store/*-libXrandr-*/lib; do
    if [ -d "$lib" ]; then
        LIB_DIRS="${LIB_DIRS:+$LIB_DIRS:}$lib"
    fi
done

if [ -n "$LIB_DIRS" ]; then
    export LD_LIBRARY_PATH="${LIB_DIRS}${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi

if [ ! -x "./target/debug/elora" ]; then
    cargo build
fi

exec ./target/debug/elora "$@"
