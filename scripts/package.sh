#!/bin/bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
version="${1:-}"
[[ "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || { printf 'Invalid release version.\n' >&2; exit 1; }
[[ "$(dist/devports --version)" == "$version" ]] || { printf 'Binary version does not match release version.\n' >&2; exit 1; }
[[ "$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml)" == "$version" ]] || { printf 'Cargo version does not match release version.\n' >&2; exit 1; }
file -b dist/devports | grep -q 'Mach-O 64-bit executable arm64' || { printf 'Expected ARM macOS binary.\n' >&2; exit 1; }
stage=$(mktemp -d "${TMPDIR:-/tmp}/devports-package.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT
cp dist/devports LICENSE "$stage/"
mkdir -p release
asset="devports-v$version-aarch64-apple-darwin.tar.gz"
COPYFILE_DISABLE=1 tar -czf "release/$asset" -C "$stage" devports LICENSE
(cd release && shasum -a 256 "$asset" > SHA256SUMS)
