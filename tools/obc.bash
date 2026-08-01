# Bash completion for the `obc` dev command (source from ~/.bashrc; `obc setup` does this).
# Completes task names, flash options, and — the good part — the actual .obcm / .gpx /
# .osm.pbf / preset files you'd pass, drawn from the repo root, the maps/ dir, and the
# web-builder cache. Works for `obc` and `./obc`.

# The tools/ dir (holds obc + justfile), found from the `obc` on PATH (following symlinks).
_obc_toolsdir() {
  local o; o="$(command -v obc 2>/dev/null)" || return 1
  o="$(readlink -f "$o" 2>/dev/null)" || return 1
  local d; d="$(dirname "$o")"
  [[ -f "$d/justfile" ]] && printf '%s\n' "$d"
}

# The repo root — the parent of tools/ — used for map/gpx/preset paths.
_obc_root() {
  local t; t="$(_obc_toolsdir)" || return 1
  dirname "$t"
}

# zsh runs this file through `bashcompinit`, which supplies `compgen`/`complete` but NOT
# `mapfile` (a bash builtin) — so filling COMPREPLY portably is on us. Reads NUL-free lines,
# which is what compgen emits.
_obc_reply() { COMPREPLY=(); local _l; while IFS= read -r _l; do COMPREPLY+=("$_l"); done; }

# Task names, from the justfile (falls back to a static list).
_obc_tasks() {
  local t; t="$(_obc_toolsdir)"
  if [[ -n "$t" ]] && command -v just >/dev/null 2>&1; then
    just --justfile "$t/justfile" --summary 2>/dev/null && return
  fi
  echo "sim flash flash-boot uart debug rtt pack bake web site desktop build test fmt bench check check-device doctor setup"
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
  find "$root/apps/obc-sim/assets" "$root/tracks" -maxdepth 1 -name '*.gpx' 2>/dev/null
}

# The shipped packer configs. `-maxdepth 1` is doing real work: it keeps
# builder/presets/skins/ out, and a skin is not something `obc pack` can use.
_obc_presets() {
  local root; root="$(_obc_root)" || return
  find "$root/builder/presets" -maxdepth 1 -name '*.json' 2>/dev/null
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
  # zsh arrays are 1-based, bash's are 0-based, and `bashcompinit` does not change that — so
  # `COMP_WORDS[1]` is the *task* in bash and the *command name* in zsh, and every per-task case
  # below silently matched nothing. `ksh_arrays` gives this function bash indexing; `local_options`
  # restores the shell's own setting on return. Guarded by ZSH_VERSION so bash never runs it.
  [ -n "${ZSH_VERSION:-}" ] && setopt local_options ksh_arrays
  local cur task idx
  cur="${COMP_WORDS[COMP_CWORD]}"
  task="${COMP_WORDS[1]:-}"

  if (( COMP_CWORD == 1 )); then
    _obc_reply < <(compgen -W "$(_obc_tasks)" -- "$cur")
    return
  fi

  idx="$(_obc_posidx)"
  case "$task" in
    sim)
      case "$idx" in
        0) compopt -o filenames 2>/dev/null; _obc_reply < <(compgen -W "$(_obc_maps)" -- "$cur"; compgen -f -X '!*.obcm' -- "$cur") ;;
        1) compopt -o filenames 2>/dev/null; _obc_reply < <(compgen -W "$(_obc_gpx) none" -- "$cur"; compgen -f -X '!*.gpx' -- "$cur") ;;
      esac ;;
    uart)
      (( idx == 0 )) && { compopt -o filenames 2>/dev/null; _obc_reply < <(compgen -W "$(_obc_gpx)" -- "$cur"; compgen -f -X '!*.gpx' -- "$cur"); } ;;
    debug)
      compopt -o filenames 2>/dev/null
      _obc_reply < <(compgen -W "ble $(_obc_gpx)" -- "$cur"; compgen -f -X '!*.gpx' -- "$cur") ;;
    flash)
      _obc_reply < <(compgen -W "ble debug-uart synth build" -- "$cur") ;;
    pack)
      case "$idx" in
        0) compopt -o filenames 2>/dev/null; _obc_reply < <(compgen -f -X '!*.pbf' -- "$cur"; compgen -d -- "$cur") ;;
        1) compopt -o filenames 2>/dev/null; _obc_reply < <(compgen -W "$(_obc_presets)" -- "$cur"; compgen -f -X '!*.json' -- "$cur") ;;
        *) compopt -o filenames 2>/dev/null; _obc_reply < <(compgen -f -- "$cur") ;;
      esac ;;
    bench)
      (( idx == 0 )) && _obc_reply < <(compgen -W "check write" -- "$cur") ;;
    desktop)
      _obc_reply < <(compgen -W "dev build" -- "$cur") ;;
    flash-boot)
      _obc_reply < <(compgen -W "rtt build" -- "$cur") ;;
    check)
      _obc_reply < <(compgen -W "fmt clippy test device docs board frontend deny wasm" -- "$cur") ;;
    doctor)
      _obc_reply < <(compgen -W "--install" -- "$cur") ;;
    test)
      _obc_reply < <(compgen -W "-p --release --" -- "$cur") ;;
  esac
}

complete -F _obc obc ./obc
