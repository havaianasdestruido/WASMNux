/// <reference lib="webworker" />
import { DiagnosticEmulator } from "./diagnostic-wasm";
import { MAX_OUTPUT_BYTES, parseGuestRequest, type GuestEvent } from "./protocol";

// This module is only instantiated by `new Worker`; it has no DOM access.
const scope = globalThis as unknown as DedicatedWorkerGlobalScope;
const MAX_PENDING_INPUT = 16384;
const ASSET_BASE = new URL("/guest/", scope.location.origin);
const controller = new AbortController();
let state: "idle" | "loading" | "running" | "stopped" = "idle";
let emulator: DiagnosticEmulator | undefined;
let channel: MessageChannel | undefined;
let pending: number[] = [];
let waitingForOutput = false;

function emit(event: GuestEvent): void {
  scope.postMessage(event);
}

function cleanup(): void {
  controller.abort();
  channel?.port1.close();
  channel?.port2.close();
  channel = undefined;
  const instance = emulator;
  emulator = undefined;
  try {
    // A Wasm trap can leave the Rust session borrowed. Never let cleanup mask
    // the original error; the parent terminates this Worker to reclaim it.
    instance?.free();
  } catch {
    // The failed instance cannot be reused and remains confined to this Worker.
  }
  pending = [];
  waitingForOutput = false;
}

function fail(error: unknown): void {
  if (state === "stopped") return;
  let pc = "";
  if (error instanceof WebAssembly.RuntimeError && emulator) {
    try {
      pc = ` at guest PC 0x${emulator.lastPc().toString(16)}`;
    } catch {
      // Preserve the original trap even if diagnostic metadata is unavailable.
    }
  }
  state = "stopped";
  cleanup();
  const detail =
    error instanceof WebAssembly.RuntimeError
      ? `Emulator trapped: ${error.message}${pc}. Diagnostic guest cannot continue.`
      : error instanceof Error
        ? error.message
        : "Unknown worker error";
  emit({ type: "error", message: detail.slice(0, 512) });
}

/** Fixed-size stream allocation, with SHA-256 before bytes enter the emulator. */
async function loadAsset(name: string, size: number, expectedHash: string): Promise<Uint8Array> {
  const response = await fetch(new URL(name, ASSET_BASE), { signal: controller.signal });
  if (!response.ok || !response.body)
    throw new Error(`Cannot load ${name} (HTTP ${response.status})`);
  const bytes = new Uint8Array(size);
  const reader = response.body.getReader();
  let offset = 0;
  try {
    while (true) {
      const result = await reader.read();
      if (result.done) break;
      if (offset + result.value.byteLength > size) throw new Error(`Unexpected size for ${name}`);
      bytes.set(result.value, offset);
      offset += result.value.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
  if (offset !== size) throw new Error(`Unexpected size for ${name}; run the diagnostic setup`);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  const actual = Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  if (actual !== expectedHash) throw new Error(`SHA-256 mismatch for ${name}`);
  return bytes;
}

/** Locally built same-origin runtime; cap its size before compiling it. */
async function loadRuntime(): Promise<Uint8Array<ArrayBuffer>> {
  const response = await fetch(new URL("wasmnux_diagnostic.wasm", ASSET_BASE), {
    signal: controller.signal,
  });
  if (!response.ok || !response.body) {
    throw new Error(
      `Cannot load diagnostic Wasm (HTTP ${response.status}); run npm run build:wasm`,
    );
  }
  const bytes = new Uint8Array(4 * 1024 * 1024);
  const reader = response.body.getReader();
  let offset = 0;
  try {
    while (true) {
      const result = await reader.read();
      if (result.done) break;
      if (offset + result.value.byteLength > bytes.byteLength) {
        await reader.cancel();
        throw new Error("Diagnostic Wasm exceeds its size limit");
      }
      bytes.set(result.value, offset);
      offset += result.value.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
  if (offset === 0) throw new Error("Diagnostic Wasm is empty");
  return bytes.slice(0, offset);
}

function tick(): void {
  if (state !== "running" || !emulator || !channel) return;
  try {
    // Keep per-slice work bounded and give incoming worker messages a chance to run.
    const input = pending.splice(0, 128);
    for (const byte of input) emulator.putInput(byte);
    emulator.runCycles(100000);
    const output: number[] = [];
    while (output.length < MAX_OUTPUT_BYTES) {
      const byte = emulator.getOutput();
      if (byte === -1) break;
      output.push(byte);
    }
    if (output.length > 0) {
      // One bounded output message may be in flight at a time. The UI acknowledges
      // only after its terminal has processed the bytes, preventing unbounded queues.
      waitingForOutput = true;
      emit({ type: "output", bytes: Uint8Array.from(output) });
    } else {
      channel.port2.postMessage(null);
    }
  } catch (error) {
    fail(error);
  }
}

async function start(): Promise<void> {
  if (state !== "idle") throw new Error("Diagnostic worker already started");
  state = "loading";
  emit({ type: "starting" });
  const wasm = await loadRuntime();
  const result = await WebAssembly.instantiate(wasm);
  if (state !== "loading") return;
  emulator = new DiagnosticEmulator(result.instance.exports);
  const firmware = await loadAsset(
    "fw_payload.elf",
    9907888,
    "3ce5529fe302144732b22636d614965bea3ac09999cfdb42912e74c33c6396e6",
  );
  const disk = await loadAsset(
    "rootfs.img",
    52428800,
    "273067980c4d8ad3898d505c1786ef69ecf6b33b8373942089a8b0ffe84b3409",
  );
  if (state !== "loading") return;
  emulator.setupProgram(firmware);
  emulator.setupDisk(disk);
  state = "running";
  channel = new MessageChannel();
  channel.port1.onmessage = tick;
  emit({ type: "running" });
  channel.port2.postMessage(null);
}

scope.addEventListener("message", (event: MessageEvent<unknown>) => {
  const request = parseGuestRequest(event.data);
  if (!request) {
    fail(new Error("Rejected invalid worker message"));
    return;
  }
  if (request.type === "start") {
    void start().catch(fail);
  } else if (request.type === "stop") {
    state = "stopped";
    cleanup();
    emit({ type: "stopped" });
  } else if (request.type === "output-consumed") {
    if (state === "running" && waitingForOutput && channel) {
      waitingForOutput = false;
      channel.port2.postMessage(null);
    }
  } else if (state === "running") {
    if (pending.length + request.bytes.length > MAX_PENDING_INPUT) {
      fail(new Error("Serial input queue overflow; diagnostic stopped"));
      return;
    }
    pending.push(...request.bytes);
  }
});
