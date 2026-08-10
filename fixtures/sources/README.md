# Authored fixture sources

These small, human-authored inputs stay in Git so their changes remain
reviewable. Directory layouts mirror their package layouts where practical;
`build-map-package.sh` combines them with generated maps/terrain before packing.

This is the only cross-component home for shared route, trip, and replay
sources. App-owned shipping payloads and test-owned protocol vectors remain
with their owners.
