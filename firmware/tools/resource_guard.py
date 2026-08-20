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
import tomllib
from dataclasses import dataclass
from pathlib import Path


HERE = Path(__file__).resolve().parent
DEFAULT_BASELINE = HERE / "resource_baseline.json"
FLASH_SECTIONS = (".vector_table", ".text", ".rodata", ".data")
RESOURCE_SECTION = ".obc_resources"
RESOURCE_NAME_BYTES = 32
RESOURCE_ENTRY_BYTES = RESOURCE_NAME_BYTES + 4
WRITABLE_SYMBOL_TYPES = frozenset("bBdDsS")
# NOBITS (`.bss`, `.uninit`) as llvm-nm spells it — the arena is one of these, and a claim that it
# is not (`d`/`D` = it acquired an initializer, `r` = it stopped being writable) is a real finding.
NOBITS_SYMBOL_TYPES = frozenset("bB")
STRICT_ALIGN_PROBE = HERE / "strict_align_probe.rs"
EMBEDDED_TARGET = "thumbv8m.main-none-eabihf"
EMBEDDED_TARGET_CFG = 'cfg(all(target_arch = "arm", target_os = "none"))'
STRICT_ALIGN_CONFIGS = (
    HERE.parent / "obc-fw-nrf54l" / ".cargo" / "config.toml",
    HERE.parent / "obc-boot" / ".cargo" / "config.toml",
)
ANSI_CSI_RE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")


class GuardError(RuntimeError):
    """A stale parser, missing artifact fact, or exceeded budget."""


@dataclass(frozen=True)
class Symbol:
    size: int
    kind: str
    name: str


@dataclass(frozen=True)
class BootChain:
    """The measured boot-path stack picture (see [`chain_cost`] for what "ceiling" means here)."""

    residual_stack: int
    task_frame: int
    task_frame_symbol: str
    chain_ceiling: int
    chain_root: str
    chain_path: tuple[str, ...]
    # A stale baselined root (renamed, or — the regression this whole block exists for — inlined
    # away into its caller). Held rather than raised at measure time so the two EXACT gates report
    # first: on a genuinely regressed image their messages are the actionable ones.
    chain_error: str | None = None


@dataclass(frozen=True)
class BoardMeasurement:
    bss: int
    data: int
    # `.uninit` — cortex-m-rt's NOLOAD section, placed after `.bss` and skipped by the reset
    # handler's zeroing loop. It used to be 1 KB of `defmt_rtt::BUFFER` and nothing else, which is
    # where the historical `uninit_max` came from. Since #1146 P2 it is also where the ~117 KB
    # **scratch arena** lives, so it is now the second-largest resident block in the image and is
    # gated in earnest — see the `uninit_max` check in `check_board`.
    uninit: int
    flash: int
    framebuffer_symbols: tuple[Symbol, ...]
    full_frame_sized_writable: tuple[Symbol, ...]
    largest_poll_frame: int | None
    # The linked scratch-arena statics (expected: exactly one), name-matched the way `FB` is. This
    # is the *symbol*, not the section, and that is the point — see the arena block in `check_board`.
    arena_symbols: tuple[Symbol, ...] = ()
    # `None` for a profile that baselines no `boot_chain_roots` — same optional-check shape as
    # `largest_poll_frame`.
    boot: BootChain | None = None

    @property
    def resident(self) -> int:
        """The review/CI contract's linked resident figure: `.bss + .data`.

        **Not** "all the RAM the image holds" — `.uninit` is resident too, and since #1146 P2 it
        carries the scratch arena. The two are gated separately (`resident_ram_max` + `uninit_max`)
        because they are separately re-approvable, but a change that only *moves* bytes between the
        two sections leaves this figure looking like a saving it is not. Read them together, and
        read `residual_stack`, which is charged for both.
        """
        return self.bss + self.data


def parse_size_output(output: str, extra_required: frozenset[str] = frozenset()) -> dict[str, int]:
    """Section sizes from `llvm-size -A`, failing loudly on a section the caller says must be there.

    `extra_required` is how the **board** legs demand `.uninit`, the section that has held the
    scratch arena (~117 KB since #1146 P3 spent the dividend on the render arm; ~92 KB as P2 left
    it) since #1146 P2. It is not in the common set because the bootloader legitimately links none.

    **What this catches, exactly:** llvm-size no longer printing the section (a stale parser) or the
    board linking no `.uninit` at all — either of which would otherwise measure it as zero and leave
    `uninit_max` green over an unaccounted-for section. It is a staleness tripwire, nothing more.

    **What it does not catch:** the *arena's* `#[link_section]` being renamed away from `.uninit`.
    The section has a second tenant (`defmt_rtt::BUFFER`, 1,024 B), so it survives the arena leaving
    it and this required-set check still passes — as do all four RAM gates, because `uninit_max` is
    a `<=` ceiling and `residual_stack_min` a `>=` floor that a *departing* arena only raises. The
    gate for that is the arena-symbol check in [`check_board`], which pins the linked static's size
    and requires the section to be big enough to contain it.
    """
    sections: dict[str, int] = {}
    for line in output.splitlines():
        match = re.match(r"^(\.[^\s]+)\s+(\d+)\s+", line.strip())
        if match:
            sections[match.group(1)] = int(match.group(2))
    required = {".bss", ".data", *FLASH_SECTIONS} | set(extra_required)
    missing = sorted(required - sections.keys())
    if missing:
        raise GuardError(f"llvm-size output is stale/incomplete; missing section(s): {', '.join(missing)}")
    return sections


def parse_stack_bounds(output: str) -> tuple[int, int]:
    """(`_stack_start`, `__euninit`) from `llvm-nm`: the residual main stack's two ends.

    The M33's stack starts at the top of the linker's `RAM` region and grows **down**; the resident
    statics grow **up** and end at `__euninit` — which, as the name says, is the end of `.uninit`,
    i.e. past `.bss` **and** the arena. Their difference is the whole stack the main task, every
    `#[inline(never)]` boot constructor and MPSL's ISRs share — which is why growth in *either*
    resident section is a stack cut, not just a RAM cost (the elevation epic's +3.7 KB of `.bss`
    moved this from 52.3 to 48.6 KB; #1146 P2 moving ~92 KB out of `.bss` into `.uninit` while
    deleting ~76 KB net gave back exactly that net, not the 168 KB `.bss` alone suggests).
    """
    addresses: dict[str, int] = {}
    for line in output.splitlines():
        match = re.match(r"^([0-9a-fA-F]+)\s+[A-Za-z?]\s+(\S+)\s*$", line.strip())
        if match and match.group(2) in ("_stack_start", "__euninit"):
            addresses[match.group(2)] = int(match.group(1), 16)
    missing = sorted({"_stack_start", "__euninit"} - addresses.keys())
    if missing:
        raise GuardError(
            f"stack-bounds parser is stale: llvm-nm did not report {', '.join(missing)}; "
            "the linker script's symbol names moved (see obc-fw-nrf54l/build.rs)"
        )
    if addresses["_stack_start"] <= addresses["__euninit"]:
        raise GuardError(
            f"linked image has no residual stack: _stack_start {addresses['_stack_start']:#x} is "
            f"not above __euninit {addresses['__euninit']:#x}; the statics overran the RAM region"
        )
    return addresses["_stack_start"], addresses["__euninit"]


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


def is_arena_symbol(name: str) -> bool:
    """The scratch arena's linked static (#1146 P2), module path included.

    Matched on `arena::ARENA` rather than a bare `ARENA` on purpose: "arena" is a common name for
    somebody else's pool (the BLE host's, embassy's task arena), and this gate pins an exact size —
    a foreign match would be a confusing failure, and a *second* match is reported as ambiguity
    rather than silently maximised over.
    """
    return re.search(r"(?:^|::)arena::ARENA(?:::h[0-9a-f]+)?$", name) is not None


SYMBOL_HEADER_RE = re.compile(r"^[0-9a-fA-F]+ <(.+)>:$")
# `sub sp, #imm` / `sub.w sp, sp, #imm` / `subw sp, sp, #imm` — every spelling LLVM emits for a
# frame allocation on thumbv8m. The `subw` arm is new and load-bearing: it is a distinct encoding
# (wide 12-bit immediate), the previous `sub(?:\.w)?` could not match it, and #1108's own
# `mount_terrain` prologue is `subw sp, sp, #0x8c4` — so the guard was blind to the very helper the
# boot-stack fix introduced. Broadening it does not move any baselined figure (the largest poll
# frame reads 9,728 B before and after).
FRAME_DECREMENT_RE = re.compile(r"\bsubw?(?:\.w)?\s+sp,\s*(?:sp,\s*)?#(0x[0-9a-fA-F]+|\d+)")
PUSH_RE = re.compile(r"\bpush(?:\.w)?\s+\{([^}]*)\}")
CALL_RE = re.compile(r"\bbl\s+0x[0-9a-fA-F]+ <([^>]+)>")
# The embassy **out-of-line task body**. `#[embassy_executor::task]` expands to
# `____embassy_<name>_task::____embassy_<name>_task_inner_function::{{closure}}` (demangled with
# `_$u7b$$u7b$closure$u7d$$u7d$`), and that closure — not `TaskStorage<F>::poll` — is where a task's
# real frame is allocated once codegen outlines it. `parse_poll_frames` never saw these, which is
# how #1084 grew the main task's frame by 2 KB unnoticed until it bricked boot.
TASK_BODY_RE = re.compile(r"____embassy_\w*?_?task.*inner_function")


@dataclass(frozen=True)
class Disassembly:
    """One pass over `llvm-objdump -d`, reused by every frame/chain check.

    `frames` is the largest single stack decrement per symbol; `pushes` the bytes the prologue
    pushes before it (callee-saved registers + `lr`), which a stack chain pays just as surely as
    the `sub sp`; `callees` the direct `bl` edges, for the boot-chain walk.
    """

    frames: dict[str, int]
    pushes: dict[str, int]
    callees: dict[str, frozenset[str]]
    # EVERY symbol header seen, including leaf functions with neither a frame nor a call. Kept
    # separately so `select_frames` can tell "the naming convention moved" (symbol absent) from
    # "the prologue spelling moved" (symbol present, frame unparsed) — a distinction that collapses
    # if membership is inferred from `frames`/`callees`.
    symbols: frozenset[str]

    def entry_cost(self, function: str) -> int:
        return self.frames.get(function, 0) + self.pushes.get(function, 0)


def parse_disassembly(disassembly: str) -> Disassembly:
    frames: dict[str, int] = {}
    pushes: dict[str, int] = {}
    callees: dict[str, set[str]] = {}
    symbols: set[str] = set()
    function = ""
    for raw_line in disassembly.splitlines():
        line = raw_line.strip()
        header = SYMBOL_HEADER_RE.match(line)
        if header:
            function = header.group(1)
            symbols.add(function)
            continue
        if not function:
            continue
        decrement = FRAME_DECREMENT_RE.search(line)
        if decrement:
            frames[function] = max(int(decrement.group(1), 0), frames.get(function, 0))
        # Prologue pushes only: once the frame is allocated, a later push is transient inside an
        # already-counted frame, not a permanent addition to the function's stack cost.
        if function not in frames:
            push = PUSH_RE.search(line)
            if push:
                count = len([reg for reg in push.group(1).split(",") if reg.strip()])
                pushes[function] = pushes.get(function, 0) + 4 * count
        call = CALL_RE.search(line)
        if call:
            callees.setdefault(function, set()).add(call.group(1))
    # No global "did we see any frame at all" check on purpose: every consumer goes through
    # `select_frames`, whose per-selector diagnostics say *which* convention went stale. A global
    # raise here would pre-empt those with a strictly less useful message.
    return Disassembly(
        frames=frames,
        pushes=pushes,
        callees={name: frozenset(edges) for name, edges in callees.items()},
        symbols=frozenset(symbols),
    )


def canonical_symbol(name: str) -> str:
    """One spelling of a path separator, so a scoped needle sees every symbol it names.

    llvm-objdump demangles a plain function to `obc_storage::flat::store::FlatStore<D>::mount`, but
    a **trait impl** to `<obc_storage..flat..store..FlatStore<D> as obc_storage..flat..seam..Store>
    ::commit` — legacy escaping renders the paths inside the `<... as ...>` brackets with `..`. A
    needle written the way a Rust path is written therefore matched the inherent methods and silently
    skipped every trait method, which is how `Store::commit`'s 2,812 B frame sat outside the #1386
    gate that was supposed to be watching it. Canonicalising here fixes it once for every scoped
    needle rather than asking each caller to spell both forms.

    It also decodes the **legacy escapes** the same renderer emits for punctuation: `$LT$` / `$GT$`
    for the angle brackets of a generic, and `$u20$` for the space in `<A as B>`. That half was added
    after the second bite of this class: FS7.5-c1 baselined a boot-chain root as
    `FlatStore$LT$D$GT$::mount_in_place` — the spelling *one host's* demangler produced — and on CI
    the symbol did not resolve. The guard went red as "stale", and the boot-chain walk fell back to a
    ceiling with the entire mount missing from it, which is the exact blindness the root was added to
    end. A needle should be spelled the way Rust spells it, `FlatStore<D>::mount_in_place`, and match
    whatever the demangler in front of it renders.

    Only the matching predicate sees this form; the reported names stay exactly as the tool emitted
    them, because a diagnostic that renames the symbol it is complaining about is a worse diagnostic.
    """
    return name.replace("..", "::").replace("$LT$", "<").replace("$GT$", ">").replace("$u20$", " ")


def select_frames(
    parsed: Disassembly, predicate, description: str, symbol_hint: str
) -> dict[str, int]:
    """The frames of every symbol `predicate` accepts, with stale-parser diagnostics.

    Two distinct failures are reported apart on purpose: symbols missing entirely means the naming
    convention moved (a compiler/embassy upgrade), while symbols present but frameless means the
    prologue spelling moved. Collapsed into one "no results" either would silently disable a guard,
    which is the failure mode that let #1084 through in the first place.
    """
    matched = [name for name in parsed.symbols if predicate(canonical_symbol(name))]
    if not matched:
        raise GuardError(
            f"{description} guard is stale: no {symbol_hint} symbols found in disassembly"
        )
    frames = {name: parsed.frames[name] for name in matched if name in parsed.frames}
    if not frames:
        raise GuardError(
            f"{description} guard is stale: {symbol_hint} symbols exist but no `sub sp, #imm` "
            "prologue was parsed"
        )
    return frames


def is_poll_symbol(name: str) -> bool:
    return "TaskStorage" in name and "poll" in name


def is_task_body_symbol(name: str) -> bool:
    return TASK_BODY_RE.search(name) is not None


def select_poll_frames(parsed: Disassembly) -> dict[str, int]:
    """`TaskStorage<F>::poll` frames — the #677 steady-state contract, unchanged."""
    return select_frames(parsed, is_poll_symbol, "poll-frame", "`TaskStorage<F>::poll`")


def select_task_body_frames(parsed: Disassembly) -> dict[str, int]:
    """The out-of-line `____embassy_*_task` bodies — see [`TASK_BODY_RE`]."""
    return select_frames(parsed, is_task_body_symbol, "task-body", "`____embassy_*_task` body")


def parse_poll_frames(disassembly: str) -> dict[str, int]:
    return select_poll_frames(parse_disassembly(disassembly))


def parse_task_body_frames(disassembly: str) -> dict[str, int]:
    return select_task_body_frames(parse_disassembly(disassembly))


def chain_cost(parsed: Disassembly, root: str) -> tuple[int, tuple[str, ...]]:
    """Deepest statically-reachable `bl` chain from `root`, as (bytes, path).

    **A conservative ceiling, not the true peak.** Every direct call edge is followed whether or
    not the path is feasible: a read-only `open_file_in_dir` monomorphization still references
    `alloc_cluster`/`update_fat`, so the reported chain includes FAT-write frames a rescan can
    never execute. That is why the boot-chain figure is gated against a baselined ceiling (drift
    detection) and the on-glass stack high-water stays the authority for real headroom.

    Indirect calls (`blx`, dyn dispatch) are invisible to it, so it is not a lower bound either.
    """
    memo: dict[str, tuple[int, tuple[str, ...]]] = {}

    def walk(function: str, active: frozenset[str]) -> tuple[int, tuple[str, ...], bool]:
        """(cost, path, truncated) — `truncated` when the recursion guard cut this subtree."""
        if function in active:  # recursion: stop, don't unroll
            return 0, (), True
        if function in memo:
            cost, path = memo[function]
            return cost, path, False
        deepest, deepest_path, truncated = 0, (), False
        for callee in sorted(parsed.callees.get(function, ())):
            cost, path, callee_truncated = walk(callee, active | {function})
            truncated = truncated or callee_truncated
            if cost > deepest:
                deepest, deepest_path = cost, path
        total = parsed.entry_cost(function) + deepest
        result = (total, (f"{function} ({parsed.entry_cost(function)} B)", *deepest_path))
        # Memoize only subtrees the recursion guard did NOT cut. A truncated value depends on which
        # functions were already on the walk's stack, so reusing it elsewhere would under-report;
        # caching the rest is what keeps a wide call graph from going exponential.
        if not truncated:
            memo[function] = result
        return (*result, truncated)

    sys.setrecursionlimit(max(sys.getrecursionlimit(), 20_000))
    cost, path, _ = walk(root, frozenset())
    return cost, path


def reaches_any(parsed: Disassembly, root: str, targets: frozenset[str]) -> bool:
    """Whether an outlined function can reach one of `targets` through direct call edges."""
    pending = [root]
    seen: set[str] = set()
    while pending:
        function = pending.pop()
        if function in targets:
            return True
        if function in seen:
            continue
        seen.add(function)
        pending.extend(parsed.callees.get(function, ()))
    return False


def resolve_symbol(parsed: Disassembly, needle: str, description: str) -> str:
    """The one symbol containing `needle` (mangling-hash-insensitive), or a stale-parser error.

    Both sides go through [`canonical_symbol`], so a baselined needle is spelled the way Rust spells
    a path and matches whichever rendering the demangler on this host produces. Not routing it
    through there is what made FS7.5-c1's `mount_in_place` root resolve locally and go stale on CI.
    """
    wanted = canonical_symbol(needle)
    matches = sorted(name for name in parsed.symbols if wanted in canonical_symbol(name))
    if not matches:
        # **Show what the demangler in front of us actually rendered.** A bare "no symbol contains
        # X" cannot distinguish "inlined away" from "spelled differently here", and the difference
        # decides the fix. Retrying on the needle's last path segment is what turns a round of
        # guessing at someone else's host into a one-shot correction: FS7.5-c1 burned a CI round
        # because the message could not say whether the symbol was gone or merely renamed.
        tail = needle.rsplit("::", 1)[-1]
        near = sorted(name for name in parsed.symbols if tail and tail in canonical_symbol(name))
        hint = (
            f" Symbols containing `{tail}`: {', '.join(name[:90] for name in near[:4])}"
            if near
            else f" No symbol contains `{tail}` either, so it really is gone from this image."
        )
        raise GuardError(
            f"{description} guard is stale: no symbol contains `{needle}`; it was renamed, "
            "inlined away, or is no longer reached from the boot path." + hint
        )
    if len(matches) > 1:
        raise GuardError(
            f"{description} guard is ambiguous: `{needle}` matches {len(matches)} symbols "
            f"({', '.join(name[:60] for name in matches[:4])}); tighten the baselined root"
        )
    return matches[0]


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


def measure_boot_chain(parsed: Disassembly, elf: Path, chain_roots: list[str]) -> BootChain:
    """The boot-path stack picture: residual stack, the largest task body, the deepest boot chain.

    `chain_roots` are the baselined `#[inline(never)]` boot constructors (substring match on the
    demangled symbol, so the mangling hash is not pinned). They are **sibling** steps — each one
    returns before the next is called from the task body — so the chain is the task frame plus the
    deepest single root, not their sum.
    """
    stack_start, euninit = parse_stack_bounds(run_tool("llvm-nm", "--demangle", elf))
    deepest = (0, "", ())
    # **Every** stale root, not the first. One masking another is how a second blind spot survives a
    # round that was opened to fix the first: the reported ceiling is missing both chains either way,
    # so a reader who fixes the one name in the message would find the guard still wrong.
    stale: list[str] = []
    resolved_roots: list[str] = []
    for needle in chain_roots:
        try:
            root = resolve_symbol(parsed, needle, f"boot-chain root `{needle}`")
        except GuardError as error:
            stale.append(str(error))
            continue
        resolved_roots.append(root)
        cost, path = chain_cost(parsed, root)
        if cost > deepest[0]:
            deepest = (cost, root, path)

    try:
        task_frames = select_task_body_frames(parsed)
    except GuardError:
        task_bodies = [name for name in parsed.symbols if is_task_body_symbol(canonical_symbol(name))]
        if task_bodies:
            # The symbols exist but their prologue spelling moved; the task parser itself owns the
            # precise stale diagnostic and this is not the optimizer-inlining case below.
            raise
        # LLVM may inline the generated embassy task body into its TaskStorage::poll
        # monomorphization. That poll then owns the same permanent thread-mode stack cost. Identify
        # the main poll by the baselined boot roots it reaches; requiring exactly one candidate is
        # what keeps this from degrading into an arbitrary "largest poll" fallback.
        poll_frames = select_poll_frames(parsed)
        targets = frozenset(resolved_roots)
        task_frames = {
            name: frame for name, frame in poll_frames.items() if reaches_any(parsed, name, targets)
        }
        require(
            len(task_frames) == 1,
            "task-body guard is stale: no out-of-line `____embassy_*_task` body exists and "
            f"{len(task_frames)} TaskStorage::poll symbols reach the baselined boot roots",
        )
    task_symbol = max(task_frames, key=lambda name: task_frames[name])
    chain_ceiling, chain_root, chain_path = deepest
    return BootChain(
        residual_stack=stack_start - euninit,
        task_frame=task_frames[task_symbol],
        task_frame_symbol=task_symbol,
        chain_ceiling=task_frames[task_symbol] + chain_ceiling,
        chain_root=chain_root,
        chain_path=chain_path,
        chain_error="; ".join(stale) if stale else None,
    )


def measure_board(
    elf: Path, framebuffer_bytes: int, include_poll: bool, chain_roots: list[str] | None
) -> BoardMeasurement:
    sections = parse_size_output(run_tool("llvm-size", "-A", elf), extra_required=frozenset({".uninit"}))
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
    boot = None
    if include_poll or chain_roots is not None:
        parsed = parse_disassembly(run_tool("llvm-objdump", "--demangle", "-d", elf))
        if include_poll:
            poll = max(select_poll_frames(parsed).values())
        if chain_roots is not None:
            boot = measure_boot_chain(parsed, elf, chain_roots)
    return BoardMeasurement(
        bss=sections[".bss"],
        data=sections[".data"],
        uninit=sections[".uninit"],
        flash=sum(sections[name] for name in FLASH_SECTIONS),
        framebuffer_symbols=framebuffer_symbols,
        full_frame_sized_writable=full_frame_sized,
        largest_poll_frame=poll,
        arena_symbols=tuple(symbol for symbol in symbols if is_arena_symbol(symbol.name)),
        boot=boot,
    )


def load_baseline(path: Path) -> dict[str, object]:
    try:
        baseline = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise GuardError(f"cannot read baseline {path}: {error}") from error
    if baseline.get("schema_version") != 3:
        raise GuardError(
            f"unsupported baseline schema in {path}; expected schema_version 3 "
            "(v2 added the boot-chain block: task_frame_limit / residual_stack_min / "
            "boot_chain_ceiling / boot_chain_roots; v3 added the deep-ride high-water gate: "
            "deep_ride_high_water / deep_ride_margin_min)"
        )
    return baseline


def require(condition: bool, message: str) -> None:
    if not condition:
        raise GuardError(message)


def check_board(args: argparse.Namespace, baseline: dict[str, object]) -> None:
    profile = baseline["board"][args.profile]
    framebuffer_bytes = profile["framebuffer_bytes"]
    include_poll = "poll_frame_limit" in profile
    chain_roots = profile.get("boot_chain_roots")
    measured = measure_board(args.elf, framebuffer_bytes, include_poll, chain_roots)
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
    # A plain ceiling, and it needs no more shape than that even now that it is a real budget: until
    # #1146 P2 `.uninit` held only `defmt_rtt::BUFFER`, and the 1,024 B baseline was there to catch
    # a NOLOAD section appearing by accident. It now also holds the ~117 KB scratch arena, so this is
    # the growth gate for the arena's largest arm — pinned exactly, like `resident_ram_max`, so any
    # arm crossing the max shows up here as a linked fact and not only as a `size_of` in the report.
    # Being a ceiling, it says nothing about the arena *shrinking* or *leaving*; `check_arena` below
    # is the gate for that, and the two are only tight together.
    require(
        measured.uninit <= profile["uninit_max"],
        f"{args.profile} .uninit grew to {measured.uninit} B, above {profile['uninit_max']} B; "
        "this is the scratch arena's section (#1146 P2) — an arm grew past the current maximum, or "
        "a new NOLOAD static appeared. Itemize/approve it exactly as a resident increase",
    )
    check_arena(args.profile, profile, measured)
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
    # Distinct from `framebuffer_count` (a *name*-matched count): this is the size-based
    # "no accidental second framebuffer" net. It is not always 1 — the ~117 KB scratch `ARENA`
    # legitimately exceeds a frame, so the expected count is pinned per profile and any *new*
    # frame-sized allocation still trips the guard. Those bytes have moved twice and the count has
    # stayed 2 throughout: they were inside `APP`, then #1146 P1 gave them their own `RENDER_SCRATCH`
    # static, and #1146 P2 made them the render arm of `arena::ARENA` in `.uninit`. Membership, not
    # the count, is what says which — so read the `candidates:` list in the failure, not just the
    # number: today it is `FB` + `ARENA`.
    expected_full_frame = profile.get("full_frame_sized_writable_count", expected_count)
    require(
        len(measured.full_frame_sized_writable) == expected_full_frame,
        f"{args.profile} has {len(measured.full_frame_sized_writable)} writable allocation(s) at "
        f"least one full frame ({framebuffer_bytes} B), expected {expected_full_frame}; "
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
    if measured.boot is not None:
        check_boot_chain(args.profile, profile, measured.boot)
    print(f"{args.profile}: resource guards passed")


def check_arena(profile_name: str, profile: dict[str, object], measured: BoardMeasurement) -> None:
    """The scratch arena (#1146 P2) as a **linked symbol**, not only as a section total.

    `.uninit`'s size alone cannot say the arena is in it. The section has a second tenant
    (`defmt_rtt::BUFFER`, 1,024 B), so an arena whose `#[link_section]` is renamed to anything else
    lands in a section nothing gates and leaves every RAM gate green: `.bss + .data` unmoved (the
    bytes did not go to `.bss`), `.uninit` back to 1,024 under a `<=` ceiling, and the residual main
    stack *risen*, comfortably over its `>=` floor. 117 KB would go missing behind four passes.

    So the three requirements here are the ones that actually pin it:

    * the static exists exactly once, under its module path (a rename or a second `arena::ARENA` is
      a hard error, never a silent max-over-matches);
    * it is NOBITS and its size is **exactly** `compile_time_allocations.arena_total` — the same
      number the `report` leg pins `size_of::<ScratchArena>()` against, so the linked image and the
      target-side table cannot drift apart;
    * `.uninit` is large enough to **contain** it. That is the membership half: it fails the moment
      the arena's bytes are somewhere else, which is precisely what the required-section check
      cannot see.

    What is still *not* proven here is that those bytes sit at the arena's address rather than
    merely fitting — a `.uninit` that grew by the arena's size for an unrelated reason while it moved
    out would satisfy the arithmetic. `uninit_max` is pinned at the exact shipping total, so that
    combination cannot pass both gates today.
    """
    expected = profile.get("compile_time_allocations", {}).get("arena_total")
    require(
        expected is not None,
        f"{profile_name} baseline has no compile_time_allocations.arena_total; the board leg pins "
        "the linked scratch arena against the same figure the report leg pins (#1146 P2)",
    )
    names = [symbol.name for symbol in measured.arena_symbols]
    require(
        len(measured.arena_symbols) == 1,
        f"{profile_name} links {len(measured.arena_symbols)} scratch-arena static(s), expected 1 "
        f"(symbols: {names}); the `arena::ARENA` static was renamed, optimized out, or duplicated — "
        "if it genuinely moved, move `is_arena_symbol` with it (#1146 P2)",
    )
    arena = measured.arena_symbols[0]
    require(
        arena.kind in NOBITS_SYMBOL_TYPES,
        f"{profile_name} scratch arena `{arena.name}` is llvm-nm type `{arena.kind}`, not NOBITS; "
        "it acquired an initializer or left writable RAM — the arena must cost flash nothing and "
        "must never be zeroed at boot",
    )
    require(
        arena.size == expected,
        f"{profile_name} linked scratch arena is {arena.size} B, not the baselined "
        f"{expected} B (`arena_total`); an arm changed size — re-pin it here and in "
        "compile_time_allocations together, and mind the max-of-arms cliff in arena.rs",
    )
    require(
        measured.uninit >= arena.size,
        f"{profile_name} .uninit is {measured.uninit} B but the scratch arena is {arena.size} B: "
        "the arena is no longer linked into `.uninit`. Its `#[link_section]` was renamed or "
        "dropped, and the section's other tenant kept every other RAM gate green (#1146 P2)",
    )
    print(f"{profile_name}: scratch arena {arena.size:,} B inside .uninit {measured.uninit:,} B")


def check_boot_chain(profile_name: str, profile: dict[str, object], boot: BootChain) -> None:
    """The three boot-path stack gates added after the #1108 STKOF (see `parse_task_body_frames`).

    Two are exact — the out-of-line task frame and the residual stack — and between them they would
    have failed #1084 twice. The third, the chain ceiling, is a conservative over-approximation
    ([`chain_cost`]) gated only against its own baseline, so it catches drift without pretending to
    be a stack-safety proof; the on-glass high-water in ARCHITECTURE_RESOURCE_BASELINE.md is that.
    """
    print(
        f"{profile_name}: residual main stack {boot.residual_stack:,} B; largest task body "
        f"{boot.task_frame:,} B; boot-chain ceiling {boot.chain_ceiling:,} B"
    )
    require(
        boot.task_frame <= profile["task_frame_limit"],
        f"{profile_name} largest out-of-line task body is {boot.task_frame} B, above the "
        f"{profile['task_frame_limit']} B limit (`{boot.task_frame_symbol[:70]}`); an async fn's "
        "non-await-crossing temporary is still a PERMANENT poll-frame slot — move fat construction "
        "behind an #[inline(never)] helper (issues #677, #1108)",
    )
    require(
        boot.residual_stack >= profile["residual_stack_min"],
        f"{profile_name} residual main stack fell to {boot.residual_stack} B, below the "
        f"{profile['residual_stack_min']} B floor: resident statics grew into the stack. Every "
        ".bss AND .uninit byte is a stack byte here (the bound is `_stack_start - __euninit`, so "
        "the scratch arena counts too) — re-trim the nrf-mem caps or re-approve the floor",
    )
    require(
        boot.chain_ceiling <= profile["boot_chain_ceiling"],
        f"{profile_name} boot-chain ceiling grew to {boot.chain_ceiling} B (task body "
        f"{boot.task_frame} B + deepest root `{boot.chain_root[:60]}`), above the baselined "
        f"{profile['boot_chain_ceiling']} B. Deepest path:\n    "
        + "\n    ".join(boot.chain_path[:10]),
    )
    require(
        boot.chain_error is None,
        f"{profile_name} {boot.chain_error}",
    )
    headroom = boot.residual_stack - boot.chain_ceiling
    require(
        headroom >= profile["boot_chain_headroom_min"],
        f"{profile_name} boot-chain headroom is {headroom} B (residual {boot.residual_stack} B − "
        f"ceiling {boot.chain_ceiling} B), under the {profile['boot_chain_headroom_min']} B floor "
        "MPSL's ISRs and the unmodelled indirect calls need. This is the #1108 failure mode: it "
        "goes red BEFORE the board stops booting",
    )
    check_deep_ride_high_water(profile_name, profile, boot)


def check_deep_ride_high_water(profile_name: str, profile: dict[str, object], boot: BootChain) -> None:
    """**The gate that compares the residual stack to a RUN rather than to its own floor.**

    Every other stack check here is self-referential: `residual_stack_min` is whatever the last
    approved build measured, so growing the residents and re-approving the floor is a green diff no
    matter how little stack is left. `boot_chain_ceiling` is a static over-approximation of *one*
    path — the boot chain — and says nothing about the deep ride path, which is where this board's
    stack actually peaks.

    So the baseline now carries the deepest **measured on-glass** high-water for each profile, and
    the residual has to clear it with a margin. This exists because FS7.5-c1 walked straight through
    the gap: +11,848 B of resident took the residual to 37,640 B, past a recorded 37,760 B peak on
    the `ble` profile, and every gate in this file went green.

    `deep_ride_high_water` is a **measurement, not a budget**. It moves only when someone runs the
    ride on glass and reads the stackmeter — never to make a build pass. If it is stale the honest
    fix is to re-measure it, and `deep_ride_high_water_measured` records when it last was.
    """
    # A missing key here used to die as a bare `KeyError` in a traceback, which reads as a crashed
    # tool rather than as the stale baseline it is. v3 added these two, so a profile without them is
    # a baseline that was hand-edited past its schema.
    missing = [key for key in ("deep_ride_high_water", "deep_ride_margin_min") if key not in profile]
    require(
        not missing,
        f"{profile_name} baseline is missing {', '.join(missing)}: schema v3 added the deep-ride "
        "gate, and a profile without it has no stack-safety check at all. Add the keys with a "
        "measured on-glass high-water — never a guess, and never a figure chosen to make a build pass",
    )
    high_water = profile["deep_ride_high_water"]
    margin_min = profile["deep_ride_margin_min"]
    margin = boot.residual_stack - high_water
    print(
        f"{profile_name}: deep-ride high-water {high_water:,} B "
        f"({profile['deep_ride_high_water_measured']}); residual clears it by {margin:,} B "
        f"(floor {margin_min:,} B)"
    )
    require(
        margin >= margin_min,
        f"{profile_name} residual main stack is {boot.residual_stack} B against a MEASURED deep-ride "
        f"high-water of {high_water} B ({profile['deep_ride_high_water_measured']}) — a margin of "
        f"{margin} B, under the {margin_min} B floor. This is not a budget to re-approve: the number "
        "on the right is what the board actually reached on glass, so a residual under it is a stack "
        "overflow on the deep ride path, not a tight one. Give the stack bytes back (delete or "
        "overlap residents), or re-measure the high-water on glass and move it with the evidence",
    )


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


def validate_strict_align_config(config: dict[str, object], path: Path) -> None:
    target = config.get("build", {}).get("target")
    require(
        target == EMBEDDED_TARGET,
        f"{path} does not select embedded target `{EMBEDDED_TARGET}` (found {target!r})",
    )
    target_table = config.get("target", {}).get(EMBEDDED_TARGET_CFG)
    require(
        isinstance(target_table, dict),
        f"{path} is missing target table `{EMBEDDED_TARGET_CFG}`",
    )
    rustflags = target_table.get("rustflags", [])
    wired = isinstance(rustflags, list) and any(
        rustflags[index] == "-C" and rustflags[index + 1] == "target-feature=+strict-align"
        for index in range(len(rustflags) - 1)
    )
    require(
        wired,
        f"{path} does not wire `-C target-feature=+strict-align` for `{EMBEDDED_TARGET_CFG}`",
    )


def check_strict_align_configs(paths: list[Path] | tuple[Path, ...]) -> None:
    for path in paths:
        try:
            with path.open("rb") as file:
                config = tomllib.load(file)
        except (OSError, tomllib.TOMLDecodeError) as error:
            raise GuardError(f"cannot read strict-align Cargo config {path}: {error}") from error
        validate_strict_align_config(config, path)
        print(f"strict-align Cargo wiring passed: {path}")


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
    check_strict_align_configs(args.configs or STRICT_ALIGN_CONFIGS)
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


def validate_build_rustflags(log: str, crate: str) -> None:
    """Prove the shipping crate's actual Cargo rustc command retained +strict-align."""

    crate_name = crate.replace("-", "_")
    # CI deliberately keeps Cargo colour enabled. Cargo wraps the ``Running`` label in SGR
    # sequences before tee writes the verbose build log, so matching the raw text would reject
    # the exact production invocation it is meant to inspect. Strip CSI controls only; the rustc
    # command and all of its arguments remain byte-for-byte visible to the checks below.
    plain_log = ANSI_CSI_RE.sub("", log)
    invocations = [
        line
        for line in plain_log.splitlines()
        if "Running `" in line and f"--crate-name {crate_name}" in line and "rustc" in line
    ]
    require(invocations, f"verbose Cargo log contains no rustc invocation for `{crate}`")
    strict = [
        line
        for line in invocations
        if re.search(r"(?:^|\s)-C(?:\s+|=?)target-feature=\+strict-align(?:\s|`|$)", line)
    ]
    require(
        strict,
        f"actual rustc invocation for `{crate}` omits `-C target-feature=+strict-align`; "
        "a job-wide RUSTFLAGS value may be shadowing the board Cargo config",
    )


def check_build_rustflags(args: argparse.Namespace) -> None:
    try:
        log = args.log.read_text(encoding="utf-8")
    except OSError as error:
        raise GuardError(f"cannot read verbose Cargo log {args.log}: {error}") from error
    validate_build_rustflags(log, args.crate)
    print(f"actual {args.crate} production rustc invocation includes +strict-align")


def check_frames(args: argparse.Namespace) -> None:
    """Gate the largest single stack frame among the symbols of one module, in any ELF.

    The poll-frame and boot-chain guards above measure the *app* image's async task frames. This
    measures ordinary synchronous frames in a named module, which is what the OBC2 store is made of
    — and until the DOS4 cutover puts that code in the app image, nothing else in CI looks at it.

    It exists because of a measured regression, not a hypothetical one. Three constructors in the
    OBC2 kernel returned or assigned a 56 KiB `CatalogModel` by value; placing one transaction cost
    206,080 B of transient stack and HardFaulted on MSPLIM, against a 51,576 B residual main stack in
    the shipping image (#1359). The fix took the largest OBC2 frame to 6,080 B. This is what keeps it
    there: a by-value constructor reintroduced anywhere under `--match` fails the build.
    """
    parsed = parse_disassembly(run_tool("llvm-objdump", "--demangle", "-d", args.elf))
    needle = args.match
    frames = select_frames(
        parsed,
        lambda name: needle in name,
        f"{needle} frame",
        f"`{needle}`",
    )
    symbol, largest = max(frames.items(), key=lambda item: item[1])
    print(f"{needle}: largest frame {largest:,} / {args.limit:,} B across {len(frames)} symbol(s)")
    require(
        largest <= args.limit,
        f"{needle} largest stack frame is {largest} B, above the {args.limit} B limit "
        f"(`{symbol[:90]}`); a value built in a return slot rather than in place is the usual cause "
        "— see KernelTransaction::mount_in_place and CatalogModel::init_empty (#1359)",
    )


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
    frames = commands.add_parser("frames", help="gate the largest stack frame of one module in any ELF")
    frames.add_argument("--elf", type=Path, required=True)
    frames.add_argument("--match", required=True, help="substring of the demangled symbol names to gate")
    frames.add_argument("--limit", type=int, required=True, help="largest permitted single `sub sp` in bytes")
    strict = commands.add_parser("strict-align", help="prove the active ARM backend honors +strict-align")
    strict.add_argument("--probe", type=Path, default=STRICT_ALIGN_PROBE)
    strict.add_argument(
        "--config",
        dest="configs",
        action="append",
        type=Path,
        help="Cargo config that must wire +strict-align (repeatable; defaults to board + boot)",
    )
    actual = commands.add_parser(
        "build-rustflags",
        help="prove a shipping crate's actual verbose Cargo invocation includes +strict-align",
    )
    actual.add_argument("--log", type=Path, required=True)
    actual.add_argument("--crate", required=True)
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
        elif args.command == "frames":
            check_frames(args)
        elif args.command == "strict-align":
            check_strict_align(args)
        else:
            check_build_rustflags(args)
    except (GuardError, KeyError, TypeError) as error:
        print(f"resource guard failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
