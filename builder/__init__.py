"""The map builder.

`app/` is the Svelte UI, shared by its two hosts — the static web tier and
the FastAPI dev server in `server/` (see `app/vite.config.ts`: one frontend,
two hosts). None of them packs anything
itself: they drive the native `obc-pack` packer (`host/obc-pack`) to build
OpenStreetMap extracts into `.obcm` maps for the OBC firmware.

The packer was historically a Python pipeline (`pack.py` + `obcm/`); it now
lives entirely in Rust, which is why nothing under this directory parses OSM.
"""
