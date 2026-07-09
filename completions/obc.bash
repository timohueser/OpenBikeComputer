# Bash completion for the `obc` dev command (source from ~/.bashrc; `obc setup` does this).
# Completes task names, flash options, and — the good part — the actual .obcm / .gpx /
# .osm.pbf / preset files you'd pass, drawn from the repo root, the maps/ dir, and the
# web-builder cache. Works for `obc`, `./obc`, and `just`.

# Find the repo root from the `obc` on PATH (following symlinks).
_obc_root() {
  local o; o="$(command -v obc 2>/dev/null)" || return 1
  o="$(readlink -f "$o" 2>/dev/null)" || return 1
  local d; d="$(dirname "$o")"
  [[ -f "$d/justfile" ]] && printf '%s\n' "$d"
}

# Task names, from the justfile (falls back to a static list).
_obc_tasks() {
  local root; root="$(_obc_root)"
  if [[ -n "$root" ]] && command -v just >/dev/null 2>&1; then
    just --justfile "$root/justfile" --summary 2>/dev/null && return
  fi
  echo "sim flash uart debug pack web build test fmt bench check-device doctor setup"
}

# .obcm maps across the repo root, maps/, and the web-builder cache.
_obc_maps() {
  local root; root="$(_obc_root)" || return
  { find "$root" -maxdepth 1 -name '*.obcm' 2>/dev/null
    find "$root/maps" -maxdepth 1 -name '*.obcm' 2>/dev/null
    find "$HOME/.cache/obcm/builds" -maxdepth 3 -name '*.obcm' 2>/dev/null; }
}

# Bundled + saved GPX tracks worth suggesting.
_obc_gpx() {
  local root; root="$(_obc_root)" || return
  find "$root/firmware/obc-sim/assets" "$root/tracks" -maxdepth 1 -name '*.gpx' 2>/dev/null
}

_obc_presets() {
  local root; root="$(_obc_root)" || return
  find "$root/packer/presets" -maxdepth 1 -name '*.json' 2>/dev/null
}

# Index of the current word among the non-flag args (0 = first positional, …).
_obc_posidx() {
  local i n=0
  for ((i = 2; i < COMP_CWORD; i++)); do
    [[ "${COMP_WORDS[i]}" == -* ]] || ((n++))
  done
  echo "$n"
}

_obc() {
  local cur task idx
  cur="${COMP_WORDS[COMP_CWORD]}"
  task="${COMP_WORDS[1]:-}"

  if (( COMP_CWORD == 1 )); then
    mapfile -t COMPREPLY < <(compgen -W "$(_obc_tasks)" -- "$cur")
    return
  fi

  idx="$(_obc_posidx)"
  case "$task" in
    sim)
      case "$idx" in
        0) compopt -o filenames 2>/dev/null; mapfile -t COMPREPLY < <(compgen -W "$(_obc_maps)" -- "$cur"; compgen -f -X '!*.obcm' -- "$cur") ;;
        1) compopt -o filenames 2>/dev/null; mapfile -t COMPREPLY < <(compgen -W "$(_obc_gpx) none" -- "$cur"; compgen -f -X '!*.gpx' -- "$cur") ;;
      esac ;;
    uart)
      (( idx == 0 )) && { compopt -o filenames 2>/dev/null; mapfile -t COMPREPLY < <(compgen -W "$(_obc_gpx)" -- "$cur"; compgen -f -X '!*.gpx' -- "$cur"); } ;;
    debug)
      compopt -o filenames 2>/dev/null
      mapfile -t COMPREPLY < <(compgen -W "ble $(_obc_gpx)" -- "$cur"; compgen -f -X '!*.gpx' -- "$cur") ;;
    flash)
      mapfile -t COMPREPLY < <(compgen -W "ble debug-uart synth build" -- "$cur") ;;
    pack)
      case "$idx" in
        0) compopt -o filenames 2>/dev/null; mapfile -t COMPREPLY < <(compgen -f -X '!*.pbf' -- "$cur"; compgen -d -- "$cur") ;;
        1) compopt -o filenames 2>/dev/null; mapfile -t COMPREPLY < <(compgen -W "$(_obc_presets)" -- "$cur"; compgen -f -X '!*.json' -- "$cur") ;;
        *) compopt -o filenames 2>/dev/null; mapfile -t COMPREPLY < <(compgen -f -- "$cur") ;;
      esac ;;
    bench)
      (( idx == 0 )) && mapfile -t COMPREPLY < <(compgen -W "check write" -- "$cur") ;;
    test)
      mapfile -t COMPREPLY < <(compgen -W "-p --release --" -- "$cur") ;;
  esac
}

complete -F _obc obc ./obc
