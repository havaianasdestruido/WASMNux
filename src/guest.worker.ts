/// <reference lib="webworker" />
import { MAX_OUTPUT_BYTES, parseGuestRequest, type GuestEvent } from "./protocol";

// This module is only instantiated by `new Worker`; it has no DOM access.
const scope = globalThis as unknown as DedicatedWorkerGlobalScope;
const MAX_PENDING_INPUT = 16384;
const ASSET_BASE = new URL("/guest/", scope.location.origin);
const controller = new AbortController();
let state: "idle" | "loading" | "running" | "stopped" = "idle";
let emulator: Emulator | undefined;
let channel: MessageChannel | undefined;
let pending: number[] = [];
let waitingForOutput = false;

interface Emulator {
  setup_program(data: Uint8Array): void;
  setup_filesystem(data: Uint8Array): void;
  enable_page_cache(enabled: boolean): void;
  run_cycles(cycles: number): void;
  get_output(): number;
  put_input(byte: number): void;
  free(): void;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && (typeof value === "object" || typeof value === "function");
}

function isEmulator(value: unknown): value is Emulator {
  return (
    isRecord(value) &&
    [
      "setup_program",
      "setup_filesystem",
      "enable_page_cache",
      "run_cycles",
      "get_output",
      "put_input",
      "free",
    ].every((name) => typeof value[name] === "function")
  );
}

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
    // A Wasm trap can leave wasm-bindgen's mutable borrow poisoned. In that
    // case free() itself throws; never let cleanup mask the original error.
    // The parent terminates this Worker after receiving the error, reclaiming it.
    instance?.free();
  } catch {
    // The failed instance cannot be reused and remains confined to this Worker.
  }
  pending = [];
  waitingForOutput = false;
}

function fail(error: unknown): void {
  if (state === "stopped") return;
  state = "stopped";
  cleanup();
  const detail =
    error instanceof WebAssembly.RuntimeError
      ? `Emulator trapped: ${error.message}. Diagnostic guest cannot continue.`
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

function tick(): void {
  if (state !== "running" || !emulator || !channel) return;
  try {
    // Keep per-slice work bounded and give incoming worker messages a chance to run.
    const input = pending.splice(0, 128);
    for (const byte of input) emulator.put_input(byte);
    emulator.run_cycles(100000);
    const output: number[] = [];
    while (output.length < MAX_OUTPUT_BYTES) {
      const byte = emulator.get_output();
      if (byte === 0) break;
      if (!Number.isInteger(byte) || byte < 0 || byte > 255) throw new Error("Invalid serial byte");
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
  const wasm = await loadAsset(
    "riscv_emu_rust_wasm_bg.wasm",
    158147,
    "e1804062b8a40bfe27af9a393bee291b0e8228435ebbc47a19d45a1faa8e3f65",
  );
  // The module is a same-origin, setup-verified upstream wasm-bindgen loader.
  const loaderUrl = new URL("riscv_emu_rust_wasm.js", ASSET_BASE).href;
  const loader: unknown = await import(/* @vite-ignore */ loaderUrl);
  if (
    !isRecord(loader) ||
    typeof loader.default !== "function" ||
    !isRecord(loader.WasmRiscv) ||
    typeof loader.WasmRiscv.new !== "function"
  ) {
    throw new Error("Invalid emulator bindings");
  }
  await loader.default(wasm);
  if (state !== "loading") return;
  const instance: unknown = loader.WasmRiscv.new();
  if (!isEmulator(instance)) throw new Error("Invalid emulator instance");
  emulator = instance;
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
  emulator.setup_program(firmware);
  emulator.setup_filesystem(disk);
  // Upstream documents this cache as experimentally unsound when guest page
  // tables change. Prefer correctness over diagnostic boot speed.
  emulator.enable_page_cache(false);
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
