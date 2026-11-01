/**
 * Complete Browser-Native Linux workstation interface (`AppShell`).
 */

import {
  FramebufferDisplayController,
  type PreferredRendererMode,
} from '../gpu/display-controller';
import type { RendererDisplayFilter } from '../gpu/types';
import { BootOrchestrator } from '../runtime/boot-orchestrator';
import type { BrowserCapabilities } from '../runtime/capabilities';
import {
  compressGzip,
  decompressGzipIfNeeded,
  LINUX_PROFILES,
  type LinuxProfileId,
} from '../runtime/profiles';
import { PersistentStorageEngine, type QuotaHealthReport } from '../storage/storage-engine';
import type { VmLifecycleStateName, VmTelemetry } from '../wasm/bindings';
import { BootDiagnosticsCollector } from './boot-diagnostics';
import {
  DEFAULT_TERMINAL_PREFERENCES,
  isTerminalPreferences,
  TerminalController,
  type TerminalThemeId,
} from './terminal-controller';

export interface AppShellBootstrapData {
  readonly abiVersion: number;
  readonly wasmUrl: string;
  readonly capabilities: BrowserCapabilities;
  readonly workerSmoke: {
    readonly transportMode: string;
    readonly serialBanner: string;
    readonly dirtyScanlineObserved: boolean;
  };
}

const PREF_KEY_TERMINAL = 'terminal_preferences_v1';
const PREF_KEY_ACTIVE_PROFILE = 'selected_profile_v1';

function isLinuxProfileId(val: unknown): val is LinuxProfileId {
  return val === 'vanilla' || val === 'debian';
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return '0 B';
  }
  if (bytes < 1024) {
    return `${String(bytes)} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KiB`;
  }
  if (bytes < 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  }
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
}

function sanitizeSnapshotSlug(raw: string): string {
  const cleaned = raw
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, '-')
    .replace(/^-+|-+$/g, '');
  return cleaned.length > 0 ? cleaned.slice(0, 48) : `snap-${String(Date.now())}`;
}

export class AppShell {
  private readonly root: HTMLElement;
  private readonly bootData: AppShellBootstrapData;
  private storage!: PersistentStorageEngine;
  private orchestrator!: BootOrchestrator;
  private terminal!: TerminalController;
  private display!: FramebufferDisplayController;
  private readonly diagnostics: BootDiagnosticsCollector;

  private selectedProfile: LinuxProfileId = 'vanilla';
  private vmState: VmLifecycleStateName | 'idle' | 'booting' = 'idle';
  private networkEnabled = true;
  private resetConfirmArmed = false;
  private activeTab: 'telemetry' | 'snapshots' | 'diagnostics' = 'telemetry';
  private logFilter = '';
  private lastTelemetry: VmTelemetry | null = null;
  private lastStepsSample: { steps: bigint; timeMs: number } | null = null;
  private measuredMips = 0;
  private quotaReport: QuotaHealthReport | null = null;
  private readonly recentErrors: string[] = [];
  private serialDecoder = new TextDecoder();

  // Bound DOM elements
  private stateBadgeEl!: HTMLElement;
  private storageBadgeEl!: HTMLElement;
  private gpuBadgeEl!: HTMLElement;
  private errorBannerEl!: HTMLElement;
  private errorMessageEl!: HTMLElement;
  private vanillaMetaEl!: HTMLElement;
  private debianMetaEl!: HTMLElement;
  private vanillaBtnEl!: HTMLButtonElement;
  private debianBtnEl!: HTMLButtonElement;
  private ramSelectEl!: HTMLSelectElement;
  private bootBtnEl!: HTMLButtonElement;
  private pauseBtnEl!: HTMLButtonElement;
  private rebootBtnEl!: HTMLButtonElement;
  private shutdownBtnEl!: HTMLButtonElement;
  private hardResetBtnEl!: HTMLButtonElement;
  private resetDiskBtnEl!: HTMLButtonElement;
  private netToggleBtnEl!: HTMLButtonElement;
  private bootStatusLineEl!: HTMLElement;
  private telemetryPanelEl!: HTMLElement;
  private snapshotsPanelEl!: HTMLElement;
  private diagnosticsPanelEl!: HTMLElement;
  private snapshotListEl!: HTMLElement;
  private snapshotInputEl!: HTMLInputElement;
  private milestoneRowEl!: HTMLElement;
  private kernelLogPreEl!: HTMLElement;

  public constructor(root: HTMLElement, bootData: AppShellBootstrapData) {
    this.root = root;
    this.bootData = bootData;
    this.diagnostics = new BootDiagnosticsCollector({
      onMilestone: () => {
        this.renderDiagnosticsView();
      },
    });
  }

  public async mount(): Promise<void> {
    this.storage = await PersistentStorageEngine.createAuto({ requestPersistence: true });
    const savedProfile = await this.storage.getPreference(
      PREF_KEY_ACTIVE_PROFILE,
      isLinuxProfileId,
    );
    if (savedProfile !== null) {
      this.selectedProfile = savedProfile;
    }
    const savedTermPrefs =
      (await this.storage.getPreference(PREF_KEY_TERMINAL, isTerminalPreferences)) ??
      DEFAULT_TERMINAL_PREFERENCES;

    this.orchestrator = new BootOrchestrator({
      storage: this.storage,
      wasmUrl: this.bootData.wasmUrl,
      events: {
        onBootProgress: (ev) => {
          this.bootStatusLineEl.textContent = `[${String(Math.round(ev.elapsedMs))} ms] ${ev.detail}`;
        },
        onSerialOutput: (bytes) => {
          this.terminal.writeOutput(bytes);
          const text = this.serialDecoder.decode(bytes, { stream: true });
          this.diagnostics.ingestSerialChunk(text);
          if (this.activeTab === 'diagnostics') {
            this.renderDiagnosticsView();
          }
        },
        onFramebufferDirty: (upd) => {
          this.display.applyDirtyUpdate(upd);
        },
        onLifecycleChange: (state, reason) => {
          this.vmState = state === 'powered_off' ? 'idle' : state;
          if (state === 'powered_off') {
            this.bootStatusLineEl.textContent = `VM powered off (${reason})`;
            this.terminal.writeSystemNotice(`\r\n[WASMNux] Guest powered off (${reason}).`);
          }
          this.syncControlsState();
          void this.refreshStorageAndSnapshots();
        },
        onTelemetry: (tel) => {
          const now = performance.now();
          if (this.lastStepsSample !== null && now > this.lastStepsSample.timeMs) {
            const deltaSteps = Number(tel.totalSteps - this.lastStepsSample.steps);
            const deltaSec = (now - this.lastStepsSample.timeMs) / 1000;
            if (deltaSteps >= 0 && deltaSec > 0) {
              this.measuredMips = deltaSteps / deltaSec / 1_000_000;
            }
          }
          this.lastStepsSample = { steps: tel.totalSteps, timeMs: now };
          this.lastTelemetry = tel;
          if (this.activeTab === 'telemetry') {
            this.renderTelemetryCards();
          }
        },
        onDiskCommitted: () => {
          void this.refreshProfileDiskLabels();
        },
        onStorageError: (err) => {
          this.reportError(`Persistent storage error: ${err.message}`);
        },
        onError: (err) => {
          this.reportError(`VM runtime error (${err.kind}): ${err.message}`);
        },
      },
    });

    this.buildDomSkeleton(savedTermPrefs.theme);

    const fbMount = this.root.querySelector<HTMLElement>('[data-testid="framebuffer-mount"]');
    const termMount = this.root.querySelector<HTMLElement>('[data-testid="terminal-mount"]');
    if (!fbMount || !termMount) {
      throw new Error('Missing framebuffer or terminal mount container');
    }

    this.terminal = new TerminalController({
      container: termMount,
      initialPreferences: savedTermPrefs,
      onInputBytes: (bytes) => {
        if (this.orchestrator.vmClient !== null && this.vmState === 'running') {
          void this.orchestrator.sendSerialInput(bytes).catch((err: unknown) => {
            const msg = err instanceof Error ? err.message : String(err);
            this.reportError(`Serial input failed: ${msg}`);
          });
        }
      },
    });
    this.terminal.writeSystemNotice(
      'WASMNux RV64GC Linux Workstation Ready. Select Vanilla Linux 7.3.1 or Debian 13.7 ("trixie") and click Boot VM.',
    );

    this.display = new FramebufferDisplayController({
      container: fbMount,
      preferredMode: 'auto',
      onBackendChanged: (backend, adapterInfo, fallbackReason) => {
        this.gpuBadgeEl.textContent =
          backend === 'webgpu' ? `GPU: WebGPU (${adapterInfo})` : 'GPU: Canvas 2D Fallback';
        this.gpuBadgeEl.dataset['tone'] = backend === 'webgpu' ? 'ok' : 'warn';
        if (fallbackReason && !fallbackReason.includes('navigator.gpu')) {
          this.reportError(fallbackReason);
        }
      },
    });
    await this.display.init();

    await this.refreshStorageAndSnapshots();
    this.syncControlsState();
    this.renderDiagnosticsView();
  }

  private buildDomSkeleton(initialTheme: TerminalThemeId): void {
    const main = document.createElement('main');
    main.className = 'wnx-shell';
    main.dataset['ready'] = 'true';

    main.innerHTML = `
      <header class="wnx-topbar">
        <div class="wnx-brand">
          <div>
            <h1>WASMNux · Browser-Native Linux</h1>
            <div class="wnx-brand-sub">RV64IMAFDC + Sv39 MMU in Rust/WASM · Linux 7.3.1 &amp; Official Debian 13.7 ("trixie")</div>
          </div>
        </div>
        <div class="wnx-badges">
          <span class="wnx-badge" data-testid="badge-vm-state" data-tone="active">State: Idle</span>
          <span class="wnx-badge" data-testid="badge-storage">Storage: probing...</span>
          <span class="wnx-badge" data-testid="badge-gpu">GPU: initializing...</span>
        </div>
      </header>

      <div class="wnx-error-banner" data-testid="error-banner" role="alert" hidden>
        <span data-testid="error-banner-text"></span>
        <button type="button" class="wnx-btn" data-testid="btn-dismiss-error">Dismiss</button>
      </div>

      <section class="wnx-control-deck" aria-label="Linux Profile and Virtual Machine Controls">
        <div class="wnx-card">
          <div class="wnx-profile-grid">
            <button type="button" class="wnx-profile-btn" data-testid="profile-vanilla" aria-pressed="true">
              <div class="wnx-profile-title">Vanilla Linux 7.3.1</div>
              <div class="wnx-profile-meta" data-testid="meta-vanilla">BusyBox 1.36.1 · 64 MiB ext4</div>
            </button>
            <button type="button" class="wnx-profile-btn" data-testid="profile-debian" aria-pressed="false">
              <div class="wnx-profile-title">Debian 13.7 ("trixie")</div>
              <div class="wnx-profile-meta" data-testid="meta-debian">glibc 2.41 · apt 3.0 / dpkg · 256 MiB ext4</div>
            </button>
          </div>
        </div>

        <div class="wnx-card">
          <div class="wnx-actions-bar">
            <button type="button" class="wnx-btn wnx-btn-primary" data-testid="btn-boot">Boot VM</button>
            <button type="button" class="wnx-btn" data-testid="btn-pause-resume" disabled>Pause</button>
            <button type="button" class="wnx-btn" data-testid="btn-reboot" disabled>Reboot</button>
            <button type="button" class="wnx-btn" data-testid="btn-shutdown" disabled>Shutdown</button>
            <button type="button" class="wnx-btn" data-testid="btn-hard-reset" disabled>Hard Reset</button>
            <button type="button" class="wnx-btn wnx-btn-danger" data-testid="btn-reset-disk">Reset Disk</button>
            <label style="font-size:0.78rem;color:var(--text-secondary);display:inline-flex;align-items:center;gap:4px;">
              RAM
              <select class="wnx-select" data-testid="select-ram">
                <option value="128">128 MiB</option>
                <option value="256">256 MiB</option>
                <option value="512">512 MiB</option>
              </select>
            </label>
            <button type="button" class="wnx-btn" data-testid="btn-toggle-net">Net: Online</button>
          </div>
          <div style="margin-top:8px;font-size:0.76rem;color:var(--text-secondary);font-family:var(--font-mono);" data-testid="boot-status-line">
            Ready to boot. Persistent guest disks are isolated per profile in OPFS + IndexedDB.
          </div>
        </div>
      </section>

      <section class="wnx-workspace">
        <div class="wnx-pane" data-testid="terminal-panel">
          <div class="wnx-pane-header">
            <strong>Guest Serial Console (/dev/ttyS0 · NS16550A)</strong>
            <div class="wnx-pane-tools">
              <button type="button" class="wnx-btn" data-testid="btn-term-copy">Copy</button>
              <button type="button" class="wnx-btn" data-testid="btn-term-paste">Paste</button>
              <button type="button" class="wnx-btn" data-testid="btn-term-stty">Sync Size</button>
              <button type="button" class="wnx-btn" data-testid="btn-font-dec" aria-label="Decrease font size">A-</button>
              <button type="button" class="wnx-btn" data-testid="btn-font-inc" aria-label="Increase font size">A+</button>
              <select class="wnx-select" data-testid="select-term-theme" aria-label="Terminal theme">
                <option value="midnight">Midnight</option>
                <option value="solarized">Solarized</option>
                <option value="amber">Amber CRT</option>
              </select>
              <button type="button" class="wnx-btn" data-testid="btn-term-clear">Clear</button>
            </div>
          </div>
          <div class="wnx-terminal-mount" data-testid="terminal-mount"></div>
        </div>

        <div class="wnx-pane" data-testid="framebuffer-panel">
          <div class="wnx-pane-header">
            <strong>SimpleFB Graphics (/dev/fb0 · 640×400)</strong>
            <div class="wnx-pane-tools">
              <select class="wnx-select" data-testid="select-gpu-mode" aria-label="Renderer backend">
                <option value="auto">Backend: Auto</option>
                <option value="webgpu">Backend: WebGPU</option>
                <option value="canvas2d">Backend: Canvas 2D</option>
              </select>
              <select class="wnx-select" data-testid="select-gpu-filter" aria-label="Display filter">
                <option value="crisp">Filter: Crisp</option>
                <option value="smooth">Filter: Smooth</option>
                <option value="crt">Filter: CRT Shader</option>
              </select>
              <button type="button" class="wnx-btn" data-testid="btn-fb-demo">Paint /dev/fb0</button>
            </div>
          </div>
          <div class="wnx-fb-mount" data-testid="framebuffer-mount"></div>
        </div>
      </section>

      <section class="wnx-inspector">
        <div class="wnx-tabs" role="tablist">
          <button type="button" class="wnx-tab-btn" role="tab" data-testid="tab-telemetry" aria-selected="true">Telemetry &amp; Storage</button>
          <button type="button" class="wnx-tab-btn" role="tab" data-testid="tab-snapshots" aria-selected="false">Snapshots &amp; Disk Import/Export</button>
          <button type="button" class="wnx-tab-btn" role="tab" data-testid="tab-diagnostics" aria-selected="false">Boot Diagnostics &amp; Capabilities</button>
        </div>

        <div data-testid="panel-telemetry">
          <div class="wnx-stat-grid" data-testid="telemetry-grid"></div>
        </div>

        <div data-testid="panel-snapshots" hidden>
          <div class="wnx-actions-bar">
            <input type="text" class="wnx-input" data-testid="input-snapshot-name" placeholder="Snapshot label (e.g. clean-apt)" />
            <button type="button" class="wnx-btn wnx-btn-primary" data-testid="btn-save-snapshot">Save Snapshot / Backup</button>
            <button type="button" class="wnx-btn" data-testid="btn-export-disk">Export Disk (.wasmnux-disk)</button>
            <label class="wnx-btn" style="display:inline-flex;align-items:center;gap:6px;">
              Import Disk
              <input type="file" data-testid="input-import-disk" accept=".wasmnux-disk,application/octet-stream" style="display:none;" />
            </label>
          </div>
          <div class="wnx-snapshot-list" data-testid="snapshot-list"></div>
        </div>

        <div data-testid="panel-diagnostics" hidden>
          <div class="wnx-milestone-pills" data-testid="milestone-list"></div>
          <div class="wnx-actions-bar">
            <input type="search" class="wnx-input" data-testid="input-log-filter" placeholder="Filter kernel / console logs..." />
            <button type="button" class="wnx-btn" data-testid="btn-download-diag">Download Diagnostics JSON</button>
          </div>
          <pre class="wnx-log-view" data-testid="kernel-log-view"></pre>
          <details style="margin-top:10px;">
            <summary style="cursor:pointer;font-size:0.8rem;color:var(--text-secondary);">Browser Runtime Capability Report</summary>
            <pre id="capability-report" class="wnx-log-view"></pre>
          </details>
        </div>
      </section>
    `;

    this.root.replaceChildren(main);

    const capPre = main.querySelector<HTMLElement>('#capability-report');
    if (capPre) {
      capPre.textContent = JSON.stringify(
        {
          abiVersion: this.bootData.abiVersion,
          workerSmoke: this.bootData.workerSmoke,
          capabilities: this.bootData.capabilities,
        },
        null,
        2,
      );
    }

    this.stateBadgeEl = this.mustElement('[data-testid="badge-vm-state"]');
    this.storageBadgeEl = this.mustElement('[data-testid="badge-storage"]');
    this.gpuBadgeEl = this.mustElement('[data-testid="badge-gpu"]');
    this.errorBannerEl = this.mustElement('[data-testid="error-banner"]');
    this.errorMessageEl = this.mustElement('[data-testid="error-banner-text"]');
    this.vanillaMetaEl = this.mustElement('[data-testid="meta-vanilla"]');
    this.debianMetaEl = this.mustElement('[data-testid="meta-debian"]');
    this.vanillaBtnEl = this.mustButton('[data-testid="profile-vanilla"]');
    this.debianBtnEl = this.mustButton('[data-testid="profile-debian"]');
    this.ramSelectEl = this.mustSelect('[data-testid="select-ram"]');
    this.bootBtnEl = this.mustButton('[data-testid="btn-boot"]');
    this.pauseBtnEl = this.mustButton('[data-testid="btn-pause-resume"]');
    this.rebootBtnEl = this.mustButton('[data-testid="btn-reboot"]');
    this.shutdownBtnEl = this.mustButton('[data-testid="btn-shutdown"]');
    this.hardResetBtnEl = this.mustButton('[data-testid="btn-hard-reset"]');
    this.resetDiskBtnEl = this.mustButton('[data-testid="btn-reset-disk"]');
    this.netToggleBtnEl = this.mustButton('[data-testid="btn-toggle-net"]');
    this.bootStatusLineEl = this.mustElement('[data-testid="boot-status-line"]');
    this.telemetryPanelEl = this.mustElement('[data-testid="panel-telemetry"]');
    this.snapshotsPanelEl = this.mustElement('[data-testid="panel-snapshots"]');
    this.diagnosticsPanelEl = this.mustElement('[data-testid="panel-diagnostics"]');
    this.snapshotListEl = this.mustElement('[data-testid="snapshot-list"]');
    this.snapshotInputEl = this.mustInput('[data-testid="input-snapshot-name"]');
    this.milestoneRowEl = this.mustElement('[data-testid="milestone-list"]');
    this.kernelLogPreEl = this.mustElement('[data-testid="kernel-log-view"]');

    const themeSelect = this.mustSelect('[data-testid="select-term-theme"]');
    themeSelect.value = initialTheme;
    this.ramSelectEl.value = String(LINUX_PROFILES[this.selectedProfile].defaultRamMib);

    this.wireDomEvents(themeSelect);
  }

  private mustElement(selector: string): HTMLElement {
    const el = this.root.querySelector<HTMLElement>(selector);
    if (!el) {
      throw new Error(`Required UI element missing: ${selector}`);
    }
    return el;
  }

  private mustButton(selector: string): HTMLButtonElement {
    const el = this.root.querySelector<HTMLButtonElement>(selector);
    if (!el) {
      throw new Error(`Required button missing: ${selector}`);
    }
    return el;
  }

  private mustInput(selector: string): HTMLInputElement {
    const el = this.root.querySelector<HTMLInputElement>(selector);
    if (!el) {
      throw new Error(`Required input missing: ${selector}`);
    }
    return el;
  }

  private mustSelect(selector: string): HTMLSelectElement {
    const el = this.root.querySelector<HTMLSelectElement>(selector);
    if (!el) {
      throw new Error(`Required select missing: ${selector}`);
    }
    return el;
  }

  private wireDomEvents(themeSelect: HTMLSelectElement): void {
    this.mustButton('[data-testid="btn-dismiss-error"]').addEventListener('click', () => {
      this.errorBannerEl.hidden = true;
    });

    this.vanillaBtnEl.addEventListener('click', () => {
      void this.selectProfile('vanilla');
    });
    this.debianBtnEl.addEventListener('click', () => {
      void this.selectProfile('debian');
    });

    this.bootBtnEl.addEventListener('click', () => {
      void this.handleBootClicked();
    });
    this.pauseBtnEl.addEventListener('click', () => {
      void this.handlePauseResumeClicked();
    });
    this.rebootBtnEl.addEventListener('click', () => {
      void this.handleRebootClicked();
    });
    this.shutdownBtnEl.addEventListener('click', () => {
      void this.handleShutdownClicked();
    });
    this.hardResetBtnEl.addEventListener('click', () => {
      void this.handleHardResetClicked();
    });
    this.resetDiskBtnEl.addEventListener('click', () => {
      void this.handleResetDiskClicked();
    });
    this.netToggleBtnEl.addEventListener('click', () => {
      this.networkEnabled = !this.networkEnabled;
      this.netToggleBtnEl.textContent = this.networkEnabled ? 'Net: Online' : 'Net: Offline';
      void this.orchestrator.setNetworkEnabled(this.networkEnabled).catch((err: unknown) => {
        this.reportError(err instanceof Error ? err.message : String(err));
      });
    });

    // Terminal toolbar
    this.mustButton('[data-testid="btn-term-copy"]').addEventListener('click', () => {
      const sel = this.terminal.getSelectionText();
      if (sel.length > 0 && 'clipboard' in navigator) {
        void navigator.clipboard.writeText(sel).catch(() => {
          this.reportError('Clipboard write permission denied by browser');
        });
      }
    });

    this.mustButton('[data-testid="btn-term-paste"]').addEventListener('click', () => {
      void (async () => {
        try {
          if ('clipboard' in navigator) {
            const text = await navigator.clipboard.readText();
            if (text.length > 0) {
              await this.terminal.pasteWithFlowControl(text);
              this.terminal.focus();
            }
          }
        } catch {
          this.reportError('Clipboard read blocked by browser; use Ctrl+Shift+V inside terminal.');
        }
      })();
    });

    this.mustButton('[data-testid="btn-term-stty"]').addEventListener('click', () => {
      this.terminal.fit();
      if (this.vmState === 'running') {
        this.terminal.sendGuestTtyResizeCommand();
      }
      this.terminal.focus();
    });

    this.mustButton('[data-testid="btn-font-dec"]').addEventListener('click', () => {
      const cur = this.terminal.preferences;
      const next = { ...cur, fontSize: Math.max(10, cur.fontSize - 1) };
      this.terminal.setPreferences(next);
      void this.storage.setPreference(PREF_KEY_TERMINAL, next);
    });

    this.mustButton('[data-testid="btn-font-inc"]').addEventListener('click', () => {
      const cur = this.terminal.preferences;
      const next = { ...cur, fontSize: Math.min(24, cur.fontSize + 1) };
      this.terminal.setPreferences(next);
      void this.storage.setPreference(PREF_KEY_TERMINAL, next);
    });

    themeSelect.addEventListener('change', () => {
      const val = themeSelect.value;
      if (val === 'midnight' || val === 'solarized' || val === 'amber') {
        const themeId: TerminalThemeId = val;
        const next = { ...this.terminal.preferences, theme: themeId };
        this.terminal.setPreferences(next);
        void this.storage.setPreference(PREF_KEY_TERMINAL, next);
      }
    });

    this.mustButton('[data-testid="btn-term-clear"]').addEventListener('click', () => {
      this.terminal.clear();
      this.terminal.focus();
    });

    // GPU toolbar
    const gpuModeSel = this.mustSelect('[data-testid="select-gpu-mode"]');
    gpuModeSel.addEventListener('change', () => {
      const v = gpuModeSel.value as PreferredRendererMode;
      void this.display.switchMode(v);
    });

    const gpuFilterSel = this.mustSelect('[data-testid="select-gpu-filter"]');
    gpuFilterSel.addEventListener('change', () => {
      const f = gpuFilterSel.value as RendererDisplayFilter;
      this.display.setFilter(f);
    });

    this.mustButton('[data-testid="btn-fb-demo"]').addEventListener('click', () => {
      if (this.vmState === 'running') {
        const flag = this.selectedProfile === 'debian' ? ' --debian' : '';
        void this.orchestrator.sendSerialInput(`/usr/bin/wasmnux-fbtest${flag}\n`);
      }
    });

    // Inspector tabs
    const tabTel = this.mustButton('[data-testid="tab-telemetry"]');
    const tabSnap = this.mustButton('[data-testid="tab-snapshots"]');
    const tabDiag = this.mustButton('[data-testid="tab-diagnostics"]');

    const switchTab = (tab: 'telemetry' | 'snapshots' | 'diagnostics'): void => {
      this.activeTab = tab;
      tabTel.setAttribute('aria-selected', String(tab === 'telemetry'));
      tabSnap.setAttribute('aria-selected', String(tab === 'snapshots'));
      tabDiag.setAttribute('aria-selected', String(tab === 'diagnostics'));
      this.telemetryPanelEl.hidden = tab !== 'telemetry';
      this.snapshotsPanelEl.hidden = tab !== 'snapshots';
      this.diagnosticsPanelEl.hidden = tab !== 'diagnostics';
      if (tab === 'telemetry') {
        this.renderTelemetryCards();
      } else if (tab === 'snapshots') {
        void this.renderSnapshotList();
      } else {
        this.renderDiagnosticsView();
      }
    };

    tabTel.addEventListener('click', () => {
      switchTab('telemetry');
    });
    tabSnap.addEventListener('click', () => {
      switchTab('snapshots');
    });
    tabDiag.addEventListener('click', () => {
      switchTab('diagnostics');
    });

    // Snapshots & Disk Import/Export
    this.mustButton('[data-testid="btn-save-snapshot"]').addEventListener('click', () => {
      void this.handleSaveSnapshotClicked();
    });
    this.mustButton('[data-testid="btn-export-disk"]').addEventListener('click', () => {
      void this.handleExportDiskClicked();
    });

    const importInput = this.mustInput('[data-testid="input-import-disk"]');
    importInput.addEventListener('change', () => {
      const file = importInput.files?.[0];
      importInput.value = '';
      if (file) {
        void this.handleImportDiskFile(file);
      }
    });

    // Diagnostics log filter + JSON download
    const filterInput = this.mustInput('[data-testid="input-log-filter"]');
    filterInput.addEventListener('input', () => {
      this.logFilter = filterInput.value;
      this.renderDiagnosticsView();
    });

    this.mustButton('[data-testid="btn-download-diag"]').addEventListener('click', () => {
      const json = this.diagnostics.buildDiagnosticReportJson({
        activeProfile: this.selectedProfile,
        vmState: this.vmState,
        rendererBackend: this.display.activeBackend,
        capabilities: this.bootData.capabilities,
        quota: this.quotaReport,
        telemetry: this.lastTelemetry,
        recentErrors: this.recentErrors,
      });
      this.triggerBlobDownload(
        new TextEncoder().encode(json),
        `wasmnux-diagnostics-${this.selectedProfile}.json`,
        'application/json',
      );
    });
  }

  private async selectProfile(next: LinuxProfileId): Promise<void> {
    if (this.selectedProfile === next && this.vmState === 'idle') {
      return;
    }
    this.resetConfirmArmed = false;
    this.resetDiskBtnEl.textContent = 'Reset Disk';

    if (this.vmState !== 'idle') {
      await this.orchestrator.shutdown();
      this.vmState = 'idle';
      this.display.clearScreen();
    }
    this.selectedProfile = next;
    this.ramSelectEl.value = String(LINUX_PROFILES[next].defaultRamMib);
    await this.storage.setPreference(PREF_KEY_ACTIVE_PROFILE, next);
    this.syncControlsState();
    await this.refreshStorageAndSnapshots();
  }

  private async handleBootClicked(): Promise<void> {
    if (this.vmState === 'booting') {
      return;
    }
    this.resetConfirmArmed = false;
    this.resetDiskBtnEl.textContent = 'Reset Disk';
    this.vmState = 'booting';
    this.syncControlsState();

    this.serialDecoder = new TextDecoder();
    this.diagnostics.reset(performance.now());
    this.lastStepsSample = null;
    this.measuredMips = 0;

    const ramMib =
      Number(this.ramSelectEl.value) || LINUX_PROFILES[this.selectedProfile].defaultRamMib;
    this.terminal.writeSystemNotice(
      `\r\n[WASMNux] Booting ${LINUX_PROFILES[this.selectedProfile].name} (${String(ramMib)} MiB RAM)...`,
    );

    try {
      const res = await this.orchestrator.bootProfile(this.selectedProfile, {
        ramMib,
        networkEnabled: this.networkEnabled,
      });
      this.vmState = 'running';
      this.bootStatusLineEl.textContent = `${LINUX_PROFILES[this.selectedProfile].name} booted (${String(Math.round(res.bootReadyMs))} ms asset+VM init)`;
      this.syncControlsState();
      this.terminal.focus();
      await this.refreshStorageAndSnapshots();
    } catch (err: unknown) {
      this.vmState = 'idle';
      this.syncControlsState();
      this.reportError(`Boot failed: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  private async handlePauseResumeClicked(): Promise<void> {
    try {
      if (this.vmState === 'running') {
        await this.orchestrator.pause();
        this.vmState = 'paused';
      } else if (this.vmState === 'paused') {
        await this.orchestrator.resume();
        this.vmState = 'running';
        this.terminal.focus();
      }
      this.syncControlsState();
    } catch (err: unknown) {
      this.reportError(err instanceof Error ? err.message : String(err));
    }
  }

  private async handleRebootClicked(): Promise<void> {
    try {
      if (this.vmState === 'running') {
        await this.orchestrator.sendSerialInput('\x03reboot\n');
      } else if (this.vmState === 'paused') {
        await this.orchestrator.hardReset();
        this.vmState = 'running';
        this.syncControlsState();
      }
    } catch (err: unknown) {
      this.reportError(err instanceof Error ? err.message : String(err));
    }
  }

  private isVmIdle(): boolean {
    return this.vmState === 'idle';
  }

  private async handleShutdownClicked(): Promise<void> {
    try {
      if (this.vmState === 'running') {
        await this.orchestrator.sendSerialInput('\x03poweroff\n');
        // Allow up to 4 seconds for guest init rcK sync/remount before forcing Worker stop
        const deadline = performance.now() + 4000;
        while (!this.isVmIdle() && performance.now() < deadline) {
          await new Promise((r) => setTimeout(r, 80));
        }
      }
      await this.orchestrator.shutdown();
      this.vmState = 'idle';
      this.syncControlsState();
      await this.refreshStorageAndSnapshots();
    } catch (err: unknown) {
      this.reportError(err instanceof Error ? err.message : String(err));
    }
  }

  private async handleHardResetClicked(): Promise<void> {
    try {
      await this.orchestrator.hardReset();
      this.vmState = 'running';
      this.syncControlsState();
      this.terminal.focus();
    } catch (err: unknown) {
      this.reportError(err instanceof Error ? err.message : String(err));
    }
  }

  private async handleResetDiskClicked(): Promise<void> {
    if (!this.resetConfirmArmed) {
      this.resetConfirmArmed = true;
      this.resetDiskBtnEl.textContent = 'Confirm Wipe Disk?';
      return;
    }
    this.resetConfirmArmed = false;
    this.resetDiskBtnEl.textContent = 'Reset Disk';
    try {
      await this.orchestrator.resetProfileStorage(this.selectedProfile);
      this.vmState = 'idle';
      this.display.clearScreen();
      this.bootStatusLineEl.textContent = `Reset ${LINUX_PROFILES[this.selectedProfile].name} persistent disk to clean factory state.`;
      this.syncControlsState();
      await this.refreshStorageAndSnapshots();
    } catch (err: unknown) {
      this.reportError(err instanceof Error ? err.message : String(err));
    }
  }

  private async handleSaveSnapshotClicked(): Promise<void> {
    const rawLabel = this.snapshotInputEl.value.trim() || `${this.selectedProfile}-snapshot`;
    const slug = sanitizeSnapshotSlug(rawLabel);
    const capBytes = LINUX_PROFILES[this.selectedProfile].diskMib * 1024 * 1024;
    try {
      let payload: Uint8Array | undefined;
      if (this.vmState === 'running' || this.vmState === 'paused') {
        const rawFull = await this.orchestrator.saveFullSnapshot();
        payload = await compressGzip(rawFull);
        await this.orchestrator.waitForPendingStorageCommits();
      }
      await this.storage.createNamedBackup(this.selectedProfile, slug, rawLabel, capBytes, payload);
      this.snapshotInputEl.value = '';
      this.bootStatusLineEl.textContent = `Saved ${payload ? 'full VM snapshot' : 'disk backup'} "${rawLabel}".`;
      await this.refreshStorageAndSnapshots();
    } catch (err: unknown) {
      this.reportError(`Snapshot save failed: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  private async handleExportDiskClicked(): Promise<void> {
    try {
      await this.orchestrator.waitForPendingStorageCommits();
      const capBytes = LINUX_PROFILES[this.selectedProfile].diskMib * 1024 * 1024;
      const bundle = await this.storage.exportProfileDiskBundle(this.selectedProfile, capBytes);
      this.triggerBlobDownload(
        bundle,
        `wasmnux-${this.selectedProfile}.wasmnux-disk`,
        'application/octet-stream',
      );
      this.bootStatusLineEl.textContent = `Exported ${formatBytes(bundle.byteLength)} disk image for ${this.selectedProfile}.`;
    } catch (err: unknown) {
      this.reportError(`Disk export failed: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  private async handleImportDiskFile(file: File): Promise<void> {
    try {
      if (file.size > 128 * 1024 * 1024) {
        throw new Error('Rejected disk bundle exceeding 128 MiB browser import safety ceiling');
      }
      const bytes = new Uint8Array(await file.arrayBuffer());
      if (this.vmState !== 'idle') {
        await this.orchestrator.shutdown();
        this.vmState = 'idle';
        this.syncControlsState();
      }
      await this.storage.importProfileDiskBundle(this.selectedProfile, bytes);
      this.bootStatusLineEl.textContent = `Imported verified disk bundle "${file.name}" into ${this.selectedProfile}.`;
      await this.refreshStorageAndSnapshots();
    } catch (err: unknown) {
      this.reportError(
        `Invalid disk import rejected: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  }

  private triggerBlobDownload(bytes: Uint8Array, filename: string, mime: string): void {
    const blob = new Blob([bytes.slice()], { type: mime });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    setTimeout(() => {
      URL.revokeObjectURL(url);
    }, 1000);
  }

  private syncControlsState(): void {
    this.vanillaBtnEl.setAttribute('aria-pressed', String(this.selectedProfile === 'vanilla'));
    this.debianBtnEl.setAttribute('aria-pressed', String(this.selectedProfile === 'debian'));

    const isRunning = this.vmState === 'running';
    const isPaused = this.vmState === 'paused';
    const isBooting = this.vmState === 'booting';
    const isActive = isRunning || isPaused;

    this.bootBtnEl.disabled = isBooting;
    this.bootBtnEl.textContent = isActive
      ? 'Restart Clean Boot'
      : isBooting
        ? 'Booting...'
        : 'Boot VM';
    this.pauseBtnEl.disabled = !isActive;
    this.pauseBtnEl.textContent = isPaused ? 'Resume' : 'Pause';
    this.rebootBtnEl.disabled = !isActive;
    this.shutdownBtnEl.disabled = !isActive;
    this.hardResetBtnEl.disabled = !isActive;
    this.ramSelectEl.disabled = isActive || isBooting;

    this.stateBadgeEl.textContent = `State: ${this.vmState.toUpperCase()}`;
    this.stateBadgeEl.dataset['tone'] = isRunning
      ? 'ok'
      : isPaused || isBooting
        ? 'warn'
        : 'active';
  }

  private async refreshProfileDiskLabels(): Promise<void> {
    const [vanStatus, debStatus] = await Promise.all([
      this.storage.getProfileStatus('vanilla', 64 * 1024 * 1024),
      this.storage.getProfileStatus('debian', 256 * 1024 * 1024),
    ]);
    this.vanillaMetaEl.textContent = `BusyBox 1.36.1 · Overlay ${formatBytes(vanStatus.overlayBytes)} / 64 MiB`;
    this.debianMetaEl.textContent = `Debian 13.7 · apt/dpkg · Overlay ${formatBytes(debStatus.overlayBytes)} / 256 MiB`;
  }

  private async refreshStorageAndSnapshots(): Promise<void> {
    await this.refreshProfileDiskLabels();
    this.quotaReport = await this.storage.inspectQuotaHealth();
    const persistTag = this.quotaReport.persisted ? 'Persisted' : 'Best-Effort';
    this.storageBadgeEl.textContent = `Storage: ${this.quotaReport.mode.toUpperCase()} (${persistTag})`;
    this.storageBadgeEl.dataset['tone'] = this.quotaReport.mode === 'opfs+idb' ? 'ok' : 'warn';

    if (this.quotaReport.lowSpaceWarning) {
      this.reportError('Browser storage quota is critically low (< 64 MiB remaining).');
    }

    this.renderTelemetryCards();
    await this.renderSnapshotList();
  }

  private renderTelemetryCards(): void {
    const grid = this.root.querySelector<HTMLElement>('[data-testid="telemetry-grid"]');
    if (!grid) {
      return;
    }
    const tel = this.lastTelemetry;
    const q = this.quotaReport;
    const gpuMetrics = this.display.getMetrics();

    const cards: ReadonlyArray<readonly [string, string]> = [
      ['Active Linux Profile', LINUX_PROFILES[this.selectedProfile].name],
      [
        'Guest RAM / WASM Heap',
        tel
          ? `${formatBytes(tel.guestRamBytes)} / ${formatBytes(tel.wasmLinearMemoryBytes)}`
          : `${this.ramSelectEl.value} MiB (offline)`,
      ],
      [
        'CPU Instructions / Throughput',
        tel
          ? `${tel.totalSteps.toLocaleString()} (${this.measuredMips.toFixed(1)} MIPS)`
          : '0 (idle)',
      ],
      [
        'Virtual Disk Overlay',
        tel
          ? `${formatBytes(tel.diskOverlayBytes)} / ${formatBytes(tel.diskCapacityBytes)}`
          : 'Idle',
      ],
      [
        'Disk Read / Write I/O',
        tel
          ? `${formatBytes(tel.diskBytesRead)} / ${formatBytes(tel.diskBytesWritten)}`
          : '0 B / 0 B',
      ],
      [
        'Browser Storage Quota',
        q
          ? `${formatBytes(q.browserUsageBytes)} / ${formatBytes(q.browserQuotaBytes)} (${q.mode})`
          : 'Probing...',
      ],
      [
        'Graphics Pipeline',
        gpuMetrics
          ? `${gpuMetrics.backend.toUpperCase()} · ${String(gpuMetrics.framesPresented)} frames`
          : 'Initializing',
      ],
      [
        'VirtIO-Net Bridge',
        `${this.networkEnabled ? 'Online' : 'Offline'} · RX ${tel ? formatBytes(tel.netRxBytes) : '0 B'}`,
      ],
    ];

    grid.innerHTML = cards
      .map(
        ([label, val]) => `
        <div class="wnx-stat-box">
          <div class="wnx-stat-label">${label}</div>
          <div class="wnx-stat-value">${val}</div>
        </div>`,
      )
      .join('');
  }

  private async renderSnapshotList(): Promise<void> {
    const items = await this.storage.listNamedBackups(this.selectedProfile);
    if (items.length === 0) {
      this.snapshotListEl.innerHTML = `<div style="font-size:0.8rem;color:var(--text-secondary);">No saved snapshots or backups for ${LINUX_PROFILES[this.selectedProfile].name}.</div>`;
      return;
    }

    this.snapshotListEl.replaceChildren();
    for (const item of items) {
      const row = document.createElement('div');
      row.className = 'wnx-snapshot-row';
      row.dataset['testid'] = `snapshot-row-${item.snapshotId}`;

      const info = document.createElement('span');
      info.textContent = `${item.label} (${item.snapshotId}) · ${formatBytes(item.byteLength)} · ${item.createdAtIso.slice(0, 19).replace('T', ' ')}`;

      const actions = document.createElement('div');
      actions.className = 'wnx-pane-tools';

      const restoreBtn = document.createElement('button');
      restoreBtn.type = 'button';
      restoreBtn.className = 'wnx-btn';
      restoreBtn.dataset['testid'] = `btn-restore-${item.snapshotId}`;
      restoreBtn.textContent = 'Restore';
      restoreBtn.addEventListener('click', () => {
        void this.handleRestoreSnapshotItem(item.snapshotId);
      });

      const delBtn = document.createElement('button');
      delBtn.type = 'button';
      delBtn.className = 'wnx-btn wnx-btn-danger';
      delBtn.dataset['testid'] = `btn-delete-${item.snapshotId}`;
      delBtn.textContent = 'Delete';
      delBtn.addEventListener('click', () => {
        void (async () => {
          await this.storage.deleteNamedBackup(this.selectedProfile, item.snapshotId);
          await this.renderSnapshotList();
        })();
      });

      actions.append(restoreBtn, delBtn);
      row.append(info, actions);
      this.snapshotListEl.appendChild(row);
    }
  }

  private async handleRestoreSnapshotItem(snapshotId: string): Promise<void> {
    try {
      const rec = await this.storage.loadNamedBackup(this.selectedProfile, snapshotId);
      const payload = await decompressGzipIfNeeded(rec.payload);
      // Detect whether payload is a full VM snapshot (`WNUXFULL`) or a disk bundle (`WASMNUXD`)
      const magic = new TextDecoder().decode(payload.subarray(0, 8));
      if (magic === 'WNUXFULL') {
        if (this.vmState === 'idle') {
          await this.handleBootClicked();
        }
        await this.orchestrator.restoreFullSnapshot(payload);
        this.vmState = 'running';
        this.syncControlsState();
        this.bootStatusLineEl.textContent = `Restored live VM snapshot "${rec.label}".`;
      } else {
        if (this.vmState !== 'idle') {
          await this.orchestrator.shutdown();
          this.vmState = 'idle';
          this.syncControlsState();
        }
        await this.storage.importProfileDiskBundle(this.selectedProfile, payload);
        this.bootStatusLineEl.textContent = `Restored disk backup "${rec.label}". Ready to boot.`;
        await this.refreshStorageAndSnapshots();
      }
    } catch (err: unknown) {
      this.reportError(
        `Snapshot restore failed: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  }

  private renderDiagnosticsView(): void {
    const milestones = this.diagnostics.getMilestones();
    if (milestones.length === 0) {
      this.milestoneRowEl.innerHTML = `<span class="wnx-badge">Boot a Linux profile to capture kernel milestones</span>`;
    } else {
      this.milestoneRowEl.innerHTML = milestones
        .map(
          (m) =>
            `<span class="wnx-badge" data-tone="ok">${m.label}: +${String(m.elapsedMs)} ms</span>`,
        )
        .join('');
    }
    const filtered = this.diagnostics.getFilteredLines(this.logFilter);
    this.kernelLogPreEl.textContent =
      filtered.length > 0 ? filtered.slice(-180).join('\n') : '(No kernel/console log lines match)';
  }

  public reportError(message: string): void {
    this.recentErrors.push(`[${new Date().toISOString()}] ${message}`);
    if (this.recentErrors.length > 50) {
      this.recentErrors.shift();
    }
    this.errorMessageEl.textContent = message;
    this.errorBannerEl.hidden = false;
  }
}
