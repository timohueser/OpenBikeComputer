# Peak View ground truth — TEMPORARY

**Nothing in this directory is part of the build, the test suites or CI, and none of it is
supported.** It is the working material behind the crest-plane work: seven photographs the owner
took near Engelberg with the positions he was standing at, and the scripts that measured our
panorama against them. It is here so the evidence and the method survive the session that made
them, not because it belongs in the repository.

**Expected outcome:** the photographs and `photos.toml` become the input to a real Peak View
regression test, and the rest is deleted. When that test exists, delete this directory. Until
then, treat every script here as a one-off: several hard-code Engelberg, several assume a
container that only exists on the machine that baked it, and none has a test of its own.

Tracked in [#1928](https://github.com/timohueser/OpenBikeComputer/issues/1928).

## What is here

| Path | What it is |
| --- | --- |
| `photos/` | The seven photographs, downscaled to fit the repository's blob budget |
| `photos.toml` | Position, LiDAR ground height, heading, pitch and lens for each |
| `tools/sidebyside.py` | Photo, Peak View before and Peak View after over one angular window |
| `tools/photo_check.py` | Scores both containers against a camera solved on the LiDAR skyline |
| `tools/fit.py`, `tools/overlay.py` | Sky segmentation, the pinhole model and the camera fit |
| `tools/crestsim.py`, `tools/validate4.py` | The numpy crest simulation that chose the bakery rule |
| `pano/` | Rust binaries over the real reader: `pano`, `obctsky`, `hybridsky`, `demprobe`, … |

`pano/` is deliberately outside the Cargo workspace: its binaries reach into `firmware/` and
`host/` at once and exist only to ask questions about the panorama.

## What it needs that is not here

Everything large. Set `PEAKVIEW_WORK` (default `/tmp/peak-view`) and put in it:

- `dem/` — Copernicus GLO-30 tiles: `obc-dem fetch --bbox 46.66,7.86,47.19,8.92 --out $PEAKVIEW_WORK/dem`
- `lidar/` — swissALTI3D over the same area, from
  `host/obc-dem/reference/fetch_reference.py --source ch`
- `eng_plain.obcd`, `eng_crest.obcd` — `obc-dem bake` then `obc-dem surface`, once without
  `--reference` and once with `--reference $PEAKVIEW_WORK/lidar`

Then `cargo build --release` in `pano/` and `python3 tools/sidebyside.py`. The Python side wants
`numpy`, `scipy`, `pillow` and `rasterio`.

## What the photographs showed

The observer's own elevation was the larger half of the "the viewpoint looks too low" report.
On the Rigidalstock summit, Copernicus GLO-30 puts the rider 97 m below where he stood.

| Point | swissALTI3D | Before | After |
| --- | --- | --- | --- |
| 46.85212, 8.41904 (below the summit) | 2564.4 m | 2506.0 m | 2592.0 m |
| 46.85227, 8.41959 (on the summit) | 2591.4 m | 2506.0 m | 2592.0 m |

The two positions are 45 m apart and share one lattice cell, so after the fix they get the same
answer: right on the summit, 28 m high below it. A 57 × 39 m lattice cannot separate them.

## Caveats a reader should not have to rediscover

- **Sky segmentation needs a threshold per photo.** Hazy distant terrain and the sky above it
  differ by about 50 counts of blue in `rigidalstock-below-titlis` and by 150 in
  `rigidalstock-top-north`. No single value separates them in both, so `sky_floor` is per photo.
- **These four photographs have no usable skyline for an automatic camera fit.** The ridges are
  hazy and low in contrast, so `photo_check.py` wanders. `sidebyside.py` fits only heading and
  pitch, with the lens and the window fixed, which is stable.
- **The EXIF is gone**, so the lens is an assumption: the iPhone main camera, 26 mm equivalent.
- **The reference stops.** swissALTI3D was fetched for about 15 km around Engelberg, so the
  Bernese Alps on the far skyline are still Copernicus. At 40 km a 30 m error is 0.04 degrees.
