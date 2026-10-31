/**
 * Main-thread client for the VM worker. All request/response correlation,
 * timeouts and event fan-out live here; the UI never talks to the Worker
 * object directly. No promise is left dangling: every request resolves or
 * rejects (timeout / worker error / dispose).
 */

import {
  parseWorkerEvent,
  type BootRequest,
  type MachineStateName,
  type WorkerEvent,
} from "./protocol.js";

export type WasmStatus = "loading" | "ready" | "unavailable";

export interface VmStats {
  readonly instructions: number;
  readonly pcHex: string;
  readonly state: MachineStateName;
}

type Listener<T> = (value: T) => void;

interface Pending {
  readonly resolve: (state: MachineStateName) => void;
  readonly reject: (error: Error) => void;
  readonly timer: ReturnType<typeof setTimeout>;
}

const BOOT_TIMEOUT_MS = 120_000;
const CONTROL_TIMEOUT_MS = 15_000;

export class VmClient {
  readonly #worker: Worker;
  readonly #pending = new Map<number, Pending>();
  #nextId = 1;
  #wasmStatus: WasmStatus = "loading";
  #coreVersion: string | undefined;
  #unavailableReason: string | undefined;

  readonly #serialListeners = new Set<Listener<Uint8Array>>();
  readonly #framebufferListeners = new Set<
    Listener<{ width: number; height: number; stride: number; pixels: Uint8Array }>
  >();
  readonly #stateListeners = new Set<Listener<{ state: MachineStateName; reason?: string }>>();
  readonly #diskListeners = new Set<Listener<Uint8Array>>();
  readonly #statsListeners = new Set<Listener<VmStats>>();
  readonly #errorListeners = new Set<Listener<string>>();
  readonly #statusListeners = new Set<Listener<WasmStatus>>();

  constructor() {
    this.#worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    this.#worker.addEventListener("message", (event: MessageEvent) => {
      let parsed: WorkerEvent;
      try {
        parsed = parseWorkerEvent(event.data as unknown);
      } catch (error) {
        this.#emitError(
          `discarding malformed worker event: ${error instanceof Error ? error.message : String(error)}`,
        );
        return;
      }
      this.#dispatch(parsed);
    });
    this.#worker.addEventListener("error", (event: ErrorEvent) => {
      this.#emitError(`worker error: ${event.message}`);
      this.#failAllPending(new Error(`worker error: ${event.message}`));
    });
  }

  get wasmStatus(): WasmStatus {
    return this.#wasmStatus;
  }

  get coreVersion(): string | undefined {
    return this.#coreVersion;
  }

  get unavailableReason(): string | undefined {
    return this.#unavailableReason;
  }

  #dispatch(event: WorkerEvent): void {
    switch (event.type) {
      case "ready":
        this.#wasmStatus = "ready";
        this.#coreVersion = event.coreVersion;
        for (const listener of this.#statusListeners) listener("ready");
        break;
      case "wasmUnavailable":
        this.#wasmStatus = "unavailable";
        this.#unavailableReason = event.reason;
        for (const listener of this.#statusListeners) listener("unavailable");
        this.#failAllPending(new Error(event.reason));
        break;
      case "booted":
        this.#settle(event.requestId, event.state);
        break;
      case "state":
        for (const listener of this.#stateListeners) {
          listener({
            state: event.state,
            ...(event.reason !== undefined ? { reason: event.reason } : {}),
          });
        }
        if (event.requestId !== undefined) {
          this.#settle(event.requestId, event.state);
        }
        break;
      case "serialOutput":
        for (const listener of this.#serialListeners) listener(event.bytes);
        break;
      case "framebuffer":
        for (const listener of this.#framebufferListeners) {
          listener({
            width: event.width,
            height: event.height,
            stride: event.stride,
            pixels: event.pixels,
          });
        }
        break;
      case "diskImage":
        for (const listener of this.#diskListeners) listener(event.bytes);
        break;
      case "stats":
        for (const listener of this.#statsListeners) {
          listener({ instructions: event.instructions, pcHex: event.pcHex, state: event.state });
        }
        break;
      case "error":
        this.#emitError(event.message);
        if (event.requestId !== undefined) {
          this.#settleError(event.requestId, new Error(event.message));
        }
        break;
    }
  }

  #emitError(message: string): void {
    for (const listener of this.#errorListeners) listener(message);
  }

  #settle(requestId: number, state: MachineStateName): void {
    const pending = this.#pending.get(requestId);
    if (pending !== undefined) {
      clearTimeout(pending.timer);
      this.#pending.delete(requestId);
      pending.resolve(state);
    }
  }

  #settleError(requestId: number, error: Error): void {
    const pending = this.#pending.get(requestId);
    if (pending !== undefined) {
      clearTimeout(pending.timer);
      this.#pending.delete(requestId);
      pending.reject(error);
    }
  }

  #failAllPending(error: Error): void {
    for (const [id, pending] of this.#pending) {
      clearTimeout(pending.timer);
      this.#pending.delete(id);
      pending.reject(error);
    }
  }

  #request<T>(make: (requestId: number) => T, timeoutMs: number): Promise<MachineStateName> {
    const requestId = this.#nextId;
    this.#nextId += 1;
    return new Promise<MachineStateName>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(requestId);
        reject(new Error(`VM request ${requestId} timed out after ${timeoutMs} ms`));
      }, timeoutMs);
      this.#pending.set(requestId, { resolve, reject, timer });
      make(requestId);
    });
  }

  /** Boots the VM. Large byte arrays are transferred, not copied. */
  boot(request: Omit<BootRequest, "type" | "requestId">): Promise<MachineStateName> {
    return this.#request((requestId) => {
      const full: BootRequest = { type: "boot", requestId, ...request };
      const transfers: ArrayBuffer[] = [request.kernel.buffer as ArrayBuffer];
      if (request.firmware !== undefined) transfers.push(request.firmware.buffer as ArrayBuffer);
      if (request.initrd !== undefined) transfers.push(request.initrd.buffer as ArrayBuffer);
      if (request.disk !== undefined) transfers.push(request.disk.buffer as ArrayBuffer);
      this.#worker.postMessage(full, transfers);
    }, BOOT_TIMEOUT_MS);
  }

  pause(): Promise<MachineStateName> {
    return this.#request((requestId) => {
      this.#worker.postMessage({ type: "pause", requestId });
    }, CONTROL_TIMEOUT_MS);
  }

  resume(): Promise<MachineStateName> {
    return this.#request((requestId) => {
      this.#worker.postMessage({ type: "resume", requestId });
    }, CONTROL_TIMEOUT_MS);
  }

  powerOff(): Promise<MachineStateName> {
    return this.#request((requestId) => {
      this.#worker.postMessage({ type: "powerOff", requestId });
    }, CONTROL_TIMEOUT_MS);
  }

  /** Sends terminal input to the guest UART (fire-and-forward). */
  serialWrite(bytes: Uint8Array): void {
    this.#worker.postMessage({ type: "serialInput", bytes }, [bytes.buffer as ArrayBuffer]);
  }

  /** Confirms a diskImage event was persisted; the worker can mark clean. */
  acknowledgeDiskPersisted(): void {
    this.#worker.postMessage({ type: "diskPersisted" });
  }

  /** Asks the worker for the current guest framebuffer. */
  requestFramebuffer(): void {
    this.#worker.postMessage({ type: "framebufferTick" });
  }

  onSerial(listener: Listener<Uint8Array>): () => void {
    this.#serialListeners.add(listener);
    return () => this.#serialListeners.delete(listener);
  }

  onFramebuffer(
    listener: Listener<{ width: number; height: number; stride: number; pixels: Uint8Array }>,
  ): () => void {
    this.#framebufferListeners.add(listener);
    return () => this.#framebufferListeners.delete(listener);
  }

  onState(listener: Listener<{ state: MachineStateName; reason?: string }>): () => void {
    this.#stateListeners.add(listener);
    return () => this.#stateListeners.delete(listener);
  }

  onDiskImage(listener: Listener<Uint8Array>): () => void {
    this.#diskListeners.add(listener);
    return () => this.#diskListeners.delete(listener);
  }

  onStats(listener: Listener<VmStats>): () => void {
    this.#statsListeners.add(listener);
    return () => this.#statsListeners.delete(listener);
  }

  onError(listener: Listener<string>): () => void {
    this.#errorListeners.add(listener);
    return () => this.#errorListeners.delete(listener);
  }

  onWasmStatus(listener: Listener<WasmStatus>): () => void {
    this.#statusListeners.add(listener);
    return () => this.#statusListeners.delete(listener);
  }

  dispose(): void {
    this.#failAllPending(new Error("VM client disposed"));
    this.#worker.terminate();
  }
}
