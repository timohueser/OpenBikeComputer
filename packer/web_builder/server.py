"""FastAPI app for the OBCM Web Builder."""
import json
import os

from fastapi import FastAPI, HTTPException
from fastapi.responses import HTMLResponse, JSONResponse, StreamingResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

from . import geofabrik, jobs

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STATIC_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "static")
# config.json ships with the repo and is the read-only factory default.
FACTORY_CONFIG = os.path.join(PROJECT_ROOT, "config.json")
# user_config.json (gitignored) holds the user's persisted edits, if any.
USER_CONFIG = os.path.join(PROJECT_ROOT, "user_config.json")
# palette.json ships with the repo: the device's 64-color gamut offered as the
# default color picker. Editable, with a generated fallback if it's missing.
PALETTE_FILE = os.path.join(PROJECT_ROOT, "palette.json")

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


@app.get("/api/config")
def get_config():
    """Return the user's persisted config if present, else factory defaults."""
    path = USER_CONFIG if os.path.exists(USER_CONFIG) else FACTORY_CONFIG
    return JSONResponse(_read_config(path))


@app.get("/api/config/factory")
def get_factory_config():
    """Return the read-only factory-default config (config.json)."""
    return JSONResponse(_read_config(FACTORY_CONFIG))


@app.put("/api/config")
def put_config(config: dict):
    """Persist the user's working config to user_config.json."""
    with open(USER_CONFIG, "w") as f:
        json.dump(config, f, indent=2)
    return {"ok": True}


@app.delete("/api/config")
def reset_config():
    """Discard user edits (delete user_config.json) and return factory defaults."""
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
    job = jobs.create_job(
        req.region_ids, req.config, req.chunk_size, req.output_name.strip(), req.bbox
    )
    return {"job_id": job.id}


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
