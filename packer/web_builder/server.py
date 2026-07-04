"""FastAPI app for the OBCM Web Builder."""
import json
import os
import subprocess

from fastapi import FastAPI, HTTPException
from fastapi.responses import FileResponse, HTMLResponse, JSONResponse, StreamingResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

from . import geofabrik, jobs, paths

PROJECT_ROOT = paths.PACKER_ROOT
STATIC_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "static")
# Shipped style presets: complete packer configs + a _meta block, default first.
PRESETS_DIR = os.path.join(PROJECT_ROOT, "presets")
# The read-only factory default served to the legacy editor.
FACTORY_CONFIG = os.path.join(PRESETS_DIR, "default.json")
# user_config.json (gitignored) holds the user's persisted edits, if any.
USER_CONFIG = os.path.join(PROJECT_ROOT, "user_config.json")
# palette.json ships with the repo: the device's 64-color gamut offered as the
# default color picker. Editable, with a generated fallback if it's missing.
PALETTE_FILE = os.path.join(PROJECT_ROOT, "palette.json")
# Repo copy of obc-pack's embedded config schema — the fallback for /api/schema
# when the binary isn't built yet.
SCHEMA_FILE = os.path.join(paths.REPO_ROOT, "firmware", "obc-pack", "schema", "config.schema.json")

app = FastAPI(title="OBCM Web Builder")


class JobRequest(BaseModel):
    region_ids: list[str]
    config: dict
    chunk_size: int = 4096
    output_name: str
    # Optional [west, south, east, north] crop box (degrees). When present the
    # selected PBFs are cropped to it (bounding-box build mode); the region_ids
    # are then just the source regions that cover the box.
    bbox: list[float] | None = None


def _default_palette():
    """The LS021B7DD02's 64-color RGB222 gamut, laid out like obc-sim's --palette
    screen (8 cols, 2x2 of 4x4 red blocks). Used when palette.json is absent."""
    levels = [0, 85, 170, 255]
    colors = []
    for row in range(8):
        for col in range(8):
            r = levels[(row // 4) * 2 + (col // 4)]
            g = levels[row % 4]
            b = levels[col % 4]
            colors.append(f"#{r:02X}{g:02X}{b:02X}")
    return {"columns": 8, "colors": colors}


@app.get("/api/regions")
def get_regions():
    try:
        return JSONResponse(geofabrik.get_regions())
    except Exception as exc:
        raise HTTPException(status_code=502, detail=f"Failed to load Geofabrik index: {exc}")


def _read_config(path: str):
    with open(path) as f:
        return json.load(f)


@app.get("/api/palette")
def get_palette():
    """Return the device color palette (palette.json), or a generated default."""
    if os.path.exists(PALETTE_FILE):
        try:
            return JSONResponse(_read_config(PALETTE_FILE))
        except Exception:
            pass  # fall through to the generated gamut
    return JSONResponse(_default_palette())


@app.get("/api/presets")
def get_presets():
    """List the shipped style presets, default first. Each entry carries the
    _meta fields plus the bare packer config (directly submittable / CLI-usable)."""
    presets = []
    for fn in sorted(os.listdir(PRESETS_DIR)):
        if not fn.endswith(".json"):
            continue
        try:
            data = _read_config(os.path.join(PRESETS_DIR, fn))
        except Exception:
            continue  # a malformed preset shouldn't take the endpoint down
        meta = data.pop("_meta", {})
        presets.append({
            "id": meta.get("id", fn[:-5]),
            "name": meta.get("name", fn[:-5]),
            "description": meta.get("description", ""),
            "version": meta.get("version", 1),
            "swatch": meta.get("swatch", []),
            "config": data,
        })
    presets.sort(key=lambda p: (p["id"] != "default", p["name"]))
    return JSONResponse(presets)


# /api/schema cache, keyed on the binary's mtime so a rebuilt obc-pack (e.g.
# during v6 work) is picked up without a server restart.
_schema_cache = {"key": None, "envelope": None}


@app.get("/api/schema")
def get_schema():
    """The config JSON Schema envelope from the exact obc-pack binary that will
    pack — the editor derives its capability from this. Falls back to the repo
    schema file when the binary isn't built."""
    bin_path = paths.rust_pack_bin()
    if bin_path:
        key = (bin_path, os.path.getmtime(bin_path))
        if _schema_cache["key"] != key:
            try:
                out = subprocess.run([bin_path, "schema"], capture_output=True, text=True, timeout=10)
                if out.returncode == 0:
                    envelope = json.loads(out.stdout)
                    envelope["source"] = "binary"
                    _schema_cache.update(key=key, envelope=envelope)
            except Exception:
                pass  # fall through to the repo file
        if _schema_cache["key"] == key:
            return JSONResponse(_schema_cache["envelope"])
    if os.path.exists(SCHEMA_FILE):
        return JSONResponse({
            "schema_version": 1,
            "format_version": None,
            "schema": _read_config(SCHEMA_FILE),
            "source": "repo-file",
        })
    raise HTTPException(
        status_code=503,
        detail="obc-pack is not built — run `cargo build --release -p obc-pack` in "
               "firmware/ (or set OBC_PACK_BIN to its path).",
    )


@app.get("/api/config")
def get_config():
    """DEPRECATED (legacy editor): the user's persisted config, else the default preset."""
    path = USER_CONFIG if os.path.exists(USER_CONFIG) else FACTORY_CONFIG
    return JSONResponse(_read_config(path))


@app.get("/api/config/factory")
def get_factory_config():
    """DEPRECATED (legacy editor): the read-only factory-default config."""
    return JSONResponse(_read_config(FACTORY_CONFIG))


@app.put("/api/config")
def put_config(config: dict):
    """DEPRECATED (legacy editor): persist the working config to user_config.json."""
    with open(USER_CONFIG, "w") as f:
        json.dump(config, f, indent=2)
    return {"ok": True}


@app.delete("/api/config")
def reset_config():
    """DEPRECATED (legacy editor): discard user edits and return factory defaults."""
    if os.path.exists(USER_CONFIG):
        os.remove(USER_CONFIG)
    return JSONResponse(_read_config(FACTORY_CONFIG))


@app.post("/api/jobs")
def post_job(req: JobRequest):
    if not req.region_ids:
        raise HTTPException(status_code=400, detail="No regions selected")
    if not req.output_name.strip():
        raise HTTPException(status_code=400, detail="Output name is required")
    if req.bbox is not None and len(req.bbox) != 4:
        raise HTTPException(status_code=400, detail="bbox must be [west, south, east, north]")
    try:
        job = jobs.create_job(
            req.region_ids, req.config, req.chunk_size, req.output_name.strip(), req.bbox
        )
    except jobs.QueueFull as exc:
        raise HTTPException(status_code=429, detail=str(exc))
    return {"job_id": job.id}


@app.get("/api/jobs/{job_id}")
def job_state(job_id: str):
    """State snapshot — lets a reloaded page re-attach to a running build."""
    job = jobs.get_job(job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="Unknown job")
    return JSONResponse(job.public_state())


@app.get("/api/jobs/{job_id}/download")
def job_download(job_id: str):
    job = jobs.get_job(job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="Unknown job")
    if job.state != "done" or not os.path.exists(job.out_path):
        raise HTTPException(status_code=409, detail=f"Build is not finished (state: {job.state})")
    return FileResponse(
        job.out_path, filename=job.download_name, media_type="application/octet-stream"
    )


@app.get("/api/jobs/{job_id}/events")
def job_events(job_id: str):
    job = jobs.get_job(job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="Unknown job")
    return StreamingResponse(
        jobs.event_iterator(job),
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"},
    )


@app.get("/")
def index():
    # Serve index.html with mtime-stamped asset URLs. The plain `/static/app.js`
    # URL is otherwise cached heuristically by browsers and silently goes stale
    # after an edit (the page loads, but with last session's JS/CSS); stamping
    # `?v=<mtime>` makes every edit a fresh URL, so it always loads.
    with open(os.path.join(STATIC_DIR, "index.html")) as f:
        html = f.read()
    for asset in ("style.css", "app.js"):
        try:
            ver = int(os.path.getmtime(os.path.join(STATIC_DIR, asset)))
        except OSError:
            continue
        html = html.replace(f"/static/{asset}", f"/static/{asset}?v={ver}")
    return HTMLResponse(html, headers={"Cache-Control": "no-cache"})


app.mount("/static", StaticFiles(directory=STATIC_DIR), name="static")
