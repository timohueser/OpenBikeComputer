# Ground-source answers

One small answer of the service of each national map in `data/trailview/ground.json`. The capture
and the units of `tools/trailview/sources.py` read them in `tools/trailview/tests/test_sources.py`,
with the service replaced by these files.

The Irish and Scottish answers are captured over a box of about 100 m. The ArcGIS answer asks for
`maxAllowableOffset=0.001` and `geometryPrecision=5`, so it stays small. The Spanish answer is
synthetic: it has the shape of a GEODE answer, and its units and descriptions are invented.

| Folder | Service and request | Licence |
| --- | --- | --- |
| `land-cover-ie` | `land-cover-ie` FeatureServer `query`: `ids.json` with `returnIdsOnly` over `-9.744,51.998,-9.742,51.999`, `features.geojson` of those ids with `OBJECTID` and `LEVEL_2_VALUE` | CC BY 4.0, © Tailte Éireann and the Environmental Protection Agency |
| `land-cover-gb-sct` | `land-cover-gb-sct` WFS `GetFeature` of `habitatsandspecies:habmosnvccleaned` over `57.021,-3.230,57.024,-3.226`: `page.json` | OGL v3, contains NatureScot information |
| `geology-es` | Synthetic: `ids.json` and `features.geojson` as the MapServer layer 8 `query` answers them, with `OBJECTID` and `DESC_UNIT` | Written for this repository |
