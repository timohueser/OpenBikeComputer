"""OBCM map-packing tools.

The `web_builder` browser UI drives the native `obc-pack` packer
(`host/obc-pack`) to build OpenStreetMap extracts into `.obcm` maps for the
OBC firmware (see ../firmware). The packer was historically a Python pipeline
(`pack.py` + `obcm/`); it now lives entirely in Rust.
"""
