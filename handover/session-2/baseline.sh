#!/usr/bin/env bash
# The new baseline of this container: Switzerland baked from the head of routing-overlay.
set -euo pipefail
B=/home/user/obc-bake
step() { echo "=== $(date +%T) $*"; }
while ! grep -q downloads-done /home/user/obc-notes/download.log; do sleep 10; done
LOCK="flock /home/user/obc-target/.build.lock"
step build
cd /home/user/obc-wt/base
git log --oneline -1
export CARGO_TARGET_DIR=/home/user/obc-target/base CARGO_INCREMENTAL=0
source ~/.obc-geos-env.sh
$LOCK ./tools/obc exec cargo build --release --locked -p obc-data-steps 2>&1 | tail -2
$LOCK ./tools/obc exec cargo build --release --locked -p planner-router-build --example cells 2>&1 | tail -2
$LOCK ./tools/obc exec cargo build --release --locked -p planner-router --example compare 2>&1 | tail -2
cp $CARGO_TARGET_DIR/release/obc-data $B/bin/obc-data-base
cp $CARGO_TARGET_DIR/release/examples/cells $B/bin/cells-base
cp $CARGO_TARGET_DIR/release/examples/compare $B/bin/compare-base
du -sh $CARGO_TARGET_DIR
step bake
cd $B
$LOCK python3 -I tools/bake_layers.py bin/obc-data-base switzerland/switzerland-bounds-sorted.osm.pbf switzerland/glo30 switzerland/run-base
step packages
mkdir switzerland/packages-base
mv switzerland/run-base/output/routing switzerland/packages-base/format-9
mv switzerland/run-base/output/cells switzerland/packages-base/format-10
$LOCK bin/cells-base install switzerland/packages-base/format-10 switzerland/packages-base/install 4
du -sh switzerland/packages-base/* switzerland/run-base/layers/* switzerland/run-base/output
df -h /home/user | tail -1
step done
