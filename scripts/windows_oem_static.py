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

def run(source_path, output_path, recover=False):
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
                                  if recover else ([], None))
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
    args = parser.parse_args(argv)
    try:
        result = run(args.source, args.output, recover=args.recover_methods)
    except Exception as exc:
        print(f"Static extraction failed: {exc}. Any output is incomplete unless manifest.complete is true.", file=sys.stderr)
        return 1
    print(json.dumps(dict(complete=result["complete"], images=len(result["images"]), output=str(args.output))))
    return 0


if __name__ == "__main__":
    sys.exit(main())
