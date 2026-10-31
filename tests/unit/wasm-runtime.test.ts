import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  DISK_CHUNK_BYTES,
  VmRuntimeError,
  WasmVmRuntime,
  encodeBridgeHttpResponse,
} from '../../src/wasm/bindings';
import { buildRiscvMiniSmokeKernel } from '../../src/wasm/test-mini-kernel';
import { VmClient } from '../../src/runtime/vm-client';
import { VmEngineHost } from '../../src/worker/vm-engine-host';
import type { WorkerMessage } from '../../src/worker/protocol';

const WASM_PATH = resolve(process.cwd(), 'public/wasm/wasmnux_wasm.wasm');
const WASM_BYTES = readFileSync(WASM_PATH);

describe('WASM Runtime Bindings & Worker Bridge (Phase 3)', () => {
  it('loads compiled wasmnux_wasm.wasm and enforces strict validation bounds', async () => {
    const rt = await WasmVmRuntime.fromBytes(WASM_BYTES);
    expect(rt.abiVersion).toBe(1);
    expect(rt.linearMemoryBytes).toBeGreaterThanOrEqual(65536);

    // Invalid profile ID rejected with typed VmRuntimeError
    expect(() => rt.createVm({ profileId: '../escape', ramMib: 32, diskMib: 8 })).toThrow(
      VmRuntimeError,
    );

    // Out-of-bounds RAM configuration rejected gracefully
    expect(() => rt.createVm({ profileId: 'vanilla', ramMib: 4, diskMib: 8 })).toThrow(
      VmRuntimeError,
    );
    expect(() => rt.createVm({ profileId: 'vanilla', ramMib: 4096, diskMib: 8 })).toThrow(
      VmRuntimeError,
    );

    // Empty kernel rejected cleanly with boot_error
    const vm = rt.createVm({ profileId: 'vanilla', ramMib: 16, diskMib: 4 });
    vm.stageKernel(new Uint8Array(0));
    expect(() => {
      vm.boot('console=ttyS0');
    }).toThrow(VmRuntimeError);

    // Corrupt snapshot & corrupt disk bundle rejected cleanly
    expect(() => {
      vm.restoreFullSnapshot(new Uint8Array([1, 2, 3, 4]));
    }).toThrow(VmRuntimeError);
    expect(() => {
      vm.importDiskBundle(new Uint8Array([9, 9, 9]));
    }).toThrow(VmRuntimeError);
    vm.destroy();
  });

  it('survives multi-cycle init -> boot -> snapshot -> restore -> destroy without leaks', async () => {
    const rt = await WasmVmRuntime.fromBytes(WASM_BYTES);
    const kernel = buildRiscvMiniSmokeKernel('VANILLA-OK');
    const decoder = new TextDecoder();

    for (let cycle = 0; cycle < 5; cycle += 1) {
      const vm = rt.createVm({
        profileId: `cycle-${String(cycle)}`,
        ramMib: 16,
        diskMib: 4,
        epochSeconds: 1_761_868_800,
      });

      const chunk0 = new Uint8Array(DISK_CHUNK_BYTES);
      chunk0[0] = 0x55;
      chunk0[1] = 0xaa;
      vm.loadBaseDiskChunk(0, chunk0);

      vm.stageKernel(kernel);
      vm.boot('console=ttyS0');
      expect(vm.getState()).toBe('running');
      const _initialDirty = vm.takeDirtyFramebuffer();
      expect(_initialDirty).not.toBeNull();

      // Run 4 steps ('j +64' + 'V' putchar)
      const firstSlice = vm.step(4);
      expect(firstSlice.outcome).toBe('executed');
      expect(decoder.decode(vm.drainSerialOutput())).toBe('V');

      // Save full VM + Disk snapshot mid-execution
      const snap = vm.saveFullSnapshot();
      expect(snap.byteLength).toBeGreaterThan(16 * 1024 * 1024);

      // Run remaining instructions to SRST PowerOff
      const secondSlice = vm.step(200);
      expect(secondSlice.outcome).toBe('poweroff');
      expect(decoder.decode(vm.drainSerialOutput())).toBe('ANILLA-OK');

      // Dirty framebuffer row 4 was recorded
      const fbDirty = vm.takeDirtyFramebuffer();
      expect(fbDirty).not.toBeNull();
      expect(fbDirty?.yMin).toBe(4);
      expect(fbDirty?.yMax).toBe(5);

      // Rewind to snapshot and verify deterministic replay of 'ANILLA-OK'!
      vm.restoreFullSnapshot(snap);
      const replaySlice = vm.step(200);
      expect(replaySlice.outcome).toBe('poweroff');
      expect(decoder.decode(vm.drainSerialOutput())).toBe('ANILLA-OK');

      const telemetry = vm.getTelemetry();
      expect(telemetry.guestRamBytes).toBe(16 * 1024 * 1024);
      expect(telemetry.diskCapacityBytes).toBe(4 * 1024 * 1024);

      const exportedBundle = vm.exportDiskBundle();
      vm.importDiskBundle(exportedBundle);

      vm.destroy();
      expect(vm.isDestroyed).toBe(true);
    }

    // Also verify HTTP response binary codec produces non-empty envelope
    const encodedHttp = encodeBridgeHttpResponse({
      requestId: 7n,
      statusCode: 200,
      statusText: 'OK',
      headers: [['Content-Type', 'application/octet-stream']],
      body: new Uint8Array([1, 2, 3]),
    });
    expect(encodedHttp.byteLength).toBeGreaterThan(20);
  });

  it('validates Worker RPC envelopes and executes cooperative fallback end-to-end', async () => {
    const events: WorkerMessage[] = [];
    const host = new VmEngineHost((m) => events.push(m));
    await host.handleRawMessage({ invalid: true });
    expect(events).toHaveLength(1);
    expect(events[0]?.kind).toBe('vm_error');
    host.dispose();

    // Cooperative VmClient test using a data: URL carrying the compiled WASM binary
    const base64Wasm = WASM_BYTES.toString('base64');
    const wasmDataUrl = `data:application/wasm;base64,${base64Wasm}`;

    let serialCollected = '';
    const decoder = new TextDecoder();
    let poweredOffPromiseResolve: (() => void) | null = null;
    const poweredOffPromise = new Promise<void>((res) => {
      poweredOffPromiseResolve = res;
    });

    const client = new VmClient(
      {
        onSerialOutput: (bytes) => {
          serialCollected += decoder.decode(bytes);
        },
        onLifecycleChange: (state) => {
          if (state === 'powered_off') {
            poweredOffPromiseResolve?.();
          }
        },
      },
      { forceInThreadFallback: true },
    );

    expect(client.transportMode).toBe('cooperative-fallback');
    await client.initVm({
      wasmUrl: wasmDataUrl,
      profileId: 'vanilla',
      ramMib: 16,
      diskMib: 4,
    });
    await client.bootVm({
      kernelImage: buildRiscvMiniSmokeKernel('CLIENT-BRIDGE-OK'),
      bootargs: 'console=ttyS0',
      sliceSteps: 5000,
    });

    await poweredOffPromise;
    expect(serialCollected).toBe('CLIENT-BRIDGE-OK');
    await client.destroyVm();
    client.dispose();
  });
});
