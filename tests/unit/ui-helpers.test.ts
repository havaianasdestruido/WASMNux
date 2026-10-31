import { describe, expect, it } from 'vitest';
import { BootDiagnosticsCollector } from '../../src/ui/boot-diagnostics';
import { chunkTextForUartPaste, isTerminalPreferences } from '../../src/ui/terminal-controller';

describe('Browser Interface Diagnostics & Terminal Helpers (Phase 8)', () => {
  it('detects Linux 7.3.1 boot milestones, strips ANSI codes, and filters dmesg lines', () => {
    const seen: string[] = [];
    const diag = new BootDiagnosticsCollector({
      onMilestone: (m) => seen.push(m.id),
    });
    diag.reset(1000);

    diag.ingestSerialChunk('\x1b[32m[    0.000000] Linux version 7.3.1 (riscv64)\x1b[0m\r\n', 1120);
    diag.ingestSerialChunk(
      '[    0.080000] simple-framebuffer 50000000.framebuffer: registered\r\n',
      1200,
    );
    diag.ingestSerialChunk(
      '[    0.110000] virtio_blk virtio0: [vda] 131072 512-byte sectors\r\n',
      1250,
    );
    diag.ingestSerialChunk('[    0.140000] virtio_net virtio1 eth0: renamed\r\n', 1300);
    diag.ingestSerialChunk('[    0.220000] EXT4-fs (vda): mounted filesystem\r\n', 1400);
    diag.ingestSerialChunk('wasmnux-vanilla:~# ', 1850);

    expect(seen).toEqual([
      'kernel_banner',
      'framebuffer_init',
      'virtio_block',
      'virtio_net',
      'rootfs_mounted',
      'userspace_ready',
    ]);
    expect(diag.getFilteredLines('EXT4-fs')).toHaveLength(1);
    expect(diag.getMilestones()[0]?.elapsedMs).toBe(120);
  });

  it('chunks multi-line pasted scripts into bounded UTF-8 UART slices and normalizes CRLF', () => {
    const script = 'echo line1\r\necho line2\r\n'.repeat(10);
    const chunks = chunkTextForUartPaste(script, 32);
    expect(chunks.length).toBeGreaterThan(5);
    for (const c of chunks) {
      expect(c.byteLength).toBeLessThanOrEqual(32);
    }
    const reassembled = chunks.map((c) => new TextDecoder().decode(c)).join('');
    expect(reassembled).not.toContain('\r');
    expect(reassembled).toContain('echo line1\necho line2\n');
  });

  it('validates persisted terminal preferences strictly at runtime', () => {
    expect(isTerminalPreferences({ fontSize: 15, theme: 'amber' })).toBe(true);
    expect(isTerminalPreferences({ fontSize: 99, theme: 'amber' })).toBe(false);
    expect(isTerminalPreferences({ fontSize: 14, theme: 'invalid' })).toBe(false);
    expect(isTerminalPreferences(null)).toBe(false);
  });
});
