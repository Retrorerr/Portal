#!/usr/bin/env python3
"""Fail when any ELF file under the given paths cannot load on 16 KB pages.

Android 15+ devices may run 16 KB page kernels, and Google Play rejects apps
whose native libraries are not 16 KB compatible. A PT_LOAD segment is
compatible when its p_align is at least 16 KB and its file offset and virtual
address agree modulo 16 KB (the kernel and the dynamic linker map whole pages).

Unlike check_elf_alignment.sh, this inspects every LOAD segment (not only the
first) and exits non-zero on failure, so CI can gate on it.

    scripts/check_elf_page_size.py assets
"""

import struct
import sys
from pathlib import Path

PAGE = 16 * 1024
PT_LOAD = 1


def load_segments(data):
    """Yield (p_offset, p_vaddr, p_align) for each PT_LOAD, or None if not ELF."""
    if data[:4] != b"\x7fELF":
        return None
    elf_class, endian = data[4], data[5]
    order = "<" if endian == 1 else ">"
    if elf_class == 2:
        phoff, = struct.unpack_from(order + "Q", data, 0x20)
        phentsize, phnum = struct.unpack_from(order + "HH", data, 0x36)
        layout = order + "IIQQQQQQ"
        fields = lambda entry: (entry[0], entry[2], entry[3], entry[7])
    else:
        phoff, = struct.unpack_from(order + "I", data, 0x1C)
        phentsize, phnum = struct.unpack_from(order + "HH", data, 0x2A)
        layout = order + "IIIIIIII"
        fields = lambda entry: (entry[0], entry[1], entry[2], entry[7])
    segments = []
    for index in range(phnum):
        entry = struct.unpack_from(layout, data, phoff + index * phentsize)
        p_type, p_offset, p_vaddr, p_align = fields(entry)
        if p_type == PT_LOAD:
            segments.append((p_offset, p_vaddr, p_align))
    return segments


def problems(segments):
    for p_offset, p_vaddr, p_align in segments:
        if p_align < PAGE:
            yield f"LOAD at offset {p_offset:#x} is aligned to {p_align:#x}"
        elif (p_vaddr - p_offset) % PAGE:
            yield f"LOAD at offset {p_offset:#x} maps to {p_vaddr:#x}, not congruent modulo 16 KB"


def main(paths):
    if not paths:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    checked = failed = 0
    for root in map(Path, paths):
        files = sorted(p for p in root.rglob("*") if p.is_file()) if root.is_dir() else [root]
        for path in files:
            with path.open("rb") as handle:
                header = handle.read(4)
                if header != b"\x7fELF":
                    continue
                data = header + handle.read()
            segments = load_segments(data)
            checked += 1
            found = list(problems(segments))
            for problem in found:
                print(f"{path}: {problem}")
            failed += bool(found)
    print(f"{checked} ELF files checked, {failed} not 16 KB compatible")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
