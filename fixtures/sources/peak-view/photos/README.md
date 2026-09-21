# `peak-view-photos`

Six photographs of the Engelberg mountains, the skyline read off each one, and the terrain shard
that `firmware/obc-app/tests/peak_view_photos.rs` draws. This is the only ground truth Peak View
has, and that test is the only consumer. This file is the tracked copy of the README the package
carries.

## Contents

| Path | What it is |
| --- | --- |
| `engelberg.obcd` | OBCT v3 surface shard, box `46.66,7.86,47.19,8.92`, posting `2^9` µdeg |
| `photos/<name>.jpg` | The six photographs, downscaled to fit a fixture package |
| `photos/<name>.json` | Position, ground height, camera and the read skyline of one photograph |

```json
{ "file": "hahnen.jpg", "lat": 46.80966, "lon": 8.42112, "ground_m": 1022.9,
  "heading_deg": 58.61, "pitch_deg": 12.71, "lens_mm": 26, "sky_floor": 0.95,
  "lidar_horizon_fraction": 1.0, "subject": "…",
  "columns": [[bearing_deg, elevation_deg], …], "rms_limit_deg": 0.38 }
```

`columns` is the skyline read off the photograph: degrees clockwise from north, and degrees above
the horizontal plane through the camera, on a quarter-degree bearing grid. `ground_m` is 2 m
swissALTI3D at the position, probed from the reference rasters, and the comparison treats it as
truth. `lidar_horizon_fraction` is the share of the cast bearings whose horizon the 2 m reference
found, rather than Copernicus beyond it. `rms_limit_deg` is the limit the test holds the drawn
skyline to.

## Bake the shard

Copernicus GLO-30 tiles `N46_E007`, `N46_E008`, `N47_E007` and `N47_E008`, with a 2 m swissALTI3D
reference archive over about 15 km around Engelberg:

```sh
obc-dem bake --sources <glo30 dir> --bbox 46.66,7.86,47.19,8.92 \
             --reference <archive root> --shard engelberg_native.obcd
obc-dem surface engelberg_native.obcd engelberg.obcd
```

The test asserts `engelberg.obcd`'s digest, because a repacked shard would move every limit at
once and without a word. `engelberg_native.obcd` is not packaged.

## How each skyline was read

1. The photograph is downscaled to 960 px on its long side and segmented into sky and ground: sky
   is markedly blue and no darker than `sky_floor` times the zenith. `sky_floor` is per
   photograph, because hazy distant terrain and the sky above it differ by about 50 counts of blue
   in one frame and by 150 in another.
2. The camera is fitted **to the 2 m swissALTI3D skyline**, never to our own surface. Only heading
   and pitch are free: roll is zero and the lens is one of the iPhone's three (13, 26 or 48 mm
   equivalent), chosen per photograph by the fit. A free four-parameter fit wanders 35° off on
   these hazy frames, so the heading is bounded to ±12° of a hint.
3. Each photo pixel column is inverted through that camera into a bearing and an elevation, and
   the columns are binned onto the quarter-degree grid by median.
4. A column survives only where the reference skyline agrees with the photograph within **0.5°**.
   Cloud, haze and a shadowed gully the sky test walks into are all dropped.

Two things will bite a reader of a failure:

- **The reference stops, and so does the box.** swissALTI3D was fetched for about 15 km around
  Engelberg, so a far horizon is Copernicus GLO-30. The shard's own box stops sooner in places:
  its west edge is 42.5 km from the Rigidalstock, nearer than the Bernese Alps on the south-west
  skyline.
- **In `below-west` the line is a near ridge, not the far horizon.** That frame is hazy enough
  that the segmentation reads the distant range as sky and stops at the ridge in front of it, and
  no `sky_floor` separates the two. The columns are still a real terrain edge at a real angle, and
  the 0.5° rule keeps only the ones the reference agrees with, but its `rms_limit_deg` is looser.

The owner took all six photographs near Engelberg and gave the positions he stood at. The EXIF was
stripped in transit, so neither the lens nor the capture time is in the files.

## Licences and attribution

- **Photographs**: © Timo Hüser, CC BY 4.0.
- **`engelberg.obcd`**, Copernicus DEM GLO-30: produced using Copernicus WorldDEM-30 © DLR e.V.
  2010-2014 and © Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by the
  European Union and ESA; all rights reserved.
- **`engelberg.obcd`** crest lifts, swissALTI3D 2 m: © swisstopo. Open data, attribution required.
  Vertical datum LN02/LHN95.
- **`photos/<name>.json`**: project-authored, GPL-3.0-only, derived from the photographs and from
  the two elevation sources above.
