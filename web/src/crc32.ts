/**
 * CRC-32 (IEEE 802.3, reflected polynomial 0xEDB88320).
 * Identical algorithm to `wasmnux_core::crc::crc32` so disk chunks can be
 * verified on either side of the WASM boundary. Cross-checked by tests
 * using the standard "123456789" -> 0xCBF43926 vector.
 */

const POLYNOMIAL = 0xedb88320;

const TABLE: Uint32Array = (() => {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i += 1) {
    let crc = i;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = crc & 1 ? (crc >>> 1) ^ POLYNOMIAL : crc >>> 1;
    }
    table[i] = crc >>> 0;
  }
  return table;
})();

/** Computes the CRC-32 of `data` as an unsigned 32-bit number. */
export function crc32(data: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of data) {
    const index = ((crc ^ byte) & 0xff) as number;
    crc = ((crc >>> 8) ^ (TABLE[index] as number)) >>> 0;
  }
  return (crc ^ 0xffffffff) >>> 0;
}
