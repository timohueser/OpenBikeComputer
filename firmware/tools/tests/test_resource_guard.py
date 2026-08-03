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


class ResourceGuardTests(unittest.TestCase):
    def test_size_parser_requires_contract_sections(self):
        with self.assertRaisesRegex(resource_guard.GuardError, "missing section.*\\.rodata"):
            resource_guard.parse_size_output(".vector_table 10 0\n.text 20 10\n.data 4 30\n.bss 8 34\n")

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

    def test_board_guard_explains_resident_ram_growth(self):
        measured = resource_guard.BoardMeasurement(101, 20, 0, 0, (), (), None)
        baseline = {
            "board": {
                "default": {
                    "framebuffer_bytes": 76_800,
                    "resident_ram_max": 120,
                    "uninit_max": 0,
                    "framebuffer_count": 0,
                }
            }
        }
        with mock.patch.object(resource_guard, "measure_board", return_value=measured):
            with self.assertRaisesRegex(resource_guard.GuardError, "resident RAM grew.*itemize/approve"):
                resource_guard.check_board(SimpleNamespace(profile="default", elf=Path("fake")), baseline)

    def test_board_guard_explains_missing_framebuffer_symbol(self):
        measured = resource_guard.BoardMeasurement(100, 20, 0, 0, (), (), None)
        baseline = {
            "board": {
                "default": {
                    "framebuffer_bytes": 76_800,
                    "resident_ram_max": 120,
                    "uninit_max": 0,
                    "framebuffer_count": 1,
                }
            }
        }
        with mock.patch.object(resource_guard, "measure_board", return_value=measured):
            with self.assertRaisesRegex(resource_guard.GuardError, "framebuffer symbol count is 0"):
                resource_guard.check_board(SimpleNamespace(profile="default", elf=Path("fake")), baseline)


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
            "uninit_max": 0,
            "framebuffer_count": 0,
            "task_frame_limit": 21_504,
            "residual_stack_min": 48_600,
            "boot_chain_ceiling": 43_008,
            "boot_chain_headroom_min": 4_096,
            "boot_chain_roots": ["link::init_store"],
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
            100, 20, 0, 0, (), (), None, resource_guard.BootChain(**boot)
        )

    def _check(self, measured, baseline):
        with mock.patch.object(resource_guard, "measure_board", return_value=measured):
            resource_guard.check_board(SimpleNamespace(profile="default", elf=Path("fake")), baseline)

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
        self._check(resource_guard.BoardMeasurement(100, 20, 0, 0, (), (), None), baseline)


if __name__ == "__main__":
    unittest.main()
