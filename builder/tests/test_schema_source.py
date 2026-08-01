"""Where ``/api/schema``'s two sources come from.

``builder/server/server.py`` serves the config JSON Schema from the obc-pack
binary when it is built, and from the checked-in repo copy otherwise. The repo
copy's *contents* are pinned on the Rust side (obc-pack's
``checked_in_schema_is_current_generated_schema``), but its **path** is built by
string-joining in Python — and that degrades silently. When obc-pack moved from
``firmware/`` to ``host/``, the binary branch kept working, ``os.path.exists``
quietly went False, and the fallback simply stopped firing; nothing failed. CI
builds obc-pack before running these tests, so the existing ``/api/schema``
coverage took the binary branch and never noticed. Hence a test for the path
itself, and one that forces the fallback branch with no binary in sight.

Run from the repo root with the uv-managed venv, e.g.::

    PYTHONPATH=. .venv/bin/python -m pytest builder/tests/
"""
import json
import os

import pytest


def _endpoint_json(response):
    return json.loads(bytes(response.body))


def test_schema_file_path_resolves():
    """SCHEMA_FILE must name a file that is actually there.

    This is the whole point of the module: the constant is assembled from path
    components, so a crate that moves leaves it pointing at nothing and the
    fallback dies without a sound.
    """
    pytest.importorskip("fastapi")
    from builder.server import server

    assert os.path.exists(server.SCHEMA_FILE), (
        f"SCHEMA_FILE points at {server.SCHEMA_FILE}, which does not exist — "
        "obc-pack's schema moved and server.py was not updated, so /api/schema's "
        "repo-file fallback can never fire."
    )
    schema = json.load(open(server.SCHEMA_FILE))
    # Shape check, not a content pin (obc-pack's Rust test owns the contents):
    # enough to catch pointing at some *other* JSON file that happens to exist.
    assert "$defs" in schema and "properties" in schema


def test_repo_file_fallback_serves_the_schema_without_a_binary(monkeypatch):
    """With no obc-pack built, /api/schema still answers — from the repo file.

    The editor derives its capability from this envelope, so an unbuilt packer
    must degrade to a usable schema rather than to a 503.
    """
    pytest.importorskip("fastapi")
    from builder.server import paths, server

    monkeypatch.setattr(paths, "rust_pack_bin", lambda: None)

    envelope = _endpoint_json(server.get_schema())
    assert envelope["source"] == "repo-file"
    assert envelope["schema_version"] == 1
    # No binary means no OBCM version to report; the editor renders this as "?".
    assert envelope["format_version"] is None
    assert envelope["schema"] == json.load(open(server.SCHEMA_FILE))
    # The fallback has to carry the parts the editor reads, not just any JSON.
    assert envelope["schema"]["properties"]["routing"]["default"]["profiles"]


def test_both_sources_agree_on_the_envelope_shape(monkeypatch):
    """Binary and repo-file envelopes carry the same keys.

    OutputTab.svelte reads schema_version, format_version and source off both
    without caring which produced it, so the fallback must not be a different
    shape from the real thing. Skipped when obc-pack isn't built.
    """
    pytest.importorskip("fastapi")
    from builder.server import paths, server

    if paths.rust_pack_bin() is None:
        pytest.skip("obc-pack binary not built")

    from_binary = _endpoint_json(server.get_schema())
    if from_binary["source"] != "binary":
        pytest.skip("obc-pack built, but `obc-pack schema` did not answer")

    monkeypatch.setattr(paths, "rust_pack_bin", lambda: None)
    from_file = _endpoint_json(server.get_schema())

    assert from_binary.keys() == from_file.keys()
    # And the checked-in copy is the schema the binary generates — the Rust
    # staleness test's claim, verified here through the endpoint that depends
    # on it, since the two now travel by different paths to get here.
    assert from_binary["schema"] == from_file["schema"]


def test_unbuilt_packer_says_where_to_build_it(monkeypatch):
    """With neither source available, the 503 names the repo root.

    The directory in this message is the same fact SCHEMA_FILE encodes, written
    out in prose — it said `firmware/` for as long as the path did.
    """
    pytest.importorskip("fastapi")
    from fastapi import HTTPException

    from builder.server import paths, server

    monkeypatch.setattr(paths, "rust_pack_bin", lambda: None)
    monkeypatch.setattr(server, "SCHEMA_FILE", "/nonexistent/config.schema.json")

    with pytest.raises(HTTPException) as excinfo:
        server.get_schema()
    assert excinfo.value.status_code == 503
    detail = excinfo.value.detail
    assert "obc web" in detail
    assert "OBC_PACK_BIN" in detail
    assert "firmware/" not in detail, f"stale build directory in the hint: {detail!r}"
