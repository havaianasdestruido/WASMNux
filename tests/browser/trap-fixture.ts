/** Minimal real RV64 ELF which executes an invalid 32-bit instruction. */
export function trapProgramCommand(): string {
  const elf = Buffer.alloc(124);
  elf.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1], 0); // ELF64, little endian
  elf.writeUInt16LE(2, 16); // ET_EXEC
  elf.writeUInt16LE(243, 18); // EM_RISCV
  elf.writeUInt32LE(1, 20);
  elf.writeBigUInt64LE(0x10078n, 24); // entry is the last word
  elf.writeBigUInt64LE(64n, 32); // program header offset
  elf.writeUInt16LE(64, 52); // ELF header size
  elf.writeUInt16LE(56, 54); // program header size
  elf.writeUInt16LE(1, 56); // one program header
  elf.writeUInt32LE(1, 64); // PT_LOAD
  elf.writeUInt32LE(5, 68); // read/execute
  elf.writeBigUInt64LE(0x10000n, 80); // virtual address
  elf.writeBigUInt64LE(0x10000n, 88); // physical address
  elf.writeBigUInt64LE(BigInt(elf.length), 96); // file size
  elf.writeBigUInt64LE(BigInt(elf.length), 104); // memory size
  elf.writeBigUInt64LE(0x1000n, 112); // alignment
  elf.writeUInt32LE(0xffffffff, 120); // guaranteed invalid/reserved instruction
  const escaped = Array.from(elf, (byte) => `\\0${byte.toString(8).padStart(3, "0")}`).join("");
  return `printf '%b' '${escaped}' >/tmp/wasmnux-trap; chmod 700 /tmp/wasmnux-trap; /tmp/wasmnux-trap`;
}
