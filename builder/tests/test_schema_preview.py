"""Security and cancellation boundaries of the localhost schema lab."""

import asyncio
import json
from pathlib import Path

import pytest


def test_runtime_catalog_url_is_read_from_process_state(monkeypatch):
    pytest.importorskip("fastapi")
    from builder.server import server

    monkeypatch.setenv("OBC_CATALOG_URL", "https://maps.example.test/revision/catalog.json")
    body = json.loads(bytes(server.get_runtime().body))
    assert body == {"catalog_url": "https://maps.example.test/revision/catalog.json"}

    monkeypatch.setenv("OBC_CATALOG_URL", "https://token:secret@maps.example.test/catalog.json")
    with pytest.raises(server.HTTPException) as excinfo:
        server.get_runtime()
    assert excinfo.value.status_code == 503
    assert "credentials" in excinfo.value.detail


def test_source_is_server_configured_and_missing_state_uses_obc_commands(monkeypatch, tmp_path):
    from builder.server import schema_preview

    missing = tmp_path / "freiburg.osm.pbf"
    monkeypatch.setenv("OBC_SCHEMA_PREVIEW_PBF", str(missing))
    status = schema_preview.source_status()
    assert not status.available
    assert "obc web preview-source" in status.detail
    assert "cargo" not in status.detail

    monkeypatch.setenv("OBC_SCHEMA_PREVIEW_PBF", "relative.osm.pbf")
    status = schema_preview.source_status()
    assert not status.available
    assert "absolute path" in status.detail


def test_config_input_is_bounded_and_never_carries_paths_or_commands():
    from builder.server import schema_preview

    assert schema_preview.validate_config(b'{"lods":[],"features":{}}') == {"lods": [], "features": {}}
    for invalid in (b"[]", b'{"lods":[]}', b"not json"):
        with pytest.raises(schema_preview.PreviewError):
            schema_preview.validate_config(invalid)
    with pytest.raises(schema_preview.PreviewError, match="limit"):
        schema_preview.validate_config(b" " * (schema_preview.MAX_CONFIG_BYTES + 1))


def test_native_pack_has_fixed_argv_temp_outputs_and_no_shell(monkeypatch, tmp_path):
    from builder.server import schema_preview

    source = tmp_path / "source.osm.pbf"
    binary = tmp_path / "obc-pack"
    source.write_bytes(b"pbf")
    binary.write_bytes(b"bin")
    binary.chmod(0o755)
    monkeypatch.setenv("OBC_SCHEMA_PREVIEW_PBF", str(source))
    monkeypatch.setattr(schema_preview.paths, "rust_pack_bin", lambda: str(binary))
    captured = {}

    class Done:
        returncode = 0

        def __init__(self, argv, **kwargs):
            captured.update(argv=argv, kwargs=kwargs)
            Path(argv[3]).write_bytes(b"OBCM-map")

        def poll(self):
            return 0

    monkeypatch.setattr(schema_preview.subprocess, "Popen", Done)
    result = asyncio.run(schema_preview.pack_config({"lods": [], "features": {}}, _connected))

    argv = captured["argv"]
    assert argv[0] == str(binary)
    assert argv[1] == str(source)
    assert argv[-2:] == ["--bbox", schema_preview.TENINGEN_BBOX]
    assert captured["kwargs"]["shell"] is False
    assert captured["kwargs"]["stdin"] is schema_preview.subprocess.DEVNULL
    assert result.body == b"OBCM-map"
    assert not Path(argv[2]).exists() and not Path(argv[3]).exists(), "temporary config/map are removed"


def test_pack_failure_is_bounded_and_does_not_publish_partial_output(monkeypatch, tmp_path):
    from builder.server import schema_preview

    source = tmp_path / "source.osm.pbf"
    binary = tmp_path / "obc-pack"
    source.write_bytes(b"pbf")
    binary.write_bytes(b"bin")
    binary.chmod(0o755)
    monkeypatch.setenv("OBC_SCHEMA_PREVIEW_PBF", str(source))
    monkeypatch.setattr(schema_preview.paths, "rust_pack_bin", lambda: str(binary))

    class Failed:
        returncode = 7

        def __init__(self, _argv, **kwargs):
            kwargs["stderr"].write(b"invalid min_lod")
            kwargs["stderr"].flush()

        def poll(self):
            return 7

    monkeypatch.setattr(schema_preview.subprocess, "Popen", Failed)
    with pytest.raises(schema_preview.PreviewError, match="invalid min_lod"):
        asyncio.run(schema_preview.pack_config({"lods": [], "features": {}}, _connected))


def test_disconnected_request_terminates_the_native_pack(monkeypatch, tmp_path):
    from builder.server import schema_preview

    source = tmp_path / "source.osm.pbf"
    binary = tmp_path / "obc-pack"
    source.write_bytes(b"pbf")
    binary.write_bytes(b"bin")
    binary.chmod(0o755)
    monkeypatch.setenv("OBC_SCHEMA_PREVIEW_PBF", str(source))
    monkeypatch.setattr(schema_preview.paths, "rust_pack_bin", lambda: str(binary))
    process = None

    class Running:
        returncode = None
        stopped = False

        def __init__(self, _argv, **_kwargs):
            nonlocal process
            process = self

        def poll(self):
            return -15 if self.stopped else None

        def terminate(self):
            self.stopped = True
            self.returncode = -15

        def kill(self):
            self.stopped = True
            self.returncode = -9

        def wait(self, _timeout=None):
            return self.returncode

    checks = 0

    async def disconnect_after_start():
        nonlocal checks
        checks += 1
        return checks > 1

    monkeypatch.setattr(schema_preview.subprocess, "Popen", Running)
    with pytest.raises(schema_preview.PreviewCancelled):
        asyncio.run(schema_preview.pack_config({"lods": [], "features": {}}, disconnect_after_start))
    assert process is not None and process.stopped


async def _connected():
    return False
