/**
 * Helper constructing a minimal authentic RISC-V Linux `Image` (`RSC\x05` header)
 * that prints an ASCII banner via SBI Putchar, paints a pixel in SimpleFB row 4,
 * and invokes SBI `SRST` (`PowerOff`). Used for deterministic WASM + Web Worker smoke verification.
 */

export function buildRiscvMiniSmokeKernel(banner: string): Uint8Array {
  const insns: number[] = [];
  const addi = (rd: number, rs1: number, imm: number): number =>
    (((imm & 0xfff) << 20) | (rs1 << 15) | (rd << 7) | 0x13) >>> 0;
  const lui = (rd: number, imm20: number): number =>
    (((imm20 & 0xfffff) << 12) | (rd << 7) | 0x37) >>> 0;
  const sw = (rs2: number, rs1: number, imm: number): number => {
    const u = imm & 0xfff;
    return (
      (((u >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (2 << 12) | ((u & 0x1f) << 7) | 0x23) >>> 0
    );
  };

  for (let i = 0; i < banner.length; i += 1) {
    const ch = banner.charCodeAt(i) & 0xff;
    insns.push(addi(10, 0, ch)); // a0 = char
    insns.push(addi(17, 0, 1)); // a7 = 1 (legacy console putchar)
    insns.push(0x0000_0073); // ecall
  }

  // Paint 1 pixel at SimpleFB row 4 (0x5000_0000 + 4 * 640 * 4 = 0x5000_2800)
  insns.push(lui(9, 0x50003));
  insns.push(addi(9, 9, -0x800));
  insns.push(lui(8, 0x0034d));
  insns.push(sw(8, 9, 0));

  // SBI v2.0 SRST PowerOff: a7 = 0x53525354 ("SRST"), a6 = 0, a0 = 0
  insns.push(lui(17, 0x53525));
  insns.push(addi(17, 17, 0x354));
  insns.push(addi(16, 0, 0));
  insns.push(addi(10, 0, 0));
  insns.push(0x0000_0073);

  const totalBytes = Math.max(256, 64 + insns.length * 4);
  const image = new Uint8Array(totalBytes);
  const dv = new DataView(image.buffer);

  dv.setUint32(0, 0x0400_006f, true); // j +64 (jump past 64-byte RISC-V Linux Image header)
  dv.setBigUint64(8, 0x0020_0000n, true); // text_offset = 2 MiB
  dv.setBigUint64(16, BigInt(Math.max(4096, totalBytes)), true); // image_size
  dv.setUint32(56, 0x0543_5352, true); // "RSC\x05"

  for (let i = 0; i < insns.length; i += 1) {
    const word = insns[i] ?? 0;
    dv.setUint32(64 + i * 4, word, true);
  }
  return image;
}
