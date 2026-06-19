#!/usr/bin/env python3
"""Throwaway Stage-3 gating probe (handover §3.1), Python/osmium side.

Dumps every node's id, osmium's 1e-7 fixed-point integer lon/lat
(`Location.x`/`.y`), and the f64 bit patterns of `Location.lon`/`.lat`, sorted by
id. Compared bit-for-bit against the Rust osmpbf dump (the `node_probe` binary):
osmium computes `lon = double(x_int) / 1e7`, and the Rust probe must reproduce the
same int and the same bits. If they match, the coordinate read is parity-safe and
ingest can proceed.

Usage:  node_probe.py <pbf>   → sorted `id x y lon_bits lat_bits` lines on stdout.
"""
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

import osmium


def fbits(v):
    return struct.unpack("<Q", struct.pack("<d", float(v)))[0]


class NodeProbe(osmium.SimpleHandler):
    def __init__(self):
        super().__init__()
        self.rows = []

    def node(self, n):
        loc = n.location
        self.rows.append((n.id, loc.x, loc.y, fbits(loc.lon), fbits(loc.lat)))


def main():
    h = NodeProbe()
    h.apply_file(sys.argv[1])
    h.rows.sort(key=lambda r: r[0])
    out = "".join(f"{i} {x} {y} {lb} {ab}\n" for (i, x, y, lb, ab) in h.rows)
    sys.stdout.write(out)


if __name__ == "__main__":
    main()
