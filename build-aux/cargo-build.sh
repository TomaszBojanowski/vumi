#!/bin/sh
# Builds the app with Cargo and copies the binary to where Meson expects it.
# Meson does not run its commands through a shell, so "cargo … && cp …" cannot
# be written in meson.build reliably; here, `set -e` stops at the first failure.
#
# Usage: cargo-build.sh <cargo> <Cargo.toml> <target dir> <locale dir> <data dir> <binary name> <output>
set -eu

cargo="$1"
manifest="$2"
target_dir="$3"
localedir="$4"
pkgdatadir="$5"
name="$6"
output="$7"

LOCALEDIR="$localedir" PKGDATADIR="$pkgdatadir" "$cargo" build --release \
    --manifest-path "$manifest" \
    --target-dir "$target_dir" \
    -p app
cp "$target_dir/release/$name" "$output"
