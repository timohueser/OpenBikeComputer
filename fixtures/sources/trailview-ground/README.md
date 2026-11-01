# Ground-source answers

One small answer of a service of each national map in `data/trailview/ground.json`. The capture
and the units of `tools/trailview/sources.py` read them in `tools/trailview/tests/test_sources.py`.
The Spanish answer replaces its service. The tests write the Irish and Scottish answers into a
FileGDB and a GeoPackage in a zip, as the archive keeps these maps.

The Irish and Scottish answers are captured over a box of about 100 m. The ArcGIS answer asks for
`maxAllowableOffset=0.001` and `geometryPrecision=5`, so it stays small. The Spanish answer is
synthetic: it has the shape of a GEODE answer, and its units and descriptions are invented.

| Folder | Service and request | Licence |
| --- | --- | --- |
| `land-cover-ie` | The `MapGenieNationalLandCover2018ITM` FeatureServer `query`: `features.geojson` of the features over `-9.744,51.998,-9.742,51.999`, with `OBJECTID` and `LEVEL_2_VALUE` | CC BY 4.0, © Tailte Éireann and the Environmental Protection Agency |
| `land-cover-gb-sct` | The NatureScot WFS `GetFeature` of `habitatsandspecies:habmosnvccleaned` over `57.021,-3.230,57.024,-3.226`: `page.json` | OGL v3, contains NatureScot information |
| `geology-es` | Synthetic: `ids.json` and `features.geojson` as the MapServer layer 8 `query` answers them, with `OBJECTID` and `DESC_UNIT` | Written for this repository |
