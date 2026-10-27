#!/usr/bin/env python3
"""The entry point of the reference archive tool. Everything is in the `ingest` package.

    python3 ingest.py fetch ch --tile 4812-4223 --work /tmp/work --out /tmp/out
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from ingest.cli import main  # noqa: E402

if __name__ == "__main__":
    sys.exit(main())
