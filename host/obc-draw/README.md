# Draw producer

Drawing geometry, contours, land and feature-tree encoding.
The producer reads shared source records and byte types. It does not depend on the other producer.

Run these commands from the repository root:

```sh
./tools/obc test -p obc-draw
cargo clippy -p obc-draw --all-targets -- -D warnings
```
