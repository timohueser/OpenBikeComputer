import pytest
from obcm.config import load_config, assign_style_ids, MAX_STYLE_ID


def test_load_valid_config(tmp_path):
    config_file = tmp_path / "config.json"
    config_file.write_text('{"features": {"highway": {"primary": {"z_index": 50, "color": "0xF9A6", "weight": 4}}}}')
    config = load_config(str(config_file))
    # IDs are auto-assigned at load time (1-based, document order).
    assert config["features"]["highway"]["primary"]["id"] == 1


def test_assign_style_ids_is_unique_and_overwrites():
    config = {
        "features": {
            "highway": {"primary": {"id": 99}, "service": {"id": 99}},
            "waterway": {"river": {}},
        }
    }
    assign_style_ids(config)
    ids = [
        config["features"]["highway"]["primary"]["id"],
        config["features"]["highway"]["service"]["id"],
        config["features"]["waterway"]["river"]["id"],
    ]
    # Colliding hand-written IDs are replaced by unique sequential ones.
    assert ids == [1, 2, 3]
    assert len(set(ids)) == len(ids)


def test_assign_style_ids_rejects_overflow():
    feats = {str(i): {} for i in range(MAX_STYLE_ID + 1)}
    config = {"features": {"highway": feats}}
    with pytest.raises(ValueError):
        assign_style_ids(config)
