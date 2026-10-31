/** Real RV64 userspace ELF: load a doubleword at a 4-byte (not 8-byte) boundary. */
export function unalignedLoadCommand(): string {
  const elf = Buffer.alloc(172);
  elf.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1], 0); // ELF64, little endian
  elf.writeUInt16LE(2, 16); // ET_EXEC
  elf.writeUInt16LE(243, 18); // EM_RISCV
  elf.writeUInt32LE(1, 20);
  elf.writeBigUInt64LE(0x10078n, 24); // instruction entry
  elf.writeBigUInt64LE(64n, 32);
  elf.writeUInt16LE(64, 52);
  elf.writeUInt16LE(56, 54);
  elf.writeUInt16LE(1, 56);
  elf.writeUInt32LE(1, 64); // PT_LOAD
  elf.writeUInt32LE(5, 68); // read/execute
  elf.writeBigUInt64LE(0x10000n, 80);
  elf.writeBigUInt64LE(0x10000n, 88);
  elf.writeBigUInt64LE(BigInt(elf.length), 96);
  elf.writeBigUInt64LE(BigInt(elf.length), 104);
  elf.writeBigUInt64LE(0x1000n, 112);
  // auipc t0,0; ld t1,44(t0); srli t1,t1,32; addi a0,t1,-1;
  // addi a7,zero,93 (Linux exit); ecall. Exit zero only if the high
  // word of the unaligned value was read at the correct bit position.
  [0x00000297, 0x02c2b303, 0x02035313, 0xfff30513, 0x05d00893, 0x00000073].forEach((word, i) => {
    elf.writeUInt32LE(word, 120 + i * 4);
  });
  elf.writeBigUInt64LE(0x1_0000_0000n, 164); // address 0x100a4, aligned to 4 only
  const escaped = Array.from(elf, (byte) => `\\0${byte.toString(8).padStart(3, "0")}`).join("");
  return `printf '%b' '${escaped}' >/tmp/wasmnux-memory; chmod 700 /tmp/wasmnux-memory; /tmp/wasmnux-memory`;
}
