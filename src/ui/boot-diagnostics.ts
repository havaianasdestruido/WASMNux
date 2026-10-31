/**
 * Real-time Linux kernel boot milestone detector, serial/dmesg ring buffer, and diagnostic report builder.
 */

import type { BrowserCapabilities } from '../runtime/capabilities';
import type { LinuxProfileId } from '../runtime/profiles';
import type { QuotaHealthReport } from '../storage/storage-engine';
import type { VmTelemetry } from '../wasm/bindings';

export type KernelMilestoneId =
  | 'kernel_banner'
  | 'framebuffer_init'
  | 'virtio_block'
  | 'virtio_net'
  | 'rootfs_mounted'
  | 'userspace_ready';

export interface KernelMilestoneRecord {
  readonly id: KernelMilestoneId;
  readonly label: string;
  readonly elapsedMs: number;
  readonly matchedLine: string;
}

interface MilestoneRule {
  readonly id: KernelMilestoneId;
  readonly label: string;
  readonly pattern: RegExp;
}

const MILESTONE_RULES: readonly MilestoneRule[] = [
  { id: 'kernel_banner', label: 'Linux Kernel Banner', pattern: /Linux version \d+\.\d+/ },
  {
    id: 'framebuffer_init',
    label: 'SimpleFB /dev/fb0 Ready',
    pattern: /simple-framebuffer|Console: switching to colour frame buffer/,
  },
  { id: 'virtio_block', label: 'VirtIO Block (/dev/vda)', pattern: /virtio_blk|^\s*vda:/ },
  { id: 'virtio_net', label: 'VirtIO Network (eth0)', pattern: /virtio_net/ },
  {
    id: 'rootfs_mounted',
    label: 'EXT4 Filesystem Mounted',
    pattern: /EXT4-fs \(vda\): mounted filesystem/,
  },
  {
    id: 'userspace_ready',
    label: 'Userspace Init & Shell Ready',
    pattern: /WASMNUX Vanilla Linux|WASMNUX_DEBIAN_BOOT_COMPLETE|wasmnux-vanilla:~#/,
  },
];

const MAX_LOG_LINES = 1500;

// Strip ANSI CSI escape codes so milestone detection and log filtering operate on clean text.
function stripAnsi(raw: string): string {
  // eslint-disable-next-line no-control-regex
  return raw.replace(/\x1b\[[0-9;?]*[a-zA-Z]/g, '').replace(/\r/g, '');
}

export class BootDiagnosticsCollector {
  private bootStartedAtMs = 0;
  private carryLine = '';
  private readonly lines: string[] = [];
  private readonly milestones = new Map<KernelMilestoneId, KernelMilestoneRecord>();
  private readonly onMilestone: ((rec: KernelMilestoneRecord) => void) | undefined;

  public constructor(opts?: { readonly onMilestone?: (rec: KernelMilestoneRecord) => void }) {
    this.onMilestone = opts?.onMilestone;
  }

  public reset(bootStartedAtMs = performance.now()): void {
    this.bootStartedAtMs = bootStartedAtMs;
    this.carryLine = '';
    this.lines.length = 0;
    this.milestones.clear();
  }

  public ingestSerialChunk(text: string, nowMs = performance.now()): void {
    const clean = stripAnsi(text);
    const combined = this.carryLine + clean;
    const parts = combined.split('\n');
    this.carryLine = parts.pop() ?? '';

    for (const rawLine of parts) {
      const trimmed = rawLine.trimEnd();
      if (trimmed.length === 0) {
        continue;
      }
      this.appendLine(trimmed, nowMs);
    }

    // Also inspect partial carryLine for prompt markers without trailing newline (e.g. `wasmnux-vanilla:~# `)
    if (this.carryLine.trim().length > 0) {
      this.checkMilestones(this.carryLine.trim(), nowMs);
    }
  }

  private appendLine(line: string, nowMs: number): void {
    if (this.lines.length >= MAX_LOG_LINES) {
      this.lines.shift();
    }
    this.lines.push(line);
    this.checkMilestones(line, nowMs);
  }

  private checkMilestones(line: string, nowMs: number): void {
    const elapsed = Math.max(0, Math.round(nowMs - this.bootStartedAtMs));
    for (const rule of MILESTONE_RULES) {
      if (!this.milestones.has(rule.id) && rule.pattern.test(line)) {
        const rec: KernelMilestoneRecord = {
          id: rule.id,
          label: rule.label,
          elapsedMs: elapsed,
          matchedLine: line.slice(0, 160),
        };
        this.milestones.set(rule.id, rec);
        this.onMilestone?.(rec);
      }
    }
  }

  public getMilestones(): readonly KernelMilestoneRecord[] {
    return MILESTONE_RULES.flatMap((r) => {
      const found = this.milestones.get(r.id);
      return found ? [found] : [];
    });
  }

  public getFilteredLines(filterQuery = ''): readonly string[] {
    const q = filterQuery.trim().toLowerCase();
    if (q.length === 0) {
      return [...this.lines];
    }
    return this.lines.filter((l) => l.toLowerCase().includes(q));
  }

  public buildDiagnosticReportJson(context: {
    readonly activeProfile: LinuxProfileId;
    readonly vmState: string;
    readonly rendererBackend: string;
    readonly capabilities: BrowserCapabilities;
    readonly quota: QuotaHealthReport | null;
    readonly telemetry: VmTelemetry | null;
    readonly recentErrors: readonly string[];
  }): string {
    const serializableTelemetry =
      context.telemetry === null
        ? null
        : {
            totalSteps: context.telemetry.totalSteps.toString(),
            mtimeTicks: context.telemetry.mtimeTicks.toString(),
            guestRamBytes: context.telemetry.guestRamBytes,
            wasmLinearMemoryBytes: context.telemetry.wasmLinearMemoryBytes,
            diskCapacityBytes: context.telemetry.diskCapacityBytes,
            diskOverlayBytes: context.telemetry.diskOverlayBytes,
            diskBytesRead: context.telemetry.diskBytesRead,
            diskBytesWritten: context.telemetry.diskBytesWritten,
            netRxBytes: context.telemetry.netRxBytes,
          };

    return JSON.stringify(
      {
        generatedAtIso: new Date().toISOString(),
        activeProfile: context.activeProfile,
        vmState: context.vmState,
        rendererBackend: context.rendererBackend,
        milestones: this.getMilestones(),
        capabilities: context.capabilities,
        quota: context.quota,
        telemetry: serializableTelemetry,
        recentErrors: context.recentErrors,
        recentLogTail: this.lines.slice(-200),
      },
      null,
      2,
    );
  }
}
