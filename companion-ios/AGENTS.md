# iOS companion rules

Build and test instructions are in [README.md](README.md).

## UI source of truth

The tracked SwiftUI implementation is authoritative. Reuse `OBCTheme` and the `OBCUI` component
kit, and read the component gallery and the screenshot tests for the current states. Do not
introduce one-off colours or chrome metrics. List rows draw the track sketch, never a map;
detail pages use native MapLibre with the published OSM tiles and fall back to the sketch. Copy stays English-only until localization is a complete feature.

## Conventions

- Use ASD-STE100 Simplified Technical English for documentation, issues and pull requests.
- Swift 6, async/await, `AsyncStream`, and `@Observable` view models.
- Decode formats at the edges into canonical domain models. Persist canonical models, never
  transport bytes.
- Surface device-write failures or reconcile them on reconnect; do not hide them with `try?`.
- Store and cancel open-ended stream tasks; capture `self` weakly and unwrap inside the loop.
- New test suites use Swift Testing; migrate an XCTest suite only when substantially rewriting it.
- Keep one feature per folder under `OBCUI`.
- Never ship mock or developer-panel code in Release.
