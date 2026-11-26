# Sourced last by a tools/ci script: runs the sections it names, or every section in SECTIONS.
# A section `a-b` is the function `a_b`, and it runs in a subshell, so one section's exported
# environment cannot reach the next.
(( $# )) || set -- "${SECTIONS[@]}"
for section in "$@"; do
  [[ " ${SECTIONS[*]} " == *" $section "* ]] || {
    echo "$0: unknown section $section (known: ${SECTIONS[*]})" >&2
    exit 2
  }
  ( "${section//-/_}" )
done
