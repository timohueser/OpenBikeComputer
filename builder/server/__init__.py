"""The builder's local dev host: a FastAPI server that backs `builder/app` with
region selection, style presets, and build jobs run through the native
`obc-pack` packer (host/obc-pack). One of the app's three hosts — the other two
(the static web tier and the Tauri desktop app) have no Python in them at all."""
