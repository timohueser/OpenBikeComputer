#!/usr/bin/env bash
# Regenerate THIRD-PARTY.md — the licence texts that have to travel with a distributed
# binary (#1149). Run it through `obc licenses`; `obc licenses --check` fails instead of
# writing, which is what CI runs.
#
# One section per *artifact*, because the obligation is per artifact: the crates inside
# UPDATE.BIN are not the crates inside the desktop installer. The host tools are absent on
# purpose — they are distributed as source, and a source tree carries the crates' own
# licence files already.
#
# The web bundle is NOT here. Its notices are generated from the emitted chunks at build
# time (builder/app/vite/third-party-licenses.ts) and ship beside it, because only the
# bundler knows which npm packages actually made it into the output.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/THIRD-PARTY.md"
TPL="$ROOT/tools/licenses/third-party.hbs"
CFG="$ROOT/about.toml"

check=0
[ "${1:-}" = "--check" ] && check=1

# Pinned, because --check diffs bytes: a generator that changed its own formatting would fail
# with nothing wrong in the tree. CI installs the same version (ci.yml, the `deny` job).
PINNED=0.9.1
have="$(cargo about --version 2>/dev/null | awk '{print $2}')" || true
if [ -z "$have" ]; then
    echo "error: cargo-about is not installed —" >&2
    echo "       cargo install cargo-about --locked --features cli --version $PINNED" >&2
    exit 1
fi
if [ "$have" != "$PINNED" ] && [ "${OBC_LICENSES_ANY_VERSION:-0}" != "1" ]; then
    echo "error: cargo-about $have is installed but THIRD-PARTY.md is pinned to $PINNED." >&2
    echo "       Install the pin, or set OBC_LICENSES_ANY_VERSION=1 to bump it deliberately" >&2
    echo "       (then update PINNED here and in ci.yml, and regenerate in the same commit)." >&2
    exit 1
fi

# artifact-title | manifest | what it is
ARTIFACTS=(
    "Device firmware (\`UPDATE.BIN\`)|firmware/obc-fw-nrf54l/Cargo.toml|the image the device runs, and the one served from updates.openbikecomputer.com"
    "Bootloader (\`obc-boot\`)|firmware/obc-boot/Cargo.toml|flashed once at manufacture; it installs the image above"
    "Desktop application|apps/obc-desktop/Cargo.toml|the Rust half of the desktop app — its web half ships its own notices beside the bundle"
)

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

{
    echo "# Third-party licences"
    echo
    echo "OpenBikeComputer is GPL-3.0 (see [\`LICENSE\`](LICENSE)). It is built on other people's"
    echo "work, and the permissive licences that work is under all ask the same thing in return:"
    echo "the copyright notice and the permission text must be handed over with the binary. This"
    echo "file is that hand-over, one section per distributed artifact."
    echo
    echo "**Generated — do not edit.** \`obc licenses\` rewrites it from the dependency graph;"
    echo "the \`deny\` CI job fails if it is out of date. Which licences are *allowed* in the tree"
    echo "is a separate question, answered by [\`deny.toml\`](deny.toml)."
    echo
    echo "Two obligations live outside this file, because their artifacts are built elsewhere:"
    echo
    echo "- **The map builder's web bundle** emits \`third-party-licenses.txt\` beside itself at"
    echo "  build time, generated from the modules the bundler actually included."
    echo "- **Map data** is © OpenStreetMap contributors, under the"
    echo "  [ODbL](https://www.openstreetmap.org/copyright); terrain is Copernicus GLO-30. Both"
    echo "  credits ship on the device (Settings ▸ System ▸ About) and in every published catalog."
    echo
    echo "GEOS deserves a line, because deny.toml's note about it predates the current tree:"
    echo "\`geos-src\` declares MIT for the *wrapper* while the C++ sources it carries are"
    echo "**LGPL-2.1**, and that obligation follows whatever links them. Today nothing"
    echo "distributed does — GEOS is reached only through \`obc-pack\`, a host tool shipped as"
    echo "source, and \`cargo tree -e normal\` finds no \`geos\` in the desktop app's graph, which"
    echo "is why no section below carries an LGPL text. Should a packaged artifact ever link it,"
    echo "the LGPL text and the corresponding source have to travel in that package."
    echo

    for entry in "${ARTIFACTS[@]}"; do
        IFS='|' read -r title manifest blurb <<<"$entry"
        echo "## $title"
        echo
        echo "_${blurb}._"
        echo
        cargo about generate --manifest-path "$ROOT/$manifest" -c "$CFG" "$TPL"
        echo
    done
} >"$tmp"

if [ "$check" = 1 ]; then
    if ! diff -u "$OUT" "$tmp"; then
        echo >&2
        echo "error: THIRD-PARTY.md is out of date — run 'obc licenses' and commit the result." >&2
        exit 1
    fi
    echo "THIRD-PARTY.md is current."
else
    mv "$tmp" "$OUT"
    trap - EXIT
    echo "wrote $OUT"
fi
