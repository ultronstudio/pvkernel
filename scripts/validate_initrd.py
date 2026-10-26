#!/usr/bin/env python3
"""Validate the generated Boot: CPIO volume and its static pvOS init ELF."""

from __future__ import annotations

import pathlib
import struct
import sys


def fail(message: str) -> None:
    raise SystemExit(f"initrd validation failed: {message}")


def aligned4(offset: int) -> int:
    return (offset + 3) & ~3


def parse(data: bytes) -> dict[str, tuple[int, bytes]]:
    entries: dict[str, tuple[int, bytes]] = {}
    offset = 0
    while True:
        end = offset + 110
        if end > len(data):
            fail("truncated newc header or missing trailer")
        header = data[offset:end]
        if header[:6] != b"070701":
            fail(f"bad newc magic at offset {offset}")
        try:
            mode = int(header[14:22], 16)
            size = int(header[54:62], 16)
            name_size = int(header[94:102], 16)
        except ValueError:
            fail(f"invalid hexadecimal field at offset {offset}")
        name_end = end + name_size
        if name_size < 1 or name_end > len(data):
            fail("filename exceeds archive")
        raw_name = data[end:name_end]
        if raw_name[-1:] != b"\0" or b"\0" in raw_name[:-1]:
            fail("filename is not NUL terminated")
        try:
            name = raw_name[:-1].decode("utf-8")
        except UnicodeDecodeError:
            fail("filename is not UTF-8")
        data_start = aligned4(name_end)
        data_end = data_start + size
        record_end = aligned4(data_end)
        if record_end > len(data) or any(data[name_end:data_start]) or any(data[data_end:record_end]):
            fail("invalid padding or file data bounds")
        payload = data[data_start:data_end]
        if name == "TRAILER!!!":
            if size != 0 or any(data[record_end:]):
                fail("invalid trailer or nonzero archive tail")
            return entries
        if name.startswith("/") or any(part in ("", ".", "..") for part in name.split("/")):
            fail(f"noncanonical path {name!r}")
        if name in entries:
            fail(f"duplicate path {name!r}")
        kind = mode & 0o170000
        if kind not in (0o040000, 0o100000):
            fail(f"unsupported node type at {name!r}")
        entries[name] = (kind, payload)
        offset = record_end


def main() -> None:
    if len(sys.argv) != 2:
        fail("usage: validate_initrd.py ARCHIVE")
    archive = pathlib.Path(sys.argv[1]).read_bytes()
    entries = parse(archive)
    for directory in ("pvOS", "pvOS/System"):
        if entries.get(directory, (None, b""))[0] != 0o040000:
            fail(f"archive does not contain directory {directory}")
    if "bin/init" in entries:
        fail("legacy Unix-style bin/init path is not accepted")
    entry = entries.get("pvOS/System/Init")
    if entry is None:
        fail(r"archive does not contain pvOS/System/Init (Boot:\pvOS\System\Init)")
    mode, elf = entry
    if mode != 0o100000:
        fail("Boot init is not a regular file")
    if len(elf) < 64 or elf[:7] != b"\x7fELF\x02\x01\x01":
        fail("Boot init is not a little-endian ELF64 file")
    e_type, machine = struct.unpack_from("<HH", elf, 16)
    if e_type != 2 or machine != 62:
        fail("Boot init is not x86_64 ET_EXEC")
    phoff = struct.unpack_from("<Q", elf, 32)[0]
    phentsize, phnum = struct.unpack_from("<HH", elf, 54)
    if phentsize != 56 or phoff + phentsize * phnum > len(elf):
        fail("invalid Boot init program header table")
    entry = struct.unpack_from("<Q", elf, 24)[0]
    executable_entry = False
    for index in range(phnum):
        at = phoff + index * phentsize
        p_type = struct.unpack_from("<I", elf, at)[0]
        if p_type in (2, 3, 7):
            fail("Boot init requests dynamic linking, an interpreter, or TLS")
        if p_type != 1:
            continue
        flags = struct.unpack_from("<I", elf, at + 4)[0]
        file_offset, virtual_address = struct.unpack_from("<QQ", elf, at + 8)
        file_size, memory_size = struct.unpack_from("<QQ", elf, at + 32)
        if file_offset + file_size > len(elf) or file_size > memory_size:
            fail("invalid Boot init PT_LOAD file or memory bounds")
        if virtual_address < 0x00400000 or virtual_address + memory_size > 0x0000400000000000:
            fail("Boot init PT_LOAD is outside the existing loader's user range")
        if flags & 0x2 and flags & 0x1:
            fail("Boot init PT_LOAD violates W^X")
        if flags & 0x1 and virtual_address <= entry < virtual_address + memory_size:
            executable_entry = True
    if not executable_entry:
        fail("Boot init entry point is not within an executable PT_LOAD")
    print(
        f"validated Boot: newc volume: {len(entries)} entries; "
        f"pvOS/System/Init is static x86_64 ET_EXEC ({len(elf)} bytes)"
    )


if __name__ == "__main__":
    main()
