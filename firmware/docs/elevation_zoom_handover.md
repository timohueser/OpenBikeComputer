# Elevation profile zoom

> **Status: DONE (2026-06-18, branch `ui-framework`).** The data layer is as planned; the
> **interaction model was revised after hands-on testing** (the user found a pan/zoom-toggle
> worse than a movable cursor). Final design:
>
> **Data — `obc-route` (`profile.rs`), as planned:**
> - **LOD pyramid**, base `PROFILE_COLS = 2048`: levels 2048/1024/512/256 (×4 B) + a
>   separate 256-col cumulative-ascent array = **exactly 16 KB resident**. Base size is the
>   one-line RAM/zoom knob. `cum_ascent` stays single-resolution (feeds only the live "to
>   climb", never the zoom draw).
> - **`Profile::window(center, zoom, target_px) -> Window{level, lo_frac, hi_frac}`** picks
>   the coarsest level holding ≥ `target_px` cols; the screen samples it per pixel via
>   `Profile::sample(level, frac)`. No geometry re-read on a detent.
>
> **Interaction — `obc-app` (`screen/statistics.rs`), REVISED from the plan below:**
> - **Cursor mode is the default** (restores Phase B's loved scrub): `turn` moves a cursor
>   along the *full* profile with a current-elevation readout; it **springs back to the live
>   position** after `IDLE_MS = 4000` idle.
> - **`hold` enters Zoom mode**; `turn` then zooms (`ZOOM_STEP` 1.2/detent, ≤ `MAX_ZOOM`
>   8×) **centred on the frozen cursor** — it does *not* spring back while zooming. A small
>   **magnifying-glass icon** marks the mode; **no zoom numbers or labels** (the level isn't
>   useful info — user's call). `hold` *or* short `back` exits, **springing back to the full
>   route + live cursor**.
> - **No pan mode** (the earlier toggle-to-pan + edge-chevrons + `N.Nx` pill were cut):
>   inspecting elsewhere = scrub the cursor, then zoom. State is local to `StatisticsScreen`.
> - **Spec** (`bikepacking-computer-ui-spec.md` §5) updated: Elevation `turn` = move cursor,
>   `hold` = enter/exit Zoom, + an "Elevation Zoom" bindings row.
> - **Verified:** `obc-route` pyramid/window unit tests + full `cargo test --workspace` +
>   clippy clean; headless snapshots — default cursor on-route (cursor at live + traveled
>   shading), scrub (cursor moved + altitude/grade readout), zoom mode (magnifying-glass
>   icon, profile zoomed about the frozen cursor). Gotcha: a scrub/zoom set via pre-replay
>   `--script` can't co-exist with a `--gpx` replay in one snapshot — the replay's `now_ms`
>   outruns the 4 s idle deadline, so the cursor springs back to live; snapshot scrub/zoom
>   without `--gpx`.
>
> The original plan (a Map-style Follow↔Pan toggle) follows for reference; the pan half was
> replaced by the cursor-first model above.

---

# (original plan) Next phase — Elevation profile zoom (after route-loading M2 Phase B)

A focused follow-up to **Phase B** (live map-matching). Phase B makes the Elevation
cursor follow the live matched position and leaves `turn` = a transient scrub placeholder.
This phase replaces that with **zoom**, using the interaction model the user specified —
deliberately mirroring the Map's Follow↔Pan toggle so the two riding views feel the same.

## Interaction model (decided with the user, 2026-06-18)

On the **Statistics** screen (`screen/statistics.rs`; was "Elevation" — renamed in the
Phase-B follow-ups):

- **`turn` (scroll) = zoom in / out** of the profile, centered on the live "you are here"
  position. Fully zoomed out = the whole route (today's view).
- **`hold` (long-press encoder) = toggle into pan/scrub mode** of the zoomed-in profile;
  in pan mode `turn` pans the window forward/back along the route.
- **`hold` again = back to zoom mode.**
- **Idle reset:** after a few seconds of no input, reset to the full-route view (and back
  to zoom mode). Same transient-inspection feel as the scrub snap-back in Phase B.

This matches the **Map** screen (`hold` toggles Follow↔Pan), so the gesture is intuitive
and consistent across the two riding views.

> Spec note: `bikepacking-computer-ui-spec.md` §5 currently lists Elevation `hold` as
> unbound/reserved. This phase binds it (zoom↔pan toggle), exactly as Map binds `hold` to
> Pan — update the spec table when this lands.

## Why it's cheap on the MCU (the question that prompted this design)

The naïve worry is that zooming re-runs `elevation_profile()` (an O(route) streaming pass)
on every detent — and at low zoom that could touch much of the route. The fix is to **not
re-read geometry for zoom at all**:

- Build a **multi-resolution profile (an LOD mip pyramid) once on load** — the same trick
  the OBCM **map** format already uses (v5). One streaming pass builds a **fine base**
  level; **coarser levels are pure min/max downsamples** of the finer one (merge adjacent
  column pairs — a few array passes, no extra chunk decodes).
- **Zoom = pick the level whose resolution matches the window + draw a sub-range.** Per-
  `turn` cost is the draw only (~chart-width columns), **flat across every zoom level**;
  zooming touches no geometry.
- The **finest** level caps zoom-in depth. If you ever want to zoom past it (very long
  route), that window is small by definition (a few chunks), so an **optional tiny local
  re-read** stays cheap. RAM is a few KB to low-tens-of-KB of `i16` pairs — trivial on the
  nRF54L15's 256 KB.

So the one real cost is the load-time build (same as today's single-resolution profile);
interaction is free. Compute is **not** the limiter — this is purely a UX feature.

## Sketch of the work

- `obc-route`: grow `Profile` (`src/profile.rs`) into a small pyramid — a finest-level
  `[(i16,i16); N]` plus downsampled coarser levels (or one fine array + a `cols_at(level)`
  view). Add `Profile::window(center_frac, zoom)` → the `(level, col_range)` to draw.
  Keep the build a single streaming pass + cheap downsamples; still built once on load and
  cached (the existing `App.profile` resident slot).
- `obc-app` `screen/statistics.rs`: add zoom state (zoom level + pan offset + an idle
  timer for the reset), rebind `turn` (zoom / pan by mode) and `hold` (toggle), and draw
  the selected window. The **current-elevation readout at the cursor** (a floating label
  like the peak label, kept clear of it) was already added in Phase B — it now reads the
  zoomed cursor.
- Idle reset reuses the same "snap back after a few seconds" timer Phase B introduces for
  the scrub cursor.

## Verify

`obc-route` unit tests for the pyramid (downsample correctness; `window()` ranges at a
few zoom levels). Headless Elevation snapshots at a couple of zoom levels + pan, and the
idle reset. `cargo test --workspace`, clippy clean.
