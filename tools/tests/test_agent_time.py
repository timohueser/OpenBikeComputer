"""Tests for `tools/agent_time.py`: a transcript reduces to seconds per bucket and per command."""

from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import agent_time


def at(seconds: int) -> str:
    return f"2026-03-01T10:{seconds // 60:02d}:{seconds % 60:02d}.000Z"


def human(t: int, text: str = "do it") -> str:
    return json.dumps({"type": "user", "timestamp": at(t), "origin": {"kind": "human"}, "message": {"content": text}})


def wake(t: int, text: str) -> str:
    return json.dumps({"type": "user", "timestamp": at(t), "origin": {"kind": "task"}, "message": {"content": text}})


def assistant(t: int, *calls: tuple[str, str, dict]) -> str:
    blocks = [{"type": "tool_use", "id": i, "name": n, "input": a} for i, n, a in calls] or [{"type": "text", "text": "ok"}]
    return json.dumps({"type": "assistant", "timestamp": at(t), "message": {"content": blocks}})


def result(t: int, *ids: str) -> str:
    return json.dumps({"type": "user", "timestamp": at(t), "message": {"content": [{"type": "tool_result", "tool_use_id": i} for i in ids]}})


class ReportTest(unittest.TestCase):
    def test_intervals_go_to_the_bucket_the_session_waited_for(self) -> None:
        lines = [
            human(0),
            assistant(5, ("a", "Bash", {"command": "cd planner && RUST_LOG=info obc test -p planner 2>&1 | tail"})),
            result(65, "a"),
            # Parallel calls: the read ends first, the build holds the rest of the interval.
            assistant(70, ("b", "Read", {"file_path": "x"}), ("c", "Bash", {"command": "cargo test -p obc-app"})),
            result(71, "b"),
            result(100, "c"),
            assistant(102),
            wake(402, '<wake reason="external-event"><event source="github" kind="check_run">'),
            assistant(404),
            wake(504, "<task-notification>build finished</task-notification>"),
            assistant(505),
            json.dumps({"type": "system", "timestamp": at(600)}),
            assistant(510, ("d", "Bash", {"command": "cargo build"})),  # Interrupted: no result.
            human(1000, "next question"),  # The person's time is not counted.
            assistant(1003, ("e", "Read", {"file_path": "x"})),
            result(1004, "e"),
        ]
        day = agent_time.report("s", lines)["days"][0]
        self.assertEqual(day["day"], "2026-03-01")
        self.assertEqual(day["seconds"], {"model": 5 + 5 + 2 + 2 + 1 + 5 + 3, "tools": 1, "build": 60 + 30 + 100, "ci": 300})
        self.assertEqual(
            day["commands"],
            [
                {"kind": "build", "key": "obc test -p planner", "count": 1, "seconds": 60},
                {"kind": "build", "key": "cargo test -p obc-app", "count": 1, "seconds": 30},
            ],
        )

    def test_command_kinds(self) -> None:
        cases = {
            "git status": None,
            "obc pr view 12": None,
            "obc ci": ("ci", "obc ci"),
            "gh run watch 123 --exit-status": ("ci", "gh run watch"),
            "npm run verify --prefix tools/verification": ("build", "npm run verify tools/verification"),
            "uv run python -m unittest tools.tests.test_x": ("build", "uv run python -m unittest"),
            "timeout 600 bash tools/ci/test.sh nextest-fast": ("build", "bash tools/ci/test.sh nextest-fast"),
        }
        for command, expected in cases.items():
            self.assertEqual(agent_time.classify_command(command), expected, command)


if __name__ == "__main__":
    unittest.main()
