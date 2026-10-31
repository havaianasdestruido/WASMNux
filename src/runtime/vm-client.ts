/**
 * Main-thread typed client for the WASMNux Web Worker (with cooperative fallback if Worker is unavailable).
 */

import {
  VmRuntimeError,
  type DirtyDiskChunk,
  type FramebufferDirtyUpdate,
  type VmLifecycleStateName,
  type VmTelemetry,
} from '../wasm/bindings';
import {
  isWorkerMessage,
  type BaseChunkInput,
  type WorkerCommand,
  type WorkerMessage,
} from '../worker/protocol';
import { VmEngineHost } from '../worker/vm-engine-host';

export interface VmClientListeners {
  readonly onSerialOutput?: (bytes: Uint8Array) => void;
  readonly onFramebufferDirty?: (update: FramebufferDirtyUpdate) => void;
  readonly onDiskDirty?: (profileId: string, chunks: readonly DirtyDiskChunk[]) => void;
  readonly onLifecycleChange?: (state: VmLifecycleStateName, reason: string) => void;
  readonly onTelemetry?: (telemetry: VmTelemetry) => void;
  readonly onError?: (err: VmRuntimeError) => void;
}

export interface InitVmParams {
  readonly wasmUrl: string;
  readonly profileId: string;
  readonly ramMib: number;
  readonly diskMib: number;
  readonly baseChunks?: readonly BaseChunkInput[];
  readonly overlayChunks?: readonly BaseChunkInput[];
  readonly maxOverlayChunks?: number | null;
  readonly readOnly?: boolean;
  readonly networkEnabled?: boolean;
}

export interface BootVmParams {
  readonly kernelImage: Uint8Array;
  readonly initrdImage?: Uint8Array | null;
  readonly bootargs: string;
  readonly sliceSteps?: number;
}

interface PendingRpc {
  readonly resolve: (msg: Extract<WorkerMessage, { kind: 'rpc_ok' }>) => void;
  readonly reject: (err: VmRuntimeError) => void;
}

export interface VmClientOptions {
  readonly forceInThreadFallback?: boolean;
  readonly createWorker?: () => Worker;
}

export class VmClient {
  private worker: Worker | null = null;
  private fallbackHost: VmEngineHost | null = null;
  private readonly listeners: VmClientListeners;
  private readonly pending = new Map<number, PendingRpc>();
  private nextReqId = 1;
  private disposed = false;
  public readonly transportMode: 'worker' | 'cooperative-fallback';

  public constructor(listeners: VmClientListeners = {}, opts: VmClientOptions = {}) {
    this.listeners = listeners;
    const canUseWorker = !opts.forceInThreadFallback && typeof Worker === 'function';

    if (canUseWorker) {
      this.transportMode = 'worker';
      const w = opts.createWorker
        ? opts.createWorker()
        : new Worker(new URL('../worker/vm.worker.ts', import.meta.url), { type: 'module' });
      w.onmessage = (ev: MessageEvent<unknown>): void => {
        this.handleInbound(ev.data);
      };
      w.onerror = (ev: ErrorEvent): void => {
        const err = new VmRuntimeError('boot_error', 5, ev.message || 'Web Worker runtime error');
        this.listeners.onError?.(err);
      };
      this.worker = w;
    } else {
      this.transportMode = 'cooperative-fallback';
      this.fallbackHost = new VmEngineHost((msg) => {
        this.handleInbound(msg);
      });
    }
  }

  public async initVm(params: InitVmParams): Promise<VmLifecycleStateName> {
    const reqId = this.allocReqId();
    const ok = await this.sendRpc({
      type: 'init_vm',
      reqId,
      wasmUrl: params.wasmUrl,
      profileId: params.profileId,
      ramMib: params.ramMib,
      diskMib: params.diskMib,
      baseChunks: params.baseChunks ?? [],
      overlayChunks: params.overlayChunks ?? [],
      maxOverlayChunks: params.maxOverlayChunks ?? null,
      readOnly: params.readOnly ?? false,
      networkEnabled: params.networkEnabled ?? true,
    });
    return ok.state ?? 'created';
  }

  public async bootVm(params: BootVmParams): Promise<VmLifecycleStateName> {
    const reqId = this.allocReqId();
    const cmd: WorkerCommand =
      params.sliceSteps !== undefined
        ? {
            type: 'boot_vm',
            reqId,
            kernelImage: params.kernelImage,
            initrdImage: params.initrdImage ?? null,
            bootargs: params.bootargs,
            sliceSteps: params.sliceSteps,
          }
        : {
            type: 'boot_vm',
            reqId,
            kernelImage: params.kernelImage,
            initrdImage: params.initrdImage ?? null,
            bootargs: params.bootargs,
          };
    const ok = await this.sendRpc(cmd);
    return ok.state ?? 'running';
  }

  public async pauseVm(): Promise<void> {
    await this.sendRpc({ type: 'pause_vm', reqId: this.allocReqId() });
  }

  public async resumeVm(): Promise<void> {
    await this.sendRpc({ type: 'resume_vm', reqId: this.allocReqId() });
  }

  public async hardResetVm(): Promise<void> {
    await this.sendRpc({ type: 'hard_reset_vm', reqId: this.allocReqId() });
  }

  public async sendSerialInput(input: Uint8Array | string): Promise<void> {
    const data = typeof input === 'string' ? new TextEncoder().encode(input) : input;
    await this.sendRpc({ type: 'serial_input', reqId: this.allocReqId(), data });
  }

  public async resizeFramebuffer(width: number, height: number): Promise<void> {
    await this.sendRpc({ type: 'resize_fb', reqId: this.allocReqId(), width, height });
  }

  public async saveSnapshot(): Promise<Uint8Array> {
    const ok = await this.sendRpc({ type: 'save_snapshot', reqId: this.allocReqId() });
    if (!ok.binary) {
      throw new VmRuntimeError('snapshot_error', 6, 'Worker returned empty snapshot payload');
    }
    return ok.binary;
  }

  public async restoreSnapshot(snapshot: Uint8Array): Promise<void> {
    await this.sendRpc({
      type: 'restore_snapshot',
      reqId: this.allocReqId(),
      snapshot,
    });
  }

  public async exportDiskBundle(): Promise<Uint8Array> {
    const ok = await this.sendRpc({ type: 'export_disk_bundle', reqId: this.allocReqId() });
    if (!ok.binary) {
      throw new VmRuntimeError('storage_error', 4, 'Worker returned empty disk bundle');
    }
    return ok.binary;
  }

  public async importDiskBundle(bundle: Uint8Array): Promise<void> {
    await this.sendRpc({
      type: 'import_disk_bundle',
      reqId: this.allocReqId(),
      bundle,
    });
  }

  public async resetDiskOverlay(): Promise<void> {
    await this.sendRpc({ type: 'reset_disk_overlay', reqId: this.allocReqId() });
  }

  public async setNetworkEnabled(enabled: boolean): Promise<void> {
    await this.sendRpc({ type: 'set_network_enabled', reqId: this.allocReqId(), enabled });
  }

  public async queryTelemetry(): Promise<VmTelemetry> {
    const ok = await this.sendRpc({ type: 'query_telemetry', reqId: this.allocReqId() });
    if (!ok.telemetry) {
      throw new VmRuntimeError('invalid_argument', 3, 'Worker omitted telemetry payload');
    }
    return ok.telemetry;
  }

  public async destroyVm(): Promise<void> {
    if (this.disposed) {
      return;
    }
    await this.sendRpc({ type: 'destroy_vm', reqId: this.allocReqId() });
  }

  public dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    for (const [, p] of this.pending) {
      p.reject(new VmRuntimeError('invalid_handle', 1, 'VmClient disposed before RPC completion'));
    }
    this.pending.clear();
    if (this.worker !== null) {
      this.worker.terminate();
      this.worker = null;
    }
    if (this.fallbackHost !== null) {
      this.fallbackHost.dispose();
      this.fallbackHost = null;
    }
  }

  private allocReqId(): number {
    const id = this.nextReqId;
    this.nextReqId += 1;
    return id;
  }

  private sendRpc(cmd: WorkerCommand): Promise<Extract<WorkerMessage, { kind: 'rpc_ok' }>> {
    if (this.disposed) {
      return Promise.reject(new VmRuntimeError('invalid_handle', 1, 'VmClient is disposed'));
    }
    return new Promise((resolve, reject) => {
      this.pending.set(cmd.reqId, { resolve, reject });
      if (this.worker !== null) {
        this.worker.postMessage(cmd);
      } else if (this.fallbackHost !== null) {
        void this.fallbackHost.handleCommand(cmd);
      }
    });
  }

  private handleInbound(raw: unknown): void {
    if (!isWorkerMessage(raw)) {
      this.listeners.onError?.(
        new VmRuntimeError('invalid_argument', 3, 'Malformed WorkerMessage envelope rejected'),
      );
      return;
    }
    switch (raw.kind) {
      case 'rpc_ok': {
        const entry = this.pending.get(raw.reqId);
        if (entry) {
          this.pending.delete(raw.reqId);
          entry.resolve(raw);
        }
        break;
      }
      case 'rpc_err': {
        const err = new VmRuntimeError(raw.errorKind, raw.code, raw.message);
        const entry = this.pending.get(raw.reqId);
        if (entry) {
          this.pending.delete(raw.reqId);
          entry.reject(err);
        } else {
          this.listeners.onError?.(err);
        }
        break;
      }
      case 'serial_output':
        this.listeners.onSerialOutput?.(raw.data);
        break;
      case 'fb_dirty':
        this.listeners.onFramebufferDirty?.(raw.update);
        break;
      case 'disk_dirty':
        this.listeners.onDiskDirty?.(raw.profileId, raw.chunks);
        break;
      case 'vm_lifecycle':
        this.listeners.onLifecycleChange?.(raw.state, raw.reason);
        break;
      case 'vm_telemetry':
        this.listeners.onTelemetry?.(raw.telemetry);
        break;
      case 'vm_error':
        this.listeners.onError?.(new VmRuntimeError(raw.errorKind, raw.code, raw.message));
        break;
    }
  }
}
