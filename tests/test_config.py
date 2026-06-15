import pytest
from obcm.config import load_config

def test_load_valid_config(tmp_path):
    config_file = tmp_path / "config.json"
    config_file.write_text('{"features": {"highway": {"primary": {"id": 10, "z_index": 50, "color": "0xF9A6", "weight": 4}}}}')
    config = load_config(str(config_file))
    assert config["features"]["highway"]["primary"]["id"] == 10
