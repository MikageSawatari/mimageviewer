"""Break down a Rust rlib (ar archive) by member kind and COFF section (sections 1.908-1.910).

Usage: python scripts/rlib_breakdown.py <path-to.rlib> [--top N]

Reports:
  * member totals: rustc metadata (lib.rmeta), COFF objects (*.o), other
  * for the COFF objects, bytes summed by section-name family
    (.debug$S / .debug$T = CodeView debug info, .text, .rdata, .data, .pdata/.xdata, other)
    plus symbol table + string table + headers ("coff overhead")
Only the standard library is used.
"""

from __future__ import annotations

import argparse
import struct
import sys
from collections import Counter
from pathlib import Path


def iter_ar_members(data: bytes):
    if not data.startswith(b"!<arch>\n"):
        raise ValueError("not an ar archive")
    pos = 8
    long_names = b""
    while pos + 60 <= len(data):
        header = data[pos : pos + 60]
        name = header[0:16].decode("ascii", "replace").rstrip()
        size = int(header[48:58].decode("ascii").strip())
        body_start = pos + 60
        body = data[body_start : body_start + size]
        if name == "//":
            long_names = body
        elif name.startswith("/") and name[1:].isdigit() and long_names:
            off = int(name[1:])
            # GNU tables end names with "/\n"; COFF (MSVC-style) tables with NUL.
            ends = [e for e in (long_names.find(b"\n", off), long_names.find(b"\0", off)) if e >= 0]
            end = min(ends) if ends else len(long_names)
            name = long_names[off:end].decode("utf-8", "replace").rstrip("/")
        elif name not in ("/", "/SYM64/", "__.SYMDEF", "__.SYMDEF SORTED"):
            name = name.rstrip("/")
        yield name, body_start, size
        pos = body_start + size + (size & 1)


def coff_sections(obj: memoryview):
    """Yield (section_name, raw_size) for a COFF object; returns overhead bytes via StopIteration value."""
    if len(obj) < 20:
        return None
    machine, nsec, _ts, symptr, nsym, opt, _ch = struct.unpack_from("<HHIIIHH", obj, 0)
    # Big-obj COFF (/bigobj) header starts with 0x0000 0xFFFF.
    if machine == 0 and nsec == 0xFFFF:
        (sig1, sig2, version, machine2, _ts2) = struct.unpack_from("<HHHHI", obj, 0)
        # ClassID (16 bytes) + SizeOfData + Flags + MetaDataSize + MetaDataOffset
        nsec, symptr, nsym = struct.unpack_from("<III", obj, 44)
        sec_off = 56
        sym_size = 20
    else:
        if machine not in (0x8664, 0x14C, 0xAA64):
            return None
        sec_off = 20 + opt
        sym_size = 18
    out = []
    for i in range(nsec):
        off = sec_off + i * 40
        raw_name = bytes(obj[off : off + 8]).rstrip(b"\0")
        size_raw = struct.unpack_from("<I", obj, off + 16)[0]
        name = raw_name.decode("ascii", "replace")
        if name.startswith("/"):
            name = "(long)"
        out.append((name, size_raw))
    return out


def family(name: str) -> str:
    if name.startswith(".debug$"):
        return name  # .debug$S / .debug$T / .debug$P / .debug$H
    if name.startswith(".debug"):
        return ".debug(dwarf)"
    for prefix in (".text", ".rdata", ".data", ".bss", ".pdata", ".xdata", ".tls", ".CRT", ".llvm"):
        if name.startswith(prefix):
            return prefix
    return "other"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("rlib", type=Path)
    ap.add_argument("--top", type=int, default=0, help="also list the N largest members")
    args = ap.parse_args()
    data = args.rlib.read_bytes()
    view = memoryview(data)
    gib = 1024**3
    total = len(data)
    kinds = Counter()
    counts = Counter()
    sections = Counter()
    section_total = 0
    members = []
    for name, start, size in iter_ar_members(data):
        members.append((size, name))
        if name == "lib.rmeta" or name.endswith(".rmeta"):
            kinds["metadata (rmeta)"] += size
            counts["metadata (rmeta)"] += 1
        elif name.endswith(".o") or name.endswith(".obj"):
            secs = coff_sections(view[start : start + size])
            if secs is None:
                kinds["object (non-COFF)"] += size
                counts["object (non-COFF)"] += 1
                continue
            kinds["COFF objects"] += size
            counts["COFF objects"] += 1
            for sname, ssize in secs:
                sections[family(sname)] += ssize
                section_total += ssize
            sections["coff overhead (headers, symbols, strings, relocs)"] += 0
        elif name in ("/", "/SYM64/", "//"):
            kinds["ar index / name table"] += size
            counts["ar index / name table"] += 1
        else:
            kinds["other members"] += size
            counts["other members"] += 1
    print(f"rlib: {args.rlib}")
    print(f"total: {total / gib:.3f} GiB ({total} bytes)")
    print("members by kind:")
    for k, v in kinds.most_common():
        print(f"  {k:28s} {v / gib:8.3f} GiB  {100 * v / total:5.1f}%  ({counts[k]} members)")
    obj_total = kinds.get("COFF objects", 0)
    if obj_total:
        overhead = obj_total - section_total
        sections["coff overhead (headers, symbols, strings, relocs)"] = overhead
        print("COFF object bytes by section family (raw data):")
        for k, v in sections.most_common():
            print(f"  {k:50s} {v / gib:8.3f} GiB  {100 * v / total:5.1f}% of rlib")
    if args.top:
        print(f"largest {args.top} members:")
        for size, name in sorted(members, reverse=True)[: args.top]:
            print(f"  {size / (1024**2):10.1f} MiB  {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
