#!/usr/bin/env bash
# capped [-m 3G] [-t 600] CMD...  Run CMD in its own memory cgroup (v1) with a hard cap and a time limit.
set -euo pipefail
mem=3G; secs=600
while [[ $# -gt 0 ]]; do case "$1" in -m) mem=$2; shift 2;; -t) secs=$2; shift 2;; *) break;; esac; done
cg=/sys/fs/cgroup/memory/capped-$$-$RANDOM
mkdir "$cg"
bytes=$(numfmt --from=iec "$mem")
echo "$bytes" > "$cg/memory.limit_in_bytes"
echo "$bytes" > "$cg/memory.memsw.limit_in_bytes" 2>/dev/null || true
cleanup() { for p in $(cat "$cg/cgroup.procs" 2>/dev/null); do kill -9 "$p" 2>/dev/null || true; done; sleep 0.2; rmdir "$cg" 2>/dev/null || true; }
trap cleanup EXIT
set +e
bash -c 'echo $$ > "$1/cgroup.procs"; shift; exec timeout "$0" "$@"' "$secs" "$cg" "$@"
rc=$?
if [[ $(cat "$cg/memory.failcnt") != 0 ]]; then echo "capped: hit the $mem cap (failcnt $(cat "$cg/memory.failcnt"))" >&2; fi
[[ $rc == 124 ]] && echo "capped: timed out after ${secs}s" >&2
exit $rc
