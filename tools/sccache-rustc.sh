#!/usr/bin/env bash
# The rustc wrapper of `obc` when obc.local sets RUSTC_WRAPPER=sccache. sccache puts every CARGO_*
# variable into its key, and CARGO_TARGET_DIR names the target of the checkout, so with it no
# checkout gets a hit from another one. rustc does not read CARGO_TARGET_DIR.
exec env -u CARGO_TARGET_DIR sccache "$@"
