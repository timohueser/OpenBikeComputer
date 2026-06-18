## STATUS: DONE — 2026-06-18 (Terminus, not Spleen)

Shipped on branch `ui-framework`. The plan below is kept as historical context; what
actually landed:

- **Font: Terminus Bold**, not Spleen. Spleen was wired first but the user found it too
  thin/light — Terminus is the bolder, chunkier misc-fixed-lineage face they wanted, and
  ships large native cuts. Three tiers (all native 1×, crisp; `Font` names unchanged):
  | Tier | cut | cap | mm |
  |---|---|---|---|
  | `Label` | ter-u24 bold | 15 px | 2.03 |
  | `Body` | ter-u28 bold | 18 px | 2.44 |
  | `Display` | ter-u32 bold | 20 px | 2.71 |
  Targets were 2.0 mm (hit exactly) and 3.3 mm; Terminus tops out at 2.71 mm native and the
  user chose that crisp ceiling over a chunkier 2× upscale.
- **Converter:** [`fonts/convert_bdf.py`](../obcm-render/fonts/convert_bdf.py) (BDF →
  embedded-graphics 1bpp strip; `--scale N` integer upscale; `--deslash-zero` swaps the
  slashed `0` for the capital-`O` ring — the user's request). Assets +
  OFL license under `obcm-render/fonts/terminus/`; the seam is
  [`obcm-render/src/font_data.rs`](../obcm-render/src/font_data.rs) (no call sites changed).
- **Layout refit (all screens):** `title_frame` slimmed to a 34 px bar with a left-aligned,
  vertically-centred title (kills the header/readout collision). Menu rows → Body; Route
  menu rows sized so exactly 4 fit, content centred, long names truncated with ".."; Ride
  control + Map off-route pill grown for the taller glyphs. **Statistics** redesigned
  Wahoo-style to keep the 2×3 grid: unit-bearing caption on top + number-only big value,
  peak label dropped, profile compacted. Speeds show one decimal; distances drop it past
  100 km.
- Tests (`obcm-render/tests/text.rs`) + `--text-demo` updated; full workspace builds clean.

---

# Handover — custom pixel font (the text overhaul)

Replace the three `embedded-graphics` built-in mono stand-ins with a real **monospace
pixel font (Spleen)** at sizes chosen to hit physical-size legibility targets, then
re-fit every screen's layout to the bigger glyphs. This is the long-parked "font/text
overhaul (#6)". Decided with the user 2026-06-18; nothing is implemented yet.

## Why now (the forcing function)

The stand-ins are `embedded-graphics` ascii mono fonts, and the **largest is `10×20`** —
there is nothing bigger. The moment the *list* tier wants `10×20`, the big *values* tier
has nowhere to go. So a custom font is forced, and we do it **cohesively** (all tiers on
one typeface) rather than bolting a custom big font onto built-in small ones (two
typefaces would look wrong).

Current mapping in [`obcm-render/src/text.rs`](../obcm-render/src/text.rs):
`Font::Label → FONT_6X10`, `Font::Body → FONT_9X15`, `Font::Display → FONT_10X20`.

## Physical-size context (use the new 1:1 mode to verify)

The panel is **240 px over 32.46 mm → 7.39 px/mm** (1 px = 0.135 mm). The simulator now
has a **1:1 "actual size" mode** (commit `59cd08d`): calibrate once (`--calibrate`, or the
control-panel button), then `--physical` renders the device at true size. **Use it to eye
the font sizes at real-world scale** — that is the whole point of having built it.

Measured cap heights of the current stand-ins: `Label` ≈ 0.95 mm, `Body` ≈ 1.5 mm,
`Display` ≈ 1.9 mm. The `Label` captions (e.g. "Avg. speed" in Statistics) at ~0.95 mm are
what prompted this.

## Decisions locked with the user

- **Font:** Spleen (clean, legible, **monospace**, **BSD-2** license, ships **BDF**).
  Fallbacks if its native size steps are too coarse or you want more character: Cozette
  (MIT) or Tamzen.
- **Monospace**, not proportional — tabular figures keep speed/distance/time digits
  column-aligned (they don't twitch as values tick over), it suits the chunky grid, and it
  keeps `text_width` exact. Proportional is a much bigger lift (per-glyph width table) and
  explicitly out of scope for v1.
- **Three tiers**, smallest floor **1.5 mm** (the user relaxed from 1.8 mm — 1.5 mm is
  plenty readable and gives a nicer spread):

  | Tier (`Font`) | Target cap | = px (7.39 px/mm) | Role |
  |---|---|---|---|
  | `Label` | ~1.5 mm | ~11 px | stat captions, HUD titles |
  | `Body` | ~2.0 mm | ~15 px | menu / list rows |
  | `Display` | ~3.0 mm | ~22 px | big stat values |

  These are **targets** — pick the Spleen native sizes whose *measured* caps land closest
  (verify via `--text-demo`, below). Spleen sizes: 5×8, 6×12, 8×16, 12×24, 16×32, 32×64;
  the steps are coarse, so if none fits a tier well, options are: integer-scale a base
  Spleen size in the renderer, or switch to a font with finer steps (Cozette/Tamzen).

## Plan

### 1. Get + convert the font (no_std, shared)
- Fetch Spleen's BDF for the chosen sizes (BSD-2 — keep the license text with the asset).
- Convert each BDF → an `embedded-graphics` `MonoFont` (a 1bpp `ImageRaw` glyph strip + a
  `GlyphMapping`). Tooling: the embedded-graphics BDF-conversion path / a small `build.rs`,
  or hand-generate. Put the generated data in a new module, e.g.
  `obcm-render/src/font_data.rs` (static `&[u8]` strips + `MonoFont` consts). Bitmaps are a
  few KB each (~6 KB for 16×32), ~10–15 KB total — trivial on the nRF54L flash.
- Sanity-check each generated `MonoFont`'s `character_size` and `baseline`.

### 2. Swap in `text.rs` (the single point)
- Point `Font::Label/Body/Display::mono()` at the three Spleen fonts. `char_width()` /
  `line_height()` follow from `character_size` automatically.
- **Every screen already routes through `draw_text`/`text_width`** (via `Canvas::text`), so
  no call sites change — only this mapping. Keep the `Label`/`Body`/`Display` names.

### 3. Re-fit layouts (the actual work)
Bigger glyphs on 240×320 are tight. Verify each screen with `--script … --png` snapshots
*and* `--physical` 1:1. Touch points (grep `Font::` + the per-screen geometry consts):
- **`screen/statistics.rs` — the pressure point.** `tile()` (label `y+5`, value `y+17`,
  tile ≈ 40 px) will need taller tiles; the 2×3 grid + header + elevation profile may not
  all fit at 3 mm values. **Design call likely:** shrink the profile, fewer stats, or
  larger tiles with a smaller profile. Also the header (`Font::Label`, line ~141), peak
  label (~182), and cursor-elevation readouts (~200/202).
- **`screen/mod.rs`** — `title_frame` (Body title + Label counter; the 30 px wood strip may
  need to grow), `list_frame`, `LIST_TOP = 42`, `empty_state`, `scrollbar`, and
  `window_start` visible-row math (taller rows → fewer visible).
- **`screen/menu.rs`** (Display-font rows), **`route_menu.rs`** (name + km + climb panes),
  **`home.rs`**, **`ride_control.rs`** (Resume/Finish/Discard rows + the confirm ring).
- **`map.rs`** — the off-route pill (`Font::Body`). The pan HUD has no text (the compass
  uses no letter), so it's unaffected.

### 4. Verify + document
- **Update `--text-demo`** (`obcm-sim/src/main.rs` text-demo path) to show the new fonts and
  **annotate each with its cap height in mm** (so the 1.5 mm floor is checkable at a glance);
  render at `--physical` to judge true size.
- Re-run the headless screen snapshots for every touched screen.
- Update `obcm-render/tests/text.rs` (metrics + the device-64 quantization check still
  holds — the color path is unchanged, only the bitmap differs).
- Update `docs/ui_framework_brief.md` / the spec (font choice) and memory.

## Risks / open items
- **License**: confirm Spleen BSD-2 and ship its license text. (Gating.)
- **Spleen's coarse size steps** may miss a tier's target — fall back to integer-scaling or
  a finer-stepped font; decide after measuring.
- **Statistics layout** at 3 mm values is the one place that may not just "re-fit" — budget
  a small design decision there.

## Pointers
- Font seam: [`obcm-render/src/text.rs`](../obcm-render/src/text.rs) (swap here),
  `Canvas::text` in [`canvas.rs`](../obcm-render/src/canvas.rs).
- Tightest layout: [`obcm-app/src/screen/statistics.rs`](../obcm-app/src/screen/statistics.rs) `tile()`.
- Verify at true size: the 1:1 calibration mode (`--calibrate` then `--physical`),
  [`obcm-sim/src/calib.rs`](../obcm-sim/src/calib.rs).
- Snapshot workflow: `cargo run -p obcm-sim -- ../freiburg.obcm --script TOKENS --png OUT --scale N`
  (tokens `r`/`l`/`p`/`h`/`b`/`B`); `--text-demo` for the font preview.
