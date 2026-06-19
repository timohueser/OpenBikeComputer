"""Background job runner: download selected PBFs, then run the native `obc-pack`.

Each job records an append-only list of events. The SSE endpoint replays the
list and follows new events, so reconnects and (single-user) multiple tabs both
work. PBF downloads are cached on disk and reused across runs.
"""
import os
import subprocess
import tempfile
import threading
import time
import uuid

import requests

from . import geofabrik

PBF_CACHE = os.path.expanduser("~/.cache/obcm/pbf")
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO_ROOT = os.path.dirname(PROJECT_ROOT)


def _rust_pack_bin():
    """Locate the native `obc-pack` packer binary, or None if it isn't built.

    Override the path with OBC_PACK_BIN; otherwise prefer the release build
    under firmware/target/ and fall back to debug. Build it with
    `cargo build --release -p obc-pack` in firmware/.
    """
    override = os.environ.get("OBC_PACK_BIN")
    if override:
        return override if os.path.exists(override) else None
    for profile in ("release", "debug"):
        p = os.path.join(REPO_ROOT, "firmware", "target", profile, "obc-pack")
        if os.path.exists(p) and os.access(p, os.X_OK):
            return p
    return None


# `obc-pack` prints these stage strings on stdout; they map to a coarse UI phase.
_STAGE_MARKERS = {
    "Merging": "merging",
    "Pass 1": "ingest",
    "Pass 2": "ingest",
    "Calculating BBox": "bbox",
    "Generating land": "land",
    "Building Quadtree": "quadtree",
    "Serializing": "serialize",
    "Writing": "serialize",
}


class Job:
    def __init__(self, region_ids, config, chunk_size, output_name, bbox=None):
        self.id = uuid.uuid4().hex[:12]
        self.region_ids = region_ids
        self.config = config
        self.chunk_size = chunk_size
        self.output_name = output_name
        self.bbox = bbox  # [west, south, east, north] in degrees, or None
        self.status = "queued"
        self.events = []
        self._lock = threading.Lock()
        self.finished = False

    def emit(self, event):
        with self._lock:
            self.events.append(event)

    def snapshot(self, start_index):
        with self._lock:
            return self.events[start_index:], len(self.events)


_JOBS = {}


def get_job(job_id):
    return _JOBS.get(job_id)


def create_job(region_ids, config, chunk_size, output_name, bbox=None):
    job = Job(region_ids, config, chunk_size, output_name, bbox)
    _JOBS[job.id] = job
    t = threading.Thread(target=_run, args=(job,), daemon=True)
    t.start()
    return job


def _download_pbf(job, region_id, url):
    os.makedirs(PBF_CACHE, exist_ok=True)
    dest = os.path.join(PBF_CACHE, f"{region_id}.osm.pbf")
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


def _crop_pbf(job, src_path, region_id, bbox, out_dir):
    """Crop one source PBF to `bbox` ([W, S, E, N], degrees) with `osmium extract`.

    Returns the cropped path, or None if the crop is empty / fails (e.g. the
    region doesn't actually reach into the box). Cropping each source up front
    keeps obc-pack from chewing through whole countries for a city-sized box.
    """
    w, s, e, n = bbox
    out = os.path.join(out_dir, f"{region_id}.crop.osm.pbf")
    cmd = [
        "osmium", "extract", "--overwrite",
        "-b", f"{w},{s},{e},{n}",
        src_path, "-o", out,
    ]
    job.emit({"type": "log", "line": "$ " + " ".join(cmd), "transient": False})
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        job.emit({"type": "log",
                  "line": f"crop {region_id} failed: {proc.stderr.strip()}",
                  "transient": False})
        return None
    if not os.path.exists(out) or os.path.getsize(out) == 0:
        return None
    job.emit({"type": "log",
              "line": f"Cropped {region_id} to box ({os.path.getsize(out)} bytes)",
              "transient": False})
    return out


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
    crop_dir = None
    try:
        job.status = "downloading"
        urls = geofabrik.region_pbf_urls(job.region_ids)
        pbf_paths = [_download_pbf(job, rid, url) for rid, url in urls]

        # Bounding-box build: crop each source PBF to the box before packing, so
        # obc-pack only sees the area of interest (and merges the crops as usual).
        if job.bbox:
            job.status = "cropping"
            job.emit({"type": "status", "status": "cropping", "detail": "cropping"})
            crop_dir = tempfile.mkdtemp(prefix="obcm-crop-")
            cropped = []
            for (rid, _url), src in zip(urls, pbf_paths):
                out = _crop_pbf(job, src, rid, job.bbox, crop_dir)
                if out:
                    cropped.append(out)
            if not cropped:
                raise RuntimeError(
                    "The bounding box does not overlap any of the selected regions' data."
                )
            pbf_paths = cropped

        # The native obc-pack binary is the only backend; fail fast (before any
        # temp file) with a clear message if it isn't built.
        rust_bin = _rust_pack_bin()
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

        out_name = job.output_name
        if not out_name.endswith(".obcm"):
            out_name += ".obcm"
        out_path = os.path.join(PROJECT_ROOT, os.path.basename(out_name))

        job.status = "converting"
        job.emit({"type": "status", "status": "converting", "detail": "starting"})

        cmd = [rust_bin, *pbf_paths, cfg_path, out_path,
               "--chunk-size", str(job.chunk_size)]
        job.emit({"type": "log", "line": "$ " + " ".join(cmd), "transient": False})

        proc = subprocess.Popen(
            cmd, cwd=PROJECT_ROOT, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, text=True, bufsize=1,
        )
        _stream_process(job, proc)
        rc = proc.wait()
        os.remove(cfg_path)

        if rc == 0 and os.path.exists(out_path):
            size = os.path.getsize(out_path)
            job.status = "done"
            job.emit({"type": "done", "output": os.path.basename(out_path),
                      "path": out_path, "size": size})
        else:
            job.status = "failed"
            job.emit({"type": "error", "message": f"obc-pack exited with code {rc}"})
    except Exception as exc:  # surface any failure to the browser
        job.status = "failed"
        job.emit({"type": "error", "message": str(exc)})
    finally:
        if crop_dir:
            import shutil
            shutil.rmtree(crop_dir, ignore_errors=True)
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
