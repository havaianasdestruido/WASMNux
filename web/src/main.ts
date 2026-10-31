import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { FramebufferRenderer } from './renderer';
import { importProfile, resetProfile, exportProfile, DISK_BYTES } from './storage-admin';
import type { Profile } from './protocol';
import type { WorkerRequest, WorkerResponse } from './protocol';
import './style.css';
import '@xterm/xterm/css/xterm.css';
function el(id: string): HTMLElement {
  const e = document.getElementById(id);
  if (!e) throw new Error(`Missing #${id}`);
  return e;
}
const status = el('status'),
  terminal = new Terminal({
    convertEol: true,
    cursorBlink: true,
    fontFamily: 'monospace',
    theme: { background: '#0b0f12' },
  });
const renderer = new FramebufferRenderer(el('display') as HTMLCanvasElement, (kind, reason) => {
  el('graphics').textContent =
    kind === 'webgpu'
      ? 'WebGPU presenting guest framebuffer'
      : `Canvas 2D fallback${reason ? `: ${reason}` : ''}`;
});
void renderer.init();
const fit = new FitAddon();
terminal.loadAddon(fit);
terminal.open(el('terminal'));
fit.fit();
const bootButton = el('boot') as HTMLButtonElement,
  pauseButton = el('pause') as HTMLButtonElement,
  shutdownButton = el('shutdown') as HTMLButtonElement,
  exportButton = el('export') as HTMLButtonElement;
let worker: Worker | undefined,
  running = false,
  paused = false;
let snapshotList: Array<{ id: string; profile: string; created: number; bytes: number }> = [];
const encoder = new TextEncoder();
function send(message: WorkerRequest, transfer: Transferable[] = []): void {
  worker?.postMessage(message, transfer);
}
function setRunning(value: boolean): void {
  running = value;
  bootButton.disabled = value;
  pauseButton.disabled = !value;
  shutdownButton.disabled = !value;
  exportButton.disabled = false;
  (el('snapshot') as HTMLButtonElement).disabled = !value;
  (el('snapshots') as HTMLSelectElement).disabled = !value;
  (el('restore') as HTMLButtonElement).disabled = !value;
  (el('delete-snapshot') as HTMLButtonElement).disabled = !value;
  (el('profile') as HTMLSelectElement).disabled = value;
}
function stop(): void {
  worker?.terminate();
  worker = undefined;
  setRunning(false);
}
function start(): void {
  stop();
  terminal.clear();
  worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
  worker.onmessage = (event: MessageEvent<WorkerResponse>) => {
    const m = event.data;
    switch (m.type) {
      case 'status':
        status.textContent = m.text;
        break;
      case 'ready':
        status.textContent = 'Guest running';
        setRunning(true);
        send({ type: 'listSnapshots' });
        break;
      case 'output':
        terminal.write(m.bytes);
        break;
      case 'frame': {
        renderer.render(m.bytes);
        let sum = 0;
        for (let i = 0; i < m.bytes.length; i += 4096) sum = (sum + m.bytes[i]!) >>> 0;
        el('display').dataset.frameChecksum = String(sum);
        break;
      }
      case 'stats':
        el('retired').textContent = m.retired.toLocaleString();
        el('slice').textContent = `${m.wallMs.toFixed(1)} ms`;
        if (m.poweredOff) {
          status.textContent = 'Guest powered off';
          stop();
        }
        break;
      case 'error':
        status.textContent = m.message;
        terminal.writeln(`\r\n[WASMNux] ${m.message}`);
        stop();
        break;
      case 'snapshots': {
        snapshotList = m.snapshots;
        const select = el('snapshots');
        select.replaceChildren();
        for (const snap of snapshotList) {
          const opt = document.createElement('option');
          opt.value = snap.id;
          opt.textContent = `${new Date(snap.created).toLocaleString()} · ${(snap.bytes / 1048576).toFixed(1)} MiB`;
          select.append(opt);
        }
        break;
      }
      case 'exported': {
        const url = URL.createObjectURL(
          new Blob([m.bytes.buffer as ArrayBuffer], { type: 'application/octet-stream' }),
        );
        const a = document.createElement('a');
        a.href = url;
        a.download = 'wasmnux-disk.img';
        a.click();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
        break;
      }
    }
  };
  worker.onerror = () => {
    status.textContent = 'Worker failed; reload to recover. Journal will recover on next boot.';
    stop();
  };
  setRunning(true);
  status.textContent = 'Starting worker…';
  send({
    type: 'boot',
    profile: (el('profile') as HTMLSelectElement).value === 'debian' ? 'debian' : 'vanilla',
  });
}
bootButton.onclick = start;
pauseButton.onclick = () => {
  paused = !paused;
  send({ type: 'pause', paused });
  pauseButton.textContent = paused ? 'Resume' : 'Pause';
};
shutdownButton.onclick = () => {
  status.textContent = 'Requesting guest shutdown…';
  send({ type: 'shutdown' });
};
terminal.onData((data) => {
  if (running) send({ type: 'input', bytes: encoder.encode(data) });
});
function selectedProfile(): Profile {
  return (el('profile') as HTMLSelectElement).value === 'debian' ? 'debian' : 'vanilla';
}
function download(bytes: Uint8Array, name: string): void {
  const url = URL.createObjectURL(
    new Blob([bytes.buffer as ArrayBuffer], { type: 'application/octet-stream' }),
  );
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
(el('import') as HTMLInputElement).onchange = async (event) => {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;
  if (running) {
    status.textContent = 'Stop the VM before importing';
    input.value = '';
    return;
  }
  const profile = selectedProfile();
  if (file.size !== DISK_BYTES[profile]) {
    status.textContent = `Import must be an exact ${DISK_BYTES[profile] / 1048576} MiB raw image`;
    input.value = '';
    return;
  }
  if (!confirm(`Replace the ${profile} disk with this import? Existing profile data will be overwritten.`)) {
    input.value = '';
    return;
  }
  try {
    status.textContent = 'Validating and importing disk…';
    await importProfile(profile, new Uint8Array(await file.arrayBuffer()));
    status.textContent = 'Disk imported; boot to verify';
  } catch (e) {
    status.textContent = e instanceof Error ? e.message : String(e);
  }
  input.value = '';
};
el('reset').onclick = () => {
  void (async () => {
    if (running) {
      status.textContent = 'Stop the VM before resetting';
      return;
    }
    const profile = selectedProfile();
    if (!confirm(`Reset ${profile}? This deletes its disk and journal. The other profile is untouched.`))
      return;
    try {
      await resetProfile(profile);
      status.textContent = `${profile} reset; next boot recreates its disk`;
    } catch (e) {
      status.textContent = e instanceof Error ? e.message : String(e);
    }
  })();
};
el('snapshot').onclick = () => send({ type: 'snapshot' });
el('restore').onclick = () => {
  const id = (el('snapshots') as HTMLSelectElement).value;
  if (!id) {
    status.textContent = 'Create a snapshot first';
    return;
  }
  if (confirm('Restore this snapshot? The current disk must match the snapshot disk.'))
    send({ type: 'restore', id });
};
el('delete-snapshot').onclick = () => {
  const id = (el('snapshots') as HTMLSelectElement).value;
  if (id && confirm('Delete this snapshot?')) send({ type: 'deleteSnapshot', id });
};
exportButton.onclick = () => {
  void (async () => {
    if (running) {
      send({ type: 'export' });
      return;
    }
    try {
      status.textContent = 'Exporting disk…';
      download(await exportProfile(selectedProfile()), `wasmnux-${selectedProfile()}.img`);
      status.textContent = 'Disk exported';
    } catch (e) {
      status.textContent = e instanceof Error ? e.message : String(e);
    }
  })();
};
el('persist').onclick = () => {
  void navigator.storage?.persist?.().then((ok) => {
    status.textContent = ok ? 'Persistent storage granted' : 'Storage remains best-effort; export backups';
  });
};
const resize = new ResizeObserver(() => fit.fit());
resize.observe(el('terminal'));
window.addEventListener('pagehide', () => {
  renderer.dispose();
  send({ type: 'shutdown' });
  resize.disconnect();
  terminal.dispose();
});
void navigator.storage?.estimate?.().then((e) => {
  el('storage').textContent = `OPFS journal · ${((e.usage ?? 0) / 1048576).toFixed(1)} MiB used`;
});
el('capabilities').textContent = [
  'object' === typeof WebAssembly ? 'WASM' : 'WASM missing',
  'gpu' in navigator ? 'WebGPU available' : 'Canvas fallback',
  typeof navigator.storage?.getDirectory === 'function' ? 'OPFS' : 'OPFS missing',
].join(' · ');
if ('serviceWorker' in navigator)
  navigator.serviceWorker.register('./sw.js').catch(() => {
    status.textContent = 'Offline cache registration failed';
  });
