import { existsSync, readFileSync, statSync } from 'node:fs';
import { resolve } from 'node:path';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { executeBridgedHttpRequest } from '../../src/net/network-bridge';
import { extractNonZeroBaseChunks } from '../../src/runtime/profiles';
import { MemoryStorageAdapter, PersistentStorageEngine } from '../../src/storage/storage-engine';
import { WasmVmRuntime, type WasmVmSession } from '../../src/wasm/bindings';

const PUBLIC_ROOT = resolve(process.cwd(), 'public');
const WASM_BYTES = readFileSync(resolve(PUBLIC_ROOT, 'wasm/wasmnux_wasm.wasm'));
const KERNEL_IMAGE = gunzipSync(
  readFileSync(resolve(PUBLIC_ROOT, 'images/vmlinux-7.3.1-riscv64.Image.gz')),
);
const DEBIAN_BASE_DISK = gunzipSync(
  readFileSync(resolve(PUBLIC_ROOT, 'images/debian-rootfs.ext4.gz')),
);
const DEBIAN_BASE_CHUNKS = extractNonZeroBaseChunks(DEBIAN_BASE_DISK);
const DEBIAN_BOOTARGS =
  'console=ttyS0,115200 earlycon=sbi root=/dev/vda rw rootwait init=/usr/sbin/init panic=-1';

const localPublicFetch: typeof fetch = (input) => {
  const rawUrl =
    typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
  const parsed = new URL(rawUrl);
  const rel = parsed.pathname.replace(/^\/+/, '');
  const fullPath = resolve(PUBLIC_ROOT, rel);
  if (!fullPath.startsWith(PUBLIC_ROOT) || !existsSync(fullPath) || !statSync(fullPath).isFile()) {
    return Promise.resolve(
      new Response('<!DOCTYPE html><html><body>SPA fallback</body></html>', {
        status: 200,
        headers: { 'Content-Type': 'text/html; charset=utf-8' },
      }),
    );
  }
  const bytes = readFileSync(fullPath);
  return Promise.resolve(
    new Response(bytes, {
      status: 200,
      headers: {
        'Content-Type': 'application/octet-stream',
        'Content-Length': String(bytes.byteLength),
      },
    }),
  );
};

async function pumpDebianUntil(
  vm: WasmVmSession,
  marker: string,
  networkEnabled = true,
  maxSlices = 1400,
): Promise<{ readonly output: string; readonly matched: boolean }> {
  const decoder = new TextDecoder();
  let output = '';
  for (let i = 0; i < maxSlices; i += 1) {
    const step = vm.step(250_000);
    const httpReqs = vm.drainHttpRequests();
    for (const req of httpReqs) {
      const resp = await executeBridgedHttpRequest(req, {
        networkEnabled,
        origin: 'http://wasmnux.test',
        fetchImpl: localPublicFetch,
      });
      vm.completeHttpResponse(resp);
    }
    const chunk = vm.drainSerialOutput();
    if (chunk.byteLength > 0) {
      output += decoder.decode(chunk, { stream: true });
    }
    if (output.includes(marker)) {
      return { output, matched: true };
    }
    if (step.outcome === 'poweroff' || step.outcome === 'reboot') {
      break;
    }
  }
  return { output, matched: false };
}

describe('Official Debian 13.7 ("trixie") riscv64 + Real APT/DPKG Verification (Phase 6)', () => {
  it('boots Debian 13.7 riscv64, runs bash/dpkg/apt-get update & install with dependency resolution over VirtIO-Net, removes packages, persists dpkg DB across cold restart, and works offline', async () => {
    const storage = new PersistentStorageEngine(new MemoryStorageAdapter());
    const diskCapBytes = 256 * 1024 * 1024;
    const rt = await WasmVmRuntime.fromBytes(WASM_BYTES);

    const vm1 = rt.createVm({
      profileId: 'debian',
      ramMib: 256,
      diskMib: 256,
    });
    vm1.setNetworkEnabled(true);
    for (const c of DEBIAN_BASE_CHUNKS) {
      vm1.loadBaseDiskChunk(c.chunkIndex, c.data);
    }
    vm1.stageKernel(KERNEL_IMAGE);
    vm1.boot(DEBIAN_BOOTARGS);

    const boot1 = await pumpDebianUntil(vm1, 'WASMNUX_DEBIAN_BOOT_COMPLETE');
    expect(boot1.matched).toBe(true);
    expect(boot1.output).toContain('Linux version 7.3.1');
    expect(boot1.output).toContain('WASMNux Debian GNU/Linux 13.7 (trixie)');

    // Wait for GNU Bash login prompt
    await pumpDebianUntil(vm1, '# ', true, 300);

    // Verify SimpleFB crimson banner rendered on /dev/fb0 during Debian boot
    const bootFb = vm1.takeDirtyFramebuffer();
    expect(bootFb).not.toBeNull();
    if (bootFb !== null) {
      expect(bootFb.width).toBe(640);
      expect(bootFb.yMax).toBe(400);
    }

    const aptScript = [
      '(',
      'set -ex',
      '. /etc/os-release',
      'test "$ID" = "debian"',
      'test "$VERSION_CODENAME" = "trixie"',
      'test "$(cat /etc/debian_version)" = "13.7"',
      'bash --version | grep "GNU bash, version 5.2" >/dev/null',
      'dpkg --version | grep "dpkg" >/dev/null',
      'apt --version | grep "apt 3.0" >/dev/null',
      'busybox --list | grep "udhcpc" >/dev/null',
      'su - root -c "printf LOGIN_%s\\\\n OK" | grep "LOGIN_OK" >/dev/null',
      'apt-get update',
      'apt-cache policy wasmnux-sysbench | grep "1.0.0" >/dev/null',
      'DEBIAN_FRONTEND=noninteractive apt-get install -y wasmnux-sysbench wasmnux-notes',
      'wasmnux-hello from-apt | grep "WASMNUX_HELLO_OK kernel=Linux-7.3.1 arch=riscv64 arg\\[1\\]=from-apt" >/dev/null',
      'wasmnux-sysbench | grep "WASMNUX_SYSBENCH_OK primes_upto_50k=5133" >/dev/null',
      'wasmnux-info | grep "WASMNUX_NOTES_OK" >/dev/null',
      'dpkg -s wasmnux-hello | grep "Status: install ok installed" >/dev/null',
      'dpkg -s wasmnux-sysbench | grep "Status: install ok installed" >/dev/null',
      'DEBIAN_FRONTEND=noninteractive apt-get remove -y wasmnux-sysbench',
      'if command -v wasmnux-sysbench >/dev/null 2>&1; then exit 1; fi',
      'dpkg -s wasmnux-hello | grep "Status: install ok installed" >/dev/null',
      'echo "debian-persisted-state-13.7" > /root/debian-persist.txt',
      'sync',
      'printf "DEBIAN_APT_SUITE_%s\\n" "VERIFIED"',
      ')',
    ].join('\n');

    vm1.pushSerialInput(`${aptScript}\n`);
    const aptRun = await pumpDebianUntil(vm1, 'DEBIAN_APT_SUITE_VERIFIED', true, 8000);
    if (!aptRun.matched) {
      throw new Error(
        `Debian APT verification did not reach completion marker. Tail:\n${aptRun.output.slice(-2500)}`,
      );
    }

    // Clean guest poweroff (`mount -o remount,ro /` flushes ext4 journal cleanly)
    vm1.pushSerialInput('poweroff\n');
    let poweredOff = false;
    let poweroffLog = '';
    const dec = new TextDecoder();
    for (let i = 0; i < 800; i += 1) {
      const res = vm1.step(250_000);
      const s = vm1.drainSerialOutput();
      if (s.byteLength > 0) {
        poweroffLog += dec.decode(s, { stream: true });
      }
      if (res.outcome === 'poweroff') {
        poweredOff = true;
        break;
      }
    }
    if (!poweredOff) {
      throw new Error(`poweroff did not complete. Serial log:\n${poweroffLog}`);
    }

    const dirtyChunks = vm1.drainDirtyDiskChunks();
    expect(dirtyChunks.length).toBeGreaterThan(0);
    await storage.commitDirtyChunks('debian', diskCapBytes, dirtyChunks);
    vm1.destroy();

    // Verify profile isolation: 'vanilla' storage remains untouched (0 chunks)
    const vanillaRecovered = await storage.openAndRecoverProfile('vanilla', 64 * 1024 * 1024);
    expect(vanillaRecovered.overlayChunks.length).toBe(0);

    // Cold-boot a brand-new Debian VM with `networkEnabled: false` (100% offline!)
    // and verify persisted dpkg database, installed `wasmnux-hello` & `wasmnux-notes`,
    // removed `wasmnux-sysbench`, `/root/debian-persist.txt`, AND offline `dpkg -i`!
    const recovered = await storage.openAndRecoverProfile('debian', diskCapBytes);
    expect(recovered.overlayChunks.length).toBeGreaterThan(0);

    const vm2 = rt.createVm({
      profileId: 'debian',
      ramMib: 256,
      diskMib: 256,
    });
    vm2.setNetworkEnabled(false);
    for (const c of DEBIAN_BASE_CHUNKS) {
      vm2.loadBaseDiskChunk(c.chunkIndex, c.data);
    }
    for (const c of recovered.overlayChunks) {
      vm2.hydrateOverlayDiskChunk(c.chunkIndex, c.data, c.crc32);
    }
    vm2.stageKernel(KERNEL_IMAGE);
    vm2.boot(DEBIAN_BOOTARGS);

    const boot2 = await pumpDebianUntil(vm2, 'WASMNUX_DEBIAN_BOOT_COMPLETE', false);
    expect(boot2.matched).toBe(true);
    await pumpDebianUntil(vm2, '# ', false, 300);

    const offlineVerify = [
      '(',
      'set -ex',
      'test "$(cat /root/debian-persist.txt)" = "debian-persisted-state-13.7"',
      'dpkg -s wasmnux-hello | grep "Status: install ok installed" >/dev/null',
      'dpkg -s wasmnux-notes | grep "Status: install ok installed" >/dev/null',
      'if command -v wasmnux-sysbench >/dev/null 2>&1; then exit 1; fi',
      'wasmnux-hello offline-boot | grep "WASMNUX_HELLO_OK" >/dev/null',
      'dpkg -i /var/cache/wasmnux-offline/wasmnux-hello_1.0.0_riscv64.deb',
      'printf "DEBIAN_OFFLINE_PERSIST_%s\\n" "OK"',
      ')',
    ].join('\n');

    vm2.pushSerialInput(`${offlineVerify}\n`);
    const res2 = await pumpDebianUntil(vm2, 'DEBIAN_OFFLINE_PERSIST_OK', false, 4000);
    if (!res2.matched) {
      throw new Error(`Offline persistence check failed. Tail:\n${res2.output.slice(-2000)}`);
    }
    vm2.destroy();
  }, 180_000);
});
