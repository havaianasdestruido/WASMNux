/**
 * Typed message protocol between the UI thread and the VM worker.
 * Both ends runtime-validate what they receive: a structured-clone payload
 * is *external input* (a buggy or hostile sender must not be able to smuggle
 * malformed objects into the VM driver). Parsers throw on any mismatch.
 */

import { parseProfileId, type ProfileId } from "../profiles.js";

export type BootModeName = "firmware" | "direct-sbi";

export interface BootRequest {
  readonly type: "boot";
  readonly requestId: number;
  readonly profile: ProfileId;
  readonly ramMiB: number;
  readonly bootargs: string;
  readonly bootMode: BootModeName;
  readonly kernel: Uint8Array;
  readonly firmware?: Uint8Array;
  readonly initrd?: Uint8Array;
  /** Existing persistent disk image, or undefined to create defaultDiskMiB. */
  readonly disk?: Uint8Array;
  readonly defaultDiskMiB: number;
}

export type WorkerRequest =
  | BootRequest
  | { readonly type: "pause" | "resume" | "powerOff"; readonly requestId: number }
  | { readonly type: "serialInput"; readonly bytes: Uint8Array }
  | { readonly type: "diskPersisted" }
  | { readonly type: "framebufferTick" };

export type MachineStateName = "created" | "running" | "paused" | "halted";

export type WorkerEvent =
  | { readonly type: "ready"; readonly coreVersion: string }
  | { readonly type: "wasmUnavailable"; readonly reason: string }
  | { readonly type: "booted"; readonly requestId: number; readonly state: MachineStateName }
  | {
      readonly type: "state";
      readonly state: MachineStateName;
      readonly requestId?: number;
      readonly reason?: string;
    }
  | { readonly type: "serialOutput"; readonly bytes: Uint8Array }
  | {
      readonly type: "framebuffer";
      readonly width: number;
      readonly height: number;
      readonly stride: number;
      readonly pixels: Uint8Array;
    }
  | { readonly type: "diskImage"; readonly bytes: Uint8Array }
  | {
      readonly type: "stats";
      readonly instructions: number;
      /** u64 PC does not fit a JS number: hex string. */
      readonly pcHex: string;
      readonly state: MachineStateName;
    }
  | { readonly type: "error"; readonly requestId?: number; readonly message: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function reqString(record: Record<string, unknown>, field: string): string {
  const value = record[field];
  if (typeof value !== "string") {
    throw new Error(`protocol field "${field}" must be a string`);
  }
  return value;
}

function reqNumber(record: Record<string, unknown>, field: string): number {
  const value = record[field];
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`protocol field "${field}" must be a finite number`);
  }
  return value;
}

function reqBytes(record: Record<string, unknown>, field: string): Uint8Array {
  const value = record[field];
  if (!(value instanceof Uint8Array)) {
    throw new Error(`protocol field "${field}" must be a Uint8Array`);
  }
  return value;
}

function optBytes(record: Record<string, unknown>, field: string): Uint8Array | undefined {
  return record[field] === undefined ? undefined : reqBytes(record, field);
}

function reqState(value: unknown): MachineStateName {
  if (value === "created" || value === "running" || value === "paused" || value === "halted") {
    return value;
  }
  throw new Error(`invalid machine state: ${JSON.stringify(value)}`);
}

/** Validates an untrusted payload as a WorkerRequest (worker side). */
export function parseWorkerRequest(value: unknown): WorkerRequest {
  if (!isRecord(value)) {
    throw new Error("worker request must be an object");
  }
  const type = value["type"];
  switch (type) {
    case "boot": {
      const profile = parseProfileId(value["profile"]);
      if (profile === undefined) {
        throw new Error("boot request carries an unknown profile id");
      }
      const bootMode = value["bootMode"];
      if (bootMode !== "firmware" && bootMode !== "direct-sbi") {
        throw new Error(`invalid boot mode: ${JSON.stringify(bootMode)}`);
      }
      const firmware = optBytes(value, "firmware");
      const initrd = optBytes(value, "initrd");
      const disk = optBytes(value, "disk");
      return {
        type: "boot",
        requestId: reqNumber(value, "requestId"),
        profile,
        ramMiB: reqNumber(value, "ramMiB"),
        bootargs: reqString(value, "bootargs"),
        bootMode,
        kernel: reqBytes(value, "kernel"),
        defaultDiskMiB: reqNumber(value, "defaultDiskMiB"),
        ...(firmware !== undefined ? { firmware } : {}),
        ...(initrd !== undefined ? { initrd } : {}),
        ...(disk !== undefined ? { disk } : {}),
      };
    }
    case "pause":
    case "resume":
    case "powerOff":
      return { type, requestId: reqNumber(value, "requestId") };
    case "serialInput":
      return { type: "serialInput", bytes: reqBytes(value, "bytes") };
    case "diskPersisted":
      return { type: "diskPersisted" };
    case "framebufferTick":
      return { type: "framebufferTick" };
    default:
      throw new Error(`unknown worker request type: ${JSON.stringify(type)}`);
  }
}

/** Validates an untrusted payload as a WorkerEvent (UI side). */
export function parseWorkerEvent(value: unknown): WorkerEvent {
  if (!isRecord(value)) {
    throw new Error("worker event must be an object");
  }
  const type = value["type"];
  switch (type) {
    case "ready":
      return { type: "ready", coreVersion: reqString(value, "coreVersion") };
    case "wasmUnavailable":
      return { type: "wasmUnavailable", reason: reqString(value, "reason") };
    case "booted":
      return {
        type: "booted",
        requestId: reqNumber(value, "requestId"),
        state: reqState(value["state"]),
      };
    case "state": {
      const event: Extract<WorkerEvent, { type: "state" }> = {
        type: "state",
        state: reqState(value["state"]),
        ...(value["requestId"] !== undefined
          ? { requestId: reqNumber(value, "requestId") }
          : {}),
        ...(value["reason"] !== undefined ? { reason: reqString(value, "reason") } : {}),
      };
      return event;
    }
    case "serialOutput":
      return { type: "serialOutput", bytes: reqBytes(value, "bytes") };
    case "framebuffer":
      return {
        type: "framebuffer",
        width: reqNumber(value, "width"),
        height: reqNumber(value, "height"),
        stride: reqNumber(value, "stride"),
        pixels: reqBytes(value, "pixels"),
      };
    case "diskImage":
      return { type: "diskImage", bytes: reqBytes(value, "bytes") };
    case "stats":
      return {
        type: "stats",
        instructions: reqNumber(value, "instructions"),
        pcHex: reqString(value, "pcHex"),
        state: reqState(value["state"]),
      };
    case "error": {
      return {
        type: "error",
        message: reqString(value, "message"),
        ...(value["requestId"] !== undefined
          ? { requestId: reqNumber(value, "requestId") }
          : {}),
      };
    }
    default:
      throw new Error(`unknown worker event type: ${JSON.stringify(type)}`);
  }
}
