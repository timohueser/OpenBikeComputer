# Swiss compiled input evidence

The published `assistant-switzerland-content` package contains the production compiler output
used by the active Swiss regional bake: 1,495 text records and 1,119 RGB222 photos, with retained
source notices. The manifest hash matches the source census. Its 1,120 content members (plus the package manifest) contain no raw
request cache, original article capture, or full-resolution source photo.

The normal deterministic fixture packer produced a 10,554,318-byte archive, SHA-256
`cf2a7ad213d91ba513fbbe24642d1615efd071c1e659c9e7885103f11c9d33b1`.
The fixture publisher uploaded it and verified it through the public domain. An empty fixture
cache downloaded and verified it. Cached verification then passed with `urllib.request.urlopen`
disabled. The tracked source record pins the compiler input and policy hashes and source coverage.

The Swiss recipe now uses this package and the same full pinned PBF, catalog region ID and cell
selection bounds as the running bake. The wider acquisition/replay boundary stays unchanged.
The recipe must not pre-extract the PBF, because that would change the selected boundary cells.
No map bake was repeated. The Swiss map output and integrated motion acceptance remain pending.

Validation: whole repository Python suite (187 tests), Python compilation, suite registry,
documentation links and diff checks pass. No Rust build, UI sweep, resource image or hardware
run was performed for this change.
