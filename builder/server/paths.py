"""Filesystem locations + job-queue limits, all env-overridable so a server
deployment can point caches and outputs anywhere without code changes. The
defaults keep today's local behavior (everything under ~/.cache/obcm)."""
import os

BUILDER_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO_ROOT = os.path.dirname(BUILDER_ROOT)

# Shared cache root: Geofabrik index, PBF downloads, and (by obc-pack's own
# convention) the land-polygon dataset.
CACHE_DIR = os.path.abspath(
    os.environ.get("OBCM_CACHE_DIR", os.path.expanduser("~/.cache/obcm"))
)
GEOFABRIK_CACHE = os.path.join(CACHE_DIR, "geofabrik")
PBF_CACHE = os.path.join(CACHE_DIR, "pbf")

# Build outputs: OUTPUT_DIR/<job_id>/<name>.obcm, served by
# /api/jobs/{id}/download — never written into the repo tree.
OUTPUT_DIR = os.path.abspath(
    os.environ.get("OBCM_OUTPUT_DIR", os.path.join(CACHE_DIR, "builds"))
)

# obc-pack is memory- and CPU-hungry, so builds run through a small worker
# pool (default: one at a time) with a bounded pending queue.
MAX_CONCURRENT_JOBS = max(1, int(os.environ.get("OBCM_MAX_CONCURRENT_JOBS", "1")))
MAX_PENDING_JOBS = int(os.environ.get("OBCM_MAX_PENDING_JOBS", "8"))

# Retention for finished builds: keep the most recent KEEP_JOBS, and nothing
# older than KEEP_JOB_SECONDS.
KEEP_JOBS = int(os.environ.get("OBCM_KEEP_JOBS", "20"))
KEEP_JOB_SECONDS = int(os.environ.get("OBCM_KEEP_JOB_SECONDS", str(24 * 3600)))


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
