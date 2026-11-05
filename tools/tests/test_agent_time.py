"""Tests for `tools/agent_time.py`: a transcript reduces to time, tokens and tool rows per day."""

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


def assistant(t: int, *calls: tuple[str, str, dict], message: str = "", output: int = 0) -> str:
    blocks = [{"type": "tool_use", "id": i, "name": n, "input": a} for i, n, a in calls] or [{"type": "text", "text": "ok"}]
    usage = {"input_tokens": 1, "output_tokens": output, "cache_read_input_tokens": 100}
    body = {"id": message or f"m{t}", "content": blocks, "usage": usage}
    return json.dumps({"type": "assistant", "timestamp": at(t), "message": body})


def result(t: int, *ids: str, text: str = "", error: bool = False) -> str:
    content = [{"type": "tool_result", "tool_use_id": i, "content": text, "is_error": error} for i in ids]
    return json.dumps({"type": "user", "timestamp": at(t), "message": {"content": content}})


class ReportTest(unittest.TestCase):
    def test_a_transcript_reduces_to_time_tokens_and_tool_rows(self) -> None:
        lines = [
            human(0),
            # One response written as two lines, each with its usage: counted once, at its final value.
            assistant(4, message="r1", output=10),
            assistant(5, ("a", "Bash", {"command": "cd planner && RUST_LOG=info obc test -p planner 2>&1 | tail"}), message="r1", output=40),
            result(65, "a", text="test result: FAILED. 1 passed; 1 failed"),
            # Parallel calls: the read ends first, the build holds the rest of the interval.
            assistant(70, ("b", "Read", {"file_path": "x"}), ("c", "Bash", {"command": "cargo test -p obc-app"})),
            result(71, "b", text="x" * 500),
            result(100, "c", text="ok", error=True),
            assistant(102),
            wake(402, '<wake reason="external-event"><event source="github" kind="check_run">'),
            assistant(404),
            wake(504, "<task-notification>build finished</task-notification>"),
            assistant(505),
            assistant(510, ("d", "Bash", {"command": "cargo build"})),  # Interrupted: no result.
            human(1000, "next question"),  # The person's time is not counted.
            assistant(1003, ("e", "Bash", {"command": "git diff --stat"})),
            result(1004, "e", text="y" * 30),
        ]
        report = agent_time.report("s", lines)
        self.assertEqual(
            report["days"],
            [{
                "day": "2026-03-01",
                "model": 4 + 1 + 5 + 2 + 2 + 1 + 5 + 3, "tools": 1, "build": 60 + 30 + 100, "ci": 300,
                "input": 7, "output": 40, "cache_read": 700, "cache_write": 0,
            }],
        )
        rows = {(r["kind"], r["key"]): {k: r[k] for k in agent_time.TOOL_FIELDS} for r in report["tools"]}
        self.assertEqual(rows, {
            ("build", "obc test -p planner"): {"calls": 1, "seconds": 60, "failed": 1, "chars": 39},
            ("build", "cargo test -p obc-app"): {"calls": 1, "seconds": 30, "failed": 1, "chars": 2},
            ("tool", "Read"): {"calls": 1, "seconds": 1, "failed": 0, "chars": 500},
            ("tool", "git diff"): {"calls": 1, "seconds": 1, "failed": 0, "chars": 30},
        })

    def test_command_keys(self) -> None:
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
        self.assertEqual(agent_time.program_key("cd x && python3 tools/ci_log.py --run 5"), "python3 ci_log.py")
        self.assertEqual(agent_time.program_key("sed -n 1,20p file"), "sed")


if __name__ == "__main__":
    unittest.main()
