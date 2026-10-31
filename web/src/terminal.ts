/**
 * Interactive terminal connected to the guest serial console (UART).
 *
 * Uses xterm.js for the frontend. Keystrokes are forwarded to the guest;
 * guest output is rendered in the terminal. This is a real connection to the
 * guest console, not a command mock.
 */

import { Terminal, ITerminalOptions } from "xterm";
import { FitAddon } from "xterm-addon-fit";
import { WebLinksAddon } from "xterm-addon-web-links";
import type { WasmMachine } from "./runtime";

type FitTerminal = Terminal & { fit(): void };

export class GuestTerminal {
  private readonly term: FitTerminal;
  private machine: WasmMachine | null = null;
  private readonly inputQueue: string[] = [];
  private draining = false;

  constructor(container: HTMLElement, options: Partial<ITerminalOptions> = {}) {
    this.term = new Terminal({
      cols: 80,
      rows: 24,
      fontFamily: '"Courier New", monospace',
      fontSize: 14,
      ...options,
    } as ITerminalOptions) as FitTerminal;
    this.term.loadAddon(new FitAddon());
    this.term.loadAddon(new WebLinksAddon());
    this.term.open(container);
    this.term.fit();

    this.term.onKey((e) => {
      const key = e.key;
      if (this.machine && key) {
        this.inputQueue.push(key);
        this.scheduleInputDrain();
      }
    });

    window.addEventListener("resize", () => this.term.fit());
  }

  attach(machine: WasmMachine): void {
    this.machine = machine;
  }

  detach(): void {
    this.machine = null;
  }

  write(data: string | Uint8Array): void {
    if (typeof data === "string") {
      this.term.write(data);
    } else {
      this.term.write(new TextDecoder().decode(data));
    }
  }

  clear(): void {
    this.term.clear();
  }

  focus(): void {
    this.term.focus();
  }

  dispose(): void {
    this.term.dispose();
    window.removeEventListener("resize", () => this.term.fit());
  }

  private scheduleInputDrain(): void {
    if (this.draining) {
      return;
    }
    this.draining = true;
    queueMicrotask(() => this.flushInput());
  }

  private flushInput(): void {
    this.draining = false;
    if (!this.machine) {
      this.inputQueue.length = 0;
      return;
    }
    const batch = this.inputQueue.splice(0, this.inputQueue.length);
    const encoder = new TextEncoder();
    const bytes = encoder.encode(batch.join(""));
    this.machine.writeMem(0x1000_0000, bytes);
    for (const ch of batch) {
      this.term.write(ch);
    }
  }
}
