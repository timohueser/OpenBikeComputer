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


# The shipping scratch arena (#1146 P2), as `llvm-nm --print-size --demangle` really prints it:
# 0x168a0 B of NOBITS at `__suninit`. Spelled out so a rename of the static fails these tests rather
# than silently disabling the gate that pins it.
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

    def _check_board(self, measured, baseline):
        with mock.patch.object(resource_guard, "measure_board", return_value=measured):
            resource_guard.check_board(SimpleNamespace(profile="default", elf=Path("fake")), baseline)

    def test_board_guard_explains_resident_ram_growth(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "resident RAM grew.*itemize/approve"):
            self._check_board(self._board_measured(bss=101), self._board_baseline())

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


# The #1108 boot-STKOF guards. `MAIN_TASK` is the real demangled spelling of the symbol that was
# invisible to `parse_poll_frames` — the whole reason this block exists — so a future embassy rename
# fails these tests rather than silently disabling the gate.
MAIN_TASK = (
    "obc_fw_nrf54l::____embassy_main_task::____embassy_main_task_inner_function"
    "::_$u7b$$u7b$closure$u7d$$u7d$::ha6608219ad9d4537"
)


class BootChainTests(unittest.TestCase):
    def test_task_body_parser_sees_the_symbol_the_poll_parser_misses(self):
        disassembly = f"""
00001000 <{MAIN_TASK}>:
    1000: f5ad 4d9f     sub.w sp, sp, #0x4f80
00002000 <embassy_executor::raw::TaskStorage$LT$F$GT$::poll::ha>:
    2000: f5ad 5dc3     sub.w sp, sp, #0x1860
"""
        self.assertEqual(max(resource_guard.parse_task_body_frames(disassembly).values()), 20_352)
        # The regression in one assertion: the pre-existing poll guard cannot see the main task.
        self.assertEqual(max(resource_guard.parse_poll_frames(disassembly).values()), 6_240)

    def test_task_body_parser_rejects_missing_symbols(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "no `____embassy_\\*_task` body"):
            resource_guard.parse_task_body_frames("00001000 <core::ptr::drop_in_place>:\n")

    def test_frame_parser_accepts_the_wide_subw_spelling(self):
        # `subw sp, sp, #imm` is a distinct encoding from `sub.w`; `mount_terrain` uses it.
        parsed = resource_guard.parse_disassembly(
            "00001000 <obc_fw_nrf54l::mount_terrain::hda0>:\n    1000: f6ad 0dc4  subw sp, sp, #0x8c4\n"
        )
        self.assertEqual(parsed.frames["obc_fw_nrf54l::mount_terrain::hda0"], 2_244)

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
            "uninit_max": UNINIT_BYTES,
            "framebuffer_count": 0,
            "compile_time_allocations": {"arena_total": ARENA_BYTES},
            "task_frame_limit": 21_504,
            "residual_stack_min": 48_600,
            "boot_chain_ceiling": 43_008,
            "boot_chain_headroom_min": 4_096,
            "boot_chain_roots": ["link::init_store"],
            # v3's deep-ride gate. The fixture's residual (48,600 B) clears this comfortably, so it
            # is inert for every pre-existing case and the tests below stay about what they were
            # about; `DeepRideHighWaterTests` is where it is exercised.
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
            resource_guard.check_board(SimpleNamespace(profile="default", elf=Path("fake")), baseline)

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
        # 5de00ce's real numbers: EL7's inlined ~2 KB terrain parse took the main task to 22,400 B.
        with self.assertRaisesRegex(resource_guard.GuardError, "task body is 22400 B.*#1108"):
            self._check(self._measured(task_frame=22_400), self._boot_baseline())

    def test_headroom_gate_fails_when_the_chain_does_not_fit(self):
        # Same image, the other exact symptom: chain 56,532 B against a 48,600 B stack.
        with self.assertRaisesRegex(resource_guard.GuardError, "headroom is -7932 B"):
            self._check(
                self._measured(chain_ceiling=56_532),
                self._boot_baseline(boot_chain_ceiling=60_000),
            )

    def test_residual_stack_gate_explains_statics_eating_the_stack(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "residual main stack fell to 44000"):
            self._check(self._measured(residual_stack=44_000), self._boot_baseline())

    def test_a_root_that_was_inlined_away_is_a_hard_error(self):
        # The #1084 mechanism itself: an #[inline(never)] boot constructor losing its attribute
        # moves its temporary into the caller's permanent frame.
        with self.assertRaisesRegex(resource_guard.GuardError, "inlined away"):
            self._check(
                self._measured(chain_error="boot-chain root `x` guard is stale: it was inlined away"),
                self._boot_baseline(),
            )

    def test_profiles_without_boot_roots_skip_the_boot_gates(self):
        baseline = self._boot_baseline()
        del baseline["board"]["default"]["boot_chain_roots"]
        # No BootChain measured, and no KeyError from the absent limits.
        self._check(
            resource_guard.BoardMeasurement(
                100, 20, UNINIT_BYTES, 0, (), (), None, (arena_symbol(),)
            ),
            baseline,
        )


if __name__ == "__main__":
    unittest.main()

class ModuleFrameGateTests(unittest.TestCase):
    """The `frames` subcommand: the OBC2 store's own stack ceiling (#1359).

    The regression it exists for is a measured one — a 56 KiB projection built in a return slot put
    206,080 B on the stack and HardFaulted the board — so the gate is tested against a disassembly
    that contains exactly that shape.
    """

    DISASSEMBLY = """
00001000 <obc_storage::obc2::transaction::KernelTransaction::commit>:
    1000: b5f0          push {r4, r5, r6, r7, lr}
    1002: b084          sub.w sp, sp, #6080
00002000 <obc_storage::obc2::fat::FatMedia::append_journal>:
    2000: b082          sub sp, #0x8
00003000 <unrelated::renderer::draw>:
    3000: b084          sub.w sp, sp, #40000
"""

    # A **trait impl**, spelled the way llvm-objdump actually demangles one: legacy escaping, and the
    # paths inside the `<... as ...>` brackets separated by `..` rather than `::`. This is the shape
    # that escaped the #1386 gate — `Store::commit` carried 2,812 B and a needle of
    # `obc_storage::flat` never saw it.
    TRAIT_IMPL = """
00004000 <_$LT$obc_storage..flat..store..FlatStore$LT$D$GT$$u20$as$u20$obc_storage..flat..seam..Store$GT$::commit::h1234>:
    4000: b5f0          push {r4, r5, r6, r7, lr}
    4002: b084          sub.w sp, sp, #2812
"""

    def _run(self, limit, match="obc2", disassembly=None):
        args = SimpleNamespace(elf=Path("image.elf"), match=match, limit=limit)
        with mock.patch.object(
            resource_guard, "run_tool", return_value=disassembly or self.DISASSEMBLY
        ):
            resource_guard.check_frames(args)

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
            self._run(8_192, match="obc3")

    def test_a_scoped_needle_reaches_trait_impl_symbols(self):
        """The #1386 hole: a needle spelled as a Rust path must gate trait methods too.

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
            "obc_storage..flat", "obc_storage..obc2"
        ).replace("#2812", "#9000")
        with self.assertRaises(resource_guard.GuardError) as caught:
            self._run(8_192, match="obc_storage::obc2", disassembly=disassembly)
        # The frame that tripped it is the trait method's, and the diagnostic names that symbol
        # rather than the inherent one it shares a module with.
        self.assertIn("9000 B", str(caught.exception))
        self.assertIn("$u20$as$u20$", str(caught.exception))
