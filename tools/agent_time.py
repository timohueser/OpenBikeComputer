#!/usr/bin/env python3
"""Claude Code `Stop` and `SubagentStop` hook: report where an agent session spent its time to the verification console.

The transcript already holds a timestamp on every event. Each interval between two events goes to
one bucket, chosen by what the session waited for at its end: the model (`model`), a build or test
(`build`), CI (`ci`) or any other tool (`tools`). A gap that ends with a person's prompt is the
person's time and is not counted. Between turns the session is idle: only the wait for a background
task it started, or for CI, counts. A subagent is a session of its own, with its parent session or
agent. The time a session waits on a subagent or on its parent is not counted, because the other
session counts it. The hook also counts tokens, and per command or tool the calls,
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
#: When parallel calls are pending, the interval goes to the slowest kind. The subagent counts its own time.
PRIORITY = {"build": 2, "ci": 1, "tool": 0, "agent": -1}
#: Prompts from a person, from the parent of a subagent (`coordinator`) or from another agent (`peer`).
PROMPTS = {"human", "coordinator", "peer"}
#: A notice that a background task ended. `stopped` also marks a task of an earlier run of a resumed session.
TASK_END = re.compile(r"<tool-use-id>([\w-]+)</tool-use-id>(?:(?!</task-notification>).)*?<status>(\w+)</status>", re.DOTALL)
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
#: A command line that waits and calls one of these polls CI. Any other wait waits on local work.
POLLS_CI = re.compile(r"\b(?:gh (?:run|pr|api)|obc (?:ci|pr))\b")
#: Commands that run the command after their options, such as `nice -n 19 cargo test`.
WRAPPERS = {"time", "nice", "ionice", "env", "command", "exec", "timeout", "stdbuf", "nohup", "setsid", "systemd-run", "flock"}
#: Shell words that start a step of a loop or a condition.
SHELL_WORDS = {"if", "then", "else", "elif", "while", "until", "do", "!", "{"}
SEGMENT = re.compile(r"&&|\|\||[;|\n]")


def command_kind(tokens: list[str]) -> str | None:
    """`build`, `ci`, `wait` for a step that only waits, or None for a command that is none of these."""
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
    if (program, *tokens[1:3]) in CI_WAITS:
        return "ci"
    if program == "sleep" or (program == "read" and "-t" in tokens):
        return "wait"
    if program == "tail" and any(t in {"-f", "-F"} or t.startswith("--pid") for t in tokens):
        return "wait"
    return None


def segments(command: str) -> Iterable[list[str]]:
    """The words of each step of a shell command line, without `cd` steps, variables, shell words and wrappers."""
    for segment in SEGMENT.split(command):
        try:
            tokens = shlex.split(segment)
        except ValueError:
            tokens = segment.split()
        while tokens:
            name = os.path.basename(tokens[0])
            if re.match(r"^\w+=", tokens[0]) or name in SHELL_WORDS:
                tokens = tokens[1:]
            elif name in WRAPPERS:
                # Options, their values and a duration come before the command. `flock` also takes a lock file.
                tokens = tokens[1:]
                while tokens and re.match(r"^-|^\w+=|^\d+(\.\d+)?[smhd]?$", tokens[0]):
                    tokens = tokens[1:]
                tokens = tokens[name == "flock" :]
            else:
                break
        if len(tokens) > 2 and os.path.basename(tokens[0]) in {"bash", "sh"} and tokens[1] == "-c":
            yield from segments(tokens[2])
        elif tokens and tokens[0] != "cd":
            yield tokens


def command_key(tokens: list[str]) -> str:
    key = []
    for token in tokens:
        if re.match(r"^\d*[<>]", token) or len(key) == 5:
            break
        # Long options and run or PR numbers would split one command into many keys.
        if not (token.isdigit() or token.startswith("--")):
            key.append(token)
    return " ".join(key)[:80]


def classify_command(command: str) -> tuple[str, str] | None:
    """The kind and a short key of the first build or CI step in a shell command line, else of its first wait."""
    steps = [(command_kind(tokens), tokens) for tokens in segments(command)]
    for kind, tokens in steps:
        if kind in {"build", "ci"}:
            return kind, command_key(tokens)
    waits = [tokens for kind, tokens in steps if kind == "wait"]
    if waits:
        return "ci" if POLLS_CI.search(command) else "build", command_key(waits[0])
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
    """The kind (`build`, `ci`, `agent` or `tool`) and the key a tool call is counted under."""
    if name in {"Agent", "Workflow"}:
        return "agent", name
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


def report(lines: Iterable[str], subagent: bool = False, start: str | None = None) -> dict[str, Any]:
    """Per UTC day: seconds per bucket and tokens; per day and tool key: calls, seconds, failures, output.

    A subagent transcript marks every event `isSidechain`. A main transcript skips such events, so
    the subagent's own report is the only one that counts them. A fork's transcript starts with a
    copy of its parent's events, up to the parent's call `start` that started the fork; the parent
    counts those.
    """
    days: dict[str, dict[str, float]] = defaultdict(lambda: dict.fromkeys(DAY_FIELDS, 0))
    tools: dict[tuple[str, str, str], dict[str, float]] = defaultdict(lambda: dict.fromkeys(TOOL_FIELDS, 0))
    usage: dict[str, tuple[str, dict[str, int]]] = {}
    pending: dict[str, tuple[str, str, datetime]] = {}
    #: The kind of every call, as it would be if it ran in the foreground.
    launched: dict[str, str] = {}
    previous: tuple[str, datetime] | None = None
    idle = False
    for line in lines:
        try:
            event = json.loads(line)
            at = datetime.fromisoformat(event["timestamp"])
        except (ValueError, KeyError, TypeError, AttributeError):
            continue
        if start:
            content = (event.get("message") or {}).get("content")
            if isinstance(content, list) and any(isinstance(b, dict) and b.get("id") == start for b in content):
                start = None
            continue
        role = event.get("type")
        # Only a main transcript marks the end of a turn.
        idle = idle or (role == "system" and event.get("subtype") == "stop_hook_summary")
        origin = (event.get("origin") or {}).get("kind")
        # In a subagent, prompts from the parent and task notifications are meta events. A compaction
        # summary carries the time of the compaction, which can lie before the events around it.
        meta = (event.get("isMeta") and origin not in {*PROMPTS, "task-notification"}) or event.get("isCompactSummary")
        own = bool(event.get("isSidechain")) == subagent
        if role not in {"user", "assistant"} or not own or meta:
            if idle and own and role == "user" and event.get("isMeta"):
                # A scheduled prompt starts a turn.
                previous, idle = ("user", max(at, previous[1]) if previous else at), False
            continue
        day = at.date().isoformat()
        message = event.get("message") or {}
        content = message.get("content") or []
        blocks = [b for b in content if isinstance(b, dict)] if isinstance(content, list) else []
        results = [b for b in blocks if b.get("type") == "tool_result"]
        text = "" if results else text_of(content).lstrip()
        prompt = role == "user" and not results and (
            origin in PROMPTS or ("origin" not in event and not text.startswith("<"))
        )
        if role == "assistant":
            # Some wakes, such as a CI monitor event, are not always written to the transcript.
            bucket = None if idle else "model"
            # One response spans several lines, and each repeats its usage.
            counts = message.get("usage") or {}
            seen = usage.setdefault(str(message.get("id")), (day, {}))[1]
            for field, name in TOKENS.items():
                seen[field] = max(seen.get(field, 0), int(counts.get(name) or 0))
        elif prompt or idle or origin == "task-notification" or (not results and previous and previous[0] == "assistant"):
            # A new turn. A call with no result, such as an interrupted one, no longer holds the session.
            pending.clear()
            task = TASK_END.search(text)
            if prompt:
                bucket = None
            elif text.startswith("<ci-monitor-event"):
                bucket = "ci"
            elif task and task.group(2) != "stopped":
                bucket = launched.get(task.group(1))
            else:
                # A message from another session, which counts its own time.
                bucket = None
        elif results or pending:
            bucket = max([p[0] for p in pending.values()] or ["tool"], key=PRIORITY.__getitem__)
        else:
            bucket = "model"
        if previous and bucket and bucket != "agent":
            days[day]["tools" if bucket == "tool" else bucket] += max(0.0, (at - previous[1]).total_seconds())
        for block in blocks:
            if role == "assistant" and block.get("type") == "tool_use":
                name, tool_input = str(block.get("name", "")), block.get("input") or {}
                kind, key = tool_key(name, tool_input)
                pending[block.get("id")] = (kind, key, at)
                launched[str(block.get("id"))] = tool_key(name, {**tool_input, "run_in_background": False})[0]
        for result in results:
            kind, key, started = pending.pop(result.get("tool_use_id"), ("tool", "unknown", at))
            output = text_of(result.get("content") or "")
            entry = tools[(day, "tool" if kind == "agent" else kind, key)]
            entry["calls"] += 1
            entry["seconds"] += max(0.0, (at - started).total_seconds())
            entry["failed"] += bool(result.get("is_error") or (kind == "build" and FAILURE.search(output)))
            entry["chars"] += len(output)
        previous = (role, max(at, previous[1]) if previous else at)
        idle = False
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
        "days": [{"day": d, **{f: round(v[f], 1) if f in BUCKETS else int(v[f]) for f in DAY_FIELDS}} for d, v in sorted(days.items())],
        "tools": kept,
    }


def session_of(hook: dict[str, Any]) -> tuple[dict[str, str], str, str | None]:
    """The session fields of the report, the transcript to read, and the call that started a fork.

    `SubagentStop` gives the main session as `session_id` and the subagent's transcript as
    `agent_transcript_path`. The `.meta.json` file next to that transcript names the agent that
    started a nested subagent, and marks a fork.
    """
    if hook.get("hook_event_name") != "SubagentStop":
        return {"session": str(hook["session_id"]), "kind": "main"}, str(hook["transcript_path"]), None
    path = str(hook["agent_transcript_path"])
    try:
        with open(path.removesuffix(".jsonl") + ".meta.json", encoding="utf-8") as file:
            meta = json.load(file)
    except (OSError, ValueError):
        meta = {}
    return {
        "session": str(hook["agent_id"]).removeprefix("agent-"),
        "kind": "subagent",
        "parent": str(meta.get("parentAgentId") or hook["session_id"]),
        "agent_type": " ".join(str(hook.get("agent_type") or meta.get("agentType") or "").split())[:80],
        "description": " ".join(str(meta.get("description") or "").split())[:100],
    }, path, str(meta["toolUseId"]) if meta.get("isFork") and meta.get("toolUseId") else None


def main() -> None:
    if os.environ.get("OBC_AGENT_TIME", "").lower() in {"off", "0", "false"}:
        return
    try:
        session, path, start = session_of(json.load(sys.stdin))
    except (ValueError, KeyError, TypeError, AttributeError):
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
            body = json.dumps({**session, **report(transcript, session["kind"] == "subagent", start)}).encode()
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
