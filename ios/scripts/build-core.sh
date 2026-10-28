#!/usr/bin/env bash
# Builds the Rust core for the iPhone and the simulator, writes the Swift side of its interface,
# and wraps both in an XCFramework the Xcode project links. Run on a Mac.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
core="$here/core"
build="$here/build"

rm -rf "$build" "$here/Frameworks" "$here/Generated"
mkdir -p "$build/headers" "$here/Frameworks" "$here/Generated"

cd "$core"
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo build --release --locked --target aarch64-apple-ios
cargo build --release --locked --target aarch64-apple-ios-sim

device="target/aarch64-apple-ios/release/libelora_mobile_core.a"
simulator="target/aarch64-apple-ios-sim/release/libelora_mobile_core.a"

# The interface is read out of the compiled library, so the Swift side always matches it.
cargo run --release --locked --bin uniffi-bindgen -- generate \
  --library "$device" --language swift --out-dir "$build/bindings"

cp "$build/bindings/elora_mobile_core.swift" "$here/Generated/"
cp "$build/bindings/elora_mobile_coreFFI.h" "$build/headers/"
cp "$build/bindings/elora_mobile_coreFFI.modulemap" "$build/headers/module.modulemap"

xcodebuild -create-xcframework \
  -library "$device" -headers "$build/headers" \
  -library "$simulator" -headers "$build/headers" \
  -output "$here/Frameworks/EloraCore.xcframework"

echo "Built $here/Frameworks/EloraCore.xcframework"
