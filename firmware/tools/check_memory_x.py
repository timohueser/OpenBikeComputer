#!/usr/bin/env python3
"""Reject a `memory.x` in the board image's crate root.

The linker resolves `INCLUDE memory.x` from the crate root before `$OUT_DIR`, so such a file
shadows the carved map that `build.rs` writes, and the M33 stack grows into the FLPR image.
"""

from __future__ import annotations

GOVERNS = ['firmware/obc-fw-nrf54l/memory.x', 'firmware/obc-fw-nrf54l/build.rs']
RULE = 'The board image crate root holds no memory.x; build.rs writes the linker map into OUT_DIR.'

import sys
from pathlib import Path

SHADOW = Path(__file__).resolve().parents[1] / "obc-fw-nrf54l" / "memory.x"


def main() -> int:
    if SHADOW.exists():
        print(f"error: remove {SHADOW}: it shadows the carved memory.x that build.rs writes", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
