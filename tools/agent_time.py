#!/usr/bin/env python3
"""Claude Code `Stop` hook: report where an agent session spent its time to the verification console.

The transcript already holds a timestamp on every event. Each interval between two events goes to
one bucket, chosen by what the session waited for at its end: the model (`model`), a build or test
(`build`), CI (`ci`) or any other tool (`tools`). A gap that ends with a person's prompt is the
person's time and is not counted. The hook also counts tokens, and per command or tool the calls,
failures and characters of output that went into the context. It prints nothing, so the model
never sees it. Each run sends the whole session, and the console replaces the session's earlier
report, so a lost upload is repaired by the next turn.
`OBC_AGENT_TIME=off` turns it off; `OBC_AGENT_TIME_URL` points it at another console.
"""

from __future__ import annotations

import json
import os
import re
import shlex
import sys
import urllib.request
from collections import defaultdict
from collections.abc import Iterable
from datetime import datetime
from typing import Any

URL = "https://releases.openbikecomputer.com/api/agent-time"
BUCKETS = ("model", "tools", "build", "ci")
#: Token fields of a day, and the transcript `usage` names they come from.
TOKENS = {
    "input": "input_tokens",
    "output": "output_tokens",
    "cache_read": "cache_read_input_tokens",
    "cache_write": "cache_creation_input_tokens",
}
DAY_FIELDS = (*BUCKETS, *TOKENS)
TOOL_FIELDS = ("calls", "seconds", "failed", "chars")
#: When parallel calls are pending, the interval goes to the slowest kind.
PRIORITY = {"build": 2, "ci": 1, "tool": 0}
#: Per day, the slowest build and CI keys and the keys with the most output.
TOP = 25
#: A pipe such as `| tail` hides the exit code, so the failure lines of common test runners count too.
FAILURE = re.compile(
    r"^(?:FAILED \(|test result: FAILED|error(?:\[E\d+\])?: |npm ERR!|# fail [1-9]|\s*FAIL \[|=+ .*\d+ failed)", re.MULTILINE
)

BUILD_PROGRAMS = {
    "cargo", "just", "make", "cmake", "ninja", "xcodebuild", "swift", "pytest", "tsc", "vite",
    "svelte-check", "npx", "west", "gradle",
}
#: `obc` tasks that only look something up.
OBC_LOOKUPS = {"pr", "req", "governs", "help", "cairn", "prose", "--agent", "--all"}
CI_WAITS = {("gh", "run", "watch"), ("gh", "run", "view"), ("gh", "pr", "checks")}
WRAPPERS = {"time", "nice", "env", "command", "exec"}
SEGMENT = re.compile(r"&&|\|\||[;|\n]")


def command_kind(tokens: list[str]) -> str | None:
    """`build`, `ci`, or None for a command that is neither."""
    program = os.path.basename(tokens[0])
    if program in BUILD_PROGRAMS:
        return "build"
    if program == "obc":
        if len(tokens) < 2 or tokens[1] in OBC_LOOKUPS:
            return None
        return "ci" if tokens[1] == "ci" else "build"
    if program in {"npm", "pnpm", "yarn"}:
        return "build" if len(tokens) > 1 and tokens[1] in {"test", "run", "ci", "install", "exec"} else None
    if program in {"python", "python3", "uv", "node"}:
        return "build" if any(t in {"pytest", "unittest", "xmlrunner", "--test"} for t in tokens) else None
    if program in {"bash", "sh"}:
        return "build" if len(tokens) > 1 and "tools/ci/" in tokens[1] else None
    if program == "sleep" or (program, *tokens[1:3]) in CI_WAITS:
        return "ci"
    return None


def segments(command: str) -> Iterable[list[str]]:
    """The words of each step of a shell command line, without `cd` steps, variables and wrappers."""
    for segment in SEGMENT.split(command):
        try:
            tokens = shlex.split(segment)
        except ValueError:
            tokens = segment.split()
        while tokens and (re.match(r"^\w+=", tokens[0]) or tokens[0] in WRAPPERS):
            tokens.pop(0)
        if tokens and tokens[0] == "timeout":
            tokens = tokens[2:]
        if tokens and tokens[0] != "cd":
            yield tokens


def classify_command(command: str) -> tuple[str, str] | None:
    """The kind and a short key of the first build or CI step in a shell command line."""
    for tokens in segments(command):
        kind = command_kind(tokens)
        if kind:
            key = []
            for token in tokens:
                if re.match(r"^\d*[<>]", token) or len(key) == 5:
                    break
                # Long options and run or PR numbers would split one command into many keys.
                if not (token.isdigit() or token.startswith("--")):
                    key.append(token)
            return kind, " ".join(key)[:80]
    return None


def program_key(command: str) -> str:
    """The program and its subcommand or script, such as `git diff` or `python3 ci_log.py`."""
    for tokens in segments(command):
        key = os.path.basename(tokens[0])
        if len(tokens) > 1 and re.fullmatch(r"[a-z][\w-]*", tokens[1]):
            key += " " + tokens[1]
        elif len(tokens) > 1 and re.search(r"\.(py|sh|mjs|ts)$", tokens[1]):
            key += " " + os.path.basename(tokens[1])
        return key[:80]
    return "bash"


def tool_key(name: str, tool_input: dict[str, Any]) -> tuple[str, str]:
    """The kind (`build`, `ci` or `tool`) and the key a tool call is counted under."""
    if name == "Bash":
        command = str(tool_input.get("command", ""))
        found = None if tool_input.get("run_in_background") else classify_command(command)
        return found or ("tool", program_key(command))
    if name == "Monitor":
        return "ci", name
    if re.match(r"^mcp__XcodeBuildMCP__.*(build|test)", name):
        return "build", name.removeprefix("mcp__XcodeBuildMCP__")
    return "tool", name.removeprefix("mcp__").replace("__", " ")[:80]


def text_of(content: Any) -> str:
    if isinstance(content, str):
        return content
    return "".join(b.get("text", "") for b in content if isinstance(b, dict))


def report(session: str, lines: Iterable[str]) -> dict[str, Any]:
    """Per UTC day: seconds per bucket and tokens; per day and tool key: calls, seconds, failures, output."""
    days: dict[str, dict[str, float]] = defaultdict(lambda: dict.fromkeys(DAY_FIELDS, 0))
    tools: dict[tuple[str, str, str], dict[str, float]] = defaultdict(lambda: dict.fromkeys(TOOL_FIELDS, 0))
    usage: dict[str, tuple[str, dict[str, int]]] = {}
    pending: dict[str, tuple[str, str, datetime]] = {}
    previous: tuple[str, datetime] | None = None
    for line in lines:
        try:
            event = json.loads(line)
            at = datetime.fromisoformat(event["timestamp"])
        except (ValueError, KeyError, TypeError, AttributeError):
            continue
        role = event.get("type")
        if role not in {"user", "assistant"} or event.get("isSidechain") or event.get("isMeta"):
            continue
        day = at.date().isoformat()
        message = event.get("message") or {}
        content = message.get("content") or []
        blocks = [b for b in content if isinstance(b, dict)] if isinstance(content, list) else []
        results = [b for b in blocks if b.get("type") == "tool_result"]
        text = "" if results else text_of(content).lstrip()
        human = role == "user" and not results and (
            (event.get("origin") or {}).get("kind") == "human" or ("origin" not in event and not text.startswith("<"))
        )
        if role == "assistant":
            bucket = "model"
            # One response spans several lines, and each repeats its usage.
            counts = message.get("usage") or {}
            seen = usage.setdefault(str(message.get("id")), (day, {}))[1]
            for field, name in TOKENS.items():
                seen[field] = max(seen.get(field, 0), int(counts.get(name) or 0))
        elif human or (not results and previous and previous[0] == "assistant"):
            # A new turn. A call with no result, such as an interrupted one, no longer holds the session.
            pending.clear()
            bucket = None if human else "build" if "<task-notification" in text else "ci"
        elif results or pending:
            bucket = max([p[0] for p in pending.values()] or ["tool"], key=PRIORITY.__getitem__)
        else:
            bucket = "model"
        if previous and bucket:
            days[day]["tools" if bucket == "tool" else bucket] += max(0.0, (at - previous[1]).total_seconds())
        for block in blocks:
            if role == "assistant" and block.get("type") == "tool_use":
                kind, key = tool_key(str(block.get("name", "")), block.get("input") or {})
                pending[block.get("id")] = (kind, key, at)
        for result in results:
            kind, key, started = pending.pop(result.get("tool_use_id"), ("tool", "unknown", at))
            output = text_of(result.get("content") or "")
            entry = tools[(day, kind, key)]
            entry["calls"] += 1
            entry["seconds"] += max(0.0, (at - started).total_seconds())
            entry["failed"] += bool(result.get("is_error") or (kind == "build" and FAILURE.search(output)))
            entry["chars"] += len(output)
        previous = (role, at)
    for day, counts in usage.values():
        for field, value in counts.items():
            days[day][field] += value
    kept = []
    for day in sorted(days):
        rows = [(k, key, v) for (d, k, key), v in tools.items() if d == day]
        slow = sorted((r for r in rows if r[0] != "tool"), key=lambda r: -r[2]["seconds"])[:TOP]
        large = sorted(rows, key=lambda r: -r[2]["chars"])[:TOP]
        for kind, key, v in {(r[0], r[1]): r for r in slow + large}.values():
            kept.append({"day": day, "kind": kind, "key": key, **{f: round(v[f], 1) if f == "seconds" else int(v[f]) for f in TOOL_FIELDS}})
    return {
        "session": session,
        "days": [{"day": d, **{f: round(v[f], 1) if f in BUCKETS else int(v[f]) for f in DAY_FIELDS}} for d, v in sorted(days.items())],
        "tools": kept,
    }


def main() -> None:
    if os.environ.get("OBC_AGENT_TIME", "").lower() in {"off", "0", "false"}:
        return
    try:
        hook = json.load(sys.stdin)
        session, path = str(hook["session_id"]), str(hook["transcript_path"])
    except (ValueError, KeyError, TypeError):
        return
    # The parent returns at once, so the agent never waits for the network.
    if hasattr(os, "fork") and os.fork():
        return
    try:
        if hasattr(os, "setsid"):
            os.setsid()
            # The harness waits until the hook's output pipes close.
            devnull = os.open(os.devnull, os.O_RDWR)
            for fd in (0, 1, 2):
                os.dup2(devnull, fd)
        with open(path, encoding="utf-8", errors="replace") as transcript:
            body = json.dumps(report(session, transcript)).encode()
        request = urllib.request.Request(
            os.environ.get("OBC_AGENT_TIME_URL", URL), data=body, headers={"Content-Type": "application/json"}
        )
        urllib.request.urlopen(request, timeout=15).close()
    except Exception:  # A report is never worth a failed hook.
        pass
    finally:
        if hasattr(os, "fork"):
            os._exit(0)


if __name__ == "__main__":
    main()
