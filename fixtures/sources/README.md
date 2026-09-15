# Authored fixture sources

These small, human-authored inputs stay in Git so their changes remain
reviewable. Directory layouts mirror their package layouts where practical;
`build-map-package.sh` combines them with generated maps/terrain before packing.

This is the only cross-component home for shared route, trip, and replay
sources. App-owned shipping payloads and test-owned protocol vectors remain
with their owners.

[Ride Assistant](ride-assistant/README.md) adds source manifests, exact review identities,
regional boundaries, and GPS motion with declarative clock and UTC-offset stamps.
