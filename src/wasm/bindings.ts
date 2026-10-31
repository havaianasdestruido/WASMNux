/**
 * Strict, zero-`any` TypeScript bindings for `public/wasm/wasmnux_wasm.wasm` (ABI v1).
 */

export const EXPECTED_WASM_ABI_VERSION = 1;
export const DISK_CHUNK_BYTES = 64 * 1024;
export const MIN_GUEST_RAM_MIB = 16;
export const MAX_GUEST_RAM_MIB = 1024;
export const DEFAULT_VANILLA_RAM_MIB = 128;
export const DEFAULT_DEBIAN_RAM_MIB = 256;

export type WasmErrorKind =
  | 'ok'
  | 'invalid_handle'
  | 'out_of_bounds'
  | 'invalid_argument'
  | 'storage_error'
  | 'boot_error'
  | 'snapshot_error'
  | 'lifecycle_error'
  | 'abi_mismatch';

export class VmRuntimeError extends Error {
  public readonly kind: WasmErrorKind;
  public readonly code: number;

  public constructor(kind: WasmErrorKind, code: number, message: string) {
    super(message);
    this.name = 'VmRuntimeError';
    this.kind = kind;
    this.code = code;
  }
}

export type VmStepOutcomeKind = 'executed' | 'wfi_idle' | 'poweroff' | 'reboot';

export interface VmStepResult {
  readonly outcome: VmStepOutcomeKind;
  readonly stepsExecuted: number;
}

export type VmLifecycleStateName =
  'created' | 'running' | 'paused' | 'powered_off' | 'reboot_requested';

export interface FramebufferDirtyUpdate {
  readonly width: number;
  readonly height: number;
  readonly yMin: number;
  readonly yMax: number;
  /** Tightly packed `XRGB8888` rows covering `[yMin, yMax)` (`(yMax - yMin) * width * 4` bytes). */
  readonly pixels: Uint8Array;
}

export interface DirtyDiskChunk {
  readonly chunkIndex: number;
  readonly crc32: number;
  readonly data: Uint8Array;
}

export interface BridgeHttpRequestMsg {
  readonly requestId: bigint;
  readonly method: string;
  readonly host: string;
  readonly path: string;
  readonly headers: ReadonlyArray<readonly [string, string]>;
  readonly body: Uint8Array;
}

export interface BridgeHttpResponseMsg {
  readonly requestId: bigint;
  readonly statusCode: number;
  readonly statusText: string;
  readonly headers: ReadonlyArray<readonly [string, string]>;
  readonly body: Uint8Array;
}

export interface VmTelemetry {
  readonly totalSteps: bigint;
  readonly mtimeTicks: bigint;
  readonly guestRamBytes: number;
  readonly wasmLinearMemoryBytes: number;
  readonly diskCapacityBytes: number;
  readonly diskOverlayBytes: number;
  readonly diskBytesRead: number;
  readonly diskBytesWritten: number;
  readonly netRxBytes: number;
}

export interface CreateVmOptions {
  readonly profileId: string;
  readonly ramMib: number;
  readonly diskMib: number;
  readonly epochSeconds?: number;
}

interface RawWasmExports {
  readonly memory: WebAssembly.Memory;
  readonly wasmnux_abi_version: () => number;
  readonly wasmnux_io_buf_resize: (len: number) => number;
  readonly wasmnux_io_buf_ptr: () => number;
  readonly wasmnux_io_buf_len: () => number;
  readonly wasmnux_last_error_code: () => number;
  readonly wasmnux_last_error_read: () => number;
  readonly wasmnux_vm_create: (
    ramMib: number,
    diskMib: number,
    epochLo: number,
    epochHi: number,
    profileLen: number,
  ) => number;
  readonly wasmnux_vm_destroy: (handle: number) => number;
  readonly wasmnux_vm_stage_kernel: (handle: number, len: number) => number;
  readonly wasmnux_vm_stage_initrd: (handle: number, len: number) => number;
  readonly wasmnux_vm_boot: (handle: number, bootargsLen: number) => number;
  readonly wasmnux_vm_step: (handle: number, maxSteps: number) => number;
  readonly wasmnux_vm_pause: (handle: number) => number;
  readonly wasmnux_vm_resume: (handle: number) => number;
  readonly wasmnux_vm_hard_reset: (handle: number) => number;
  readonly wasmnux_vm_state: (handle: number) => number;
  readonly wasmnux_uart_push_input: (handle: number, len: number) => number;
  readonly wasmnux_uart_drain_output: (handle: number) => number;
  readonly wasmnux_fb_ptr: (handle: number) => number;
  readonly wasmnux_fb_byte_len: (handle: number) => number;
  readonly wasmnux_fb_width: (handle: number) => number;
  readonly wasmnux_fb_height: (handle: number) => number;
  readonly wasmnux_fb_resize: (handle: number, width: number, height: number) => number;
  readonly wasmnux_fb_take_dirty: (handle: number) => bigint;
  readonly wasmnux_disk_load_base_chunk: (handle: number, chunkIndex: number) => number;
  readonly wasmnux_disk_hydrate_overlay_chunk: (
    handle: number,
    chunkIndex: number,
    hasCrc: number,
    expectedCrc32: number,
  ) => number;
  readonly wasmnux_disk_set_quota_chunks: (handle: number, maxChunks: number) => number;
  readonly wasmnux_disk_set_read_only: (handle: number, readOnly: number) => number;
  readonly wasmnux_disk_reset_overlay: (handle: number) => number;
  readonly wasmnux_disk_pop_dirty_chunk: (handle: number) => number;
  readonly wasmnux_disk_last_dirty_crc32: (handle: number) => number;
  readonly wasmnux_disk_requeue_dirty_chunk: (handle: number, chunkIndex: number) => number;
  readonly wasmnux_disk_export_bundle: (handle: number) => number;
  readonly wasmnux_disk_import_bundle: (handle: number, len: number) => number;
  readonly wasmnux_vm_save_full_snapshot: (handle: number) => number;
  readonly wasmnux_vm_restore_full_snapshot: (handle: number, len: number) => number;
  readonly wasmnux_net_set_enabled: (handle: number, enabled: number) => number;
  readonly wasmnux_net_pop_http_request: (handle: number) => number;
  readonly wasmnux_net_complete_http_response: (handle: number, len: number) => number;
  readonly wasmnux_vm_telemetry: (handle: number) => number;
}

const REQUIRED_FN_NAMES = [
  'wasmnux_abi_version',
  'wasmnux_io_buf_resize',
  'wasmnux_io_buf_ptr',
  'wasmnux_io_buf_len',
  'wasmnux_last_error_code',
  'wasmnux_last_error_read',
  'wasmnux_vm_create',
  'wasmnux_vm_destroy',
  'wasmnux_vm_stage_kernel',
  'wasmnux_vm_stage_initrd',
  'wasmnux_vm_boot',
  'wasmnux_vm_step',
  'wasmnux_vm_pause',
  'wasmnux_vm_resume',
  'wasmnux_vm_hard_reset',
  'wasmnux_vm_state',
  'wasmnux_uart_push_input',
  'wasmnux_uart_drain_output',
  'wasmnux_fb_ptr',
  'wasmnux_fb_byte_len',
  'wasmnux_fb_width',
  'wasmnux_fb_height',
  'wasmnux_fb_resize',
  'wasmnux_fb_take_dirty',
  'wasmnux_disk_load_base_chunk',
  'wasmnux_disk_hydrate_overlay_chunk',
  'wasmnux_disk_set_quota_chunks',
  'wasmnux_disk_set_read_only',
  'wasmnux_disk_reset_overlay',
  'wasmnux_disk_pop_dirty_chunk',
  'wasmnux_disk_last_dirty_crc32',
  'wasmnux_disk_requeue_dirty_chunk',
  'wasmnux_disk_export_bundle',
  'wasmnux_disk_import_bundle',
  'wasmnux_vm_save_full_snapshot',
  'wasmnux_vm_restore_full_snapshot',
  'wasmnux_net_set_enabled',
  'wasmnux_net_pop_http_request',
  'wasmnux_net_complete_http_response',
  'wasmnux_vm_telemetry',
] as const;

function mapErrorCode(code: number): WasmErrorKind {
  switch (code) {
    case 0:
      return 'ok';
    case 1:
      return 'invalid_handle';
    case 2:
      return 'out_of_bounds';
    case 3:
      return 'invalid_argument';
    case 4:
      return 'storage_error';
    case 5:
      return 'boot_error';
    case 6:
      return 'snapshot_error';
    case 7:
      return 'lifecycle_error';
    default:
      return 'invalid_argument';
  }
}

function validateRawExports(exportsObj: WebAssembly.Exports): RawWasmExports {
  const mem = exportsObj['memory'];
  if (!(mem instanceof WebAssembly.Memory)) {
    throw new VmRuntimeError('abi_mismatch', -1, 'WASM module does not export linear memory');
  }
  for (const fnName of REQUIRED_FN_NAMES) {
    if (typeof exportsObj[fnName] !== 'function') {
      throw new VmRuntimeError(
        'abi_mismatch',
        -1,
        `WASM module missing required export function: ${fnName}`,
      );
    }
  }
  // Safe structural return after verifying every required property at runtime.
  return exportsObj as unknown as RawWasmExports;
}

const utf8Encoder = new TextEncoder();
const utf8Decoder = new TextDecoder('utf-8', { fatal: false });

export class WasmVmRuntime {
  private readonly raw: RawWasmExports;

  private constructor(raw: RawWasmExports) {
    this.raw = raw;
  }

  public static async fromBytes(wasmBytes: BufferSource): Promise<WasmVmRuntime> {
    const result = await WebAssembly.instantiate(wasmBytes, {});
    const instance = 'instance' in result ? result.instance : result;
    const raw = validateRawExports(instance.exports);
    const abi = raw.wasmnux_abi_version() >>> 0;
    if (abi !== EXPECTED_WASM_ABI_VERSION) {
      throw new VmRuntimeError(
        'abi_mismatch',
        abi,
        `Incompatible WASM ABI version ${String(abi)} (expected ${String(EXPECTED_WASM_ABI_VERSION)})`,
      );
    }
    return new WasmVmRuntime(raw);
  }

  public static async fromUrl(url: string): Promise<WasmVmRuntime> {
    const resp = await fetch(url);
    if (!resp.ok) {
      throw new VmRuntimeError(
        'boot_error',
        resp.status,
        `Failed fetching WASM binary from ${url}: HTTP ${String(resp.status)}`,
      );
    }
    const buf = await resp.arrayBuffer();
    return WasmVmRuntime.fromBytes(buf);
  }

  public get abiVersion(): number {
    return this.raw.wasmnux_abi_version() >>> 0;
  }

  public get linearMemoryBytes(): number {
    return this.raw.memory.buffer.byteLength;
  }

  public stageInput(bytes: Uint8Array): void {
    const ptr = this.raw.wasmnux_io_buf_resize(bytes.byteLength) >>> 0;
    if (ptr === 0) {
      throw this.readLastError('Failed resizing WASM I/O buffer');
    }
    const memView = new Uint8Array(this.raw.memory.buffer);
    if (ptr + bytes.byteLength > memView.byteLength) {
      throw new VmRuntimeError('out_of_bounds', 2, 'WASM I/O buffer pointer exceeds linear memory');
    }
    memView.set(bytes, ptr);
  }

  public readOutput(byteLen: number): Uint8Array {
    if (byteLen === 0) {
      return new Uint8Array(0);
    }
    const ptr = this.raw.wasmnux_io_buf_ptr() >>> 0;
    const cap = this.raw.wasmnux_io_buf_len() >>> 0;
    if (byteLen > cap || ptr + byteLen > this.raw.memory.buffer.byteLength) {
      throw new VmRuntimeError('out_of_bounds', 2, 'WASM output slice out of linear memory bounds');
    }
    return new Uint8Array(this.raw.memory.buffer, ptr, byteLen).slice();
  }

  public readLastError(fallbackMessage: string): VmRuntimeError {
    const code = this.raw.wasmnux_last_error_code() >>> 0;
    const msgLen = this.raw.wasmnux_last_error_read() >>> 0;
    const msgBytes = msgLen > 0 ? this.readOutput(msgLen) : new Uint8Array(0);
    const detail = msgBytes.byteLength > 0 ? utf8Decoder.decode(msgBytes) : fallbackMessage;
    return new VmRuntimeError(mapErrorCode(code), code, detail);
  }

  public createVm(opts: CreateVmOptions): WasmVmSession {
    const idBytes = utf8Encoder.encode(opts.profileId);
    this.stageInput(idBytes);
    const epochSec = Math.max(0, Math.floor(opts.epochSeconds ?? Date.now() / 1000));
    const epochLo = epochSec >>> 0;
    const epochHi = Math.floor(epochSec / 0x1_0000_0000) >>> 0;
    const handle =
      this.raw.wasmnux_vm_create(
        opts.ramMib >>> 0,
        opts.diskMib >>> 0,
        epochLo,
        epochHi,
        idBytes.byteLength,
      ) >>> 0;
    if (handle === 0) {
      throw this.readLastError('Failed creating WASM VM instance');
    }
    return new WasmVmSession(this, this.raw, handle, opts.profileId);
  }
}

export class WasmVmSession {
  private readonly runtime: WasmVmRuntime;
  private readonly raw: RawWasmExports;
  private handle: number;
  public readonly profileId: string;

  public constructor(
    runtime: WasmVmRuntime,
    raw: RawWasmExports,
    handle: number,
    profileId: string,
  ) {
    this.runtime = runtime;
    this.raw = raw;
    this.handle = handle;
    this.profileId = profileId;
  }

  private ensureActive(): number {
    if (this.handle === 0) {
      throw new VmRuntimeError('invalid_handle', 1, 'VM session has already been destroyed');
    }
    return this.handle;
  }

  public get isDestroyed(): boolean {
    return this.handle === 0;
  }

  public destroy(): void {
    if (this.handle === 0) {
      return;
    }
    const h = this.handle;
    this.handle = 0;
    const rc = this.raw.wasmnux_vm_destroy(h);
    if (rc !== 0) {
      throw this.runtime.readLastError('Failed destroying VM handle');
    }
  }

  public stageKernel(kernelImage: Uint8Array): void {
    const h = this.ensureActive();
    this.runtime.stageInput(kernelImage);
    if (this.raw.wasmnux_vm_stage_kernel(h, kernelImage.byteLength) !== 0) {
      throw this.runtime.readLastError('Failed staging kernel image');
    }
  }

  public stageInitrd(initrdImage: Uint8Array): void {
    const h = this.ensureActive();
    this.runtime.stageInput(initrdImage);
    if (this.raw.wasmnux_vm_stage_initrd(h, initrdImage.byteLength) !== 0) {
      throw this.runtime.readLastError('Failed staging initrd image');
    }
  }

  public boot(bootargs: string): void {
    const h = this.ensureActive();
    const argBytes = utf8Encoder.encode(bootargs);
    this.runtime.stageInput(argBytes);
    if (this.raw.wasmnux_vm_boot(h, argBytes.byteLength) !== 0) {
      throw this.runtime.readLastError('Failed booting guest Linux image');
    }
  }

  public step(maxSteps: number): VmStepResult {
    const h = this.ensureActive();
    const clamped = Math.max(1, Math.min(0x0fff_ffff, Math.floor(maxSteps)));
    const packed = this.raw.wasmnux_vm_step(h, clamped) >>> 0;
    const code = (packed >>> 28) & 0x0f;
    const stepsExecuted = packed & 0x0fff_ffff;
    switch (code) {
      case 0:
        return { outcome: 'executed', stepsExecuted };
      case 1:
        return { outcome: 'wfi_idle', stepsExecuted };
      case 2:
        return { outcome: 'poweroff', stepsExecuted };
      case 3:
        return { outcome: 'reboot', stepsExecuted };
      default:
        throw this.runtime.readLastError('VM step failed');
    }
  }

  public pause(): void {
    const h = this.ensureActive();
    if (this.raw.wasmnux_vm_pause(h) !== 0) {
      throw this.runtime.readLastError('Failed pausing VM');
    }
  }

  public resume(): void {
    const h = this.ensureActive();
    if (this.raw.wasmnux_vm_resume(h) !== 0) {
      throw this.runtime.readLastError('Failed resuming VM');
    }
  }

  public hardReset(): void {
    const h = this.ensureActive();
    if (this.raw.wasmnux_vm_hard_reset(h) !== 0) {
      throw this.runtime.readLastError('Failed resetting VM');
    }
  }

  public getState(): VmLifecycleStateName {
    const h = this.ensureActive();
    const code = this.raw.wasmnux_vm_state(h) >>> 0;
    switch (code) {
      case 0:
        return 'created';
      case 1:
        return 'running';
      case 2:
        return 'paused';
      case 3:
        return 'powered_off';
      case 4:
        return 'reboot_requested';
      default:
        throw new VmRuntimeError('invalid_handle', 1, 'Invalid VM lifecycle state');
    }
  }

  public pushSerialInput(input: Uint8Array | string): void {
    const h = this.ensureActive();
    const bytes = typeof input === 'string' ? utf8Encoder.encode(input) : input;
    if (bytes.byteLength === 0) {
      return;
    }
    this.runtime.stageInput(bytes);
    if (this.raw.wasmnux_uart_push_input(h, bytes.byteLength) !== 0) {
      throw this.runtime.readLastError('Failed pushing serial input');
    }
  }

  public drainSerialOutput(): Uint8Array {
    const h = this.ensureActive();
    const len = this.raw.wasmnux_uart_drain_output(h) >>> 0;
    return this.runtime.readOutput(len);
  }

  public getFramebufferDimensions(): { readonly width: number; readonly height: number } {
    const h = this.ensureActive();
    return {
      width: this.raw.wasmnux_fb_width(h) >>> 0,
      height: this.raw.wasmnux_fb_height(h) >>> 0,
    };
  }

  public resizeFramebuffer(width: number, height: number): void {
    const h = this.ensureActive();
    if (this.raw.wasmnux_fb_resize(h, width >>> 0, height >>> 0) !== 0) {
      throw this.runtime.readLastError('Failed resizing guest framebuffer');
    }
  }

  public takeDirtyFramebuffer(): FramebufferDirtyUpdate | null {
    const h = this.ensureActive();
    const packed = BigInt.asUintN(64, this.raw.wasmnux_fb_take_dirty(h));
    if (packed === 0xffff_ffff_ffff_ffffn) {
      return null;
    }
    const yMin = Number((packed >> 32n) & 0xffff_ffffn);
    const yMax = Number(packed & 0xffff_ffffn);
    const width = this.raw.wasmnux_fb_width(h) >>> 0;
    const height = this.raw.wasmnux_fb_height(h) >>> 0;
    if (yMin >= yMax || width === 0 || height === 0) {
      return null;
    }
    const ptr = this.raw.wasmnux_fb_ptr(h) >>> 0;
    const totalLen = this.raw.wasmnux_fb_byte_len(h) >>> 0;
    const stride = width * 4;
    const byteStart = yMin * stride;
    const byteLen = (yMax - yMin) * stride;
    if (
      ptr === 0 ||
      byteStart + byteLen > totalLen ||
      ptr + byteStart + byteLen > this.raw.memory.buffer.byteLength
    ) {
      throw new VmRuntimeError('out_of_bounds', 2, 'Framebuffer VRAM slice out of bounds');
    }
    const pixels = new Uint8Array(this.raw.memory.buffer, ptr + byteStart, byteLen).slice();
    return { width, height, yMin, yMax, pixels };
  }

  public loadBaseDiskChunk(chunkIndex: number, data: Uint8Array): void {
    const h = this.ensureActive();
    if (data.byteLength !== DISK_CHUNK_BYTES) {
      throw new VmRuntimeError(
        'invalid_argument',
        3,
        `Base disk chunk must be exactly ${String(DISK_CHUNK_BYTES)} bytes`,
      );
    }
    this.runtime.stageInput(data);
    if (this.raw.wasmnux_disk_load_base_chunk(h, chunkIndex >>> 0) !== 0) {
      throw this.runtime.readLastError('Failed loading base disk chunk');
    }
  }

  public hydrateOverlayDiskChunk(
    chunkIndex: number,
    data: Uint8Array,
    expectedCrc32?: number,
  ): void {
    const h = this.ensureActive();
    if (data.byteLength !== DISK_CHUNK_BYTES) {
      throw new VmRuntimeError(
        'invalid_argument',
        3,
        `Overlay disk chunk must be exactly ${String(DISK_CHUNK_BYTES)} bytes`,
      );
    }
    this.runtime.stageInput(data);
    const hasCrc = expectedCrc32 !== undefined ? 1 : 0;
    const crc = (expectedCrc32 ?? 0) >>> 0;
    if (this.raw.wasmnux_disk_hydrate_overlay_chunk(h, chunkIndex >>> 0, hasCrc, crc) !== 0) {
      throw this.runtime.readLastError('Failed hydrating overlay disk chunk');
    }
  }

  public setDiskQuotaChunks(maxChunks: number | null): void {
    const h = this.ensureActive();
    const arg = maxChunks === null ? 0xffff_ffff : Math.max(0, Math.floor(maxChunks)) >>> 0;
    if (this.raw.wasmnux_disk_set_quota_chunks(h, arg) !== 0) {
      throw this.runtime.readLastError('Failed setting disk quota');
    }
  }

  public setDiskReadOnly(readOnly: boolean): void {
    const h = this.ensureActive();
    if (this.raw.wasmnux_disk_set_read_only(h, readOnly ? 1 : 0) !== 0) {
      throw this.runtime.readLastError('Failed setting disk read-only state');
    }
  }

  public resetDiskOverlay(): void {
    const h = this.ensureActive();
    if (this.raw.wasmnux_disk_reset_overlay(h) !== 0) {
      throw this.runtime.readLastError('Failed resetting disk overlay');
    }
  }

  public drainDirtyDiskChunks(): DirtyDiskChunk[] {
    const h = this.ensureActive();
    const out: DirtyDiskChunk[] = [];
    for (;;) {
      const idx = this.raw.wasmnux_disk_pop_dirty_chunk(h);
      if (idx < 0) {
        break;
      }
      const crc32 = this.raw.wasmnux_disk_last_dirty_crc32(h) >>> 0;
      const data = this.runtime.readOutput(DISK_CHUNK_BYTES);
      out.push({ chunkIndex: idx, crc32, data });
    }
    return out;
  }

  public requeueDirtyDiskChunk(chunkIndex: number): void {
    const h = this.ensureActive();
    if (this.raw.wasmnux_disk_requeue_dirty_chunk(h, chunkIndex >>> 0) !== 0) {
      throw this.runtime.readLastError('Failed requeuing dirty disk chunk');
    }
  }

  public exportDiskBundle(): Uint8Array {
    const h = this.ensureActive();
    const len = this.raw.wasmnux_disk_export_bundle(h) >>> 0;
    if (len === 0) {
      throw this.runtime.readLastError('Failed exporting disk bundle');
    }
    return this.runtime.readOutput(len);
  }

  public importDiskBundle(bundle: Uint8Array): void {
    const h = this.ensureActive();
    this.runtime.stageInput(bundle);
    if (this.raw.wasmnux_disk_import_bundle(h, bundle.byteLength) !== 0) {
      throw this.runtime.readLastError('Failed importing disk bundle');
    }
  }

  public saveFullSnapshot(): Uint8Array {
    const h = this.ensureActive();
    const len = this.raw.wasmnux_vm_save_full_snapshot(h) >>> 0;
    if (len === 0) {
      throw this.runtime.readLastError('Failed saving full VM snapshot');
    }
    return this.runtime.readOutput(len);
  }

  public restoreFullSnapshot(snapshot: Uint8Array): void {
    const h = this.ensureActive();
    this.runtime.stageInput(snapshot);
    if (this.raw.wasmnux_vm_restore_full_snapshot(h, snapshot.byteLength) !== 0) {
      throw this.runtime.readLastError('Failed restoring full VM snapshot');
    }
  }

  public setNetworkEnabled(enabled: boolean): void {
    const h = this.ensureActive();
    if (this.raw.wasmnux_net_set_enabled(h, enabled ? 1 : 0) !== 0) {
      throw this.runtime.readLastError('Failed toggling virtual network state');
    }
  }

  public drainHttpRequests(): BridgeHttpRequestMsg[] {
    const h = this.ensureActive();
    const out: BridgeHttpRequestMsg[] = [];
    for (;;) {
      const len = this.raw.wasmnux_net_pop_http_request(h) >>> 0;
      if (len === 0) {
        break;
      }
      const raw = this.runtime.readOutput(len);
      out.push(decodeBridgeHttpRequest(raw));
    }
    return out;
  }

  public completeHttpResponse(resp: BridgeHttpResponseMsg): void {
    const h = this.ensureActive();
    const encoded = encodeBridgeHttpResponse(resp);
    this.runtime.stageInput(encoded);
    if (this.raw.wasmnux_net_complete_http_response(h, encoded.byteLength) !== 0) {
      throw this.runtime.readLastError('Failed completing bridged HTTP response');
    }
  }

  public getTelemetry(): VmTelemetry {
    const h = this.ensureActive();
    const len = this.raw.wasmnux_vm_telemetry(h) >>> 0;
    if (len !== 64) {
      throw this.runtime.readLastError('Failed reading VM telemetry');
    }
    const buf = this.runtime.readOutput(64);
    const dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
    return {
      totalSteps: dv.getBigUint64(0, true),
      mtimeTicks: dv.getBigUint64(8, true),
      guestRamBytes: Number(dv.getBigUint64(16, true)),
      wasmLinearMemoryBytes: this.runtime.linearMemoryBytes,
      diskCapacityBytes: Number(dv.getBigUint64(24, true)),
      diskOverlayBytes: Number(dv.getBigUint64(32, true)),
      diskBytesRead: Number(dv.getBigUint64(40, true)),
      diskBytesWritten: Number(dv.getBigUint64(48, true)),
      netRxBytes: Number(dv.getBigUint64(56, true)),
    };
  }
}

export function decodeBridgeHttpRequest(raw: Uint8Array): BridgeHttpRequestMsg {
  const dv = new DataView(raw.buffer, raw.byteOffset, raw.byteLength);
  let cur = 0;
  const readU32 = (): number => {
    if (cur + 4 > raw.byteLength) {
      throw new VmRuntimeError('out_of_bounds', 2, 'Truncated HTTP request envelope');
    }
    const v = dv.getUint32(cur, true);
    cur += 4;
    return v;
  };
  const readStr = (): string => {
    const len = readU32();
    if (cur + len > raw.byteLength) {
      throw new VmRuntimeError('out_of_bounds', 2, 'Truncated HTTP request string');
    }
    const s = utf8Decoder.decode(raw.subarray(cur, cur + len));
    cur += len;
    return s;
  };

  if (raw.byteLength < 12) {
    throw new VmRuntimeError('out_of_bounds', 2, 'HTTP request envelope shorter than header');
  }
  const requestId = dv.getBigUint64(0, true);
  cur = 8;
  const method = readStr();
  const host = readStr();
  const path = readStr();
  const hdrCount = readU32();
  const headers: Array<readonly [string, string]> = [];
  for (let i = 0; i < hdrCount; i += 1) {
    const k = readStr();
    const v = readStr();
    headers.push([k, v]);
  }
  const bodyLen = readU32();
  if (cur + bodyLen > raw.byteLength) {
    throw new VmRuntimeError('out_of_bounds', 2, 'Truncated HTTP request body');
  }
  const body = raw.slice(cur, cur + bodyLen);
  return { requestId, method, host, path, headers, body };
}

export function encodeBridgeHttpResponse(resp: BridgeHttpResponseMsg): Uint8Array {
  const statusTextBytes = utf8Encoder.encode(resp.statusText);
  const encodedHeaders = resp.headers.map(
    ([k, v]) => [utf8Encoder.encode(k), utf8Encoder.encode(v)] as const,
  );

  let total = 8 + 2 + 4 + statusTextBytes.byteLength + 4;
  for (const [kb, vb] of encodedHeaders) {
    total += 4 + kb.byteLength + 4 + vb.byteLength;
  }
  total += 4 + resp.body.byteLength;

  const out = new Uint8Array(total);
  const dv = new DataView(out.buffer);
  let cur = 0;
  dv.setBigUint64(cur, resp.requestId, true);
  cur += 8;
  dv.setUint16(cur, resp.statusCode & 0xffff, true);
  cur += 2;

  const writeBytes = (b: Uint8Array): void => {
    dv.setUint32(cur, b.byteLength, true);
    cur += 4;
    out.set(b, cur);
    cur += b.byteLength;
  };

  writeBytes(statusTextBytes);
  dv.setUint32(cur, encodedHeaders.length, true);
  cur += 4;
  for (const [kb, vb] of encodedHeaders) {
    writeBytes(kb);
    writeBytes(vb);
  }
  writeBytes(resp.body);
  return out;
}
