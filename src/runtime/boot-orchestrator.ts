/**
 * High-level Linux VM boot & persistent storage orchestrator (`BootOrchestrator`).
 * Connects `VmClient` (Worker WASM execution) with `PersistentStorageEngine` (OPFS + IndexedDB).
 */

import type { ProfileManifestRecord } from '../storage/idb-backend';
import type { PersistentStorageEngine } from '../storage/storage-engine';
import type { DirtyDiskChunk } from '../wasm/bindings';
import type { BaseChunkInput } from '../worker/protocol';
import {
  decompressGzipIfNeeded,
  extractNonZeroBaseChunks,
  getLinuxProfile,
  type LinuxProfileDefinition,
  type LinuxProfileId,
} from './profiles';
import { VmClient, type VmClientListeners } from './vm-client';

export interface BootProgressEvent {
  readonly phase:
    | 'recovering_storage'
    | 'loading_kernel'
    | 'loading_initrd'
    | 'loading_base_disk'
    | 'initializing_vm'
    | 'booting_kernel'
    | 'running';
  readonly detail: string;
  readonly elapsedMs: number;
}

export interface BootOrchestratorOptions {
  readonly storage: PersistentStorageEngine;
  readonly wasmUrl?: string;
  readonly events?: VmClientListeners & {
    readonly onBootProgress?: (ev: BootProgressEvent) => void;
    readonly onDiskCommitted?: (manifest: ProfileManifestRecord) => void;
    readonly onStorageError?: (err: Error) => void;
  };
  readonly assetFetcher?: (url: string) => Promise<Uint8Array>;
}

interface CachedBaseAssets {
  readonly kernel: Uint8Array;
  readonly initrd: Uint8Array;
  readonly baseChunks: readonly BaseChunkInput[];
}

export class BootOrchestrator {
  public readonly storage: PersistentStorageEngine;
  private readonly wasmUrl: string;
  private readonly events: BootOrchestratorOptions['events'];
  private readonly fetchAsset: (url: string) => Promise<Uint8Array>;
  private readonly baseAssetCache = new Map<LinuxProfileId, CachedBaseAssets>();

  private client: VmClient | null = null;
  private activeProfile: LinuxProfileDefinition | null = null;
  private pendingFlushChain: Promise<void> = Promise.resolve();
  private networkEnabled = true;

  public constructor(opts: BootOrchestratorOptions) {
    this.storage = opts.storage;
    this.wasmUrl = opts.wasmUrl ?? '/wasm/wasmnux_wasm.wasm';
    this.events = opts.events;
    this.fetchAsset =
      opts.assetFetcher ??
      (async (url: string): Promise<Uint8Array> => {
        const resp = await fetch(url);
        if (!resp.ok) {
          throw new Error(`Failed to fetch guest asset ${url} (HTTP ${String(resp.status)})`);
        }
        return new Uint8Array(await resp.arrayBuffer());
      });
  }

  public get activeProfileId(): LinuxProfileId | null {
    return this.activeProfile?.id ?? null;
  }

  public get vmClient(): VmClient | null {
    return this.client;
  }

  public async bootProfile(
    profileId: LinuxProfileId,
    overrides?: {
      readonly ramMib?: number;
      readonly readOnlyDisk?: boolean;
      readonly networkEnabled?: boolean;
    },
  ): Promise<{ readonly bootReadyMs: number; readonly recoveredOrphanChunks: number }> {
    const t0 = performance.now();
    await this.shutdown();

    const profile = getLinuxProfile(profileId);
    this.activeProfile = profile;
    const capacityBytes = profile.diskMib * 1024 * 1024;
    this.networkEnabled = overrides?.networkEnabled ?? true;

    this.reportProgress(t0, 'recovering_storage', `Recovering ${profile.name} persistent disk...`);
    const recovered = await this.storage.openAndRecoverProfile(profile.id, capacityBytes);

    let assets = this.baseAssetCache.get(profile.id);
    if (!assets) {
      this.reportProgress(t0, 'loading_kernel', `Loading Linux ${profile.kernelVersion} kernel...`);
      const kernelGz = await this.fetchAsset(profile.kernelUrl);
      const kernel = await decompressGzipIfNeeded(kernelGz);

      this.reportProgress(t0, 'loading_initrd', `Loading ${profile.name} initramfs...`);
      const initrdGz = await this.fetchAsset(profile.initrdUrl);
      const initrd = await decompressGzipIfNeeded(initrdGz);

      this.reportProgress(t0, 'loading_base_disk', `Preparing ${profile.name} ext4 disk...`);
      const diskGz = await this.fetchAsset(profile.baseDiskUrl);
      const rawDisk = await decompressGzipIfNeeded(diskGz);
      const baseChunks = extractNonZeroBaseChunks(rawDisk);

      assets = { kernel, initrd, baseChunks };
      this.baseAssetCache.set(profile.id, assets);
    }

    this.reportProgress(t0, 'initializing_vm', 'Initializing RV64GC WASM VM in Web Worker...');
    const client = new VmClient({
      onSerialOutput: (data) => this.events?.onSerialOutput?.(data),
      onFramebufferDirty: (upd) => this.events?.onFramebufferDirty?.(upd),
      onLifecycleChange: (state, reason) => this.events?.onLifecycleChange?.(state, reason),
      onTelemetry: (tel) => this.events?.onTelemetry?.(tel),
      onError: (err) => this.events?.onError?.(err),
      onDiskDirty: (pid, chunks) => {
        this.enqueueDiskCommit(pid, capacityBytes, chunks);
        this.events?.onDiskDirty?.(pid, chunks);
      },
    });
    this.client = client;

    await client.initVm({
      wasmUrl: this.wasmUrl,
      profileId: profile.id,
      ramMib: overrides?.ramMib ?? profile.defaultRamMib,
      diskMib: profile.diskMib,
      baseChunks: assets.baseChunks,
      overlayChunks: recovered.overlayChunks,
      maxOverlayChunks: null,
      readOnly: overrides?.readOnlyDisk ?? false,
      networkEnabled: this.networkEnabled,
    });

    this.reportProgress(t0, 'booting_kernel', `Booting Linux ${profile.kernelVersion}...`);
    await client.bootVm({
      kernelImage: assets.kernel.slice(),
      initrdImage: assets.initrd.slice(),
      bootargs: profile.bootargs,
    });

    const bootReadyMs = performance.now() - t0;
    this.reportProgress(t0, 'running', `${profile.name} running`);
    return {
      bootReadyMs,
      recoveredOrphanChunks: recovered.recoveredOrphanChunks,
    };
  }

  public sendSerialInput(data: Uint8Array | string): Promise<void> {
    if (!this.client) {
      return Promise.reject(new Error('No active VM session'));
    }
    return this.client.sendSerialInput(data);
  }

  public async waitForPendingStorageCommits(): Promise<void> {
    await this.pendingFlushChain;
  }

  public async shutdown(): Promise<void> {
    if (this.client !== null) {
      await this.client.destroyVm();
      this.client.dispose();
      this.client = null;
    }
    await this.pendingFlushChain;
    this.activeProfile = null;
  }

  public async resetProfileStorage(profileId: LinuxProfileId): Promise<void> {
    if (this.activeProfile?.id === profileId) {
      await this.shutdown();
    }
    const profile = getLinuxProfile(profileId);
    await this.storage.resetProfileDisk(profile.id, profile.diskMib * 1024 * 1024);
  }

  private enqueueDiskCommit(
    profileId: string,
    capacityBytes: number,
    chunks: readonly DirtyDiskChunk[],
  ): void {
    const run = this.pendingFlushChain.then(async () => {
      try {
        const manifest = await this.storage.commitDirtyChunks(profileId, capacityBytes, chunks);
        this.events?.onDiskCommitted?.(manifest);
      } catch (err: unknown) {
        const errorObj = err instanceof Error ? err : new Error(String(err));
        this.events?.onStorageError?.(errorObj);
      }
    });
    this.pendingFlushChain = run;
  }

  private reportProgress(t0: number, phase: BootProgressEvent['phase'], detail: string): void {
    this.events?.onBootProgress?.({
      phase,
      detail,
      elapsedMs: performance.now() - t0,
    });
  }
}
