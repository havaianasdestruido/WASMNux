/// <reference lib="webworker" />
/**
 * VM worker: owns the WASMNux WebAssembly machine. The emulator core is
 * synchronous, so the worker slices execution (1M instructions per tick by
 * default) and yields between slices — the UI thread never blocks on the
 * guest, and WFI backoff avoids busy-spinning an idle guest.
 *
 * The WASM package is loaded from /wasm/wasmnux_wasm.js (built by
 * scripts/build-wasm.sh or the CI artifact). If it is absent the worker
 * reports `wasmUnavailable` with an actionable reason — there is no
 * shell-in-TypeScript fallback anywhere, by design.
 */

import { parseWorkerRequest, type WorkerEvent, type WorkerRequest } from "./protocol.js";

interface WasmMachine {
  state(): string;
  load_kernel(image: Uint8Array): void;
  load_firmware(elf: Uint8Array): void;
  load_initrd(initrd: Uint8Array): void;
  attach_disk(image: Uint8Array): void;
  create_disk(sizeMib: number): void;
  boot(): void;
  pause(): void;
  resume(): void;
  power_off(): void;
  run_slice(maxInstructions: number): string;
  instructions(): bigint;
  pc(): bigint;
  serial_write(bytes: Uint8Array): boolean;
  serial_read(): Uint8Array;
  disk_dirty(): boolean;
  export_disk(): Uint8Array | undefined;
  mark_disk_clean(): void;
  framebuffer_info(): Uint32Array | undefined;
  framebuffer_bytes(): Uint8Array | undefined;
}

interface WasmModule {
  default: () => Promise<unknown>;
  Machine: new (ramMiB: number, bootargs: string, bootMode: string, fb: boolean) => WasmMachine;
  core_version: () => string;
}

function isWasmModule(value: unknown): value is WasmModule {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return (
    typeof record["default"] === "function" &&
    typeof record["Machine"] === "function" &&
    typeof record["core_version"] === "function"
  );
}

interface WorkerScope {
  postMessage(message: WorkerEvent, transfer?: Transferable[]): void;
  addEventListener(type: "message", handler: (event: MessageEvent) => void): void;
}

// Single audited cast: bundler lib types mix Window and Worker scopes.
const scope = globalThis as unknown as WorkerScope;

const SLICE_INSTRUCTIONS = 1_000_000;
const WAITING_POLL_MS = 25;
const DISK_SYNC_INTERVAL_MS = 2_000;
const STATS_INTERVAL_MS = 500;

let wasm: WasmModule | undefined;
let machine: WasmMachine | undefined;
let timer: ReturnType<typeof setTimeout> | undefined;
let lastDiskSync = 0;
let lastStats = 0;
let pendingInput: Uint8Array[] = [];

function post(event: WorkerEvent, transfer?: Transferable[]): void {
  scope.postMessage(event, transfer);
}

function postError(message: string, requestId?: number): void {
  post({
    type: "error",
    message,
    ...(requestId !== undefined ? { requestId } : {}),
  });
}

function machineState(): "created" | "running" | "paused" | "halted" {
  const state = machine?.state();
  return state === "running" || state === "paused" || state === "halted" ? state : "created";
}

function flushSerial(): void {
  if (machine === undefined) {
    return;
  }
  const out = machine.serial_read();
  if (out.length > 0) {
    post({ type: "serialOutput", bytes: out }, [out.buffer]);
  }
}

function syncDisk(force: boolean): void {
  if (machine === undefined) {
    return;
  }
  const now = Date.now();
  if (!force && (!machine.disk_dirty() || now - lastDiskSync < DISK_SYNC_INTERVAL_MS)) {
    return;
  }
  const image = machine.export_disk();
  if (image !== undefined) {
    lastDiskSync = now;
    // mark_disk_clean happens only after the UI confirms persistence
    // (the `diskPersisted` request), never optimistically.
    post({ type: "diskImage", bytes: image }, [image.buffer]);
  }
}

function postStats(force: boolean): void {
  if (machine === undefined) {
    return;
  }
  const now = Date.now();
  if (!force && now - lastStats < STATS_INTERVAL_MS) {
    return;
  }
  lastStats = now;
  post({
    type: "stats",
    instructions: Number(machine.instructions()),
    pcHex: `0x${machine.pc().toString(16)}`,
    state: machineState(),
  });
}

function stopLoop(): void {
  if (timer !== undefined) {
    clearTimeout(timer);
    timer = undefined;
  }
}

function flushInput(): void {
  if (machine === undefined) {
    return;
  }
  while (pendingInput.length > 0) {
    const head = pendingInput[0] as Uint8Array;
    if (!machine.serial_write(head)) {
      return; // UART queue full; retry next tick.
    }
    pendingInput.shift();
  }
}

function schedule(delayMs: number): void {
  if (timer !== undefined || machine === undefined) {
    return;
  }
  timer = setTimeout(tick, delayMs);
}

function tick(): void {
  timer = undefined;
  if (machine === undefined) {
    return;
  }
  try {
    flushInput();
    const reason = machine.run_slice(SLICE_INSTRUCTIONS);
    flushSerial();
    syncDisk(false);
    postStats(false);
    switch (reason) {
      case "slice":
        schedule(0);
        break;
      case "waiting":
        schedule(WAITING_POLL_MS);
        break;
      case "shutdown":
        syncDisk(true);
        postStats(true);
        post({ type: "state", state: "halted", reason: "guest requested shutdown (SBI SRST)" });
        break;
      case "reboot":
        syncDisk(true);
        post({ type: "state", state: "running", reason: "guest rebooted (SBI SRST)" });
        schedule(0);
        break;
      default:
        postError(`unknown run_slice reason from core: ${JSON.stringify(reason)}`);
    }
  } catch (error) {
    postError(error instanceof Error ? error.message : String(error));
  }
}

function requireWasm(): WasmModule {
  if (wasm === undefined) {
    throw new Error("WASM core is unavailable in this worker (see wasmUnavailable reason)");
  }
  return wasm;
}

function handleRequest(request: WorkerRequest): void {
  switch (request.type) {
    case "boot": {
      stopLoop();
      const module = requireWasm();
      machine = new module.Machine(request.ramMiB, request.bootargs, request.bootMode, true);
      if (request.firmware !== undefined) {
        machine.load_firmware(request.firmware);
      }
      machine.load_kernel(request.kernel);
      if (request.initrd !== undefined) {
        machine.load_initrd(request.initrd);
      }
      if (request.disk !== undefined) {
        machine.attach_disk(request.disk);
      } else {
        machine.create_disk(request.defaultDiskMiB);
      }
      machine.boot();
      pendingInput = [];
      post({ type: "booted", requestId: request.requestId, state: machineState() });
      postStats(true);
      schedule(0);
      break;
    }
    case "pause": {
      stopLoop();
      machine?.pause();
      flushSerial();
      post({ type: "state", state: machineState(), requestId: request.requestId });
      break;
    }
    case "resume": {
      machine?.resume();
      post({ type: "state", state: machineState(), requestId: request.requestId });
      schedule(0);
      break;
    }
    case "powerOff": {
      stopLoop();
      syncDisk(true);
      flushSerial();
      machine?.power_off();
      postStats(true);
      post({ type: "state", state: machineState(), requestId: request.requestId });
      machine = undefined;
      break;
    }
    case "serialInput": {
      pendingInput.push(request.bytes);
      flushInput();
      if (timer !== undefined) {
        // Wake a guest poll-sleeping in WFI immediately.
        stopLoop();
        schedule(0);
      }
      break;
    }
    case "diskPersisted": {
      machine?.mark_disk_clean();
      break;
    }
    case "framebufferTick": {
      if (machine !== undefined) {
        const info = machine.framebuffer_info();
        const pixels = machine.framebuffer_bytes();
        if (info !== undefined && pixels !== undefined && info.length === 3) {
          post(
            {
              type: "framebuffer",
              width: info[0] as number,
              height: info[1] as number,
              stride: info[2] as number,
              pixels,
            },
            [pixels.buffer],
          );
        }
      }
      break;
    }
  }
}

async function init(): Promise<void> {
  try {
    // Built from the runtime origin so Vite's static import analysis never
  // rewrites this into its own module pipeline (which refuses /public files).
  const wasmModuleUrl: string = `${self.location.origin}/wasm/wasmnux_wasm.js`;
    const loaded: unknown = await import(/* @vite-ignore */ wasmModuleUrl);
    if (!isWasmModule(loaded)) {
      throw new Error("/wasm/wasmnux_wasm.js does not export the expected bindings");
    }
    await loaded.default();
    wasm = loaded;
    post({ type: "ready", coreVersion: loaded.core_version() });
  } catch (error) {
    post({
      type: "wasmUnavailable",
      reason:
        "WebAssembly VM core not found at /wasm/wasmnux_wasm.js. Build it with " +
        "`./scripts/build-wasm.sh` (Rust + wasm-bindgen) or place the CI " +
        "`wasmnux-wasm-pkg` artifact under web/public/wasm/. Underlying error: " +
        (error instanceof Error ? error.message : String(error)),
    });
  }
}

scope.addEventListener("message", (event: MessageEvent) => {
  let request: WorkerRequest;
  try {
    request = parseWorkerRequest(event.data as unknown);
  } catch (error) {
    postError(`rejecting malformed worker request: ${error instanceof Error ? error.message : String(error)}`);
    return;
  }
  try {
    handleRequest(request);
  } catch (error) {
    const requestId =
      request.type === "boot" ||
      request.type === "pause" ||
      request.type === "resume" ||
      request.type === "powerOff"
        ? request.requestId
        : undefined;
    postError(error instanceof Error ? error.message : String(error), requestId);
  }
});

void init();
