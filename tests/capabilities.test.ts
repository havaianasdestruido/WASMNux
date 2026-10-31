import { afterEach, describe, expect, it, vi } from "vitest";
import { detectCapabilities, queryPersistence } from "../src/capabilities";

afterEach(() => vi.unstubAllGlobals());

describe("browser capability detection", () => {
  it("detects unsupported APIs without throwing", async () => {
    vi.stubGlobal("navigator", undefined);
    vi.stubGlobal("Worker", undefined);
    vi.stubGlobal("indexedDB", undefined);
    expect(detectCapabilities()).toMatchObject({
      worker: false,
      indexedDB: false,
      opfs: false,
      webGPU: false,
      persistentStorage: false,
      sharedArrayBuffer: false,
    });
    expect(await queryPersistence()).toBe("unavailable");
  });

  it("does not claim WebGPU is available when its property is undefined", () => {
    vi.stubGlobal("navigator", { gpu: undefined });
    expect(detectCapabilities().webGPU).toBe(false);
  });

  it("distinguishes persistence from best effort and errors", async () => {
    const persisted = vi.fn<() => Promise<boolean>>();
    vi.stubGlobal("navigator", { storage: { persisted } });
    persisted
      .mockResolvedValueOnce(true)
      .mockResolvedValueOnce(false)
      .mockRejectedValueOnce(new Error("denied"));
    expect(await queryPersistence()).toBe("persisted");
    expect(await queryPersistence()).toBe("best-effort");
    expect(await queryPersistence()).toBe("error");
  });
});
