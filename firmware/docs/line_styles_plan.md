# Dashed & two-colour lines (railways, borders) — PLAN (STATUS: SHIPPED — superseded by epic #556)

> **Shipped as sub-issue #558 of epic [#556](https://github.com/timohueser/OpenBikeComputer/issues/556)**
> (OBCM **v10**, not the version this plan predates). Kept for design history; the `serialize.py` /
> `static/app.js` references are stale (now `obc-pack/src/serialize.rs` + the Svelte frontend), and
> the `lib.rs` line numbers below predate the refactor. **Trust the shipped code:** `Span.style_id`,
> `stroke_dashed` and `walk_dashes` in `../obc-render/src/stroke.rs`; `draw_line`'s dashed / railway
> dispatch there too. The readable tour is `../../docs/content/software/rendering.md` (§6 line styles).

**Part 2 of 3** of the line-rendering roadmap ([route arrows](route_arrows_plan.md) → line styles →
[road casing](road_casing_plan.md)). This part introduces the **secondary-colour + line-style style
field** in the OBCM format — infrastructure that **part 3 (road casing) reuses**, so build it here,
once. Land [part 1](route_arrows_plan.md)'s timing harness first so cost is measurable.

> **Note (post-refactor):** the renderer was refactored after this plan was written, so the
> `lib.rs` line numbers below are stale. The `Span`/`draw_line`/`draw_map` seams this plan targets
> are now factored out — see **[`draw_phase_onramp_plan.md`](draw_phase_onramp_plan.md)** for the
> current anchors and the `style_id`-in-`Span` + dispatch plan this builds on.

## Goal

Render lines that aren't flat single-colour strokes:

- **Dashed** single-colour lines — admin borders ([`admin_level.2`](../../packer/presets/default.json#L72), drawn solid
  today).
- **Two-colour dashed** lines — railways (dash/tie colour over a base), e.g.
  [`railway.rail`](../../packer/presets/default.json#L38) (black, weight 2, solid today).

## Context — what exists today (cited)

- Every line is a solid single-colour stroke: the `Kind::Line` arm of
  [`MapRenderer::render`](../obc-render/src/lib.rs#L478) → `Polyline::with_stroke(color, weight)`.
  **embedded-graphics 0.8 has no dash support** (the `dashArray` in the webapp is Leaflet/SVG preview,
  not eg) — dashing must be done by hand.
- **Style record = 6 bytes**, `struct "<BbHBB"` = `id, z_index(i8), color(u16 RGB565), weight(u8),
  flags(u8)` ([serialize.py `pack_style_dict`](../../obcm/serialize.py#L6),
  [reader.rs `parse_styles`](../obc-reader/src/reader.rs#L508), [`OBCM_Spec.md` §2](../../OBCM_Spec.md)).
  **flags bits 0–1 = priority; bits 2–7 are free.** There is **no second colour** field today.
- `Style` struct: [reader.rs:28](../obc-reader/src/reader.rs#L28). The draw-time `Span`
  ([lib.rs:349](../obc-render/src/lib.rs#L349)) caches the resolved `color`/`weight` but **not**
  `style_id` — it's 13 bytes used, padded to 14.
- Authoring: [`packer/presets/default.json`](../../packer/presets/default.json) is the source of truth; the webapp editor builds a
  per-style row (colour `<input type=color>`, weight, z, priority `<select>`) at
  [`app.js` ~L530–674](../../packer/web_builder/static/app.js#L530).

## Format change → **v6**

We must bump the format (the repo hard-cuts old versions; [reader.rs:176](../obc-reader/src/reader.rs#L176)
rejects `!= 5`). Add both fields the roadmap needs **now**, since casing wants the secondary colour too:

1. **Line-style** in flags **bits 2–3**: `0 = solid`, `1 = dashed`, `2 = railway` (two-colour dashed),
   `3 = reserved` (road casing, part 3, may take this or a separate bit). Reader:
   `line_style = (flags >> 2) & 0x03`.
2. **Secondary colour**: append `u16` RGB565 → record grows **6 → 8 bytes**, pack `"<BbHBBH"`. Semantics:
   dash/tie colour for railways; **casing colour for part 3**. A sentinel (e.g. `0x0000` meaning "unset")
   or defaulting to `color` keeps solid styles unaffected.

Touch list: [`OBCM_Spec.md` §2](../../OBCM_Spec.md) (version byte → 6, record layout), and the reader's
version gate (decide: hard-cut to 6 like v4→v5, or accept 5 & 6).

## Renderer

- **`Style`** ([reader.rs:28](../obc-reader/src/reader.rs#L28)) gains `line_style: u8` + `color2: u16`;
  `parse_styles` reads them ([reader.rs:524](../obc-reader/src/reader.rs#L524)).
- **Getting line-style + color2 to the draw loop.** The `Span` doesn't carry `style_id`. Two options:
  - **(a) zero-RAM:** add `style_id: u8` into the `Span`'s spare **padding byte** (13 used / 14 sized,
    so it's free) and look up `line_style`/`color2` from the style table at draw time.
  - **(b) simplest:** just add `line_style: u8` + `color2: u16` to the `Span` (≈ +9 KB across
    `MAX_SPANS`). RAM is **not** a hard limit (the 200 KB assert is a chosen fill-target — retune
    `MAX_SPANS`/`MAX_FRAME_POINTS` + LODs if needed), so this is acceptable; (a) is just cleaner.
- **`stroke_dashed()`** (new, in `lib.rs`): walk the **already-projected** screen points, accumulate
  arc-length in **screen pixels** (zoom-independent dash look), and emit short eg `Line` strokes for the
  "on" intervals in the dash colour. In the `Kind::Line` arm, switch on `line_style`:
  - `solid` → today's single `Polyline` (unchanged).
  - `dashed` → `stroke_dashed(color)` only.
  - `railway` → solid base pass in `color`, then `stroke_dashed(color2)` on top (the cheap, well-reading
    "light ties over dark rail" look). Optional **perpendicular crossties** instead of in-line dashes:
    at each tie position draw a short segment along the normal `(-dy, dx)` (same perpendicular trick as
    [`arm`](../obc-app/src/screen/map.rs#L353)) — more "railway", a bit more code.
  - Dash on/off lengths = screen-space consts; reuse the existing overflow guards.

## Authoring

- **`packer/presets/default.json`** (and the other presets): `railway.rail` → `line_style: "railway"`, `color2: "0x…"` (a light tie colour on the
  **RGB222 grid** — the panel is 64-colour, so the second colour must be a *visibly distinct* step, see
  `_meta.palette_note`). `admin_level.2` → `line_style: "dashed"`. **Also extend `obc-pack`'s
  `schema/config.schema.json` + the `schema_*` pinning tests in `config.rs` — the web builder gates its
  v6 editor columns on the schema served by the binary.**
- **`serialize.py` `pack_style_dict`** ([L6](../../obcm/serialize.py#L6)): map `s.get("line_style")` →
  flag bits, append `s.get("color2", color)` to the `struct.pack` (now `"<BbHBBH"`).
- **`packer/web_builder/static/app.js`** style row (~[L672](../../packer/web_builder/static/app.js#L672)): add a line-style
  `<select>` (solid/dashed/railway) + a 2nd-colour `<input type=color>` mirroring the existing colour
  control. Leaflet preview can reflect dashed with its own `dashArray` (already used cosmetically,
  [app.js:129](../../packer/web_builder/static/app.js#L129)).

## Cost

- **RAM:** format +2 bytes/style (×≤254 ≈ 0.5 KB, trivial). Draw path zero (option a) or ≈ +9 KB
  (option b, fine).
- **CPU:** negligible in aggregate — a railway is ~2× a plain line (base + dash pass), a dashed border is
  *less* than today (only on-portions drawn); both are a small fraction of features. Many tiny `Line`
  draws add per-call overhead but only on those features. Measure with part 1's harness if curious.

## Verify

Requires a **repack** of `freiburg.obcm` with the new config (≈ 213 MB, minutes — see `packer/pack.py`).
Then headless-render near a railway and an admin border:

```
cd firmware && cargo run -q -p obc-sim --release -- ../freiburg.obcm --size 240x320 --scale 3 \
  --center <lon>,<lat> --zoom <z> --png /tmp/lines.png
```

- Rail reads as two-colour dashed; border reads as dashed; solid styles unchanged.
- **`obc-reader/tests/format.rs`**: extend the byte builders for the **8-byte** record; round-trip
  `line_style` + `color2`. (These tests are the format contract — update them with the spec.)
- `cargo test --workspace` green; no_std crates build for `thumbv8m.main-none-eabihf`.

## Open questions

- Hard-cut to v6 or accept v5 & v6? (Repo precedent: hard-cut.)
- Railway: in-line dashes (cheap) vs perpendicular crossties (prettier) — decide by eye at 240 px.
- Dash on/off lengths; whether dashes scale at all with zoom or stay pure screen-space.
