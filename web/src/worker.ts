import { isRequest, type WorkerResponse } from './protocol';
import { Runtime } from './runtime';
import { openDisk } from './opfs';
import { cachedAsset, sha256, DISK_BYTES } from './storage-admin';
import { putSnapshot, listSnapshots, getSnapshot, deleteSnapshot, type SnapshotMeta } from './metadata';
const send = (m: WorkerResponse, transfer: Transferable[] = []): void => {
  self.postMessage(m, { transfer });
};
const encoder = new TextEncoder();
let runtime: Runtime | undefined, storage: Awaited<ReturnType<typeof openDisk>> | undefined;
let currentProfile: 'vanilla' | 'debian' | undefined;
let lastFrame = 0;
let paused = false,
  releaseLock: (() => void) | undefined;
async function asset(name: string, hash: string): Promise<Uint8Array> {
  return cachedAsset(name, hash, new URL('./', self.location.href));
}
async function boot(profile: 'vanilla' | 'debian'): Promise<void> {
  currentProfile = profile;
  const diskBytes = DISK_BYTES[profile];
  if (!navigator.locks) throw new Error('Web Locks required for safe single-owner disk access');
  await new Promise<void>((resolve, reject) => {
    void navigator.locks
      .request(`wasmnux-${profile}`, { ifAvailable: true }, async (lock) => {
        if (!lock) {
          reject(new Error('Profile is already open in another tab'));
          return;
        }
        await new Promise<void>((held) => {
          releaseLock = held;
          resolve();
        });
      })
      .catch(reject);
  });
  try {
    send({ type: 'status', text: 'Loading and verifying guest assets…' });
    const manifest = (await (
      await fetch(new URL('manifest.json', new URL('./', self.location.href)))
    ).json()) as Record<string, string>;
    const [fw, kernel, initrd, compressed] = await Promise.all([
      asset('fw_jump.bin', manifest['fw_jump.bin']!),
      asset('Image', manifest['Image']!),
      asset('initramfs.cpio', manifest['initramfs.cpio']!),
      asset(
        profile === 'debian' ? 'debian.imgz' : 'vanilla.imgz',
        manifest[profile === 'debian' ? 'debian.imgz' : 'vanilla.imgz']!,
      ),
    ]);
    const stream = new Blob([compressed.buffer as ArrayBuffer])
      .stream()
      .pipeThrough(new DecompressionStream('gzip'));
    const initial = new Uint8Array(await new Response(stream).arrayBuffer());
    storage = await openDisk(profile, initial, diskBytes);
    const disk = storage.disk;
    runtime = await Runtime.create(
      {
        read(o, p, n) {
          new Uint8Array(runtime!.exports.memory.buffer, p, n).set(disk.read(o, n));
        },
        write(o, p, n) {
          disk.write(o, new Uint8Array(runtime!.exports.memory.buffer, p, n));
        },
        flush() {
          disk.flush();
        },
      },
      {
        firmware: fw,
        kernel,
        initrd: profile === 'debian' ? new Uint8Array(0) : initrd,
        sectors: diskBytes / 512,
        cmdline:
          profile === 'debian'
            ? 'console=ttyS0 earlycon=sbi root=/dev/vda rw rootwait init=/wnx-init.sh'
            : 'console=ttyS0 earlycon=sbi rdinit=/init',
      },
    );
    send({ type: 'ready' });
    scheduleTick();
  } catch (e) {
    cleanup();
    throw e;
  }
}
function scheduleTick(): void {
  setTimeout(
    () => {
      try {
        if (runtime && !runtime.poweredOff) {
          if (!paused) {
            const start = performance.now();
            runtime.step();
            const bytes = runtime.output();
            if (bytes.length) send({ type: 'output', bytes }, [bytes.buffer]);
            if (performance.now() - lastFrame > 100) {
              lastFrame = performance.now();
              const frame = runtime.framebuffer();
              send({ type: 'frame', bytes: frame }, [frame.buffer]);
            }
            send({
              type: 'stats',
              retired: runtime.retired,
              wallMs: performance.now() - start,
              poweredOff: runtime.poweredOff,
            });
          }
          scheduleTick();
        } else if (runtime?.poweredOff) {
          send({ type: 'stats', retired: runtime.retired, wallMs: 0, poweredOff: true });
          cleanup();
        }
      } catch (e) {
        cleanup();
        send({ type: 'error', message: e instanceof Error ? e.message : String(e) });
      }
    },
    paused ? 50 : 0,
  );
}
function cleanup(): void {
  currentProfile = undefined;
  runtime?.dispose();
  runtime = undefined;
  storage?.close();
  storage = undefined;
  releaseLock?.();
  releaseLock = undefined;
}
let queue: Promise<void> = Promise.resolve();
self.onmessage = (event: MessageEvent<unknown>) => {
  if (!isRequest(event.data)) {
    send({ type: 'error', message: 'Invalid worker request' });
    return;
  }
  const message = event.data;
  queue = queue
    .then(async () => {
      try {
        switch (message.type) {
          case 'boot':
            if (runtime) throw new Error('VM already running');
            await boot(message.profile);
            break;
          case 'input':
            if (!runtime) throw new Error('VM is not running');
            runtime.input(message.bytes);
            break;
          case 'pause':
            paused = message.paused;
            break;
          case 'shutdown':
            if (runtime) runtime.input(encoder.encode('\npoweroff -f\n'));
            await new Promise((r) => setTimeout(r, 1500));
            cleanup();
            send({ type: 'status', text: 'Stopped; disk flushed' });
            break;
          case 'export': {
            if (!storage) throw new Error('VM is not running');
            storage.disk.flush();
            const bytes = storage.disk.read(0, storage.disk.sizeBytes);
            send({ type: 'exported', bytes }, [bytes.buffer]);
            break;
          }
          case 'snapshot': {
            if (!runtime || !storage) throw new Error('VM is not running');
            paused = true;
            storage.disk.flush();
            const diskHash = await sha256(storage.disk.read(0, storage.disk.sizeBytes));
            const state = runtime.snapshot();
            const meta: SnapshotMeta = {
              id: crypto.randomUUID(),
              profile: currentProfile!,
              created: Date.now(),
              diskHash,
              bytes: state.length,
            };
            const root = await navigator.storage.getDirectory();
            const dir = await root.getDirectoryHandle('wasmnux-snapshots', { create: true });
            const fh = await dir.getFileHandle(meta.id, { create: true });
            const w = await fh.createWritable();
            await w.write(state.buffer as ArrayBuffer);
            await w.close();
            await putSnapshot(meta);
            send({ type: 'snapshots', snapshots: await listSnapshots(currentProfile!) });
            paused = false;
            break;
          }
          case 'restore': {
            if (!runtime || !storage) throw new Error('VM is not running');
            const meta = await getSnapshot(message.id);
            if (!meta || meta.profile !== currentProfile)
              throw new Error('Snapshot not found for this profile');
            storage.disk.flush();
            const diskHash = await sha256(storage.disk.read(0, storage.disk.sizeBytes));
            if (diskHash !== meta.diskHash)
              throw new Error('Disk changed since snapshot; restore the matching disk backup first');
            const root = await navigator.storage.getDirectory();
            const dir = await root.getDirectoryHandle('wasmnux-snapshots');
            const state = new Uint8Array(
              await (await (await dir.getFileHandle(meta.id)).getFile()).arrayBuffer(),
            );
            runtime.restore(state);
            send({ type: 'status', text: 'Snapshot restored' });
            break;
          }
          case 'deleteSnapshot':
            await deleteSnapshot(message.id);
            send({ type: 'snapshots', snapshots: await listSnapshots(currentProfile!) });
            break;
          case 'listSnapshots':
            send({ type: 'snapshots', snapshots: await listSnapshots(currentProfile!) });
            break;
          case 'import':
            throw new Error('Stop the VM before importing a disk');
        }
      } catch (e) {
        cleanup();
        send({ type: 'error', message: e instanceof Error ? e.message : String(e) });
      }
    })
    .catch((e: unknown) => {
      send({ type: 'error', message: String(e) });
    });
};
self.onerror = () => {
  cleanup();
};
