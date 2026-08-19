#!/usr/bin/env python3
"""Fast, path-based source overview backed by tokei's code-line counter."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import PurePosixPath


AREAS = {
    "firmware": "Device firmware",
    "ios": "iOS companion",
    "web": "Web & desktop",
    "pipeline": "Map & weather pipeline",
    "tools": "Host & developer tooling",
}

# Manifests, generated data, fixtures, documentation, and hardware design files are
# useful repository content, but not source LOC. Tokei decides how these are parsed.
SOURCE_SUFFIXES = {
    ".bash",
    ".c",
    ".cc",
    ".cpp",
    ".css",
    ".h",
    ".html",
    ".js",
    ".mjs",
    ".py",
    ".rs",
    ".s",
    ".sh",
    ".svelte",
    ".swift",
    ".ts",
    ".tsx",
}
SOURCE_NAMES = {"justfile", "obc"}
PIPELINE_CRATES = {
    "obc-bake",
    "obc-dem",
    "obc-mkimage",
    "obc-pack",
    "obc-wx-bake",
    "obc-wx-client",
    "obcm-assemble",
}
TEST_SUPPORT_COMPONENTS = {
    "apps/obc-sim",
    "host/obc-bench",
    "host/obc-fixtures",
    "host/obcm-testkit",
}
TEST_NAME = re.compile(r"(?:^|[._-])(bench(?:mark)?s?|specs?|tests?)(?:[._-]|$)")


def is_source(path: PurePosixPath) -> bool:
    if path.name == "Package.swift":
        return False
    return path.suffix.lower() in SOURCE_SUFFIXES or path.name.lower() in SOURCE_NAMES


def is_test_support(path: PurePosixPath) -> bool:
    text = path.as_posix()
    if any(text == prefix or text.startswith(prefix + "/") for prefix in TEST_SUPPORT_COMPONENTS):
        return True
    if any(
        part.lower() in {"test", "tests", "benches", "benchmarks", "fixtures", "scripts"}
        for part in path.parts
    ):
        return True
    if any(part.lower().endswith(("tests", "uitests")) for part in path.parts):
        return True
    return bool(TEST_NAME.search(path.name.lower()))


def ios_component(parts: tuple[str, ...]) -> str:
    if (
        len(parts) >= 5
        and parts[1:3] == ("Packages", "OBCKit")
        and parts[3] in {"Sources", "Tests"}
    ):
        return parts[4].removesuffix("Tests")
    if len(parts) > 1 and parts[1] == "OBCCompanion":
        return "iOS app"
    if len(parts) > 1 and parts[1] == "OBCCompanionUITests":
        return "iOS app"
    return "Companion support"


def web_component(parts: tuple[str, ...]) -> str:
    if parts[:3] == ("builder", "app", "src"):
        if len(parts) >= 5 and parts[3] == "lib":
            return f"frontend/{parts[4]}" if len(parts) > 5 else "frontend/core"
        if len(parts) >= 4 and parts[3] in {"components", "routes"}:
            return f"frontend/{parts[3]}"
        return "frontend/shell"
    return parts[1]


def classify(path_text: str) -> tuple[str, str, str] | None:
    """Return (area key, stable component, implementation|support)."""
    path = PurePosixPath(path_text)
    if not is_source(path):
        return None
    parts = path.parts
    text = path.as_posix()
    forced_support = False

    if len(parts) >= 3 and parts[0] == "firmware" and parts[1].startswith("obc-"):
        if "vendor" in parts:
            return None
        if parts[1] == "obc-render" and "fonts" in parts:
            return "tools", "firmware-tools", "support"
        if not ({"src", "tests", "benches"} & set(parts[2:])):
            if path.name != "build.rs":
                return None
            forced_support = True
        area, component = "firmware", parts[1]
    elif parts and parts[0] == "companion-ios":
        area, component = "ios", ios_component(parts)
    elif text.startswith("builder/app/") or text == "builder/build-wasm-bridges.sh" or (
        len(parts) >= 2
        and parts[0] == "apps"
        and parts[1] in {
            "obc-desktop",
            "obc-skin-preview",
            "obc-web-assemble",
            "obc-web-convert",
            "obc-web-demo",
        }
    ):
        area = "web"
        if parts[0] == "apps":
            component = parts[1]
            forced_support = "src" not in parts and "tests" not in parts
        elif text == "builder/build-wasm-bridges.sh":
            component, forced_support = "frontend/build", True
        elif text.startswith("builder/app/src/"):
            component = web_component(parts)
        elif text == "builder/app/index.html":
            component = "frontend/shell"
        else:
            component = (
                "frontend/dev-harness"
                if text.startswith("builder/app/dev-harness/")
                else "frontend/build"
            )
            forced_support = True
    elif (
        (len(parts) >= 2 and parts[0] == "host" and parts[1] in PIPELINE_CRATES)
        or text.startswith("builder/server/")
        or text.startswith("builder/tests/")
        or text == "builder/__init__.py"
        or text == "fixtures/build-map-package.sh"
    ):
        area = "pipeline"
        if parts[0] == "builder":
            component = "builder-server"
        elif parts[0] == "fixtures":
            component = "fixtures"
        else:
            component = parts[1]
    elif parts and parts[0] in {"tools", "ops", "copydesk", "docs"}:
        area, component = "tools", parts[0]
        if parts[0] == "tools" and len(parts) > 1 and parts[1] == "rain-radar-demo":
            component = "rain-radar-demo"
    elif text.startswith("firmware/tools/"):
        area, component = "tools", "firmware-tools"
    elif text == "firmware/ui-snapshots.sh":
        area, component = "tools", "firmware-tools"
    elif len(parts) >= 2 and parts[0] in {"host", "apps"}:
        area, component = "tools", parts[1]
    else:
        return None

    kind = (
        "support"
        if forced_support or area == "tools" or is_test_support(path)
        else "implementation"
    )
    return area, component, kind


def tracked_files() -> list[str]:
    result = subprocess.run(
        ["git", "ls-files", "-z"], check=True, stdout=subprocess.PIPE, text=False
    )
    return [path.decode() for path in result.stdout.split(b"\0") if path]


def tokei_reports(paths: list[str]) -> dict:
    try:
        result = subprocess.run(
            ["tokei", "--files", "--output", "json", *paths],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
    except FileNotFoundError:
        raise SystemExit("tokei is required; install it with: cargo install tokei --locked")
    except subprocess.CalledProcessError as error:
        raise SystemExit(error.stderr.strip() or "tokei failed") from error
    return json.loads(result.stdout)


def code_lines(stats: dict) -> int:
    return stats.get("code", 0) + sum(code_lines(blob) for blob in stats.get("blobs", {}).values())


def collect() -> tuple[dict, int]:
    classified = {}
    for path in tracked_files():
        category = classify(path)
        if category:
            classified[path] = category

    totals = defaultdict(lambda: defaultdict(lambda: defaultdict(int)))
    counted = set()
    for language, payload in tokei_reports(list(classified)).items():
        if language == "Total":
            continue
        for report in payload.get("reports", []):
            path = report["name"]
            category = classified.get(path)
            if not category or path in counted:
                continue
            area, component, kind = category
            totals[area][component][kind] += code_lines(report["stats"])
            counted.add(path)
    return totals, len(counted)


def color(text: str, code: str, enabled: bool) -> str:
    return f"\033[{code}m{text}\033[0m" if enabled else text


def area_rows(totals: dict) -> list[tuple[str, int, int]]:
    rows = []
    for key, label in AREAS.items():
        components = totals.get(key, {})
        implementation = sum(item["implementation"] for item in components.values())
        support = sum(item["support"] for item in components.values())
        rows.append((label, implementation, support))
    return rows


def print_table(
    title: str, rows: list[tuple[str, int, int]], color_enabled: bool, bars: bool
) -> None:
    rows = [row for row in rows if row[1] or row[2]]
    name_width = max(18, *(len(row[0]) for row in rows))
    print(color(title, "1;36", color_enabled))
    suffix = "  Mix" if bars else ""
    print(
        f"{'Module':<{name_width}}  {'Implementation':>14}  "
        f"{'Test/support':>12}  {'Total':>10}{suffix}"
    )
    print(color("─" * (name_width + 44 + (20 if bars else 0)), "2", color_enabled))
    largest = max((impl + support for _, impl, support in rows), default=1)
    for name, implementation, support in rows:
        total = implementation + support
        impl_cell = color(f"{implementation:>14,}", "32", color_enabled)
        support_cell = color(f"{support:>12,}", "33", color_enabled)
        line = f"{name:<{name_width}}  {impl_cell}  {support_cell}  {total:>10,}"
        if bars:
            width = max(1, round(16 * total / largest))
            impl_width = round(width * implementation / total) if total else 0
            bar = color("█" * impl_width, "32", color_enabled) + color(
                "▒" * (width - impl_width), "33", color_enabled
            )
            line += f"  {bar}"
        print(line)
    implementation = sum(row[1] for row in rows)
    support = sum(row[2] for row in rows)
    total = implementation + support
    print(color("─" * (name_width + 44 + (20 if bars else 0)), "2", color_enabled))
    print(
        f"{'Repository total' if bars else 'Subtotal':<{name_width}}  "
        f"{color(f'{implementation:>14,}', '1;32', color_enabled)}  "
        f"{color(f'{support:>12,}', '1;33', color_enabled)}  {total:>10,}"
    )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Show tracked source lines by product area (default: overview only)."
    )
    parser.add_argument(
        "area",
        nargs="?",
        choices=[*AREAS, "all"],
        help="show component detail for one area, or all areas",
    )
    args = parser.parse_args()

    totals, file_count = collect()
    color_enabled = sys.stdout.isatty() and "NO_COLOR" not in os.environ
    print()
    print_table("OpenBikeComputer source lines", area_rows(totals), color_enabled, bars=True)

    selected = list(AREAS) if args.area == "all" else ([args.area] if args.area else [])
    for key in selected:
        components = totals.get(key, {})
        rows = sorted(
            (
                (name, values["implementation"], values["support"])
                for name, values in components.items()
            ),
            key=lambda row: (-(row[1] + row[2]), row[0]),
        )
        print("\n")
        print_table(f"{AREAS[key]} · components", rows, color_enabled, bars=False)

    print()
    if not args.area:
        print(
            color(
                "Detail: obc loc firmware|ios|web|pipeline|tools|all",
                "2",
                color_enabled,
            )
        )
    print(
        color(
            f"{file_count:,} tracked source files · code lines from tokei · "
            "vendor/generated/config/data/docs prose excluded",
            "2",
            color_enabled,
        )
    )
    print(
        color(
            "Path-based split: inline test modules remain implementation; simulators, "
            "benchmarks, harnesses, and developer tools are test/support.",
            "2",
            color_enabled,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
