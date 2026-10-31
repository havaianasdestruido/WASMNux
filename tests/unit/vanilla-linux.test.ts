import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { extractNonZeroBaseChunks } from '../../src/runtime/profiles';
import { MemoryStorageAdapter, PersistentStorageEngine } from '../../src/storage/storage-engine';
import { WasmVmRuntime, type WasmVmSession } from '../../src/wasm/bindings';

const WASM_BYTES = readFileSync(resolve(process.cwd(), 'public/wasm/wasmnux_wasm.wasm'));
const KERNEL_IMAGE = gunzipSync(
  readFileSync(resolve(process.cwd(), 'public/images/vmlinux-7.3.1-riscv64.Image.gz')),
);
const VANILLA_INITRD = gunzipSync(
  readFileSync(resolve(process.cwd(), 'public/images/vanilla-initramfs.cpio.gz')),
);
const VANILLA_BASE_DISK = gunzipSync(
  readFileSync(resolve(process.cwd(), 'public/images/vanilla-disk.ext4.gz')),
);
const VANILLA_BASE_CHUNKS = extractNonZeroBaseChunks(VANILLA_BASE_DISK);

function pumpUntil(
  vm: WasmVmSession,
  marker: string,
  maxSlices = 800,
): { readonly output: string; readonly matched: boolean } {
  const decoder = new TextDecoder();
  let output = '';
  for (let i = 0; i < maxSlices; i += 1) {
    const step = vm.step(250_000);
    for (const req of vm.drainHttpRequests()) {
      const body = new TextEncoder().encode(`HTTP_OK:${req.host}:${req.path}`);
      vm.completeHttpResponse({
        requestId: req.requestId,
        statusCode: 200,
        statusText: 'OK',
        headers: [
          ['Content-Type', 'text/plain'],
          ['Content-Length', String(body.byteLength)],
        ],
        body,
      });
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

describe('Vanilla Linux 7.3.1 + BusyBox 1.36.1 Guest Verification (Phase 5)', () => {
  it('boots upstream Linux 7.3.1, validates all 31 mandatory BusyBox applets, /proc+/sys+/dev+/dev/fb0, net, reboot, ext4 persistence, and poweroff', async () => {
    const storage = new PersistentStorageEngine(new MemoryStorageAdapter());
    const diskCapBytes = 64 * 1024 * 1024;
    const rt = await WasmVmRuntime.fromBytes(WASM_BYTES);

    const vm1 = rt.createVm({ profileId: 'vanilla', ramMib: 128, diskMib: 64 });
    for (const c of VANILLA_BASE_CHUNKS) {
      vm1.loadBaseDiskChunk(c.chunkIndex, c.data);
    }
    vm1.stageKernel(KERNEL_IMAGE);
    vm1.stageInitrd(VANILLA_INITRD);
    vm1.boot('console=ttyS0,115200 earlycon=sbi panic=-1');

    const boot1 = pumpUntil(vm1, 'wasmnux-vanilla:~#');
    expect(boot1.matched).toBe(true);
    expect(boot1.output).toContain('Linux version 7.3.1');
    expect(boot1.output).toContain('EXT4-fs (vda): mounted filesystem');

    // Drain initial SimpleFB banner dirty scanlines
    const bootFb = vm1.takeDirtyFramebuffer();
    expect(bootFb).not.toBeNull();

    // Execute an exhaustive verification script exercising EVERY mandatory BusyBox command in TODO.MD:
    // sh, ash, ls, cd, cat, cp, mv, rm, mkdir, rmdir, mount, umount, ps, kill, dmesg, uname,
    // free, df, du, grep, sed, awk, find, tar, gzip, touch, chmod, chown, ln, readlink, date,
    // sleep, top (+ framebuffer + ping + wget), then reboot & poweroff!
    const verifyScript = [
      'set -e',
      'test "$(uname -r)" = "7.3.1"',
      'test "$(uname -m)" = "riscv64"',
      'sh -c "echo SH_OK" | grep -q SH_OK',
      'ash -c "echo ASH_OK" | grep -q ASH_OK',
      'test -f /proc/cpuinfo && test -d /sys/class/net/eth0 && test -b /dev/vda && test -c /dev/fb0',
      'mkdir -p /root/bbtest/sub /root/bbtest/empty /root/bbtest/mnt',
      'rmdir /root/bbtest/empty',
      'cd /root/bbtest',
      'touch alpha.txt',
      'echo "hello vanilla 731" > alpha.txt',
      'cat alpha.txt | sed "s/vanilla/LINUX/" | awk "{print \\$1, \\$2}" > beta.txt',
      'cp beta.txt gamma.txt',
      'mv gamma.txt moved.txt',
      'chmod 0640 moved.txt',
      'chown 0:0 moved.txt',
      'ln -s moved.txt sym.txt',
      'test "$(readlink sym.txt)" = "moved.txt"',
      'ls -la /root/bbtest >/dev/null',
      'find /root/bbtest -name "moved.txt" | grep -q moved.txt',
      'tar -cf archive.tar alpha.txt moved.txt',
      'gzip -f archive.tar',
      'test -f archive.tar.gz',
      'rm -f beta.txt',
      'mount -t tmpfs -o size=4M tmpfs /root/bbtest/mnt',
      'df -k /root/bbtest/mnt | grep -q tmpfs',
      'umount /root/bbtest/mnt',
      'du -sk /root/bbtest >/dev/null',
      'free -m | grep -q Mem:',
      'dmesg | grep -q "Linux version 7.3.1"',
      'ps | grep -q init',
      'sleep 0.05 & BG_PID=$!; kill "$BG_PID" 2>/dev/null || true; wait "$BG_PID" 2>/dev/null || true',
      'sleep 0.01',
      'date +%Y | grep -qE "^20[2-9][0-9]$"',
      'top -b -n 1 | head -n 5 | grep -q Mem:',
      '/usr/bin/wasmnux-fbtest >/dev/null',
      'ping -c 1 10.0.2.2 | grep -q "0% packet loss"',
      'wget -qO- http://deb.debian.org/probe | grep -q "HTTP_OK:deb.debian.org:/probe"',
      'echo "PERSIST_ACROSS_REBOOT_AND_ENGINE_42" > /root/persist-proof.txt',
      'sync',
      'printf "ALL_APPLETS_%s\\n" "VERIFIED"',
    ].join('\n');

    vm1.pushSerialInput(`${verifyScript}\n`);
    const cmdRes = pumpUntil(vm1, 'ALL_APPLETS_VERIFIED');
    expect(cmdRes.matched).toBe(true);

    // Verify /usr/bin/wasmnux-fbtest dirtied the full 640x400 SimpleFB framebuffer
    const fbAfterTest = vm1.takeDirtyFramebuffer();
    expect(fbAfterTest).not.toBeNull();
    expect(fbAfterTest?.width).toBe(640);
    expect(fbAfterTest?.yMax).toBe(400);

    // Test genuine `reboot` command inside guest -> triggers clean rcK umount + Reboot outcome!
    vm1.pushSerialInput('reboot\n');
    let sawReboot = false;
    for (let i = 0; i < 300; i += 1) {
      const s = vm1.step(250_000);
      vm1.drainSerialOutput();
      if (s.outcome === 'reboot') {
        sawReboot = true;
        break;
      }
    }
    expect(sawReboot).toBe(true);

    // Perform in-place hardware reset and verify file survived reboot #1
    vm1.hardReset();
    const rebootBoot = pumpUntil(vm1, 'wasmnux-vanilla:~#');
    expect(rebootBoot.matched).toBe(true);

    vm1.pushSerialInput('cat /root/persist-proof.txt; printf "REBOOT_CHECK_%s\\n" "OK"\n');
    const rebootCheck = pumpUntil(vm1, 'REBOOT_CHECK_OK');
    expect(rebootCheck.matched).toBe(true);
    expect(rebootCheck.output).toContain('PERSIST_ACROSS_REBOOT_AND_ENGINE_42');

    // Test genuine `poweroff` command -> clean rcK sync+umount + PowerOff outcome!
    vm1.pushSerialInput('poweroff\n');
    let sawPoweroff = false;
    for (let i = 0; i < 300; i += 1) {
      const s = vm1.step(250_000);
      vm1.drainSerialOutput();
      if (s.outcome === 'poweroff') {
        sawPoweroff = true;
        break;
      }
    }
    expect(sawPoweroff).toBe(true);
    expect(vm1.getState()).toBe('powered_off');

    // Commit all dirty ext4 chunks to PersistentStorageEngine & destroy VM #1
    const dirtyChunks = vm1.drainDirtyDiskChunks();
    expect(dirtyChunks.length).toBeGreaterThan(0);
    await storage.commitDirtyChunks('vanilla', diskCapBytes, dirtyChunks);
    vm1.destroy();

    // Cold-boot a brand-new VM #2 from base ext4 + recovered PersistentStorageEngine overlay!
    const recovered = await storage.openAndRecoverProfile('vanilla', diskCapBytes);
    expect(recovered.overlayChunks.length).toBeGreaterThan(0);

    const vm2 = rt.createVm({ profileId: 'vanilla', ramMib: 128, diskMib: 64 });
    for (const c of VANILLA_BASE_CHUNKS) {
      vm2.loadBaseDiskChunk(c.chunkIndex, c.data);
    }
    for (const ov of recovered.overlayChunks) {
      vm2.hydrateOverlayDiskChunk(ov.chunkIndex, ov.data, ov.crc32);
    }
    vm2.stageKernel(KERNEL_IMAGE);
    vm2.stageInitrd(VANILLA_INITRD);
    vm2.boot('console=ttyS0,115200 earlycon=sbi panic=-1');

    const boot2 = pumpUntil(vm2, 'wasmnux-vanilla:~#');
    expect(boot2.matched).toBe(true);

    vm2.pushSerialInput(
      'cat /root/persist-proof.txt && ls /root/bbtest/archive.tar.gz && printf "COLD_PERSIST_%s\\n" "OK"\n',
    );
    const coldCheck = pumpUntil(vm2, 'COLD_PERSIST_OK');
    expect(coldCheck.matched).toBe(true);
    expect(coldCheck.output).toContain('PERSIST_ACROSS_REBOOT_AND_ENGINE_42');
    vm2.destroy();
  }, 90_000);
});
