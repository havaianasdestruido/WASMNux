/**
 * Cooperative WASM VM execution host used inside `vm.worker.ts` (and fallback cooperative mode).
 */

import { executeBridgedHttpRequest } from '../net/network-bridge';
import { normalizeBasePath, splitAppBaseUrl } from '../runtime/asset-url';
import {
  VmRuntimeError,
  WasmVmRuntime,
  type BridgeHttpRequestMsg,
  type WasmVmSession,
} from '../wasm/bindings';
import { isWorkerCommand, type WorkerCommand, type WorkerMessage } from './protocol';

const DEFAULT_SLICE_STEPS = 250_000;
const TELEMETRY_INTERVAL_MS = 250;
const DISK_FLUSH_INTERVAL_MS = 200;

export type MessageSink = (msg: WorkerMessage, transfer?: Transferable[]) => void;

export class VmEngineHost {
  private readonly emit: MessageSink;
  private runtime: WasmVmRuntime | null = null;
  private cachedWasmUrl: string | null = null;
  private session: WasmVmSession | null = null;
  private loopTimer: ReturnType<typeof setTimeout> | null = null;
  private sliceSteps = DEFAULT_SLICE_STEPS;
  private lastTelemetryAt = 0;
  private lastDiskFlushAt = 0;
  private networkEnabled = true;
  private appBaseUrl: string | null = null;

  public constructor(emit: MessageSink) {
    this.emit = emit;
  }

  public async handleRawMessage(raw: unknown): Promise<void> {
    if (!isWorkerCommand(raw)) {
      this.emit({
        kind: 'vm_error',
        errorKind: 'invalid_argument',
        code: 3,
        message: 'Malformed WorkerCommand envelope rejected by runtime validator',
      });
      return;
    }
    await this.handleCommand(raw);
  }

  public async handleCommand(cmd: WorkerCommand): Promise<void> {
    try {
      switch (cmd.type) {
        case 'init_vm': {
          this.stopLoop();
          if (this.session !== null && !this.session.isDestroyed) {
            this.session.destroy();
          }
          this.session = null;

          if (this.runtime === null || this.cachedWasmUrl !== cmd.wasmUrl) {
            this.runtime = await WasmVmRuntime.fromUrl(cmd.wasmUrl);
            this.cachedWasmUrl = cmd.wasmUrl;
          }

          const vm = this.runtime.createVm({
            profileId: cmd.profileId,
            ramMib: cmd.ramMib,
            diskMib: cmd.diskMib,
          });
          for (const chunk of cmd.baseChunks) {
            vm.loadBaseDiskChunk(chunk.chunkIndex, chunk.data);
          }
          for (const chunk of cmd.overlayChunks) {
            vm.hydrateOverlayDiskChunk(chunk.chunkIndex, chunk.data, chunk.crc32);
          }
          vm.setDiskQuotaChunks(cmd.maxOverlayChunks);
          vm.setDiskReadOnly(cmd.readOnly);
          this.networkEnabled = cmd.networkEnabled;
          this.appBaseUrl = cmd.appBaseUrl ?? null;
          vm.setNetworkEnabled(cmd.networkEnabled);

          this.session = vm;
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId, state: vm.getState() });
          this.emit({ kind: 'vm_lifecycle', state: 'created', reason: 'initialized' });
          break;
        }
        case 'boot_vm': {
          const vm = this.requireSession();
          this.sliceSteps = Math.max(1_000, cmd.sliceSteps ?? DEFAULT_SLICE_STEPS);
          vm.stageKernel(cmd.kernelImage);
          if (cmd.initrdImage !== null && cmd.initrdImage.byteLength > 0) {
            vm.stageInitrd(cmd.initrdImage);
          }
          vm.boot(cmd.bootargs);
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId, state: vm.getState() });
          this.emit({ kind: 'vm_lifecycle', state: 'running', reason: 'booted' });
          this.scheduleSlice(0);
          break;
        }
        case 'pause_vm': {
          const vm = this.requireSession();
          this.stopLoop();
          vm.pause();
          this.flushDirtyOutputs(vm, true);
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId, state: 'paused' });
          this.emit({ kind: 'vm_lifecycle', state: 'paused', reason: 'user_paused' });
          break;
        }
        case 'resume_vm': {
          const vm = this.requireSession();
          vm.resume();
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId, state: 'running' });
          this.emit({ kind: 'vm_lifecycle', state: 'running', reason: 'user_resumed' });
          this.scheduleSlice(0);
          break;
        }
        case 'hard_reset_vm': {
          const vm = this.requireSession();
          this.stopLoop();
          vm.hardReset();
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId, state: 'running' });
          this.emit({ kind: 'vm_lifecycle', state: 'running', reason: 'hard_reset' });
          this.scheduleSlice(0);
          break;
        }
        case 'destroy_vm': {
          this.stopLoop();
          if (this.session !== null && !this.session.isDestroyed) {
            this.flushDirtyOutputs(this.session, true);
            this.session.destroy();
          }
          this.session = null;
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId, state: 'powered_off' });
          break;
        }
        case 'serial_input': {
          const vm = this.requireSession();
          vm.pushSerialInput(cmd.data);
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId });
          if (vm.getState() === 'running' && this.loopTimer === null) {
            this.scheduleSlice(0);
          }
          break;
        }
        case 'resize_fb': {
          const vm = this.requireSession();
          vm.resizeFramebuffer(cmd.width, cmd.height);
          this.flushDirtyOutputs(vm, false);
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId });
          break;
        }
        case 'save_snapshot': {
          const vm = this.requireSession();
          this.flushDirtyOutputs(vm, true);
          const snap = vm.saveFullSnapshot();
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId, binary: snap }, [snap.buffer]);
          break;
        }
        case 'restore_snapshot': {
          const vm = this.requireSession();
          this.stopLoop();
          vm.restoreFullSnapshot(cmd.snapshot);
          this.flushDirtyOutputs(vm, true);
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId, state: vm.getState() });
          this.emit({ kind: 'vm_lifecycle', state: 'running', reason: 'snapshot_restored' });
          this.scheduleSlice(0);
          break;
        }
        case 'export_disk_bundle': {
          const vm = this.requireSession();
          this.flushDirtyOutputs(vm, true);
          const bundle = vm.exportDiskBundle();
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId, binary: bundle }, [bundle.buffer]);
          break;
        }
        case 'import_disk_bundle': {
          const vm = this.requireSession();
          vm.importDiskBundle(cmd.bundle);
          this.flushDirtyOutputs(vm, true);
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId });
          break;
        }
        case 'reset_disk_overlay': {
          const vm = this.requireSession();
          vm.resetDiskOverlay();
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId });
          break;
        }
        case 'set_network_enabled': {
          const vm = this.requireSession();
          this.networkEnabled = cmd.enabled;
          vm.setNetworkEnabled(cmd.enabled);
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId });
          break;
        }
        case 'query_telemetry': {
          const vm = this.requireSession();
          this.emit({ kind: 'rpc_ok', reqId: cmd.reqId, telemetry: vm.getTelemetry() });
          break;
        }
      }
    } catch (err: unknown) {
      const vmErr = toVmError(err);
      this.emit({
        kind: 'rpc_err',
        reqId: cmd.reqId,
        errorKind: vmErr.kind,
        code: vmErr.code,
        message: vmErr.message,
      });
    }
  }

  public dispose(): void {
    this.stopLoop();
    if (this.session !== null && !this.session.isDestroyed) {
      this.session.destroy();
    }
    this.session = null;
  }

  private requireSession(): WasmVmSession {
    if (this.session === null || this.session.isDestroyed) {
      throw new VmRuntimeError('invalid_handle', 1, 'No active VM session initialized');
    }
    return this.session;
  }

  private stopLoop(): void {
    if (this.loopTimer !== null) {
      clearTimeout(this.loopTimer);
      this.loopTimer = null;
    }
  }

  private scheduleSlice(delayMs: number): void {
    if (this.loopTimer !== null) {
      return;
    }
    this.loopTimer = setTimeout(() => {
      this.loopTimer = null;
      this.runOneSlice();
    }, delayMs);
  }

  private runOneSlice(): void {
    const vm = this.session;
    if (vm === null || vm.isDestroyed || vm.getState() !== 'running') {
      return;
    }

    try {
      const result = vm.step(this.sliceSteps);
      const now = performance.now();
      const forceDisk =
        result.outcome === 'poweroff' ||
        result.outcome === 'reboot' ||
        now - this.lastDiskFlushAt >= DISK_FLUSH_INTERVAL_MS;

      this.flushDirtyOutputs(vm, forceDisk);
      if (forceDisk) {
        this.lastDiskFlushAt = now;
      }

      if (now - this.lastTelemetryAt >= TELEMETRY_INTERVAL_MS) {
        this.lastTelemetryAt = now;
        this.emit({ kind: 'vm_telemetry', telemetry: vm.getTelemetry() });
      }

      const httpReqs = vm.drainHttpRequests();
      for (const req of httpReqs) {
        void this.dispatchBridgedHttp(vm, req);
      }

      if (result.outcome === 'poweroff') {
        this.emit({ kind: 'vm_lifecycle', state: 'powered_off', reason: 'guest_poweroff' });
        return;
      }
      if (result.outcome === 'reboot') {
        vm.hardReset();
        this.emit({ kind: 'vm_lifecycle', state: 'running', reason: 'guest_reboot' });
        this.scheduleSlice(0);
        return;
      }

      const nextDelay = result.outcome === 'wfi_idle' ? 6 : 0;
      this.scheduleSlice(nextDelay);
    } catch (err: unknown) {
      const vmErr = toVmError(err);
      this.emit({
        kind: 'vm_error',
        errorKind: vmErr.kind,
        code: vmErr.code,
        message: vmErr.message,
      });
    }
  }

  private flushDirtyOutputs(vm: WasmVmSession, drainDisk: boolean): void {
    const serial = vm.drainSerialOutput();
    if (serial.byteLength > 0) {
      this.emit({ kind: 'serial_output', data: serial }, [serial.buffer]);
    }
    const fb = vm.takeDirtyFramebuffer();
    if (fb !== null) {
      this.emit({ kind: 'fb_dirty', update: fb }, [fb.pixels.buffer]);
    }
    if (drainDisk) {
      const dirtyChunks = vm.drainDirtyDiskChunks();
      if (dirtyChunks.length > 0) {
        this.emit({
          kind: 'disk_dirty',
          profileId: vm.profileId,
          chunks: dirtyChunks,
        });
      }
    }
  }

  /**
   * Resolve the published application origin + sub-path used for guest mirror requests.
   *
   * `appBaseUrl` is supplied by the main thread (`BootOrchestrator`) because a module Worker's
   * own `location` points at its bundled `assets/*.js` chunk rather than the app root on
   * sub-path deployments such as GitHub Pages project sites.
   */
  private resolveMirrorLocation(): { readonly origin?: string; readonly basePath?: string } {
    const parts = this.appBaseUrl === null ? null : splitAppBaseUrl(this.appBaseUrl);
    if (parts === null) {
      return {};
    }
    return {
      origin: parts.origin,
      basePath: normalizeBasePath(parts.basePath),
    };
  }

  private async dispatchBridgedHttp(vm: WasmVmSession, req: BridgeHttpRequestMsg): Promise<void> {
    const response = await executeBridgedHttpRequest(req, {
      networkEnabled: this.networkEnabled,
      ...this.resolveMirrorLocation(),
    });
    if (!vm.isDestroyed) {
      vm.completeHttpResponse(response);
      if (vm.getState() === 'running' && this.loopTimer === null) {
        this.scheduleSlice(0);
      }
    }
  }
}

function toVmError(err: unknown): VmRuntimeError {
  if (err instanceof VmRuntimeError) {
    return err;
  }
  if (err instanceof Error) {
    return new VmRuntimeError('invalid_argument', 3, err.message);
  }
  return new VmRuntimeError('invalid_argument', 3, String(err));
}
