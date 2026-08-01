"""Security and cancellation boundaries of the localhost schema lab."""

import asyncio
import json
from pathlib import Path
import subprocess

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


def test_runtime_catalog_proxy_rejects_other_origins_credentials_and_path_escape(monkeypatch):
    pytest.importorskip("fastapi")
    from builder.server import server

    monkeypatch.setenv("OBC_CATALOG_URL", "https://maps.example.test/cell-catalog/catalog.json")
    assert (
        server._catalog_object_url("https://maps.example.test/cell-catalog/cells/12/3.obcm")
        == "https://maps.example.test/cell-catalog/cells/12/3.obcm"
    )
    for hostile in (
        "https://other.example/cell-catalog/cells/12/3.obcm",
        "https://user:secret@maps.example.test/cell-catalog/cells/12/3.obcm",
        "https://maps.example.test/private/object",
        "https://maps.example.test/cell-catalog/%2e%2e/private",
    ):
        with pytest.raises(server.HTTPException) as excinfo:
            server._catalog_object_url(hostile)
        assert excinfo.value.status_code == 400


def test_runtime_catalog_proxy_bounds_reads(monkeypatch):
    pytest.importorskip("fastapi")
    from builder.server import server

    class Headers:
        def get(self, _name):
            return None

        def get_content_type(self):
            return "application/octet-stream"

    class Reply:
        headers = Headers()

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return None

        def read(self, limit):
            assert limit == server.MAX_CATALOG_OBJECT_BYTES + 1
            return b"catalog-object"

    seen = []

    def open_request(request, timeout):
        seen.append((request.full_url, timeout))
        return Reply()

    monkeypatch.setattr(server.urllib.request, "urlopen", open_request)
    body, content_type = server._fetch_catalog_object("https://maps.example.test/cell-catalog/object")
    assert body == b"catalog-object"
    assert content_type == "application/octet-stream"
    assert seen == [("https://maps.example.test/cell-catalog/object", 30)]


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
    source.write_bytes(b"pbf" * 512)
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
    assert result.diagnostics == ()
    assert not Path(argv[2]).exists() and not Path(argv[3]).exists(), "temporary config/map are removed"


def test_pack_failure_is_bounded_and_does_not_publish_partial_output(monkeypatch, tmp_path):
    from builder.server import schema_preview

    source = tmp_path / "source.osm.pbf"
    binary = tmp_path / "obc-pack"
    source.write_bytes(b"pbf" * 512)
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
    source.write_bytes(b"pbf" * 512)
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


def test_preview_source_is_prepared_atomically_with_smart_osmium(monkeypatch, tmp_path):
    from builder.server import schema_preview

    monkeypatch.delenv("OBC_SCHEMA_PREVIEW_PBF", raising=False)
    monkeypatch.setenv("OBCM_CACHE_DIR", str(tmp_path / "cache"))
    full = schema_preview._full_source()
    full.parent.mkdir(parents=True)
    full.write_bytes(b"source" * 1024)
    monkeypatch.setattr(schema_preview.shutil, "which", lambda name: "/usr/local/bin/osmium" if name == "osmium" else None)
    calls = []

    def run(argv, **kwargs):
        calls.append((argv, kwargs))
        output = Path(argv[argv.index("--output") + 1])
        output.write_bytes(b"prepared" * 1024)
        return subprocess.CompletedProcess(argv, 0, "", "")

    monkeypatch.setattr(schema_preview.subprocess, "run", run)
    target = schema_preview.prepare_source()

    assert target == (tmp_path / "cache/schema-preview/teningen-reference-complete.osm.pbf").resolve()
    assert target.read_bytes().startswith(b"prepared")
    argv, kwargs = calls[0]
    assert argv[:3] == ["/usr/local/bin/osmium", "extract", "--strategy=smart"]
    assert argv[argv.index("--bbox") + 1] == schema_preview.TENINGEN_BBOX
    assert "--output-format=pbf" in argv
    assert argv[-1] == str(full)
    assert kwargs["shell"] is False
    assert not list(target.parent.glob("*.part"))


def test_failed_source_preparation_leaves_no_partial_target(monkeypatch, tmp_path):
    from builder.server import schema_preview

    monkeypatch.delenv("OBC_SCHEMA_PREVIEW_PBF", raising=False)
    monkeypatch.setenv("OBCM_CACHE_DIR", str(tmp_path / "cache"))
    full = schema_preview._full_source()
    full.parent.mkdir(parents=True)
    full.write_bytes(b"source" * 1024)
    monkeypatch.setattr(schema_preview.shutil, "which", lambda _name: "/usr/bin/osmium")

    def fail(argv, **_kwargs):
        Path(argv[argv.index("--output") + 1]).write_bytes(b"partial")
        return subprocess.CompletedProcess(argv, 4, "", "bad polygon references")

    monkeypatch.setattr(schema_preview.subprocess, "run", fail)
    target = schema_preview._default_source()
    with pytest.raises(schema_preview.PreviewError, match="bad polygon references"):
        schema_preview.prepare_source()
    assert not target.exists()
    assert not list(target.parent.glob("*.part"))


def test_configured_preview_source_is_never_downloaded_to_or_overwritten(monkeypatch, tmp_path):
    from builder.server import schema_preview

    custom = tmp_path / "mine.osm.pbf"
    monkeypatch.setenv("OBC_SCHEMA_PREVIEW_PBF", str(custom))
    monkeypatch.setattr(
        schema_preview,
        "_download_full_source",
        lambda _target: pytest.fail("a configured custom target must never enter the downloader"),
    )
    with pytest.raises(schema_preview.PreviewError, match="not overwritten"):
        schema_preview.prepare_source()


def test_missing_osmium_uses_the_obc_doctor_remedy_before_downloading(monkeypatch, tmp_path):
    from builder.server import schema_preview

    monkeypatch.delenv("OBC_SCHEMA_PREVIEW_PBF", raising=False)
    monkeypatch.setenv("OBCM_CACHE_DIR", str(tmp_path / "cache"))
    monkeypatch.setattr(schema_preview.shutil, "which", lambda _name: None)
    monkeypatch.setattr(
        schema_preview,
        "_download_full_source",
        lambda _target: pytest.fail("dependency preflight must happen before a large download"),
    )
    with pytest.raises(schema_preview.PreviewError, match="obc doctor"):
        schema_preview.prepare_source()
