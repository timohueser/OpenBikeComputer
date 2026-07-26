"""Background job runner: download selected PBFs, then run the native `obc-pack`.

Jobs enter a bounded FIFO queue served by a small worker pool
(OBCM_MAX_CONCURRENT_JOBS, default 1 — obc-pack is memory-hungry), so a burst of
requests can't fork a pile of packers. Each job records an append-only list of
events; the SSE endpoint replays the list and follows new events, so reconnects
and multiple tabs both work. PBF downloads are cached on disk and reused across
runs. Outputs land in a per-job directory under OBCM_OUTPUT_DIR and are served
by `GET /api/jobs/{id}/download`; finished jobs are swept by count and age.
"""
import os
import queue
import shutil
import subprocess
import tempfile
import threading
import time
import uuid

import requests

from . import geofabrik, paths


class QueueFull(Exception):
    """Raised by create_job when the pending queue is at capacity (HTTP 429)."""


# `obc-pack` prints these stage strings on stdout; they map to a coarse UI phase.
# Order matters to the UI: it derives a percentage from the phase's index, so a
# marker must never fire after a later-indexed one. "Merging" is a one-line
# announcement on the multi-region path, not a stage of its own: since #920 the
# merge happens *inside* the ingest passes rather than as an osmium step before
# them, so it prints once and "Pass 0/1/2" follow immediately. (The old
# "Cropping" marker is gone with the `osmium extract` call that printed it.)
_STAGE_MARKERS = {
    "Merging": "merging",
    "Pass 0": "ingest",
    "Pass 1": "ingest",
    "Pass 2": "ingest",
    "Calculating BBox": "bbox",
    "Generating land": "land",
    "Building Quadtree": "quadtree",
    "Serializing": "serialize",
    "Writing": "serialize",
}


def _sanitize_output_name(name):
    """A filesystem-friendly `.obcm` basename — the download filename."""
    base = os.path.basename((name or "").strip())
    base = "".join(c for c in base if c.isalnum() or c in "._- ").strip()
    if not base or base == ".obcm":
        base = "output.obcm"
    if not base.endswith(".obcm"):
        base += ".obcm"
    return base


class Job:
    def __init__(self, region_ids, config, chunk_size, output_name, bbox=None):
        self.id = uuid.uuid4().hex[:12]
        self.region_ids = region_ids
        self.config = config
        self.chunk_size = chunk_size
        self.bbox = bbox  # [west, south, east, north] in degrees, or None
        self.created_at = time.time()
        # Coarse lifecycle for /api/jobs/{id}; the SSE events carry the
        # fine-grained phase strings (downloading/merging/ingest/...).
        self.state = "queued"  # queued | running | done | error
        self.error = None
        self.download_name = _sanitize_output_name(output_name)
        self.out_dir = os.path.join(paths.OUTPUT_DIR, self.id)
        self.out_path = os.path.join(self.out_dir, self.download_name)
        self.events = []
        self._lock = threading.Lock()
        self.finished = False

    def emit(self, event):
        with self._lock:
            self.events.append(event)

    def snapshot(self, start_index):
        with self._lock:
            return self.events[start_index:], len(self.events)

    def public_state(self):
        """The `GET /api/jobs/{id}` snapshot: enough for a page reload to decide
        whether to re-follow the event stream or offer the download directly."""
        d = {
            "id": self.id,
            "state": self.state,
            "created_at": self.created_at,
            "output": self.download_name,
        }
        if self.state == "done" and os.path.exists(self.out_path):
            d["size"] = os.path.getsize(self.out_path)
            d["download_url"] = f"/api/jobs/{self.id}/download"
        if self.error:
            d["error"] = self.error
        return d


_JOBS = {}
_QUEUE = queue.Queue()
_workers_lock = threading.Lock()
_workers_started = False


def get_job(job_id):
    return _JOBS.get(job_id)


def _ensure_workers():
    global _workers_started
    with _workers_lock:
        if _workers_started:
            return
        for i in range(paths.MAX_CONCURRENT_JOBS):
            threading.Thread(target=_worker, daemon=True, name=f"obcm-build-{i}").start()
        _workers_started = True


def _worker():
    while True:
        job = _QUEUE.get()
        try:
            _run(job)
        finally:
            _QUEUE.task_done()


def _sweep():
    """Evict finished jobs beyond KEEP_JOBS or older than KEEP_JOB_SECONDS
    (deleting their output dirs), plus orphaned dirs from previous runs —
    the in-memory job table doesn't survive a restart, the disk does."""
    now = time.time()
    finished = sorted(
        (j for j in _JOBS.values() if j.finished),
        key=lambda j: j.created_at,
        reverse=True,
    )
    for i, job in enumerate(finished):
        if i >= paths.KEEP_JOBS or now - job.created_at > paths.KEEP_JOB_SECONDS:
            shutil.rmtree(job.out_dir, ignore_errors=True)
            _JOBS.pop(job.id, None)
    if os.path.isdir(paths.OUTPUT_DIR):
        for name in os.listdir(paths.OUTPUT_DIR):
            if name in _JOBS:
                continue
            p = os.path.join(paths.OUTPUT_DIR, name)
            try:
                orphaned = now - os.path.getmtime(p) > paths.KEEP_JOB_SECONDS
            except OSError:
                continue
            if orphaned:
                shutil.rmtree(p, ignore_errors=True)


def create_job(region_ids, config, chunk_size, output_name, bbox=None):
    _sweep()
    pending = sum(1 for j in _JOBS.values() if j.state == "queued")
    if pending >= paths.MAX_PENDING_JOBS:
        raise QueueFull(f"{pending} builds already queued — try again in a bit.")
    job = Job(region_ids, config, chunk_size, output_name, bbox)
    _JOBS[job.id] = job
    if pending:
        job.emit({"type": "status", "status": "queued", "detail": f"position {pending + 1}"})
    _ensure_workers()
    _QUEUE.put(job)
    return job


def _download_pbf(job, region_id, url):
    os.makedirs(paths.PBF_CACHE, exist_ok=True)
    dest = os.path.join(paths.PBF_CACHE, f"{region_id}.osm.pbf")
    if os.path.exists(dest) and os.path.getsize(dest) > 0:
        job.emit({"type": "log", "line": f"Using cached PBF for {region_id}", "transient": False})
        return dest

    job.emit({"type": "status", "status": "downloading", "detail": region_id})
    tmp = dest + ".part"
    with requests.get(url, stream=True, timeout=120) as resp:
        resp.raise_for_status()
        total = int(resp.headers.get("Content-Length", 0))
        done = 0
        last_pct = -1
        with open(tmp, "wb") as f:
            for chunk in resp.iter_content(chunk_size=1 << 16):
                f.write(chunk)
                done += len(chunk)
                if total:
                    pct = int(done * 100 / total)
                    if pct != last_pct:
                        last_pct = pct
                        job.emit({"type": "progress", "phase": "download",
                                  "region": region_id, "pct": pct})
    os.replace(tmp, dest)
    job.emit({"type": "log", "line": f"Downloaded {region_id} ({done} bytes)", "transient": False})
    return dest


def _stream_process(job, proc):
    """Read proc output, splitting on both \\n and \\r so tqdm bars surface as
    transient log lines while normal prints are committed lines."""
    buf = ""
    while True:
        ch = proc.stdout.read(1)
        if ch == "" and proc.poll() is not None:
            break
        if ch in ("\n", "\r"):
            line = buf
            buf = ""
            if line.strip():
                phase = next((p for marker, p in _STAGE_MARKERS.items() if marker in line), None)
                job.emit({"type": "log", "line": line.rstrip(), "transient": (ch == "\r")})
                if phase:
                    job.emit({"type": "status", "status": "converting", "detail": phase})
        else:
            buf += ch
    if buf.strip():
        job.emit({"type": "log", "line": buf.rstrip(), "transient": False})


def _run(job):
    job.state = "running"
    try:
        urls = geofabrik.region_pbf_urls(job.region_ids)
        pbf_paths = [_download_pbf(job, rid, url) for rid, url in urls]

        # The native obc-pack binary is the only backend; fail fast (before any
        # temp file) with a clear message if it isn't built.
        rust_bin = paths.rust_pack_bin()
        if rust_bin is None:
            raise RuntimeError(
                "obc-pack binary not found — build it with "
                "`cargo build --release -p obc-pack` in firmware/ "
                "(or set OBC_PACK_BIN to its path)."
            )

        # Write the editor's config to a temp file for obc-pack.
        cfg_fd, cfg_path = tempfile.mkstemp(suffix=".json", prefix="obcm-config-")
        with os.fdopen(cfg_fd, "w") as f:
            import json
            json.dump(job.config, f)

        os.makedirs(job.out_dir, exist_ok=True)
        job.emit({"type": "status", "status": "converting", "detail": "starting"})

        # Several regions are handed over as-is: obc-pack merges them inside its
        # own ingest passes (#920), so there is no `osmium` on the box and no
        # merged intermediate `.pbf` in the cache. On a duplicate id — adjacent
        # regions share their border — the FIRST region listed wins, which is the
        # order `region_pbf_urls` returned them in.
        cmd = [rust_bin, *pbf_paths, cfg_path, job.out_path,
               "--chunk-size", str(job.chunk_size)]
        # Bounding-box build: obc-pack crops during ingest (issue #910), so no
        # `osmium extract` step and no temporary cropped PBFs on disk. It also
        # errors out itself, with the box in the message, when the box misses the
        # selected regions entirely.
        if job.bbox:
            w, s, e, n = job.bbox
            cmd += ["--bbox", f"{w},{s},{e},{n}"]
        job.emit({"type": "log", "line": "$ " + " ".join(cmd), "transient": False})

        proc = subprocess.Popen(
            cmd, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, text=True, bufsize=1,
        )
        _stream_process(job, proc)
        rc = proc.wait()
        os.remove(cfg_path)

        if rc == 0 and os.path.exists(job.out_path):
            size = os.path.getsize(job.out_path)
            job.state = "done"
            job.emit({"type": "done", "output": job.download_name, "size": size,
                      "download_url": f"/api/jobs/{job.id}/download"})
        else:
            raise RuntimeError(f"obc-pack exited with code {rc}")
    except Exception as exc:  # surface any failure to the browser
        job.state = "error"
        job.error = str(exc)
        job.emit({"type": "error", "message": str(exc)})
    finally:
        job.finished = True


def event_iterator(job, poll=0.2):
    """Yield SSE-formatted strings for a job, replaying history then following."""
    import json
    idx = 0
    while True:
        new, idx = job.snapshot(idx)
        for ev in new:
            yield f"data: {json.dumps(ev)}\n\n"
        if job.finished:
            # Flush any final events that arrived between snapshot and finish.
            new, idx = job.snapshot(idx)
            for ev in new:
                yield f"data: {json.dumps(ev)}\n\n"
            break
        time.sleep(poll)
