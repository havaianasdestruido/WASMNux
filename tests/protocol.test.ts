import { describe, expect, it } from "vitest";
import {
  MAX_INPUT_BYTES,
  MAX_OUTPUT_BYTES,
  parseGuestEvent,
  parseGuestRequest,
} from "../src/protocol";

describe("worker message boundaries", () => {
  it("accepts bounded byte messages and rejects malformed or oversized requests", () => {
    expect(parseGuestRequest({ type: "start" })).toEqual({ type: "start" });
    expect(parseGuestRequest({ type: "output-consumed" })).toEqual({ type: "output-consumed" });
    expect(parseGuestRequest({ type: "input", bytes: new Uint8Array(1) })).not.toBeNull();
    for (const value of [
      null,
      "start",
      { type: "input", bytes: [] },
      { type: "input", bytes: new Uint8Array(0) },
      { type: "input", bytes: new Uint8Array(MAX_INPUT_BYTES + 1) },
      { type: "reset" },
    ]) {
      expect(parseGuestRequest(value)).toBeNull();
    }
  });

  it("validates worker events before the UI consumes them", () => {
    expect(parseGuestEvent({ type: "running" })).toEqual({ type: "running" });
    expect(parseGuestEvent({ type: "output", bytes: new Uint8Array(3) })).not.toBeNull();
    expect(
      parseGuestEvent({ type: "output", bytes: new Uint8Array(MAX_OUTPUT_BYTES + 1) }),
    ).toBeNull();
    expect(parseGuestEvent({ type: "error", message: "x".repeat(513) })).toBeNull();
    expect(parseGuestEvent({ type: "error", message: 42 })).toBeNull();
  });
});
