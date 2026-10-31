import { describe, expect, it, vi } from "vitest";
import { DiagnosticEmulator } from "../src/diagnostic-wasm";

function fixture() {
  const memory = new WebAssembly.Memory({ initial: 1, maximum: 4 });
  const exports = {
    memory,
    vm_new: vi.fn(() => 0),
    vm_alloc_image: vi.fn(() => 32),
    vm_load_program: vi.fn(() => 0),
    vm_load_disk: vi.fn(() => 0),
    vm_run_cycles: vi.fn(() => 0),
    vm_put_input: vi.fn(() => 0),
    vm_get_output: vi.fn(() => -1),
    vm_last_pc: vi.fn(() => 0x1234n),
    vm_free: vi.fn(() => 0),
  };
  return { memory, exports };
}

describe("diagnostic Wasm bindings", () => {
  it("rejects missing exports and initialization errors", () => {
    expect(() => new DiagnosticEmulator(null)).toThrow("Invalid diagnostic exports");
    expect(() => new DiagnosticEmulator({})).toThrow("memory is unavailable");
    const { exports } = fixture();
    exports.vm_new.mockReturnValue(1);
    expect(() => new DiagnosticEmulator(exports)).toThrow("initialization failed");
    const { vm_free: _removed, ...missing } = fixture().exports;
    expect(() => new DiagnosticEmulator(missing)).toThrow("Missing diagnostic export: vm_free");
  });

  it("copies bounded images into the post-allocation memory and checks the destination", () => {
    const { exports, memory } = fixture();
    const emulator = new DiagnosticEmulator(exports);
    exports.vm_alloc_image.mockImplementationOnce(() => {
      memory.grow(1);
      return 65536;
    });
    emulator.setupProgram(Uint8Array.of(1, 2, 3));
    expect(Array.from(new Uint8Array(memory.buffer, 65536, 3))).toEqual([1, 2, 3]);
    expect(exports.vm_load_program).toHaveBeenCalledOnce();
    emulator.setupDisk(Uint8Array.of(8, 9));
    expect(Array.from(new Uint8Array(memory.buffer, 32, 2))).toEqual([8, 9]);
    expect(exports.vm_load_disk).toHaveBeenCalledOnce();
    exports.vm_alloc_image.mockReturnValue(memory.buffer.byteLength - 1);
    expect(() => emulator.setupDisk(Uint8Array.of(1, 2))).toThrow("image buffer");
    expect(() => emulator.setupDisk(new Uint8Array())).toThrow("Invalid diagnostic image");
  });

  it("validates serial bytes and cycles, preserves zero output, and surfaces errors", () => {
    const { exports } = fixture();
    const emulator = new DiagnosticEmulator(exports);
    expect(() => emulator.runCycles(0)).toThrow("cycle count");
    expect(() => emulator.runCycles(100_001)).toThrow("cycle count");
    expect(() => emulator.putInput(256)).toThrow("input byte");
    emulator.runCycles(1);
    emulator.putInput(255);
    exports.vm_get_output.mockReturnValueOnce(0).mockReturnValueOnce(-1).mockReturnValueOnce(-2);
    expect(emulator.getOutput()).toBe(0);
    expect(emulator.getOutput()).toBe(-1);
    expect(emulator.lastPc()).toBe(0x1234n);
    exports.vm_last_pc.mockReturnValueOnce(-1n);
    expect(emulator.lastPc()).toBe(0xffff_ffff_ffff_ffffn);
    expect(() => emulator.getOutput()).toThrow("output byte");
    exports.vm_put_input.mockReturnValueOnce(3);
    expect(() => emulator.putInput(1)).toThrow("status 3");
    emulator.free();
    emulator.free();
    expect(exports.vm_free).toHaveBeenCalledOnce();
    expect(() => emulator.runCycles(1)).toThrow("cycle count");
  });
});
