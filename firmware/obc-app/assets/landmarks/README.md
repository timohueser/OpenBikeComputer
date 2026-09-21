# Landmark photo assets

Each file is a 216 × 240 row-major image, one RGB222 byte per pixel (`00_RR_GG_BB`, channel levels
0, 85, 170, 255). Each file is 51,840 bytes, with no header, palette or compression. The simulator
and the board display demo share these files and their renderer.

To reproduce the bytes, read the source PNG as RGB8 in row order and pack each pixel as
`((red / 85) << 4) | ((green / 85) << 2) | (blue / 85)` with integer division. The source PNGs are
the ordered 4 × 4 samples in the

## Credits

Each file is resized, padded and ordered-dithered from the source, and keeps the source licence.
The application shows these credits and URLs through Sources. No endorsement is implied.

| File | Photo | Source | Licence |
| --- | --- | --- | --- |
| `aare-large.rgb222` | *Aareschlucht 166 7*, Pazit Polak | [Commons](https://commons.wikimedia.org/wiki/File:Aareschlucht_166_7.jpg) | [CC BY-SA 2.0](https://creativecommons.org/licenses/by-sa/2.0/) |
| `falls-large.rgb222` | *Schattenhalb Reichenbachfall 7-05-2024 10-56-28*, Paul Hermans | [Commons](https://commons.wikimedia.org/wiki/File:Schattenhalb_Reichenbachfall_7-05-2024_10-56-28.jpg) | [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) |
| `dunlough-large.rgb222` | *2019-07-30-Dunlough Castle-0819*, Superbass / Wikimedia Commons | [Commons](https://commons.wikimedia.org/wiki/File:2019-07-30-Dunlough_Castle-0819.jpg) | [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) |
