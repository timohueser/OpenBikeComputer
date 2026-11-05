#!/usr/bin/env python3
"""Claude Code `Stop` hook: report where an agent session spent its time to the verification console.

The transcript already holds a timestamp on every event. Each interval between two events goes to
one bucket, chosen by what the session waited for at its end: the model (`model`), a build or test
(`build`), CI (`ci`) or any other tool (`tools`). A gap that ends with a person's prompt is the
person's time and is not counted. The hook prints nothing, so the model never sees it.
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
#: When parallel calls are pending, the interval goes to the slowest kind.
PRIORITY = {"build": 2, "ci": 1, "tools": 0}
MAX_COMMANDS = 40

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


def classify_command(command: str) -> tuple[str, str] | None:
    """The kind and a short key of the first build or CI step in a shell command line."""
    for segment in SEGMENT.split(command):
        try:
            tokens = shlex.split(segment)
        except ValueError:
            tokens = segment.split()
        while tokens and (re.match(r"^\w+=", tokens[0]) or tokens[0] in WRAPPERS):
            tokens.pop(0)
        if tokens and tokens[0] == "timeout":
            tokens = tokens[2:]
        if not tokens or tokens[0] == "cd":
            continue
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


def tool_kind(name: str, tool_input: dict[str, Any]) -> tuple[str, str | None]:
    if name == "Bash" and not tool_input.get("run_in_background"):
        found = classify_command(str(tool_input.get("command", "")))
        if found:
            return found
    if name == "Monitor":
        return "ci", None
    if re.match(r"^mcp__XcodeBuildMCP__.*(build|test)", name):
        return "build", name.removeprefix("mcp__XcodeBuildMCP__")
    return "tools", None


def text_of(content: Any) -> str:
    if isinstance(content, str):
        return content
    return "".join(b.get("text", "") for b in content if isinstance(b, dict))


def report(session: str, lines: Iterable[str]) -> dict[str, Any]:
    """Seconds per bucket and per command for each UTC day of the session."""
    seconds: dict[str, dict[str, float]] = defaultdict(lambda: dict.fromkeys(BUCKETS, 0.0))
    commands: dict[str, dict[tuple[str, str], list[float]]] = defaultdict(lambda: defaultdict(lambda: [0, 0.0]))
    pending: dict[str, tuple[str, str | None, datetime]] = {}
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
        content = (event.get("message") or {}).get("content") or []
        blocks = [b for b in content if isinstance(b, dict)] if isinstance(content, list) else []
        results = [b for b in blocks if b.get("type") == "tool_result"]
        text = "" if results else text_of(content).lstrip()
        human = role == "user" and not results and (
            (event.get("origin") or {}).get("kind") == "human" or ("origin" not in event and not text.startswith("<"))
        )
        if role == "assistant":
            bucket = "model"
        elif human or (not results and previous and previous[0] == "assistant"):
            # A new turn. A call with no result, such as an interrupted one, no longer holds the session.
            pending.clear()
            bucket = None if human else "build" if "<task-notification" in text else "ci"
        elif results or pending:
            bucket = max([p[0] for p in pending.values()] or ["tools"], key=PRIORITY.__getitem__)
        else:
            bucket = "model"
        if previous and bucket:
            seconds[at.date().isoformat()][bucket] += max(0.0, (at - previous[1]).total_seconds())
        for block in blocks:
            if role == "assistant" and block.get("type") == "tool_use":
                kind, key = tool_kind(str(block.get("name", "")), block.get("input") or {})
                pending[block.get("id")] = (kind, key, at)
        for result in results:
            kind, key, started = pending.pop(result.get("tool_use_id"), ("tools", None, at))
            if key:
                entry = commands[at.date().isoformat()][(kind, key)]
                entry[0] += 1
                entry[1] += max(0.0, (at - started).total_seconds())
        previous = (role, at)
    days = []
    for day in sorted(seconds):
        top = sorted(commands[day].items(), key=lambda item: -item[1][1])[:MAX_COMMANDS]
        days.append({
            "day": day,
            "seconds": {b: round(s, 1) for b, s in seconds[day].items()},
            "commands": [{"kind": k, "key": key, "count": c, "seconds": round(s, 1)} for (k, key), (c, s) in top],
        })
    return {"session": session, "days": days}


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
