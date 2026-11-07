#!/bin/sh
# Build twice, in two different directories, and compare the binaries.
#
# The README claims reproducible builds so that a published binary can be
# checked against this source. Without the remapping below that claim is false:
# the build is deterministic, but the path it was built in leaks into the output
# through build scripts that embed their OUT_DIR, so the same source in two
# directories gives two different binaries.
#
# The remapping is therefore part of the release recipe and not an optimisation.
set -eu
cd "$(dirname "$0")/.."
root=$(pwd)
work=${TMPDIR:-/tmp}/fm-reproduce.$$
trap 'rm -rf "$work"' EXIT

build() {
    dir="$1"
    rm -rf "$dir"
    CARGO_TARGET_DIR="$dir" \
    RUSTFLAGS="--remap-path-prefix=$root=/build --remap-path-prefix=$dir=/target --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}/registry=/registry" \
        cargo build --release -q -p fm-cli
    sha256sum "$dir/release/fm-cli" | cut -d' ' -f1
}

printf 'building twice in two different paths\n'
one=$(build "$work/a")
two=$(build "$work/bb-deliberately-longer")

printf '  %s\n  %s\n' "$one" "$two"
if [ "$one" = "$two" ]; then
    printf 'reproducible: identical binaries from different build paths\n'
else
    printf 'NOT reproducible: the build path still leaks into the output\n'
    exit 1
fi
