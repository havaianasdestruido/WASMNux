/**
 * WASMNux browser application entry point.
 *
 * Wires together the WASM runtime, storage, renderer, terminal, and UI.
 * Browser execution cannot be verified in this build sandbox (no browser
 * binary is installed); the module is validated with `tsc --noEmit`.
 */

import { loadWasmModule, RunResultCode, WasmMachine } from "./runtime";
import { detectPersistence, OpfsDisk } from "./storage";
import { Renderer } from "./renderer";
import { GuestTerminal } from "./terminal";
import type { LinuxProfile, VmStatus } from "./types";

const WASM_URL = "./wasmnux_core.wasm";

const profileSelect = document.getElementById(
  "profile-select",
) as HTMLSelectElement | null;
const bootBtn = document.getElementById("boot-btn") as HTMLButtonElement | null;
const shutdownBtn = document.getElementById(
  "shutdown-btn",
) as HTMLButtonElement | null;
const rebootBtn = document.getElementById(
  "reboot-btn",
) as HTMLButtonElement | null;
const terminalContainer = document.getElementById("terminal");
const canvas = document.getElementById("gfx") as HTMLCanvasElement | null;
const statusEl = document.getElementById("status");
const capsEl = document.getElementById("caps");

let machine: WasmMachine | null = null;
let wasmMemory: WebAssembly.Memory | null = null;
let status: VmStatus = "stopped";
let profile: LinuxProfile = "vanilla";
let renderer: Renderer | null = null;
let terminal: GuestTerminal | null = null;
let animationFrame: number | null = null;

function setStatus(s: VmStatus): void {
  status = s;
  if (statusEl) {
    statusEl.textContent = `status: ${s}`;
  }
  if (bootBtn) {
    bootBtn.disabled = s === "running" || s === "booting";
  }
  if (shutdownBtn) {
    shutdownBtn.disabled = s === "stopped";
  }
  if (rebootBtn) {
    rebootBtn.disabled = s === "stopped";
  }
}

async function init(): Promise<void> {
  const webgpu = (navigator as unknown as { gpu?: unknown }).gpu !== undefined;
  const opfs =
    (navigator.storage as unknown as { getDirectory?: () => unknown })
      .getDirectory !== undefined;
  const indexedDbAvailable = typeof indexedDB !== "undefined";
  const sab = typeof SharedArrayBuffer !== "undefined";
  const info = await detectPersistence();

  if (capsEl) {
    capsEl.innerHTML = `
      webgpu: ${webgpu}<br>
      opfs: ${opfs}<br>
      indexedDB: ${indexedDbAvailable}<br>
      SharedArrayBuffer: ${sab}<br>
      persistence: ${info.persistence} (durable=${info.durable})<br>
      storage: ${info.used} / ${info.quota} bytes
    `;
  }

  let wasmBytes: ArrayBuffer;
  try {
    const res = await fetch(WASM_URL);
    if (!res.ok) {
      throw new Error(`WASM fetch failed: ${res.status}`);
    }
    wasmBytes = await res.arrayBuffer();
  } catch (err) {
    setStatus("error");
    if (statusEl) {
      statusEl.textContent = `failed to load WASM runtime: ${String(err)}`;
    }
    return;
  }

  const { exports } = await loadWasmModule(wasmBytes);
  wasmMemory = exports.memory;
  machine = new WasmMachine(exports, 256 * 1024 * 1024, 1);

  if (canvas) {
    renderer = new Renderer(canvas);
    await renderer.initialize();
  }
  if (terminalContainer) {
    terminal = new GuestTerminal(terminalContainer);
    terminal.attach(machine);
  }

  if (profileSelect) {
    profile = (profileSelect.value as LinuxProfile) || "vanilla";
    profileSelect.addEventListener("change", (e) => {
      profile = (e.target as HTMLSelectElement).value as LinuxProfile;
    });
  }
  if (bootBtn) {
    bootBtn.addEventListener("click", () => {
      void boot();
    });
  }
  if (shutdownBtn) {
    shutdownBtn.addEventListener("click", () => shutdown());
  }
  if (rebootBtn) {
    rebootBtn.addEventListener("click", () => reboot());
  }

  setStatus("stopped");
}

async function boot(): Promise<void> {
  if (!machine || !wasmMemory) {
    return;
  }
  setStatus("booting");
  try {
    const disk = new OpfsDisk(`disks/${profile}.disk`);
    await disk.ready();
    const data = await disk.exportToArrayBuffer();
    const view = new Uint8Array(data);
    machine.writeMem(0x8000_0000, view);
    const dtb = machine.loadDeviceTree();
    machine.setBootArgs(0, dtb);

    runLoop();
    setStatus("running");
  } catch (err) {
    setStatus("error");
    console.error("boot failed", err);
  }
}

function runLoop(): void {
  if (!machine || status !== "running") {
    return;
  }
  const rc = machine.run(10_000);
  if (rc === RunResultCode.Trap) {
    setStatus("error");
    return;
  }
  const out = machine.drainConsole();
  if (out.length && terminal) {
    terminal.write(out);
  }
  animationFrame = requestAnimationFrame(runLoop);
}

function shutdown(): void {
  if (machine) {
    machine.halt();
  }
  if (animationFrame !== null) {
    cancelAnimationFrame(animationFrame);
    animationFrame = null;
  }
  setStatus("stopped");
}

function reboot(): void {
  shutdown();
  setTimeout(() => boot(), 100);
}

document.addEventListener("DOMContentLoaded", () => {
  init().catch((err) => {
    console.error("init failed", err);
    setStatus("error");
  });
});
