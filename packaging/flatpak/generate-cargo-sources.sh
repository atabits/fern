#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

curl -fsSL \
  "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py" \
  -o "$TMP/flatpak-cargo-generator.py"

python3 "$TMP/flatpak-cargo-generator.py" ../../Cargo.lock -o cargo-sources.json
echo "Generated packaging/flatpak/cargo-sources.json"
