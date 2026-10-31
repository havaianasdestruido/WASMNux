export type Profile = 'vanilla' | 'debian';
export type WorkerRequest =
  | { type: 'boot'; profile: Profile }
  | { type: 'input'; bytes: Uint8Array }
  | { type: 'pause'; paused: boolean }
  | { type: 'shutdown' }
  | { type: 'export' }
  | { type: 'import'; bytes: Uint8Array }
  | { type: 'snapshot' }
  | { type: 'restore'; id: string }
  | { type: 'deleteSnapshot'; id: string }
  | { type: 'listSnapshots' };
export type WorkerResponse =
  | { type: 'status'; text: string }
  | { type: 'ready' }
  | { type: 'output'; bytes: Uint8Array }
  | { type: 'stats'; retired: number; wallMs: number; poweredOff: boolean }
  | { type: 'exported'; bytes: Uint8Array }
  | { type: 'frame'; bytes: Uint8Array }
  | { type: 'error'; message: string }
  | { type: 'snapshots'; snapshots: Array<{ id: string; profile: string; created: number; bytes: number }> };
export function isRequest(v: unknown): v is WorkerRequest {
  if (typeof v !== 'object' || v === null || !('type' in v)) return false;
  const r = v as Record<string, unknown>;
  switch (r.type) {
    case 'boot':
      return r.profile === 'vanilla' || r.profile === 'debian';
    case 'input':
      return r.bytes instanceof Uint8Array && r.bytes.length <= 65536;
    case 'pause':
      return typeof r.paused === 'boolean';
    case 'shutdown':
    case 'export':
      return true;
    case 'import':
      return (
        r.bytes instanceof Uint8Array &&
        (r.bytes.length === 64 * 1024 * 1024 || r.bytes.length === 256 * 1024 * 1024)
      );
    case 'snapshot':
    case 'listSnapshots':
      return true;
    case 'restore':
    case 'deleteSnapshot':
      return typeof r.id === 'string' && /^[0-9a-f-]{36}$/.test(r.id);
    default:
      return false;
  }
}
