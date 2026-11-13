#!/usr/bin/env bash
# Container setup of epic #3227: the tools of handover/README.md.
set -uo pipefail
cd /home/user
step() { echo "=== $(date +%T) $*"; }
step geos
if [[ ! -x ~/geos-install/bin/geos-config ]]; then
  tmp=$(mktemp -d)
  curl -fL --retry 3 -o "$tmp/geos.tar.bz2" https://github.com/libgeos/geos/releases/download/3.14.1/geos-3.14.1.tar.bz2 &&
  tar -xjf "$tmp/geos.tar.bz2" -C "$tmp" &&
  cmake -S "$tmp/geos-3.14.1" -B "$tmp/build" -DCMAKE_INSTALL_PREFIX="$HOME/geos-install" -DBUILD_TESTING=OFF -DCMAKE_BUILD_TYPE=Release >/dev/null &&
  cmake --build "$tmp/build" -j 4 >/dev/null && cmake --install "$tmp/build" >/dev/null
  rm -rf "$tmp"
fi
bash /home/user/OpenBikeComputer/tools/install-geos.sh
step osmium
(apt-get update -qq; apt-get install -y -qq osmium-tool) 2>&1 | tail -3
osmium --version | head -1
step just;    cargo install just --locked 2>&1 | tail -1
step nextest; cargo install cargo-nextest --locked 2>&1 | tail -1
step deny;    cargo install cargo-deny@0.20.2 --locked 2>&1 | tail -1
step about;   cargo install cargo-about@0.9.1 --locked --features cli 2>&1 | tail -1
step done
