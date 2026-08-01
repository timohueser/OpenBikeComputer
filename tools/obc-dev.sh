# shellcheck shell=bash
# Shared helpers for the `obc` dev tasks (sourced by the recipes in ../justfile).
# Not meant to be run directly. Relies on $OBC_ROOT being exported by the justfile.

# ---- pretty output (only colorize a real terminal) --------------------------
if [[ -t 2 ]]; then
  _c_dim=$'\033[2m'; _c_red=$'\033[31m'; _c_grn=$'\033[32m'
  _c_ylw=$'\033[33m'; _c_cyn=$'\033[36m'; _c_rst=$'\033[0m'
else
  _c_dim=; _c_red=; _c_grn=; _c_ylw=; _c_cyn=; _c_rst=
fi

_say()  { printf '%s» %s%s\n'   "$_c_cyn" "$*" "$_c_rst" >&2; }
_ok()   { printf '%s  ✓ %s%s\n' "$_c_grn" "$*" "$_c_rst" >&2; }
_warn() { printf '%s  ! %s%s\n' "$_c_ylw" "$*" "$_c_rst" >&2; }
_err()  { printf '%s  ✗ %s%s\n' "$_c_red" "$*" "$_c_rst" >&2; }
_hint() { printf '%s    %s%s\n' "$_c_dim" "$*" "$_c_rst" >&2; }

# Echo a command (dimmed) then run it. Set OBC_DRY_RUN=1 to only echo.
_run() {
  printf '%s▸ %s%s\n' "$_c_dim" "$*" "$_c_rst" >&2
  [[ -n "${OBC_DRY_RUN:-}" ]] && return 0
  "$@"
}

# ---- setup ------------------------------------------------------------------
# Source personal defaults (gitignored). Do this first so OBC_* overrides win.
obc_init() {
  [[ -n "${OBC_ROOT:-}" ]] || { echo "obc: OBC_ROOT unset (run via the obc wrapper)" >&2; return 1; }
  local local_file="${OBC_TOOLS:-$OBC_ROOT}/obc.local"
  # `obc.local` is the operator-facing environment file. Auto-export it so values
  # such as OBC_CATALOG_URL and the R2 credentials reach the Rust binaries and the
  # desktop process without making every local line repeat `export`.
  if [[ -f "$local_file" ]]; then
    set -a
    source "$local_file"
    set +a
  fi
  # remember where the user invoked us, before recipes cd around
  OBC_PWD="${OBC_PWD:-$PWD}"
  return 0
}

# Expose the locally-built GEOS to obc-pack / the workspace build.
ensure_geos() {
  [[ -f "$HOME/.obc-geos-env.sh" ]] && source "$HOME/.obc-geos-env.sh"
  return 0
}

# Find a RISC-V gcc for the FLPR blob the board build cross-compiles, and export
# RISCV_GCC. Honors an RISCV_GCC override from obc.local. Fails loudly if none.
ensure_riscv() {
  if [[ -n "${RISCV_GCC:-}" && -x "${RISCV_GCC}" ]]; then export RISCV_GCC; return 0; fi
  local c
  for c in riscv64-elf-gcc riscv-none-elf-gcc riscv64-unknown-elf-gcc; do
    command -v "$c" >/dev/null 2>&1 && return 0   # on PATH — build.rs auto-detects
  done
  local g
  for g in "$HOME"/.local/xPacks/*/bin/riscv-none-elf-gcc \
           "$HOME"/.local/xPacks/*/*/bin/riscv-none-elf-gcc \
           "$HOME"/.local/*riscv*/bin/riscv-none-elf-gcc \
           "$HOME"/xpack-riscv*/bin/riscv-none-elf-gcc; do
    [[ -x "$g" ]] && { export RISCV_GCC="$g"; _say "RISCV_GCC=$g"; return 0; }
  done
  _err "no RISC-V gcc found — the board build cross-compiles a RISC-V FLPR blob and needs one."
  _hint "Install an xPack toolchain (no root needed), then point obc.local at it:"
  _hint "  https://github.com/xpack-dev-tools/riscv-none-elf-gcc-xpack/releases"
  _hint "  echo 'RISCV_GCC=\$HOME/.local/xPacks/.../bin/riscv-none-elf-gcc' >> obc.local"
  _hint "or, with sudo:  sudo dnf install gcc-riscv64-unknown-elf"
  return 1
}

# Fail (with a fix hint) if probe-rs — needed to flash and to attach RTT — is missing.
ensure_probe() {
  command -v probe-rs >/dev/null 2>&1 && return 0
  _err "probe-rs not found — needed to flash the board and to attach RTT."
  _hint "Install:  cargo install probe-rs-tools --locked   (or: obc doctor --install)"
  return 1
}

# ---- path / default resolution ----------------------------------------------
_abspath() { case "$1" in /*) printf '%s\n' "$1";; *) printf '%s\n' "${OBC_PWD:-$PWD}/$1";; esac; }

# Where to look for maps (newest wins). Default: the repo ROOT first — that's
# where sim maps usually live — then maps/, then the web-builder cache.
# Override with OBC_MAPS_DIRS (colon-separated) in obc.local. Emits "<dir>\t<maxdepth>".
_maps_search() {
  if [[ -n "${OBC_MAPS_DIRS:-}" ]]; then
    local d; while IFS= read -r d; do [[ -n "$d" ]] && printf '%s\t2\n' "$d"; done < <(tr ':' '\n' <<<"$OBC_MAPS_DIRS")
  else
    printf '%s\t1\n' "$OBC_ROOT"
    printf '%s\t1\n' "$OBC_ROOT/maps"
    printf '%s\t3\n' "$HOME/.cache/obcm/builds"
  fi
}

# All .obcm across the search dirs, newest first, de-duplicated.
_all_maps() {
  local dir depth
  while IFS=$'\t' read -r dir depth; do
    [[ -d "$dir" ]] || continue
    find "$dir" -maxdepth "$depth" -name '*.obcm' -printf '%T@\t%p\n' 2>/dev/null
  done < <(_maps_search) | sort -rn | cut -f2- | awk '!seen[$0]++'
}

# Resolve a map: explicit arg > $OBC_MAP > newest built map > friendly error.
resolve_map() {
  local m="${1:-}"
  [[ -n "$m" ]] && { _abspath "$m"; return 0; }
  [[ -n "${OBC_MAP:-}" ]] && { _abspath "$OBC_MAP"; return 0; }
  local newest; newest="$(_all_maps | head -n1)"
  if [[ -n "$newest" ]]; then _warn "no map given — using newest: ${newest##*/}"; printf '%s\n' "$newest"; return 0; fi
  # last resort: the committed sample fixture, so a fresh clone's `obc sim` just works
  local fx="$OBC_ROOT/apps/obc-sim/assets/grimsel.obcm"
  [[ -f "$fx" ]] && { _warn "no map found — using the bundled sample: ${fx##*/}"; printf '%s\n' "$fx"; return 0; }
  _err "no map file. Pass one:  obc sim <map.obcm>"
  _hint "or set OBC_MAP in obc.local, or build one:  obc pack <region.osm.pbf>   /   obc web"
  return 1
}

# Resolve a GPX: explicit arg > $OBC_GPX > bundled default. 'none'/'-' means skip.
resolve_gpx() {
  local g="${1:-}"
  [[ "$g" == none || "$g" == "-" ]] && { printf 'none\n'; return 0; }
  [[ -n "$g" ]] && { _abspath "$g"; return 0; }
  [[ -n "${OBC_GPX:-}" ]] && { _abspath "$OBC_GPX"; return 0; }
  local def="$OBC_ROOT/apps/obc-sim/assets/grimsel-climb.gpx"
  [[ -f "$def" ]] && { printf '%s\n' "$def"; return 0; }
  printf '\n'   # nothing — caller decides whether that is fatal
}

# Split "$@" into positional args (_POS) and pass-through flags (_EXTRA).
# Rule: everything after a literal `--` is pass-through verbatim; before it,
# bare -flags are pass-through and the rest are positional. So a flag that takes
# a value (e.g. --scale 2) goes after `--`:  obc sim map.obcm -- --scale 2
split_args() {
  _POS=(); _EXTRA=(); local a sep=0
  for a in "$@"; do
    if (( sep )); then _EXTRA+=("$a"); continue; fi
    case "$a" in
      --)  sep=1 ;;
      -*)  _EXTRA+=("$a") ;;
      *)   _POS+=("$a") ;;
    esac
  done
}
