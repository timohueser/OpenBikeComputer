# Captured ground-source answers

One small answer of the service of each national map in `data/trailview/ground.json`. The capture
and the vector reader of `tools/trailview/sources.py` read them in
`tools/trailview/tests/test_sources.py`, with the service replaced by these files.

Each answer covers a box of about 100 m. The ArcGIS answers ask for `maxAllowableOffset=0.001` and
`geometryPrecision=5`, so they stay small.

| Folder | Service and request | Licence |
| --- | --- | --- |
| `land-cover-ie` | `land-cover-ie` FeatureServer `query`: `ids.json` with `returnIdsOnly` over `-9.744,51.998,-9.742,51.999`, `features.geojson` of those ids with `LEVEL_2_VALUE` | CC BY 4.0, © Tailte Éireann and the Environmental Protection Agency |
| `land-cover-gb-sct` | `land-cover-gb-sct` WFS `GetFeature` of `habitatsandspecies:habmosnvccleaned` over `57.021,-3.230,57.024,-3.226`: `page.json` | OGL v3, contains NatureScot information |
| `geology-es` | `geology-es` MapServer layer 8 `query`: `ids.json` with `returnIdsOnly` over `-4.856,43.194,-4.855,43.195`, `features.geojson` of those ids with `DESC_UNIT` | GEODE 1:50 000 © IGME |
