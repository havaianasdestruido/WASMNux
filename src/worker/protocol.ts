/**
 * Validated RPC message protocol between main UI thread (`VmClient`) and `vm.worker.ts`.
 */

import type {
  DirtyDiskChunk,
  FramebufferDirtyUpdate,
  VmLifecycleStateName,
  VmTelemetry,
  WasmErrorKind,
} from '../wasm/bindings';

export interface BaseChunkInput {
  readonly chunkIndex: number;
  readonly data: Uint8Array;
  readonly crc32?: number;
}

export type WorkerCommand =
  | {
      readonly type: 'init_vm';
      readonly reqId: number;
      readonly wasmUrl: string;
      /** Absolute URL of the published application root (used for same-origin mirror URLs). */
      readonly appBaseUrl?: string;
      readonly profileId: string;
      readonly ramMib: number;
      readonly diskMib: number;
      readonly baseChunks: readonly BaseChunkInput[];
      readonly overlayChunks: readonly BaseChunkInput[];
      readonly maxOverlayChunks: number | null;
      readonly readOnly: boolean;
      readonly networkEnabled: boolean;
    }
  | {
      readonly type: 'boot_vm';
      readonly reqId: number;
      readonly kernelImage: Uint8Array;
      readonly initrdImage: Uint8Array | null;
      readonly bootargs: string;
      readonly sliceSteps?: number;
    }
  | {
      readonly type: 'pause_vm';
      readonly reqId: number;
    }
  | {
      readonly type: 'resume_vm';
      readonly reqId: number;
    }
  | {
      readonly type: 'hard_reset_vm';
      readonly reqId: number;
    }
  | {
      readonly type: 'destroy_vm';
      readonly reqId: number;
    }
  | {
      readonly type: 'serial_input';
      readonly reqId: number;
      readonly data: Uint8Array;
    }
  | {
      readonly type: 'resize_fb';
      readonly reqId: number;
      readonly width: number;
      readonly height: number;
    }
  | {
      readonly type: 'save_snapshot';
      readonly reqId: number;
    }
  | {
      readonly type: 'restore_snapshot';
      readonly reqId: number;
      readonly snapshot: Uint8Array;
    }
  | {
      readonly type: 'export_disk_bundle';
      readonly reqId: number;
    }
  | {
      readonly type: 'import_disk_bundle';
      readonly reqId: number;
      readonly bundle: Uint8Array;
    }
  | {
      readonly type: 'reset_disk_overlay';
      readonly reqId: number;
    }
  | {
      readonly type: 'set_network_enabled';
      readonly reqId: number;
      readonly enabled: boolean;
    }
  | {
      readonly type: 'query_telemetry';
      readonly reqId: number;
    };

export type WorkerMessage =
  | {
      readonly kind: 'rpc_ok';
      readonly reqId: number;
      readonly state?: VmLifecycleStateName;
      readonly binary?: Uint8Array;
      readonly telemetry?: VmTelemetry;
    }
  | {
      readonly kind: 'rpc_err';
      readonly reqId: number;
      readonly errorKind: WasmErrorKind;
      readonly code: number;
      readonly message: string;
    }
  | {
      readonly kind: 'serial_output';
      readonly data: Uint8Array;
    }
  | {
      readonly kind: 'fb_dirty';
      readonly update: FramebufferDirtyUpdate;
    }
  | {
      readonly kind: 'disk_dirty';
      readonly profileId: string;
      readonly chunks: readonly DirtyDiskChunk[];
    }
  | {
      readonly kind: 'vm_lifecycle';
      readonly state: VmLifecycleStateName;
      readonly reason: string;
    }
  | {
      readonly kind: 'vm_telemetry';
      readonly telemetry: VmTelemetry;
    }
  | {
      readonly kind: 'vm_error';
      readonly errorKind: WasmErrorKind;
      readonly code: number;
      readonly message: string;
    };

function isObjectRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

export function isWorkerCommand(value: unknown): value is WorkerCommand {
  if (!isObjectRecord(value)) {
    return false;
  }
  const t = value['type'];
  const reqId = value['reqId'];
  if (typeof t !== 'string' || typeof reqId !== 'number' || !Number.isInteger(reqId)) {
    return false;
  }
  switch (t) {
    case 'init_vm':
      return (
        typeof value['wasmUrl'] === 'string' &&
        (value['appBaseUrl'] === undefined || typeof value['appBaseUrl'] === 'string') &&
        typeof value['profileId'] === 'string' &&
        typeof value['ramMib'] === 'number' &&
        typeof value['diskMib'] === 'number' &&
        Array.isArray(value['baseChunks']) &&
        Array.isArray(value['overlayChunks'])
      );
    case 'boot_vm':
      return value['kernelImage'] instanceof Uint8Array && typeof value['bootargs'] === 'string';
    case 'pause_vm':
    case 'resume_vm':
    case 'hard_reset_vm':
    case 'destroy_vm':
    case 'save_snapshot':
    case 'export_disk_bundle':
    case 'reset_disk_overlay':
    case 'query_telemetry':
      return true;
    case 'serial_input':
      return value['data'] instanceof Uint8Array;
    case 'resize_fb':
      return typeof value['width'] === 'number' && typeof value['height'] === 'number';
    case 'restore_snapshot':
      return value['snapshot'] instanceof Uint8Array;
    case 'import_disk_bundle':
      return value['bundle'] instanceof Uint8Array;
    case 'set_network_enabled':
      return typeof value['enabled'] === 'boolean';
    default:
      return false;
  }
}

export function isWorkerMessage(value: unknown): value is WorkerMessage {
  if (!isObjectRecord(value)) {
    return false;
  }
  const kind = value['kind'];
  if (typeof kind !== 'string') {
    return false;
  }
  switch (kind) {
    case 'rpc_ok':
      return typeof value['reqId'] === 'number';
    case 'rpc_err':
      return (
        typeof value['reqId'] === 'number' &&
        typeof value['errorKind'] === 'string' &&
        typeof value['code'] === 'number' &&
        typeof value['message'] === 'string'
      );
    case 'serial_output':
      return value['data'] instanceof Uint8Array;
    case 'fb_dirty':
      return isObjectRecord(value['update']);
    case 'disk_dirty':
      return typeof value['profileId'] === 'string' && Array.isArray(value['chunks']);
    case 'vm_lifecycle':
      return typeof value['state'] === 'string' && typeof value['reason'] === 'string';
    case 'vm_telemetry':
      return isObjectRecord(value['telemetry']);
    case 'vm_error':
      return (
        typeof value['errorKind'] === 'string' &&
        typeof value['code'] === 'number' &&
        typeof value['message'] === 'string'
      );
    default:
      return false;
  }
}
