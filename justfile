# OpenBikeComputer dev tasks — invoke with `obc <task>` (see `obc setup`), or `just <task>`.
# Personal defaults (map/gpx/preset/toolchain paths) live in a gitignored `obc.local`
# at the repo root — copy obc.local.example to obc.local and edit. `OBC_DRY_RUN=1 obc …`
# prints the underlying commands without running them.

set shell := ["bash", "-euo", "pipefail", "-c"]
set positional-arguments := true

export OBC_ROOT := justfile_directory()
lib := justfile_directory() / "scripts/obc-dev.sh"

# List the available tasks.
default:
    @just --justfile '{{justfile()}}' --list --unsorted

# ── Simulator ────────────────────────────────────────────────────────────────

# Run the simulator (always --physical). Args: [MAP.obcm] [TRACK.gpx] [-- extra sim flags].
# MAP defaults to $OBC_MAP or the newest .obcm in the repo root; TRACK to $OBC_GPX
# or the bundled grimsel-climb.gpx. Pass `none` as the track to run without a GPS.
# Run the simulator on a map + GPX, always at physical size. Args: [MAP] [TRACK.gpx] [-- sim flags]
sim *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source "{{lib}}"; obc_init; ensure_geos
    split_args "$@"
    mapf="$(resolve_map "${_POS[0]:-}")" || exit 1
    gpxf="$(resolve_gpx "${_POS[1]:-}")"
    cmd=(cargo run --release -p obc-sim -- "$mapf" --physical)
    [[ -n "$gpxf" && "$gpxf" != none ]] && cmd+=(--gpx "$gpxf")
    (( ${#_EXTRA[@]} )) && cmd+=("${_EXTRA[@]}")
    cd "$OBC_ROOT/firmware"; _run "${cmd[@]}"

# ── Board firmware ───────────────────────────────────────────────────────────

# Build & flash the board. Opts (space-separated): ble  debug-uart  synth  build.
# e.g.  obc flash            (real sensors)   obc flash debug-uart
#       obc flash ble        (companion link) obc flash ble debug-uart
#       obc flash build      (compile only, no flash)
# Build & flash the board. Opts: ble debug-uart synth build (e.g. obc flash ble debug-uart)
flash *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source "{{lib}}"; obc_init; ensure_riscv || exit 1
    ble=0 du=0 synth=0 action=run
    for o in "$@"; do case "$o" in
      ble)                ble=1 ;;
      debug-uart|du)      du=1 ;;
      synth)              synth=1 ;;
      build|build-only)   action=build ;;
      *) _err "unknown flash option: '$o'  (want: ble debug-uart synth build)"; exit 1 ;;
    esac; done
    [[ "$action" == run ]] && { ensure_probe || exit 1; }
    a=(); feats=()
    (( ble ))   && { a+=(--no-default-features); feats+=(ble); }
    (( du ))    && feats+=(debug-uart)
    (( synth )) && feats+=(synth)
    (( ${#feats[@]} )) && a+=(--features "$(IFS=,; echo "${feats[*]}")")
    cd "$OBC_ROOT/firmware/obc-fw-nrf54l"
    _run cargo "$action" --release "${a[@]}"

# Feed a recorded ride to a debug-uart board over VCOM (obc-usb-host).
# Args: [TRACK.gpx] [-- --port TTY | --baud N | --list].
# Feed a GPX to a debug-uart board over VCOM. Args: [TRACK.gpx] [-- --port TTY | --list]
uart *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source "{{lib}}"; obc_init; ensure_geos
    split_args "$@"
    cd "$OBC_ROOT/firmware"
    # `--list` just enumerates serial ports; no GPX needed.
    for e in "${_EXTRA[@]:-}"; do [[ "$e" == --list ]] && { _run cargo run --release -p obc-usb-host -- --list; exit 0; }; done
    gpxf="$(resolve_gpx "${_POS[0]:-}")"
    [[ -z "$gpxf" || "$gpxf" == none ]] && { _err "no GPX to feed. Pass one:  obc uart <track.gpx>"; exit 1; }
    cmd=(cargo run --release -p obc-usb-host -- --gpx "$gpxf")
    (( ${#_EXTRA[@]} )) && cmd+=("${_EXTRA[@]}")
    _run "${cmd[@]}"

# Indoor ride in one step: flash the debug-uart firmware, then feed it a GPX over
# VCOM. Args: [ble] [TRACK.gpx]. Flashes with `probe-rs download` (returns cleanly,
# no RTT), then runs the feeder TUI in the foreground. Want defmt logs too? Run
# `obc flash debug-uart` in a second terminal instead.
# One step: flash debug-uart firmware, then feed it a GPX. Args: [ble] [TRACK.gpx]
debug *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source "{{lib}}"; obc_init; ensure_riscv || exit 1; ensure_probe || exit 1
    ble=""; gpx=""
    for a in "$@"; do case "$a" in ble) ble=ble ;; *) gpx="$a" ;; esac; done
    feats="debug-uart"; nodef=()
    [[ -n "$ble" ]] && { feats="ble,debug-uart"; nodef=(--no-default-features); }
    elf="$OBC_ROOT/firmware/obc-fw-nrf54l/target/thumbv8m.main-none-eabihf/release/obc-fw-nrf54l"
    _say "building debug-uart firmware…"
    ( cd "$OBC_ROOT/firmware/obc-fw-nrf54l" && _run cargo build --release "${nodef[@]}" --features "$feats" )
    _say "flashing over J-Link…"
    _run probe-rs download --chip nRF54L15 --verify "$elf"
    _run probe-rs reset --chip nRF54L15
    _say "flashed — starting the feeder (Ctrl-C to stop)…"
    just --justfile '{{justfile()}}' uart ${gpx:+"$gpx"}

# ── Map packing & the web builder ────────────────────────────────────────────

# Pack an OSM extract into a .obcm map. Args: <region.osm.pbf> [preset.json] [out.obcm]
# [-- --chunk-size N | --no-land]. Preset defaults to packer/presets/default.json; the
# output defaults to <region>.obcm in the repo root (so `obc sim` picks it up next).
# Pack an OSM extract to a .obcm map. Args: <region.osm.pbf> [preset.json] [out.obcm]
pack *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source "{{lib}}"; obc_init; ensure_geos
    split_args "$@"
    pbf="${_POS[0]:-}"
    [[ -n "$pbf" ]] || { _err "usage: obc pack <region.osm.pbf> [preset.json] [out.obcm]"; exit 1; }
    pbf="$(_abspath "$pbf")"
    preset="${_POS[1]:-${OBC_PRESET:-$OBC_ROOT/packer/presets/default.json}}"; preset="$(_abspath "$preset")"
    out="${_POS[2]:-}"
    if [[ -n "$out" ]]; then out="$(_abspath "$out")"
    else base="$(basename "$pbf")"; base="${base%.osm.pbf}"; base="${base%.pbf}"; out="$OBC_ROOT/${base}.obcm"; fi
    ( cd "$OBC_ROOT/firmware" && _run cargo build --release -p obc-pack )
    cmd=("$OBC_ROOT/firmware/target/release/obc-pack" "$pbf" "$preset" "$out")
    (( ${#_EXTRA[@]} )) && cmd+=("${_EXTRA[@]}")
    _run "${cmd[@]}"
    _say "wrote $out"

# Run the web map builder at http://localhost:8000 (bootstraps the venv, obc-pack,
# and the frontend on first run). Extra args pass through (e.g. --no-browser).
# Run the web map builder on :8000 (bootstraps venv/frontend on first run)
web *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source "{{lib}}"; obc_init; ensure_geos
    cd "$OBC_ROOT"
    export OBC_PACK_BIN="$OBC_ROOT/firmware/target/release/obc-pack"
    if [[ ! -x .venv/bin/python ]]; then
      _say "creating .venv + installing packer deps…"
      _run python3 -m venv .venv
      _run .venv/bin/pip install -q --upgrade pip
      _run .venv/bin/pip install -q -r packer/requirements.txt
    fi
    [[ -x "$OBC_PACK_BIN" ]] || { _say "building obc-pack…"; ( cd firmware && _run cargo build --release -p obc-pack ); }
    if [[ ! -d packer/web_builder/static/dist ]]; then
      _say "building the frontend…"; ( cd packer/web_builder/frontend && _run npm ci && _run npm run build )
    fi
    _run .venv/bin/python -m packer.web_builder "$@"

# ── Host workspace chores ────────────────────────────────────────────────────

# Build the host workspace (simulator + shared crates + packer).
build:
    #!/usr/bin/env bash
    set -euo pipefail
    source "{{lib}}"; obc_init; ensure_geos
    cd "$OBC_ROOT/firmware"; _run cargo build --release

# Run the host test suite. Extra args pass through (e.g. -p obc-pack).
test *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source "{{lib}}"; obc_init; ensure_geos
    cd "$OBC_ROOT/firmware"; _run cargo test "$@"

# Format everything: the workspace AND the excluded board crate (the required two-step).
fmt:
    #!/usr/bin/env bash
    set -euo pipefail
    source "{{lib}}"; obc_init
    cd "$OBC_ROOT/firmware"
    _run cargo fmt --all
    _run cargo fmt --manifest-path obc-fw-nrf54l/Cargo.toml

# Render benchmark + pixel-hash tripwire. Args: (none) | check | write.
bench *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source "{{lib}}"; obc_init; ensure_geos
    cd "$OBC_ROOT/firmware"
    case "${1:-}" in
      check) _run cargo run -p obc-bench --release -- --check obc-bench/hashes.txt ;;
      write) _run cargo run -p obc-bench --release -- --write-hashes obc-bench/hashes.txt ;;
      *)     _run cargo run -p obc-bench --release ;;
    esac

# Confirm the shared stack still compiles for the device target (no board crate).
check-device:
    #!/usr/bin/env bash
    set -euo pipefail
    source "{{lib}}"; obc_init
    cd "$OBC_ROOT/firmware"; _run cargo build -p obc-app --target thumbv8m.main-none-eabihf

# ── Meta ─────────────────────────────────────────────────────────────────────

# Check your toolchain and report what's missing (and how to fix it).
doctor:
    #!/usr/bin/env bash
    set -uo pipefail
    source "{{lib}}"; obc_init
    _say "OpenBikeComputer toolchain check"
    ck() { if eval "$2" >/dev/null 2>&1; then _ok "$1"; else _err "$1"; [[ -n "${3:-}" ]] && _hint "$3"; fi; }
    ck "rust / cargo"                 'command -v cargo'         'install rustup: https://rustup.rs'
    ck "just"                         'command -v just'          'cargo install just'
    ck "device target (thumbv8m)"     'rustup target list --installed | grep -q thumbv8m.main-none-eabihf' 'rustup target add thumbv8m.main-none-eabihf'
    ck "wasm target (docs demo)"      'rustup target list --installed | grep -q wasm32-unknown-unknown'     'rustup target add wasm32-unknown-unknown'
    ck "GEOS (obc-pack)"              '[[ -f "$HOME/.obc-geos-env.sh" ]] && { source "$HOME/.obc-geos-env.sh"; command -v geos-config; }' 'build GEOS ≥3.14 and write ~/.obc-geos-env.sh (see repo notes)'
    ck "RISC-V gcc (board FLPR blob)" 'command -v riscv64-elf-gcc || command -v riscv-none-elf-gcc || command -v riscv64-unknown-elf-gcc || { [[ -n "${RISCV_GCC:-}" && -x "${RISCV_GCC:-/nonexistent}" ]]; }' 'xPack riscv-none-elf-gcc, then set RISCV_GCC in obc.local'
    ck "probe-rs (flashing)"          'command -v probe-rs'      'cargo install probe-rs-tools --locked'
    ck "node + npm (web builder)"     'command -v node && command -v npm' 'install Node 22+'
    ck "python venv (web builder)"    '[[ -x "$OBC_ROOT/.venv/bin/python" ]]' 'run `obc web` once to bootstrap it'
    m="$(_all_maps | head -n1 || true)"
    if [[ -n "$m" ]]; then _ok "default sim map → ${m##*/}"; else _warn "no .obcm found — build one with 'obc pack' or 'obc web', or drop one in the repo root"; fi

# One-time: link `obc` into ~/.local/bin and enable bash completion.
setup:
    #!/usr/bin/env bash
    set -euo pipefail
    source "{{lib}}"; obc_init
    mkdir -p "$HOME/.local/bin"
    ln -sf "$OBC_ROOT/obc" "$HOME/.local/bin/obc"
    _ok "linked ~/.local/bin/obc → $OBC_ROOT/obc"
    comp="$OBC_ROOT/completions/obc.bash"
    line="[ -f \"$comp\" ] && source \"$comp\""
    if ! grep -qF "$comp" "$HOME/.bashrc" 2>/dev/null; then
      printf '\n# OpenBikeComputer `obc` completion\n%s\n' "$line" >> "$HOME/.bashrc"
      _ok "added completion to ~/.bashrc"
    else _ok "completion already in ~/.bashrc"; fi
    case ":$PATH:" in *":$HOME/.local/bin:"*) : ;; *) _warn "~/.local/bin is not on PATH — add it in ~/.bashrc";; esac
    _say "open a new shell (or: source ~/.bashrc) to pick up 'obc' + completion"
