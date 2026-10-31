import { describe, expect, it } from "vitest";

import { crc32 } from "../src/crc32.js";

describe("crc32", () => {
  it("matches the standard check vector", () => {
    expect(crc32(new TextEncoder().encode("123456789"))).toBe(0xcbf43926);
  });

  it("matches the vector shared with the Rust core", () => {
    // Same expectation as wasmnux-core crc tests: guards Rust/TS parity.
    expect(crc32(new TextEncoder().encode("wasmnux"))).toBe(0x4eeaca72);
  });

  it("handles empties and detects one-byte changes", () => {
    expect(crc32(new Uint8Array(0))).toBe(0);
    const a = crc32(new TextEncoder().encode("persistent-data"));
    const b = crc32(new TextEncoder().encode("persistent-datb"));
    expect(a).not.toBe(b);
  });
});
