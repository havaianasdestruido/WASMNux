/**
 * Interactive UART terminal controller wrapping `@xterm/xterm` + `@xterm/addon-fit`.
 *
 * Connects user keystrokes directly to the guest NS16550A serial input stream (zero command faking),
 * paces multi-line clipboard pastes in chunks so the guest 8250 TTY RX buffer never overflows,
 * and persists user theme & font-size preferences in `PersistentStorageEngine`.
 */

import { FitAddon } from '@xterm/addon-fit';
import { Terminal, type ITheme } from '@xterm/xterm';

export type TerminalThemeId = 'midnight' | 'solarized' | 'amber';

export interface TerminalPreferences {
  readonly fontSize: number;
  readonly theme: TerminalThemeId;
}

export const DEFAULT_TERMINAL_PREFERENCES: TerminalPreferences = {
  fontSize: 14,
  theme: 'midnight',
};

export function isTerminalPreferences(val: unknown): val is TerminalPreferences {
  if (typeof val !== 'object' || val === null) {
    return false;
  }
  const rec = val as Record<string, unknown>;
  return (
    typeof rec['fontSize'] === 'number' &&
    rec['fontSize'] >= 10 &&
    rec['fontSize'] <= 26 &&
    (rec['theme'] === 'midnight' || rec['theme'] === 'solarized' || rec['theme'] === 'amber')
  );
}

const TERMINAL_THEMES: Readonly<Record<TerminalThemeId, ITheme>> = {
  midnight: {
    background: '#090d16',
    foreground: '#e2e8f0',
    cursor: '#38bdf8',
    selectionBackground: 'rgba(56, 189, 248, 0.28)',
    black: '#0f172a',
    red: '#f87171',
    green: '#34d399',
    yellow: '#fbbf24',
    blue: '#60a5fa',
    magenta: '#c084fc',
    cyan: '#22d3ee',
    white: '#f1f5f9',
  },
  solarized: {
    background: '#002b36',
    foreground: '#93a1a1',
    cursor: '#2aa198',
    selectionBackground: 'rgba(42, 161, 152, 0.30)',
    black: '#073642',
    red: '#dc322f',
    green: '#859900',
    yellow: '#b58900',
    blue: '#268bd2',
    magenta: '#d33682',
    cyan: '#2aa198',
    white: '#eee8d5',
  },
  amber: {
    background: '#120d05',
    foreground: '#fbbf24',
    cursor: '#f59e0b',
    selectionBackground: 'rgba(245, 158, 11, 0.28)',
    black: '#1c1408',
    red: '#ef4444',
    green: '#f59e0b',
    yellow: '#fde047',
    blue: '#fb923c',
    magenta: '#f97316',
    cyan: '#fcd34d',
    white: '#fef3c7',
  },
};

const PASTE_CHUNK_BYTES = 64;
const PASTE_CHUNK_DELAY_MS = 10;

/**
 * Split multi-line pasted text into bounded UTF-8 slices for safe UART RX FIFO pacing.
 */
export function chunkTextForUartPaste(
  text: string,
  maxChunkBytes = PASTE_CHUNK_BYTES,
): Uint8Array[] {
  const normalized = text.replace(/\r\n/g, '\n');
  const encoded = new TextEncoder().encode(normalized);
  if (encoded.byteLength === 0) {
    return [];
  }
  const chunks: Uint8Array[] = [];
  for (let offset = 0; offset < encoded.byteLength; offset += maxChunkBytes) {
    chunks.push(encoded.slice(offset, Math.min(encoded.byteLength, offset + maxChunkBytes)));
  }
  return chunks;
}

export interface TerminalControllerOptions {
  readonly container: HTMLElement;
  readonly initialPreferences?: TerminalPreferences;
  readonly onInputBytes: (bytes: Uint8Array) => void;
  readonly onDimensionsChanged?: (cols: number, rows: number) => void;
}

export class TerminalController {
  private readonly term: Terminal;
  private readonly fitAddon: FitAddon;
  private readonly onInputBytes: (bytes: Uint8Array) => void;
  private readonly onDimensionsChanged: ((cols: number, rows: number) => void) | undefined;
  private resizeObserver: ResizeObserver | null = null;
  private prefs: TerminalPreferences;
  private disposed = false;

  public constructor(opts: TerminalControllerOptions) {
    this.prefs = opts.initialPreferences ?? DEFAULT_TERMINAL_PREFERENCES;
    this.onInputBytes = opts.onInputBytes;
    this.onDimensionsChanged = opts.onDimensionsChanged;

    this.term = new Terminal({
      cursorBlink: true,
      convertEol: false,
      scrollback: 5000,
      fontFamily:
        '"JetBrains Mono", "Fira Code", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
      fontSize: this.prefs.fontSize,
      theme: TERMINAL_THEMES[this.prefs.theme],
      allowProposedApi: false,
    });
    this.fitAddon = new FitAddon();
    this.term.loadAddon(this.fitAddon);
    this.term.open(opts.container);

    const encoder = new TextEncoder();
    this.term.onData((data) => {
      if (!this.disposed) {
        this.onInputBytes(encoder.encode(data));
      }
    });

    this.fit();
    if (typeof ResizeObserver !== 'undefined') {
      this.resizeObserver = new ResizeObserver(() => {
        this.fit();
      });
      this.resizeObserver.observe(opts.container);
    }
  }

  public get dimensions(): { readonly cols: number; readonly rows: number } {
    return { cols: this.term.cols, rows: this.term.rows };
  }

  public get preferences(): TerminalPreferences {
    return this.prefs;
  }

  public writeOutput(bytes: Uint8Array | string): void {
    if (this.disposed) {
      return;
    }
    this.term.write(bytes);
  }

  public writeSystemNotice(line: string): void {
    if (this.disposed) {
      return;
    }
    this.term.writeln(`\x1b[90m${line}\x1b[0m`);
  }

  public clear(): void {
    this.term.clear();
  }

  public focus(): void {
    this.term.focus();
  }

  public fit(): void {
    if (this.disposed) {
      return;
    }
    try {
      this.fitAddon.fit();
      this.onDimensionsChanged?.(this.term.cols, this.term.rows);
    } catch {
      // Ignore layout-zero transient states
    }
  }

  public setPreferences(next: TerminalPreferences): void {
    const clampedSize = Math.max(10, Math.min(26, Math.round(next.fontSize)));
    this.prefs = { fontSize: clampedSize, theme: next.theme };
    this.term.options.fontSize = clampedSize;
    this.term.options.theme = TERMINAL_THEMES[next.theme];
    this.fit();
  }

  public getSelectionText(): string {
    return this.term.getSelection();
  }

  public async pasteWithFlowControl(text: string): Promise<void> {
    const chunks = chunkTextForUartPaste(text, PASTE_CHUNK_BYTES);
    for (let i = 0; i < chunks.length; i += 1) {
      if (this.disposed) {
        break;
      }
      const chunk = chunks[i];
      if (chunk !== undefined) {
        this.onInputBytes(chunk);
      }
      if (i + 1 < chunks.length) {
        await new Promise((r) => setTimeout(r, PASTE_CHUNK_DELAY_MS));
      }
    }
  }

  public sendGuestTtyResizeCommand(): void {
    const { cols, rows } = this.dimensions;
    const cmd = `stty cols ${String(cols)} rows ${String(rows)} 2>/dev/null\n`;
    this.onInputBytes(new TextEncoder().encode(cmd));
  }

  public dispose(): void {
    this.disposed = true;
    this.resizeObserver?.disconnect();
    this.resizeObserver = null;
    this.term.dispose();
  }
}
