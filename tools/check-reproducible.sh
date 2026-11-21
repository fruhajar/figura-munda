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

# The claim is that the same source in two different directories gives the same
# binary, so the source directory is what has to differ. Varying only
# CARGO_TARGET_DIR, which is what this did, left the thing the README cites it
# for unchecked: the build path leaks in through build scripts that embed their
# own OUT_DIR, and OUT_DIR lives under the target directory, so that half was
# real. The source half was not being tested at all.
copy() {
    dest="$1"
    mkdir -p "$dest"
    tar -c --exclude=target --exclude=.git --exclude=out -C "$root" . | tar -x -C "$dest"
}

build() {
    src="$1"
    out="$src/target"
    CARGO_TARGET_DIR="$out" \
    RUSTFLAGS="--remap-path-prefix=$out=/target --remap-path-prefix=$src=/build --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}/registry=/registry" \
        cargo build --release -q -p fm-cli --manifest-path "$src/Cargo.toml"
    sha256sum "$out/release/fm-cli" | cut -d' ' -f1
}

printf 'copying the source to two paths of different lengths\n'
short="$work/a"
long="$work/bb-deliberately-longer-source-path"
copy "$short"
copy "$long"

printf 'building in each\n'
one=$(build "$short")
two=$(build "$long")

printf '  %s\n  %s\n' "$one" "$two"
if [ "$one" = "$two" ]; then
    printf 'reproducible: identical binaries from different build paths\n'
else
    printf 'NOT reproducible: the build path still leaks into the output\n'
    exit 1
fi
