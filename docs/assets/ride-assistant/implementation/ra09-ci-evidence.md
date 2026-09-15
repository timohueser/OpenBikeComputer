# Landmark build and resource evidence

## Browser assembly

CI run [34937553283](https://github.com/timohueser/OpenBikeComputer/actions/runs/34937553283),
job 104278898259, measured commit `2c562608` after the ordinary release build with
wasm-pack and wasm-opt `-Oz`:

- Assembly WebAssembly: 689,089 bytes raw; 262,891 bytes compressed.
- WebAssembly and JavaScript together: 272,582 bytes compressed.
- The 282,624-byte compressed budget passes.

The raw assembly budget is a structural dependency guard. Its policy permits an
update when the assembly engine grows for required work. The landmark pass reads
bounded records, resolves duplicate QIDs, merges shared schedules, interns source
content, and streams verified blobs into the installed map. These operations are
part of normal cell assembly.

Production dependency comparison used `cargo tree -p obc-web-assemble --target
wasm32-unknown-unknown -e normal,build --prefix none` on the route-facts parent and
this branch. Only `miniz_oxide 0.8.9` and `adler2 2.0.1` were added, through the
ordinary map reader. The graph does not include the map packer, GEOS, a renderer,
or App. Development dependencies were excluded from this comparison.

The raw budget is 744 KiB, with about 10 percent space above the measured program.
The compressed budget stays at 276 KiB. Device allocation limits, stack limits,
measured hardware high-water values, and verification budgets do not change.
No base image was rebuilt. CI supplies the build evidence; the final integrated
shipping image and physical checks remain pending.

## Board dependencies

The standalone board lockfile now resolves the same decoder versions as the root
workspace. `obc licenses` regenerated the required miniz_oxide and adler2 notices.
No package versions were upgraded.
