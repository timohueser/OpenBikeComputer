# Bash completion for the `obc` dev command (source from ~/.bashrc; `obc setup` does this).
# Completes task names, flash options, and — the good part — the actual .obcm / .gpx /
# .osm.pbf / preset files you'd pass, drawn from the repo root, the maps/ dir, and the
# web-builder cache. Works for `obc` and `./obc`.

# Select the checkout as the installed entry point does.
_obc_toolsdir() {
  local checkout; checkout="$(git rev-parse --show-toplevel 2>/dev/null || true)"
  if [[ -n "$checkout" && -f "$checkout/tools/obc" && -d "$checkout/firmware/obc-app" &&
        ( -f "$checkout/justfile" || -f "$checkout/tools/justfile" ) ]]; then
    printf '%s\n' "$checkout/tools"
    return
  fi
  local o d; o="$(command -v obc 2>/dev/null)" || return 1
  while [[ -h "$o" ]]; do
    d="$(cd -P "$(dirname "$o")" && pwd)" || return 1
    o="$(readlink "$o")"; [[ "$o" != /* ]] && o="$d/$o"
  done
  d="$(cd -P "$(dirname "$o")" && pwd)" || return 1
  [[ -f "$d/../justfile" || -f "$d/justfile" ]] && printf '%s\n' "$d"
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

# Task names, from the justfile (falls back to a static list). The `agent` group stays out,
# because completion is a person's tool; OBC_COMPLETE_ALL=1 offers it too.
_obc_tasks() {
  local t; t="$(_obc_toolsdir)"
  if [[ -n "$t" ]]; then
    local scope=(); [[ "${OBC_COMPLETE_ALL:-}" == 1 ]] && scope=(--all)
    local justfile="$t/../justfile"
    [[ -f "$justfile" ]] || justfile="$t/justfile"
    python3 "$t/tasks.py" --justfile "$justfile" --names "${scope[@]}" 2>/dev/null && return
  fi
  echo "fixtures sim board flash flash-boot uart debug rtt pack bake web site desktop build test fmt licenses bench check check-device clean doctor setup"
}

_obc_fixture_ids() {
  local root; root="$(_obc_root)" || return
  python3 "$root/tools/fixtures.py" complete "$1" 2>/dev/null
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
  find "$root/fixtures/sources" "$root/tracks" -maxdepth 5 -name '*.gpx' 2>/dev/null
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
    _obc_reply < <(compgen -W "help $(_obc_tasks)" -- "$cur")
    return
  fi

  idx="$(_obc_posidx)"
  case "$task" in
    help)
      (( idx == 0 )) && _obc_reply < <(compgen -W "$(_obc_tasks)" -- "$cur") ;;
    sim)
      case "$idx" in
        0) compopt -o filenames 2>/dev/null; _obc_reply < <(compgen -W "$(_obc_fixture_ids scenarios) $(_obc_maps)" -- "$cur"; compgen -f -X '!*.obcm' -- "$cur") ;;
        1) compopt -o filenames 2>/dev/null; _obc_reply < <(compgen -W "$(_obc_gpx) none" -- "$cur"; compgen -f -X '!*.gpx' -- "$cur") ;;
      esac ;;
    uart)
      (( idx == 0 )) && { compopt -o filenames 2>/dev/null; _obc_reply < <(compgen -W "$(_obc_gpx)" -- "$cur"; compgen -f -X '!*.gpx' -- "$cur"); } ;;
    debug)
      compopt -o filenames 2>/dev/null
      _obc_reply < <(compgen -W "$(_obc_gpx)" -- "$cur"; compgen -f -X '!*.gpx' -- "$cur") ;;
    flash)
      _obc_reply < <(compgen -W "debug-uart synth build" -- "$cur") ;;
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
    board)
      (( idx == 0 )) && _obc_reply < <(compgen -W "doctor run download attach reset" -- "$cur") ;;
    flash-boot)
      _obc_reply < <(compgen -W "rtt build" -- "$cur") ;;
    check)
      _obc_reply < <(compgen -W "fmt clippy test device docs board frontend deny wasm full" -- "$cur") ;;
    doctor)
      _obc_reply < <(compgen -W "--install" -- "$cur") ;;
    test)
      _obc_reply < <(compgen -W "-p fixtures full --release --" -- "$cur") ;;
    clean)
      _obc_reply < <(compgen -W "--apply --days" -- "$cur") ;;
    fixtures)
      if (( idx == 0 )); then
        _obc_reply < <(compgen -W "list show sync verify prune pack publish" -- "$cur")
      else
        _obc_reply < <(compgen -W "$(_obc_fixture_ids targets) --apply" -- "$cur")
      fi ;;
  esac
}

complete -F _obc obc ./obc
