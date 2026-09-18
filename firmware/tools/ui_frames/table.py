"""Read `firmware/ui-frames.toml` into frames.

The table holds three constructs, and this module is the only thing that knows them:

* `{name}` in a script expands a fragment from `[script]`, and a fragment can use another one.
* `t*n` in a script repeats one token n times, so `f*64` is a line of 64 tokens.
* `langs` expands one row into one frame for each language, named `<name>-<lang>`.

`{name}` in `map` and in `args` expands a path from `[paths]`, whose own `{fixtures}` and `{repo}`
come from the caller. A row that names something the table does not hold is an error here rather
than a render that fails later for an unrelated-looking reason.
"""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass, replace
from pathlib import Path

FRAGMENT = re.compile(r"\{([a-z0-9_]+)\}")
REPEAT = re.compile(r"^(\S+)\*(\d+)$")
KEYS = {"name", "map", "env", "script", "expect", "args", "boot", "langs"}
DEPTH = 8


class TableError(Exception):
    """A table that cannot be read, or a row that names something the table does not hold."""


@dataclass(frozen=True)
class Frame:
    """One rendered PNG: what the simulator opens, how it is driven, and where it must land."""

    name: str
    expect: str
    script: str = ""
    map: str | None = None
    envs: tuple[str, ...] = ()
    args: tuple[str, ...] = ()
    boot: bool = True
    lang: str | None = None


def substitute(text: str, values: dict[str, str], what: str) -> str:
    def one(match: re.Match[str]) -> str:
        try:
            return values[match.group(1)]
        except KeyError:
            raise TableError(f"unknown {what} `{match.group(1)}` in {text!r}") from None

    return FRAGMENT.sub(one, text)


def expand_script(text: str, fragments: dict[str, str]) -> str:
    for _ in range(DEPTH):
        if not FRAGMENT.search(text):
            break
        text = substitute(text, fragments, "script fragment")
    else:
        raise TableError(f"script fragments nest more than {DEPTH} deep in {text!r}")
    tokens: list[str] = []
    for token in text.split():
        repeat = REPEAT.match(token)
        if repeat:
            tokens.extend([repeat.group(1)] * int(repeat.group(2)))
        else:
            tokens.append(token)
    return " ".join(tokens)


def fragments(path: Path) -> dict[str, str]:
    """The `[script]` fragments alone, for a staging recipe that walks the same menus."""
    return tomllib.loads(Path(path).read_text()).get("script", {})


def load(path: Path, *, fixtures: str = "", repo: str = "") -> list[Frame]:
    """Every frame the table declares, in table order, with `langs` already expanded."""
    try:
        document = tomllib.loads(Path(path).read_text())
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise TableError(f"cannot read {path}: {exc}") from exc

    roots = {"fixtures": str(fixtures), "repo": str(repo)}
    paths = {name: substitute(value, roots, "root") for name, value in document.get("paths", {}).items()}
    fragments = document.get("script", {})

    frames: list[Frame] = []
    names: set[str] = set()
    for row in document.get("frame", []):
        unknown = set(row) - KEYS
        if unknown:
            raise TableError(f"frame {row.get('name', '?')}: unknown key(s) {sorted(unknown)}")
        if not row.get("name") or not row.get("expect"):
            raise TableError(f"frame {row.get('name', '?')}: `name` and `expect` are required")
        env = row.get("env", [])
        base = Frame(
            name=row["name"],
            expect=row["expect"],
            script=expand_script(row.get("script", ""), fragments),
            map=substitute(row["map"], paths, "path") if "map" in row else None,
            envs=(env,) if isinstance(env, str) else tuple(env),
            args=tuple(substitute(str(arg), paths, "path") for arg in row.get("args", [])),
            boot=row.get("boot", True),
        )
        langs = row.get("langs")
        expanded = [replace(base, name=f"{base.name}-{lang}", lang=lang) for lang in langs] if langs else [base]
        for frame in expanded:
            if frame.name in names:
                raise TableError(f"two frames are named {frame.name}")
            names.add(frame.name)
            frames.append(frame)
    if not frames:
        raise TableError(f"{path} declares no frame")
    return frames
