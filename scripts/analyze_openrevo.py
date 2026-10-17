#!/usr/bin/env python3
"""Inventory the frozen OpenRevo 0.8.5 PE without executing or exporting its code."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import sys
import tempfile

EXPECTED_SIZE = 8098816
EXPECTED_SHA256 = "3e3b9d2297e36a0361d56c0f6357afdd9ebc8b201b943546c5cf8002cfabda09"
MAX_SECTIONS = 96
MAX_IMPORT_DESCRIPTORS = 256
MAX_IMPORT_SYMBOLS = 4096
MAX_NAME_BYTES = 1024
MARKERS = {
    "wmi-cim-class": b"AcpiTest_MULong",
    "wmi-ec-method": b"GetSetULong",
    "acpi-device-id": b"ACPI\\INOU0000",
    "bitland-inert-bridge": b"Hardware barrier: Baolongda platform blocked",
    "bitland-circuit-breaker": b"CRITICAL: Baolongda platform detected!",
    "igpu-native-guard": b"Anti-brick safety guard intercepted illegal iGPU write",
    "ec-write-diagnostic": b"raw_driver_write_ec",
    "models-config-name": b"models.json",
    "factory-specs-field": b"factory_specs",
    "tauri-dependency": b"tauri-2.11.5",
}
ASSET_PATTERN = re.compile(rb"/assets/[A-Za-z0-9_.-]+\.(?:js|css|png)(?![A-Za-z0-9_.-])")


class FormatError(ValueError):
    """Invalid or unsupported file; no complete inventory can be produced."""


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class PEImage:
    """Bounded PE32+ reader; no loader, shell, or third-party PE dependency."""

    def __init__(self, data: bytes):
        self.data = data
        self.sections: list[dict] = []
        self.require(0, 64)
        if data[:2] != b"MZ":
            raise FormatError("missing DOS MZ magic")
        pe = self.u32(0x3C)
        if pe < 64:
            raise FormatError("PE header overlaps DOS header")
        self.require(pe, 24)
        if data[pe:pe + 4] != b"PE\0\0":
            raise FormatError("missing PE signature")
        self.machine = self.u16(pe + 4)
        count, optional_size = self.u16(pe + 6), self.u16(pe + 20)
        if self.machine != 0x8664:
            raise FormatError("only x86-64 PE32+ is supported")
        if not 1 <= count <= MAX_SECTIONS:
            raise FormatError("invalid section count")
        optional = pe + 24
        self.require(optional, optional_size)
        if optional_size < 112 or self.u16(optional) != 0x20B:
            raise FormatError("invalid PE32+ optional header")
        self.image_base = self.u64(optional + 24)
        self.header_size = self.u32(optional + 60)
        directory_count = self.u32(optional + 108)
        if directory_count > 16 or 112 + directory_count * 8 > optional_size:
            raise FormatError("data directories exceed optional header")
        section_table = optional + optional_size
        self.require(section_table, count * 40)
        if self.header_size < section_table + count * 40 or self.header_size > len(data):
            raise FormatError("invalid SizeOfHeaders")
        for index in range(count):
            pos = section_table + index * 40
            virtual_size, rva, raw_size, raw_offset = struct.unpack_from("<IIII", data, pos + 8)
            if rva < self.header_size and max(virtual_size, raw_size):
                raise FormatError("section RVA overlaps headers")
            if rva + max(virtual_size, raw_size) > 1 << 32:
                raise FormatError("section RVA overflows 32 bits")
            if raw_size:
                self.require(raw_offset, raw_size)
                if raw_offset < self.header_size:
                    raise FormatError("section raw data overlaps headers")
            section = {"name": data[pos:pos + 8].rstrip(b"\0").decode("ascii", "replace"),
                       "rva": rva, "virtual_size": virtual_size,
                       "raw_offset": raw_offset, "raw_size": raw_size}
            for previous in self.sections:
                for start, length, other, other_length in (
                    (rva, max(virtual_size, raw_size), previous["rva"], max(previous["virtual_size"], previous["raw_size"])),
                    (raw_offset, raw_size, previous["raw_offset"], previous["raw_size"]),
                ):
                    if length and other_length and max(start, other) < min(start + length, other + other_length):
                        raise FormatError("overlapping section ranges")
            self.sections.append(section)
        self.directories = [struct.unpack_from("<II", data, optional + 112 + i * 8)
                            for i in range(directory_count)]

    def require(self, offset: int, length: int) -> None:
        if offset < 0 or length < 0 or offset > len(self.data) or length > len(self.data) - offset:
            raise FormatError("truncated file range")

    def u16(self, offset: int) -> int:
        self.require(offset, 2)
        return struct.unpack_from("<H", self.data, offset)[0]

    def u32(self, offset: int) -> int:
        self.require(offset, 4)
        return struct.unpack_from("<I", self.data, offset)[0]

    def u64(self, offset: int) -> int:
        self.require(offset, 8)
        return struct.unpack_from("<Q", self.data, offset)[0]

    def mapped_range(self, rva: int, length: int = 1) -> tuple[int, int]:
        if rva < 0 or length < 1 or rva + length > 1 << 32:
            raise FormatError("invalid 32-bit RVA range")
        if rva < self.header_size:
            if rva + length > self.header_size:
                raise FormatError("RVA crosses header boundary")
            self.require(rva, length)
            return rva, self.header_size
        for section in self.sections:
            delta = rva - section["rva"]
            if 0 <= delta < max(section["virtual_size"], section["raw_size"]):
                if delta + length > section["raw_size"]:
                    raise FormatError("RVA reaches virtual-only or truncated section data")
                return section["raw_offset"] + delta, section["raw_offset"] + section["raw_size"]
        raise FormatError("unmapped RVA")

    def name(self, rva: int) -> tuple[str, int]:
        offset, end = self.mapped_range(rva)
        nul = self.data.find(b"\0", offset, min(end, offset + MAX_NAME_BYTES + 1))
        if nul < 0:
            raise FormatError("unterminated or oversized import name")
        value = self.data[offset:nul]
        if not value or any(c < 32 or c > 126 for c in value):
            raise FormatError("invalid ASCII import name")
        return value.decode("ascii"), offset

    def imports(self) -> list[dict]:
        if len(self.directories) < 2:
            return []
        rva, length = self.directories[1]
        if not rva and not length:
            return []
        if not rva or length < 20:
            raise FormatError("invalid import directory range")
        offset, _ = self.mapped_range(rva, length)
        result, symbols = [], 0
        for index in range(min(length // 20, MAX_IMPORT_DESCRIPTORS)):
            pos = offset + index * 20
            fields = struct.unpack_from("<IIIII", self.data, pos)
            if not any(fields):
                return result
            original, _, _, dll_rva, first = fields
            if not dll_rva or not first:
                raise FormatError("import descriptor lacks DLL name or IAT")
            dll, name_offset = self.name(dll_rva)
            thunk = original or first
            if not thunk:
                raise FormatError("missing import thunk table")
            entries = []
            table_offset, table_end = self.mapped_range(thunk, 8)
            table_rva_end = thunk + table_end - table_offset
            while True:
                if thunk + 8 > table_rva_end:
                    raise FormatError("unterminated thunk table within its mapped range")
                thunk_offset, _ = self.mapped_range(thunk, 8)
                value = self.u64(thunk_offset)
                if not value:
                    break
                symbols += 1
                if symbols > MAX_IMPORT_SYMBOLS:
                    raise FormatError("import symbol limit exceeded")
                if value & (1 << 63):
                    if value & 0x7FFFFFFFFFFF0000:
                        raise FormatError("invalid ordinal thunk reserved bits")
                    entries.append({"ordinal": value & 0xFFFF, "thunk_offset": hex(thunk_offset), "status": "seen"})
                else:
                    if value > 0xFFFFFFFF:
                        raise FormatError("import name RVA exceeds 32 bits")
                    self.mapped_range(value, 3)
                    symbol, symbol_offset = self.name(value + 2)
                    entries.append({"name": symbol, "name_offset": hex(symbol_offset),
                                    "thunk_offset": hex(thunk_offset), "status": "seen"})
                thunk += 8
            result.append({"dll": dll, "name_offset": hex(name_offset),
                           "descriptor_offset": hex(pos), "symbols": entries, "status": "seen"})
        raise FormatError("unterminated import descriptor table or descriptor limit exceeded")


def marker_inventory(data: bytes) -> list[dict]:
    result = []
    for marker_id, marker in MARKERS.items():
        offsets = [hex(m.start()) for m in re.finditer(re.escape(marker), data)]
        result.append({"id": marker_id, "status": "seen" if offsets else "unknown",
                       "offsets": offsets, "marker_sha256": digest(marker), "marker_bytes": len(marker)})
    return result


def asset_inventory(data: bytes) -> list[dict]:
    assets: dict[bytes, list[str]] = {}
    for match in ASSET_PATTERN.finditer(data):
        assets.setdefault(match[0], []).append(hex(match.start()))
    return [{"path_marker": name.decode("ascii"), "offsets": offsets,
             "marker_sha256": digest(name), "status": "seen",
             "payload_decoded": False} for name, offsets in sorted(assets.items())]


def analyze_bytes(data: bytes) -> dict:
    if not data.startswith(b"MZ"):
        raise FormatError("missing DOS MZ magic")
    if len(data) != EXPECTED_SIZE:
        raise FormatError(f"expected exactly {EXPECTED_SIZE} bytes, got {len(data)}")
    sha = digest(data)
    if sha != EXPECTED_SHA256:
        raise FormatError("SHA-256 does not match the frozen OpenRevo 0.8.5 executable")
    image = PEImage(data)
    return {"schema_version": 1, "status": "seen", "method": "static read only; no execution or payload export",
            "subject": {"sha256": sha, "bytes": len(data), "executed": False},
            "offset_convention": "zero-based absolute byte offsets in the frozen executable",
            "pe": {"machine": hex(image.machine), "sections": image.sections, "imports": image.imports()},
            "markers": marker_inventory(data), "asset_markers": asset_inventory(data),
            "limits": {"protocol_compatibility": "unknown", "hardware_behavior": "unknown",
                       "complete_backend_guard_coverage": "unknown", "windows_runtime": "unknown"}}


def write_exclusive(path: Path, payload: str) -> None:
    # Finish the file in a private directory on the same filesystem, then publish
    # through an atomic hard link. Existing destinations are never replaced or
    # removed, even when another writer creates one during publication.
    with tempfile.TemporaryDirectory(prefix=".analyze-openrevo-", dir=path.parent) as staging:
        staged = Path(staging) / "result.json"
        with staged.open("x", encoding="utf-8") as handle:
            os.chmod(staged, 0o600)
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.link(staged, path)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="unmodified open-revo.exe from the 0.8.5 archive")
    parser.add_argument("--output", type=Path, help="create a new metadata JSON file; existing paths are never overwritten")
    args = parser.parse_args(argv)
    try:
        with args.input.open("rb") as handle:
            data = handle.read(EXPECTED_SIZE + 1)
        report = analyze_bytes(data)
        payload = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
        if args.output is None:
            sys.stdout.write(payload)
        else:
            write_exclusive(args.output, payload)
        return 0
    except (OSError, FormatError) as error:
        print(f"analyze_openrevo: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
