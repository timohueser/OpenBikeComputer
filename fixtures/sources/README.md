# Authored fixture sources

These small, human-authored inputs stay in Git so their changes remain
reviewable. Directory layouts mirror their package layouts where practical;
`build-map-package.sh` combines them with generated maps/terrain before packing.

This is the only cross-component home for shared route, trip, and replay
sources. App-owned shipping payloads and test-owned protocol vectors remain
with their owners.

The weather-event `event.json` manifests are tracked here too. Their package
copies describe and hash every external upstream, service, and truth member;
`tracked_sources` makes a manifest edit require a package rebuild.
