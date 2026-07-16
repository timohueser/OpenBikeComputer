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
        with self.assertRaisesRegex(resource_guard.GuardError, "poll symbols exist"):
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


if __name__ == "__main__":
    unittest.main()
