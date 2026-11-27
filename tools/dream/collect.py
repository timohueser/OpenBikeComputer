#!/usr/bin/env python3
"""Write the pull requests merged since the last dream run as Markdown for the dream agent.

Only text from accounts with write access reaches the output. Number, changed paths and diff
stats of each pull request always do.

Usage: python3 tools/dream/collect.py --out DIR [--since ISO8601]
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from datetime import UTC, datetime, timedelta
from pathlib import Path

# In a repository of a user account, each of these has write access.
TRUSTED = {"OWNER", "COLLABORATOR"}
AREA_HEAD = "dream/pages"
WINDOW = re.compile(r"^Dream window: (\S+) to (\S+)$", re.M)
LEARNED = re.compile(r"^Learned:", re.M)
RULE = re.compile(r"^-{3,}[ \t]*$", re.M)
STAMP = "%Y-%m-%dT%H:%M:%SZ"

# The window when no dream pull request has merged and no `--since` is given.
DEFAULT_DAYS = 7
# Merged `dream/pages` pull requests read to find the newest merge: a late comment reorders them.
MARKER_PULLS = 20
# The largest page the GraphQL API returns.
PAGE = 100
# Characters of a body; the `Learned:` section is taken out before the cut.
BODY_CHARS = 4_000
# Characters of a `Learned:` section.
LEARNED_CHARS = 2_000
# Characters of each comment, review and review comment.
COMMENT_CHARS = 1_000
# Changed paths listed in a record; the rest is a count.
PATHS = 100
# Conversation comments, reviews, and comments of each review read for one record.
COMMENTS, REVIEWS, REVIEW_COMMENTS = 40, 50, 30

MARKER_QUERY = f"""
query($owner: String!, $name: String!) {{
  repository(owner: $owner, name: $name) {{
    owner {{ __typename }}
    pullRequests(headRefName: "{AREA_HEAD}", baseRefName: "develop", states: MERGED,
                 first: {MARKER_PULLS}, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
      nodes {{ number mergedAt isCrossRepository body }}
    }}
  }}
}}"""

LIST_QUERY = f"""
query($owner: String!, $name: String!, $cursor: String) {{
  repository(owner: $owner, name: $name) {{
    pullRequests(baseRefName: "develop", states: MERGED, first: {PAGE}, after: $cursor,
                 orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
      pageInfo {{ hasNextPage endCursor }}
      nodes {{ number mergedAt updatedAt headRefName }}
    }}
  }}
}}"""

RECORD_QUERY = f"""
query($owner: String!, $name: String!, $number: Int!) {{
  repository(owner: $owner, name: $name) {{
    pullRequest(number: $number) {{
      number title body mergedAt additions deletions authorAssociation author {{ login }}
      files(first: {PATHS}) {{ totalCount nodes {{ path additions deletions }} }}
      comments(first: {COMMENTS}) {{ nodes {{ authorAssociation author {{ login }} body }} }}
      reviews(first: {REVIEWS}) {{
        nodes {{
          authorAssociation author {{ login }} state body
          comments(first: {REVIEW_COMMENTS}) {{ nodes {{ authorAssociation author {{ login }} path body }} }}
        }}
      }}
    }}
  }}
}}"""


def graphql(query: str, **variables: str | int | None) -> dict:
    """The one call to GitHub."""
    args = ["gh", "api", "graphql", "-f", f"query={query}"]
    for key, value in variables.items():
        if value is not None:
            # `-F` would read a value that starts with `@` as a file name.
            args += ["-F" if isinstance(value, int) else "-f", f"{key}={value}"]
    done = subprocess.run(args, capture_output=True, text=True)
    if done.returncode:
        raise SystemExit(f"collect: gh api graphql failed: {done.stderr.strip()}")
    return json.loads(done.stdout)["data"]


def trusted(node: dict) -> bool:
    """Whether the text of a pull request, comment or review may reach the agent."""
    return node.get("authorAssociation") in TRUSTED


def stamp(text: str) -> str:
    """The canonical form; GitHub uses it too, so stamps compare as strings."""
    try:
        return datetime.strptime(text, STAMP).strftime(STAMP)
    except ValueError:
        raise SystemExit(f"collect: {text!r} is not a UTC time like 2026-11-27T04:31:00Z") from None


def window_start(repo: tuple[str, str], since: str | None, through: str) -> str:
    data = graphql(MARKER_QUERY, owner=repo[0], name=repo[1])["repository"]
    if data["owner"]["__typename"] != "User":
        raise SystemExit("collect: the owner of the repository is an organization; "
                         "OWNER and COLLABORATOR do not mean write access there")
    merged = [p for p in data["pullRequests"]["nodes"] if not p["isCrossRepository"]]
    if merged:
        last = max(merged, key=lambda p: p["mergedAt"])
        found = WINDOW.search((last["body"] or "").replace("\r\n", "\n"))
        if not found:
            raise SystemExit(f"collect: the dream pull request {last['number']} has no line "
                             "'Dream window: <since> to <through>'")
        return stamp(found[2])
    if since:
        return stamp(since)
    return (datetime.strptime(through, STAMP) - timedelta(days=DEFAULT_DAYS)).strftime(STAMP)


def merged_numbers(repo: tuple[str, str], since: str, through: str) -> list[int]:
    """The pull requests merged in (since, through], newest merge first."""
    found, cursor = [], None
    while True:
        page = graphql(LIST_QUERY, owner=repo[0], name=repo[1], cursor=cursor)
        pulls = page["repository"]["pullRequests"]
        found += [
            p for p in pulls["nodes"]
            if since < p["mergedAt"] <= through and not p["headRefName"].startswith("dream/")
        ]
        # A pull request updates when it merges, so all later pages merged before `since`.
        if not pulls["pageInfo"]["hasNextPage"] or not pulls["nodes"] or pulls["nodes"][-1]["updatedAt"] < since:
            break
        cursor = pulls["pageInfo"]["endCursor"]
    return [p["number"] for p in sorted(found, key=lambda p: p["mergedAt"], reverse=True)]


def split_learned(body: str) -> tuple[str, str]:
    """The body without its last `Learned:` section, and that section."""
    starts = list(LEARNED.finditer(body))
    if not starts:
        return body, ""
    start = starts[-1].start()
    end = RULE.search(body, start)
    stop = end.start() if end else len(body)
    section = body[start:stop].strip()
    # The template of a pull request has the bare line.
    if section == "Learned:":
        return body, ""
    return body[:start] + body[stop:], section


def quote(text: str, limit: int) -> str:
    """A fence that no backtick run inside the text can close, so the text stays a quote: a
    mention or `Closes #N` in it does nothing."""
    text = text.replace("\r\n", "\n").strip()
    note = ""
    if len(text) > limit:
        note = f"\n(cut at {limit:,} of {len(text):,} characters)"
        text = text[:limit]
    runs = [len(r) for r in re.findall(r"`+", text)]
    fence = "`" * max(3, max(runs, default=0) + 1)
    return f"{fence}text\n{text}\n{fence}{note}\n"


def login(node: dict) -> str:
    return (node.get("author") or {}).get("login", "ghost")


def areas(paths: list[str]) -> list[str]:
    return sorted({p.split("/")[0] if "/" in p else "root" for p in paths})


def record(pull: dict) -> tuple[str, bool]:
    """The Markdown of one pull request, and whether it has a `Learned:` section."""
    files = pull["files"]
    lines = [
        f"# Pull request {pull['number']}", "",
        "Each fenced block quotes what its author wrote.", "",
        f"Merged {pull['mergedAt']}. +{pull['additions']} -{pull['deletions']} "
        f"in {files['totalCount']} files.", "",
    ]
    learned = ""
    if trusted(pull):
        body, learned = split_learned((pull["body"] or "").replace("\r\n", "\n"))
        lines += [f"## Title, by {login(pull)}", "", quote(pull["title"], BODY_CHARS)]
        if body.strip():
            lines += ["## Body", "", quote(body, BODY_CHARS)]
        if learned:
            lines += ["## Learned", "", quote(learned, LEARNED_CHARS)]
    else:
        lines += ["Title and body are left out: the author has no write access.", ""]
    lines += [f"## Changed paths ({files['totalCount']})", ""]
    lines += [f"- +{f['additions']} -{f['deletions']} {f['path']}" for f in files["nodes"]]
    if files["totalCount"] > len(files["nodes"]):
        lines.append(f"- and {files['totalCount'] - len(files['nodes'])} more")
    lines.append("")
    comments = [c for c in pull["comments"]["nodes"] if trusted(c)]
    if comments:
        lines += ["## Comments", ""]
        for c in comments:
            lines += [f"### Comment by {login(c)}", "", quote(c["body"], COMMENT_CHARS)]
    reviews = [r for r in pull["reviews"]["nodes"] if trusted(r)]
    if reviews:
        lines += ["## Reviews", ""]
        for r in reviews:
            lines += [f"### Review by {login(r)}: {r['state']}", ""]
            if r["body"].strip():
                lines.append(quote(r["body"], COMMENT_CHARS))
            for c in (c for c in r["comments"]["nodes"] if trusted(c)):
                lines += [f"#### Comment by {login(c)} on {c['path']}", "", quote(c["body"], COMMENT_CHARS)]
    return "\n".join(lines).rstrip() + "\n", bool(learned)


def index_line(pull: dict, learned: bool) -> str:
    stats = f"+{pull['additions']} -{pull['deletions']}"
    files = pull["files"]
    dirs = ", ".join(areas([f["path"] for f in files["nodes"]]))
    # GitHub lists paths in name order, so a cut list holds the first directories only.
    if files["totalCount"] > len(files["nodes"]):
        dirs += f" (from the first {len(files['nodes'])} of {files['totalCount']} paths)"
    title = pull["title"] if trusted(pull) else "(title left out: the author has no write access)"
    return f"- {pull['number']} {stats} {dirs}{'; Learned' if learned else ''}: {title}"


def collect(repo: tuple[str, str], out: Path, since: str | None, now: datetime) -> tuple[int, str, str]:
    """Write the index and one record per pull request; nothing is written before all is read."""
    through = now.astimezone(UTC).strftime(STAMP)
    start = window_start(repo, since, through)
    pages = {}
    index = [
        "# Merged pull requests", "",
        f"Dream window: {start} to {through}. Newest merge first. "
        "Each line: number, lines added and removed, top-level directories, title.", "",
    ]
    for number in merged_numbers(repo, start, through):
        pull = graphql(RECORD_QUERY, owner=repo[0], name=repo[1], number=number)["repository"]["pullRequest"]
        pages[number], learned = record(pull)
        index.append(index_line(pull, learned))
    out.mkdir(parents=True, exist_ok=True)
    (out / "index.md").write_text("\n".join(index) + "\n", encoding="utf-8")
    for number, text in pages.items():
        (out / f"{number}.md").write_text(text, encoding="utf-8")
    return len(pages), start, through


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--since", help="the window start when no dream pull request has merged")
    args = parser.parse_args(argv)
    repo = os.environ.get("GITHUB_REPOSITORY", "").split("/")
    if len(repo) != 2 or not all(repo):
        raise SystemExit("collect: set GITHUB_REPOSITORY to owner/name")
    count, since, through = collect((repo[0], repo[1]), args.out, args.since or None, datetime.now(UTC))
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as f:
            f.write(f"count={count}\nsince={since}\nthrough={through}\n")
    print(f"collect: {count} pull request(s) merged from {since} to {through} in {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
