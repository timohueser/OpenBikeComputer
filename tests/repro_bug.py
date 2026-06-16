import pytest
from obcm.ingest import ingest_osm
import os

def test_ingest_osm_filter_bug(tmp_path):
    pbf_file = tmp_path / "test.pbf"
    pbf_file.write_text("not a real pbf") # pyosmium will fail later, but we want to see the AttributeError first
    config = {"features": {}}
    
    try:
        ingest_osm(str(pbf_file), config)
    except AttributeError as e:
        assert "filter" in str(e)
    except Exception as e:
        # If it fails with something else, it might have passed the filter call
        print(f"Failed with: {type(e).__name__}: {e}")
