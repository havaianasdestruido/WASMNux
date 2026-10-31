/**
 * WASMNux browser application entry point. Orchestrates the profile
 * selector, persistent disks, VM worker lifecycle, terminal, framebuffer
 * renderer and diagnostics. All guest interaction flows through the real
 * UART byte stream; the UI never fabricates kernel output.
 */

import "@xterm/xterm/css/xterm.css";
import "./style.css";

import { fetchAssetBytes, probeGuestAssets, type AssetState } from "./assets.js";
import {
  detectCapabilities,
  probeWebGpu,
  readStorageSnapshot,
  requestPersistence,
  type Capabilities,
} from "./capabilities.js";
import {
  createFramebufferRenderer,
  generateTestPattern,
  type FramebufferRenderer,
} from "./gpu/renderer.js";
import {
  PROFILE_IDS,
  PROFILES,
  parseProfileId,
  type GuestManifest,
  type Profile,
  type ProfileId,
} from "./profiles.js";
import { MemoryChunkStore, openDurableStore, type ChunkStore } from "./storage/chunkStore.js";
import { SparseDisk } from "./storage/sparseDisk.js";
import { GuestTerminal } from "./terminal.js";
import { VmClient, type VmStats } from "./vm/vmClient.js";
import type { MachineStateName } from "./vm/protocol.js";

const MiB = 1024 * 1024;

function el<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (node === null) {
    throw new Error(`missing required element #${id}`);
  }
  return node as T;
}

function formatBytes(bytes: number | undefined): string {
  if (bytes === undefined) {
    return "unknown";
  }
  if (bytes >= 1024 * MiB) return `${(bytes / (1024 * MiB)).toFixed(2)} GiB`;
  if (bytes >= MiB) return `${(bytes / MiB).toFixed(1)} MiB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${bytes} B`;
}

class App {
  readonly #capabilities: Capabilities = detectCapabilities();
  readonly #store: ChunkStore;
  readonly #storeIsDurable: boolean;
  readonly #terminal: GuestTerminal;
  readonly #vm: VmClient;
  readonly #disks = new Map<ProfileId, SparseDisk>();
  readonly #assetStates = new Map<ProfileId, AssetState>();
  #selected: ProfileId = "vanilla";
  #bootedProfile: ProfileId | undefined;
  #vmState: MachineStateName = "created";
  #renderer: FramebufferRenderer | undefined;
  #rendererNote = "not initialised";
  #frameTimer: ReturnType<typeof setInterval> | undefined;
  #diskImportChain: Promise<void> = Promise.resolve();

  constructor() {
    const durable = openDurableStore();
    this.#store = durable ?? new MemoryChunkStore();
    this.#storeIsDurable = durable !== undefined;

    this.#terminal = new GuestTerminal(el("terminal"));
    this.#vm = new VmClient();

    this.#terminal.onInput((bytes) => {
      if (this.#vmState === "running" || this.#vmState === "paused") {
        this.#vm.serialWrite(bytes);
      }
    });

    this.#vm.onSerial((bytes) => this.#terminal.writeGuestBytes(bytes));
    this.#vm.onError((message) => this.#log(`VM error: ${message}`, "err"));
    this.#vm.onState(({ state, reason }) => this.#handleState(state, reason));
    this.#vm.onStats((stats) => this.#renderStats(stats));
    this.#vm.onWasmStatus(() => {
      this.#renderWasmState();
      this.#renderControls();
    });
    this.#vm.onDiskImage((bytes) => {
      this.#diskImportChain = this.#diskImportChain.then(() => this.#persistDiskImage(bytes));
    });
    this.#vm.onFramebuffer((frame) => {
      this.#renderer?.present(frame);
      el("framebuffer-status").textContent =
        `Live guest framebuffer: ${frame.width}×${frame.height} (rendered by ${this.#renderer?.kind ?? "?"})`;
    });

    this.#wireButtons();
    this.#renderProfiles();
    this.#renderCapabilities();
    this.#renderWasmState();
    this.#renderControls();
  }

  async init(): Promise<void> {
    this.#log(
      this.#storeIsDurable
        ? `Persistent storage backend: ${this.#store.kind}`
        : "WARNING: no durable browser storage available; disks are kept in memory only and will NOT survive a reload.",
      this.#storeIsDurable ? "info" : "warn",
    );
    if (!this.#capabilities.webAssembly) {
      this.#log("This browser has no WebAssembly: the virtual machine cannot run here.", "err");
    }
    await this.#renderStoragePanel();
    await this.#refreshAssetStates();
    await this.#selectProfile(this.#selected, true);
    await this.#initRenderer();
    await this.#registerServiceWorker();
    const gpu = await probeWebGpu();
    this.#log(
      gpu.ok
        ? "WebGPU probe: adapter + device acquired successfully."
        : `WebGPU probe: unavailable (${gpu.reason}). Canvas 2D fallback will be used; the guest stays fully usable.`,
    );
  }

  // -----------------------------------------------------------------------
  // Diagnostics
  // -----------------------------------------------------------------------

  #log(message: string, level: "info" | "warn" | "err" = "info"): void {
    const list = el<HTMLOListElement>("diagnostics");
    const item = document.createElement("li");
    item.textContent = `${new Date().toLocaleTimeString()} — ${message}`;
    if (level !== "info") {
      item.className = level;
    }
    list.append(item);
    while (list.children.length > 200) {
      list.firstElementChild?.remove();
    }
    item.scrollIntoView({ block: "nearest" });
    if (level === "err") {
      console.error(message);
    } else if (level === "warn") {
      console.warn(message);
    }
  }

  // -----------------------------------------------------------------------
  // Capabilities & storage panels
  // -----------------------------------------------------------------------

  #renderCapabilities(): void {
    const caps = this.#capabilities;
    const rows: Array<[string, boolean, string]> = [
      ["WebAssembly", caps.webAssembly, "required for the VM core"],
      ["Web Workers", caps.webWorkers, "VM runs off the UI thread"],
      ["WebGPU API", caps.webGpuApi, "GPU framebuffer presentation"],
      ["OPFS", caps.opfs, "fast persistent disk bytes"],
      ["IndexedDB", caps.indexedDb, "metadata / storage fallback"],
      ["SharedArrayBuffer", caps.sharedArrayBuffer, "optional threaded WASM"],
      ["Cross-origin isolated", caps.crossOriginIsolated, "needed for SharedArrayBuffer"],
      ["Service Worker", caps.serviceWorker, "PWA offline shell"],
      ["Storage persist API", caps.storagePersistApi, "eviction resistance"],
    ];
    const table = el<HTMLTableElement>("capabilities");
    table.replaceChildren();
    for (const [name, ok, note] of rows) {
      const tr = document.createElement("tr");
      const nameCell = document.createElement("td");
      nameCell.textContent = name;
      const noteCell = document.createElement("td");
      noteCell.textContent = note;
      noteCell.className = "small";
      const stateCell = document.createElement("td");
      stateCell.textContent = ok ? "✓" : "—";
      stateCell.style.color = ok ? "var(--accent)" : "var(--muted)";
      tr.append(nameCell, noteCell, stateCell);
      table.append(tr);
    }
    const backend = document.createElement("tr");
    const label = document.createElement("td");
    label.textContent = "Disk backend in use";
    const value = document.createElement("td");
    value.textContent = this.#storeIsDurable ? this.#store.kind : "memory (NOT persistent)";
    value.colSpan = 2;
    value.style.textAlign = "right";
    backend.append(label, value);
    table.append(backend);
  }

  async #renderStoragePanel(): Promise<void> {
    try {
      const snapshot = await readStorageSnapshot();
      el("storage-info").textContent =
        `Persisted (eviction-resistant): ${
          snapshot.persisted === undefined ? "API unavailable" : snapshot.persisted ? "yes" : "NO — data may be evicted"
        } · Origin usage: ${formatBytes(snapshot.usageBytes)} / quota ${formatBytes(snapshot.quotaBytes)}`;
    } catch (error) {
      this.#log(`storage.estimate failed: ${error instanceof Error ? error.message : String(error)}`, "warn");
    }
  }

  // -----------------------------------------------------------------------
  // Profiles & guest assets
  // -----------------------------------------------------------------------

  #renderProfiles(): void {
    const container = el("profile-cards");
    container.replaceChildren();
    for (const id of PROFILE_IDS) {
      const profile = PROFILES[id];
      const card = document.createElement("button");
      card.type = "button";
      card.className = "profile-card";
      card.dataset["profile"] = id;
      card.setAttribute(
        "aria-pressed",
        String(this.#selected === id),
      );
      const state = this.#assetStates.get(id);
      const badgeClass = state?.kind === "ready" ? "ok" : state?.kind === "invalid" ? "bad" : "warn";
      const badgeText =
        state === undefined
          ? "checking assets…"
          : state.kind === "ready"
            ? `assets ready · kernel ${state.manifest.kernelVersion}`
            : state.kind === "missing"
              ? "guest assets not built"
              : "guest assets invalid";
      card.innerHTML = `
        <h3>${profile.displayName}</h3>
        <p>${profile.description}</p>
        <div class="meta">
          <span class="badge ${badgeClass}">${badgeText}</span>
          <span class="badge">${profile.ramMiB} MiB RAM</span>
          <span class="badge">${profile.defaultDiskMiB} MiB disk</span>
        </div>`;
      card.addEventListener("click", () => {
        void this.#selectProfile(id).catch((error: unknown) =>
          this.#log(`select profile failed: ${error instanceof Error ? error.message : String(error)}`, "err"),
        );
      });
      container.append(card);
    }
    this.#renderAssetDetail();
  }

  #renderAssetDetail(): void {
    const detail = el("asset-detail");
    const state = this.#assetStates.get(this.#selected);
    if (state === undefined) {
      detail.textContent = "Probing guest assets…";
      return;
    }
    if (state.kind === "ready") {
      const m = state.manifest;
      detail.textContent =
        `Guest assets: kernel ${m.kernelVersion} (${m.kernelImage})` +
        `${m.firmwareImage !== undefined ? `, firmware ${m.firmwareImage}` : ", direct-SBI boot"}` +
        `${m.initrdImage !== undefined ? `, initrd ${m.initrdImage}` : ""}` +
        `${m.diskImage !== undefined ? `, seed disk ${m.diskImage}` : ""} · built ${m.builtAt}`;
    } else {
      detail.textContent = state.detail;
    }
  }

  async #refreshAssetStates(): Promise<void> {
    for (const id of PROFILE_IDS) {
      this.#assetStates.set(id, await probeGuestAssets(PROFILES[id]));
    }
    this.#renderProfiles();
    this.#renderControls();
  }

  async #selectProfile(id: ProfileId, initial = false): Promise<void> {
    if (this.#vmState === "running" || this.#vmState === "paused") {
      if (!initial) {
        this.#log("Power off the running VM before switching profiles.", "warn");
      }
      return;
    }
    this.#selected = id;
    this.#renderProfiles();
    await this.#ensureDisk(PROFILES[id]);
    await this.#renderDiskPanel();
    this.#renderControls();
  }

  // -----------------------------------------------------------------------
  // Disks
  // -----------------------------------------------------------------------

  async #ensureDisk(profile: Profile): Promise<SparseDisk> {
    const existing = this.#disks.get(profile.id);
    if (existing !== undefined) {
      return existing;
    }
    try {
      const opened = await SparseDisk.open(this.#store, profile.diskId);
      this.#disks.set(profile.id, opened);
      return opened;
    } catch {
      // Not created yet: create with the profile default, then seed it from
      // the built root-disk image when the profile ships one (Debian).
      const created = await SparseDisk.open(this.#store, profile.diskId, {
        capacityBytes: profile.defaultDiskMiB * MiB,
      });
      this.#disks.set(profile.id, created);
      const state = this.#assetStates.get(profile.id);
      if (state?.kind === "ready" && state.manifest.diskImage !== undefined) {
        this.#log(`Seeding ${profile.displayName} disk from ${state.manifest.diskImage} (first boot only)…`);
        const seed = await fetchAssetBytes(profile, state.manifest.diskImage);
        if (seed.length !== created.capacityBytes) {
          this.#log(
            `Seed disk is ${formatBytes(seed.length)} but profile disk is ${formatBytes(created.capacityBytes)}; recreating disk at seed size.`,
          );
          await created.destroy();
          const recreated = await SparseDisk.open(this.#store, profile.diskId, {
            capacityBytes: seed.length,
          });
          this.#disks.set(profile.id, recreated);
          await recreated.importImage(seed);
          return recreated;
        }
        await created.importImage(seed);
      }
      return created;
    }
  }

  async #renderDiskPanel(): Promise<void> {
    const disk = this.#disks.get(this.#selected);
    const info = el("disk-info");
    if (disk === undefined) {
      info.textContent = "disk not open";
      return;
    }
    const usage = disk.usage();
    info.textContent =
      `capacity ${formatBytes(usage.capacityBytes)} · allocated ${formatBytes(usage.allocatedBytes)} ` +
      `(${usage.allocatedChunks} chunks) · generation ${usage.generation}${disk.dirty ? " · unflushed writes pending" : ""}`;
    const select = el<HTMLSelectElement>("snapshot-list");
    const snapshots = await disk.listSnapshots();
    select.replaceChildren();
    for (const name of snapshots) {
      const option = document.createElement("option");
      option.value = name;
      option.textContent = name;
      select.append(option);
    }
    select.disabled = snapshots.length === 0;
  }

  async #persistDiskImage(bytes: Uint8Array): Promise<void> {
    const profileId = this.#bootedProfile ?? this.#selected;
    try {
      const disk = await this.#ensureDisk(PROFILES[profileId]);
      await disk.importImage(bytes);
      this.#vm.acknowledgeDiskPersisted();
      await this.#renderDiskPanel();
      await this.#renderStoragePanel();
    } catch (error) {
      this.#log(
        `persisting guest disk failed (guest keeps running; will retry on next sync): ${
          error instanceof Error ? error.message : String(error)
        }`,
        "err",
      );
    }
  }

  // -----------------------------------------------------------------------
  // VM lifecycle
  // -----------------------------------------------------------------------

  #bootBlockers(profile: Profile): string[] {
    const reasons: string[] = [];
    if (!this.#capabilities.webAssembly || !this.#capabilities.webWorkers) {
      reasons.push("This browser lacks WebAssembly/Web Workers.");
    }
    if (this.#vm.wasmStatus === "loading") {
      reasons.push("WASM core still loading…");
    }
    if (this.#vm.wasmStatus === "unavailable") {
      reasons.push(this.#vm.unavailableReason ?? "WASM core unavailable.");
    }
    const state = this.#assetStates.get(profile.id);
    if (state === undefined) {
      reasons.push("Guest assets not probed yet.");
    } else if (state.kind !== "ready") {
      reasons.push(state.detail);
    }
    if (this.#vmState === "running" || this.#vmState === "paused") {
      reasons.push("A VM is already running; power it off before booting again.");
    }
    return reasons;
  }

  #renderControls(): void {
    const profile = PROFILES[this.#selected];
    const blockers = this.#bootBlockers(profile);
    const boot = el<HTMLButtonElement>("btn-boot");
    boot.disabled = blockers.length > 0;
    const list = el<HTMLUListElement>("boot-reasons");
    list.replaceChildren();
    for (const reason of blockers) {
      const li = document.createElement("li");
      li.textContent = reason;
      list.append(li);
    }
    el<HTMLButtonElement>("btn-pause").disabled = this.#vmState !== "running";
    el<HTMLButtonElement>("btn-resume").disabled = this.#vmState !== "paused";
    el<HTMLButtonElement>("btn-poweroff").disabled =
      this.#vmState !== "running" && this.#vmState !== "paused";
    el("vm-state").textContent = this.#vmState;
  }

  #renderWasmState(): void {
    const node = el("wasm-state");
    node.textContent =
      this.#vm.wasmStatus === "ready"
        ? `ready (core v${this.#vm.coreVersion ?? "?"})`
        : this.#vm.wasmStatus === "unavailable"
          ? "unavailable — see boot blockers"
          : "loading…";
  }

  #renderStats(stats: VmStats): void {
    el("vm-stats").textContent =
      `instructions retired: ${stats.instructions.toLocaleString()} · pc ${stats.pcHex}`;
  }

  #handleState(state: MachineStateName, reason?: string): void {
    this.#vmState = state;
    if (reason !== undefined) {
      this.#log(reason);
      this.#terminal.writeHostNotice(reason);
    }
    if (state === "halted" || state === "created") {
      this.#stopFrameLoop();
    }
    this.#renderControls();
    void this.#renderDiskPanel();
  }

  async #boot(): Promise<void> {
    const profile = PROFILES[this.#selected];
    const state = this.#assetStates.get(profile.id);
    if (state?.kind !== "ready") {
      this.#log("Refusing to boot: guest assets are not ready (nothing will be faked).", "err");
      return;
    }
    const manifest: GuestManifest = state.manifest;
    const disk = await this.#ensureDisk(profile);
    this.#log(
      `Booting ${profile.displayName}: kernel ${manifest.kernelVersion}, ${profile.ramMiB} MiB RAM, ` +
        `${manifest.firmwareImage !== undefined ? "OpenSBI firmware" : "built-in SBI shim"}, disk ${formatBytes(disk.capacityBytes)}.`,
    );
    this.#terminal.clear();
    this.#terminal.writeHostNotice(
      `Booting ${profile.displayName} (Linux ${manifest.kernelVersion}). Everything after this line is guest UART output.`,
    );
    const [kernel, firmware, initrd, diskBytes] = await Promise.all([
      fetchAssetBytes(profile, manifest.kernelImage),
      manifest.firmwareImage !== undefined
        ? fetchAssetBytes(profile, manifest.firmwareImage)
        : Promise.resolve(undefined),
      manifest.initrdImage !== undefined
        ? fetchAssetBytes(profile, manifest.initrdImage)
        : Promise.resolve(undefined),
      disk.exportImage(),
    ]);
    this.#bootedProfile = profile.id;
    const result = await this.#vm.boot({
      profile: profile.id,
      ramMiB: profile.ramMiB,
      bootargs: profile.bootargs,
      bootMode: firmware !== undefined ? "firmware" : "direct-sbi",
      kernel,
      ...(firmware !== undefined ? { firmware } : {}),
      ...(initrd !== undefined ? { initrd } : {}),
      disk: diskBytes,
      defaultDiskMiB: profile.defaultDiskMiB,
    });
    this.#vmState = result;
    this.#startFrameLoop();
    this.#renderControls();
    this.#terminal.focus();
  }

  #startFrameLoop(): void {
    this.#stopFrameLoop();
    this.#frameTimer = setInterval(() => {
      if (this.#vmState === "running") {
        this.#vm.requestFramebuffer();
      }
    }, 100);
  }

  #stopFrameLoop(): void {
    if (this.#frameTimer !== undefined) {
      clearInterval(this.#frameTimer);
      this.#frameTimer = undefined;
    }
  }

  // -----------------------------------------------------------------------
  // Renderer & service worker
  // -----------------------------------------------------------------------

  async #initRenderer(): Promise<void> {
    const canvas = el<HTMLCanvasElement>("framebuffer");
    const recreate = async (reason: string): Promise<void> => {
      this.#log(`WebGPU renderer issue: ${reason}. Falling back to Canvas 2D.`, "warn");
      this.#renderer?.dispose();
      // A canvas that already vended a WebGPU context refuses a 2D one, so
      // the fallback renderer gets a fresh canvas element in its place.
      const fresh = document.createElement("canvas");
      fresh.id = canvas.id;
      fresh.width = canvas.width;
      fresh.height = canvas.height;
      canvas.replaceWith(fresh);
      this.#renderer = await createFramebufferRenderer(fresh, () => undefined);
      this.#renderRendererBadge();
    };
    this.#renderer = await createFramebufferRenderer(canvas, (reason) => {
      this.#rendererNote = reason;
      void recreate(reason);
    });
    this.#renderRendererBadge();
  }

  #renderRendererBadge(): void {
    const badge = el("renderer-badge");
    const kind = this.#renderer?.kind;
    badge.textContent =
      kind === "webgpu"
        ? "renderer: WebGPU (WGSL swizzle + texture upload)"
        : `renderer: Canvas 2D fallback (${this.#rendererNote})`;
    badge.className = `badge ${kind === "webgpu" ? "ok" : "warn"}`;
  }

  async #registerServiceWorker(): Promise<void> {
    if (!this.#capabilities.serviceWorker) {
      return;
    }
    try {
      await navigator.serviceWorker.register("/sw.js");
      this.#log("Service worker registered (app shell cached for offline startup; guest disks persist separately in OPFS/IndexedDB).");
    } catch (error) {
      this.#log(
        `Service worker registration failed: ${error instanceof Error ? error.message : String(error)}`,
        "warn",
      );
    }
  }

  // -----------------------------------------------------------------------
  // Buttons
  // -----------------------------------------------------------------------

  #wireButtons(): void {
    const run =
      (fn: () => Promise<void> | void) =>
      (): void => {
        void Promise.resolve(fn()).catch((error: unknown) => {
          this.#log(error instanceof Error ? error.message : String(error), "err");
        });
      };

    el("btn-boot").addEventListener("click", run(() => this.#boot()));
    el("btn-pause").addEventListener("click", run(async () => {
      this.#vmState = await this.#vm.pause();
      this.#renderControls();
    }));
    el("btn-resume").addEventListener("click", run(async () => {
      this.#vmState = await this.#vm.resume();
      this.#renderControls();
    }));
    el("btn-poweroff").addEventListener("click", run(async () => {
      this.#vmState = await this.#vm.powerOff();
      await this.#diskImportChain;
      this.#terminal.writeHostNotice("Virtual machine powered off. Disk state was persisted before shutdown completed.");
      this.#renderControls();
      await this.#renderDiskPanel();
    }));
    el("btn-clear-terminal").addEventListener("click", () => this.#terminal.clear());

    el("btn-request-persist").addEventListener("click", run(async () => {
      const granted = await requestPersistence();
      this.#log(
        granted === undefined
          ? "navigator.storage.persist() is not exposed by this browser."
          : granted
            ? "Persistent storage granted."
            : "Persistent storage was NOT granted by the browser; disks remain evictable.",
        granted ? "info" : "warn",
      );
      await this.#renderStoragePanel();
    }));

    el("btn-snapshot").addEventListener("click", run(async () => {
      const name = el<HTMLInputElement>("snapshot-name").value.trim();
      if (name === "") {
        throw new Error("give the snapshot a name first");
      }
      const disk = await this.#ensureDisk(PROFILES[this.#selected]);
      await disk.snapshot(name);
      el<HTMLInputElement>("snapshot-name").value = "";
      this.#log(`Snapshot "${name}" created for ${PROFILES[this.#selected].displayName}.`);
      await this.#renderDiskPanel();
    }));
    el("btn-restore").addEventListener("click", run(async () => {
      if (this.#vmState === "running" || this.#vmState === "paused") {
        throw new Error("power off the VM before restoring a disk snapshot");
      }
      const name = el<HTMLSelectElement>("snapshot-list").value;
      if (name === "") throw new Error("no snapshot selected");
      const disk = await this.#ensureDisk(PROFILES[this.#selected]);
      await disk.restoreSnapshot(name);
      this.#log(`Restored ${PROFILES[this.#selected].displayName} disk to snapshot "${name}".`);
      await this.#renderDiskPanel();
    }));
    el("btn-delete-snapshot").addEventListener("click", run(async () => {
      const name = el<HTMLSelectElement>("snapshot-list").value;
      if (name === "") throw new Error("no snapshot selected");
      const disk = await this.#ensureDisk(PROFILES[this.#selected]);
      await disk.deleteSnapshot(name);
      await this.#renderDiskPanel();
    }));

    el("btn-export-disk").addEventListener("click", run(async () => {
      const disk = await this.#ensureDisk(PROFILES[this.#selected]);
      const image = await disk.exportImage();
      const blob = new Blob([image as BlobPart], { type: "application/octet-stream" });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `wasmnux-${this.#selected}-disk.img`;
      anchor.click();
      setTimeout(() => URL.revokeObjectURL(url), 5_000);
      this.#log(`Exported ${PROFILES[this.#selected].displayName} disk (${formatBytes(image.length)}).`);
    }));

    el<HTMLInputElement>("import-disk-file").addEventListener("change", (event) => {
      void (async () => {
        try {
          if (this.#vmState === "running" || this.#vmState === "paused") {
            throw new Error("power off the VM before importing a disk image");
          }
          const input = event.target as HTMLInputElement;
          const file = input.files?.[0];
          input.value = "";
          if (file === undefined) return;
          const bytes = new Uint8Array(await file.arrayBuffer());
          if (bytes.length === 0 || bytes.length % 512 !== 0) {
            throw new Error(`invalid disk image: size ${bytes.length} is not a positive multiple of 512`);
          }
          if (bytes.length > 4 * 1024 * MiB) {
            throw new Error("invalid disk image: larger than the 4 GiB import limit");
          }
          let disk = await this.#ensureDisk(PROFILES[this.#selected]);
          if (disk.capacityBytes !== bytes.length) {
            this.#log(`Import replaces the ${formatBytes(disk.capacityBytes)} disk with a ${formatBytes(bytes.length)} image.`);
            await disk.destroy();
            this.#disks.delete(this.#selected);
            disk = await SparseDisk.open(this.#store, PROFILES[this.#selected].diskId, {
              capacityBytes: bytes.length,
            });
            this.#disks.set(this.#selected, disk);
          }
          await disk.importImage(bytes);
          this.#log(`Imported disk image (${formatBytes(bytes.length)}) for ${PROFILES[this.#selected].displayName}.`);
          await this.#renderDiskPanel();
        } catch (error) {
          this.#log(`disk import failed: ${error instanceof Error ? error.message : String(error)}`, "err");
        }
      })();
    });

    el("btn-reset-disk").addEventListener("click", run(async () => {
      if (this.#vmState === "running" || this.#vmState === "paused") {
        throw new Error("power off the VM before resetting a disk");
      }
      const profile = PROFILES[this.#selected];
      if (!window.confirm(`Destroy ALL data on the ${profile.displayName} disk (including snapshots)? This cannot be undone.`)) {
        return;
      }
      const disk = await this.#ensureDisk(profile);
      await disk.destroy();
      this.#disks.delete(profile.id);
      await this.#ensureDisk(profile);
      this.#log(`${profile.displayName} disk reset to a fresh ${profile.defaultDiskMiB} MiB image.`);
      await this.#renderDiskPanel();
    }));

    el("btn-test-pattern").addEventListener("click", () => {
      this.#renderer?.present(generateTestPattern(1024, 600));
      el("framebuffer-status").textContent =
        `Renderer self-test pattern shown (NOT guest output) via ${this.#renderer?.kind ?? "?"}.`;
    });
  }

  /**
   * Real storage hooks for the Playwright persistence suite: they drive the
   * exact SparseDisk the guest uses, at a marker offset. They never run
   * concurrently with the guest (tests use them while the VM is off).
   */
  testHooks(): WasmnuxTestHooks {
    return {
      version: "0.1.0",
      storeKind: this.#store.kind,
      wasmStatus: (): string => this.#vm.wasmStatus,
      wasmReason: (): string | undefined => this.#vm.unavailableReason,
      profileAssetKind: (profile: string): string => {
        const id = parseProfileId(profile);
        if (id === undefined) return "unknown-profile";
        return this.#assetStates.get(id)?.kind ?? "unknown";
      },
      writeDiskMarker: async (profile: string, text: string): Promise<void> => {
        const id = parseProfileId(profile);
        if (id === undefined) throw new Error(`unknown profile ${JSON.stringify(profile)}`);
        const bytes = new TextEncoder().encode(text);
        if (bytes.length > MARKER_LENGTH) throw new Error("marker too long");
        const disk = await this.#ensureDisk(PROFILES[id]);
        const padded = new Uint8Array(MARKER_LENGTH);
        padded.set(bytes);
        await disk.write(MARKER_OFFSET, padded);
        await disk.flush();
        if (id === this.#selected) await this.#renderDiskPanel();
      },
      readDiskMarker: async (profile: string): Promise<string> => {
        const id = parseProfileId(profile);
        if (id === undefined) throw new Error(`unknown profile ${JSON.stringify(profile)}`);
        const disk = await this.#ensureDisk(PROFILES[id]);
        const raw = await disk.read(MARKER_OFFSET, MARKER_LENGTH);
        const end = raw.indexOf(0);
        return new TextDecoder().decode(end === -1 ? raw : raw.subarray(0, end));
      },
    };
  }
}

/** Test/automation hooks (real disk APIs, used by the Playwright suite). */
export interface WasmnuxTestHooks {
  readonly version: string;
  readonly storeKind: string;
  wasmStatus(): string;
  wasmReason(): string | undefined;
  profileAssetKind(profile: string): string;
  writeDiskMarker(profile: string, text: string): Promise<void>;
  readDiskMarker(profile: string): Promise<string>;
}

const MARKER_OFFSET = 1024 * 1024;
const MARKER_LENGTH = 4096;

async function main(): Promise<void> {
  const app = new App();
  // Install test hooks before init()'s async probing so automation never
  // races the app's own startup; hook methods observe live app state.
  const w = window as unknown as Record<string, unknown>;
  w["__wasmnux"] = app.testHooks();
  await app.init();
}

void main().catch((error: unknown) => {
  console.error("WASMNux failed to start", error);
  document.body.insertAdjacentHTML(
    "beforeend",
    `<p style="color:#ff7b72;padding:1rem">WASMNux failed to start: ${
      error instanceof Error ? error.message : String(error)
    }</p>`,
  );
});
