/**
 * The browser terminal: an xterm.js view onto the guest NS16550 UART byte
 * stream. Bytes in both directions go to/from the VM worker unchanged —
 * no command is ever interpreted or answered by TypeScript.
 */

import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";

export class GuestTerminal {
  readonly #terminal: Terminal;
  readonly #fit: FitAddon;
  readonly #resizeObserver: ResizeObserver;
  readonly #disposables: Array<{ dispose(): void }> = [];
  readonly #decoder = new TextDecoder();
  #inputHandler: ((bytes: Uint8Array) => void) | undefined;

  constructor(container: HTMLElement) {
    this.#terminal = new Terminal({
      convertEol: false,
      cursorBlink: true,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      fontSize: 13,
      scrollback: 5000,
      theme: {
        background: "#0b0f14",
        foreground: "#d6deeb",
        cursor: "#7ee787",
      },
    });
    this.#fit = new FitAddon();
    this.#terminal.loadAddon(this.#fit);
    this.#terminal.open(container);
    this.#fit.fit();

    const encoder = new TextEncoder();
    this.#disposables.push(
      this.#terminal.onData((data) => {
        this.#inputHandler?.(encoder.encode(data));
      }),
    );
    this.#disposables.push(
      this.#terminal.onBinary((data) => {
        const bytes = new Uint8Array(data.length);
        for (let i = 0; i < data.length; i += 1) {
          bytes[i] = data.charCodeAt(i) & 0xff;
        }
        this.#inputHandler?.(bytes);
      }),
    );

    this.#resizeObserver = new ResizeObserver(() => {
      try {
        this.#fit.fit();
      } catch {
        // Container detached mid-resize; the next observation will refit.
      }
    });
    this.#resizeObserver.observe(container);
  }

  /** Registers where keystrokes/paste go (the VM's UART input queue). */
  onInput(handler: (bytes: Uint8Array) => void): void {
    this.#inputHandler = handler;
  }

  /** Writes raw guest UART output (UTF-8/ANSI handled by xterm). */
  writeGuestBytes(bytes: Uint8Array): void {
    this.#terminal.write(bytes);
  }

  /** Writes a host-side notice line, visually marked as NOT guest output. */
  writeHostNotice(text: string): void {
    this.#terminal.write(`\r\n\x1b[33m[wasmnux host]\x1b[0m ${this.#decoder.decode(new TextEncoder().encode(text))}\r\n`);
  }

  clear(): void {
    this.#terminal.clear();
  }

  focus(): void {
    this.#terminal.focus();
  }

  get dimensions(): { cols: number; rows: number } {
    return { cols: this.#terminal.cols, rows: this.#terminal.rows };
  }

  dispose(): void {
    this.#resizeObserver.disconnect();
    for (const disposable of this.#disposables) {
      disposable.dispose();
    }
    this.#terminal.dispose();
  }
}
