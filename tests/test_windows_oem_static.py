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


class DynamicTests(unittest.TestCase):
    def wrap_code(self, code):
        body=struct.pack("<HHII",0x3013,8,len(code),0)+code
        return body+bytes((-len(body))%4)+b"VBPD"

    def wrapper(self, desc):
        # Synthetic constant branch skips two unsupported bytes; no vendor instructions.
        code=b"\x16\x2c\x02\xff\xff\x14\x28"+struct.pack("<I",0x0a0001dc)
        for value in (desc['source_rva'],desc['length'],desc['external_local_rva'],desc['external_local_length'],desc['fallback_local_rva'],4128,desc['builder_discriminator']):
            code+=b"\x20"+struct.pack("<i",value)
        offset=len(code)
        code+=b"\x28"+struct.pack("<I",0x0600182f)+b"\x2a"
        desc['wrapper_call_offset']=hex(offset)
        body=self.wrap_code(code)
        desc['wrapper_sha256']=m.digest(body)
        return body

    def fixture(self):
        stored = bytes(range(32))
        fixed = bytes(((v ^ 0x3c)-0xc3)&255 for v in stored)
        encrypted_key = bytes(range(16))
        key = m.rc4(fixed, encrypted_key)
        source = bytearray(1024)
        source[:32], source[32:48] = stored, encrypted_key
        methods, descriptors = [], []
        for i in range(6):
            token = 0x06000001+i
            wrapper = b"\x06\x2aVBPD"
            body = b"\x06\x2a"
            rva = 4096+200+i*16
            cipher = m.rc4(key, body)
            source[rva-4096:rva-4096+len(cipher)] = cipher
            methods.append((wrapper,dict(token=hex(token),record_rva=hex(4096+64+i*24),source_rva="0x1000")))
            descriptors.append(dict(token=token,wrapper_sha256=m.digest(wrapper),wrapper_call_offset="0x10",source_rva=rva,length=len(cipher),cipher_sha256=m.digest(cipher),body_sha256=m.digest(body),external_local_rva=0,external_local_length=0,external_local_sha256=None,fallback_local_rva=0,builder_discriminator=i))
        methods=[(self.wrapper(d), methods[i][1]) for i,d in enumerate(descriptors)]
        section = types.SimpleNamespace(PointerToRawData=0,SizeOfRawData=len(source),VirtualAddress=4096)
        return source, types.SimpleNamespace(sections=[section]), methods, descriptors

    def recover(self, source, pe, methods, descriptors, qualified_hash=None):
        import json
        if qualified_hash is None:
            qualified_hash=m.digest(json.dumps(descriptors,sort_keys=True,separators=(",",":")).encode())
        with patch.object(m,"DYNAMIC_DESCRIPTOR_SHA256",qualified_hash),patch.object(m,"DYNAMIC_DESCRIPTORS",descriptors), patch.object(m,"DYNAMIC_KEY_FIELD_RVA",4096), patch.object(m,"DYNAMIC_LAYER_KEY_RVA",4128), patch.object(m,"DYNAMIC_LAYER_CIPHER_SHA256",m.digest(bytes(range(16)))):
            return m.recover_dynamic_methods(bytes(source),pe,methods)

    def test_synthetic_six_body_recovery_and_no_key_bytes_receipt(self):
        source, pe, methods, descriptors = self.fixture()
        result, origin = self.recover(source,pe,methods,descriptors)
        self.assertEqual(len(result),6)
        self.assertTrue(all(x[1] == b"\x06\x2a" for x in result))
        self.assertNotIn("layer_key_sha256",origin)
        self.assertNotIn("key_bytes",origin)
        self.assertEqual(origin["stored_fixed_key_size"],32)

    def test_key_cipher_wrapper_offsets_and_caps_rejected(self):
        for mode in ("storedkey","encryptedkey","cipher","wrapper","overlap","cap","duplicate","external"):
            source, pe, methods, descriptors = self.fixture()
            if mode == "storedkey": source[0] ^= 1
            if mode == "encryptedkey": source[32] ^= 1
            if mode == "cipher": source[200] ^= 1
            if mode == "wrapper": methods[0] = (b"wrong",methods[0][1])
            if mode == "overlap": descriptors[1]['source_rva']=descriptors[0]['source_rva']
            if mode == "cap": descriptors[0]['length']=m.MAX_COMPACT
            if mode == "duplicate": descriptors[1]['token']=descriptors[0]['token']
            if mode == "external": descriptors[0]['external_local_rva']=4900
            with self.subTest(mode=mode),self.assertRaises(m.FormatError):self.recover(source,pe,methods,descriptors)

    def test_frozen_descriptor_argument_corruption_rejected(self):
        import json
        for field in ("wrapper_call_offset","builder_discriminator"):
            source,pe,methods,descriptors=self.fixture()
            qualified=m.digest(json.dumps(descriptors,sort_keys=True,separators=(",",":")).encode())
            descriptors[0][field]="0x11" if field=="wrapper_call_offset" else 999
            with self.subTest(field=field),self.assertRaisesRegex(m.FormatError,"descriptor manifest SHA"):
                self.recover(source,pe,methods,descriptors,qualified)

    def test_actual_wrapper_argument_mutations_rejected_even_with_new_descriptor_hash(self):
        for field in ("wrapper_call_offset","builder_discriminator","source_rva","length","external_local_rva","external_local_length","fallback_local_rva"):
            source,pe,methods,descriptors=self.fixture()
            if field=="wrapper_call_offset":descriptors[0][field]="0x00"
            else:descriptors[0][field]+=1
            with self.subTest(field=field),self.assertRaisesRegex(m.FormatError,"wrapper (call offset|builder arguments)"):
                self.recover(source,pe,methods,descriptors)

    def test_wrapper_unknown_branch_opcode_call_and_operand_boundaries_rejected(self):
        cases=[b"\x02\x2d\x00",b"\x2b\xfe",b"\x20\x00\x00\x00\x00\x2b\xfb",b"\x20",b"\x06",b"\xff",b"\x28"+struct.pack("<I",0x06009999)]
        for code in cases:
            with self.subTest(code=code.hex()),self.assertRaises(m.FormatError):
                m.wrapper_builder_arguments(self.wrap_code(code),0x06000001)

    def test_external_local_signature_read_and_corruption(self):
        source,pe,methods,descriptors=self.fixture()
        signature=b"\x07\x01\x08"
        source[800:803]=signature
        descriptors[0].update(external_local_rva=4896,external_local_length=3,external_local_sha256=m.digest(signature))
        methods[0]=(self.wrapper(descriptors[0]),methods[0][1])
        result,_=self.recover(source,pe,methods,descriptors)
        self.assertEqual(result[0][2],signature)
        self.assertEqual(result[0][3]['external_local_bounds']['local_count'],1)
        source[802]^=1
        with self.assertRaisesRegex(m.FormatError,"local signature SHA"):self.recover(source,pe,methods,descriptors)
        for b in (b"",b"\x07\xff",b"\x07\x05\x08",b"\x06\x01\x08"):
            with self.subTest(signature=b),self.assertRaises(m.FormatError):m.external_local_bounds(b)

    def eh_body(self):
        return struct.pack("<HHII",0x301b,8,2,0)+b"\x00\x2a\x00\x00"+b"\x41\x1c\x00\x00"+struct.pack("<IIIIII",2,0,1,1,1,0)

    def test_real_standard_header_and_eh_extents_without_vbpd(self):
        b=self.eh_body();bounds=m.dynamic_body_bounds(b)
        self.assertEqual(bounds['body_end'],len(b))
        self.assertEqual(bounds['sections'][0]['clauses'][0]['flags'],2)
        for offset,value in ((4,100),(24,3),(36,3),(40,1),(0,0x401b)):
            bad=bytearray(b)
            struct.pack_into("<H" if offset==0 else "<I",bad,offset,value)
            with self.subTest(offset=offset),self.assertRaises(m.FormatError):m.dynamic_body_bounds(bytes(bad))
        for bad in (b[:-1],b+b"VBPD",b"\x00",b"\x02",b"\x03",b"\xfe\x2a"):
            with self.subTest(body=bad),self.assertRaises(m.FormatError):m.dynamic_body_bounds(bad)

    def test_dynamic_flag_independent_output_privacy_and_partial_failure(self):
        import json
        for fail in (False,True):
            source,rva=StaticTests().synthetic_source()
            record=dict(token="06000001",descriptor={},bounds={})
            with tempfile.TemporaryDirectory() as directory:
                path=Path(directory)/"source";path.write_bytes(source);output=Path(directory)/"output"
                original=Path.open
                def failing(path,*args,**kwargs):
                    if fail and path.name.endswith(".dynamic.bin"):raise OSError("injected dynamic output failure")
                    return original(path,*args,**kwargs)
                with patch.object(m,"SOURCE_SHA256",m.digest(source)),patch.object(m,"PAYLOAD_RVAS",(rva,)),patch.object(m,"method_metadata",return_value={}),patch.object(m,"recover_methods",return_value=([],{})),patch.object(m,"recover_dynamic_methods",return_value=([(b"cipher",b"\x06\x2a",None,record)],{})),patch.object(Path,"open",failing):
                    if fail:
                        with self.assertRaises(OSError):m.run(path,output,recover_dynamic=True)
                    else:m.run(path,output,recover_dynamic=True)
                manifest=json.loads((output/"manifest.json").read_text())
                self.assertEqual(manifest['complete'],not fail)
                self.assertEqual(manifest['dynamic_recovery']['complete'],not fail)
                self.assertFalse((output/"methods").exists())
                self.assertNotIn('method_recovery',manifest)
                self.assertEqual(output.stat().st_mode&0o777,0o700)
                self.assertEqual((output/'dynamic-methods').stat().st_mode&0o777,0o700)
                for f in (output/'dynamic-methods').iterdir():self.assertEqual(f.stat().st_mode&0o777,0o600)


if __name__ == "__main__": unittest.main()
