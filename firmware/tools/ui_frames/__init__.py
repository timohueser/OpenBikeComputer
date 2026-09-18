"""The UI snapshot sweep: the frame table, the staging environments, and the renderer.

`firmware/ui-frames.toml` holds the frames as data, this package renders them, and
`firmware/ui-snapshots.sha256` holds one digest row for each frame. One frame, the whole sweep and
the digest check are three modes of the same code, so they cannot disagree. `obc shot` is the
command; `python3 -m ui_frames --help` is the same thing with `firmware/tools` on `PYTHONPATH`.
"""
