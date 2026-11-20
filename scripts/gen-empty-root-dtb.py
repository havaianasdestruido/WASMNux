#!/usr/bin/env python3
"""Compile drivers/of/empty_root.dts to a DTB without invoking dtc.

`CONFIG_OF_FLATTREE` makes drivers/of/Makefile pull in `empty_root.dtb.o`, so
the kernel cannot link without a compiled `empty_root.dtb` even when no board
device tree is in use. dtc itself is a host tool that needs flex and bison, so on
a host without them the build stops dead at `No rule to make target
'drivers/of/empty_root.dtb.o'`.

There is exactly one input (`drivers/of/empty_root.dts`, ten lines) and it is a
fixed shape: a root node with `#address-cells = <2>` and `#size-cells = <2>`.
Encoding it directly against the Devicetree Specification v0.4 layout is both
smaller and far less error-prone than porting a dts lexer.

Usage:
  gen-empty-root-dtb.py <out.dtb>

The result is combined with post-processing by the kernel's own
`cmd_dt_S_dtb`, so the bytes must be a valid FDT blob -- which this function
asserts before writing.
"""

import struct
import sys

FDT_MAGIC = 0xD00DFEED
FDT_BEGIN_NODE = 0x1
FDT_END_NODE = 0x2
FDT_PROP = 0x3
FDT_END = 0x9
FDT_VERSION = 17
FDT_LAST_COMP_VERSION = 16


def align4(buf):
    pad = (-len(buf)) % 4
    return buf + b"\x00" * pad


def build_fdt():
    # --- structure block ---------------------------------------------------
    strings = []          # property name table
    struct_block = bytearray()

    def begin_node(name):
        struct_block.extend(struct.pack(">I", FDT_BEGIN_NODE))
        struct_block.extend(align4(name.encode() + b"\x00"))

    def prop(name, value):
        if name not in strings:
            strings.append(name)
        nameoff = 0
        for s in strings[:-1]:
            nameoff += len(s) + 1
        struct_block.extend(struct.pack(">III", FDT_PROP, len(value), nameoff))
        struct_block.extend(align4(value))

    def end_node():
        struct_block.extend(struct.pack(">I", FDT_END_NODE))

    begin_node("")
    prop("#address-cells", struct.pack(">I", 2))
    prop("#size-cells", struct.pack(">I", 2))
    end_node()
    struct_block.extend(struct.pack(">I", FDT_END))

    strings_block = b"".join(s.encode() + b"\x00" for s in strings)
    memrsv = struct.pack(">QQ", 0, 0)          # single empty terminator entry

    off_memrsv = 40                            # header is 40 bytes
    off_struct = off_memrsv + len(memrsv)
    off_strings = off_struct + len(struct_block)

    header = struct.pack(
        ">10I",
        FDT_MAGIC,
        off_struct + len(struct_block) + len(strings_block),  # totalsize
        off_struct,
        off_strings,
        off_memrsv,
        FDT_VERSION,
        FDT_LAST_COMP_VERSION,
        0,                                     # boot_cpuid_phys
        len(strings_block),
        len(struct_block),
    )

    return bytes(header) + memrsv + bytes(struct_block) + strings_block


def verify(blob):
    magic, totalsize, off_struct, off_strings, off_rsvmap, version, comp, boot_cpu, size_strings, size_struct = struct.unpack_from(">10I", blob, 0)
    assert magic == FDT_MAGIC, "bad FDT magic"
    assert totalsize == len(blob), "totalsize %d != %d" % (totalsize, len(blob))
    assert size_strings and size_struct, "empty structure/strings blocks"
    assert blob[off_struct:off_struct + 4] == struct.pack(">I", FDT_BEGIN_NODE), "structure block must open with BEGIN_NODE"
    assert struct.pack(">I", FDT_END) in blob[off_struct:off_struct + size_struct], "missing FDT_END"


def main():
    out = sys.argv[1]
    blob = build_fdt()
    verify(blob)
    with open(out, "wb") as fh:
        fh.write(blob)
    print("[gen-empty-root-dtb] wrote %s (%d bytes)" % (out, len(blob)))


if __name__ == "__main__":
    main()
