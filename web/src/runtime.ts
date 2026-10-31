/**
 * WASM runtime bindings for the `wasmnux-core` emulator.
 *
 * Interop uses the raw WebAssembly JavaScript API (exported `extern "C"`
 * functions plus the exported WASM linear memory). This is the TypeScript
 * side of the Rust/TypeScript boundary described in ARCHITECTURE.md.
 */

import type { MachineInfo } from "./types";

export interface WasmExports {
  memory: WebAssembly.Memory;
  wasmnux_machine_new(mem_size: number, harts: number): number;
  wasmnux_machine_free(ptr: number): void;
  wasmnux_machine_reset(ptr: number): void;
  wasmnux_machine_halt(ptr: number): void;
  wasmnux_machine_load_elf(
    ptr: number,
    image_ptr: number,
    image_len: number,
    entry_out: number,
  ): number;
  wasmnux_machine_load_device_tree(ptr: number, addr_out: number): number;
  wasmnux_machine_set_boot_args(
    ptr: number,
    hart_id: bigint,
    dtb_addr: bigint,
  ): void;
  wasmnux_machine_step(ptr: number): number;
  wasmnux_machine_run(ptr: number, max_instructions: bigint): number;
  wasmnux_machine_mem_ptr(ptr: number): number;
  wasmnux_machine_mem_size(ptr: number): bigint;
  wasmnux_machine_pc(ptr: number): bigint;
  wasmnux_machine_reg(ptr: number, idx: number): bigint;
  wasmnux_machine_cycles(ptr: number): bigint;
  wasmnux_machine_mtime(ptr: number): bigint;
  wasmnux_machine_drain_console(
    ptr: number,
    out_ptr: number,
    out_len: number,
  ): number;
  wasmnux_machine_write_mem(
    ptr: number,
    addr: bigint,
    src: number,
    len: number,
  ): number;
}

export class WasmRuntimeError extends Error {
  constructor(
    public code: number,
    message: string,
  ) {
    super(message);
    this.name = "WasmRuntimeError";
  }
}

export enum StepResultCode {
  Ok = 0,
  Trap = 1,
  Error = 2,
}

export enum RunResultCode {
  Ok = 0,
  Trap = 1,
  Error = 2,
}

/**
 * Thin, typed handle over a guest machine instance living in WASM memory.
 */
export class WasmMachine {
  private readonly exports: WasmExports;
  private readonly mem: Uint8Array;
  private readonly view: DataView;
  private ptr: number;
  private disposed = false;

  // Scratch region for out-parameters, carved from the top of guest memory.
  private scratchBase: number;
  private scratchOffset = 0;

  constructor(exports: WasmExports, memSizeBytes: number, harts: number) {
    this.exports = exports;
    // Create the guest machine in WASM memory. Memory may grow, so re-fetch
    // the buffer views afterwards.
    this.ptr = exports.wasmnux_machine_new(memSizeBytes, harts);
    if (this.ptr === 0) {
      throw new WasmRuntimeError(-1, "failed to allocate guest machine");
    }
    this.mem = new Uint8Array(exports.memory.buffer);
    this.view = new DataView(exports.memory.buffer);
    // Reserve the last 4 KiB of guest memory as scratch for out-params.
    this.scratchBase = memSizeBytes - 4096;
    if (this.scratchBase < 0) {
      throw new WasmRuntimeError(-2, "guest memory too small for scratch");
    }
  }

  get disposedState(): boolean {
    return this.disposed;
  }

  dispose(): void {
    if (!this.disposed) {
      this.exports.wasmnux_machine_free(this.ptr);
      this.disposed = true;
    }
  }

  reset(): void {
    this.throwIfDisposed();
    this.exports.wasmnux_machine_reset(this.ptr);
  }

  halt(): void {
    this.throwIfDisposed();
    this.exports.wasmnux_machine_halt(this.ptr);
  }

  /**
   * Load an ELF64 image that has already been copied into WASM linear memory.
   * Returns the entry point address.
   */
  loadElf(imagePtr: number, imageLen: number): bigint {
    this.throwIfDisposed();
    const entryOut = this.allocScratch(8);
    const rc = this.exports.wasmnux_machine_load_elf(
      this.ptr,
      imagePtr,
      imageLen,
      entryOut,
    );
    if (rc !== 0) {
      throw new WasmRuntimeError(rc, `ELF load failed (rc=${rc})`);
    }
    const lo = this.view.getUint32(entryOut, true);
    const hi = this.view.getUint32(entryOut + 4, true);
    return (BigInt(hi) << 32n) | BigInt(lo >>> 0);
  }

  loadDeviceTree(): bigint {
    this.throwIfDisposed();
    const addrOut = this.allocScratch(8);
    const rc = this.exports.wasmnux_machine_load_device_tree(this.ptr, addrOut);
    if (rc !== 0) {
      throw new WasmRuntimeError(rc, `device tree load failed (rc=${rc})`);
    }
    const lo = this.view.getUint32(addrOut, true);
    const hi = this.view.getUint32(addrOut + 4, true);
    return (BigInt(hi) << 32n) | BigInt(lo >>> 0);
  }

  setBootArgs(hartId: number, dtbAddr: number | bigint): void {
    this.throwIfDisposed();
    this.exports.wasmnux_machine_set_boot_args(
      this.ptr,
      BigInt(hartId),
      typeof dtbAddr === "bigint" ? dtbAddr : BigInt(dtbAddr),
    );
  }

  step(): StepResultCode {
    this.throwIfDisposed();
    return this.exports.wasmnux_machine_step(this.ptr) as StepResultCode;
  }

  run(maxInstructions: number): RunResultCode {
    this.throwIfDisposed();
    return this.exports.wasmnux_machine_run(
      this.ptr,
      BigInt(maxInstructions),
    ) as RunResultCode;
  }

  memPtr(): number {
    this.throwIfDisposed();
    return this.exports.wasmnux_machine_mem_ptr(this.ptr);
  }

  memSize(): bigint {
    this.throwIfDisposed();
    return this.exports.wasmnux_machine_mem_size(this.ptr);
  }

  pc(): bigint {
    this.throwIfDisposed();
    return this.exports.wasmnux_machine_pc(this.ptr);
  }

  reg(idx: number): bigint {
    this.throwIfDisposed();
    if (idx < 0 || idx > 31) {
      throw new RangeError(`register index ${idx} out of range`);
    }
    return this.exports.wasmnux_machine_reg(this.ptr, idx);
  }

  cycles(): bigint {
    this.throwIfDisposed();
    return this.exports.wasmnux_machine_cycles(this.ptr);
  }

  mtime(): bigint {
    this.throwIfDisposed();
    return this.exports.wasmnux_machine_mtime(this.ptr);
  }

  /**
   * Drain guest console output into a byte array.
   */
  drainConsole(maxBytes: number = 4096): Uint8Array {
    this.throwIfDisposed();
    const outPtr = this.allocScratch(maxBytes);
    const n = this.exports.wasmnux_machine_drain_console(
      this.ptr,
      outPtr,
      maxBytes,
    );
    if (n < 0) {
      throw new WasmRuntimeError(-n, `console drain failed (rc=${n})`);
    }
    return this.mem.slice(outPtr, outPtr + n);
  }

  /**
   * Copy `src` bytes into guest memory at `addr`.
   */
  writeMem(addr: number | bigint, src: Uint8Array): void {
    this.throwIfDisposed();
    const tmp = this.allocScratch(src.byteLength);
    this.mem.set(src, tmp);
    const rc = this.exports.wasmnux_machine_write_mem(
      this.ptr,
      typeof addr === "bigint" ? addr : BigInt(addr),
      tmp,
      src.byteLength,
    );
    if (rc !== 0) {
      throw new WasmRuntimeError(rc, `guest write failed (rc=${rc})`);
    }
  }

  info(): MachineInfo {
    this.throwIfDisposed();
    return {
      memSize: Number(this.memSize()),
      pc: Number(this.pc()),
      cycles: Number(this.cycles()),
      mtime: Number(this.mtime()),
    };
  }

  private throwIfDisposed(): void {
    if (this.disposed) {
      throw new Error("guest machine has been disposed");
    }
  }

  private allocScratch(len: number): number {
    if (this.scratchOffset + len > 4096) {
      throw new WasmRuntimeError(-3, "scratch region exhausted");
    }
    const addr = this.scratchBase + this.scratchOffset;
    this.scratchOffset += len;
    return addr;
  }
}

/**
 * Load a WASM module and return its typed exports + memory.
 */
export async function loadWasmModule(
  wasmBytes: ArrayBuffer,
  imports: WebAssembly.Imports = {},
): Promise<{ exports: WasmExports; instance: WebAssembly.Instance }> {
  const result = await WebAssembly.instantiate(wasmBytes, imports);
  return {
    exports: result.instance.exports as unknown as WasmExports,
    instance: result.instance,
  };
}
