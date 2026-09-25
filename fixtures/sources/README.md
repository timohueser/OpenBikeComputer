# Authored fixture sources

Small human-authored inputs that stay in Git so their changes stay reviewable. Directory layouts
mirror their package layouts where practical, and `build-map-package.sh` combines them with
generated maps and terrain before packing.

This is the only cross-component home for shared route, trip and replay sources. App-owned
shipping payloads and test-owned protocol vectors stay with their owners.

[Route import](route-import/README.md) holds the captured planner exports, each with its
provenance, its licence and the duplicate a test holds identical to it.

[Kandel ride](kandel-ride/README.md) holds the GPX source and the command for a simulated finished ride.

[Ride Assistant](ride-assistant/README.md) holds the source manifests, regional boundaries, and
the GPS replays with their clock and UTC-offset stamps.
