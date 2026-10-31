/**
 * SparseDisk: a persistent virtual disk on top of a ChunkStore.
 *
 * Design (crash consistency):
 *  - The disk is split into fixed 64 KiB chunks. Unwritten chunks are holes
 *    that read as zeroes and cost no storage.
 *  - Chunk objects are immutable and content+sequence addressed
 *    (`chunks/<seq>-<index>-<crc32>`). A write never mutates an existing
 *    object; it creates a new one. The manifest maps chunk index -> object
 *    reference + CRC-32.
 *  - `write()` uploads new chunk objects immediately but only records them
 *    in the in-memory (pending) map. `flush()` replaces the single
 *    manifest object. A crash between the two leaves the previous
 *    manifest intact: the disk reverts cleanly to the last flushed state
 *    and the orphan chunk objects are invisible garbage (GC is a TODO,
 *    tracked in TODO.MD Phase 4).
 *  - Every chunk read verifies CRC-32; corruption is a hard error with the
 *    chunk index attached, never silently zero-filled.
 *  - Snapshots are extra manifests pointing at the same immutable chunk
 *    objects (copy-on-write): cheap, and a snapshot can never be hurt by
 *    later writes to the live disk.
 *  - All public operations are serialised on an internal promise chain:
 *    overlapping calls (e.g. a UI read while a reset's destroy/recreate is
 *    in flight) can never tear a manifest or chunk write. This was a real
 *    bug caught by the browser e2e suite (chunk X missing / manifest not
 *    JSON under concurrent reset+read). Cross-tab exclusion would need
 *    Web Locks and is tracked in TODO.MD Phase 4.
 */

import { crc32 } from "../crc32.js";
import type { ChunkStore } from "./chunkStore.js";

export const CHUNK_SIZE = 64 * 1024;
export const SECTOR_SIZE = 512;

export class DiskError extends Error {
  public constructor(message: string) {
    super(message);
    this.name = "DiskError";
  }
}

export class DiskCorruptionError extends DiskError {
  public readonly chunkIndex: number;
  public constructor(chunkIndex: number, detail: string) {
    super(`disk chunk ${chunkIndex} failed integrity check: ${detail}`);
    this.name = "DiskCorruptionError";
    this.chunkIndex = chunkIndex;
  }
}

interface ChunkRef {
  readonly key: string;
  readonly crc: number;
  /** Stored length (the final chunk of a disk may be short). */
  readonly length: number;
}

interface ManifestV1 {
  readonly magic: "WASMNUX-DISK";
  readonly version: 1;
  readonly capacityBytes: number;
  readonly chunkSize: number;
  /** Monotonic flush generation. */
  readonly generation: number;
  /** Map of chunkIndex (as string) -> ChunkRef for allocated chunks. */
  readonly chunks: Readonly<Record<string, ChunkRef>>;
  readonly updatedAt: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseManifest(raw: Uint8Array): ManifestV1 {
  let parsed: unknown;
  try {
    parsed = JSON.parse(new TextDecoder().decode(raw)) as unknown;
  } catch {
    throw new DiskError("disk manifest is not valid JSON");
  }
  if (!isRecord(parsed)) {
    throw new DiskError("disk manifest is not an object");
  }
  if (parsed["magic"] !== "WASMNUX-DISK" || parsed["version"] !== 1) {
    throw new DiskError("disk manifest magic/version mismatch");
  }
  const capacity = parsed["capacityBytes"];
  const chunkSize = parsed["chunkSize"];
  const generation = parsed["generation"];
  const chunks = parsed["chunks"];
  if (
    typeof capacity !== "number" ||
    typeof chunkSize !== "number" ||
    typeof generation !== "number" ||
    !isRecord(chunks)
  ) {
    throw new DiskError("disk manifest fields are malformed");
  }
  const cleanChunks: Record<string, ChunkRef> = {};
  for (const [index, ref] of Object.entries(chunks)) {
    if (
      !isRecord(ref) ||
      typeof ref["key"] !== "string" ||
      typeof ref["crc"] !== "number" ||
      typeof ref["length"] !== "number" ||
      !/^\d+$/.test(index)
    ) {
      throw new DiskError(`disk manifest chunk entry ${index} is malformed`);
    }
    cleanChunks[index] = {
      key: ref["key"],
      crc: ref["crc"],
      length: ref["length"],
    };
  }
  return {
    magic: "WASMNUX-DISK",
    version: 1,
    capacityBytes: capacity,
    chunkSize,
    generation,
    chunks: cleanChunks,
    updatedAt: typeof parsed["updatedAt"] === "string" ? parsed["updatedAt"] : "",
  };
}

export interface DiskUsage {
  readonly capacityBytes: number;
  readonly allocatedChunks: number;
  readonly allocatedBytes: number;
  readonly generation: number;
}

export interface OpenOptions {
  /** Required when the disk does not exist yet. Sector multiple. */
  readonly capacityBytes?: number;
}

export class SparseDisk {
  readonly #store: ChunkStore;
  readonly #prefix: string;
  #capacity: number;
  #generation: number;
  #flushed: Map<number, ChunkRef>;
  #pending: Map<number, ChunkRef>;
  #seq: number;
  /** Serialises every public async operation (see class doc). */
  #tail: Promise<void>;

  private constructor(
    store: ChunkStore,
    prefix: string,
    capacity: number,
    generation: number,
    chunks: Map<number, ChunkRef>,
  ) {
    this.#store = store;
    this.#prefix = prefix;
    this.#capacity = capacity;
    this.#generation = generation;
    this.#flushed = chunks;
    this.#pending = new Map();
    this.#seq = 0;
    this.#tail = Promise.resolve();
  }

  /** Runs `op` after all previously enqueued operations settle. */
  #enqueue<T>(op: () => Promise<T>): Promise<T> {
    const result = this.#tail.then(op, op);
    this.#tail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  #manifestKey(): string {
    return `${this.#prefix}/manifest.json`;
  }

  #snapshotKey(id: string): string {
    if (!/^[a-zA-Z0-9_-]{1,64}$/.test(id)) {
      throw new DiskError(`invalid snapshot id: ${JSON.stringify(id)}`);
    }
    return `${this.#prefix}/snapshots/${id}.json`;
  }

  /** Opens an existing disk or creates a fresh one when capacity is given. */
  static async open(
    store: ChunkStore,
    diskId: string,
    options: OpenOptions = {},
  ): Promise<SparseDisk> {
    const prefix = `disks/${diskId}`;
    const manifestRaw = await store.get(`${prefix}/manifest.json`);
    if (manifestRaw !== undefined) {
      const manifest = parseManifest(manifestRaw);
      if (manifest.chunkSize !== CHUNK_SIZE) {
        throw new DiskError(
          `disk chunk size ${manifest.chunkSize} does not match runtime ${CHUNK_SIZE}`,
        );
      }
      const chunks = new Map<number, ChunkRef>();
      for (const [index, ref] of Object.entries(manifest.chunks)) {
        chunks.set(Number(index), ref);
      }
      return new SparseDisk(store, prefix, manifest.capacityBytes, manifest.generation, chunks);
    }
    const capacity = options.capacityBytes;
    if (capacity === undefined) {
      throw new DiskError(`disk "${diskId}" does not exist and no capacity was given`);
    }
    if (capacity <= 0 || capacity % SECTOR_SIZE !== 0) {
      throw new DiskError(`capacity ${capacity} must be a positive multiple of ${SECTOR_SIZE}`);
    }
    const disk = new SparseDisk(store, prefix, capacity, 0, new Map());
    await disk.#flushNow(); // persist the empty manifest so the disk exists durably
    return disk;
  }

  get capacityBytes(): number {
    return this.#capacity;
  }

  /** True when writes happened since the last flush. */
  get dirty(): boolean {
    return this.#pending.size > 0;
  }

  #refFor(index: number): ChunkRef | undefined {
    return this.#pending.get(index) ?? this.#flushed.get(index);
  }

  async #loadChunk(index: number): Promise<Uint8Array | undefined> {
    const ref = this.#refFor(index);
    if (ref === undefined) {
      return undefined;
    }
    const data = await this.#store.get(ref.key);
    if (data === undefined) {
      throw new DiskCorruptionError(index, `chunk object ${ref.key} is missing`);
    }
    if (data.length !== ref.length) {
      throw new DiskCorruptionError(
        index,
        `length ${data.length} != manifest length ${ref.length}`,
      );
    }
    if (crc32(data) !== ref.crc) {
      throw new DiskCorruptionError(index, "CRC-32 mismatch");
    }
    return data;
  }

  #checkRange(offset: number, length: number): void {
    if (
      !Number.isSafeInteger(offset) ||
      !Number.isSafeInteger(length) ||
      offset < 0 ||
      length < 0 ||
      offset + length > this.#capacity
    ) {
      throw new DiskError(
        `range [${offset}, ${offset + length}) is outside disk capacity ${this.#capacity}`,
      );
    }
  }

  /** Reads `length` bytes at `offset`; holes read as zeroes. CRC-verified. */
  async #readNow(offset: number, length: number): Promise<Uint8Array> {
    this.#checkRange(offset, length);
    const out = new Uint8Array(length);
    let position = 0;
    while (position < length) {
      const absolute = offset + position;
      const index = Math.floor(absolute / CHUNK_SIZE);
      const inChunk = absolute % CHUNK_SIZE;
      const span = Math.min(CHUNK_SIZE - inChunk, length - position);
      const chunk = await this.#loadChunk(index);
      if (chunk !== undefined) {
        out.set(chunk.subarray(inChunk, inChunk + span), position);
      }
      position += span;
    }
    return out;
  }

  /**
   * Writes `data` at `offset`. New chunk objects are uploaded immediately;
   * durability is only promised after `flush()`.
   */
  async #writeNow(offset: number, data: Uint8Array): Promise<void> {
    this.#checkRange(offset, data.length);
    let position = 0;
    while (position < data.length) {
      const absolute = offset + position;
      const index = Math.floor(absolute / CHUNK_SIZE);
      const inChunk = absolute % CHUNK_SIZE;
      const span = Math.min(CHUNK_SIZE - inChunk, data.length - position);
      const chunkLen = Math.min(CHUNK_SIZE, this.#capacity - index * CHUNK_SIZE);
      const existing = await this.#loadChunk(index);
      const next = new Uint8Array(chunkLen);
      if (existing !== undefined) {
        next.set(existing.subarray(0, chunkLen));
      }
      next.set(data.subarray(position, position + span), inChunk);
      // All-zero chunks become holes again: pending map stores a tombstone
      // by removing the ref on flush. We track it with a zero-length marker.
      const crc = crc32(next);
      this.#seq += 1;
      if (next.every((b) => b === 0)) {
        this.#pending.set(index, { key: "", crc: 0, length: 0 });
      } else {
        const key = `${this.#prefix}/chunks/${this.#generation + 1}-${this.#seq}-${index}-${crc.toString(16)}`;
        await this.#store.put(key, next);
        this.#pending.set(index, { key, crc, length: next.length });
      }
      position += span;
    }
  }

  /** Merged live view (flushed + pending) used by snapshot/flush/export. */
  #liveMap(): Map<number, ChunkRef> {
    const merged = new Map(this.#flushed);
    for (const [index, ref] of this.#pending) {
      if (ref.length === 0) {
        merged.delete(index);
      } else {
        merged.set(index, ref);
      }
    }
    return merged;
  }

  async #writeManifest(key: string, generation: number, map: Map<number, ChunkRef>): Promise<void> {
    const chunks: Record<string, ChunkRef> = {};
    for (const [index, ref] of map) {
      chunks[String(index)] = ref;
    }
    const manifest: ManifestV1 = {
      magic: "WASMNUX-DISK",
      version: 1,
      capacityBytes: this.#capacity,
      chunkSize: CHUNK_SIZE,
      generation,
      chunks,
      updatedAt: new Date().toISOString(),
    };
    await this.#store.put(key, new TextEncoder().encode(JSON.stringify(manifest)));
  }

  /**
   * Durable commit point: replaces the manifest with the pending state.
   * After a successful flush, a crash reopens exactly this state.
   */
  async #flushNow(): Promise<void> {
    const merged = this.#liveMap();
    this.#generation += 1;
    await this.#writeManifest(this.#manifestKey(), this.#generation, merged);
    this.#flushed = merged;
    this.#pending = new Map();
  }

  /** Discards unflushed writes (pending chunk objects become orphans). */
  discardPending(): void {
    this.#pending = new Map();
  }

  /**
   * Creates a named copy-on-write snapshot of the live (flushed + pending
   * chunk objects) state. Pending chunk objects are already uploaded, so a
   * snapshot is crash-consistent too; it never mutates the live manifest.
   */
  async #snapshotNow(id: string): Promise<void> {
    const key = this.#snapshotKey(id);
    if ((await this.#store.get(key)) !== undefined) {
      throw new DiskError(`snapshot "${id}" already exists`);
    }
    await this.#writeManifest(key, this.#generation, this.#liveMap());
  }

  /** Snapshot ids currently stored for this disk. */
  async #listSnapshotsNow(): Promise<string[]> {
    const keys = await this.#store.list(`${this.#prefix}/snapshots/`);
    return keys
      .map((k) => k.slice(k.lastIndexOf("/") + 1).replace(/\.json$/, ""))
      .sort();
  }

  /** Restores the live disk to a snapshot (flushes the restored manifest). */
  async #restoreSnapshotNow(id: string): Promise<void> {
    const raw = await this.#store.get(this.#snapshotKey(id));
    if (raw === undefined) {
      throw new DiskError(`snapshot "${id}" does not exist`);
    }
    const manifest = parseManifest(raw);
    if (manifest.capacityBytes !== this.#capacity) {
      throw new DiskError("snapshot capacity does not match this disk");
    }
    const chunks = new Map<number, ChunkRef>();
    for (const [index, ref] of Object.entries(manifest.chunks)) {
      chunks.set(Number(index), ref);
    }
    this.#pending = new Map();
    this.#flushed = chunks;
    await this.#flushNow();
  }

  async #deleteSnapshotNow(id: string): Promise<void> {
    await this.#store.delete(this.#snapshotKey(id));
  }

  usage(): DiskUsage {
    const live = this.#liveMap();
    let allocatedBytes = 0;
    for (const ref of live.values()) {
      allocatedBytes += ref.length;
    }
    return {
      capacityBytes: this.#capacity,
      allocatedChunks: live.size,
      allocatedBytes,
      generation: this.#generation,
    };
  }

  /**
   * Exports the full disk image (holes materialised as zeroes) — the byte
   * stream handed to the WASM VM at boot and to the user for backups.
   */
  async exportImage(): Promise<Uint8Array> {
    return this.#enqueue(() => this.#readNow(0, this.#capacity));
  }

  /**
   * Replaces the whole disk with `image` (import / VM sync path). The
   * image is chunk-diffed: chunks whose CRC already matches the live map
   * are not re-uploaded. Ends with a flush, so an import is durable and a
   * crash cannot expose a half-imported image through the manifest.
   */
  async #importImageNow(image: Uint8Array): Promise<void> {
    if (image.length !== this.#capacity) {
      throw new DiskError(
        `image size ${image.length} does not equal disk capacity ${this.#capacity}`,
      );
    }
    this.#pending = new Map();
    const chunkCount = Math.ceil(this.#capacity / CHUNK_SIZE);
    for (let index = 0; index < chunkCount; index += 1) {
      const start = index * CHUNK_SIZE;
      const slice = image.subarray(start, Math.min(start + CHUNK_SIZE, image.length));
      if (slice.every((b) => b === 0)) {
        this.#pending.set(index, { key: "", crc: 0, length: 0 });
        continue;
      }
      const chunkData = new Uint8Array(slice);
      const crc = crc32(chunkData);
      const existing = this.#refFor(index);
      if (existing !== undefined && existing.crc === crc && existing.length === chunkData.length) {
        this.#pending.set(index, existing);
        continue;
      }
      this.#seq += 1;
      const key = `${this.#prefix}/chunks/${this.#generation + 1}-${this.#seq}-${index}-${crc.toString(16)}`;
      await this.#store.put(key, chunkData);
      this.#pending.set(index, { key, crc, length: chunkData.length });
    }
    await this.#flushNow();
  }

  /** Deletes the disk manifest, snapshots and all chunk objects. */
  async #destroyNow(): Promise<void> {
    const keys = await this.#store.list(`${this.#prefix}/`);
    for (const key of keys) {
      await this.#store.delete(key);
    }
    this.#pending = new Map();
    this.#flushed = new Map();
  }

  // -- Public API: every operation is serialised through #enqueue so that
  // overlapping calls can never interleave chunk/manifest mutations.

  async read(offset: number, length: number): Promise<Uint8Array> {
    return this.#enqueue(() => this.#readNow(offset, length));
  }

  async write(offset: number, data: Uint8Array): Promise<void> {
    return this.#enqueue(() => this.#writeNow(offset, data));
  }

  async flush(): Promise<void> {
    return this.#enqueue(() => this.#flushNow());
  }

  async snapshot(id: string): Promise<void> {
    return this.#enqueue(() => this.#snapshotNow(id));
  }

  async listSnapshots(): Promise<string[]> {
    return this.#enqueue(() => this.#listSnapshotsNow());
  }

  async restoreSnapshot(id: string): Promise<void> {
    return this.#enqueue(() => this.#restoreSnapshotNow(id));
  }

  async deleteSnapshot(id: string): Promise<void> {
    return this.#enqueue(() => this.#deleteSnapshotNow(id));
  }

  async importImage(image: Uint8Array): Promise<void> {
    return this.#enqueue(() => this.#importImageNow(image));
  }

  async destroy(): Promise<void> {
    return this.#enqueue(() => this.#destroyNow());
  }
}
