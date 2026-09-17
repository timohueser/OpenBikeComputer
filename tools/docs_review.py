"""List documentation to review after source changes; this does not certify prose."""
import argparse
from pathlib import Path
import re
import subprocess


ROOT = Path(__file__).resolve().parents[1]
LINK = re.compile(r"\]\((?:<([^>]+)>|([^\s)]+))(?:\s+[^)]*)?\)|\[src:([^\]]+)\]")


def git(root: Path, *args: str) -> list[str]:
    result = subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, text=True)
    return [path for path in result.stdout.split("\0") if path]


def review_queue(root: Path, changed: set[str], documents: list[str]) -> dict[str, set[str]]:
    queue: dict[str, set[str]] = {}
    for source in changed:
        directory = (root / source).parent
        while directory.is_relative_to(root):
            readme = directory / "README.md"
            if readme.is_file():
                queue.setdefault(readme.relative_to(root).as_posix(), set()).add(source)
                break
            directory = directory.parent
    for name in documents:
        document = root / name
        if not document.is_file():
            continue
        for match in LINK.finditer(document.read_text()):
            target = next(part for part in match.groups() if part is not None).split("#", 1)[0]
            if match.group(3) is not None:
                target = "src:" + target
            if not target or (":" in target and not target.startswith("src:")):
                continue
            resolved = (root / target[4:] if target.startswith("src:") else document.parent / target).resolve()
            if not resolved.is_relative_to(root):
                continue
            relative = resolved.relative_to(root).as_posix().rstrip("/")
            for source in changed:
                if source == relative or source.startswith(relative + "/"):
                    queue.setdefault(name, set()).add(source)
    return queue


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--base", help="compare against the merge base of this ref; default origin/develop")
    selection.add_argument("--since", help="review committed changes since a Git date, for example 1.week")
    args = parser.parse_args(argv)
    try:
        if args.since:
            changed = set(git(ROOT, "log", "--since=" + args.since, "--format=", "--name-only", "-z"))
            changed = {path.lstrip("\n") for path in changed if path.strip()}
        else:
            base = git(ROOT, "merge-base", args.base or "origin/develop", "HEAD")[0].strip()
            changed = set(git(ROOT, "diff", "--name-only", "-z", base))
        changed.update(git(ROOT, "ls-files", "--others", "--exclude-standard", "-z"))
        documents = git(ROOT, "ls-files", "-z", "*.md")
        queue = review_queue(ROOT, changed, documents)
    except (subprocess.CalledProcessError, OSError) as error:
        print(f"Documentation review failed: {getattr(error, 'stderr', '') or error}")
        return 1
    print(f"Documentation review candidates for {len(changed)} changed paths:")
    for document, sources in sorted(queue.items()):
        print(document)
        for source in sorted(sources)[:3]:
            print(f"  <- {source}")
        if len(sources) > 3:
            print(f"  and {len(sources) - 3} more changed paths")
    print("Check claims against source; follow docs/README.md for copy ownership and plan status.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
