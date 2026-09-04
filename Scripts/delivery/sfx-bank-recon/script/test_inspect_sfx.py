"""Offline regression tests; no game process or proprietary fixture is required."""

import struct
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import capstone

from inspect_sfx import Binary, layout


def synthetic_binary(code):
    binary = Binary.__new__(Binary)
    binary.text = SimpleNamespace(VirtualAddress=0x1000)
    binary.code = code
    binary.functions = [(0x1000, 0x1000 + len(code))]
    binary.starts = [0x1000]
    binary.pe = SimpleNamespace(get_data=lambda start, size: code[start - 0x1000:start - 0x1000 + size])
    binary.md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    binary.md.detail = True
    binary.cache = {}
    return binary


class InspectionTests(unittest.TestCase):
    def test_bounds_are_half_open(self):
        binary = synthetic_binary(b"\x90\xc3")
        self.assertIsNone(binary.bounds(0xfff))
        self.assertEqual(binary.bounds(0x1001), (0x1000, 0x1002))
        self.assertIsNone(binary.bounds(0x1002))

    def test_relative_call(self):
        binary = synthetic_binary(b"\xe8\x20\x00\x00\x00\xc3")
        refs = binary.references({0x1025}, calls=True)
        self.assertEqual([r["rva"] for r in refs], ["0x1000"])

    def test_embedded_opcode_is_not_a_call(self):
        binary = synthetic_binary(b"\xb8\xe8\x00\x00\x00\x00\xc3")
        self.assertEqual(binary.references({0x1006}, calls=True), [])

    def test_rip_relative_address(self):
        binary = synthetic_binary(b"\x48\x8d\x05\x20\x00\x00\x00\xc3")
        refs = binary.references({0x1027})
        self.assertEqual([r["target"] for r in refs], ["0x1027"])

    def test_field_scan_requires_word_operand(self):
        binary = synthetic_binary(bytes.fromhex("0fbf8162020000 8b8162020000 c3"))
        self.assertEqual([r["rva"] for r in binary.field_reads(0x262)], ["0x1000"])

    def test_full_exception_directory_is_used(self):
        entries = 12000
        pdata = b"".join(struct.pack("<III", 0x1000 + i * 2, 0x1002 + i * 2, 0)
                         for i in range(entries))
        section = SimpleNamespace(Name=b".text\0", get_data=lambda: b"\xc3")
        directory = SimpleNamespace(VirtualAddress=0x8000, Size=len(pdata))
        pe = SimpleNamespace(
            sections=[section],
            OPTIONAL_HEADER=SimpleNamespace(ImageBase=0x140000000, DATA_DIRECTORY=[None] * 3 + [directory]),
            get_data=lambda start, size: pdata,
            DIRECTORY_ENTRY_EXCEPTION=[],
        )
        with patch.object(Path, "read_bytes", return_value=b"fixture"), patch("inspect_sfx.pefile.PE", return_value=pe):
            binary = Binary("fixture.exe")
        self.assertEqual(len(binary.functions), entries)
        self.assertIsNotNone(binary.bounds(0x1000 + (entries - 1) * 2))

    def test_primitive_c_layout_and_tail_alignment(self):
        with tempfile.TemporaryDirectory() as folder:
            source = Path(folder) / "generated.rs"
            source.write_text(
                "#[repr(C)]\npub struct NPC_PARAM_ST {\n"
                "    prefix: u8,\n    values: [u32; 2],\n"
                "    sfx_res_bank_id: i16,\n}\n", encoding="utf-8")
            result = layout(source, "sfx_res_bank_id")
            self.assertEqual(result["offset"], "0xc")
            self.assertEqual(result["total_size"], 16)
            with self.assertRaises(ValueError):
                layout(source, "missing")


if __name__ == "__main__":
    unittest.main()
