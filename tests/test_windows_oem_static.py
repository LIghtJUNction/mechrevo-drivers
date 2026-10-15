"""Synthetic-format regressions. No vendor binary or vendor key is embedded."""
import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import types

SPEC = importlib.util.spec_from_file_location("windows_oem_static", Path(__file__).resolve().parents[1] / "scripts/windows_oem_static.py")
m = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(m)


def fixture(count=2):
    offset = 94 + count * 36
    b = bytearray(offset)
    struct.pack_into("<IHHHHHIQ", b, 0, offset, 0x8664, count, 240, 0x2022, 0x20b, 0x1000, 0x140000000)
    struct.pack_into("<I", b, 34, (count + 1)*4096)
    struct.pack_into("<HH", b, 42, 3, 0x140)
    for i in range(count):
        data = bytes(range(i*16, i*16+16))
        key = bytes([i+1])*16
        struct.pack_into("<5I", b, 94+i*36, (i+1)*4096, 32, len(data), len(b), 0x60000020)
        b[114+i*36:130+i*36] = key
        b.extend(m.rc4(key, data))
    return bytes(b)


class StaticTests(unittest.TestCase):
    def test_rfc6229_key1_40bit_offsets(self):
        # RFC 6229 section 2: https://www.rfc-editor.org/rfc/rfc6229.html#section-2
        stream = m.rc4(bytes.fromhex("0102030405"), bytes(272))
        self.assertEqual(stream[:16].hex(), "b2396305f03dc027ccc3524a0a1118a8")
        self.assertEqual(stream[256:272].hex(), "1cfcf62b03eddb641d77dfcf7f8d8c93")

    def test_empty_or_oversized_key(self):
        for key in (b"", bytes(257)):
            with self.assertRaises(m.FormatError): m.rc4(key, b"x")

    def test_rebuild_section_bytes_and_synthetic_fields(self):
        analysis, sections = m.rebuild(fixture())
        pe = m.dependency().PE(data=analysis)
        self.assertEqual(len(pe.sections), 2)
        for i, section in enumerate(pe.sections):
            self.assertEqual(section.get_data()[:16], bytes(range(i*16, i*16+16)))
            self.assertEqual(sections[i]["sha256"], m.digest(bytes(range(i*16, i*16+16))))
            self.assertEqual(section.Name, f".s{i}".encode().ljust(8, b"\0"))
        self.assertTrue(all(not d.VirtualAddress and not d.Size for d in pe.OPTIONAL_HEADER.DATA_DIRECTORY[6:]))
        pe.close()

    def test_compact_truncation(self):
        for size in (0, 93, 129, len(fixture())-1):
            with self.subTest(size=size), self.assertRaises(m.FormatError): m.rebuild(fixture()[:size])

    def test_bad_offsets_sizes_and_overlap(self):
        changes = [(0, 0), (34, 0xFFFFFFFF), (94, 0xFFFFFFF8), (94+12, 2),
                   (94+8, 0xFFFFFFFF), (130, 4096), (130+12, 166), (14, 0x9999)]
        for offset, value in changes:
            b = bytearray(fixture()); struct.pack_into("<I", b, offset, value)
            with self.subTest(offset=offset), self.assertRaises(m.FormatError): m.rebuild(bytes(b))

    def test_directories_outside_mapping_and_security_offset(self):
        for index, rva, n in ((0, 0x9999, 8), (0, 4096, 0), (4, 4096, 8)):
            b=bytearray(fixture()); struct.pack_into("<II", b, 46+index*8, rva, n)
            with self.subTest(index=index), self.assertRaises(m.FormatError): m.rebuild(bytes(b))

    def test_rva_mapping_exact_length_and_ambiguity(self):
        s=types.SimpleNamespace(PointerToRawData=2, SizeOfRawData=4, VirtualAddress=4096)
        pe=types.SimpleNamespace(sections=[s])
        self.assertEqual(m.read_rva(b"abcdef", pe, 4096, 4), b"cdef")
        for source, sections, rva, n in ((b"abc", [s],4096,4), (b"abcdef",[s],4098,3), (b"abcdef",[s,s],4096,1)):
            with self.assertRaises(m.FormatError): m.read_rva(source, types.SimpleNamespace(sections=sections),rva,n)

    def test_payload_cap_and_truncation(self):
        for n in (0, m.MAX_COMPACT, 12):
            source=struct.pack("<I",n)+bytes(20)
            section=types.SimpleNamespace(PointerToRawData=0,SizeOfRawData=len(source),VirtualAddress=4096)
            with self.assertRaises(m.FormatError): m.extract_compact(source,types.SimpleNamespace(sections=[section]),4096)

    def test_bad_hash_creates_no_output(self):
        with tempfile.TemporaryDirectory() as directory:
            source=Path(directory)/"source"; source.write_bytes(b"not qualified")
            output=Path(directory)/"output"
            with self.assertRaisesRegex(m.FormatError,"SHA256"): m.run(source,output)
            self.assertFalse(output.exists())

    def synthetic_source(self):
        compact=fixture(); analysis,_=m.rebuild(compact)
        b=bytearray(analysis)
        encrypted=struct.pack("<I",len(compact))+bytes(4)+bytes(range(16))+m.rc4(bytes(range(16)),compact)
        raw=len(b); b.extend(encrypted)
        # Use the existing first section as one larger file-backed data region.
        struct.pack_into("<I",b,328+16,len(b)-512)
        struct.pack_into("<I",b,328+20,512)
        struct.pack_into("<I",b,328+8,len(b)-512)
        # Remove second section and adjust section count, avoiding original source overlap.
        struct.pack_into("<H",b,70,1)
        return bytes(b),4096+raw-512

    def test_new_output_receipt_and_refusal_to_overwrite(self):
        source,rva=self.synthetic_source()
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/"source"; path.write_bytes(source)
            output=Path(directory)/"output"
            with patch.object(m,"SOURCE_SHA256",m.digest(source)), patch.object(m,"PAYLOAD_RVAS",(rva,)):
                result=m.run(path,output)
                self.assertTrue(result["complete"])
                self.assertEqual(output.stat().st_mode & 0o777, 0o700)
                self.assertEqual(result["images"][0]["compact_sha256"],m.digest(fixture()))
                before=(output/"manifest.json").read_bytes()
                with self.assertRaises(FileExistsError): m.run(path,output)
                self.assertEqual((output/"manifest.json").read_bytes(),before)

    def test_existing_file_and_symlink_output_refused(self):
        source, rva = self.synthetic_source()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "source"; path.write_bytes(source)
            existing = Path(directory) / "existing"; existing.write_bytes(b"preserved")
            link = Path(directory) / "link"; link.symlink_to(existing)
            with patch.object(m, "SOURCE_SHA256", m.digest(source)), patch.object(m, "PAYLOAD_RVAS", (rva,)):
                for output in (existing, link):
                    with self.subTest(output=output), self.assertRaises(FileExistsError): m.run(path, output)
            self.assertEqual(existing.read_bytes(), b"preserved")
            self.assertTrue(link.is_symlink())

    def test_original_pe_raw_overlap_refused_before_output(self):
        source, rva = self.synthetic_source()
        b = bytearray(source)
        struct.pack_into("<H", b, 70, 2)
        # Restore second section header already present: its raw bytes now overlap first.
        source = bytes(b)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "source"; path.write_bytes(source)
            output = Path(directory) / "output"
            with patch.object(m, "SOURCE_SHA256", m.digest(source)), patch.object(m, "PAYLOAD_RVAS", (rva,)):
                with self.assertRaisesRegex(m.FormatError, "overlapping"): m.run(path, output)
            self.assertFalse(output.exists())

    def test_write_failure_does_not_claim_complete(self):
        source,rva=self.synthetic_source()
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/"source"; path.write_bytes(source)
            output=Path(directory)/"output"
            original=Path.open
            def failing(path,*args,**kwargs):
                if path.name.endswith("analysis.exe"): raise OSError("injected write failure")
                return original(path,*args,**kwargs)
            with patch.object(m,"SOURCE_SHA256",m.digest(source)), patch.object(m,"PAYLOAD_RVAS",(rva,)), patch.object(Path,"open",failing):
                with self.assertRaises(OSError): m.run(path,output)
            import json
            self.assertFalse(json.loads((output/"manifest.json").read_text())["complete"])

    def test_missing_dependency_message(self):
        import sys
        with patch.dict(sys.modules,{"pefile":None}):
            with self.assertRaisesRegex(m.FormatError,"pefile is required"): m.dependency()


class MethodRecoveryTests(unittest.TestCase):
    def body(self, fat=False):
        if fat:
            return struct.pack("<HHII", 0x3013, 8, 1, 0) + b"\x2a" + bytes(3) + b"VBPD"
        return b"\x06\x2aVBPD"

    def fixture(self, duplicate=False):
        plain = self.body()
        key = bytes(range(16))  # synthetic test data only
        source = bytearray(4096 + len(plain))
        source[:2] = b"\x06\x2a"
        source[2:6] = b"FOAP"
        struct.pack_into("<i", source, 6, 32-10)
        source[32:36] = b"FWSN"
        struct.pack_into("<I", source, 36, 2 if duplicate else 1)
        for i in range(2 if duplicate else 1):
            q = 4096 + 40 + 24*i
            struct.pack_into("<IiIIII", source, 40+24*i, 0x06000001, 8192-q-8, 8192, len(plain), 0, 0)
        source[4096:] = m.rc4(key, plain)
        section = types.SimpleNamespace(PointerToRawData=0, SizeOfRawData=len(source), VirtualAddress=4096)
        return source, types.SimpleNamespace(sections=[section]), key

    def recover(self, source, pe, key, source_map=None):
        if source_map is None:
            source_map = {0x06000001:dict(rid=1, methoddef_row_rva="0x111", original_rva="0x222")}
        with patch.object(m, "JIT_STUB_RVA", 4096), patch.object(m, "native_method_key", return_value=(key, [])):
            return m.recover_methods(bytes(source), pe, b"synthetic", source_map)

    def test_tiny_and_fat_body_header(self):
        for fat in (False, True):
            details = m.body_header(self.body(fat))
            self.assertEqual(details["header_kind"], "fat" if fat else "tiny")
            self.assertEqual(details["code_size"], 1)

    def test_body_marker_header_codesize_tail_and_eh(self):
        bad = [b"\x06\x2aBAD!", b"\x00VBPD", b"\x03VBPD", b"\xfe\x2aVBPD",
               self.body()[:-4]+b"extraVBPD"]
        fat = bytearray(self.body(True)); struct.pack_into("<I", fat, 4, 100); bad.append(bytes(fat))
        fat = bytearray(self.body(True)); struct.pack_into("<H", fat, 0, 0x4013); bad.append(bytes(fat))
        fat = bytearray(self.body(True)); struct.pack_into("<H", fat, 0, 0x301b); bad.append(bytes(fat))
        for body in bad:
            with self.subTest(body=body.hex()), self.assertRaises(m.FormatError): m.body_header(body)

    def test_fwsn_source_map_and_roundtrip(self):
        source, pe, key = self.fixture()
        methods, origin = self.recover(source, pe, key)
        self.assertEqual(methods[0][0], self.body())
        self.assertEqual(methods[0][1]["rid"], 1)
        self.assertEqual(origin["table_records"], 1)
        self.assertEqual(methods[0][1]["encrypted_sha256"], m.digest(bytes(source[4096:])))

    def test_bad_fwsn_tokens_duplicates_offsets_cipher_and_marker(self):
        for mode in ("token", "duplicate", "relative", "fallback", "size", "cipher", "table", "stub", "unmapped"):
            source, pe, key = self.fixture(mode == "duplicate")
            source_map = None
            if mode == "token": struct.pack_into("<I", source, 40, 0x02000001)
            if mode == "relative": struct.pack_into("<i", source, 44, -9000)
            if mode == "fallback": struct.pack_into("<I", source, 48, 0xFFFFFFFF)
            if mode == "size": struct.pack_into("<I", source, 52, 1000000)
            if mode == "cipher": source[-1] ^= 1
            if mode == "table": source[32] ^= 1
            if mode == "stub": source[2] ^= 1
            if mode == "unmapped": source_map = {}
            with self.subTest(mode=mode), self.assertRaises(m.FormatError): self.recover(source, pe, key, source_map)

    def test_native_key_is_read_from_instruction_immediates(self):
        analysis, _ = m.rebuild(fixture(1))
        image = bytearray(analysis)
        image.extend(bytes(512+4096-len(image)))
        struct.pack_into("<I", image, 328+8, 4096)
        struct.pack_into("<I", image, 328+16, 4096)
        expected = bytes(range(16))
        for i, (rva, displacement) in enumerate(((0x1dd2,0x1b0),(0x1de2,0x1b4),(0x1df0,0x1b8),(0x1dfa,0x1bc))):
            pos=512+rva-4096
            image[pos:pos+10] = b"\xc7\x85"+struct.pack("<I",displacement)+expected[i*4:i*4+4]
        key, provenance = m.native_method_key(bytes(image))
        self.assertEqual(key, expected)
        self.assertEqual(len(provenance), 4)
        image[512+0x1dd2-4096] ^= 1
        with self.assertRaises(m.FormatError): m.native_method_key(bytes(image))

    def test_metadata_methoddef_rid_mapping_and_row_truncation(self):
        metadata = bytearray(106)
        metadata[:4] = b"BSJB"
        struct.pack_into("<I", metadata, 12, 4)
        metadata[16:20] = b"v4\0\0"
        struct.pack_into("<HH", metadata, 20, 0, 1)
        struct.pack_into("<II", metadata, 24, 64, 42)
        metadata[32:35] = b"#~\0"
        struct.pack_into("<Q", metadata, 64+8, 1 << 6)
        struct.pack_into("<I", metadata, 64+24, 1)
        struct.pack_into("<I", metadata, 64+28, 0x2345)
        source = bytearray(32) + metadata
        struct.pack_into("<II", source, 8, 4128, len(metadata))
        section = types.SimpleNamespace(PointerToRawData=0, SizeOfRawData=len(source), VirtualAddress=4096)
        directories = [types.SimpleNamespace(VirtualAddress=0) for _ in range(15)]
        directories[14].VirtualAddress = 4096
        pe = types.SimpleNamespace(sections=[section], OPTIONAL_HEADER=types.SimpleNamespace(DATA_DIRECTORY=directories))
        mapping = m.method_metadata(bytes(source), pe)
        self.assertEqual(mapping, {0x06000001:dict(rid=1, methoddef_row_rva=hex(4128+92), original_rva="0x2345")})
        struct.pack_into("<I", source, 32+64+24, 2)
        with self.assertRaisesRegex(m.FormatError, "MethodDef rows"): m.method_metadata(bytes(source), pe)

    def test_method_write_failure_preserves_incomplete_manifest(self):
        source, rva = StaticTests().synthetic_source()
        record = dict(token="0x6000001", recovered_sha256=m.digest(self.body()))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/"source"; path.write_bytes(source)
            output = Path(directory)/"output"
            original = Path.open
            def failing(path,*args,**kwargs):
                if path.parent.name == "methods": raise OSError("method write failure")
                return original(path,*args,**kwargs)
            with patch.object(m,"SOURCE_SHA256",m.digest(source)), patch.object(m,"PAYLOAD_RVAS",(rva,)), patch.object(m,"method_metadata",return_value={}), patch.object(m,"recover_methods",return_value=([(self.body(),record)],{})), patch.object(Path,"open",failing):
                with self.assertRaises(OSError): m.run(path,output,recover=True)
            import json
            manifest=json.loads((output/"manifest.json").read_text())
            self.assertFalse(manifest["complete"])
            self.assertFalse(manifest["method_recovery"]["complete"])


if __name__ == "__main__": unittest.main()
