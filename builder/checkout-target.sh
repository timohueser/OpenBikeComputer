# Sourced by the build scripts with `repo_dir` set to the checkout (`pwd -P`). Cargo takes a crate as fresh
# when this checkout's sources are older than the crate's last build, and a target that several checkouts
# share holds one build of each crate: it gives the build of another checkout. So with CARGO_TARGET_DIR set,
# a checkout builds in `checkouts/<key>/` of it. `obc clean` (tools/dev_cleanup.py) computes the same key.
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR/checkouts/$(printf '%s' "$repo_dir" | sha256sum | cut -c1-16)"
  export CARGO_TARGET_DIR
fi
