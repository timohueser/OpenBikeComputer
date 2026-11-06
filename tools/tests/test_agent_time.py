"""Tests for `tools/agent_time.py`: a transcript reduces to time, tokens and tool rows per day."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import agent_time


def at(seconds: int) -> str:
    return f"2026-03-01T10:{seconds // 60:02d}:{seconds % 60:02d}.000Z"


def human(t: int, text: str = "do it", **extra: object) -> str:
    return json.dumps({"type": "user", "timestamp": at(t), "origin": {"kind": "human"}, "message": {"content": text}, **extra})


def wake(t: int, text: str, kind: str = "task", **extra: object) -> str:
    return json.dumps({"type": "user", "timestamp": at(t), "origin": {"kind": kind}, "message": {"content": text}, **extra})


def assistant(t: int, *calls: tuple[str, str, dict], message: str = "", output: int = 0, **extra: object) -> str:
    blocks = [{"type": "tool_use", "id": i, "name": n, "input": a} for i, n, a in calls] or [{"type": "text", "text": "ok"}]
    usage = {"input_tokens": 1, "output_tokens": output, "cache_read_input_tokens": 100}
    body = {"id": message or f"m{t}", "content": blocks, "usage": usage}
    return json.dumps({"type": "assistant", "timestamp": at(t), "message": body, **extra})


def result(t: int, *ids: str, text: str = "", error: bool = False, **extra: object) -> str:
    content = [{"type": "tool_result", "tool_use_id": i, "content": text, "is_error": error} for i in ids]
    return json.dumps({"type": "user", "timestamp": at(t), "message": {"content": content}, **extra})


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
        report = agent_time.report(lines)
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

    def test_a_subagent_counts_its_own_time_once(self) -> None:
        side = {"isSidechain": True}
        meta = {**side, "isMeta": True}
        subagent = [
            json.dumps({"type": "user", "timestamp": at(0), "message": {"content": "find it"}, **side}),
            assistant(2, ("s1", "Bash", {"command": "cargo test", "run_in_background": True}), **side),
            result(3, "s1", **side),
            assistant(4, **side),
            wake(64, "<task-notification><task-id>bx1</task-id></task-notification>", "task-notification", **meta),
            assistant(66, **side),
            # The parent sends a new message after the subagent stopped: the parent's time.
            wake(500, "and also this", "coordinator", **meta),
            assistant(503, **side),
        ]
        main = [
            human(0),
            assistant(1, ("f", "Agent", {"prompt": "find it"})),
            subagent[1],  # An older main transcript holds subagent events too.
            result(70, "f", text="found"),
            assistant(72, ("g", "Agent", {"prompt": "more"})),  # Runs in the background.
            result(73, "g"),
            assistant(74),
            wake(600, "<task-notification><task-id>a17</task-id></task-notification>", "task-notification"),
            assistant(601),
            wake(700, "a note", "peer"),
            assistant(702),
        ]
        times = lambda r: {b: r["days"][0][b] for b in agent_time.BUCKETS}
        self.assertEqual(times(agent_time.report(subagent, subagent=True)), {"model": 2 + 1 + 2 + 3, "tools": 1, "build": 60, "ci": 0})
        self.assertEqual(times(agent_time.report(main)), {"model": 1 + 2 + 1 + 1 + 2, "tools": 0, "build": 0, "ci": 0})
        self.assertEqual(agent_time.report(subagent)["days"], [])

    def test_a_subagent_reports_as_its_own_session(self) -> None:
        self.assertEqual(
            agent_time.session_of({"hook_event_name": "Stop", "session_id": "s1", "transcript_path": "/x.jsonl"}),
            ({"session": "s1", "kind": "main"}, "/x.jsonl"),
        )
        with tempfile.TemporaryDirectory() as directory:
            path = f"{directory}/agent-a2.jsonl"
            hook = {"hook_event_name": "SubagentStop", "session_id": "s1", "agent_id": "a2", "agent_type": "Explore", "agent_transcript_path": path}
            fields = {"session": "a2", "kind": "subagent", "parent": "s1", "agent_type": "Explore", "description": ""}
            self.assertEqual(agent_time.session_of(hook), (fields, path))
            # A nested subagent's parent is the agent that started it.
            Path(f"{directory}/agent-a2.meta.json").write_text(json.dumps({"parentAgentId": "a1", "description": "Find the bug"}))
            self.assertEqual(agent_time.session_of(hook)[0], {**fields, "parent": "a1", "description": "Find the bug"})

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
