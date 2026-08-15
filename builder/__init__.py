"""The map builder.

`app/` is the Svelte UI, shared by all three of its hosts — the static web
tier, the Tauri desktop app, and the FastAPI dev server in `server/` (see
`app/vite.config.ts`: one frontend, three hosts). None of them packs anything
itself: they drive the native `obc-pack` packer (`host/obc-pack`) to build
OpenStreetMap extracts into `.obcm` maps for the OBC firmware.

The packer was historically a Python pipeline (`pack.py` + `obcm/`); it now
lives entirely in Rust, which is why nothing under this directory parses OSM.
"""
