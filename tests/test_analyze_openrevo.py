"""Synthetic malformed PE tests; frozen vendor acceptance is run separately."""
from __future__ import annotations

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "analyze_openrevo.py"
spec = importlib.util.spec_from_file_location("analyze_openrevo", SCRIPT)
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)


def fixture() -> bytearray:
    """One x64 section, named and ordinal imports, with unrelated overlay bytes."""
    data = bytearray(0x800)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 0x80)
    data[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<HH", data, 0x84, 0x8664, 1)
    struct.pack_into("<H", data, 0x94, 0xF0)
    struct.pack_into("<H", data, 0x98, 0x20B)
    struct.pack_into("<Q", data, 0xB0, 0x140000000)
    struct.pack_into("<I", data, 0xD4, 0x200)
    struct.pack_into("<I", data, 0x104, 16)
    struct.pack_into("<II", data, 0x110, 0x1000, 40)
    data[0x188:0x190] = b".rdata\0\0"
    struct.pack_into("<IIII", data, 0x190, 0x500, 0x1000, 0x400, 0x200)
    struct.pack_into("<IIIII", data, 0x200, 0x1100, 0, 0, 0x1080, 0x1120)
    data[0x280:0x28E] = b"SYNTHETIC.dll\0"
    for offset in (0x300, 0x320):
        struct.pack_into("<QQQ", data, offset, 0x1180, (1 << 63) | 7, 0)
    struct.pack_into("<H", data, 0x380, 17)
    data[0x382:0x390] = b"SyntheticCall\0"
    return data


def add_second_section(data: bytearray, rva: int, raw: int) -> None:
    struct.pack_into("<H", data, 0x86, 2)
    data[0x1B0:0x1B8] = b".second\0"
    struct.pack_into("<IIII", data, 0x1B8, 0x100, rva, 0x100, raw)


class PEReaderTests(unittest.TestCase):
    def test_successful_named_ordinal_and_iat_fallback(self):
        for fallback in (False, True):
            with self.subTest(fallback=fallback):
                data = fixture()
                if fallback:
                    struct.pack_into("<I", data, 0x200, 0)
                imports = module.PEImage(bytes(data)).imports()
                self.assertEqual(len(imports), 1)
                self.assertEqual(imports[0]["dll"], "SYNTHETIC.dll")
                self.assertEqual(imports[0]["name_offset"], "0x280")
                self.assertEqual(imports[0]["symbols"][0]["name"], "SyntheticCall")
                self.assertEqual(imports[0]["symbols"][0]["name_offset"], "0x382")
                self.assertEqual(imports[0]["symbols"][1]["ordinal"], 7)
                self.assertEqual(imports[0]["symbols"][0]["thunk_offset"], "0x320" if fallback else "0x300")

    def test_truncated_header_stages(self):
        original = fixture()
        for cut in (0, 2, 63, 0x81, 0x90, 0x97, 0x100, 0x187, 0x1AF):
            with self.subTest(cut=cut), self.assertRaises(module.FormatError):
                module.PEImage(bytes(original[:cut]))

    def test_invalid_entry_magic_and_offsets(self):
        for offset, value in ((0, b"NO"), (0x80, b"PX\0\0"), (0x84, b"\x4c\x01"), (0x98, b"\x0b\x01")):
            with self.subTest(offset=offset):
                data = fixture(); data[offset:offset + len(value)] = value
                with self.assertRaises(module.FormatError): module.PEImage(bytes(data))
        for value in (0, 63, 0x7FF, 0xFFFFFFFF):
            data = fixture(); struct.pack_into("<I", data, 0x3C, value)
            with self.subTest(pe_offset=value), self.assertRaises(module.FormatError): module.PEImage(bytes(data))

    def test_optional_and_section_header_limits(self):
        for field, value, fmt in ((0x94, 100, "H"), (0x104, 17, "I"), (0x86, 0, "H"), (0x86, 97, "H"), (0xD4, 0x180, "I")):
            data = fixture(); struct.pack_into("<" + fmt, data, field, value)
            with self.subTest(field=field, value=value), self.assertRaises(module.FormatError): module.PEImage(bytes(data))
        data = fixture(); struct.pack_into("<H", data, 0x94, 112)
        with self.assertRaises(module.FormatError): module.PEImage(bytes(data))

    def test_raw_bounds_header_overlap_and_rva_overflow(self):
        for offset, value in ((0x198, 0x601), (0x19C, 0x100), (0x194, 0xFFFFFF00), (0x194, 0x100)):
            data = fixture(); struct.pack_into("<I", data, offset, value)
            with self.subTest(offset=offset, value=value), self.assertRaises(module.FormatError): module.PEImage(bytes(data))

    def test_ambiguous_raw_and_virtual_sections_rejected(self):
        for rva, raw in ((0x1200, 0x600), (0x2000, 0x400)):
            data = fixture(); add_second_section(data, rva, raw)
            with self.subTest(rva=rva, raw=raw), self.assertRaises(module.FormatError): module.PEImage(bytes(data))

    def test_rva_file_backed_boundary(self):
        image = module.PEImage(bytes(fixture()))
        self.assertEqual(image.mapped_range(0x1000), (0x200, 0x600))
        self.assertEqual(image.mapped_range(0x13FF), (0x5FF, 0x600))
        for rva, size in ((0x13FF, 2), (0x1400, 1), (0x1FF, 2), (0xFFFFFFFF, 2), (-1, 1), (0x1000, 0)):
            with self.subTest(rva=rva, size=size), self.assertRaises(module.FormatError): image.mapped_range(rva, size)

    def test_import_directory_must_contain_terminator_and_complete_range(self):
        for rva, length in ((0, 40), (0x1000, 0), (0x1000, 19), (0x1000, 20), (0x13F0, 40), (0x1600, 40)):
            data = fixture(); struct.pack_into("<II", data, 0x110, rva, length)
            with self.subTest(rva=rva, length=length), self.assertRaises(module.FormatError): module.PEImage(bytes(data)).imports()
        data = fixture(); data[0x214:0x228] = data[0x200:0x214]
        with self.assertRaises(module.FormatError): module.PEImage(bytes(data)).imports()

    def test_no_import_directory_is_valid(self):
        data = fixture(); struct.pack_into("<II", data, 0x110, 0, 0)
        self.assertEqual(module.PEImage(bytes(data)).imports(), [])

    def test_import_name_and_ordinal_pointer_bounds(self):
        for value in (0x100001180, 0x13FF, (1 << 63) | (1 << 32) | 7):
            data = fixture(); struct.pack_into("<Q", data, 0x300, value)
            with self.subTest(thunk=value), self.assertRaises(module.FormatError): module.PEImage(bytes(data)).imports()
        data = fixture(); struct.pack_into("<II", data, 0x200, 0, 0); struct.pack_into("<I", data, 0x210, 0)
        with self.assertRaises(module.FormatError): module.PEImage(bytes(data)).imports()

    def test_missing_dll_pointer_and_missing_iat_rejected(self):
        for offset in (0x20C, 0x210):
            data = fixture(); struct.pack_into("<I", data, offset, 0)
            with self.subTest(offset=offset), self.assertRaises(module.FormatError):
                module.PEImage(bytes(data)).imports()

    def test_dll_string_cannot_consume_nul_from_overlay(self):
        data = fixture(); struct.pack_into("<I", data, 0x20C, 0x13F0); data[0x5F0:0x600] = b"A" * 16
        self.assertEqual(data[0x600], 0)
        with self.assertRaises(module.FormatError): module.PEImage(bytes(data)).imports()

    def test_thunk_cannot_terminate_in_an_adjacent_section(self):
        data = fixture(); struct.pack_into("<I", data, 0x190, 0x400); add_second_section(data, 0x1400, 0x600)
        struct.pack_into("<I", data, 0x200, 0x13F8); struct.pack_into("<Q", data, 0x5F8, (1 << 63) | 9)
        with self.assertRaises(module.FormatError): module.PEImage(bytes(data)).imports()

    def test_thunk_name_cannot_put_hint_and_name_in_different_sections(self):
        data = fixture(); struct.pack_into("<I", data, 0x190, 0x400); add_second_section(data, 0x1400, 0x600)
        struct.pack_into("<Q", data, 0x300, 0x13FE); data[0x600:0x604] = b"Bad\0"
        with self.assertRaises(module.FormatError): module.PEImage(bytes(data)).imports()

    def test_resource_budgets_reject_incomplete_inventory(self):
        with patch.object(module, "MAX_IMPORT_SYMBOLS", 1), self.assertRaises(module.FormatError): module.PEImage(bytes(fixture())).imports()
        with patch.object(module, "MAX_IMPORT_DESCRIPTORS", 1), self.assertRaises(module.FormatError): module.PEImage(bytes(fixture())).imports()
        with patch.object(module, "MAX_NAME_BYTES", 3), self.assertRaises(module.FormatError): module.PEImage(bytes(fixture())).imports()

    def test_asset_marker_inventory_never_exports_payload(self):
        payload = b"PRIVATE_VENDOR_BODY_DO_NOT_EXPORT"
        data = b"/assets/a-test.js\0" + payload + b"/assets/a-test.js\0/assets/b.css\0"
        inventory = module.asset_inventory(data)
        self.assertEqual(inventory[0]["offsets"], ["0x0", hex(18 + len(payload))])
        self.assertFalse(inventory[0]["payload_decoded"])
        self.assertNotIn(payload.decode(), json.dumps(inventory))

    def test_asset_extensions_cannot_match_prefixes_of_other_files(self):
        for candidate in (b"/assets/not-json.json", b"/assets/index.css.map", b"/assets/index.jsBAD"):
            with self.subTest(candidate=candidate):
                self.assertEqual(module.asset_inventory(candidate), [])


class OutputTests(unittest.TestCase):
    def run_main(self, data: bytes, output: Path, patch_identity: bool = True) -> int:
        source = output.parent / "input.exe"; source.write_bytes(data)
        with contextlib.ExitStack() as stack:
            stack.enter_context(contextlib.redirect_stderr(io.StringIO()))
            if patch_identity:
                stack.enter_context(patch.object(module, "EXPECTED_SIZE", len(data)))
                stack.enter_context(patch.object(module, "EXPECTED_SHA256", module.digest(data)))
            return module.main([str(source), "--output", str(output)])

    def test_cli_success_synthetic_identity_is_separate_from_frozen_acceptance(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "inventory.json"
            self.assertEqual(self.run_main(bytes(fixture()), output), 0)
            report = json.loads(output.read_text())
            self.assertFalse(report["subject"]["executed"])
            self.assertEqual(report["pe"]["imports"][0]["symbols"][0]["name"], "SyntheticCall")
            self.assertEqual(report["limits"]["hardware_behavior"], "unknown")

    def test_rejects_wrong_magic_size_and_digest_without_output(self):
        data = bytes(fixture())
        cases = ((b"NO" + data[2:], True), (data[:-1], False), (data, False))
        with tempfile.TemporaryDirectory() as temp:
            for index, (candidate, patch_identity) in enumerate(cases):
                output = Path(temp) / f"{index}.json"
                with self.subTest(index=index):
                    self.assertEqual(self.run_main(candidate, output, patch_identity), 1)
                    self.assertFalse(output.exists())
            with patch.object(module, "EXPECTED_SIZE", len(data)):
                output = Path(temp) / "digest.json"
                self.assertEqual(self.run_main(data, output, False), 1)
                self.assertFalse(output.exists())

    def test_identity_match_cannot_bypass_malformed_pe(self):
        data = bytearray(fixture()); struct.pack_into("<I", data, 0x3C, 0xFFFFFFFF)
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "out.json"
            self.assertEqual(self.run_main(bytes(data), output), 1)
            self.assertFalse(output.exists())

    def test_existing_file_directory_and_symlinks_are_never_overwritten(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); target = root / "preserve"; target.write_text("KEEP")
            directory = root / "dir"; directory.mkdir()
            link = root / "link"; link.symlink_to(target)
            dangling = root / "dangling"; dangling.symlink_to(root / "missing")
            for output in (target, directory, link, dangling):
                with self.subTest(output=output.name): self.assertEqual(self.run_main(bytes(fixture()), output), 1)
            self.assertEqual(target.read_text(), "KEEP")
            self.assertTrue(directory.is_dir()); self.assertTrue(link.is_symlink()); self.assertTrue(dangling.is_symlink())

    def test_publish_failure_leaves_no_partial_destination(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "result.json"
            with patch.object(module.os, "link", side_effect=OSError("injected publication failure")):
                self.assertEqual(self.run_main(bytes(fixture()), output), 1)
            self.assertFalse(output.exists())
            self.assertEqual(sorted(p.name for p in Path(temp).iterdir()), ["input.exe"])

    def test_concurrent_destination_is_preserved_on_publish_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "result.json"
            def concurrent_writer(source, destination):
                Path(destination).write_text("CONCURRENT")
                raise FileExistsError("destination appeared")
            with patch.object(module.os, "link", side_effect=concurrent_writer):
                self.assertEqual(self.run_main(bytes(fixture()), output), 1)
            self.assertEqual(output.read_text(), "CONCURRENT")
            self.assertFalse(any(p.is_dir() for p in Path(temp).iterdir()))


if __name__ == "__main__":
    unittest.main()
