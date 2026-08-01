"""Bounded native packing for the localhost maintainer schema lab.

The product web and desktop hosts never import or expose this module.  The
FastAPI maintainer host accepts only a JSON config; the source, binary, crop,
command line, output path and resource bounds all remain server-owned.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.request

from . import paths


SOURCE_NAME = "europe_germany_baden-wuerttemberg_freiburg-regbez-latest.osm.pbf"
SOURCE_URL = (
    "https://download.geofabrik.de/europe/germany/"
    "baden-wuerttemberg/freiburg-regbez-latest.osm.pbf"
)

# The view at the coarsest authored scale is 7.2 x 9.6 km.  This crop leaves a
# little more than 20% padding around that 240 x 320 frame, while remaining a
# tiny fraction of the Freiburg extract.  The camera itself stays on Teningen.
TENINGEN_BBOX = "7.749,48.070,7.879,48.190"

MAX_CONFIG_BYTES = 512 * 1024
MAX_MAP_BYTES = 32 * 1024 * 1024
PACK_TIMEOUT_SECONDS = 180.0
POLL_SECONDS = 0.05


class PreviewError(Exception):
    """A safe, user-facing schema-preview failure."""


class PreviewCancelled(Exception):
    """The browser replaced or left a preview request."""


@dataclass(frozen=True)
class SourceStatus:
    available: bool
    label: str
    configured: bool
    detail: str


@dataclass(frozen=True)
class PackResult:
    body: bytes
    duration_ms: int
    log: str


def _default_source() -> Path:
    cache = Path(os.environ.get("OBCM_CACHE_DIR", Path.home() / ".cache" / "obcm"))
    return cache / "geofabrik" / SOURCE_NAME


def _source_candidate() -> tuple[Path, bool]:
    configured = os.environ.get("OBC_SCHEMA_PREVIEW_PBF")
    if configured:
        path = Path(configured)
        if not path.is_absolute():
            raise PreviewError("OBC_SCHEMA_PREVIEW_PBF must be an absolute path ending in .osm.pbf.")
        return path, True
    return _default_source(), False


def source_status() -> SourceStatus:
    try:
        candidate, configured = _source_candidate()
    except PreviewError as error:
        return SourceStatus(False, "configured preview source", True, str(error))

    if not candidate.name.endswith(".osm.pbf"):
        return SourceStatus(
            False,
            candidate.name or "configured preview source",
            configured,
            "The preview source must be an .osm.pbf file.",
        )
    if not candidate.is_file():
        return SourceStatus(
            False,
            candidate.name,
            configured,
            "Source not found. Run `obc web preview-source`, or set "
            "OBC_SCHEMA_PREVIEW_PBF to an existing absolute path in tools/obc.local.",
        )
    try:
        resolved = candidate.resolve(strict=True)
    except OSError as error:
        return SourceStatus(False, candidate.name, configured, f"Preview source cannot be opened: {error}.")
    if not resolved.is_file():
        return SourceStatus(False, candidate.name, configured, "Preview source is not a regular file.")
    return SourceStatus(True, resolved.name, configured, "Ready to pack the fixed Teningen crop.")


def source_path() -> Path:
    status = source_status()
    if not status.available:
        raise PreviewError(status.detail)
    candidate, _ = _source_candidate()
    return candidate.resolve(strict=True)


def validate_config(body: bytes) -> dict[str, object]:
    if len(body) > MAX_CONFIG_BYTES:
        raise PreviewError(f"The schema config exceeds the {MAX_CONFIG_BYTES // 1024} KiB preview limit.")
    try:
        config = json.loads(body)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PreviewError(f"The schema config is not valid JSON: {error}.") from error
    if not isinstance(config, dict):
        raise PreviewError("The schema config must be one JSON object.")
    if not isinstance(config.get("lods"), list) or not isinstance(config.get("features"), dict):
        raise PreviewError("The schema config must contain lods and features.")
    return config


def _pack_binary() -> Path:
    binary = paths.rust_pack_bin()
    if not binary:
        raise PreviewError("The native packer is missing. Stop and run `obc web` again to build it.")
    path = Path(binary)
    try:
        resolved = path.resolve(strict=True)
    except OSError as error:
        raise PreviewError(f"The configured native packer cannot be opened: {error}.") from error
    if not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise PreviewError("The configured native packer is not an executable file.")
    return resolved


async def _stop(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        await asyncio.to_thread(process.wait, 2.0)
    except subprocess.TimeoutExpired:
        process.kill()
        await asyncio.to_thread(process.wait)


_pack_lock = asyncio.Lock()


async def pack_config(
    config: dict[str, object],
    disconnected: Callable[[], Awaitable[bool]],
    *,
    timeout: float = PACK_TIMEOUT_SECONDS,
) -> PackResult:
    """Pack one fixed crop, cancelling promptly when its HTTP client goes away."""

    source = source_path()
    binary = _pack_binary()
    started = time.monotonic()

    async with _pack_lock:
        if await disconnected():
            raise PreviewCancelled()
        with tempfile.TemporaryDirectory(prefix="obc-schema-preview-") as directory:
            root = Path(directory)
            config_path = root / "schema.json"
            output_path = root / "teningen.obcm"
            stdout_path = root / "stdout.log"
            stderr_path = root / "stderr.log"
            config_path.write_text(json.dumps(config, separators=(",", ":")), encoding="utf-8")

            # No shell, no request-controlled argv, no request-controlled cwd or
            # paths.  stdout/stderr go to files so a chatty failure cannot fill a
            # pipe and deadlock the bounded poll loop.
            with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
                process = subprocess.Popen(
                    [
                        os.fspath(binary),
                        os.fspath(source),
                        os.fspath(config_path),
                        os.fspath(output_path),
                        "--bbox",
                        TENINGEN_BBOX,
                    ],
                    cwd=paths.REPO_ROOT,
                    stdin=subprocess.DEVNULL,
                    stdout=stdout,
                    stderr=stderr,
                    shell=False,
                )
                while process.poll() is None:
                    if await disconnected():
                        await _stop(process)
                        raise PreviewCancelled()
                    if time.monotonic() - started > timeout:
                        await _stop(process)
                        raise PreviewError(f"The Teningen preview pack exceeded its {int(timeout)} s time limit.")
                    await asyncio.sleep(POLL_SECONDS)

            stdout_text = stdout_path.read_text(encoding="utf-8", errors="replace")
            stderr_text = stderr_path.read_text(encoding="utf-8", errors="replace")
            log = "\n".join(part.strip() for part in (stdout_text, stderr_text) if part.strip())[-4000:]
            if process.returncode != 0:
                detail = log or f"native packer exited with status {process.returncode}"
                raise PreviewError(detail)
            try:
                size = output_path.stat().st_size
            except OSError as error:
                raise PreviewError(f"The native packer produced no preview map: {error}.") from error
            if size <= 0 or size > MAX_MAP_BYTES:
                raise PreviewError(
                    f"The preview map is {size} bytes; the allowed range is 1–{MAX_MAP_BYTES} bytes."
                )
            result = output_path.read_bytes()

    return PackResult(result, round((time.monotonic() - started) * 1000), log)


def download_source() -> Path:
    """Atomically cache the one fixed Geofabrik extract used by the lab."""

    target, _ = _source_candidate()
    if not target.name.endswith(".osm.pbf"):
        raise PreviewError("OBC_SCHEMA_PREVIEW_PBF must end in .osm.pbf.")
    if target.exists():
        if target.is_file():
            return target.resolve(strict=True)
        raise PreviewError(f"Refusing to replace non-file preview source {target}.")
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary_name = tempfile.mkstemp(prefix=f".{target.name}.", suffix=".part", dir=target.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(fd, "wb") as output, urllib.request.urlopen(SOURCE_URL, timeout=60) as response:
            while True:
                block = response.read(1024 * 1024)
                if not block:
                    break
                output.write(block)
            output.flush()
            os.fsync(output.fileno())
        if temporary.stat().st_size < 1024:
            raise PreviewError("The downloaded preview source is unexpectedly short.")
        os.replace(temporary, target)
        return target.resolve(strict=True)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise


def main() -> int:
    import argparse

    parser = argparse.ArgumentParser(description="Manage the fixed OBC schema-preview source")
    parser.add_argument("--download-source", action="store_true")
    args = parser.parse_args()
    if not args.download_source:
        parser.error("--download-source is required")
    try:
        target = download_source()
    except (OSError, PreviewError, urllib.error.URLError) as error:
        print(f"obc web preview-source: {error}", file=os.sys.stderr)
        return 1
    print(f"schema-preview source: {target}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
