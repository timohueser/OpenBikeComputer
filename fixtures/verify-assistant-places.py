#!/usr/bin/env python3
"""Verify captured Ride Assistant places through the production OBCM path."""
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent
subprocess.run([
    "cargo", "test", "--locked", "-p", "obc-pack", "--features", "external-fixtures",
    "--test", "assistant_places", "--", "--nocapture",
], cwd=ROOT, env={**os.environ, "OBC_REQUIRE_FIXTURES": "1"}, check=True)
