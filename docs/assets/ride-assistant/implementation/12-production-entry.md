# RA12 — Make the four reviewed screens the production Assistant

Parent: #1734. Depends on RA06, RA07, RA10 and RA11. This is consolidation and removal of mocks, not
a second implementation of their data or navigation logic.

Implementation dependencies: [RA06 — #1741](https://github.com/timohueser/OpenBikeComputer/issues/1741), [RA07 — #1742](https://github.com/timohueser/OpenBikeComputer/issues/1742), [RA10 — #1745](https://github.com/timohueser/OpenBikeComputer/issues/1745), [RA11 — #1746](https://github.com/timohueser/OpenBikeComputer/issues/1746).

## Entry and shared behavior

Make Ride Assistant a normal Up + Select top-drawer entry, independent of `assistant_demo` and its
Bluetooth substitution flag. Bluetooth remains in Settings. Keep Find a place, What's next, Easier
route and Landmarks active. Road blocked, Back on route and Worth a detour stay visibly inactive
placeholders. Next town stays absent. Do not remove the existing functional detour entry just to
direct the rider to the Road blocked placeholder.

Move discovery ownership into Assistant. Remove duplicate POI/Up Ahead menu/drawer entries only
after their full category/source/waypoint capabilities are reachable through Find a place or Explore
ahead. Update context drawer capabilities for production filter and Sources state, not mock flags.
Reuse existing `Screen`, `UiRuntime::prepare_base`, query scratch, navigation events,
list/pager/card vocabulary and i18n catalogue. Screens draw immutable prepared state.

Maintain Back/Up/Down/Select semantics, stable browsing and full source identity across all entry
paths. One selected-place details model and one visit preview serve Find a place, Explore ahead and
Landmarks. Authored route-waypoint details cannot offer Add stop again. Active visit details remain
reachable while the rider explores another question. Use existing shared loading/error patterns, no
per-screen spinner state machines or new event bus.

Port the accepted 240 × 320 layouts faithfully. Preserve grade colors, compact What's next facts,
large ordered-dither images, Sources drawer, and Easier route's centered card/table alignment. Use
normal units settings and supported UI languages rather than hard-coded English/metric mock strings.
Article language is separately source-defined; translating the UI does not translate it. Check
longest supported labels without reducing font sizes or clipping credits.

## Delete and isolate

Remove `Demo`, static Stops, fixed profile/timeline entries, synthetic alternative geometry,
compiled photo arrays, scripted arrival and temporary-route indexes from shipping behavior. Keep
small synthetic fixtures only in explicit tests or an opt-in design harness that cannot be mistaken
for production. Default simulator startup and fixture scenarios must open normal App code. Remove
stale CLI/panel controls or label remaining design tools clearly; do not leave them as the path
required to demonstrate the new features.

Update public conceptual docs and simulator/build/run READMEs. Preserve human-copy ownership. Do not
change unrelated Peak View, main recording semantics, automatic recovery policy or the three
placeholder features.

## Acceptance and verification

From ordinary startup, reach all four features through physical button gestures. Traverse category
and source filters, More places, custom-waypoint details, text/photo/Sources, accepted visit and
Easier preview, then return with stable selection. Confirm former Nearby/Up Ahead capabilities are
retained, Bluetooth remains reachable, existing detour works and placeholders do not navigate. Test
no active route, no fix, no content/coverage, error/cancel and active recording. Run affected
App/simulator/i18n suites and scoped Clippy. Spot-check named frames; RA13 owns the only final
snapshot sweep. No UI redesign or broad navigation rewrite is justified by this consolidation.
