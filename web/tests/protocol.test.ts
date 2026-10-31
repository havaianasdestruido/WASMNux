import { describe, expect, it } from "vitest";

import { generateTestPattern, swizzleXrgbToRgba } from "../src/gpu/renderer.js";
import { parseGuestAssetsState } from "../src/assets.js";
import { probeGuestAssets } from "../src/assets.js";
import { PROFILES } from "../src/profiles.js";
import { parseWorkerEvent, parseWorkerRequest } from "../src/vm/protocol.js";

describe("worker protocol validation", () => {
  it("accepts a full boot request and rejects tampered ones", () => {
    const good = parseWorkerRequest({
      type: "boot",
      requestId: 1,
      profile: "vanilla",
      ramMiB: 256,
      bootargs: "console=ttyS0",
      bootMode: "firmware",
      kernel: new Uint8Array([1, 2, 3]),
      firmware: new Uint8Array([4]),
      defaultDiskMiB: 256,
    });
    expect(good.type).toBe("boot");

    expect(() => parseWorkerRequest(null)).toThrow();
    expect(() => parseWorkerRequest({ type: "eraseEverything" })).toThrow(/unknown/);
    expect(() =>
      parseWorkerRequest({
        type: "boot",
        requestId: 1,
        profile: "arch",
        ramMiB: 1,
        bootargs: "",
        bootMode: "firmware",
        kernel: new Uint8Array(1),
        defaultDiskMiB: 1,
      }),
    ).toThrow(/profile/);
    expect(() =>
      parseWorkerRequest({
        type: "boot",
        requestId: 1,
        profile: "vanilla",
        ramMiB: 1,
        bootargs: "",
        bootMode: "sideways",
        kernel: new Uint8Array(1),
        defaultDiskMiB: 1,
      }),
    ).toThrow(/boot mode/);
    expect(() => parseWorkerRequest({ type: "serialInput", bytes: "ls\n" })).toThrow(/Uint8Array/);
    expect(() => parseWorkerRequest({ type: "pause" })).toThrow(/requestId/);
  });

  it("accepts worker events and rejects malformed ones", () => {
    expect(parseWorkerEvent({ type: "ready", coreVersion: "0.1.0" }).type).toBe("ready");
    expect(
      parseWorkerEvent({ type: "stats", instructions: 5, pcHex: "0x80000000", state: "running" })
        .type,
    ).toBe("stats");
    expect(() => parseWorkerEvent({ type: "state", state: "vibing" })).toThrow(/machine state/);
    expect(() => parseWorkerEvent({ type: "serialOutput", bytes: [65] })).toThrow(/Uint8Array/);
    expect(() => parseWorkerEvent({ type: "ready" })).toThrow(/coreVersion/);
    expect(() => parseWorkerEvent(42)).toThrow();
  });
});

describe("guest asset probing", () => {
  it("reports missing assets with build instructions instead of faking readiness", async () => {
    const state = await probeGuestAssets(PROFILES.vanilla, () =>
      Promise.resolve({
        ok: false,
        status: 404,
        json: () => Promise.reject(new Error("no body")),
        arrayBuffer: () => Promise.resolve(new ArrayBuffer(0)),
      }),
    );
    expect(state.kind).toBe("missing");
    if (state.kind === "missing") {
      expect(state.detail).toMatch(/guest\/scripts/);
    }
  });

  it("accepts a real manifest and flags corrupted ones", async () => {
    const manifest = {
      profile: "Debian",
      kernelVersion: "7.3.1",
      kernelImage: "Image",
      diskImage: "debian-trixie-riscv64.img",
      builtAt: "2026-10-31T00:00:00Z",
    };
    const okState = await probeGuestAssets(PROFILES.Debian, () =>
      Promise.resolve({
        ok: true,
        status: 200,
        json: () => Promise.resolve(manifest),
        arrayBuffer: () => Promise.resolve(new ArrayBuffer(0)),
      }),
    );
    expect(okState.kind).toBe("ready");

    const badState = await probeGuestAssets(PROFILES.Debian, () =>
      Promise.resolve({
        ok: true,
        status: 200,
        json: () => Promise.resolve({ profile: "Debian" }),
        arrayBuffer: () => Promise.resolve(new ArrayBuffer(0)),
      }),
    );
    expect(badState.kind).toBe("invalid");
  });

});

describe("framebuffer pixel conversion", () => {
  it("swizzles x8r8g8b8 to RGBA exactly", () => {
    // One pixel: bytes [B=0x10, G=0x20, R=0x30, X=0x00] -> RGBA(0x30,0x20,0x10,255)
    const rgba = swizzleXrgbToRgba({
      width: 1,
      height: 1,
      stride: 4,
      pixels: new Uint8Array([0x10, 0x20, 0x30, 0x00]),
    });
    expect([...rgba]).toEqual([0x30, 0x20, 0x10, 0xff]);
  });

  it("respects row stride padding", () => {
    const frame = generateTestPattern(16, 8);
    const padded = { ...frame, stride: 16 * 4 + 8, pixels: new Uint8Array((16 * 4 + 8) * 8) };
    for (let y = 0; y < 8; y += 1) {
      padded.pixels.set(
        frame.pixels.subarray(y * frame.stride, (y + 1) * frame.stride),
        y * padded.stride,
      );
    }
    expect(swizzleXrgbToRgba(padded)).toEqual(swizzleXrgbToRgba(frame));
  });

  it("test pattern is deterministic with the documented bar colours", () => {
    const first = generateTestPattern(64, 32);
    const second = generateTestPattern(64, 32);
    expect(first.pixels).toEqual(second.pixels);
    // First bar is white: B=G=R=255.
    expect([first.pixels[0], first.pixels[1], first.pixels[2]]).toEqual([255, 255, 255]);
  });
});
