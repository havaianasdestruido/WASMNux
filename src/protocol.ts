export const MAX_INPUT_BYTES = 4096;
export const MAX_OUTPUT_BYTES = 32768;

export type GuestRequest =
  | { type: "start" }
  | { type: "stop" }
  | { type: "output-consumed" }
  | { type: "input"; bytes: Uint8Array };

export type GuestEvent =
  | { type: "starting" }
  | { type: "running" }
  | { type: "stopped" }
  | { type: "output"; bytes: Uint8Array }
  | { type: "error"; message: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object";
}

export function parseGuestRequest(value: unknown): GuestRequest | null {
  if (!isRecord(value)) return null;
  if (value.type === "start" || value.type === "stop" || value.type === "output-consumed") {
    return { type: value.type };
  }
  if (
    value.type === "input" &&
    value.bytes instanceof Uint8Array &&
    value.bytes.byteLength > 0 &&
    value.bytes.byteLength <= MAX_INPUT_BYTES
  ) {
    return { type: "input", bytes: value.bytes };
  }
  return null;
}

export function parseGuestEvent(value: unknown): GuestEvent | null {
  if (!isRecord(value)) return null;
  if (value.type === "starting" || value.type === "running" || value.type === "stopped") {
    return { type: value.type };
  }
  if (
    value.type === "output" &&
    value.bytes instanceof Uint8Array &&
    value.bytes.byteLength > 0 &&
    value.bytes.byteLength <= MAX_OUTPUT_BYTES
  ) {
    return { type: "output", bytes: value.bytes };
  }
  if (value.type === "error" && typeof value.message === "string" && value.message.length <= 512) {
    return { type: "error", message: value.message };
  }
  return null;
}
