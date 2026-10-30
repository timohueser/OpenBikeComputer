"""The weekly report: the skipped pull requests, the size estimates, the ratchet, and drift between
the approved system docs and the code.

The `Mode:` line of a pull request body is a record: `change #N`, `fix` or `skip: <reason>`. Nothing
checks it, and the report does not count an unknown value.

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
MODE = re.compile(r"change #(?P<issue>[1-9]\d*)|fix|skip: (?P<reason>\S.*)")
# GitHub hides an unclosed comment up to the end of the body.
COMMENT = re.compile(r"<!--.*?(?:-->|\Z)", re.S)
FENCE = re.compile(r"^```.*?^```", re.S | re.M)
SIZES = ("S", "M", "L")

#: `api(path, method="GET", payload=None)` on `repos/{owner}/{repo}/`.
Api = Callable[..., object]
#: `send(method, url, body)` to the Anthropic API; it gives the response text.
Send = Callable[[str, str, dict | None], str]

INSTRUCTIONS = """\
You check one system document of a software repository against the pull requests merged since \
the document last changed. The document describes the current state of the system and the \
reasons for it.

A pull request with the mode `change #N` gives the TL;DR of its design issue N. Any other pull \
request gives its diff, limited to the files of the system, without tests. The input may be cut at \
a size limit.

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
    GitHub from notifying the user that it names, and `&lt;` keeps `<!--` from hiding the rest of the
    report."""
    return " ".join(str(text).split()).replace("@", "@\u200b").replace("<", "&lt;")


def visible(body: str) -> str:
    """The body without HTML comments and code blocks."""
    return FENCE.sub("", COMMENT.sub("", body))


def pull_mode(pull: dict) -> re.Match | None:
    """The value of the first `Mode:` line of a pull request body, or None when it is unknown."""
    for line in visible(pull.get("body") or "").splitlines():
        if line.startswith("Mode:"):
            return MODE.fullmatch(line[len("Mode:"):].strip())
    return None


def issue_size(body: str) -> str | None:
    """The first letter after `**Size:**` on its line, when it is S, M or L."""
    match = re.search(r"^\*\*Size:\*\*[ \t]*([SML])", visible(body), re.M)
    return match.group(1) if match else None


def git(root: Path, *args: str) -> str | None:
    done = subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True)
    return done.stdout.strip() if done.returncode == 0 else None


def gh(root: Path, endpoint: str, method: str = "GET", payload: dict | None = None):
    """`gh api ENDPOINT`, which reads `GH_TOKEN` and `GH_REPO` in CI. A payload is the JSON body."""
    body = ["--input", "-"] if payload is not None else []
    done = subprocess.run(["gh", "api", "--method", method, endpoint, *body], cwd=root, capture_output=True, text=True,
                          input=json.dumps(payload) if payload is not None else None)
    if done.returncode:
        raise OSError(done.stderr.strip() or "gh api failed")
    return json.loads(done.stdout) if done.stdout.strip() else None


# ── step 1: no tokens ────────────────────────────────────────────────────────


def merged(api: Api, now: datetime) -> list[dict]:
    """The pull requests merged in the 7 days before `now`, by number. The list is newest update
    first, so it stops at a page that ends before the window."""
    since = now - timedelta(days=7)
    found = []
    for page in itertools.count(1):
        pulls = api(f"pulls?state=closed&sort=updated&direction=desc&per_page=100&page={page}")
        found += [pull for pull in pulls if pull.get("merged_at") and datetime.fromisoformat(pull["merged_at"]) >= since]
        if len(pulls) < 100 or datetime.fromisoformat(pulls[-1]["updated_at"]) < since:
            return sorted(found, key=lambda pull: pull["number"])


def skips(pulls: list[dict]) -> list[tuple[int, str, str]]:
    """The pull requests with `Mode: skip: <reason>`, as (number, title, reason)."""
    return [(pull["number"], pull["title"], mode["reason"]) for pull in pulls
            if (mode := pull_mode(pull)) and mode["reason"]]


def is_test(path: str, patterns: list[str]) -> bool:
    """A pattern with no `/` matches the file name only, so `*` cannot cross a folder."""
    name = path.rsplit("/", 1)[-1]
    return any(cairn.glob_matches(path if "/" in pattern else name, pattern) for pattern in patterns)


def diff_lines(root: Path, commit: str, head: str) -> dict[str, int]:
    """Each path that changed between the commit and the head, with its added and removed lines. A
    binary file counts 0."""
    out = subprocess.run(["git", "-C", str(root), "diff", "--numstat", "--no-renames", "-z", commit, head],
                         capture_output=True, text=True, check=True).stdout
    found = {}
    for record in filter(None, out.split("\0")):
        added, removed, path = record.split("\t", 2)
        found[path] = 0 if added == "-" else int(added) + int(removed)
    return found


def measure(root: Path, config: dict, docs: list[cairn.Doc], merge: str) -> tuple[str, list[str]]:
    """The size of a merge against its first parent, and the signals that decide it. Each signal
    gives the smallest size that allows it, and the largest of those is the size. A system doc
    touches only its own system, and the map counts for no system and no lines."""
    parent, folder, limits = f"{merge}^1", config["docs"].rstrip("/") + "/", config["size"]
    lines = diff_lines(root, parent, merge)
    lines.pop(folder + cairn.MAP, None)
    before, after = cairn.shown_at(root, parent), cairn.shown_at(root, merge)
    system_docs, new_docs = {}, []
    for path in lines:
        name = path[len(folder):-len(".md")]
        if path.startswith(folder) and path.endswith(".md") and "/" not in name:
            had, has = (bool(text and cairn.FRONT.match(text)) for text in (before(path), after(path)))
            if had or has:
                system_docs[path] = name
            if has and not had:
                new_docs.append(name)
    others = [path for path in lines if path not in system_docs]
    touched = {s for hits in cairn.owners(others, docs).values() for s in hits} | set(system_docs.values())
    changed = sum(n for path, n in lines.items() if not is_test(path, config["tests"]))
    specs = sorted(path for path in lines if path.startswith(config["specs"].rstrip("/") + "/"))

    def fit(count: int, key: str) -> str:
        return next((s for s in SIZES[:2] if count <= limits[s][key]), "L")

    signals = [(fit(len(touched), "systems"), f"{len(touched)} system{'' if len(touched) == 1 else 's'}"),
               (fit(changed, "lines"), f"{changed:,} changed lines outside tests")]
    signals += [("M", f"a contract change in {', '.join(specs)}")] if specs else []
    signals += [("L", f"a new system doc for {name}") for name in sorted(new_docs)]
    found = max((s for s, _ in signals), key=SIZES.index)
    return found, [text for s, text in signals if s == found]


def estimate(root: Path, config: dict, docs: list[cairn.Doc], api: Api, merge: str, issue: int) -> tuple[str, str, list[str]]:
    """The planned and the measured size of a change, and the signals of the measured size. A size
    that cannot be read is `unknown`, and the signals say why. The docs and the config are today's,
    not those of the merge."""
    planned, measured, why = "unknown", "unknown", []
    try:
        planned = issue_size(api(f"issues/{issue}").get("body") or "") or "unknown"
        if planned == "unknown":
            why.append(f"issue #{issue} has no Size line")
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        why.append(f"issue #{issue} cannot be read: {error}")
    if not merge:
        return planned, measured, [*why, "the pull request has no merge commit"]
    if git(root, "rev-parse", "--verify", "--quiet", f"{merge}^1") is None:
        return planned, measured, [*why, f"the merge commit {merge[:12]} is not in this checkout"]
    try:
        measured, signals = measure(root, config, docs, merge)
        why += signals
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        why.append(f"the merge commit {merge[:12]} cannot be read: {error}")
    return planned, measured, why


def size_lines(root: Path, config: dict, docs: list[cairn.Doc], api: Api, pulls: list[dict]) -> list[str]:
    rows, counts = [], Counter()
    for pull in pulls:
        mode = pull_mode(pull)
        if not mode or not mode["issue"]:
            continue
        issue = int(mode["issue"])
        planned, measured, why = estimate(root, config, docs, api, pull.get("merge_commit_sha") or "", issue)
        # A pull request that cites an L issue is one part of an epic, so it cannot be larger.
        if planned == "L":
            counts["against an epic"] += 1
        elif "unknown" in (planned, measured):
            counts["unknown"] += 1
        else:
            order = SIZES.index(measured) - SIZES.index(planned)
            counts["as planned" if order == 0 else "larger" if order > 0 else "smaller"] += 1
        rows.append(f"| #{pull['number']} | #{issue} | {planned} | {measured} | {untrusted('; '.join(why)).replace('|', '/')} |")
    if not rows:
        return ["None."]
    kinds = ["as planned", "larger", "smaller"] + [kind for kind in ("against an epic", "unknown") if counts[kind]]
    return [f"{len(rows)} pull request(s): {', '.join(f'{counts[kind]} {kind}' for kind in kinds)}.", "",
            "| Pull request | Issue | Planned | Measured | Signals |", "| --- | --- | --- | --- | --- |", *rows]


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
    for line in (git(root, "log", "--first-parent", "--format=%H %s", f"{since}..HEAD") or "").splitlines():
        commit, _, subject = line.partition(" ")
        match = MERGE.match(subject)
        if match:
            found.append((commit, int(match.group(1))))
    return found


def tldr(body: str) -> str:
    match = re.search(r"^## TL;DR[ \t]*\n(.*?)(?=^---[ \t]*$|^## |\Z)", body, re.S | re.M)
    return match.group(1).strip() if match else "The issue has no TL;DR section."


def evidence(root: Path, read: Api, number: int, commit: str, files: list[str]) -> str:
    """A `change #N` gives the TL;DR of issue N, because it had to update the doc already. Any other
    pull request gives its diff on the files."""
    pull = read(f"pulls/{number}")
    mode = pull_mode(pull)
    head = f"## Pull request #{number}: {' '.join(pull.get('title', '').split())}\nMode: {mode[0] if mode else 'none'}\n\n"
    if mode and mode["issue"]:
        return head + tldr(read(f"issues/{mode['issue']}").get("body") or "")
    return head + (git(root, "diff", "--no-renames", f"{commit}^1", commit, "--", *files) or "")


def drift_inputs(root: Path, docs: list[cairn.Doc], config: dict, read: Api) -> list[Inputs]:
    """For each approved doc, the pull requests merged since the doc last changed that touch the
    files of its system outside the `tests` and `report.ignore` globs and the map, newest first."""
    folder = config["docs"].rstrip("/")
    dropped, limit = config["tests"] + config["report"]["ignore"], config["report"]["max_input_chars"]
    found = []
    for doc in docs:
        path = f"{folder}/{doc.system}.md"
        since = git(root, "log", "-1", "--first-parent", "--format=%H", "HEAD", "--", path) if doc.status == "approved" else None
        parts, numbers = [], []
        for commit, number in merges(root, since) if since else []:
            changed = (git(root, "diff", "--name-only", "--no-renames", "-z", f"{commit}^1", commit) or "").split("\0")
            kept = [f for f in changed if f and f != f"{folder}/{cairn.MAP}" and not is_test(f, dropped)]
            files = sorted(f for f, hits in cairn.owners(kept, docs).items() if doc.system in hits)
            if files:
                numbers.append(number)
                parts.append(evidence(root, read, number, commit, files))
        text = "\n\n".join(parts)
        if parts:
            found.append(Inputs(doc.system, path, tuple(numbers), text[:limit], len(text) if len(text) > limit else 0))
    return found


# ── step 2: one batch ────────────────────────────────────────────────────────


class ApiError(OSError):
    """An HTTP error answer of the Anthropic API."""

    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code


def sender(key: str) -> Send:
    def send(method: str, url: str, body: dict | None) -> str:
        data = None if body is None else json.dumps(body).encode()
        headers = {"x-api-key": key, "anthropic-version": VERSION, "content-type": "application/json"}
        try:
            with urllib.request.urlopen(urllib.request.Request(url, data, headers, method=method), timeout=120) as answer:
                return answer.read().decode()
        except urllib.error.HTTPError as error:
            raise ApiError(error.code, f"{method} {url}: HTTP {error.code}: {error.read().decode(errors='replace')[:500]}") from error
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


def attempt(batch: str, action: Callable[[], object]) -> object | None:
    """The result of `action`, or None after an error that can pass: a network error, HTTP 429 or
    HTTP 5xx. Any other HTTP error is permanent, such as a revoked key, so it stops the run."""
    try:
        return action()
    except ApiError as error:
        if error.code != 429 and error.code < 500:
            raise cairn.CairnError(f"batch {batch}: {error}") from error
    except OSError:
        pass
    return None


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
    output = {"effort": chosen[1], "format": {"type": "json_schema", "schema": SCHEMA}}
    prefix = call(send, "POST", "messages/count_tokens", {"model": out.model, "system": system, "output_config": output,
                                                          "messages": [{"role": "user", "content": "."}]})["input_tokens"]
    requests = []
    for i, item in enumerate(inputs):
        doc = (root / item.doc).read_text(encoding="utf-8")
        params = {
            "model": out.model,
            "max_tokens": settings["max_tokens"],
            "system": system,
            "messages": [{"role": "user", "content": f'<system_doc path="{item.doc}">\n{doc}\n</system_doc>\n\n'
                                                     f"<changes>\n{item.text}\n</changes>"}],
            "output_config": output,
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
    identity, path = batch["id"], f"messages/batches/{batch['id']}"
    deadline = clock() + settings["timeout_minutes"] * 60
    # A canceled batch still ends, and its results hold the requests that ran, which are paid for.
    while batch["processing_status"] != "ended":
        if clock() > deadline and not out.canceled:
            out.canceled = attempt(identity, lambda: call(send, "POST", f"{path}/cancel")) is not None
        sleep(POLL_SECONDS)
        batch = attempt(identity, lambda: call(send, "GET", path)) or batch
    results = None
    while results is None:
        results = attempt(identity, lambda: send("GET", batch["results_url"], None))
        if results is None:
            sleep(POLL_SECONDS)
    for line in results.splitlines():
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
    # The size estimates and the drift inputs read the same pull requests and issues.
    read = functools.cache(api)
    lines = ["# cairn report", "", f"Written on {now:%Y-%m-%d} at {git(root, 'rev-parse', '--short', 'HEAD')}.",
             "", "## Skips merged in the last 7 days", ""]
    try:
        pulls = merged(read, now)
        rows = [f"| #{n} | {untrusted(t).replace('|', '/')} | {untrusted(r).replace('|', '/')} |" for n, t, r in skips(pulls)]
        lines += ["| Pull request | Title | Reason |", "| --- | --- | --- |", *rows] if rows else ["None."]
        lines += ["", "## Size estimates of the changes merged in the last 7 days", "", *size_lines(root, config, docs, read, pulls)]
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
        inputs = drift_inputs(root, docs, config, read)
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
        def api(path: str, method: str = "GET", payload: dict | None = None):
            return gh(root, f"repos/{{owner}}/{{repo}}/{path}", method, payload)

        text, failed = write(root, config, docs, result, baseline, api, drifting, os.environ.get("ANTHROPIC_API_KEY"),
                             datetime.now(timezone.utc))
        print(text)
        if publishing:
            def pin(node: str) -> None:
                answer = gh(root, "graphql", "POST", {"query": PIN, "variables": {"id": node}})
                if (answer or {}).get("errors"):
                    raise OSError(json.dumps(answer["errors"]))

            print(f"cairn: wrote {publish(text, api, pin)}", file=sys.stderr)
    except (cairn.CairnError, OSError, RuntimeError, KeyError, ValueError, subprocess.CalledProcessError) as error:
        print(f"cairn: {error}", file=sys.stderr)
        return 1
    return 1 if failed else 0
