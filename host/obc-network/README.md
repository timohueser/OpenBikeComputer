# Network producer

Graph construction, seam cuts, POI and navigation encoding.
The producer reads shared source records and byte types. It does not depend on the other producer.

Run these commands from the repository root:

```sh
./tools/obc test -p obc-network
cargo clippy -p obc-network --all-targets -- -D warnings
```
