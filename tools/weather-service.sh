#!/usr/bin/env bash
# Pause, resume, or inspect the one deployed weather bakery over SSH.
# Invoked by `obc weather`; keep the remote half self-contained so the VPS needs
# only systemd, not a checkout of this repository.
set -euo pipefail

action="${1:-status}"
target="${2:-${OBC_WX_SSH_TARGET:-root@wx}}"

case "$action" in
  status|start|stop) ;;
  *)
    echo "weather-service: unknown action '$action' (want: status, start, or stop)" >&2
    exit 2
    ;;
esac

case "$target" in
  ''|-*|*[[:space:]]*)
    echo "weather-service: unsafe SSH target '$target'" >&2
    echo "set OBC_WX_SSH_TARGET=root@<hostname-or-ip> in tools/obc.local" >&2
    exit 2
    ;;
esac

command -v ssh >/dev/null 2>&1 || {
  echo "weather-service: ssh is required" >&2
  exit 127
}

echo "weather bakery: $action on $target" >&2
ssh -o BatchMode=yes -o ConnectTimeout=10 -- "$target" bash -s -- "$action" <<'REMOTE'
set -euo pipefail

action="${1:-status}"
cycle_timer=obc-wx-bake@cycle.timer
cycle_service=obc-wx-bake@cycle.service
binary=/usr/local/bin/obc-wx-bake

if [ "$(id -u)" -ne 0 ]; then
  echo "weather-service: connect as root (set OBC_WX_SSH_TARGET=root@<host>)" >&2
  exit 1
fi

describe_binary() {
  if [ ! -x "$binary" ]; then
    printf 'publisher: absent (%s)\n' "$binary"
  elif grep -aFq 'obc-wx-bake/0.1 https://github.com/timohueser/OpenBikeComputer' "$binary" 2>/dev/null; then
    printf 'publisher: in-process S3 (%s)\n' "$binary"
  else
    printf 'publisher: unrecognized binary (%s)\n' "$binary"
  fi
}

show_status() {
  describe_binary
  echo
  echo "Installed weather timers:"
  systemctl list-unit-files 'obc-wx-bake@*.timer' --no-pager || true
  echo
  echo "Weather schedule:"
  systemctl list-timers --all 'obc-wx-bake@*' --no-pager || true
  echo
  printf 'cycle timer:   enabled=%s active=%s\n' \
    "$(systemctl is-enabled "$cycle_timer" 2>/dev/null || true)" \
    "$(systemctl is-active "$cycle_timer" 2>/dev/null || true)"
  printf 'cycle service: active=%s result=%s\n' \
    "$(systemctl is-active "$cycle_service" 2>/dev/null || true)" \
    "$(systemctl show -p Result --value "$cycle_service" 2>/dev/null || true)"
  echo
  echo "Recent publishes:"
  journalctl -u "$cycle_service" --since '-90 minutes' --no-pager \
    | grep -E 'Starting |published |retired |Failed |failure|error' \
    | tail -n 30 || true
}

case "$action" in
  status)
    show_status
    ;;
  stop)
    systemctl disable --now "$cycle_timer" >/dev/null 2>&1 || true
    # Stop a bake already in flight as well. The publisher is fail-closed before
    # its manifest swap; after the swap, its sweep is idempotent and the bucket's
    # lifecycle rule collects anything it did not finish.
    systemctl stop "$cycle_service" || true
    if systemctl is-active --quiet "$cycle_timer"; then
      echo "weather-service: $cycle_timer is still active after stop" >&2
      exit 1
    fi
    echo "weather bakery stopped: $cycle_timer disabled and in-flight bake stopped"
    show_status
    ;;
  start)
    if ! systemctl cat "$cycle_timer" >/dev/null 2>&1 || [ ! -x "$binary" ]; then
      echo "weather-service: canonical publisher is not installed" >&2
      echo "run ops/weather/install.sh with the current binary before resuming" >&2
      exit 1
    fi
    systemctl enable --now "$cycle_timer"
    systemctl is-enabled --quiet "$cycle_timer"
    systemctl is-active --quiet "$cycle_timer"
    echo "weather bakery started: $cycle_timer enabled"
    show_status
    ;;
esac
REMOTE
