#!/usr/bin/env bash
# Install a user-local RISC-V (rv32emc) GNU toolchain for the board's FLPR blob — no root.
# Fetches an xPack riscv-none-elf-gcc release, extracts it under ~/.local/xPacks, and points
# obc.local at it via RISCV_GCC. Idempotent: skips if a toolchain is already visible.
# Called by `obc doctor --install`; safe to run standalone.
set -euo pipefail

VER="${RISCV_XPACK_VER:-14.2.0-3}"          # bump here if the pin ages out
TOOLS="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"   # this tools/ dir (holds obc.local)
DEST="$HOME/.local/xPacks"

if command -v riscv-none-elf-gcc riscv64-elf-gcc riscv64-unknown-elf-gcc >/dev/null 2>&1; then
  echo "✓ a RISC-V gcc is already on PATH — nothing to do."; exit 0
fi
if [[ -n "${RISCV_GCC:-}" && -x "${RISCV_GCC:-/nonexistent}" ]]; then
  echo "✓ RISCV_GCC already set to $RISCV_GCC"; exit 0
fi

case "$(uname -m)" in
  x86_64|amd64) ARCH=x64 ;;
  aarch64|arm64) ARCH=arm64 ;;
  *) echo "✗ unsupported arch $(uname -m) — grab a toolchain manually and set RISCV_GCC in obc.local" >&2; exit 1 ;;
esac
case "$(uname -s)" in
  Linux) OS=linux ;;
  Darwin) OS=darwin ;;
  *) echo "✗ unsupported OS $(uname -s)" >&2; exit 1 ;;
esac

TARBALL="xpack-riscv-none-elf-gcc-${VER}-${OS}-${ARCH}.tar.gz"
URL="https://github.com/xpack-dev-tools/riscv-none-elf-gcc-xpack/releases/download/v${VER}/${TARBALL}"
BIN="$DEST/xpack-riscv-none-elf-gcc-${VER}/bin/riscv-none-elf-gcc"

if [[ ! -x "$BIN" ]]; then
  mkdir -p "$DEST"
  tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
  echo "» downloading $TARBALL …"
  if command -v curl >/dev/null 2>&1; then curl -fL --retry 3 -o "$tmp/$TARBALL" "$URL"
  elif command -v wget >/dev/null 2>&1; then wget -O "$tmp/$TARBALL" "$URL"
  else echo "✗ need curl or wget" >&2; exit 1; fi
  echo "» extracting into $DEST …"
  tar -xzf "$tmp/$TARBALL" -C "$DEST"
fi
[[ -x "$BIN" ]] || { echo "✗ toolchain not found at $BIN after extract" >&2; exit 1; }

# Point tools/obc.local at it (create/append, don't duplicate).
LOCAL="$TOOLS/obc.local"
if ! { [[ -f "$LOCAL" ]] && grep -q '^RISCV_GCC=' "$LOCAL"; }; then
  printf 'RISCV_GCC="%s"\n' "$BIN" >> "$LOCAL"
  echo "» wrote RISCV_GCC to obc.local"
else
  echo "! obc.local already sets RISCV_GCC — leaving it; new toolchain at:"
  echo "    $BIN"
fi
echo "✓ RISC-V gcc ready: $("$BIN" --version | head -1)"
