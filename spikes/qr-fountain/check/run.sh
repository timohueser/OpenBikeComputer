#!/usr/bin/env bash
# Checks that the Swift receiver decodes what the JS sender encodes, with frames missed.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
swiftc -O "$here/../ios/QRFountain/Fountain.swift" "$here/main.swift" -o "$work/check"
for args in "1 600 0" "600 600 0" "120000 600 0" "120000 600 0.3" "120000 601 0.6" "40000 50 0.5"; do
  set -- $args
  node "$here/frames.mjs" "$work" "$1" "$2" "$3"
  printf '%7s bytes, %4s-byte blocks, %3s loss: ' "$1" "$2" "$3"
  "$work/check" "$work"
done
