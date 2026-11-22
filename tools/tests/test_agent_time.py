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
            assistant(107, ("d", "Bash", {"command": "nice -n 19 cargo build"})),  # Interrupted: no result.
            human(1000, "next question"),  # The person's time is not counted.
            assistant(1003, ("e", "Bash", {"command": "git diff --stat"})),
            result(1004, "e", text="y" * 30),
        ]
        report = agent_time.report(lines)
        self.assertEqual(
            report["days"],
            [{
                "day": "2026-03-01",
                "model": 4 + 1 + 5 + 2 + 5 + 3, "tools": 1, "build": 60 + 30, "ci": 0,
                "input": 5, "output": 40, "cache_read": 500, "cache_write": 0,
            }],
        )
        rows = {(r["kind"], r["key"]): {k: r[k] for k in agent_time.TOOL_FIELDS} for r in report["tools"]}
        self.assertEqual(rows, {
            ("build", "obc test -p planner"): {"calls": 1, "seconds": 60, "failed": 1, "chars": 39},
            ("build", "cargo test -p obc-app"): {"calls": 1, "seconds": 30, "failed": 1, "chars": 2},
            ("tool", "Read"): {"calls": 1, "seconds": 1, "failed": 0, "chars": 500},
            ("tool", "git diff"): {"calls": 1, "seconds": 1, "failed": 0, "chars": 30},
        })

    def test_between_turns_only_the_waits_the_session_started_count(self) -> None:
        stop = lambda t: json.dumps({"type": "system", "subtype": "stop_hook_summary", "timestamp": at(t)})
        ended = lambda status: f"<task-notification><tool-use-id>bg</tool-use-id><status>{status}</status></task-notification>"
        lines = [
            human(0),
            assistant(1, ("bg", "Bash", {"command": "nice -n 19 cargo test", "run_in_background": True})),
            result(2, "bg"),
            assistant(3), stop(3),
            wake(304, ended("completed"), "task-notification"),
            assistant(306), stop(306),
            wake(407, "<ci-monitor-event>checks passed</ci-monitor-event>", None),
            assistant(408), stop(408),
            json.dumps({"type": "user", "timestamp": at(1000), "isMeta": True, "message": {"content": "check the run"}}),
            assistant(1002), stop(1002),
            # A resumed session gets a notice for each task of its earlier run.
            wake(2000, ended("stopped"), "task-notification"),
            assistant(2001), stop(2001),
            wake(2500, "<system-reminder>The session you started finished.</system-reminder>", None),
            # A compaction summary carries the earlier time of the compaction.
            json.dumps({"type": "user", "timestamp": at(100), "isCompactSummary": True, "message": {"content": "Summary"}}),
            assistant(2503), stop(2503),
            assistant(2900),  # A wake that the transcript does not record.
        ]
        days = agent_time.report(lines)["days"]
        self.assertEqual({b: days[0][b] for b in agent_time.BUCKETS}, {"model": 1 + 1 + 2 + 1 + 2 + 1 + 3, "tools": 1, "build": 301, "ci": 101})

    def test_a_subagent_counts_its_own_time_once(self) -> None:
        side = {"isSidechain": True}
        meta = {**side, "isMeta": True}
        subagent = [
            json.dumps({"type": "user", "timestamp": at(0), "message": {"content": "find it"}, **side}),
            assistant(2, ("s1", "Bash", {"command": "cargo test", "run_in_background": True}), **side),
            result(3, "s1", **side),
            assistant(4, **side),
            wake(64, "<task-notification><tool-use-id>s1</tool-use-id><status>completed</status></task-notification>", "task-notification", **meta),
            assistant(66, **side),
            # The parent sends a new message after the subagent stopped: the parent's time.
            wake(500, "and also this", "coordinator", **meta),
            assistant(503, ("h", "SubagentHandback", {"message": "done"}), ("s2", "Bash", {"command": "cargo build", "run_in_background": True}), **side),
            result(504, "h", **side),
            # A shell task of the subagent ends after the handback: a new turn.
            wake(604, "<task-notification><tool-use-id>s2</tool-use-id><status>completed</status></task-notification>", "task-notification", **meta),
        ]
        main = [
            human(0),
            assistant(1, ("f", "Agent", {"prompt": "find it"})),
            subagent[1],  # An older main transcript holds subagent events too.
            result(70, "f", text="found"),
            assistant(72, ("g", "Workflow", {"script": "x"})),  # Runs in the background.
            result(73, "g"),
            assistant(74),
            wake(600, "<task-notification><tool-use-id>g</tool-use-id><status>completed</status></task-notification>", "task-notification"),
            assistant(601),
            wake(700, "a note", "peer"),
            assistant(702),
        ]
        times = lambda r: {b: r["days"][0][b] for b in agent_time.BUCKETS}
        self.assertEqual(times(agent_time.report(subagent, subagent=True)), {"model": 2 + 1 + 2 + 3, "tools": 1 + 1, "build": 60 + 100, "ci": 0})
        self.assertEqual(times(agent_time.report(main)), {"model": 1 + 2 + 1 + 1 + 2, "tools": 0, "build": 0, "ci": 0})
        self.assertEqual(agent_time.report(subagent)["days"], [])

    def test_a_fork_skips_the_copy_of_its_parent(self) -> None:
        side = {"isSidechain": True}
        copied = [human(0, **side), assistant(30, ("spawn", "Agent", {"prompt": "fork"}), output=500, **side)]
        own = [json.dumps({"type": "user", "timestamp": at(31), "message": {"content": "fork task"}, **side}), assistant(36, output=7, **side)]
        days = agent_time.report(copied + own, subagent=True, start="spawn")["days"]
        self.assertEqual({k: days[0][k] for k in ("model", "output")}, {"model": 5, "output": 7})

    def test_a_subagent_reports_as_its_own_session(self) -> None:
        self.assertEqual(
            agent_time.session_of({"hook_event_name": "Stop", "session_id": "s1", "transcript_path": "/x.jsonl"}),
            ({"session": "s1", "kind": "main"}, "/x.jsonl", None),
        )
        with tempfile.TemporaryDirectory() as directory:
            path = f"{directory}/agent-a2.jsonl"
            hook = {"hook_event_name": "SubagentStop", "session_id": "s1", "agent_id": "a2", "agent_type": "Explore", "agent_transcript_path": path}
            fields = {"session": "a2", "kind": "subagent", "parent": "s1", "agent_type": "Explore", "description": ""}
            self.assertEqual(agent_time.session_of(hook), (fields, path, None))
            # A nested subagent's parent is the agent that started it.
            meta = {"parentAgentId": "a1", "description": "Find the bug", "isFork": True, "toolUseId": "t1"}
            Path(f"{directory}/agent-a2.meta.json").write_text(json.dumps(meta))
            self.assertEqual(agent_time.session_of(hook), ({**fields, "parent": "a1", "description": "Find the bug"}, path, "t1"))

    def test_command_keys(self) -> None:
        cases = {
            "git status": None,
            "obc pr view 12": None,
            "obc ci": ("ci", "obc ci"),
            "gh run watch 123 --exit-status": ("ci", "gh run watch"),
            "npm run verify --prefix tools/verification": ("build", "npm run verify tools/verification"),
            "uv run python -m unittest tools.tests.test_x": ("build", "uv run python -m unittest"),
            "timeout 600 bash tools/ci/test.sh nextest-fast": ("build", "bash tools/ci/test.sh nextest-fast"),
            "systemd-run --user --scope -p MemoryMax=3G nice -n 19 cargo test": ("build", "cargo test"),
            "flock /tmp/gpu.lock obc exec node shoot.mjs": ("build", "obc exec node shoot.mjs"),
            # A wait waits on local work, unless the line polls CI.
            "until grep -q done out.log; do sleep 5; done": ("build", "sleep"),
            "timeout 90 bash -c 'tail --pid=42 -f /dev/null'": ("build", "tail -f /dev/null"),
            "for i in 1 2 3; do s=$(gh pr view 12 --json state); sleep 30; done": ("ci", "sleep"),
        }
        for command, expected in cases.items():
            self.assertEqual(agent_time.classify_command(command), expected, command)
        self.assertEqual(agent_time.program_key("cd x && python3 tools/ci_log.py --run 5"), "python3 ci_log.py")
        self.assertEqual(agent_time.program_key("sed -n 1,20p file"), "sed")


if __name__ == "__main__":
    unittest.main()
