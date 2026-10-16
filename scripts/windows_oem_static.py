#!/usr/bin/env python3
"""Qualified, non-executing extraction of OEM compact native analysis images."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys

SOURCE_SHA256 = "a2b7d81ecf5c1dbdfc19738f17ba5e01e884d1b3688beb668858727307e2a318"
PAYLOAD_RVAS = (0x663BD8, 0x6C1600)
JIT_STUB_RVA = 0x7A5AA8
MAX_SOURCE = 64 * 1024 * 1024
MAX_COMPACT = 1000000
MAX_IMAGE = 16 * 1024 * 1024
SYNTHETIC = ["DOS header and PE placement", "COFF timestamp and symbol fields",
             "unrepresented optional-header fields", "section names",
             "4096 section alignment and 512 file alignment", "raw file pointers and padding",
             "data directories after the first six are zero"]


class FormatError(ValueError):
    pass


def digest(data):
    return hashlib.sha256(data).hexdigest()


def rc4(key, data):
    if not key or len(key) > 256:
        raise FormatError("RC4 key length must be 1..256 bytes")
    s = list(range(256))
    j = 0
    for i in range(256):
        j = (j + s[i] + key[i % len(key)]) & 255
        s[i], s[j] = s[j], s[i]
    i = j = 0
    out = bytearray()
    for value in data:
        i = (i + 1) & 255
        j = (j + s[i]) & 255
        s[i], s[j] = s[j], s[i]
        out.append(value ^ s[(s[i] + s[j]) & 255])
    return bytes(out)


def span(offset, length, limit, label):
    if not (0 <= offset <= 0xFFFFFFFF and 0 <= length <= 0xFFFFFFFF):
        raise FormatError(label + ": integer range")
    end = offset + length
    if end > 0xFFFFFFFF or end > limit:
        raise FormatError(label + ": truncated or out of bounds")
    return offset, end


def disjoint(ranges, start, end, label):
    if any(start < z and a < end for a, z in ranges):
        raise FormatError(label + ": overlapping ranges")
    ranges.append((start, end))


def read_rva(source, pe, rva, length):
    """Require exactly one wholly file-backed mapping; never use PE zero-fill."""
    span(rva, length, 0xFFFFFFFF, "source RVA")
    candidates = []
    for section in pe.sections:
        raw, n = int(section.PointerToRawData), int(section.SizeOfRawData)
        start = int(section.VirtualAddress)
        span(raw, n, len(source), "source section")
        if start <= rva and rva + length <= start + n:
            candidates.append(raw + rva - start)
    if len(candidates) != 1:
        raise FormatError("source RVA is not uniquely file-backed")
    start, end = span(candidates[0], length, len(source), "source payload")
    return source[start:end]


def extract_compact(source, pe, rva):
    header = read_rva(source, pe, rva, 24)
    length = struct.unpack_from("<I", header)[0]
    if not 0 < length < MAX_COMPACT:
        raise FormatError("compact payload exceeds cap or is empty")
    return rc4(header[8:24], read_rva(source, pe, rva + 24, length))


def rebuild(compact):
    if not 94 <= len(compact) < MAX_COMPACT:
        raise FormatError("compact length outside bounds")
    u16 = lambda offset: struct.unpack_from("<H", compact, offset)[0]
    u32 = lambda offset: struct.unpack_from("<I", compact, offset)[0]
    count = u16(6)
    if not (1 <= count <= 32 and u16(4) == 0x8664 and u16(8) == 240 and u16(12) == 0x20B):
        raise FormatError("unsupported compact header")
    data_offset = 94 + count * 36
    if u32(0) != data_offset or len(compact) < data_offset:
        raise FormatError("invalid compact section table offset")
    size = u32(34)
    if not 4096 <= size <= MAX_IMAGE:
        raise FormatError("image size outside cap")
    virtual_ranges, raw_ranges, sections = [], [], []
    for i in range(count):
        offset = 94 + 36 * i
        rva, vsize, n, raw, flags = struct.unpack_from("<5I", compact, offset)
        if not n or raw < data_offset or rva < 4096:
            raise FormatError("invalid section bounds")
        _, raw_end = span(raw, n, len(compact), "compact section")
        _, virtual_end = span(rva, max(vsize, n), size, "virtual section")
        disjoint(raw_ranges, raw, raw_end, "compact raw")
        disjoint(virtual_ranges, rva, virtual_end, "virtual section")
        data = rc4(compact[offset + 20:offset + 36], compact[raw:raw_end])
        sections.append((rva, vsize, data, flags, raw))
    entry = u32(14)
    if entry and not any(a <= entry < z for a, z in virtual_ranges):
        raise FormatError("entry RVA outside sections")
    directories = [struct.unpack_from("<II", compact, 46 + 8*i) for i in range(6)]
    for index, (rva, n) in enumerate(directories):
        if bool(rva) != bool(n):
            raise FormatError("incomplete data directory")
        if n:
            _, end = span(rva, n, size, "data directory")
            # Security directory is a file offset, not an RVA. It is unsupported.
            if index == 4 or not any(a <= rva and end <= z for a, z in virtual_ranges):
                raise FormatError("unsupported or unmapped data directory")
    align = lambda n: (n + 511) // 512 * 512
    hsize = align(64 + 24 + 240 + 40 * count)
    output = bytearray(hsize)
    def put(offset, fmt, *values):
        struct.pack_into("<" + fmt, output, offset, *values)
    output[:2] = b"MZ"
    put(60, "I", 64)
    output[64:68] = b"PE\0\0"
    put(68, "HHIIIHH", u16(4), count, 0, 0, 0, 240, u16(10))
    opt = 88
    put(opt, "H", 0x20B)
    put(opt+16, "I", entry)
    put(opt+24, "Q", struct.unpack_from("<Q", compact, 18)[0])
    put(opt+32, "II", 4096, 512)
    put(opt+56, "II", size, hsize)
    put(opt+68, "HH", u16(42), u16(44))
    put(opt+108, "I", 16)
    for i, directory in enumerate(directories):
        put(opt+112+8*i, "II", *directory)
    records = []
    for i, (rva, vsize, data, flags, raw) in enumerate(sections):
        pointer = align(len(output))
        output.extend(b"\0" * (pointer - len(output)))
        output.extend(data)
        output.extend(b"\0" * (align(len(output)) - len(output)))
        sh = 64 + 24 + 240 + 40*i
        output[sh:sh+8] = f".s{i}".encode().ljust(8, b"\0")
        put(sh+8, "IIIIIIHHI", vsize, rva, len(data), pointer, 0, 0, 0, 0, flags)
        records.append(dict(index=i, rva=hex(rva), virtual_size=vsize, raw_size=len(data),
                            compact_offset=raw, analysis_raw_offset=pointer,
                            characteristics=hex(flags), sha256=digest(data)))
    return bytes(output), records


def dependency():
    try:
        import pefile
    except ImportError as exc:
        raise FormatError("pefile is required; install it separately before analysis") from exc
    return pefile


def inventory(pe):
    exports = [dict(name=(s.name or b"").decode("ascii", "replace"), rva=hex(s.address), ordinal=s.ordinal)
               for s in getattr(getattr(pe, "DIRECTORY_ENTRY_EXPORT", None), "symbols", [])]
    imports = [dict(dll=d.dll.decode("ascii", "replace"),
                    names=[(i.name or b"").decode("ascii", "replace") or f"ordinal:{i.ordinal}" for i in d.imports])
               for d in getattr(pe, "DIRECTORY_ENTRY_IMPORT", [])]
    return dict(exports=exports, imports=imports,
                relocation_blocks=len(getattr(pe, "DIRECTORY_ENTRY_BASERELOC", [])), warnings=pe.get_warnings())



def method_metadata(source, pe):
    """Read only CLR metadata table layout needed to locate MethodDef RID rows."""
    directory = pe.OPTIONAL_HEADER.DATA_DIRECTORY[14]
    cli = read_rva(source, pe, int(directory.VirtualAddress), 16)
    metadata_rva, metadata_size = struct.unpack_from("<II", cli, 8)
    if not 20 <= metadata_size <= MAX_SOURCE:
        raise FormatError("CLR metadata size outside bounds")
    b = read_rva(source, pe, metadata_rva, metadata_size)
    if b[:4] != b"BSJB":
        raise FormatError("invalid CLR metadata signature")
    version_size = struct.unpack_from("<I", b, 12)[0]
    _, pos = span(16, version_size, len(b), "metadata version")
    pos = (pos + 3) & ~3
    span(pos, 4, len(b), "metadata stream count")
    count = struct.unpack_from("<H", b, pos + 2)[0]
    if not 1 <= count <= 64:
        raise FormatError("metadata stream count outside cap")
    pos += 4
    streams, ranges = {}, []
    for _ in range(count):
        span(pos, 8, len(b), "metadata stream")
        offset, size = struct.unpack_from("<II", b, pos)
        name_start = pos + 8
        end = b.find(b"\0", name_start, min(name_start + 32, len(b)))
        if end < 0:
            raise FormatError("unterminated metadata stream name")
        name = b[name_start:end]
        if name in streams:
            raise FormatError("duplicate metadata stream")
        _, stream_end = span(offset, size, len(b), "metadata stream data")
        disjoint(ranges, offset, stream_end, "metadata streams")
        streams[name] = (offset, size)
        pos = (end + 4) & ~3
    tables = [streams[k] for k in (b"#~", b"#-") if k in streams]
    if len(tables) != 1:
        raise FormatError("missing or ambiguous CLR tables stream")
    offset, size = tables[0]
    if size < 24 or offset < pos:
        raise FormatError("invalid metadata tables header")
    table = b[offset:offset + size]
    heap_flags = table[6]
    valid = struct.unpack_from("<Q", table, 8)[0]
    counts = {}
    row_pos = 24
    for table_id in range(64):
        if valid & (1 << table_id):
            span(row_pos, 4, len(table), "metadata row counts")
            counts[table_id] = struct.unpack_from("<I", table, row_pos)[0]
            row_pos += 4
    if not 0 < counts.get(6, 0) <= 0xFFFFFF:
        raise FormatError("invalid MethodDef row count")
    heap = lambda bit: 4 if heap_flags & bit else 2
    index = lambda table_id: 4 if counts.get(table_id, 0) >= 65536 else 2
    coded = lambda ids, bits: 4 if max((counts.get(t, 0) for t in ids), default=0) >= (1 << (16-bits)) else 2
    string, guid, blob = heap(1), heap(2), heap(4)
    row_sizes = [2 + string + 3*guid,
                 coded((0, 26, 35, 1), 2) + 2*string,
                 4 + 2*string + coded((2, 1, 27), 2) + index(4) + index(6),
                 index(4), 2 + string + blob, index(6)]
    for table_id, row_size in enumerate(row_sizes):
        _, row_pos = span(row_pos, counts.get(table_id, 0)*row_size, len(table), "metadata table rows")
    method_size = 8 + string + blob + index(8)
    span(row_pos, counts[6]*method_size, len(table), "MethodDef rows")
    methods = {}
    for rid in range(1, counts[6] + 1):
        position = row_pos + (rid - 1)*method_size
        methods[0x06000000 | rid] = dict(rid=rid, methoddef_row_rva=hex(metadata_rva + offset + position),
                                       original_rva=hex(struct.unpack_from("<I", table, position)[0]))
    return methods


def native_method_key(analysis):
    pefile = dependency()
    pe = pefile.PE(data=analysis)
    try:
        parts = []
        origins = []
        for rva, displacement in ((0x1DD2, 0x1B0), (0x1DE2, 0x1B4), (0x1DF0, 0x1B8), (0x1DFA, 0x1BC)):
            instruction = read_rva(analysis, pe, rva, 10)
            if instruction[:6] != b"\xc7\x85" + struct.pack("<I", displacement):
                raise FormatError("native key instruction provenance mismatch")
            parts.append(instruction[6:10])
            origins.append(dict(instruction_rva=hex(rva), immediate_offset=6, immediate_size=4))
        return b"".join(parts), origins
    finally:
        pe.close()


def body_header(body):
    if len(body) < 5 or body[-4:] != b"VBPD":
        raise FormatError("recovered body missing VBPD marker")
    kind = body[0] & 3
    if kind == 2:
        header_size, code_size, flags = 1, body[0] >> 2, 2
        local_signature = 0
    elif kind == 3:
        if len(body) < 16:
            raise FormatError("truncated fat method header")
        flags = struct.unpack_from("<H", body)[0]
        header_size = (flags >> 12) * 4
        if header_size != 12:
            raise FormatError("unsupported fat method header size")
        code_size, local_signature = struct.unpack_from("<II", body, 4)
        if local_signature and (local_signature >> 24 != 0x11 or not (local_signature & 0xFFFFFF)):
            raise FormatError("invalid local signature token")
    else:
        raise FormatError("unsupported recovered method header")
    _, end = span(header_size, code_size, len(body) - 4, "method code")
    has_sections = kind == 3 and bool(flags & 8)
    section_records = []
    if has_sections:
        position = (end + 3) & ~3
        while True:
            span(position, 4, len(body)-4, "method section header")
            section_kind = body[position]
            fat = bool(section_kind & 0x40)
            if section_kind & 0x3F != 1:
                raise FormatError("unsupported method extra section kind")
            size = int.from_bytes(body[position+1:position+4], "little") if fat else body[position+1]
            clause_size = 24 if fat else 12
            if size < 4 or (size-4) % clause_size:
                raise FormatError("invalid exception section size")
            _, section_end = span(position, size, len(body)-4, "exception section")
            section_records.append(dict(offset=position, size=size, fat=fat))
            end = section_end
            if not section_kind & 0x80:
                break
            position = (section_end + 3) & ~3
    padding = body[end:-4]
    if len(padding) > 3 or any(padding):
        raise FormatError("unexpected method tail bytes")
    return dict(header_kind="tiny" if kind == 2 else "fat", header_size=header_size,
                code_size=code_size, local_signature_token=hex(local_signature),
                extra_sections=section_records, padding_size=len(padding), marker="VBPD")


def recover_methods(source, pe, analysis, source_map):
    stub = JIT_STUB_RVA
    first = read_rva(source, pe, stub, 1)[0]
    if first & 3 != 2:
        raise FormatError("qualified JIT stub is not tiny")
    stub_end = stub + 1 + (first >> 2)
    trailer = read_rva(source, pe, stub_end, 8)
    if trailer[:4] != b"FOAP":
        raise FormatError("JIT stub FOAP marker missing")
    table_rva = stub_end + 8 + struct.unpack_from("<i", trailer, 4)[0]
    header = read_rva(source, pe, table_rva, 8)
    if header[:4] != b"FWSN":
        raise FormatError("FWSN table signature missing")
    count = struct.unpack_from("<I", header, 4)[0]
    if not 0 < count <= 100000:
        raise FormatError("FWSN record count outside cap")
    table = read_rva(source, pe, table_rva + 8, count * 24)
    key, origins = native_method_key(analysis)
    seen, methods = set(), []
    total_size = 0
    for i in range(count):
        token, relative, fallback, size, local_offset, extra = struct.unpack_from("<IiIIII", table, i*24)
        if token >> 24 != 6 or token not in source_map or token in seen:
            raise FormatError("invalid, duplicate or unmapped MethodDef token")
        seen.add(token)
        record_rva = table_rva + 8 + i*24
        payload_rva = record_rva + 8 + relative
        if payload_rva != fallback or not 0 < size < MAX_COMPACT:
            raise FormatError("FWSN payload offset or length invalid")
        total_size += size
        if total_size > MAX_SOURCE:
            raise FormatError("method aggregate payload exceeds cap")
        cipher = read_rva(source, pe, payload_rva, size)
        plain = rc4(key, cipher)
        details = body_header(plain)
        # Record local_offset as opaque provenance; it is not a body-relative offset.
        methods.append((plain, dict(token=hex(token), index=i, record_rva=hex(record_rva),
            signed_relative=relative, source_rva=hex(payload_rva), fallback_rva=hex(fallback),
            length=size, local_signature_offset=local_offset, extra=extra,
            encrypted_sha256=digest(cipher), recovered_sha256=digest(plain),
            **source_map[token], **details)))
    return methods, dict(table_rva=hex(table_rva), table_records=count, record_size=24,
        stub_rva=hex(stub), stub_trailer_rva=hex(stub_end), table_sha256=digest(header+table),
        key_origin="native machine-code instruction immediates; key bytes are not recorded", key_instructions=origins,
        native_analysis_sha256=digest(analysis), all_tokens_unique=True,
        verification="Header, code bounds, EH section sizes and MethodDef RID mapping only; not CLR stack or full CIL verification")


# Qualified source facts, not vendor instructions or cryptographic key bytes.
# FWSN wrapper hashes bind the descriptor arguments to independently recovered bodies.
DYNAMIC_DESCRIPTORS = ({'token': 100667985,
  'wrapper_sha256': 'e06f82f555e7f5c1e1e88d0cd8436414760ae7decb472999cc561d6713e089f5',
  'wrapper_call_offset': '0x26',
  'source_rva': 7552968,
  'length': 19,
  'cipher_sha256': '8a730e95a8261e552b9c21fdcb61e521c75146bd84e37469c60716d3ceaddf0f',
  'body_sha256': 'f6d6d4da10b91e822806ab622da2e4dcf93be51956f87b80db87f69f78060507',
  'external_local_rva': 0,
  'external_local_length': 0,
  'external_local_sha256': None,
  'fallback_local_rva': 0,
  'builder_discriminator': 5},
 {'token': 100669807,
  'wrapper_sha256': '5dc44c355bdaf95fa0ecf4ba10c1ef649ed2a4271e8f1a42d65601cb0be4b9fb',
  'wrapper_call_offset': '0x29',
  'source_rva': 7012208,
  'length': 75,
  'cipher_sha256': '4b340c021351241cd67760eed872ae8c0a846465ed03fc91e6267db31ba7054c',
  'body_sha256': 'be8d7476a2fbd14bb7785a5aea8f56a6bbf2b6018ef6a460fc0bd7425ac653c2',
  'external_local_rva': 6932712,
  'external_local_length': 32,
  'external_local_sha256': 'c52c01dcadfae21c887530df86c2d87481dffc18e4a62bd6a3c666127e6ff620',
  'fallback_local_rva': 9033915,
  'builder_discriminator': 4},
 {'token': 100669939,
  'wrapper_sha256': '95c7ba02ff0a7c590d706f7ca1cdb2e58ff9c92fa220df94e69bb47a096a45ea',
  'wrapper_call_offset': '0x2e',
  'source_rva': 6694080,
  'length': 1841,
  'cipher_sha256': '496fecabd5fbfc0c88698481e4ca7e42455f6ce7ea05fc019a70080b3a7c8bbf',
  'body_sha256': 'f59f609ce2aaf02bd54a79239e887c48b9639446d11e1200cc3dd6ab3b12a6f1',
  'external_local_rva': 0,
  'external_local_length': 0,
  'external_local_sha256': None,
  'fallback_local_rva': 0,
  'builder_discriminator': 3},
 {'token': 100669940,
  'wrapper_sha256': '432d59f2f4762fec65e08a06c41289a2f5bd15e3a514bba375b5b740d3515cb1',
  'wrapper_call_offset': '0x34',
  'source_rva': 6667976,
  'length': 27,
  'cipher_sha256': 'b1b018305bb9cd323ad58c2c2532cd817a4c1c8989860e0f1655c703def86d89',
  'body_sha256': 'dd665ec79b2c27dd5253c51ec3b9927ad5a1a1605bdac429df9479901ab25497',
  'external_local_rva': 0,
  'external_local_length': 0,
  'external_local_sha256': None,
  'fallback_local_rva': 0,
  'builder_discriminator': 2},
 {'token': 100669942,
  'wrapper_sha256': '9acf82a1abe3bfc4a6a4cba34a63570a3d2feb2710624c272c7203ede06ae232',
  'wrapper_call_offset': '0x30',
  'source_rva': 7605856,
  'length': 608,
  'cipher_sha256': 'eb8623cf30301efc48b928afde547939fb6728b91f68748033a3303563305743',
  'body_sha256': 'cdd6b5aa71054ecba7da7ad51263c2ca41999aab3a7feff1cd677ce9ca4323d3',
  'external_local_rva': 7566520,
  'external_local_length': 70,
  'external_local_sha256': '9ad88046290fb6b79f145e762fd5a559bd7014703f038345e0c9e4cd85cd85dc',
  'fallback_local_rva': 9034257,
  'builder_discriminator': 1},
 {'token': 100670619,
  'wrapper_sha256': '0667187bc4815e71c3d58133933f8893e1ec0f59b082bc3de73b56304636e808',
  'wrapper_call_offset': '0x26',
  'source_rva': 6635000,
  'length': 87,
  'cipher_sha256': 'a9a23873f7f38982ea1f3b8af8ecc3cd340ae0f36e173f7b450a8c59a2085c26',
  'body_sha256': 'cdc78381c23050f9024170835ded1528f5d7fa2df060569b72d70d3bc204289e',
  'external_local_rva': 7003384,
  'external_local_length': 32,
  'external_local_sha256': '0f7e97248bdf41d765bff6c0e7cc4639e9314a18eb56c26f27e883f0be1fe722',
  'fallback_local_rva': 9032973,
  'builder_discriminator': 0})
DYNAMIC_DESCRIPTOR_SHA256 = "35ecc08cfb52a376df811f80013964b5bd7f369f61fb856954b9db39dc0e2bf4"
DYNAMIC_KEY_FIELD_TOKEN = 0x040025AC
DYNAMIC_KEY_FIELD_RVA = 0x792328
DYNAMIC_LAYER_KEY_RVA = 0x70B6F0
DYNAMIC_LAYER_CIPHER_SHA256 = "186bc65ef99789fb8d3272dd26dbe356898d3d64540963d1389fd426d5db595b"


def dynamic_body_bounds(body):
    """Standard CIL body extents, without the FWSN outer VBPD trailer."""
    if not body or len(body) >= MAX_COMPACT:
        raise FormatError("dynamic body length outside cap")
    kind = body[0] & 3
    if kind == 2:
        header_size, code_size, stack, flags, local = 1, body[0] >> 2, 8, 2, 0
    elif kind == 3:
        span(0, 12, len(body), "dynamic fat header")
        flags, stack, code_size, local = struct.unpack_from("<HHII", body)
        header_size = (flags >> 12)*4
        if header_size != 12 or flags & 0x0FE4:
            raise FormatError("unsupported dynamic fat flags/header")
        if local and (local >> 24 != 0x11 or not (local & 0xFFFFFF)):
            raise FormatError("invalid dynamic locals token")
    else:
        raise FormatError("invalid dynamic method header")
    if not code_size:
        raise FormatError("empty dynamic method code")
    _, end = span(header_size, code_size, len(body), "dynamic code")
    sections = []
    if kind == 3 and flags & 8:
        position = (end + 3) & ~3
        if any(body[end:position]):
            raise FormatError("nonzero dynamic section alignment padding")
        while True:
            span(position, 4, len(body), "dynamic EH header")
            section_kind = body[position]
            if section_kind & 0x3F != 1:
                raise FormatError("unsupported dynamic EH kind")
            fat = bool(section_kind & 0x40)
            size = int.from_bytes(body[position+1:position+4], "little") if fat else body[position+1]
            clause_size = 24 if fat else 12
            if size < 4 or (size-4) % clause_size:
                raise FormatError("invalid dynamic EH size")
            if not fat and any(body[position+2:position+4]):
                raise FormatError("invalid small EH reserved bytes")
            _, end = span(position, size, len(body), "dynamic EH section")
            clauses = []
            for offset in range(position+4, end, clause_size):
                f, start, n, handler, hn, extra = struct.unpack_from("<IIIIII" if fat else "<HHBHBI", body, offset)
                if f not in (0, 1, 2, 4) or not n or not hn:
                    raise FormatError("invalid dynamic EH clause flags/extent")
                span(start, n, code_size, "EH try")
                span(handler, hn, code_size, "EH handler")
                if f == 1 and extra >= code_size:
                    raise FormatError("EH filter outside method code")
                if f in (2, 4) and extra:
                    raise FormatError("finally/fault clause has unexpected token")
                if f == 0 and (extra >> 24 not in (1, 2, 0x1B) or not (extra & 0xFFFFFF)):
                    raise FormatError("invalid EH catch type token")
                clauses.append(dict(flags=f, try_start=start, try_length=n, handler_start=handler,
                                    handler_length=hn, class_token_or_filter=hex(extra)))
            sections.append(dict(offset=position, kind=section_kind, fat=fat, size=size, clauses=clauses))
            if not section_kind & 0x80:
                break
            position = (end+3)&~3
            if any(body[end:position]):
                raise FormatError("nonzero chained EH alignment padding")
    if end != len(body):
        raise FormatError("unexpected trailing dynamic method bytes")
    return dict(format="tiny" if kind == 2 else "fat", header_size=header_size,
                code_size=code_size, max_stack=stack, flags=hex(flags), locals_token=hex(local),
                sections=sections, body_end=end, trailing_bytes=0)


def external_local_bounds(signature):
    """Validate local-signature prefix/count bounds; full type grammar is separate."""
    if len(signature) < 2 or len(signature) > 65536 or signature[0] != 7:
        raise FormatError("invalid external local signature header/length")
    first = signature[1]
    width = 1 if first < 0x80 else 2 if first < 0xC0 else 4 if first < 0xE0 else 0
    if not width or len(signature) < 1 + width:
        raise FormatError("invalid compressed local count")
    count = (first & (0x7F if width == 1 else 0x3F if width == 2 else 0x1F))
    for value in signature[2:1+width]:
        count = (count << 8) | value
    if count > len(signature) - 1 - width:
        raise FormatError("external local count exceeds available type bytes")
    return dict(local_count=count, count_size=width,
                verification="Prefix/compressed count and minimum byte bounds only; full signature type grammar is independently audited")



def wrapper_builder_arguments(wrapper, token):
    """Bounded static prefix evaluation; symbolic values never call OEM/CLR code."""
    header = body_header(wrapper)
    start, size = header['header_size'], header['code_size']
    code = wrapper[start:start+size]
    pc, stack, locals_, decoded = 0, [], {}, {}
    def pop():
        if not stack:
            raise FormatError("wrapper stack underflow")
        return stack.pop()
    def integer(value):
        if type(value) is not int:
            raise FormatError("wrapper arithmetic/branch needs known integer")
        return value
    def i32(value):
        value &= 0xFFFFFFFF
        return value - 0x100000000 if value & 0x80000000 else value
    for step in range(1000):
        span(pc, 1, len(code), "wrapper instruction")
        opcode = code[pc]
        op_size = 1
        operand_size = 0
        if opcode == 0xFE:
            span(pc, 2, len(code), "wrapper extended opcode")
            opcode = 0xFE00 | code[pc+1]
            op_size = 2
        if opcode in (0x0E, 0x11, 0x13, 0x1F, 0x2B, 0x2C, 0x2D):
            operand_size = 1
        elif opcode in (0x20, 0x28, 0x38, 0x39, 0x3A, 0x74):
            operand_size = 4
        elif opcode in (0xFE09, 0xFE0C, 0xFE0E):
            operand_size = 2
        elif opcode not in (0x00, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
                             0x0A, 0x0B, 0x0C, 0x0D, 0x14, 0x15, 0x16, 0x17, 0x18,
                             0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x25, 0x26,
                             0x58, 0x59, 0x5A, 0x5F, 0x60, 0x61, 0x62, 0x63, 0x64,
                             0x65, 0x66, 0x69, 0x6D):
            raise FormatError("unsupported reachable wrapper opcode")
        _, end = span(pc, op_size+operand_size, len(code), "wrapper operand")
        if pc not in decoded:
            if any(pc < z and a < end for a, z in decoded.values()):
                raise FormatError("wrapper branch overlaps decoded instruction boundary")
            decoded[pc] = (pc, end)
        value = int.from_bytes(code[pc+op_size:end], "little")
        next_pc = end
        if opcode == 0x00:
            pass
        elif 0x02 <= opcode <= 0x05 or opcode in (0x0E, 0xFE09):
            stack.append(("arg", opcode-2 if opcode <= 5 else value))
        elif 0x06 <= opcode <= 0x09 or opcode in (0x11, 0xFE0C):
            index = opcode-6 if opcode <= 9 else value
            if index not in locals_:
                raise FormatError("uninitialized wrapper local")
            stack.append(locals_[index])
        elif 0x0A <= opcode <= 0x0D or opcode in (0x13, 0xFE0E):
            index = opcode-0x0A if opcode <= 0x0D else value
            if index >= 256:
                raise FormatError("wrapper local index exceeds cap")
            locals_[index] = pop()
        elif opcode == 0x14:
            stack.append(None)
        elif 0x15 <= opcode <= 0x1E:
            stack.append(opcode-0x16)
        elif opcode in (0x1F, 0x20):
            stack.append(int.from_bytes(code[pc+op_size:end], "little", signed=True))
        elif opcode == 0x25:
            item = pop(); stack.extend((item, item))
        elif opcode == 0x26:
            pop()
        elif opcode == 0x74:
            item = pop(); stack.append(item)  # symbolic cast; never resolve a runtime type
        elif opcode == 0x28:
            if value == 0x0A0001DC:
                stack.append(("currentmethod", token))
            elif value == 0x0600182F:
                if len(stack) != 9:
                    raise FormatError("builder call wrapper stack is not exactly nine arguments")
                return dict(call_offset=hex(pc), args=stack, steps=step+1,
                            visited_instruction_count=len(decoded),
                            verification="Reachable bounded prefix only; skipped bytes and full original CFG are not decoded")
            else:
                raise FormatError("unsupported wrapper call target")
        elif opcode in (0x2B, 0x2C, 0x2D, 0x38, 0x39, 0x3A):
            relative = int.from_bytes(code[pc+op_size:end], "little", signed=True)
            take = True
            if opcode not in (0x2B, 0x38):
                predicate = pop()
                if predicate is not None and type(predicate) is not int:
                    raise FormatError("unknown symbolic wrapper branch predicate")
                take = bool(predicate) if opcode in (0x2D, 0x3A) else not bool(predicate)
            if take:
                next_pc = end + relative
                span(next_pc, 1, len(code), "wrapper branch target")
                if any(a < next_pc < z for a, z in decoded.values()):
                    raise FormatError("wrapper branch targets decoded operand")
        elif opcode in (0x69, 0x6D):
            item = integer(pop())
            stack.append(i32(item) if opcode == 0x69 else item & 0xFFFFFFFF)
        elif opcode in (0x65, 0x66):
            item = integer(pop());stack.append(i32(-item if opcode == 0x65 else ~item))
        else:
            right, left = integer(pop()), integer(pop())
            if opcode == 0x58: result = left+right
            elif opcode == 0x59: result = left-right
            elif opcode == 0x5A: result = left*right
            elif opcode == 0x5F: result = left & right
            elif opcode == 0x60: result = left | right
            elif opcode == 0x61: result = left ^ right
            elif opcode == 0x62: result = left << (right & 31)
            elif opcode == 0x63: result = i32(left) >> (right & 31)
            else: result = (left & 0xFFFFFFFF) >> (right & 31)
            stack.append(i32(result))
        if len(stack) > 128:
            raise FormatError("wrapper stack exceeds cap")
        pc = next_pc
    raise FormatError("wrapper prefix step limit exceeded")

def recover_dynamic_methods(source, pe, methods):
    descriptor_sha = digest(json.dumps(DYNAMIC_DESCRIPTORS, sort_keys=True, separators=(",", ":")).encode())
    if descriptor_sha != DYNAMIC_DESCRIPTOR_SHA256:
        raise FormatError("qualified dynamic descriptor manifest SHA mismatch")
    wrappers = {int(record['token'], 16): (body, record) for body, record in methods}
    stored = read_rva(source, pe, DYNAMIC_KEY_FIELD_RVA, 32)
    fixed = bytes(((value ^ 0x3C) - 0xC3) & 255 for value in stored)
    encrypted_key = read_rva(source, pe, DYNAMIC_LAYER_KEY_RVA, 16)
    if digest(encrypted_key) != DYNAMIC_LAYER_CIPHER_SHA256:
        raise FormatError("dynamic layer key cipher provenance mismatch")
    key = rc4(fixed, encrypted_key)
    seen, payload_ranges, local_ranges, results = set(), [], [], []
    all_ranges = [(DYNAMIC_KEY_FIELD_RVA, DYNAMIC_KEY_FIELD_RVA+32),
                  (DYNAMIC_LAYER_KEY_RVA, DYNAMIC_LAYER_KEY_RVA+16)]
    if len(DYNAMIC_DESCRIPTORS) != 6:
        raise FormatError("qualified dynamic descriptor count mismatch")
    for desc in DYNAMIC_DESCRIPTORS:
        token = desc['token']
        if token in seen or token not in wrappers or digest(wrappers[token][0]) != desc['wrapper_sha256']:
            raise FormatError("dynamic descriptor wrapper hash/token mismatch")
        seen.add(token)
        call = wrapper_builder_arguments(wrappers[token][0], token)
        expected = [desc['source_rva'], desc['length'], desc['external_local_rva'],
                    desc['external_local_length'], desc['fallback_local_rva'],
                    DYNAMIC_LAYER_KEY_RVA, desc['builder_discriminator']]
        if call['call_offset'] != desc['wrapper_call_offset'] or call['args'][1] != ("currentmethod", token):
            raise FormatError("dynamic wrapper call offset/current-method mismatch")
        if call['args'][2:] != expected:
            raise FormatError("dynamic wrapper builder arguments disagree with descriptor")
        rva, n = desc['source_rva'], desc['length']
        if not 0 < n < MAX_COMPACT:
            raise FormatError("dynamic descriptor payload length outside cap")
        _, end = span(rva, n, 0xFFFFFFFF, "dynamic payload")
        disjoint(payload_ranges, rva, end, "dynamic payloads")
        disjoint(all_ranges, rva, end, "dynamic key/payload/local regions")
        cipher = read_rva(source, pe, rva, n)
        if digest(cipher) != desc['cipher_sha256']:
            raise FormatError("dynamic cipher SHA mismatch")
        body = rc4(key, cipher)
        if digest(body) != desc['body_sha256']:
            raise FormatError("dynamic body SHA mismatch (key or cipher corruption)")
        bounds = dynamic_body_bounds(body)
        local_rva, local_size = desc['external_local_rva'], desc['external_local_length']
        if bool(local_rva) != bool(local_size):
            raise FormatError("incomplete external local signature descriptor")
        if desc["fallback_local_rva"]:
            # Only file backing is established here; fallback contents/semantics are not inferred.
            read_rva(source, pe, desc["fallback_local_rva"], 1)
        signature = None
        local_details = None
        if local_size:
            if not 0 < local_size <= 65536:
                raise FormatError("external local signature exceeds cap")
            _, local_end = span(local_rva, local_size, 0xFFFFFFFF, "external local signature")
            disjoint(local_ranges, local_rva, local_end, "external local signatures")
            disjoint(all_ranges, local_rva, local_end, "dynamic key/payload/local regions")
            signature = read_rva(source, pe, local_rva, local_size)
            if digest(signature) != desc['external_local_sha256']:
                raise FormatError("external local signature SHA mismatch")
            local_details = external_local_bounds(signature)
        results.append((cipher, body, signature, dict(token=f"{token:08x}", descriptor=desc,
            wrapper_record_rva=wrappers[token][1]['record_rva'],
            wrapper_source_rva=wrappers[token][1]['source_rva'],
            wrapper_argument_verification=dict(call_offset=call['call_offset'], steps=call['steps'],
                visited_instruction_count=call['visited_instruction_count'],
                arguments_match_descriptor=True, scope=call['verification']), bounds=bounds,
            external_local_bounds=local_details)))
    # Read-only key material intentionally never enters receipts or source literals.
    origin = dict(stored_fixed_key_field_token=hex(DYNAMIC_KEY_FIELD_TOKEN), stored_fixed_key_rva=hex(DYNAMIC_KEY_FIELD_RVA),
        stored_fixed_key_size=32, fixed_key_derivation="u8((stored XOR 0x3c)-0xc3)",
        layer_key_cipher_rva=hex(DYNAMIC_LAYER_KEY_RVA), layer_key_cipher_size=16,
        layer_key_cipher_sha256=digest(encrypted_key),
        descriptor_origin="Frozen independently audited descriptors; recovered wrapper hashes and bounded reachable constant-prefix nine builder arguments reverified from actual bytes",
        descriptor_manifest_sha256=descriptor_sha,
        counting_boundary="Six payloads implement existing wrapper MethodDefs, not six additional methods beyond 5183 outers",
        verification="Static bytes only; standard header/code/EH extents and local signature prefix bounds, not CLR stack or complete CIL/signature verification")
    return results, origin

def run(source_path, output_path, recover=False, recover_dynamic=False):
    pefile = dependency()
    if source_path.stat().st_size > MAX_SOURCE:
        raise FormatError("source exceeds 64 MiB cap")
    with source_path.open("rb") as source_file:
        source = source_file.read(MAX_SOURCE + 1)
    if len(source) > MAX_SOURCE or digest(source) != SOURCE_SHA256:
        raise FormatError("source SHA256 is not the qualified v5.17.51.34 GCUService.exe")
    pe = pefile.PE(data=source)
    try:
        # Refuse overlapping or truncated original PE sections before mapping payloads.
        raw_ranges, virtual_ranges = [], []
        for section in pe.sections:
            raw, n = int(section.PointerToRawData), int(section.SizeOfRawData)
            rva, vsize = int(section.VirtualAddress), int(section.Misc_VirtualSize)
            _, end = span(raw, n, len(source), "source section")
            if n:
                disjoint(raw_ranges, raw, end, "source raw")
            _, end = span(rva, max(n, vsize), 0xFFFFFFFF, "source virtual")
            if end > rva:
                disjoint(virtual_ranges, rva, end, "source virtual")
        # Validate and transform both payloads before creating any output directory.
        images = []
        for rva in PAYLOAD_RVAS:
            compact = extract_compact(source, pe, rva)
            analysis, sections = rebuild(compact)
            parsed = pefile.PE(data=analysis)
            try:
                info = inventory(parsed)
            finally:
                parsed.close()
            images.append((rva, compact, analysis, sections, info))
        methods, method_origin = (recover_methods(source, pe, images[0][2], method_metadata(source, pe))
                                  if recover or recover_dynamic else ([], None))
        dynamics, dynamic_origin = (recover_dynamic_methods(source, pe, methods) if recover_dynamic else ([], None))
        manifest = dict(schema_version=1, complete=False, source=str(source_path),
                        source_sha256=digest(source), source_size=len(source),
                        source_warnings=pe.get_warnings(), synthetic=SYNTHETIC,
                        limitation="Static analysis container only; never executed, not an original runnable DLL or complete original CFG",
                        images=[])
        output_path.mkdir(mode=0o700)  # private output; atomic refusal for existing directories, files or symlinks
        def receipt():
            (output_path / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        receipt()
        for rva, compact, analysis, sections, info in images:
            name = f"native-{rva:x}"
            compact_name, analysis_name = name + ".compact.bin", name + ".compact.analysis.exe"
            with (output_path / compact_name).open("xb") as f:
                f.write(compact)
            with (output_path / analysis_name).open("xb") as f:
                f.write(analysis)
            manifest["images"].append(dict(rva=hex(rva), compact=compact_name,
                compact_sha256=digest(compact), compact_size=len(compact), analysis=analysis_name,
                analysis_sha256=digest(analysis), analysis_size=len(analysis), sections=sections, **info))
            receipt()
        if recover:
            method_dir = output_path / "methods"
            method_dir.mkdir()
            manifest["method_recovery"] = dict(complete=False, origin=method_origin, methods=[])
            receipt()
            for plain, record in methods:
                name = f"{int(record['token'], 16):08x}.recovered.bin"
                with (method_dir / name).open("xb") as f:
                    f.write(plain)
                manifest["method_recovery"]["methods"].append(dict(file="methods/"+name, **record))
            manifest["method_recovery"]["complete"] = True
        if recover_dynamic:
            dynamic_dir = output_path / "dynamic-methods"
            dynamic_dir.mkdir(mode=0o700)
            manifest["dynamic_recovery"] = dict(complete=False, origin=dynamic_origin, methods=[])
            receipt()
            for cipher, body, signature, record in dynamics:
                outputs = {}
                for suffix, data in (("cipher", cipher), ("dynamic", body), ("external-local-sig", signature)):
                    if data is None:
                        continue
                    name = record['token'] + "." + suffix + ".bin"
                    dest = dynamic_dir / name
                    with dest.open("xb") as f:
                        f.write(data)
                    dest.chmod(0o600)
                    if dest.read_bytes() != data:
                        raise FormatError("dynamic output readback mismatch")
                    outputs[suffix] = dict(file="dynamic-methods/"+name, size=len(data), sha256=digest(data), full_readback_equal=True)
                manifest["dynamic_recovery"]["methods"].append(dict(**record, outputs=outputs))
            manifest["dynamic_recovery"]["complete"] = True
        manifest["complete"] = True
        receipt()
        return manifest
    finally:
        pe.close()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="new directory; existing paths are refused")
    parser.add_argument("--recover-methods", action="store_true", help="also recover qualified FWSN method bodies as static bytes")
    parser.add_argument("--recover-dynamic-methods", action="store_true", help="also recover six qualified inner CIL bodies without executing OEM code")
    args = parser.parse_args(argv)
    try:
        result = run(args.source, args.output, recover=args.recover_methods, recover_dynamic=args.recover_dynamic_methods)
    except Exception as exc:
        print(f"Static extraction failed: {exc}. Any output is incomplete unless manifest.complete is true.", file=sys.stderr)
        return 1
    print(json.dumps(dict(complete=result["complete"], images=len(result["images"]), output=str(args.output))))
    return 0


if __name__ == "__main__":
    sys.exit(main())
