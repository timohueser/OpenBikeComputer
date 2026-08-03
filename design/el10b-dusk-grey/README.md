# The dusk contour grey (#1095, EL10b)

`0xAD55` is the day skin's contour grey and the **dusk skin's street colour**, so dusk
needed its own value. `#1088` §5.4 left the choice open and never mocked it; these are
the frames it was made from. Two candidates, both unused elsewhere as a line colour and
both on the panel's RGB222 grid:

| candidate | what else uses it in dusk | verdict |
| :-- | :-- | :-- |
| **`0x52AA`** slate grey | buildings, subway, tram | **picked** |
| `0xAD4A` khaki | nothing | rejected |

**Why `0x52AA`.** It is the quietest neutral the dark skin has left, so the ladder
recedes the way `0xAD55` recedes on the day map: at planning zoom the nesting reads as
landform and the orange road and blue water stay on top of it. `0xAD4A` is warm, and on
black it comes *forward* — it reads as another line you could follow, joins the tan
track / amber road family, and half-vanishes where it crosses the olive vegetation fill.
That is §5.1's "the warm band is the trail palette" objection, transplanted to night.
Sharing a value with buildings is safe in the way §5.4 anticipated: a filled block and a
1 px dashed line are not confusable marks, and alpine contours and mapped buildings
essentially do not share a frame.

Baked from `~/obc-map-sources/grimsel.osm.pbf` at the canonical Grimsel bbox with the
dusk skin's values stamped over `builder/presets/schema.json`, then:

```sh
obc-sim dusk.obcm --center 8320000,46590000 --zoom 19.8076 --scale 3 --png dusk-…-wide.png
#   ride --zoom 44.567
```
