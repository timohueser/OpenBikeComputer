"""The weekly report: the overrides, the ratchet, and drift between the approved system docs and the code.

Step 1 reads git and GitHub and costs no tokens. For each approved doc, it collects the pull
requests merged since the doc last changed that touch the files of its system. Step 2 sends one
request for each such system in one Message Batch, within the run budget. Each request asks which
statements of the doc are now false and which behaviour the doc does not state.
"""

from __future__ import annotations

import functools
import http.client
import itertools
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from collections import Counter
from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path

import cairn
import mode as modes

API = "https://api.anthropic.com/v1"
VERSION = "2026-10-21"
LABEL = "cairn-report"
TITLE = "cairn report"
GLOSSARY = "glossary.md"
MERGE = re.compile(r"Merge pull request #([1-9]\d*)\b")
# A batch can run longer than the default cache lifetime of five minutes.
CACHE = {"type": "ephemeral", "ttl": "1h"}
# A one-hour cache write costs twice the input price.
CACHE_WRITE = 2
POLL_SECONDS = 60
# GitHub refuses an issue body longer than 65,536 characters.
BODY_LIMIT = 65_000
PIN = "mutation($id: ID!) { pinIssue(input: {issueId: $id}) { issue { number } } }"

#: `api(path, method="GET", payload=None)` on `repos/{owner}/{repo}/`.
Api = Callable[..., object]
#: `send(method, url, body)` to the Anthropic API; it gives the response text.
Send = Callable[[str, str, dict | None], str]

INSTRUCTIONS = """\
You check one system document of a software repository against the pull requests merged since \
the document last changed. The document describes the current state of the system and the \
reasons for it.

Each pull request has a mode. A `change` gives the TL;DR of its approved design issue. A `fix` or \
an `override` gives its diff, limited to the files of the system, without tests. The input may be \
cut at a size limit.

Report only what the pull requests prove:
- false_statements: a statement in the document that the pull requests make false. Give the exact \
quote, the evidence (the pull request and what it changed), and a replacement for the quote.
- missing_behaviour: behaviour that the pull requests add and that the document must state but \
does not. Give the behaviour, the evidence, and one sentence for the document.

Do not report style, wording, or parts that the pull requests do not touch. When the document is \
still correct, return two empty lists.\
"""

FINDINGS = {"false_statements": ("quote", "evidence", "replacement"),
            "missing_behaviour": ("behaviour", "evidence", "sentence")}
SCHEMA = {
    "type": "object",
    "additionalProperties": False,
    "required": list(FINDINGS),
    "properties": {name: {"type": "array", "items": {
        "type": "object", "additionalProperties": False, "required": list(keys),
        "properties": {key: {"type": "string"} for key in keys}}} for name, keys in FINDINGS.items()},
}


def untrusted(text: str) -> str:
    """Text from a pull request or the model, on one line. A zero-width space after each `@` keeps
    GitHub from notifying the user that it names."""
    return " ".join(str(text).split()).replace("@", "@\u200b")


def pull_mode(pull: dict) -> modes.Mode | None:
    return modes.read(modes.body_value(pull.get("body") or ""), "the pull request body")[0]


# ── step 1: no tokens ────────────────────────────────────────────────────────


def overrides(api: Api, now: datetime) -> list[tuple[int, str, str]]:
    """The pull requests merged in the 7 days before `now` whose mode is an override, as (number,
    title, reason). The list is newest update first, so it stops at a page that ends before the
    window."""
    since = now - timedelta(days=7)
    found = []
    for page in itertools.count(1):
        pulls = api(f"pulls?state=closed&sort=updated&direction=desc&per_page=100&page={page}")
        for pull in pulls:
            mode = pull_mode(pull)
            if pull.get("merged_at") and datetime.fromisoformat(pull["merged_at"]) >= since and mode and mode.kind == "override":
                found.append((pull["number"], pull["title"], mode.reason))
        if len(pulls) < 100 or datetime.fromisoformat(pulls[-1]["updated_at"]) < since:
            return sorted(found)


@dataclass(frozen=True)
class Inputs:
    system: str
    doc: str
    pulls: tuple[int, ...]
    text: str
    #: The size of the text before the cap, or 0 when the cap did not cut it.
    cut: int = 0


def merges(root: Path, since: str) -> list[tuple[str, int]]:
    """The merge commits of pull requests after `since` on the first-parent line of HEAD, newest
    first, with their numbers."""
    found = []
    for line in (modes.git(root, "log", "--first-parent", "--format=%H %s", f"{since}..HEAD") or "").splitlines():
        commit, _, subject = line.partition(" ")
        match = MERGE.match(subject)
        if match:
            found.append((commit, int(match.group(1))))
    return found


def tldr(body: str) -> str:
    match = re.search(r"^## TL;DR[ \t]*\n(.*?)(?=^---[ \t]*$|^## |\Z)", body, re.S | re.M)
    return match.group(1).strip() if match else "The issue has no TL;DR section."


def evidence(root: Path, read: Api, number: int, commit: str, files: list[str]) -> str:
    """A `change` gives the TL;DR of its issue, because it had to update the doc already. Any other
    pull request gives its diff on the files."""
    pull = read(f"pulls/{number}")
    mode = pull_mode(pull)
    head = f"## Pull request #{number}: {untrusted(pull.get('title', ''))}\nMode: {mode.line() if mode else 'none'}\n\n"
    if mode and mode.kind == "change":
        return head + tldr(modes.issue_body(mode.issue, read(f"issues/{mode.issue}")))
    return head + (modes.git(root, "diff", "--no-renames", f"{commit}^1", commit, "--", *files) or "")


def drift_inputs(root: Path, docs: list[cairn.Doc], config: dict, api: Api) -> list[Inputs]:
    """For each approved doc, the pull requests merged since the doc last changed that touch the
    files of its system outside the `fix.tests` and `report.ignore` globs and the map, newest first."""
    folder = config["docs"].rstrip("/")
    dropped, limit = config["fix"]["tests"] + config["report"]["ignore"], config["report"]["max_input_chars"]
    read = functools.cache(api)
    found = []
    for doc in docs:
        path = f"{folder}/{doc.system}.md"
        since = modes.git(root, "log", "-1", "--first-parent", "--format=%H", "HEAD", "--", path) if doc.status == "approved" else None
        parts, numbers = [], []
        for commit, number in merges(root, since) if since else []:
            changed = (modes.git(root, "diff", "--name-only", "--no-renames", "-z", f"{commit}^1", commit) or "").split("\0")
            kept = [f for f in changed if f and f != f"{folder}/{cairn.MAP}" and not cairn.is_test(f, dropped)]
            files = sorted(f for f, hits in cairn.owners(kept, docs).items() if doc.system in hits)
            if files:
                numbers.append(number)
                parts.append(evidence(root, read, number, commit, files))
        text = "\n\n".join(parts)
        if parts:
            found.append(Inputs(doc.system, path, tuple(numbers), text[:limit], len(text) if len(text) > limit else 0))
    return found


# ── step 2: one batch ────────────────────────────────────────────────────────


def sender(key: str) -> Send:
    def send(method: str, url: str, body: dict | None) -> str:
        data = None if body is None else json.dumps(body).encode()
        headers = {"x-api-key": key, "anthropic-version": VERSION, "content-type": "application/json"}
        try:
            with urllib.request.urlopen(urllib.request.Request(url, data, headers, method=method), timeout=120) as answer:
                return answer.read().decode()
        except urllib.error.HTTPError as error:
            raise OSError(f"{method} {url}: HTTP {error.code}: {error.read().decode(errors='replace')[:500]}") from error
        except http.client.HTTPException as error:
            raise OSError(f"{method} {url}: {error!r}") from error

    return send


def call(send: Send, method: str, path: str, body: dict | None = None) -> dict:
    return json.loads(send(method, f"{API}/{path}", body))


def resolve(send: Send, name: str) -> str:
    """An exact model ID is a pin. Any other name is a family prefix, which gives its newest model."""
    models = call(send, "GET", "models?limit=1000")["data"]
    if any(m["id"] == name for m in models):
        return name
    family = [m for m in models if m["id"].startswith(name + "-")]
    if not family:
        raise cairn.CairnError(f"the Models API lists no model {name} and no model whose ID starts with {name}-")
    return max(family, key=lambda m: m["created_at"])["id"]


def cost(usage: dict, price: dict) -> float:
    """The cost in USD of the usage of one request at batch prices. Each cache write is a one-hour
    write."""
    return (usage.get("input_tokens", 0) * price["input"]
            + usage.get("cache_creation_input_tokens", 0) * price["input"] * CACHE_WRITE
            + usage.get("cache_read_input_tokens", 0) * price["cache_read"]
            + usage.get("output_tokens", 0) * price["output"]) / 1e6


def findings(text: str) -> dict[str, list[dict[str, str]]]:
    """The answer, checked against SCHEMA. Structured outputs hold the shape unless the answer
    stops early, at `max_tokens` or at a refusal."""
    data = json.loads(text)
    if not isinstance(data, dict) or set(data) != set(FINDINGS):
        raise ValueError(f"the keys are not {', '.join(FINDINGS)}")
    for name, keys in FINDINGS.items():
        if not isinstance(data[name], list) or any(
                not isinstance(item, dict) or set(item) != set(keys) or not all(isinstance(v, str) for v in item.values())
                for item in data[name]):
            raise ValueError(f"each item of {name} needs the strings {', '.join(keys)}")
    return data


def outcome(result: dict) -> dict | str:
    """The findings of one batch result, or why it has none."""
    if result["type"] != "succeeded":
        return f"the request {result['type']}: {json.dumps(result.get('error'))[:300]}"
    message = result["message"]
    try:
        return findings("".join(b.get("text", "") for b in message["content"] if b["type"] == "text"))
    except ValueError as error:
        return f"bad JSON (stop reason {message.get('stop_reason')}): {error}"


@dataclass
class Drift:
    model: str
    budget: float
    #: The worst-case cost of the requests in the batch.
    estimate: float = 0.0
    #: The cost of the usage that the results report.
    spent: float = 0.0
    canceled: bool = False
    skipped: list[str] = field(default_factory=list)
    #: The findings of each checked system, or why it has none.
    found: dict[str, dict | str] = field(default_factory=dict)


def drift(send: Send, root: Path, inputs: list[Inputs], config: dict,
          sleep: Callable[[float], None] = time.sleep, clock: Callable[[], float] = time.monotonic) -> Drift:
    """Count the tokens of each request and stop at the run budget, then send one batch and wait
    for it. The worst case writes the shared prefix to the cache in each request and uses all of
    `max_tokens`."""
    settings = config["report"]
    chosen = cairn.model_for(config, "drift", "api")
    if chosen is None:
        raise cairn.CairnError("the drift role has no model in the api tier")
    out = Drift(resolve(send, chosen[0]), settings["budget_usd"])
    price = settings["batch_prices"].get(out.model)
    if price is None:
        raise cairn.CairnError(f"no price for {out.model}; add it to {cairn.CONFIG}")
    glossary = root / config["docs"] / GLOSSARY
    system = [{"type": "text", "cache_control": CACHE,
               "text": INSTRUCTIONS + (f"\n\n<glossary>\n{glossary.read_text()}\n</glossary>" if glossary.exists() else "")}]
    prefix = call(send, "POST", "messages/count_tokens",
                  {"model": out.model, "system": system, "messages": [{"role": "user", "content": "."}]})["input_tokens"]
    requests = []
    for i, item in enumerate(inputs):
        doc = (root / item.doc).read_text(encoding="utf-8")
        params = {
            "model": out.model,
            "max_tokens": settings["max_tokens"],
            "system": system,
            "messages": [{"role": "user", "content": f'<system_doc path="{item.doc}">\n{doc}\n</system_doc>\n\n'
                                                     f"<changes>\n{item.text}\n</changes>"}],
            "output_config": {"effort": chosen[1], "format": {"type": "json_schema", "schema": SCHEMA}},
        }
        tokens = call(send, "POST", "messages/count_tokens", {k: v for k, v in params.items() if k != "max_tokens"})["input_tokens"]
        worst = cost({"cache_creation_input_tokens": prefix, "input_tokens": max(tokens - prefix, 0),
                      "output_tokens": settings["max_tokens"]}, price)
        if out.estimate + worst > out.budget:
            out.skipped = [later.system for later in inputs[i:]]
            break
        out.estimate += worst
        requests.append({"custom_id": item.system, "params": params})
    if not requests:
        return out
    batch = call(send, "POST", "messages/batches", {"requests": requests})
    path = f"messages/batches/{batch['id']}"
    deadline = clock() + settings["timeout_minutes"] * 60
    # A failed request tries again at the next poll. A canceled batch still ends, and its results
    # hold the requests that ran, which are paid for.
    while batch["processing_status"] != "ended":
        if clock() > deadline and not out.canceled:
            try:
                call(send, "POST", f"{path}/cancel")
                out.canceled = True
            except OSError:
                pass
        sleep(POLL_SECONDS)
        try:
            batch = call(send, "GET", path)
        except OSError:
            continue
    for line in send("GET", batch["results_url"], None).splitlines():
        if line.strip():
            row = json.loads(line)
            out.found[row["custom_id"]] = outcome(row["result"])
            if row["result"]["type"] == "succeeded":
                out.spent += cost(row["result"]["message"].get("usage", {}), price)
    return out


# ── the report ───────────────────────────────────────────────────────────────


def drift_lines(result: Drift, timeout: int) -> list[str]:
    costs = f"worst-case estimate ${result.estimate:.2f}, real cost ${result.spent:.2f}, run budget ${result.budget:g}"
    lines = [f"Model {result.model}; {costs}."]
    if result.canceled:
        lines += ["", f"The batch did not end in {timeout} minutes, so it was canceled. Requests that did not run have no findings."]
    if result.skipped:
        lines += ["", f"Skipped at the run budget: {', '.join(result.skipped)}."]
    for system, found in sorted(result.found.items()):
        lines += ["", f"### {system}", ""]
        if isinstance(found, str):
            lines.append(f"No findings: {untrusted(found)}.")
            continue
        for item in found["false_statements"]:
            lines += [f"- **False:** {untrusted(item['quote'])}", f"  - Evidence: {untrusted(item['evidence'])}",
                      f"  - Replace with: {untrusted(item['replacement'])}"]
        for item in found["missing_behaviour"]:
            lines += [f"- **Missing:** {untrusted(item['behaviour'])}", f"  - Evidence: {untrusted(item['evidence'])}",
                      f"  - Add: {untrusted(item['sentence'])}"]
        if not found["false_statements"] and not found["missing_behaviour"]:
            lines.append("The doc still holds.")
    return lines


def write(root: Path, config: dict, docs: list[cairn.Doc], result: cairn.Result, baseline: dict[str, int],
          api: Api, drifting: bool, key: str | None, now: datetime) -> tuple[str, bool]:
    """The report, and whether a part of it failed. Step 2 runs only when `drifting`."""
    failed = False
    lines = ["# cairn report", "", f"Written on {now:%Y-%m-%d} at {modes.git(root, 'rev-parse', '--short', 'HEAD')}.",
             "", "## Overrides merged in the last 7 days", ""]
    try:
        found = overrides(api, now)
        rows = [f"| #{n} | {untrusted(t).replace('|', '/')} | {untrusted(r).replace('|', '/')} |" for n, t, r in found]
        lines += ["| Pull request | Title | Reason |", "| --- | --- | --- |", *rows] if rows else ["None."]
    except (OSError, ValueError, KeyError, TypeError) as error:
        failed = True
        lines.append(f"The pull requests cannot be read: {error}")

    counts = Counter(system for v in result.violations for system in v.systems)
    lines += ["", "## Ratchet", "", f"{len(result.violations)} violations; the baseline holds {len(baseline)}.", "",
              "| System | Violations |", "| --- | --- |",
              *(f"| {system} | {n} |" for system, n in sorted(counts.items(), key=lambda pair: (-pair[1], pair[0])))]
    unowned = sorted(v.id.removeprefix("unowned ") for v in result.violations if v.id.startswith("unowned "))
    lines += ["", "## Files with no owner", "", *([f"- `{path}`" for path in unowned] or ["None."])]

    lines += ["", "## Drift", ""]
    try:
        inputs = drift_inputs(root, docs, config, api)
    except (OSError, ValueError, KeyError, TypeError) as error:
        return "\n".join([*lines, f"The inputs cannot be read: {error}"]), True
    if not inputs:
        approved = sum(doc.status == "approved" for doc in docs)
        return "\n".join([*lines, f"{approved} approved doc(s); no pull request touched their systems since they changed."]), failed
    for item in inputs:
        cut = f"; cut from {item.cut:,} to {len(item.text):,} characters" if item.cut else ""
        lines.append(f"- {item.system}: {', '.join(f'#{n}' for n in item.pulls)}{cut}")
    lines.append("")
    if not drifting:
        return "\n".join([*lines, "Step 2 runs only with `--drift`, because it spends tokens."]), failed
    if not key:
        return "\n".join([*lines, "Step 2 is skipped: `ANTHROPIC_API_KEY` is not set."]), True
    try:
        lines += drift_lines(drift(sender(key), root, inputs, config), config["report"]["timeout_minutes"])
    except (OSError, ValueError, KeyError, TypeError, cairn.CairnError) as error:
        failed = True
        lines.append(f"Step 2 failed: {error}")
    return "\n".join(lines), failed


def publish(body: str, api: Api, pin: Callable[[str], object]) -> str:
    """Replace the body of the open issue labelled cairn-report, or create that issue and pin it.
    A failed pin only adds a note to the body. Gives the URL of the issue."""
    if len(body) > BODY_LIMIT:
        body = body[:BODY_LIMIT] + "\n\nThe report is cut here, at the size limit of an issue body."
    issues = [issue for issue in api(f"issues?labels={LABEL}&state=open") if "pull_request" not in issue]
    if issues:
        return api(f"issues/{issues[0]['number']}", "PATCH", {"body": body})["html_url"]
    try:
        api(f"labels/{LABEL}")
    except OSError as error:
        if "Not Found" not in str(error):
            raise
        api("labels", "POST", {"name": LABEL, "color": "5319e7", "description": "The weekly cairn report"})
    issue = api("issues", "POST", {"title": TITLE, "body": body, "labels": [LABEL]})
    try:
        pin(issue["node_id"])
    except OSError as error:
        api(f"issues/{issue['number']}", "PATCH", {"body": f"{body}\n\nThe issue could not be pinned: {error}"})
    return issue["html_url"]


def main(publishing: bool, drifting: bool) -> int:
    root = cairn.ROOT
    try:
        config = json.loads((root / cairn.CONFIG).read_text())
        docs, files, units, edges, lines = cairn.gather(root, config)
        result = cairn.analyse(docs, files, units, edges, lines, config)
        path = root / config["baseline"]
        baseline = json.loads(path.read_text()) if path.exists() else {}
        api = functools.partial(modes.api, root)
        text, failed = write(root, config, docs, result, baseline, api, drifting, os.environ.get("ANTHROPIC_API_KEY"),
                             datetime.now(timezone.utc))
        print(text)
        if publishing:
            def pin(node: str) -> None:
                answer = modes.gh(root, "graphql", "POST", {"query": PIN, "variables": {"id": node}})
                if (answer or {}).get("errors"):
                    raise OSError(json.dumps(answer["errors"]))

            print(f"cairn: wrote {publish(text, api, pin)}", file=sys.stderr)
    except (cairn.CairnError, OSError, RuntimeError, KeyError, ValueError, subprocess.CalledProcessError) as error:
        print(f"cairn: {error}", file=sys.stderr)
        return 1
    return 1 if failed else 0
