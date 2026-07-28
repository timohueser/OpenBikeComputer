"""Routing-profile round-trips through the web builder's pack path (N6).

These tests exercise the same `obc-pack` binary the builder API shells out to
(``packer/web_builder/jobs.py``): a config carrying custom ``routing.profiles``
is packed over the tiny corpus fixture, and the resulting §8.6 profile-table
bytes are compared against independently quantized expectations. A sub-1.0
multiplier must be rejected with the packer's admissibility error — the same
message the builder surfaces as a job failure. The pure API endpoints
(``/api/schema``, ``/api/presets``) are checked with FastAPI's TestClient so no
network is needed.

Run from the repo root with the uv-managed venv, e.g.::

    PYTHONPATH=. .venv/bin/python -m pytest packer/tests/
"""
import json
import os
import struct
import subprocess

import pytest

HERE = os.path.dirname(os.path.abspath(__file__))
PACKER_ROOT = os.path.dirname(HERE)
REPO_ROOT = os.path.dirname(PACKER_ROOT)
TINY_PBF = os.path.join(HERE, "corpus", "data", "tiny.osm.pbf")
DEFAULT_PRESET = os.path.join(PACKER_ROOT, "presets", "default.json")

# §8.6 canonical class order (must match obc-pack/src/nav.rs and OBCM_Spec §8.6).
HIGHWAY_CLASSES = [
    "cycleway", "path", "track", "footway", "steps", "bridleway", "living_street",
    "residential", "service", "unclassified", "tertiary", "secondary", "primary", "trunk_cycl",
]
SURFACE_CLASSES = ["unknown", "paved", "compacted", "gravel", "dirt", "rough", "cobbles", "grass"]
NAME_LEN = 12
PROFILE_RECORD_LEN = 52  # 12 name + 32 highway + 8 surface
OBCM_VERSION = 10


def _pack_bin():
    override = os.environ.get("OBC_PACK_BIN")
    if override and os.path.exists(override):
        return override
    for profile in ("release", "debug"):
        p = os.path.join(REPO_ROOT, "target", profile, "obc-pack")
        if os.path.exists(p) and os.access(p, os.X_OK):
            return p
    return None


requires_pack = pytest.mark.skipif(
    _pack_bin() is None or not os.path.exists(TINY_PBF),
    reason="obc-pack binary or tiny corpus fixture not built",
)


def _quantize(v):
    """Mirror obc-pack's quantize_multiplier: forbidden -> 0, else round(v*16) clamped 16..255."""
    if v == "forbidden":
        return 0
    q = round(v * 16)
    return max(16, min(255, q))


def _expected_record(profile):
    """Build the 52-byte §8.6 record we expect the packer to write for a profile."""
    default_q = _quantize(profile.get("default", 2.0))
    highway = [default_q] * 32
    surface = [default_q] * 8
    for cls, val in profile.get("highway", {}).items():
        highway[HIGHWAY_CLASSES.index(cls)] = _quantize(val)
    for cls, val in profile.get("surface", {}).items():
        surface[SURFACE_CLASSES.index(cls)] = _quantize(val)
    name = profile["name"].encode("utf-8")
    assert len(name) <= NAME_LEN
    name_field = name + b"\xff" * (NAME_LEN - len(name))
    return name_field + bytes(highway) + bytes(surface)


def _pack(tmp_path, profiles, min_component_edges=None):
    """Pack the tiny corpus with the given routing.profiles; return the .obcm bytes."""
    cfg = json.load(open(DEFAULT_PRESET))
    cfg.pop("_meta", None)
    routing = {"profiles": profiles}
    if min_component_edges is not None:
        routing["min_component_edges"] = min_component_edges
    cfg["routing"] = routing
    cfg_path = tmp_path / "config.json"
    cfg_path.write_text(json.dumps(cfg))
    out_path = tmp_path / "out.obcm"
    # --no-land avoids the network land-polygon dataset; the profile table is
    # written regardless of graph contents (§8.6 is always present).
    proc = subprocess.run(
        [_pack_bin(), TINY_PBF, str(cfg_path), str(out_path), "--no-land"],
        capture_output=True, text=True,
    )
    assert proc.returncode == 0, f"obc-pack failed: {proc.stdout}\n{proc.stderr}"
    return out_path.read_bytes()


def _profile_table(data):
    """Locate and slice the §8.6 profile table out of a packed .obcm."""
    assert data[:4] == b"OBCM", "bad magic"
    assert data[4] == OBCM_VERSION, f"expected OBCM v{OBCM_VERSION}, got v{data[4]}"
    (nav_off,) = struct.unpack_from("<I", data, 36)  # header: Nav Graph Offset @ 36
    (table_off,) = struct.unpack_from("<I", data, nav_off + 22)  # dir: Profile Table Offset @ 22
    count = data[nav_off + 26]  # dir: Profile Count @ 26 (u8)
    assert 1 <= count <= 8, f"profile count {count} out of range"
    end = table_off + count * PROFILE_RECORD_LEN
    return count, data[table_off:end]


CUSTOM = {
    "name": "TestBike",
    "default": 4.0,  # -> 64
    "highway": {"cycleway": 1.0, "track": 7.0, "steps": "forbidden", "primary": 2.5},
    "surface": {"paved": 1.0, "gravel": 5.0, "rough": "forbidden"},
}
MINIMAL = {"name": "Zwei", "default": 2.0}


@requires_pack
def test_custom_profiles_round_trip_to_86_bytes(tmp_path):
    """A config with custom profiles packs to §8.6 records that match byte-for-byte."""
    data = _pack(tmp_path, [CUSTOM, MINIMAL])
    count, table = _profile_table(data)
    assert count == 2
    rec0 = table[:PROFILE_RECORD_LEN]
    rec1 = table[PROFILE_RECORD_LEN:2 * PROFILE_RECORD_LEN]
    assert rec0 == _expected_record(CUSTOM)
    assert rec1 == _expected_record(MINIMAL)
    # Spot-check the load-bearing cells directly against the spec's encoding.
    assert rec0[NAME_LEN + HIGHWAY_CLASSES.index("cycleway")] == 16   # 1.0x
    assert rec0[NAME_LEN + HIGHWAY_CLASSES.index("steps")] == 0       # forbidden
    assert rec0[NAME_LEN + HIGHWAY_CLASSES.index("footway")] == 64    # inherits default 4.0x
    assert rec0[NAME_LEN + 32 + SURFACE_CLASSES.index("rough")] == 0  # forbidden


@requires_pack
def test_forbidden_primary_is_quantized_to_zero(tmp_path):
    """The acceptance-criterion shape: switching a class to forbidden zeroes its wire byte."""
    profile = {"name": "NoBigRoads", "default": 2.0, "highway": {"primary": "forbidden"}}
    data = _pack(tmp_path, [profile])
    _count, table = _profile_table(data)
    assert table[NAME_LEN + HIGHWAY_CLASSES.index("primary")] == 0


@requires_pack
def test_single_profile_count(tmp_path):
    data = _pack(tmp_path, [CUSTOM])
    count, table = _profile_table(data)
    assert count == 1
    assert table == _expected_record(CUSTOM)


@requires_pack
def test_sub_one_multiplier_is_rejected_with_admissibility_error(tmp_path):
    """A multiplier below 1.0 must be rejected by the packer with the admissibility error
    — the identical failure the builder API surfaces as a job error."""
    cfg = json.load(open(DEFAULT_PRESET))
    cfg.pop("_meta", None)
    cfg["routing"] = {"profiles": [{"name": "Bad", "highway": {"cycleway": 0.5}}]}
    cfg_path = tmp_path / "config.json"
    cfg_path.write_text(json.dumps(cfg))
    proc = subprocess.run(
        [_pack_bin(), TINY_PBF, str(cfg_path), str(tmp_path / "out.obcm"), "--no-land"],
        capture_output=True, text=True,
    )
    assert proc.returncode != 0, "a sub-1.0 multiplier must fail the pack"
    msg = proc.stdout + proc.stderr
    assert "below 1.0" in msg
    assert "admissible" in msg


# --- Pure builder-API endpoints (no network) --------------------------------
# Called directly (not via TestClient) so the suite needs no httpx — the
# handlers return FastAPI JSONResponses whose body we decode.

def _endpoint_json(response):
    return json.loads(bytes(response.body))


def test_schema_endpoint_exposes_routing_defaults():
    """The builder serves the canonical shipped profiles as the schema default —
    the single source of truth the frontend's 'Reset to defaults' reads."""
    pytest.importorskip("fastapi")
    from packer.web_builder import server

    schema = _endpoint_json(server.get_schema())["schema"]
    routing = schema["properties"]["routing"]
    names = [p["name"] for p in routing["default"]["profiles"]]
    assert names == ["Road", "Gravel", "MTB", "Touring"]
    hw_enum = schema["$defs"]["profile"]["properties"]["highway"]["propertyNames"]["enum"]
    assert hw_enum[:5] == HIGHWAY_CLASSES[:5]
    mult = schema["$defs"]["multiplier"]["oneOf"]
    assert any(o.get("minimum") == 1.0 for o in mult)
    assert any(o.get("const") == "forbidden" for o in mult)


def test_presets_round_trip_routing():
    """Every shipped preset carries a complete routing section (CLI-usable), so the
    builder shows its profiles when a preset is loaded."""
    pytest.importorskip("fastapi")
    from packer.web_builder import server

    presets = _endpoint_json(server.get_presets())
    assert presets, "no presets served"
    for p in presets:
        routing = p["config"].get("routing")
        assert routing is not None, f"preset {p['id']} lost its routing section"
        assert 1 <= len(routing["profiles"]) <= 8
        assert all("name" in prof for prof in routing["profiles"])
