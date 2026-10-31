import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import "./style.css";
import { detectCapabilities, queryPersistence } from "./capabilities";
import { FramebufferPresenter } from "./graphics/framebuffer";
import { MAX_INPUT_BYTES, parseGuestEvent } from "./protocol";
import { IndexedDbDiskStore } from "./storage/indexeddb-disk";

function element(id: string): HTMLElement {
  const found = document.getElementById(id);
  if (!(found instanceof HTMLElement)) throw new Error(`Missing element: ${id}`);
  return found;
}

function button(id: string): HTMLButtonElement {
  const found = element(id);
  if (!(found instanceof HTMLButtonElement)) throw new Error(`Not a button: ${id}`);
  return found;
}

const startButton = button("start");
const stopButton = button("stop");
const vmStatus = element("vm-status");
const message = element("message");
const storageStatus = element("storage-status");
const terminal = new Terminal({
  cursorBlink: true,
  fontFamily: '"SFMono-Regular", Consolas, "Liberation Mono", monospace',
  fontSize: 13,
  lineHeight: 1.35,
  scrollback: 3000,
  theme: {
    background: "#0c100e",
    foreground: "#c9d4cc",
    cursor: "#a6e7ae",
    selectionBackground: "#456e51",
    black: "#161d18",
    green: "#8fd19a",
    brightGreen: "#b5edba",
  },
});
const fit = new FitAddon();
terminal.loadAddon(fit);
terminal.open(element("terminal"));
fit.fit();
terminal.writeln("\x1b[90mWASMNux diagnostic console — guest not started.\x1b[0m");
terminal.writeln("\x1b[90mBoot to view output from the actual RISC-V guest.\x1b[0m");

const capabilities = detectCapabilities();
for (const [id, available] of [
  ["cap-wasm", capabilities.webAssembly],
  ["cap-worker", capabilities.worker],
  ["cap-gpu", capabilities.webGPU],
  ["cap-opfs", capabilities.opfs],
  ["cap-shared", capabilities.sharedArrayBuffer],
] as const) {
  const item = element(id);
  item.textContent = available ? "Available" : "Unavailable";
  item.dataset.available = String(available);
}

async function refreshPersistence(): Promise<void> {
  const status = await queryPersistence();
  const labels = {
    persisted: "Persistence granted",
    "best-effort": "Best effort · eviction possible",
    unavailable: "Persistence API unavailable",
    error: "Could not check persistence",
  };
  storageStatus.textContent = labels[status];
}
void refreshPersistence();

button("request-persistence").addEventListener("click", () => {
  void (async () => {
    if (typeof navigator.storage?.persist !== "function") {
      storageStatus.textContent = "Persistence request unavailable";
      return;
    }
    try {
      const granted = await navigator.storage.persist();
      storageStatus.textContent = granted
        ? "Persistence granted"
        : "Request denied · eviction possible";
    } catch {
      storageStatus.textContent = "Persistence request failed";
    }
  })();
});

button("inspect-storage").addEventListener("click", () => {
  void (async () => {
    const status = element("block-status");
    status.textContent = "Checking prototype block store…";
    try {
      const store = await IndexedDbDiskStore.open();
      try {
        const [vanilla, debian] = await Promise.all([store.info("vanilla"), store.info("debian")]);
        const names = [vanilla && "Vanilla", debian && "Debian"].filter(Boolean);
        status.textContent = names.length
          ? `Provisioned: ${names.join(", ")}. Not connected to a guest.`
          : "No profile disks provisioned. Not connected to a guest.";
      } finally {
        store.close();
      }
    } catch {
      status.textContent =
        "Prototype block store unavailable or corrupt. Not connected to a guest.";
    }
  })();
});

let displayPresenter: FramebufferPresenter | undefined;
let displayGeneration = 0;
button("test-display").addEventListener("click", () => {
  void (async () => {
    const generation = ++displayGeneration;
    const action = button("test-display");
    const result = element("display-result");
    element("display-test").hidden = false;
    action.disabled = true;
    result.textContent = "Initializing renderer…";
    displayPresenter?.dispose();
    displayPresenter = undefined;
    let presenter: FramebufferPresenter | undefined;
    try {
      presenter = await FramebufferPresenter.create(element("framebuffer-host"), {
        onBackendChange: (kind, reason) => {
          if (generation === displayGeneration) result.textContent = `${kind}: ${reason}`;
        },
      });
      if (generation !== displayGeneration) {
        presenter.dispose();
        return;
      }
      displayPresenter = presenter;
      const width = 96;
      const height = 24;
      const pixels = new Uint8Array(width * height * 4);
      const colors = [
        [145, 211, 154],
        [89, 146, 172],
        [223, 190, 132],
      ] as const;
      for (let y = 0; y < height; y++) {
        for (let x = 0; x < width; x++) {
          const color = colors[Math.floor(x / 32)];
          if (!color) throw new Error("Invalid display test pattern");
          const offset = (y * width + x) * 4;
          pixels.set([...color, 255], offset);
        }
      }
      await presenter.present(width, height, pixels);
      if (generation === displayGeneration) {
        result.textContent =
          presenter.kind === "webgpu" ? "WebGPU renderer active" : "Canvas 2D fallback active";
      }
    } catch {
      // A stale asynchronous attempt must never tear down its successor.
      presenter?.dispose();
      if (presenter && displayPresenter === presenter) displayPresenter = undefined;
      if (generation === displayGeneration) result.textContent = "Display check failed.";
    } finally {
      if (generation === displayGeneration) action.disabled = false;
    }
  })();
});

type VmState = "idle" | "loading" | "running" | "error";
let state: VmState = "idle";
let worker: Worker | undefined;
function setState(next: VmState, description: string): void {
  state = next;
  const labels: Record<VmState, string> = {
    idle: "●  Stopped",
    loading: "●  Starting",
    running: "●  Running",
    error: "●  Error",
  };
  vmStatus.dataset.state = next;
  vmStatus.textContent = labels[next];
  message.textContent = description;
  startButton.disabled = next === "loading" || next === "running";
  stopButton.disabled = next !== "loading" && next !== "running";
}

function stopWorker(): void {
  worker?.terminate();
  worker = undefined;
}

function sendInput(text: string): void {
  if (state !== "running" || !worker || !text) return;
  const encoded = new TextEncoder().encode(text);
  // Large pastes are rejected rather than flooding the emulator's bounded input queue.
  if (encoded.length > MAX_INPUT_BYTES) {
    message.textContent = `Input is limited to ${MAX_INPUT_BYTES} bytes per paste.`;
    return;
  }
  worker.postMessage({ type: "input", bytes: encoded });
}
terminal.onData(sendInput);

startButton.addEventListener("click", () => {
  if (state === "loading" || state === "running") return;
  if (!capabilities.webAssembly || !capabilities.worker || !globalThis.isSecureContext) {
    setState("error", "A secure context, WebAssembly and Web Workers are required.");
    return;
  }
  stopWorker();
  terminal.clear();
  terminal.writeln("\x1b[90mStarting an isolated, non-persistent diagnostic guest…\x1b[0m");
  setState("loading", "Loading and verifying diagnostic assets. This may take a moment.");
  try {
    const active = new Worker(new URL("./guest.worker.ts", import.meta.url), { type: "module" });
    worker = active;
    active.onmessage = (event: MessageEvent<unknown>) => {
      if (worker !== active) return;
      const response = parseGuestEvent(event.data);
      if (!response) {
        stopWorker();
        setState("error", "Rejected invalid response from diagnostic worker.");
      } else if (response.type === "output") {
        terminal.write(response.bytes, () => {
          if (worker === active) active.postMessage({ type: "output-consumed" });
        });
      } else if (response.type === "running") {
        setState("running", "Emulator running. Wait for the guest console prompt.");
        terminal.focus();
      } else if (response.type === "error") {
        stopWorker();
        terminal.writeln(`\r\n\x1b[31mDiagnostic error: ${response.message}\x1b[0m`);
        setState("error", response.message);
      }
    };
    active.onerror = () => {
      if (worker !== active) return;
      stopWorker();
      setState("error", "Diagnostic worker failed unexpectedly.");
    };
    active.postMessage({ type: "start" });
  } catch (error) {
    stopWorker();
    setState(
      "error",
      error instanceof Error ? error.message : "Could not start diagnostic worker.",
    );
  }
});

stopButton.addEventListener("click", () => {
  stopWorker();
  terminal.writeln("\r\n\x1b[90mDiagnostic stopped. All guest changes were discarded.\x1b[0m");
  setState("idle", "Diagnostic stopped. All guest changes were discarded.");
});

button("copy").addEventListener("click", () => {
  void (async () => {
    const selected = terminal.getSelection();
    if (!selected) {
      message.textContent = "Select text in the console before copying.";
      return;
    }
    try {
      await navigator.clipboard.writeText(selected);
      message.textContent = "Terminal selection copied.";
    } catch {
      message.textContent = "Clipboard copy was denied or is unavailable.";
    }
  })();
});
button("paste").addEventListener("click", () => {
  void (async () => {
    if (state !== "running") {
      message.textContent = "Start the diagnostic before pasting.";
      return;
    }
    try {
      sendInput(await navigator.clipboard.readText());
      terminal.focus();
    } catch {
      message.textContent = "Clipboard paste was denied or is unavailable.";
    }
  })();
});

const resizeObserver = new ResizeObserver(() => fit.fit());
resizeObserver.observe(element("terminal"));
window.addEventListener("pagehide", (event) => {
  stopWorker();
  displayGeneration++;
  displayPresenter?.dispose();
  displayPresenter = undefined;
  button("test-display").disabled = false;
  element("display-test").hidden = true;
  if (event.persisted) {
    terminal.writeln("\r\n\x1b[90mPage suspended. The temporary guest was discarded.\x1b[0m");
    setState("idle", "Page suspended. The temporary guest was discarded.");
  } else {
    resizeObserver.disconnect();
    terminal.dispose();
  }
});
window.addEventListener("pageshow", (event) => {
  if (event.persisted) fit.fit();
});
