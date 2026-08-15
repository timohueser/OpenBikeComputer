#!/usr/bin/env bash
# Build GEOS from source into ~/geos-install — no root — for obc-pack / obc-sim / the
# workspace build (geos-sys needs ≥3.14, newer than distro packages). Writes the shell
# shim ~/.obc-geos-env.sh that the `obc` tasks source. Idempotent. Needs cmake + a C++
# compiler + make. Called by `obc doctor --install`; safe to run standalone.
#
# NOT needed for the desktop app: obc-desktop builds its own GEOS in, statically
# (#907, its README). This script is for the workspace — the CLI packer, the sim, and
# CI's host jobs — where linking the system library is what keeps builds fast.
set -euo pipefail

VER="${GEOS_VER:-3.14.1}"
PREFIX="$HOME/geos-install"
ENVSH="$HOME/.obc-geos-env.sh"

write_env() {
  cat > "$ENVSH" <<EOF
# Source this to expose the locally-built GEOS $VER to obc-pack (geos-sys).
export PATH="\$HOME/geos-install/bin:\$PATH"
export PKG_CONFIG_PATH="\$HOME/geos-install/lib64/pkgconfig:\$HOME/geos-install/lib/pkgconfig:\${PKG_CONFIG_PATH:-}"
export LD_LIBRARY_PATH="\$HOME/geos-install/lib64:\$HOME/geos-install/lib:\${LD_LIBRARY_PATH:-}"
EOF
  echo "» wrote $ENVSH"
}

if [[ -x "$PREFIX/bin/geos-config" ]]; then
  echo "✓ GEOS already built at $PREFIX ($("$PREFIX/bin/geos-config" --version))"
  [[ -f "$ENVSH" ]] || write_env
  exit 0
fi

for t in cmake make cc; do
  command -v "$t" >/dev/null 2>&1 || { echo "✗ missing build tool: $t (install it first)" >&2; exit 1; }
done

tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
TARBALL="geos-${VER}.tar.bz2"
URL="https://download.osgeo.org/geos/${TARBALL}"
echo "» downloading $TARBALL …"
if command -v curl >/dev/null 2>&1; then curl -fL --retry 3 -o "$tmp/$TARBALL" "$URL"
elif command -v wget >/dev/null 2>&1; then wget -O "$tmp/$TARBALL" "$URL"
else echo "✗ need curl or wget" >&2; exit 1; fi

echo "» extracting + building (this takes a few minutes) …"
tar -xjf "$tmp/$TARBALL" -C "$tmp"
cmake -S "$tmp/geos-${VER}" -B "$tmp/build" -DCMAKE_INSTALL_PREFIX="$PREFIX" -DBUILD_TESTING=OFF -DCMAKE_BUILD_TYPE=Release
cmake --build "$tmp/build" -j "$(nproc 2>/dev/null || echo 4)"
cmake --install "$tmp/build"

[[ -x "$PREFIX/bin/geos-config" ]] || { echo "✗ geos-config missing after install" >&2; exit 1; }
write_env
echo "✓ GEOS ready: $("$PREFIX/bin/geos-config" --version)"
