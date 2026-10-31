/**
 * Shared types for the WASMNux browser runtime.
 */

export type LinuxProfile = "vanilla" | "debian";

export type VmStatus = "stopped" | "booting" | "running" | "paused" | "error";

export type PersistenceStatus =
  "unknown" | "granted" | "denied" | "quota-pressure";

export interface MachineInfo {
  memSize: number;
  pc: number;
  cycles: number;
  mtime: number;
}

export interface VmSnapshot {
  id: string;
  profile: LinuxProfile;
  createdAt: number;
  label: string;
}

export interface StorageInfo {
  used: number;
  quota: number;
  persistence: PersistenceStatus;
  durable: boolean;
}

export interface BootDiagnostics {
  lines: string[];
  bootedAt?: number;
  errors: string[];
}

export interface Capabilities {
  webgpu: boolean;
  opfs: boolean;
  indexedDB: boolean;
  sharedArrayBuffer: boolean;
  persistence: boolean;
}

export interface GuestConfig {
  profile: LinuxProfile;
  memSizeMb: number;
  diskImageId?: string;
}
