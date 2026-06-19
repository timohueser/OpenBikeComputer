"""FastAPI app for the OBCM Web Builder."""
import json
import os

from fastapi import FastAPI, HTTPException
from fastapi.responses import FileResponse, JSONResponse, StreamingResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

from . import geofabrik, jobs

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STATIC_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "static")
# config.json ships with the repo and is the read-only factory default.
FACTORY_CONFIG = os.path.join(PROJECT_ROOT, "config.json")
# user_config.json (gitignored) holds the user's persisted edits, if any.
USER_CONFIG = os.path.join(PROJECT_ROOT, "user_config.json")

app = FastAPI(title="OBCM Web Builder")


class JobRequest(BaseModel):
    region_ids: list[str]
    config: dict
    chunk_size: int = 4096
    output_name: str


@app.get("/api/regions")
def get_regions():
    try:
        return JSONResponse(geofabrik.get_regions())
    except Exception as exc:
        raise HTTPException(status_code=502, detail=f"Failed to load Geofabrik index: {exc}")


def _read_config(path: str):
    with open(path) as f:
        return json.load(f)


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
    job = jobs.create_job(req.region_ids, req.config, req.chunk_size, req.output_name.strip())
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
    return FileResponse(os.path.join(STATIC_DIR, "index.html"))


app.mount("/static", StaticFiles(directory=STATIC_DIR), name="static")
