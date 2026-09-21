# `peak-view-photos`

Seven photographs of the Engelberg mountains, the skyline read off each one, and the terrain
shard the Peak View regression test draws. This is the only ground truth Peak View has.

`firmware/obc-app/tests/peak_view_photos.rs` is the only consumer. This file is the tracked copy
of the README the package carries.

## Contents

| Path | What it is |
| --- | --- |
| `engelberg.obcd` | OBCT v3 surface shard, box `46.66,7.86,47.19,8.92`, posting `2^9` µdeg |
| `photos/<name>.jpg` | The seven photographs, downscaled to fit a fixture package |
| `photos/<name>.json` | Position, ground height, camera and the read skyline of each one |

| View | Position | swissALTI3D ground | Camera |
| --- | --- | ---: | --- |
| `hahnen` | 46.80966, 8.42112 (Brunni) | 1022.9 m | 26 mm, heading 58.6°, pitch +12.7° |
| `rigidalstock-from-brunni` | 46.80966, 8.42112 | 1022.9 m | 26 mm, heading 10.6°, pitch +7.9° |
| `urnerstaffel` | 46.86881, 8.44342 | 1694.5 m | 26 mm, heading 141.8°, pitch +18.6° |
| `below-titlis` | 46.85212, 8.41904 | 2564.5 m | 48 mm, heading 186.3°, pitch −0.0° |
| `below-west` | 46.85212, 8.41904 | 2564.5 m | 48 mm, heading 226.7°, pitch +0.6° |
| `top-south` | 46.85227, 8.41959 | 2591.3 m | 48 mm, heading 150.6°, pitch −0.7° |
| `top-north` | 46.85227, 8.41959 | 2591.3 m | 48 mm, heading 307.5°, pitch −0.4° |

`photos/<name>.json`:

```json
{ "file": "hahnen.jpg", "lat": 46.80966, "lon": 8.42112, "ground_m": 1022.9,
  "heading_deg": 58.61, "pitch_deg": 12.71, "lens_mm": 26, "sky_floor": 0.95,
  "lidar_horizon_fraction": 1.0, "subject": "…",
  "columns": [[bearing_deg, elevation_deg], …], "rms_limit_deg": 0.38 }
```

`columns` is the skyline read off the photograph, in degrees clockwise from north and degrees
above the horizontal plane through the camera, on a quarter-degree bearing grid. `rms_limit_deg`
is the limit the test holds the drawn skyline to.

## Where the photographs come from

The owner took all seven near Engelberg, Switzerland, and gave the positions he stood at. The
EXIF was stripped in transit, so neither the lens nor the capture time is in the files.
`ground_m` is 2 m swissALTI3D at the position, which is the best figure available and the one the
comparison treats as truth.

## How the shard was baked

Copernicus GLO-30 tiles `N46_E007`, `N46_E008`, `N47_E007`, `N47_E008`, with a 2 m swissALTI3D
reference archive over 15 km around Engelberg (24 archive tiles, `4809–4812 / 4222–4227`,
ingested 2026-09-21):

```sh
obc-dem bake --sources <glo30 dir> --bbox 46.66,7.86,47.19,8.92 \
             --reference <archive root> --shard engelberg_native.obcd
obc-dem surface engelberg_native.obcd engelberg.obcd
```

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `engelberg_native.obcd` (not packaged) | 25 165 904 | `52594dc011918bf57e8f570b91859b92ec608e4527e62729596165e2035fa5b4` |
| `engelberg.obcd` | 37 798 912 | `f49c570cf0cfa4c37408f5e5183c07f25cd9f65fc878015622cc881c11ce233c` |

The bake lifts 83 516 samples, the largest by 391 m at 46.788608, 8.443904, and covers 79.2 % of
the box (Copernicus GLO-30 has no data over the box's lakes and its western corner).

## How each skyline was read

Once, with the segmentation and the camera model of `scratch/peak-view/tools` (deleted in the same
change; `git log` holds them):

1. The photograph is downscaled to 960 px on its long side and segmented into sky and ground: sky
   is markedly blue and no darker than `sky_floor` times the zenith. `sky_floor` has to be set per
   photograph, because hazy distant terrain and the sky above it differ by about 50 counts of blue
   in one of these frames and by 150 in another.
2. The camera is fitted **to the 2 m swissALTI3D skyline**, never to our own surface. Only the
   heading and the pitch are free; the roll is zero and the lens is one of the iPhone's three
   (13, 26 or 48 mm equivalent), chosen per photograph by the fit. A free four-parameter fit
   wanders 35° off on these hazy frames.
3. Each photo pixel column is inverted through that camera into a bearing and an elevation, and
   the columns are binned onto the quarter-degree grid by median.
4. A column survives only where the reference skyline agrees with the photograph's within
   **0.5°**. Everything else — cloud, haze, a shadowed gully the sky test walks into — is dropped.

Two caveats a reader should not have to rediscover:

- **The reference stops.** swissALTI3D was fetched for about 15 km around Engelberg, so a far
  horizon is Copernicus GLO-30. `lidar_horizon_fraction` in each file is the share of the cast
  bearings whose horizon the LiDAR found: 1.0 for the three valley views, 0.54 and 0.32 for the
  two south-west ones, 0.81 for `top-south` and **0.02** for `top-north`, whose horizon is the
  Mittelland 50 km away.
- **In `below-west` and `top-north` the line is a near ridge, not the far horizon.** Those two
  frames are hazy enough that the segmentation reads the distant range as sky and stops at the
  ridge in front of it. The columns are still a real terrain edge at a real angle, and the 0.5°
  rule keeps only the ones the reference agrees with, but their limits are looser for it.

## Licences and attribution

- **Photographs**: © Timo Hüser, CC BY 4.0.
- **`engelberg.obcd`**, Copernicus DEM GLO-30: produced using Copernicus WorldDEM-30 © DLR e.V.
  2010-2014 and © Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by the
  European Union and ESA; all rights reserved.
- **`engelberg.obcd`** crest lifts, swissALTI3D 2 m: © swisstopo. Open data, attribution required.
  Vertical datum LN02/LHN95.
- **`photos/<name>.json`**: project-authored, GPL-3.0-only, derived from the photographs and from
  the two elevation sources above.
