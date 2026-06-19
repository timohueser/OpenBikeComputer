# User-position marker — handoff guide

> **STATUS: IMPLEMENTED.** All three parts landed: the format bumped **v3 → v4**
> (header field for the marker color, the recommended design in §3), the webapp
> gained a Marker color picker, and `MapRenderer::draw_marker` (chevron when
> moving, diamond when stationary) is wired into `App::render_frame`. `monaco.obcm`
> was re-packed to v4 from the cached PBF (the other root `*.obcm` are still v3 and
> now obsolete — re-pack them when needed, since the v4 reader rejects v3 by
> design). Tests added in `obcm/tests/format.rs` + `obcm/tests/marker.rs` +
> `obc-app/tests/marker.rs`; clippy clean; firmware target builds. The notes below
> are kept as the original brief (they describe the *pre-change* v3 layout).

**Read this first if you're picking up the "user-position marker" work.** It is a
self-contained brief: the goal, the *abstraction boundaries* (read §2 twice — it's
the whole point), the three parts (format color, webapp editor, shared renderer),
the exact APIs and file/line anchors, migration, and the verify workflow. You
should not need the original chat transcript.

This is the natural follow-up to the control panel + heading-up rotation (see
[`control-panel-handoff.md`](control-panel-handoff.md)). Heading rotation just
landed (commit `0e577fa`); the marker is roadmap item §9.1 there.

---

## 1. Goal

Draw the **user's position** on the map: a small triangle/chevron at
`AppState.user_fix`, pointing along the user's course. Plus a **small tweak to the
OBCM format and the webapp stylesheet editor so the map designer can set the
marker's color** (shape stays fixed for now — a later iteration may make it
configurable, so leave room but don't build it).

Behavior that should fall out for free (verify it does):
- **Heading-up** orientation → the marker points straight **up** (the map already
  rotated so the course is up).
- **North-up** orientation → the marker points along the **course**.
- **Follow** mode → the marker sits at screen center (camera tracks the fix).
- **Free** mode → the marker rides wherever the fix projects; **skip drawing when
  it's off-screen**.
- **No course** (stationary fix, `course == None`) → draw a non-directional dot,
  not a triangle. (The device's compass is a *separate* future sensor — see §7 of
  the control-panel handoff. Don't invent a heading here.)

---

## 2. Abstraction boundaries — THE crux (read carefully)

The marker has to render **byte-identically on the simulator and the real nRF54L
firmware**. They are two *hosts* behind a hardware-abstraction layer; the marker
must live entirely on the **shared** side so neither host reimplements it.

```
obc-sim (host)              firmware (host)        ← own ONLY: window/panel,
  Framebuffer DrawTarget       LS021B7DD02 driver      DrawTarget, color policy,
  color_of() policy            native color map        GPS/GPIO drivers
        └──────────────┬───────────────┘
                  obc-app::App           ← composes state + pixels: tick(),
                    AppState.user_fix        render_frame(). Marker WIRING lives here.
                       │
                  obcm (no_std)           ← Reader (marker color from the file),
                    Reader, MapRenderer       MapRenderer (the actual triangle
                    Viewport                  drawing). Marker GEOMETRY lives here.
```

Three hard rules, each a boundary you must not cross the wrong way:

1. **The marker is drawn in the shared path, never in a host.** Do **not** add
   marker drawing to `obc-sim/src/gui.rs` or `framebuffer.rs`. The host passes a
   `DrawTarget` and a `color_fn`; that's all it contributes. Both hosts already
   call `App::render_frame` — the marker rides along inside it.

2. **Dependency direction is `obc-sim → obc-app → obcm`.** `Fix`/`AppState` live
   in `obc-app`; `obcm` cannot see them. So the drawing primitive in `obcm` takes
   **plain numbers** (`lon: i32, lat: i32, course: Option<f32>, color`), and
   `obc-app` unpacks `self.state.user_fix` into those. Never make `obcm` depend on
   `obc-app`.

3. **The marker color flows through the same pipe as map style colors.** It is
   stored in the `.obcm` file (read by `Reader`, shared) and resolved to a device
   pixel via the host's `color_fn` (so it quantizes to 64-color on the device and
   stays true-color in the sim). Don't hardcode a color in the renderer, and don't
   pass a color down from the host — read it from the file like everything else.

The split, concretely:
- **`obcm`** — `Reader` exposes the marker color (from the file); `MapRenderer`
  gets a `draw_marker(target, vp, lon, lat, course, color)` that does the screen
  projection + triangle fill. Knows nothing about `Fix` or app state.
- **`obc-app`** — `App::render_frame`, after `MapRenderer::render`, reads
  `self.state.user_fix`, resolves the marker color via `color_fn`, and calls
  `draw_marker`. This is the *only* glue.
- **hosts** — unchanged. They already pass `target` + `color_fn` to `render_frame`.

---

## 3. Part A — marker color in the OBCM format

### Recommended design: a header field, bump v3 → v4
Store the marker color as a `uint16` (RGB565) **in the header**. It's a single,
global, map-presentation property — it belongs next to the bbox, not in the
feature **Style Table** (which is strictly "OSM feature type → render props"; the
marker is not a feature). This keeps both tables clean and gives the no_std reader
an O(1) read at a fixed offset, matching the format's "no runtime discovery"
principle.

**Header today** (`packer/obcm/serialize.py:215`, `reader.rs:166`): `struct "<4sBiiiiIBI"`,
**30 bytes**, version `0x03`:

| Off | Field | Type |
|--|--|--|
| 0 | Magic `OBCM` | char[4] |
| 4 | Version | u8 |
| 5..21 | bbox (lat,lon,lat,lon) | 4× i32 |
| 21 | Style Offset | u32 |
| 25 | LOD Count | u8 |
| 26 | LOD Table Offset | u32 |

**Header v4**: append `H` → `struct "<4sBiiiiIBIH"`, **32 bytes**, version `0x04`,
new field **Marker Color (u16 RGB565)** at offset 30. `Style Offset` becomes 32 and
everything after the header shifts +2 (the offsets are all stored, so the machinery
still works — see migration).

### Migration (no PBF sources are checked in — plan for offline)
`*.obcm` are all v3 and there are **no `.pbf` files in the repo**, so you can't just
re-pack. Two options:

- **(preferred for dev) A v3→v4 byte-shift upgrader** (~40 lines, no OSM data
  needed). For each file: set byte[4]=`4`; insert 2 bytes (the marker color) right
  after byte 30; then fix the stored offsets that now point 2 bytes later —
  `Style Offset`→32, `LOD Table Offset`+=2, and **each LOD entry's `Index Offset`
  +=2** (LOD table is `LOD Count` × 18-byte `"<fIIHI"` records; `Index Offset` is
  the 2nd field). Chunk starts are derived from `Index Offset` so they follow
  automatically; quadtree node values are chunk *indices* and feature anchors are
  node-relative, so neither changes. `monaco.obcm` (775 KB) is the one to convert
  for the sim; the big ones (kz 548 MB) can wait.
- **(clean, needs network) Re-pack** `monaco` from a fresh extract:
  `download monaco-latest.osm.pbf` (Geofabrik, ~1 MB) then
  `.venv/bin/python packer/pack.py monaco.osm.pbf config.json monaco.obcm` once the
  packer writes v4.

> **Alternative considered — a 2-byte trailer at EOF** (read `data[len-2..]`,
> version 4). Migration becomes a 3-line append (no offset rewriting) and the
> packer needs zero offset changes. It's tempting given no PBFs, but it deviates
> from the spec's front-loaded, explicit-offset layout. Pick it only if the
> upgrader's offset-shuffling feels too risky; otherwise prefer the header field.

### Files to touch (Part A)
- `packer/obcm/serialize.py`: `V3_HEADER_LEN = 30` (line 180) → add `V4_HEADER_LEN = 32`;
  `serialize_lods` (184) header pack (215) → `"<4sBiiiiIBIH"`, version `0x04`,
  append `marker_color`; recompute `lod_table_offset = V4_HEADER_LEN + len(style_data)`.
  Read the color from `config.get("marker", {}).get("color", "0xF800")`
  (accept `int` or `"0x…"` str, like `pack_style_dict` does at lines 20-22).
- `obcm/src/reader.rs`: `HEADER_LEN` 30→32 (line 17); accept `version == 4`
  (line 167); read `marker_color = rd_u16(data, 30)` (add a `rd_u16` helper beside
  `rd_u32`); add `pub marker_color: u16` to `Reader` (struct ~150) and set it in
  `new` (159). Bump `Style Offset`/`LOD Table Offset` validation as needed (the
  values come from the file, so just keep the bounds checks).
- `OBCM_Spec.md`: bump title/version to v4, header to 32 bytes + the new row, a
  short "Marker" note, and a version-history line (v3 dropped like v2 was).

Default color: `0xF800` (bright red) reads well over the blue sea (`0x001F`) and
pale land (`0xEFD5`). It's the designer's choice in the editor; this is only the
fallback when `config` has no `marker`.

---

## 4. Part B — marker color in the webapp stylesheet editor

The config object is `{ lods, features }` and round-trips through
`/api/config` (GET/PUT → `user_config.json`; factory default is `config.json`).
Add a top-level `marker: { color: "0xF800" }`.

Anchors in `packer/web_builder/static/app.js`:
- **Reuse the existing RGB565 helpers** `rgb565ToHex` (line 251) / `hexToRgb565`
  (260) and the `<input type="color">` pattern already used per-feature (615-626).
- `adoptConfig` (286): default it — `config.marker = config.marker || { color: "0xF800" }`.
- `buildExportConfig` (314): include it — `const out = { lods: config.lods, features: config.features, marker: config.marker }`.
- Add a small **"Marker"** block (a label + color input) near the global/LOD
  controls; `oninput` → set `config.marker.color = hexToRgb565(input.value)` then
  call the debounced save (`scheduleSave`, ~line 332).
- `config.py::assign_style_ids` (8) only walks `features`, so it already ignores
  `marker` — no change needed there. Confirm `jobs.py` passes the full config
  through to `serialize_lods` (it builds from `req.config`).

Keep it simple: **color only.** No shape/size UI now.

---

## 5. Part C — drawing the marker (shared renderer)

### `obcm/src/render.rs` — the drawing primitive
Add to `MapRenderer` (it already owns `screen: Vec<Point>` and `xs: Vec<f32>`
scratch, and a private `fill_polygon`):

```rust
/// Draw the user-position marker: a chevron at (lon,lat) pointing along `course`
/// (deg CW from north), or a dot when course is None. Screen-space size is fixed
/// (zoom-independent). Call AFTER `render` so it sits on top. Skips drawing when
/// the anchor projects outside the view.
pub fn draw_marker<D: DrawTarget>(
    &mut self, target: &mut D, vp: &Viewport,
    lon: i32, lat: i32, course: Option<f32>, color: D::Color,
) { /* ... */ }
```

Geometry guidance:
- Project the anchor: `let (sx, sy) = vp.to_screen(lon, lat);`. If it's outside
  `[0,w]×[0,h]` (small margin), return.
- **On-screen heading** — let the projection do the rotation bookkeeping so this
  works for north-up *and* heading-up without special cases: project a second
  point a small ground step ahead along the course and take the screen vector
  between them, then normalize. A robust ground step: move `k` µdeg of latitude
  north-ish along the course — `lat2 = lat + (cos θ)·k`, `lon2 = lon + (sin θ)·k /
  aspect` (use `vp.aspect`; θ = course.to_radians()), pick `k` so the projected
  delta is a few px, then normalize the screen delta to unit length. (Equivalent
  closed form: screen angle from up `= θ − vp.course_rad`. The two-point method is
  preferred — it stays correct if the projection ever changes.)
- Build a fixed-size triangle (~10–14 px) around `(sx,sy)` oriented along that unit
  vector and fill it via the existing `fill_polygon` (push 3 `Point`s, `ring_lens =
  &[3]`). For `course == None`, fill a small square/diamond (no orientation).
- Use `libm` for trig (no_std), as `aspect_for_lat`/rotation already do.

Don't put the marker inside `render()` — keep `render()` about *map data* and add
this as a separate overlay the app composes.

### `obc-app/src/app.rs` — the wiring (the only glue)
In `App::render_frame` (line 134), after `self.renderer.render(...)`:

```rust
if let Some(fix) = self.state.user_fix {
    let marker_color = color_fn(reader.marker_color());
    self.renderer.draw_marker(target, &vp, fix.lon, fix.lat, fix.course, marker_color);
}
```

`vp` is already built earlier in the method; `color_fn` is the same one used for the
backdrop and styles, so the marker quantizes correctly on the device. `Fix` is
already imported here. **No host changes** — `obc-sim` and the firmware call
`render_frame` exactly as before.

### A note on `user_fix` lifecycle
`AppState.update` records `user_fix` every tick in both modes (`app.rs:68`). In the
sim, the panel writes the fix (`SimLocationSource`); `--heading DEG` seeds a fix
*with a course* in both the GUI and `--png` paths — which means **`--heading 45
--png out.png` will now also render the marker**, a free headless visual check.
Consider always seeding a center fix in the `--png` path so the marker shows even
without `--heading` (optional).

---

## 6. Build / run / verify

Run cargo from `firmware/`. Sample maps are at the repo root (`../monaco.obcm`).

```sh
# After the format bump, convert monaco to v4 first (see §3 migration), else the
# v4 reader will reject the v3 file.

cd firmware
cargo build --release -p obc-sim
cargo clippy --workspace --all-targets        # must stay clean
cargo test --workspace                         # obcm 12, obc-app 10, obc-sim 7 (+ your new tests)
cargo build -p obc-app --target thumbv8m.main-none-eabihf   # firmware-readiness MUST hold

# Headless visual check (marker rides on the seeded fix):
./target/release/obc-sim ../monaco.obcm --true-color --heading 45 --png /tmp/m.png
# then Read /tmp/m.png — expect the chevron at screen center pointing up (heading-up).
```

Tests to add:
- **obcm**: a `format.rs` case asserting a v4 buffer's `marker_color` round-trips,
  and that the reader accepts v4 / rejects the old v3 (or document the chosen
  policy). Consider a `draw_marker` smoke test against a tiny mock `DrawTarget`
  (assert a pixel near the projected anchor gets the marker color).
- **obc-app**: `render_frame` draws the marker when `user_fix` is set and skips it
  when `None`; the dot-vs-chevron branch on `course`.

Gotchas / invariants to preserve:
- **`--png` north-up with no fix must stay byte-identical** to the pre-marker
  baseline (no fix ⇒ no marker ⇒ unchanged). Guard it like the rotation change did.
- **eframe 0.29 can't screenshot the Controls viewport** — verify the panel live
  (`cargo run`), not via `--screenshot` (that only grabs the main window).
- The marker is **fixed screen size** — it must not scale with zoom.

---

## 7. Decisions locked / open

**Locked**
- Marker drawing lives in the **shared renderer** (`obcm` draws, `obc-app` wires);
  hosts unchanged. (§2)
- Marker **color** is map-configurable via the file + webapp; **shape is fixed**
  for now.
- Color resolves through the host `color_fn` (device quantization), like styles.
- Position/course come from `AppState.user_fix`; on-screen orientation is derived
  *through the projection*, so heading-up vs north-up needs no special case.

**Open (decide as you implement — pick, note it, move on)**
- **Header field vs EOF trailer** for the color (§3). Recommendation: header field
  + the byte-shift upgrader. The trailer is the low-friction fallback.
- **Off-screen policy in Free mode**: skip entirely, or clamp-to-edge with an arrow
  (common nav-UI touch). Start with skip; clamp is a nice follow-up.
- **Stationary glyph**: a filled dot vs a dot-with-accuracy-ring. Start with a dot.
- Whether to also seed a center fix in `--png` unconditionally (nice for testing).

**Out of scope (future, but design so they slot in)**
- Configurable marker **shape/size** in the editor (you're leaving room, not
  building it).
- A dedicated **compass** sensor for stationary heading (separate optional trait).
- GPX replay driving the fix (a different `LocationSource`; the marker just works).
