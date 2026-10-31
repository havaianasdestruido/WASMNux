import { expect, test } from "@playwright/test";

test("instantiates the locally built Rust Wasm and enforces ABI and memory limits", async ({
  page,
}) => {
  await page.goto("/");
  const result = await page.evaluate(async () => {
    const response = await fetch("/guest/wasmnux_diagnostic.wasm");
    if (!response.ok) throw new Error(`Wasm unavailable: ${response.status}`);
    const { instance } = await WebAssembly.instantiate(await response.arrayBuffer());
    const wasm = instance.exports as unknown as {
      memory: WebAssembly.Memory;
      vm_new: () => number;
      vm_alloc_image: (size: number) => number;
      vm_load_disk: () => number;
      vm_run_cycles: (count: number) => number;
      vm_put_input: (byte: number) => number;
      vm_get_output: () => number;
      vm_last_pc: () => bigint;
      vm_free: () => number;
    };
    const created = wasm.vm_new();
    const duplicate = wasm.vm_new();
    const empty = wasm.vm_alloc_image(0);
    const tooLarge = wasm.vm_alloc_image(64 * 1024 * 1024 + 1);
    const offset = wasm.vm_alloc_image(4);
    const insideMemory = offset > 0 && offset + 4 <= wasm.memory.buffer.byteLength;
    if (insideMemory) new Uint8Array(wasm.memory.buffer, offset, 4).set([1, 2, 3, 4]);
    const secondAllocation = wasm.vm_alloc_image(4);
    const badOrder = wasm.vm_load_disk();
    const prematureCycles = wasm.vm_run_cycles(1);
    const prematureInput = wasm.vm_put_input(65);
    const emptyOutput = wasm.vm_get_output();
    const pc = wasm.vm_last_pc().toString();
    let memoryLimitEnforced = false;
    try {
      wasm.memory.grow(8192); // 512 MiB plus the existing pages exceeds the linked cap.
    } catch (error) {
      memoryLimitEnforced = error instanceof RangeError;
    }
    const freed = wasm.vm_free();
    const missingOutput = wasm.vm_get_output();
    return {
      created,
      duplicate,
      empty,
      tooLarge,
      insideMemory,
      secondAllocation,
      badOrder,
      prematureCycles,
      prematureInput,
      emptyOutput,
      pc,
      memoryLimitEnforced,
      freed,
      missingOutput,
    };
  });
  expect(result).toEqual({
    created: 0,
    duplicate: 1,
    empty: 0,
    tooLarge: 0,
    insideMemory: true,
    secondAllocation: 0,
    badOrder: 1,
    prematureCycles: 1,
    prematureInput: 1,
    emptyOutput: -1,
    pc: "0",
    memoryLimitEnforced: true,
    freed: 0,
    missingOutput: -2,
  });
});

test("the real Wasm rejects partial-sector disk images and permits a replacement", async ({
  page,
}) => {
  await page.goto("/");
  const result = await page.evaluate(async () => {
    const [moduleResponse, programResponse] = await Promise.all([
      fetch("/guest/wasmnux_diagnostic.wasm"),
      fetch("/guest/fw_payload.elf"),
    ]);
    if (!moduleResponse.ok || !programResponse.ok) throw new Error("Diagnostic assets unavailable");
    const program = new Uint8Array(await programResponse.arrayBuffer());
    const { instance } = await WebAssembly.instantiate(await moduleResponse.arrayBuffer());
    const wasm = instance.exports as unknown as {
      memory: WebAssembly.Memory;
      vm_new: () => number;
      vm_alloc_image: (size: number) => number;
      vm_load_program: () => number;
      vm_load_disk: () => number;
      vm_run_cycles: (count: number) => number;
      vm_free: () => number;
    };
    const created = wasm.vm_new();
    const offset = wasm.vm_alloc_image(program.length);
    if (!offset || offset + program.length > wasm.memory.buffer.byteLength) {
      throw new Error("Invalid program buffer");
    }
    new Uint8Array(wasm.memory.buffer, offset, program.length).set(program);
    const programLoaded = wasm.vm_load_program();
    const partialReserved = wasm.vm_alloc_image(513) > 0;
    const partialRejected = wasm.vm_load_disk();
    const prematureCycles = wasm.vm_run_cycles(1);
    const replacementReserved = wasm.vm_alloc_image(512) > 0;
    const freed = wasm.vm_free();
    return {
      created,
      programLoaded,
      partialReserved,
      partialRejected,
      prematureCycles,
      replacementReserved,
      freed,
    };
  });
  expect(result).toEqual({
    created: 0,
    programLoaded: 0,
    partialReserved: true,
    partialRejected: 2,
    prematureCycles: 1,
    replacementReserved: true,
    freed: 0,
  });
});
