import { describe, expect, it } from "vitest";

import { assertSafeKey } from "../src/storage/chunkStore.js";
import { parseGuestManifest, parseProfileId, PROFILE_IDS, PROFILES } from "../src/profiles.js";

describe("profiles", () => {
  it("defines exactly the two isolated profiles", () => {
    expect(PROFILE_IDS).toEqual(["vanilla", "Debian"]);
    expect(PROFILES.vanilla.diskId).not.toBe(PROFILES.Debian.diskId);
    expect(PROFILES.vanilla.assetPath).not.toBe(PROFILES.Debian.assetPath);
  });

  it("parseProfileId only accepts known ids", () => {
    expect(parseProfileId("vanilla")).toBe("vanilla");
    expect(parseProfileId("Debian")).toBe("Debian");
    expect(parseProfileId("arch")).toBeUndefined();
    expect(parseProfileId(42)).toBeUndefined();
    expect(parseProfileId(undefined)).toBeUndefined();
  });

  it("parseGuestManifest accepts a well-formed manifest", () => {
    const manifest = parseGuestManifest(
      {
        profile: "vanilla",
        kernelVersion: "7.3.1",
        kernelImage: "Image.gz",
        firmwareImage: "fw_jump.elf",
        initrdImage: "initramfs.cpio.gz",
        builtAt: "2026-10-31T00:00:00Z",
      },
      "vanilla",
    );
    expect(manifest.kernelVersion).toBe("7.3.1");
    expect(manifest.firmwareImage).toBe("fw_jump.elf");
    expect(manifest.diskImage).toBeUndefined();
  });

  it("parseGuestManifest rejects wrong profiles, missing fields and paths", () => {
    expect(() =>
      parseGuestManifest(
        { profile: "Debian", kernelVersion: "7", kernelImage: "k", builtAt: "now" },
        "vanilla",
      ),
    ).toThrow(/profile mismatch/);
    expect(() =>
      parseGuestManifest({ profile: "vanilla", kernelVersion: "7", builtAt: "now" }, "vanilla"),
    ).toThrow(/kernelImage/);
    expect(() =>
      parseGuestManifest(
        {
          profile: "vanilla",
          kernelVersion: "7",
          kernelImage: "../../etc/passwd",
          builtAt: "now",
        },
        "vanilla",
      ),
    ).toThrow(/bare filename/);
    expect(() => parseGuestManifest("nope", "vanilla")).toThrow(/not an object/);
  });

  it("assertSafeKey blocks traversal keys at the storage boundary", () => {
    expect(assertSafeKey("disks/a/chunks/1")).toBe("disks/a/chunks/1");
    expect(() => assertSafeKey("../x")).toThrow();
    expect(() => assertSafeKey("/abs")).toThrow();
    expect(() => assertSafeKey("a//b")).toThrow();
    expect(() => assertSafeKey("a/../b")).toThrow();
    expect(() => assertSafeKey("a\\b")).toThrow();
  });
});
