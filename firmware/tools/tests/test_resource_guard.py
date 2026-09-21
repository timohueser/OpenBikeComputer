import importlib.util
import struct
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock


SCRIPT = Path(__file__).parents[1] / "resource_guard.py"
SPEC = importlib.util.spec_from_file_location("resource_guard", SCRIPT)
resource_guard = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = resource_guard
SPEC.loader.exec_module(resource_guard)


# The shipping scratch arena, as `llvm-nm --print-size --demangle` prints it. Spelled out so a
# rename of the static fails these tests rather than silently disabling the gate that pins it.
ARENA_NAME = "obc_fw_nrf54l::arena::ARENA::ha27c553b3defd127"
ARENA_BYTES = 92_320
UNINIT_BYTES = 93_344  # the arena + `defmt_rtt::BUFFER`, its only other tenant


def arena_symbol(size=ARENA_BYTES, kind="b", name=ARENA_NAME):
    return resource_guard.Symbol(size, kind, name)


class ResourceGuardTests(unittest.TestCase):
    def test_size_parser_requires_contract_sections(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "missing section.*\\.rodata"):
            resource_guard.parse_size_output(".vector_table 10 0\n.text 20 10\n.data 4 30\n.bss 8 34\n")

    def test_size_parser_requires_uninit_when_the_caller_asks(self):
        """#1146 P2: the scratch arena lives in `.uninit`, so a board leg that cannot see the
        section must fail rather than measure it as zero."""
        full = ".vector_table 10 0\n.text 20 10\n.rodata 6 30\n.data 4 36\n.bss 8 40\n"
        self.assertEqual(resource_guard.parse_size_output(full)[".bss"], 8)  # bootloader shape: fine
        with self.assertRaisesRegex(resource_guard.GuardError, "missing section.*\\.uninit"):
            resource_guard.parse_size_output(full, extra_required=frozenset({".uninit"}))
        with_uninit = full + ".uninit 92320 48\n"
        self.assertEqual(
            resource_guard.parse_size_output(with_uninit, extra_required=frozenset({".uninit"}))[".uninit"],
            92_320,
        )

    def test_nm_parser_and_framebuffer_identity(self):
        symbols = resource_guard.parse_nm_output(
            "20000060 00012c00 b obc_fw_nrf54l::FB::h1234abcd\n"
            "20012c60 00000504 b obc_fw_nrf54l::ROW_DIFF::h5678abcd\n"
        )
        self.assertEqual(symbols[0].size, 76_800)
        self.assertTrue(resource_guard.is_framebuffer_symbol(symbols[0].name))
        self.assertFalse(resource_guard.is_framebuffer_symbol(symbols[1].name))

    def test_nm_parser_fails_loudly_when_format_is_stale(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "no sized symbols"):
            resource_guard.parse_nm_output("unexpected llvm-nm output")

    def test_poll_parser_accepts_both_thumb_spellings(self):
        disassembly = """
00001000 <embassy_executor::raw::TaskStorage$LT$F$GT$::poll::ha>:
    1000: b082          sub sp, #0x8
00002000 <embassy_executor::raw::TaskStorage$LT$F$GT$::poll::hb>:
    2000: f5ad 5dc3     sub.w sp, sp, #0x1860
"""
        frames = resource_guard.parse_poll_frames(disassembly)
        self.assertEqual(max(frames.values()), 6_240)

    def test_poll_parser_rejects_missing_symbols(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "no `TaskStorage"):
            resource_guard.parse_poll_frames("00001000 <some_other_function>:\n")

    def test_poll_parser_rejects_stale_instruction_match(self):
        disassembly = """
00001000 <embassy_executor::raw::TaskStorage$LT$F$GT$::poll::ha>:
    1000: dead beef     future-prologue-spelling sp
"""
        with self.assertRaisesRegex(resource_guard.GuardError, "symbols exist but no `sub sp"):
            resource_guard.parse_poll_frames(disassembly)

    def test_strict_align_parser_extracts_only_requested_function(self):
        assembly = """
decode_u32:
\tldrb\tr1, [r0]
.Lfunc_end0:
other:
\tldr\tr0, [r0]
.Lfunc_end1:
"""
        self.assertIn("ldrb", resource_guard.function_assembly(assembly, "decode_u32"))
        self.assertNotIn("\tldr\t", resource_guard.function_assembly(assembly, "decode_u32"))

    def test_strict_align_parser_fails_loudly_when_format_is_stale(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "assembly not found"):
            resource_guard.function_assembly("unexpected assembly", "decode_u32")

    def test_strict_align_config_requires_shipping_target_and_flag(self):
        valid = {
            "build": {"target": resource_guard.EMBEDDED_TARGET},
            "target": {
                resource_guard.EMBEDDED_TARGET_CFG: {
                    "rustflags": ["-C", "target-feature=+strict-align"]
                }
            },
        }
        resource_guard.validate_strict_align_config(valid, Path("valid.toml"))

        wrong_target = dict(valid)
        wrong_target["build"] = {"target": "host"}
        with self.assertRaisesRegex(resource_guard.GuardError, "does not select embedded target"):
            resource_guard.validate_strict_align_config(wrong_target, Path("wrong-target.toml"))

        missing_flag = {
            "build": {"target": resource_guard.EMBEDDED_TARGET},
            "target": {resource_guard.EMBEDDED_TARGET_CFG: {"rustflags": ["-C", "opt-level=3"]}},
        }
        with self.assertRaisesRegex(resource_guard.GuardError, "does not wire"):
            resource_guard.validate_strict_align_config(missing_flag, Path("missing-flag.toml"))

    def test_actual_shipping_rustc_invocation_requires_strict_align(self):
        valid = (
            "Running `rustc --crate-name obc_fw_nrf54l --edition=2021 "
            "-C target-feature=+strict-align src/main.rs`"
        )
        resource_guard.validate_build_rustflags(valid, "obc-fw-nrf54l")
        with self.assertRaisesRegex(resource_guard.GuardError, "actual rustc invocation.*omits"):
            resource_guard.validate_build_rustflags(
                "Running `rustc --crate-name obc_fw_nrf54l --edition=2021 src/main.rs`",
                "obc-fw-nrf54l",
            )
        with self.assertRaisesRegex(resource_guard.GuardError, "no rustc invocation"):
            resource_guard.validate_build_rustflags("Fresh release build", "obc-fw-nrf54l")

    def test_actual_shipping_rustc_invocation_accepts_cargo_colour_sequences(self):
        ci_log = (
            "\x1b[1m\x1b[92m     Running\x1b[0m `rustc --crate-name obc_boot "
            "--edition=2021 -C target-feature=+strict-align src/main.rs`\n"
        )
        resource_guard.validate_build_rustflags(ci_log, "obc-boot")

    def test_resource_table_is_self_describing(self):
        def entry(name, value):
            return name.encode().ljust(32, b"\0") + struct.pack("<I", value)

        table = resource_guard.decode_resource_table(entry("format_version", 1) + entry("app", 42))
        self.assertEqual(table, {"format_version": 1, "app": 42})

    def test_resource_table_rejects_layout_drift(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "not a multiple"):
            resource_guard.decode_resource_table(b"short")

    def _board_baseline(self, **overrides):
        profile = {
            "framebuffer_bytes": 76_800,
            "resident_ram_max": 120,
            "resident_ram_slack": 8,
            "uninit_max": UNINIT_BYTES,
            "framebuffer_count": 0,
            "compile_time_allocations": {"arena_total": ARENA_BYTES},
        }
        profile.update(overrides)
        return {"board": {"default": profile}}

    def _board_measured(self, **overrides):
        fields = {
            "bss": 100,
            "data": 20,
            "uninit": UNINIT_BYTES,
            "flash": 0,
            "framebuffer_symbols": (),
            "full_frame_sized_writable": (),
            "largest_poll_frame": None,
            "arena_symbols": (arena_symbol(),),
        }
        fields.update(overrides)
        return resource_guard.BoardMeasurement(**fields)

    def _check_board(self, measured, baseline, ci_authority=True):
        args = SimpleNamespace(profile="default", elf=Path("fake"), ci_authority=ci_authority)
        with mock.patch.object(resource_guard, "measure_board", return_value=measured):
            resource_guard.check_board(args, baseline)

    def test_board_guard_explains_resident_ram_growth(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "resident RAM grew.*itemize/approve"):
            self._check_board(self._board_measured(bss=101), self._board_baseline())

    def test_board_guard_fails_when_the_link_shrinks_out_of_the_resident_band(self):
        """The half a `<=` ceiling cannot have: a link that saves RAM must re-pin the row.

        Without it the recorded ceiling drifts above the real link one saving at a time, and the
        headroom the next slice reads off the baseline is fiction.
        """
        with self.assertRaisesRegex(
            resource_guard.GuardError, "111 B .*, 9 B below the 120 B ceiling.*8 B slack"
        ):
            self._check_board(self._board_measured(bss=91), self._board_baseline())
        self._check_board(self._board_measured(bss=92), self._board_baseline())  # 8 B below: in band

    def test_the_shrink_gate_is_a_warning_without_CI_authority(self):
        """`measured_resident` is the `embedded` job's link, and a host toolchain links less.

        So a head build outside that job must not fail this end of the band — it would reject every
        local measurement run for a reason outside the change, and name a build that is not the
        gate's source as the place to re-pin from.
        """
        with mock.patch("builtins.print") as printed:
            self._check_board(
                self._board_measured(bss=91), self._board_baseline(), ci_authority=False
            )
        warning = next(c.args[0] for c in printed.call_args_list if "WARNING" in str(c.args[0]))
        self.assertIn("9 B below the 120 B ceiling", warning)
        self.assertIn("`embedded` CI job", warning)

    def test_board_guard_explains_missing_framebuffer_symbol(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "framebuffer symbol count is 0"):
            self._check_board(self._board_measured(), self._board_baseline(framebuffer_count=1))

    def test_arena_gate_catches_the_renamed_link_section(self):
        """#1150 review: the finding this gate exists for.

        `.uninit` has a second tenant, so an arena whose `#[link_section]` is renamed off it leaves
        the section present at 1,024 B — the required-section check passes, `.bss + .data` is
        unmoved (the bytes did not fall back to `.bss`, they went to the new section), `uninit_max`
        is a ceiling, and the residual stack only *grew*. Every RAM gate green, 92 KB missing. Only
        the arena-must-fit-in-.uninit half sees it.
        """
        resource_guard.parse_size_output(
            ".vector_table 10 0\n.text 20 10\n.rodata 6 30\n.data 4 36\n.bss 8 40\n.uninit 1024 48\n",
            extra_required=frozenset({".uninit"}),
        )  # the required-set tripwire is happy: the section is still there
        with self.assertRaisesRegex(resource_guard.GuardError, r"no longer linked into `\.uninit`"):
            self._check_board(
                self._board_measured(uninit=1_024),
                self._board_baseline(uninit_max=UNINIT_BYTES),
            )

    def test_arena_gate_pins_the_linked_size_to_the_report_figure(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "arena is 92336 B, not the baselined 92320"):
            self._check_board(
                self._board_measured(arena_symbols=(arena_symbol(size=92_336),)),
                self._board_baseline(),
            )

    def test_arena_gate_fails_loudly_when_the_static_is_renamed_or_doubled(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "links 0 scratch-arena static"):
            self._check_board(self._board_measured(arena_symbols=()), self._board_baseline())
        with self.assertRaisesRegex(resource_guard.GuardError, "links 2 scratch-arena static"):
            self._check_board(
                self._board_measured(arena_symbols=(arena_symbol(), arena_symbol(name="x::arena::ARENA"))),
                self._board_baseline(),
            )

    def test_arena_gate_requires_a_nobits_static(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "not NOBITS"):
            self._check_board(
                self._board_measured(arena_symbols=(arena_symbol(kind="d"),)),
                self._board_baseline(),
            )

    def test_arena_symbol_matcher_ignores_other_arenas(self):
        self.assertTrue(resource_guard.is_arena_symbol(ARENA_NAME))
        self.assertTrue(resource_guard.is_arena_symbol("obc_fw_nrf54l::arena::ARENA"))
        self.assertFalse(resource_guard.is_arena_symbol("obc_fw_nrf54l::arena::GATE::ha95"))
        self.assertFalse(resource_guard.is_arena_symbol("nrf_sdc::mem::ARENA::hbe"))
        self.assertFalse(resource_guard.is_arena_symbol("embassy_executor::TASK_ARENA::hbe"))

    def test_the_shipping_board_measurement_passes(self):
        self._check_board(self._board_measured(), self._board_baseline())


# The boot stack-overflow guards. `MAIN_TASK` is the demangled spelling of the symbol
# `parse_poll_frames` cannot see, so an embassy rename fails these tests rather than silently
# disabling the gate.
MAIN_TASK = (
    "obc_fw_nrf54l::____embassy_main_task::____embassy_main_task_inner_function"
    "::_$u7b$$u7b$closure$u7d$$u7d$::ha6608219ad9d4537"
)


class FixedEntryTests(unittest.TestCase):
    # Captured fixed entries from the saved shipping image, including the first body instruction.
    CAPTURED = """
00091500 <embassy_executor::raw::TaskStorage$LT$F$GT$::poll::hb9e01918510eab0b>:
   91500: b5f0          push {r4, r5, r6, r7, lr}
   91502: af03          add r7, sp, #0xc
   91504: e92d 0f00     push.w {r8, r9, r10, r11}
   91508: b081          sub sp, #0x4
   9150a: ed2d 8b0a     vpush {d8, d9, d10, d11, d12}
   9150e: f5ad 5d1a     sub.w sp, sp, #0x2680
   91512: b082          sub sp, #0x8
   91514: a940          add r1, sp, #0x100
00071c80 <obc_fw_nrf54l::arena::NavGuard::transform>:
   71c80: b5f0          push {r4, r5, r6, r7, lr}
   71c82: af03          add r7, sp, #0xc
   71c84: e92d 0f00     push.w {r8, r9, r10, r11}
   71c88: b081          sub sp, #0x4
   71c8a: ed2d 8b10     vpush {d8, d9, d10, d11, d12, d13, d14, d15}
   71c8e: f5ad 5dc4     sub.w sp, sp, #0x1880
   71c92: b2c9          uxtb r1, r1
0012e088 <obc_route::splice::Splicer::finish_splice>:
  12e088: b5f0          push {r4, r5, r6, r7, lr}
  12e08a: af03          add r7, sp, #0xc
  12e08c: e92d 0f00     push.w {r8, r9, r10, r11}
  12e090: b081          sub sp, #0x4
  12e092: ed2d 8b04     vpush {d8, d9}
  12e096: b090          sub sp, #0x40
  12e098: f24b 3448     movw r4, #0xb348
00041364 <obc_fw_nrf54l::storage::Writer::try_call_owned>:
   41364: b5f0          push {r4, r5, r6, r7, lr}
   41366: af03          add r7, sp, #0xc
   41368: f84d 8d04     str r8, [sp, #-4]!
   4136c: f5ad 7d7e     sub.w sp, sp, #0x3f8
   41370: 4604          mov r4, r0
0001affe <obc_storage::flat::store::FlatStore::current_revision>:
   1affe: b5f0          push {r4, r5, r6, r7, lr}
   1b000: af03          add r7, sp, #0xc
   1b002: f84d bd04     str r11, [sp, #-4]!
   1b006: b0c0          sub sp, #0x100
   1b008: 4604          mov r4, r0
"""

    def test_captured_entries_include_alignment_and_vfp_saves(self):
        parsed = resource_guard.parse_disassembly(self.CAPTURED)
        self.assertEqual(max(resource_guard.select_poll_frames(parsed).values()), 9_944)
        for needle, expected in [("transform", 6_376), ("finish_splice", 120),
                                 ("try_call_owned", 1_040), ("current_revision", 280)]:
            frames = resource_guard.select_frames(parsed, lambda name: needle in name, needle, needle)
            self.assertEqual(list(frames.values()), [expected])
            self.assertEqual(parsed.entry_cost(next(iter(frames))), expected)

    def test_ranges_split_decrements_and_body_boundary(self):
        parsed = resource_guard.parse_disassembly("""
00001000 <ranges>:
    1000: b5f0          push {r4-r7, lr}
    1002: 466f          mov r7, sp
    1004: b081          sub sp, #4
    1006: ed2d 8a04     vpush {s16-s19}
    100a: f50d 7b00     add.w r11, sp, #0
    100e: f1ad 0d20     sub.w sp, sp, #32
    1012: f2ad 0d10     subw sp, sp, #16
    1016: 4608          mov r0, r1
    1018: b510          push {r4, lr}
    101a: f5ad 4d80     sub.w sp, sp, #0x4000
00002000 <epilogue>:
    2000: b500          push {lr}
    2002: b082          sub sp, #8
    2004: b002          add sp, #8
    2006: bd00          pop {pc}
    2008: b5f0          push {r4-r7, lr}
00003000 <branches>:
    3000: b500          push {lr}
    3002: 2800          cmp r0, #0
    3004: d001          beq 0x300a <branches+0xa>
    3006: b090          sub sp, #64
    3008: e001          b 0x300e <branches+0xe>
    300a: b0a0          sub sp, #128
00004000 <body_store>:
    4000: e920 0006     stmdb r0!, {r1, r2}
    4004: b090          sub sp, #64
""")
        self.assertEqual(parsed.entry_cost("ranges"), 88)
        self.assertEqual(parsed.entry_cost("epilogue"), 12)
        self.assertEqual(parsed.entry_cost("branches"), 4)
        self.assertEqual(parsed.entry_cost("body_store"), 0)

    def test_unsupported_guarded_entry_never_passes_with_a_partial_cost(self):
        for instruction in ["sub.w sp, sp, r0", "vpush {d15-d8}", "push {future}", "<unknown>",
                            "str r8, [sp, #-8]!", "str r8, [sp], #-4", "strd r8, r9, [sp, #-8]!",
                            "str future, [sp, #-4]!", "stmdb sp!, {r8}"]:
            with self.subTest(instruction=instruction):
                parsed = resource_guard.parse_disassembly(
                    "00001000 <guarded::known>:\n    1000: b084 sub sp, #16\n"
                    "00002000 <guarded::unknown>:\n    2000: b500 push {lr}\n"
                    f"    2002: dead beef {instruction}\n"
                )
                with self.assertRaisesRegex(resource_guard.GuardError, "unsupported fixed entry.*unknown"):
                    resource_guard.select_frames(parsed, lambda name: "guarded" in name, "guarded", "guarded")
                with self.assertRaisesRegex(resource_guard.GuardError, "unsupported fixed entry"):
                    resource_guard.chain_cost(parsed, "guarded::unknown")


class BootChainTests(unittest.TestCase):
    def test_task_body_parser_sees_the_symbol_the_poll_parser_misses(self):
        disassembly = f"""
00001000 <{MAIN_TASK}>:
    1000: f5ad 4d9f     sub.w sp, sp, #0x4f80
00002000 <embassy_executor::raw::TaskStorage$LT$F$GT$::poll::ha>:
    2000: f5ad 5dc3     sub.w sp, sp, #0x1860
"""
        self.assertEqual(max(resource_guard.parse_task_body_frames(disassembly).values()), 20_352)
        # The poll guard cannot see the main task.
        self.assertEqual(max(resource_guard.parse_poll_frames(disassembly).values()), 6_240)

    def test_task_body_parser_rejects_missing_symbols(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "no `____embassy_\\*_task` body"):
            resource_guard.parse_task_body_frames("00001000 <core::ptr::drop_in_place>:\n")

    def test_an_inlined_main_task_is_measured_at_the_poll_that_reaches_boot(self):
        root = "obc_fw_nrf54l::link::init_store::hd4"
        poll = "embassy_executor::raw::TaskStorage$LT$F$GT$::poll::h01"
        parsed = resource_guard.parse_disassembly(
            f"00001000 <{poll}>:\n"
            "     ff8: b5f0          push {r4-r7, lr}\n"
            "     ffa: af03          add r7, sp, #12\n"
            "     ffc: ed2d 8b04     vpush {d8-d9}\n"
            "     ffe: b081          sub sp, #4\n"
            "    1000: f5ad 6d80     sub.w sp, sp, #0x1000\n"
            f"    1004: f000 f800     bl 0x2000 <{root}>\n"
            f"00002000 <{root}>:\n"
            "    2000: b084          sub sp, #0x10\n"
        )
        with mock.patch.object(resource_guard, "run_tool", return_value=""), mock.patch.object(
            resource_guard, "parse_stack_bounds", return_value=(0x2007D000, 0x20071228)
        ):
            boot = resource_guard.measure_boot_chain(parsed, Path("fake"), ["link::init_store"])
        self.assertEqual(boot.task_frame, 4_136)
        self.assertEqual(boot.task_frame_symbol, poll)
        self.assertEqual(boot.chain_ceiling, 4_152)

    def test_frame_parser_accepts_the_wide_subw_spelling(self):
        # `subw sp, sp, #imm` is a distinct encoding from `sub.w`; `mount_terrain` uses it.
        parsed = resource_guard.parse_disassembly(
            "00001000 <obc_fw_nrf54l::mount_terrain::hda0>:\n    1000: f6ad 0dc4  subw sp, sp, #0x8c4\n"
        )
        self.assertEqual(parsed.frames["obc_fw_nrf54l::mount_terrain::hda0"], 2_244)

    def test_a_generic_root_resolves_from_either_demangler_spelling(self):
        """**The FS7.5-c1 regression, pinned.** A boot-chain root was baselined as
        `FlatStore$LT$D$GT$::mount_in_place` — the spelling one host's demangler produced — and on CI
        it did not resolve: the guard went red as "stale" *and* the chain fell back to a ceiling with
        the whole mount missing from it. A needle is spelled the way Rust spells a path; both
        renderings must find the same symbol."""
        parsed = resource_guard.parse_disassembly(
            "00001000 <obc_storage::flat::store::FlatStore$LT$D$GT$::mount_in_place::h47f>:\n"
            "    1000: b084          sub sp, #0x10\n"
        )
        rust_spelling = resource_guard.resolve_symbol(parsed, "FlatStore<D>::mount_in_place", "root")
        escaped_spelling = resource_guard.resolve_symbol(parsed, "FlatStore$LT$D$GT$::mount_in_place", "root")
        self.assertEqual(rust_spelling, escaped_spelling)
        # And the name reported back is the one the tool emitted, not the normalised form.
        self.assertIn("$LT$", rust_spelling)

    def test_a_stale_root_reports_what_the_demangler_actually_rendered(self):
        """A bare "no symbol contains X" cannot tell "inlined away" from "spelled differently on this
        host", and the difference decides the fix. FS7.5-c1 burned a CI round on exactly that
        ambiguity, so the message now shows the candidates."""
        parsed = resource_guard.parse_disassembly(
            "00001000 <obc_storage::flat::store::FlatStore$LT$SomeCard$GT$::mount_in_place::h47f>:\n"
            "    1000: b084          sub sp, #0x10\n"
        )
        with self.assertRaises(resource_guard.GuardError) as caught:
            resource_guard.resolve_symbol(parsed, "FlatStore<D>::mount_in_place", "root")
        message = str(caught.exception)
        self.assertIn("Symbols containing `mount_in_place`", message)
        self.assertIn("SomeCard", message, "the message must show the rendering that does exist")

    def test_a_genuinely_absent_root_says_so_rather_than_offering_candidates(self):
        parsed = resource_guard.parse_disassembly(
            "00001000 <something::else::hff>:\n    1000: b084          sub sp, #0x10\n"
        )
        with self.assertRaises(resource_guard.GuardError) as caught:
            resource_guard.resolve_symbol(parsed, "gone::mount_in_place", "root")
        self.assertIn("really is gone from this image", str(caught.exception))

    def test_every_stale_boot_chain_root_is_reported_not_just_the_first(self):
        """One stale root masking another is how a second blind spot survives the round opened to fix
        the first: the reported ceiling is missing both chains either way, so a reader who fixes the
        one name in the message would find the guard still wrong."""
        parsed = resource_guard.parse_disassembly(
            "00001000 <embassy_executor::raw::TaskStorage$LT$F$GT$::poll::h01>:\n"
            "    1000: b084          sub sp, #0x10\n"
            "00002000 <obc_fw_nrf54l::link::init_store::hd4>:\n"
            "    2000: b082          sub sp, #0x8\n"
        )
        with mock.patch.object(resource_guard, "run_tool", return_value=""), mock.patch.object(
            resource_guard, "parse_stack_bounds", return_value=(0x2007D000, 0x20071228)
        ), mock.patch.object(
            resource_guard, "select_task_body_frames", return_value={"main::task": 1_024}
        ):
            boot = resource_guard.measure_boot_chain(
                parsed, Path("fake"), ["link::init_store", "gone::one", "gone::two"]
            )
        self.assertIsNotNone(boot.chain_error)
        self.assertIn("gone::one", boot.chain_error)
        self.assertIn("gone::two", boot.chain_error, "a second stale root must not be masked")
        # The root that *did* resolve still contributes, so the ceiling is not silently zero.
        self.assertGreater(boot.chain_ceiling, 1_024)

    def test_stack_bounds_are_the_residual_stack(self):
        output = "20071228 B __euninit\n2007d000 A _stack_start\n20000000 D __edata\n"
        stack_start, euninit = resource_guard.parse_stack_bounds(output)
        self.assertEqual(stack_start - euninit, 48_600)

    def test_stack_bounds_reject_statics_overrunning_ram(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "no residual stack"):
            resource_guard.parse_stack_bounds("2007d000 B __euninit\n20071228 A _stack_start\n")

    def test_stack_bounds_parser_fails_loudly_when_linker_symbols_move(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "did not report _stack_start"):
            resource_guard.parse_stack_bounds("20071228 B __euninit\n")

    def test_chain_cost_sums_frames_and_pushes_along_the_deepest_edge(self):
        parsed = resource_guard.parse_disassembly("""
00001000 <root>:
    1000: b5f0          push {r4, r5, r6, r7, lr}
    1002: b084          sub sp, #0x10
    1004: f000 f800     bl 0x2000 <shallow>
    1008: f000 f800     bl 0x3000 <deep>
00002000 <shallow>:
    2000: b082          sub sp, #0x8
00003000 <deep>:
    3000: b084          sub sp, #0x10
    3002: f000 f800     bl 0x4000 <leaf>
00004000 <leaf>:
    4000: b082          sub sp, #0x8
""")
        # root (16 + 5*4 pushed) + deep (16) + leaf (8) — the shallow branch is not counted.
        cost, path = resource_guard.chain_cost(parsed, "root")
        self.assertEqual(cost, 60)
        self.assertEqual([step.split(" ")[0] for step in path], ["root", "deep", "leaf"])

    def test_chain_cost_survives_recursion(self):
        parsed = resource_guard.parse_disassembly("""
00001000 <a>:
    1000: b082          sub sp, #0x8
    1002: f000 f800     bl 0x2000 <b>
00002000 <b>:
    2000: b082          sub sp, #0x8
    2002: f000 f800     bl 0x1000 <a>
""")
        cost, _ = resource_guard.chain_cost(parsed, "a")
        self.assertEqual(cost, 16)

    def _boot_baseline(self, **overrides):
        profile = {
            "framebuffer_bytes": 76_800,
            "resident_ram_max": 120,
            "resident_ram_slack": 8,
            "uninit_max": UNINIT_BYTES,
            "framebuffer_count": 0,
            "compile_time_allocations": {"arena_total": ARENA_BYTES},
            "task_frame_limit": 21_504,
            "residual_stack_min": 48_600,
            "boot_chain_ceiling": 43_008,
            "boot_chain_headroom_min": 4_096,
            "boot_chain_roots": ["link::init_store"],
            # The deep-ride gate. The fixture's residual clears it comfortably, so it is inert for
            # every other case here; `DeepRideHighWaterTests` is where it is exercised.
            "deep_ride_high_water": 35_808,
            "deep_ride_high_water_measured": "2026-07-04 (fixture)",
            "deep_ride_margin_min": 0,
        }
        profile.update(overrides)
        return {"board": {"default": profile}}

    def _measured(self, **overrides):
        boot = {
            "residual_stack": 48_600,
            "task_frame": 20_352,
            "task_frame_symbol": MAIN_TASK,
            "chain_ceiling": 41_556,
            "chain_root": "obc_fw_nrf54l::link::init_store::hd4",
            "chain_path": ("obc_fw_nrf54l::link::init_store::hd4 (14756 B)",),
        }
        boot.update(overrides)
        return resource_guard.BoardMeasurement(
            bss=100,
            data=20,
            uninit=UNINIT_BYTES,
            flash=0,
            framebuffer_symbols=(),
            full_frame_sized_writable=(),
            largest_poll_frame=None,
            arena_symbols=(arena_symbol(),),
            boot=resource_guard.BootChain(**boot),
        )

    def _check(self, measured, baseline):
        with mock.patch.object(resource_guard, "measure_board", return_value=measured):
            resource_guard.check_board(
                SimpleNamespace(profile="default", elf=Path("fake"), ci_authority=True), baseline
            )

    def test_a_residual_under_the_measured_deep_ride_peak_fails(self):
        """**The gate FS7.5-c1 walked through.** Every other stack check here compares the residual
        to its own approved floor, so growing the residents and re-approving is green no matter how
        little stack is left. This one compares it to a number that came off the board."""
        with self.assertRaises(resource_guard.GuardError) as caught:
            self._check(
                # The chain is shrunk so the headroom gate above stays green: this test is about the
                # deep-ride check firing on its own, not about it queueing behind another failure.
                self._measured(residual_stack=35_000, chain_ceiling=10_000),
                self._boot_baseline(residual_stack_min=35_000, boot_chain_ceiling=60_000),
            )
        message = str(caught.exception)
        self.assertIn("MEASURED deep-ride high-water", message)
        self.assertIn("not a budget to re-approve", message)

    def test_the_margin_floor_is_enforced_above_the_bare_high_water(self):
        """A margin floor of zero is the weakest form of the invariant, not the only one: a profile
        that sets a real floor must fail while it is still *above* the measured peak."""
        with self.assertRaises(resource_guard.GuardError) as caught:
            self._check(
                self._measured(residual_stack=36_808, chain_ceiling=10_000),
                self._boot_baseline(
                    residual_stack_min=36_808, boot_chain_ceiling=60_000, deep_ride_margin_min=4_096
                ),
            )
        self.assertIn("a margin of 1000 B, under the 4096 B floor", str(caught.exception))

    def test_the_shipping_measurement_passes(self):
        self._check(self._measured(), self._boot_baseline())

    def test_task_frame_gate_fails_the_image_that_bricked_boot(self):
        # Real numbers from an image whose inlined terrain parse grew the main task's frame.
        with self.assertRaisesRegex(resource_guard.GuardError, "task body is 22400 B.*#1108"):
            self._check(self._measured(task_frame=22_400), self._boot_baseline())

    def test_headroom_gate_fails_when_the_chain_does_not_fit(self):
        # The same image's other symptom: the boot chain passing the stack.
        with self.assertRaisesRegex(resource_guard.GuardError, "headroom is -7932 B"):
            self._check(
                self._measured(chain_ceiling=56_532),
                self._boot_baseline(boot_chain_ceiling=60_000),
            )

    def test_residual_stack_gate_explains_statics_eating_the_stack(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "residual main stack fell to 44000"):
            self._check(self._measured(residual_stack=44_000), self._boot_baseline())

    def test_a_root_that_was_inlined_away_is_a_hard_error(self):
        # The mechanism itself: an `#[inline(never)]` boot constructor that loses its attribute
        # moves its temporary into the caller's permanent frame.
        with self.assertRaisesRegex(resource_guard.GuardError, "inlined away"):
            self._check(
                self._measured(chain_error="boot-chain root `x` guard is stale: it was inlined away"),
                self._boot_baseline(),
            )

    def test_profiles_without_boot_roots_skip_the_boot_gates(self):
        baseline = self._boot_baseline()
        del baseline["board"]["default"]["boot_chain_roots"]
        # No boot chain measured, and no error from the absent limits.
        self._check(
            resource_guard.BoardMeasurement(
                100, 20, UNINIT_BYTES, 0, (), (), None, (arena_symbol(),)
            ),
            baseline,
        )


if __name__ == "__main__":
    unittest.main()

class ModuleFrameGateTests(unittest.TestCase):
    """The module frame gate selects and bounds inherent and trait methods."""

    DISASSEMBLY = """
00001000 <obc_storage::frame_fixture::transaction::KernelTransaction::commit>:
    1000: b5f0          push {r4, r5, r6, r7, lr}
    1002: b084          sub.w sp, sp, #6080
00002000 <obc_storage::frame_fixture::fat::FatMedia::append_journal>:
    2000: b082          sub sp, #0x8
00003000 <unrelated::renderer::draw>:
    3000: b084          sub.w sp, sp, #40000
"""

    # A trait impl, spelled the way llvm-objdump demangles one: legacy escaping, and the paths
    # inside the brackets separated by `..` rather than `::`. That shape escapes a needle written
    # with the ordinary separator.
    TRAIT_IMPL = """
00004000 <_$LT$obc_storage..flat..store..FlatStore$LT$D$GT$$u20$as$u20$obc_storage..flat..seam..Store$GT$::commit::h1234>:
    4000: b5f0          push {r4, r5, r6, r7, lr}
    4002: b084          sub.w sp, sp, #2812
"""

    def _run(self, limit, match="frame_fixture", disassembly=None):
        args = SimpleNamespace(elf=Path("image.elf"), match=match, limit=limit)
        with mock.patch.object(
            resource_guard, "run_tool", return_value=disassembly or self.DISASSEMBLY
        ) as tool:
            resource_guard.check_frames(args)
        tool.assert_called_once_with("llvm-objdump", "--mcpu=cortex-m33", "--demangle", "-d", args.elf)

    def test_the_measured_ceiling_passes(self):
        self._run(8_192)

    def test_a_return_slot_constructor_fails_the_gate(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "above the 4096 B limit"):
            self._run(4_096)

    def test_frames_outside_the_match_are_not_gated(self):
        # The 40,000 B renderer frame is far over the limit and belongs to another module.
        self._run(8_192)

    def test_a_module_that_vanished_is_a_stale_guard_rather_than_a_pass(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "guard is stale"):
            self._run(8_192, match="absent_module")

    def test_a_scoped_needle_reaches_trait_impl_symbols(self):
        """A needle spelled as a Rust path must gate trait methods too.

        Before canonicalisation this needle matched nothing in a disassembly of only trait impls —
        the guard read as "stale" rather than as "everything passed", which is the one saving grace,
        but mixed with inherent methods (as every real ELF is) it silently passed a 2,812 B frame it
        was pointed at.
        """
        self.assertIn(
            "obc_storage::flat::seam::Store",
            resource_guard.canonical_symbol(
                "_$LT$obc_storage..flat..store..FlatStore$LT$D$GT$$u20$as$u20$"
                "obc_storage..flat..seam..Store$GT$::commit::h1234"
            ),
        )
        # It is selected, and it is gated: the frame is the one the trait method carries.
        with self.assertRaisesRegex(resource_guard.GuardError, "above the 2000 B limit"):
            self._run(2_000, match="obc_storage::flat", disassembly=self.TRAIT_IMPL)
        self._run(4_096, match="obc_storage::flat", disassembly=self.TRAIT_IMPL)

    def test_a_trait_impl_does_not_hide_behind_an_inherent_method(self):
        """The real shape: one module, one inherent frame and one trait frame, one needle.

        The trait frame is deliberately the **larger** of the two and the limit clears the inherent
        one, so this test can only pass if the needle reached the trait method: with the `..` symbol
        left un-canonicalised the needle still matches the inherent `KernelTransaction::commit`, the
        guard reports 6,080 B against an 8,192 B limit, and nothing is raised. An earlier version of
        this test used a limit below *both* frames and so passed either way — vacuous, and caught in
        review.
        """
        disassembly = self.DISASSEMBLY + self.TRAIT_IMPL.replace(
            "obc_storage..flat", "obc_storage..frame_fixture"
        ).replace("#2812", "#9000")
        with self.assertRaises(resource_guard.GuardError) as caught:
            self._run(8_192, match="obc_storage::frame_fixture", disassembly=disassembly)
        # The frame that tripped it is the trait method's, and the diagnostic names that symbol
        # rather than the inherent one it shares a module with.
        self.assertIn("9020 B", str(caught.exception))
        self.assertIn("$u20$as$u20$", str(caught.exception))
