/** Checked host bindings for the locally built historical diagnostic, not a stable VM API. */
type WasmFn = (...args: number[]) => number;
type ExportName =
  | "vm_new"
  | "vm_alloc_image"
  | "vm_load_program"
  | "vm_load_disk"
  | "vm_run_cycles"
  | "vm_put_input"
  | "vm_get_output"
  | "vm_free";

type WasmExports = Record<ExportName, WasmFn> & {
  memory: WebAssembly.Memory;
  vm_last_pc: () => bigint;
};

const names: ExportName[] = [
  "vm_new",
  "vm_alloc_image",
  "vm_load_program",
  "vm_load_disk",
  "vm_run_cycles",
  "vm_put_input",
  "vm_get_output",
  "vm_free",
];

function validateExports(value: unknown): WasmExports {
  if (value === null || typeof value !== "object") throw new Error("Invalid diagnostic exports");
  const exports = value as Record<string, unknown>;
  if (!(exports.memory instanceof WebAssembly.Memory)) {
    throw new Error("Diagnostic memory is unavailable");
  }
  for (const name of names) {
    if (typeof exports[name] !== "function") throw new Error(`Missing diagnostic export: ${name}`);
  }
  if (typeof exports.vm_last_pc !== "function")
    throw new Error("Missing diagnostic export: vm_last_pc");
  return exports as WasmExports;
}

function checkStatus(status: number, operation: string): void {
  if (status !== 0) throw new Error(`Diagnostic ${operation} failed (status ${status})`);
}

export class DiagnosticEmulator {
  private readonly wasm: WasmExports;
  private freed = false;

  constructor(exports: unknown) {
    this.wasm = validateExports(exports);
    checkStatus(this.wasm.vm_new(), "initialization");
  }

  private loadImage(data: Uint8Array, commit: "vm_load_program" | "vm_load_disk"): void {
    if (this.freed || data.byteLength === 0 || data.byteLength > 64 * 1024 * 1024) {
      throw new Error("Invalid diagnostic image");
    }
    const offset = this.wasm.vm_alloc_image(data.byteLength);
    // Allocation can grow memory, so read memory.buffer only after the call.
    const memory = this.wasm.memory.buffer;
    if (
      !Number.isSafeInteger(offset) ||
      offset <= 0 ||
      offset > memory.byteLength ||
      data.byteLength > memory.byteLength - offset
    ) {
      throw new Error("Invalid diagnostic image buffer");
    }
    new Uint8Array(memory, offset, data.byteLength).set(data);
    checkStatus(this.wasm[commit](), commit);
  }

  setupProgram(data: Uint8Array): void {
    this.loadImage(data, "vm_load_program");
  }

  setupDisk(data: Uint8Array): void {
    this.loadImage(data, "vm_load_disk");
  }

  runCycles(cycles: number): void {
    if (this.freed || !Number.isInteger(cycles) || cycles < 1 || cycles > 100_000) {
      throw new Error("Invalid diagnostic cycle count");
    }
    checkStatus(this.wasm.vm_run_cycles(cycles), "execution");
  }

  putInput(byte: number): void {
    if (this.freed || !Number.isInteger(byte) || byte < 0 || byte > 255) {
      throw new Error("Invalid diagnostic input byte");
    }
    checkStatus(this.wasm.vm_put_input(byte), "serial input");
  }

  getOutput(): number {
    if (this.freed) throw new Error("Diagnostic already freed");
    const byte = this.wasm.vm_get_output();
    if (!Number.isInteger(byte) || byte < -1 || byte > 255) {
      throw new Error("Invalid diagnostic output byte");
    }
    return byte;
  }

  lastPc(): bigint {
    const pc = this.wasm.vm_last_pc();
    if (typeof pc !== "bigint") throw new Error("Invalid diagnostic program counter");
    return BigInt.asUintN(64, pc);
  }

  free(): void {
    if (this.freed) return;
    this.freed = true;
    checkStatus(this.wasm.vm_free(), "cleanup");
  }
}
