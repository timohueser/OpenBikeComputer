"""Repository locations used by the local maintainer schema editor."""
import os

BUILDER_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO_ROOT = os.path.dirname(BUILDER_ROOT)

def rust_pack_bin():
    """Locate the native `obc-pack` binary, or None if it isn't built.

    Override the path with OBC_PACK_BIN; otherwise prefer the release build
    under the workspace's target/ and fall back to debug. Build it with
    `cargo build --release -p obc-pack` from the repo root.
    """
    override = os.environ.get("OBC_PACK_BIN")
    if override:
        return override if os.path.exists(override) else None
    for profile in ("release", "debug"):
        p = os.path.join(REPO_ROOT, "target", profile, "obc-pack")
        if os.path.exists(p) and os.access(p, os.X_OK):
            return p
    return None
