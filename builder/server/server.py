"""Local maintainer host for the schema editor."""
import json
import os
import subprocess
from urllib.error import HTTPError, URLError
from urllib.parse import unquote, urlsplit
import urllib.request

from fastapi import FastAPI, HTTPException, Request
from fastapi.responses import HTMLResponse, JSONResponse, Response
from fastapi.staticfiles import StaticFiles

from . import paths
from . import schema_preview

PROJECT_ROOT = paths.BUILDER_ROOT
STATIC_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "static")
# The shipped style documents. Since #1036 this directory is one schema.json (a
# complete packer config + a _meta block) plus a skins/ subdirectory; only the schema
# can be handed to the packer, so only the top level is listed.
PRESETS_DIR = os.path.join(PROJECT_ROOT, "presets")
# palette.json ships with the repo: the device's 64-color gamut offered as the
# default color picker. Editable, with a generated fallback if it's missing.
PALETTE_FILE = os.path.join(PROJECT_ROOT, "palette.json")
# Generated repo copy of obc-pack's config schema — the fallback for /api/schema
# when the binary isn't built yet. A Rust stale-generation test pins the file's
# *contents*; test_schema_source.py pins this path, because a string-built path
# that stops resolving degrades silently (the fallback just never fires).
SCHEMA_FILE = os.path.join(paths.REPO_ROOT, "host", "obc-pack", "schema", "config.schema.json")

app = FastAPI(title="OBC Schema Editor")

MAX_CATALOG_OBJECT_BYTES = 128 * 1024 * 1024


def _catalog_url() -> str:
    value = os.environ.get(
        "OBC_CATALOG_URL",
        "https://maps.openbikecomputer.com/cell-catalog/catalog.json",
    )
    parsed = urlsplit(value)
    if (
        parsed.scheme not in ("http", "https")
        or not parsed.netloc
        or parsed.username
        or parsed.password
        or parsed.fragment
        or not parsed.path.endswith("/catalog.json")
    ):
        raise HTTPException(
            status_code=503,
            detail="OBC_CATALOG_URL must be an absolute http(s) URL ending in /catalog.json, without credentials.",
        )
    return value


def _catalog_object_url(value: str) -> str:
    root = urlsplit(_catalog_url())
    target = urlsplit(value)
    root_dir = root.path.rsplit("/", 1)[0].rstrip("/") + "/"
    decoded_path = unquote(target.path)
    if (
        target.scheme != root.scheme
        or target.netloc != root.netloc
        or target.username
        or target.password
        or target.fragment
        or not decoded_path.startswith(root_dir)
        or any(part == ".." for part in decoded_path.split("/"))
    ):
        raise HTTPException(status_code=400, detail="Catalog object URL is outside the configured catalog tree.")
    return value


def _fetch_catalog_object(url: str) -> tuple[bytes, str]:
    request = urllib.request.Request(url, headers={"User-Agent": "OpenBikeComputer maintainer host"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            _catalog_object_url(response.geturl())
            length = response.headers.get("Content-Length")
            if length and int(length) > MAX_CATALOG_OBJECT_BYTES:
                raise HTTPException(status_code=413, detail="Catalog object exceeds the local proxy limit.")
            body = response.read(MAX_CATALOG_OBJECT_BYTES + 1)
            if len(body) > MAX_CATALOG_OBJECT_BYTES:
                raise HTTPException(status_code=413, detail="Catalog object exceeds the local proxy limit.")
            return body, response.headers.get_content_type()
    except HTTPException:
        raise
    except HTTPError as error:
        raise HTTPException(status_code=error.code, detail=f"Published catalog returned HTTP {error.code}.") from error
    except (OSError, URLError, ValueError) as error:
        raise HTTPException(status_code=502, detail=f"Published catalog could not be read: {error}.") from error


@app.get("/api/catalog/root")
def get_catalog_root():
    url = _catalog_url()
    body, content_type = _fetch_catalog_object(url)
    return Response(content=body, media_type=content_type, headers={"X-OBC-Catalog-Url": url})


@app.get("/api/catalog/object")
def get_catalog_object(url: str):
    resolved = _catalog_object_url(url)
    body, content_type = _fetch_catalog_object(resolved)
    return Response(content=body, media_type=content_type)


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
    """List the shipped, bakeable style documents — since #1036 the one schema.
    Each entry carries the _meta fields plus the bare packer config (directly
    submittable / CLI-usable). Skins live in presets/skins/ and are deliberately
    absent: a skin is presentation stamped onto already-baked bytes and carries no
    LOD ladder, so it is not something this endpoint's consumer can build with."""
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
    presets.sort(key=lambda p: p["name"])
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
        detail="The native packer and checked-in schema are unavailable. Restart with `obc web`; "
               "maintainers may alternatively set OBC_PACK_BIN to an executable path.",
    )


@app.get("/api/schema-preview/status")
def get_schema_preview_status():
    status = schema_preview.source_status()
    return JSONResponse({
        "available": status.available,
        "label": status.label,
        "configured": status.configured,
        "detail": status.detail,
        "bbox": schema_preview.TENINGEN_BBOX,
    })


@app.post("/api/schema-preview")
async def build_schema_preview(request: Request):
    # Read one byte beyond the cap so an exact-limit body is accepted without
    # allowing an unbounded request to be decoded first.
    body = b""
    async for chunk in request.stream():
        body += chunk
        if len(body) > schema_preview.MAX_CONFIG_BYTES:
            raise HTTPException(status_code=413, detail="Schema preview config is too large.")
    try:
        config = schema_preview.validate_config(body)
        result = await schema_preview.pack_config(config, request.is_disconnected)
    except schema_preview.PreviewCancelled:
        return Response(status_code=499)
    except schema_preview.PreviewError as error:
        raise HTTPException(status_code=422, detail=str(error)) from error
    return Response(
        content=result.body,
        media_type="application/vnd.openbikecomputer.obcm",
        headers={
            "X-OBC-Pack-Duration-Ms": str(result.duration_ms),
            "X-OBC-Pack-Bytes": str(len(result.body)),
            "X-OBC-Pack-Diagnostics": schema_preview.encode_diagnostics(result.diagnostics),
        },
    )


# The SPA (builder/app/, built by Vite into static/dist/ —
# gitignored, so a fresh checkout needs one `npm run build`). Mounted last:
# every /api route above wins, everything else falls through to the app.
# Without a build, "/" explains how to produce one.
DIST_DIR = os.path.join(STATIC_DIR, "dist")

if os.path.exists(os.path.join(DIST_DIR, "index.html")):
    app.mount("/", StaticFiles(directory=DIST_DIR, html=True), name="app")
else:

    @app.get("/")
    def missing_dist():
        return HTMLResponse(
            "<!doctype html><meta charset='utf-8'><title>OBCM Web Builder</title>"
            "<body style='font-family: system-ui; max-width: 40rem; margin: 4rem auto;"
            " color: #24331c; background: #ece8cf; padding: 0 1rem;'>"
            "<h1>Frontend not built yet</h1>"
            "<p>The web builder's UI is compiled from <code>builder/app/</code>. "
            "Build it once (requires Node):</p>"
            "<pre>cd builder/app\nnpm ci\nnpm run build</pre>"
            "<p>…then restart this server.</p>",
            status_code=503,
        )
