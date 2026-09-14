# Landmark photo assets

Each file is a 216 × 240 row-major image, one RGB222 byte per pixel: `00_RR_GG_BB`.
Each channel has levels 0, 85, 170, and 255. Each file has 51,840 bytes, with no header,
palette table, or compression. These fixed demo assets do not define a map file contract.
The accepted image size is the large version; small comparison PNGs remain in the photo study.

The simulator and board display demo share these files and their renderer. The source PNGs
are the ordered 4 × 4 samples in the [photo study](../../../../docs/assets/ride-assistant/landmark-photos/README.md).
To reproduce the bytes, read the PNG as RGB8 in row order and pack each pixel as
`((red / 85) << 4) | ((green / 85) << 2) | (blue / 85)` using integer division.

- `aare-large.rgb222`: *Aareschlucht 166 7*, Pazit Polak. Resized, padded, and ordered dither.
  [Source](https://commons.wikimedia.org/wiki/File:Aareschlucht_166_7.jpg),
  [CC BY-SA 2.0](https://creativecommons.org/licenses/by-sa/2.0/).
- `falls-large.rgb222`: *Schattenhalb Reichenbachfall 7-05-2024 10-56-28*, Paul Hermans.
  Resized, padded, and ordered dither.
  [Source](https://commons.wikimedia.org/wiki/File:Schattenhalb_Reichenbachfall_7-05-2024_10-56-28.jpg),
  [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/).

- `dunlough-large.rgb222`: *2019-07-30-Dunlough Castle-0819*,
  Superbass / Wikimedia Commons. Resized, padded, and ordered dither.
  [Source](https://commons.wikimedia.org/wiki/File:2019-07-30-Dunlough_Castle-0819.jpg),
  [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/).

The adaptations retain each photo's respective licence. The application provides these
credits, source URLs, and licence URLs through Sources. No endorsement is implied.
