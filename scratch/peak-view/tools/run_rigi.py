import os
import pathlib
import sys
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from photo_check import run, SP

ROOT = pathlib.Path(__file__).resolve().parent.parent
# Baked containers, reference DEMs and output images. They are large and are not in Git;
# ../README.md says how to make them.
WORK = os.environ.get("PEAKVIEW_WORK", "/tmp/peak-view")
U = str(ROOT / "photos")
BELOW = (46.85212, 8.41904)
TOP = (46.85227, 8.41959)
# The sky floor is per photo: hazy terrain and the sky above it differ by 50 counts of blue in
# `below-titlis` and by 150 in `top-north`, so no one value separates them in both.
CASES = {
    "below-titlis": (f"{U}/rigidalstock-below-titlis.jpg", BELOW, 187.0, 0.95, None),
    "below-west":   (f"{U}/rigidalstock-below-west.jpg", BELOW, 250.0, 0.95, None),
    "top-south":    (f"{U}/rigidalstock-top-south.jpg", TOP,   170.0, 0.95, None),
    "top-north":    (f"{U}/rigidalstock-top-north.jpg", TOP,   311.0, 0.72, None),
}
for name in sys.argv[1:]:
    photo, pos, yaw, floor, crop = CASES[name]
    run((name, photo, pos, yaw, floor, crop), f"{WORK}/lidar", f"{WORK}/dem", f"{WORK}/eng_plain.obcd", f"{WORK}/eng_crest.obcd")
