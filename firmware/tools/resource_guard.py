#!/usr/bin/env python3
"""Report and gate OpenBikeComputer linked resources from release ELFs.

This is the single parser used for board RAM/framebuffer/poll-frame checks, the bootloader flash
budget, and the report-only compile-time allocation table. It intentionally consumes LLVM's text
tools rather than an ELF Python package so a fresh checkout needs only the selected Rust toolchain.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path


HERE = Path(__file__).resolve().parent
DEFAULT_BASELINE = HERE / "resource_baseline.json"
FLASH_SECTIONS = (".vector_table", ".text", ".rodata", ".data")
RESOURCE_SECTION = ".obc_resources"
RESOURCE_NAME_BYTES = 32
RESOURCE_ENTRY_BYTES = RESOURCE_NAME_BYTES + 4
WRITABLE_SYMBOL_TYPES = frozenset("bBdDsS")
STRICT_ALIGN_PROBE = HERE / "strict_align_probe.rs"


class GuardError(RuntimeError):
    """A stale parser, missing artifact fact, or exceeded budget."""


@dataclass(frozen=True)
class Symbol:
    size: int
    kind: str
    name: str


@dataclass(frozen=True)
class BoardMeasurement:
    bss: int
    data: int
    uninit: int
    flash: int
    framebuffer_symbols: tuple[Symbol, ...]
    full_frame_sized_writable: tuple[Symbol, ...]
    largest_poll_frame: int | None

    @property
    def resident(self) -> int:
        """The review/CI contract's linked resident figure: `.bss + .data`."""
        return self.bss + self.data


def parse_size_output(output: str) -> dict[str, int]:
    sections: dict[str, int] = {}
    for line in output.splitlines():
        match = re.match(r"^(\.[^\s]+)\s+(\d+)\s+", line.strip())
        if match:
            sections[match.group(1)] = int(match.group(2))
    required = {".bss", ".data", *FLASH_SECTIONS}
    missing = sorted(required - sections.keys())
    if missing:
        raise GuardError(f"llvm-size output is stale/incomplete; missing section(s): {', '.join(missing)}")
    return sections


def parse_nm_output(output: str) -> list[Symbol]:
    symbols: list[Symbol] = []
    for line in output.splitlines():
        # llvm-nm --print-size prints both address and size in hexadecimal.
        match = re.match(r"^[0-9a-fA-F]+\s+([0-9a-fA-F]+)\s+([A-Za-z?])\s+(.+)$", line.strip())
        if match:
            symbols.append(Symbol(int(match.group(1), 16), match.group(2), match.group(3)))
    if not symbols:
        raise GuardError("llvm-nm output is stale/incomplete; no sized symbols were parsed")
    return symbols


def is_framebuffer_symbol(name: str) -> bool:
    return re.search(r"(?:^|::)FB(?:::h[0-9a-f]+)?$", name) is not None


def parse_poll_frames(disassembly: str) -> dict[str, int]:
    frames: dict[str, int] = {}
    function = ""
    saw_poll_symbol = False
    for line in disassembly.splitlines():
        header = re.match(r"^[0-9a-fA-F]+ <(.+)>:$", line.strip())
        if header:
            function = header.group(1)
            if "TaskStorage" in function and "poll" in function:
                saw_poll_symbol = True
            continue
        decrement = re.search(
            r"\bsub(?:\.w)?\s+sp,\s*(?:sp,\s*)?#(0x[0-9a-fA-F]+|\d+)", line
        )
        if decrement and "TaskStorage" in function and "poll" in function:
            value = int(decrement.group(1), 0)
            frames[function] = max(value, frames.get(function, 0))
    if not saw_poll_symbol:
        raise GuardError(
            "poll-frame guard is stale: no `TaskStorage<F>::poll` symbols found in disassembly"
        )
    if not frames:
        raise GuardError(
            "poll-frame guard is stale: poll symbols exist but no `sub sp, #imm` prologue was parsed"
        )
    return frames


def decode_resource_table(raw: bytes) -> dict[str, int]:
    if not raw:
        raise GuardError(f"{RESOURCE_SECTION} is empty")
    if len(raw) % RESOURCE_ENTRY_BYTES:
        raise GuardError(
            f"{RESOURCE_SECTION} has {len(raw)} bytes, not a multiple of {RESOURCE_ENTRY_BYTES}; "
            "the report parser and Rust Entry layout disagree"
        )
    resources: dict[str, int] = {}
    for offset in range(0, len(raw), RESOURCE_ENTRY_BYTES):
        entry = raw[offset : offset + RESOURCE_ENTRY_BYTES]
        name_bytes = entry[:RESOURCE_NAME_BYTES].split(b"\0", 1)[0]
        try:
            name = name_bytes.decode("ascii")
        except UnicodeDecodeError as error:
            raise GuardError(f"non-ASCII resource name at table offset {offset}") from error
        if not name:
            raise GuardError(f"empty resource name at table offset {offset}")
        if name in resources:
            raise GuardError(f"duplicate resource table entry `{name}`")
        resources[name] = int.from_bytes(entry[RESOURCE_NAME_BYTES:], "little")
    if resources.get("format_version") != 1:
        raise GuardError(
            f"unsupported resource table format {resources.get('format_version')!r}; expected 1"
        )
    return resources


def find_llvm_tool(name: str) -> Path:
    try:
        sysroot = Path(
            subprocess.run(
                ["rustc", "--print", "sysroot"], check=True, text=True, capture_output=True
            ).stdout.strip()
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise GuardError(f"cannot locate {name}: `rustc --print sysroot` failed") from error
    matches = sorted(sysroot.glob(f"lib/rustlib/*/bin/{name}"))
    if not matches:
        raise GuardError(
            f"cannot locate {name} under {sysroot}; run `rustup component add llvm-tools`"
        )
    return matches[0]


def run_tool(tool: str, *args: str | Path) -> str:
    executable = find_llvm_tool(tool)
    try:
        return subprocess.run(
            [str(executable), *(str(arg) for arg in args)],
            check=True,
            text=True,
            capture_output=True,
        ).stdout
    except subprocess.CalledProcessError as error:
        detail = error.stderr.strip() or error.stdout.strip() or f"exit {error.returncode}"
        raise GuardError(f"{tool} failed: {detail}") from error


def extract_resource_table(elf: Path) -> dict[str, int]:
    objcopy = find_llvm_tool("llvm-objcopy")
    with tempfile.TemporaryDirectory(prefix="obc-resource-") as directory:
        output = Path(directory) / "resources.bin"
        try:
            subprocess.run(
                [str(objcopy), f"--dump-section={RESOURCE_SECTION}={output}", str(elf)],
                check=True,
                text=True,
                capture_output=True,
            )
        except subprocess.CalledProcessError as error:
            detail = error.stderr.strip() or error.stdout.strip() or f"exit {error.returncode}"
            raise GuardError(
                f"cannot extract {RESOURCE_SECTION}; build this report ELF with "
                f"`--features resource-report`: {detail}"
            ) from error
        if not output.exists():
            raise GuardError(
                f"{RESOURCE_SECTION} is missing; build this report ELF with `--features resource-report`"
            )
        return decode_resource_table(output.read_bytes())


def measure_board(elf: Path, framebuffer_bytes: int, include_poll: bool) -> BoardMeasurement:
    sections = parse_size_output(run_tool("llvm-size", "-A", elf))
    symbols = parse_nm_output(
        run_tool("llvm-nm", "--print-size", "--size-sort", "--demangle", elf)
    )
    framebuffer_symbols = tuple(symbol for symbol in symbols if is_framebuffer_symbol(symbol.name))
    full_frame_sized = tuple(
        symbol
        for symbol in symbols
        if symbol.kind in WRITABLE_SYMBOL_TYPES and symbol.size >= framebuffer_bytes
    )
    poll = None
    if include_poll:
        frames = parse_poll_frames(run_tool("llvm-objdump", "--demangle", "-d", elf))
        poll = max(frames.values())
    return BoardMeasurement(
        bss=sections[".bss"],
        data=sections[".data"],
        uninit=sections.get(".uninit", 0),
        flash=sum(sections[name] for name in FLASH_SECTIONS),
        framebuffer_symbols=framebuffer_symbols,
        full_frame_sized_writable=full_frame_sized,
        largest_poll_frame=poll,
    )


def load_baseline(path: Path) -> dict[str, object]:
    try:
        baseline = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise GuardError(f"cannot read baseline {path}: {error}") from error
    if baseline.get("schema_version") != 1:
        raise GuardError(f"unsupported baseline schema in {path}; expected schema_version 1")
    return baseline


def require(condition: bool, message: str) -> None:
    if not condition:
        raise GuardError(message)


def check_board(args: argparse.Namespace, baseline: dict[str, object]) -> None:
    profile = baseline["board"][args.profile]
    framebuffer_bytes = profile["framebuffer_bytes"]
    include_poll = "poll_frame_limit" in profile
    measured = measure_board(args.elf, framebuffer_bytes, include_poll)
    print(
        f"{args.profile}: .bss {measured.bss:,} B + .data {measured.data:,} B "
        f"= {measured.resident:,} B linked resident; .uninit {measured.uninit:,} B; "
        f"flash {measured.flash:,} B"
    )
    require(
        measured.resident <= profile["resident_ram_max"],
        f"{args.profile} resident RAM grew to {measured.resident} B (.bss + .data), above the "
        f"approved {profile['resident_ram_max']} B baseline; itemize/approve the increase",
    )
    require(
        measured.uninit <= profile["uninit_max"],
        f"{args.profile} .uninit grew to {measured.uninit} B, above {profile['uninit_max']} B",
    )
    expected_count = profile["framebuffer_count"]
    require(
        len(measured.framebuffer_symbols) == expected_count,
        f"{args.profile} framebuffer symbol count is {len(measured.framebuffer_symbols)}, expected "
        f"{expected_count}; symbols: {[s.name for s in measured.framebuffer_symbols]}",
    )
    for symbol in measured.framebuffer_symbols:
        require(
            symbol.size == framebuffer_bytes,
            f"{args.profile} framebuffer `{symbol.name}` is {symbol.size} B, expected "
            f"{framebuffer_bytes} B (240 x 320 x 1)",
        )
    require(
        len(measured.full_frame_sized_writable) == expected_count,
        f"{args.profile} has {len(measured.full_frame_sized_writable)} writable allocation(s) at "
        f"least one full frame ({framebuffer_bytes} B), expected {expected_count}; "
        f"candidates: {[(s.name, s.size) for s in measured.full_frame_sized_writable]}",
    )
    if include_poll:
        assert measured.largest_poll_frame is not None
        print(f"{args.profile}: largest guarded poll frame {measured.largest_poll_frame:,} B")
        require(
            measured.largest_poll_frame <= profile["poll_frame_limit"],
            f"{args.profile} largest guarded poll frame is {measured.largest_poll_frame} B, above "
            f"the {profile['poll_frame_limit']} B safety limit; move large construction "
            "temporaries behind an #[inline(never)] .bss initializer (see issue #677)",
        )
    print(f"{args.profile}: resource guards passed")


def check_report(args: argparse.Namespace, baseline: dict[str, object]) -> None:
    expected = baseline["board"][args.profile]["compile_time_allocations"]
    measured = extract_resource_table(args.elf)
    missing = sorted(set(expected) - measured.keys())
    extra = sorted(set(measured) - {"format_version", *expected.keys()})
    require(not missing, f"{args.profile} resource report is missing entries: {', '.join(missing)}")
    require(not extra, f"{args.profile} resource report has unbaselined entries: {', '.join(extra)}")
    drift = {
        name: (expected[name], measured[name])
        for name in expected
        if expected[name] != measured.get(name)
    }
    require(
        not drift,
        f"{args.profile} compile-time allocation drift: "
        + ", ".join(f"{name} {old}->{new} B" for name, (old, new) in sorted(drift.items())),
    )
    print(f"{args.profile}: compile-time allocation table ({len(expected)} entries)")
    for name in expected:
        print(f"  {name:24} {measured[name]:>8,} B")
    print(f"{args.profile}: allocation report matches baseline")


def check_boot(args: argparse.Namespace, baseline: dict[str, object]) -> None:
    sections = parse_size_output(run_tool("llvm-size", "-A", args.elf))
    flash = sum(sections[name] for name in FLASH_SECTIONS)
    budget = baseline["bootloader"]["flash_budget"]
    print(f"obc-boot flash footprint: {flash:,} / {budget:,} B")
    require(
        flash <= budget,
        f"obc-boot is {flash} B, over the {budget} B bootloader slot; see obc-boot/memory.x",
    )
    print("obc-boot: flash guard passed")


def function_assembly(assembly: str, function: str) -> str:
    match = re.search(
        rf"(?ms)^{re.escape(function)}:\n(?P<body>.*?)(?=^\.Lfunc_end\d+:)", assembly
    )
    if match is None:
        raise GuardError(f"strict-align probe parser is stale: `{function}` assembly not found")
    return match.group("body")


def compile_probe(probe: Path, output: Path, strict: bool) -> tuple[str, str]:
    command = [
        "rustc",
        "--crate-type",
        "lib",
        "--edition",
        "2021",
        "--target",
        "thumbv8m.main-none-eabihf",
        "-O",
        "--emit",
        f"asm={output}",
    ]
    if strict:
        command.extend(("-C", "target-feature=+strict-align"))
    command.append(str(probe))
    try:
        result = subprocess.run(command, check=True, text=True, capture_output=True)
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "").strip() or str(error)
        raise GuardError(f"strict-align compiler probe failed: {detail}") from error
    return output.read_text(), result.stderr


def check_strict_align(args: argparse.Namespace) -> None:
    with tempfile.TemporaryDirectory(prefix="obc-strict-align-") as directory:
        directory = Path(directory)
        strict_asm, warning = compile_probe(args.probe, directory / "strict.s", True)
        normal_asm, _ = compile_probe(args.probe, directory / "normal.s", False)
    strict_body = function_assembly(strict_asm, "decode_u32")
    normal_body = function_assembly(normal_asm, "decode_u32")
    strict_byte_loads = len(re.findall(r"\bldrb(?:\.w)?\b", strict_body))
    strict_word_loads = len(re.findall(r"\bldr(?:\.w)?\b", strict_body))
    normal_word_loads = len(re.findall(r"\bldr(?:\.w)?\b", normal_body))
    require(
        strict_byte_loads == 4 and strict_word_loads == 0,
        "+strict-align is not honored by the active backend: decode_u32 must lower to four byte "
        f"loads and no word load (saw ldrb={strict_byte_loads}, ldr={strict_word_loads})",
    )
    require(
        normal_word_loads >= 1,
        "strict-align control probe is no longer discriminating: the build without the flag did "
        "not combine the four bytes into a word load",
    )
    warning_line = next((line.strip() for line in warning.splitlines() if "strict-align" in line), "none")
    print(f"rustc +strict-align diagnostic: {warning_line}")
    print("strict-align backend check passed: 4 x ldrb with flag; ldr control without flag")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    root.add_argument("--baseline", type=Path, default=DEFAULT_BASELINE)
    commands = root.add_subparsers(dest="command", required=True)
    board = commands.add_parser("board", help="gate linked board RAM, framebuffer, and poll frame")
    board.add_argument("--profile", choices=("default", "ble"), required=True)
    board.add_argument("--elf", type=Path, required=True)
    report = commands.add_parser("report", help="gate report-only target-side size_of table")
    report.add_argument("--profile", choices=("default", "ble"), required=True)
    report.add_argument("--elf", type=Path, required=True)
    boot = commands.add_parser("boot", help="gate the bootloader flash slot")
    boot.add_argument("--elf", type=Path, required=True)
    strict = commands.add_parser("strict-align", help="prove the active ARM backend honors +strict-align")
    strict.add_argument("--probe", type=Path, default=STRICT_ALIGN_PROBE)
    return root


def main() -> int:
    args = parser().parse_args()
    try:
        baseline = load_baseline(args.baseline)
        if args.command == "board":
            check_board(args, baseline)
        elif args.command == "report":
            check_report(args, baseline)
        elif args.command == "boot":
            check_boot(args, baseline)
        else:
            check_strict_align(args)
    except (GuardError, KeyError, TypeError) as error:
        print(f"resource guard failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
