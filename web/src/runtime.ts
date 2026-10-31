export interface Exports {
  memory: WebAssembly.Memory;
  wnx_abi_version(): number;
  wnx_alloc(n: number): number;
  wnx_free(p: number, n: number): void;
  wnx_create(
    ram: number,
    sectors: number,
    fw: number,
    fl: number,
    k: number,
    kl: number,
    i: number,
    il: number,
    c: number,
    cl: number,
  ): number;
  wnx_destroy(p: number): void;
  wnx_step(p: number, budget: number): bigint;
  wnx_input(p: number, data: number, len: number): number;
  wnx_output(p: number, data: number, cap: number): number;
  wnx_powered_off(p: number): number;
  wnx_instret(p: number): number;
  wnx_framebuffer(p: number, data: number, cap: number): number;
  wnx_snapshot_size(p: number): bigint;
  wnx_snapshot_save(p: number, data: number, cap: number): bigint;
  wnx_snapshot_load(p: number, data: number, len: number): number;
  wnx_error(p: number, data: number, cap: number): number;
}
export interface Host {
  read(offset: number, p: number, n: number): void;
  write(offset: number, p: number, n: number): void;
  flush(): void;
}
export class Runtime {
  private handle = 0;
  readonly exports: Exports;
  private constructor(
    instance: WebAssembly.Instance,
    private host: Host,
  ) {
    this.exports = instance.exports as unknown as Exports;
  }
  static async create(
    host: Host,
    assets: {
      firmware: Uint8Array;
      kernel: Uint8Array;
      initrd: Uint8Array;
      cmdline: string;
      sectors: number;
    },
  ): Promise<Runtime> {
    const holder: { runtime?: Runtime } = {};
    const imports: WebAssembly.Imports = {
      env: { wnx_wall_time_ns: () => Math.floor((performance.timeOrigin + performance.now()) * 1e6) },
      wnx: {
        disk_read: (o: number, p: number, n: number) => {
          try {
            holder.runtime!.host.read(o, p, n);
            return 0;
          } catch {
            return -1;
          }
        },
        disk_write: (o: number, p: number, n: number) => {
          try {
            holder.runtime!.host.write(o, p, n);
            return 0;
          } catch {
            return -1;
          }
        },
        disk_flush: () => {
          try {
            holder.runtime!.host.flush();
            return 0;
          } catch {
            return -1;
          }
        },
        entropy: (p: number, n: number) => {
          try {
            crypto.getRandomValues(new Uint8Array(holder.runtime!.exports.memory.buffer, p, n));
            return 0;
          } catch {
            return -1;
          }
        },
      },
    };
    const { instance } = await WebAssembly.instantiateStreaming(
      fetch(new URL('../assets/wasmnux_runtime.wasm', import.meta.url)),
      imports,
    );
    const runtime = new Runtime(instance, host);
    holder.runtime = runtime;
    const e = runtime.exports;
    if (e.wnx_abi_version() !== 1) throw new Error('Runtime ABI mismatch');
    const allocations: Array<[number, number]> = [];
    const copy = (b: Uint8Array): number => {
      const p = e.wnx_alloc(b.length);
      if (!p) throw new Error('WASM allocation failed');
      new Uint8Array(e.memory.buffer, p, b.length).set(b);
      allocations.push([p, b.length]);
      return p;
    };
    const fw = copy(assets.firmware),
      k = copy(assets.kernel),
      i = copy(assets.initrd),
      c = copy(new TextEncoder().encode(assets.cmdline));
    runtime.handle = e.wnx_create(
      256,
      assets.sectors,
      fw,
      assets.firmware.length,
      k,
      assets.kernel.length,
      i,
      assets.initrd.length,
      c,
      allocations[3]![1],
    );
    for (const [p, n] of allocations) e.wnx_free(p, n);
    if (!runtime.handle) {
      runtime.dispose();
      throw new Error('Invalid VM assets or configuration');
    }
    return runtime;
  }
  step(budget = 2_000_000): bigint {
    const n = this.exports.wnx_step(this.handle, budget);
    if (n < 0n) throw new Error(this.error());
    return n;
  }
  input(bytes: Uint8Array): void {
    const e = this.exports,
      p = e.wnx_alloc(bytes.length);
    try {
      new Uint8Array(e.memory.buffer, p, bytes.length).set(bytes);
      if (e.wnx_input(this.handle, p, bytes.length)) throw new Error(this.error());
    } finally {
      e.wnx_free(p, bytes.length);
    }
  }
  output(): Uint8Array {
    const e = this.exports,
      p = e.wnx_alloc(65536);
    try {
      const n = e.wnx_output(this.handle, p, 65536);
      if (n < 0) throw new Error('Serial output capacity exceeded');
      return new Uint8Array(e.memory.buffer, p, n).slice();
    } finally {
      e.wnx_free(p, 65536);
    }
  }
  error(): string {
    const e = this.exports,
      p = e.wnx_alloc(4096);
    try {
      const n = e.wnx_error(this.handle, p, 4096);
      return n > 0
        ? new TextDecoder().decode(new Uint8Array(e.memory.buffer, p, n))
        : 'Runtime trapped or invalid handle';
    } finally {
      e.wnx_free(p, 4096);
    }
  }
  get poweredOff(): boolean {
    return this.exports.wnx_powered_off(this.handle) === 1;
  }
  snapshot(): Uint8Array {
    const e = this.exports;
    const n = Number(e.wnx_snapshot_size(this.handle));
    if (n <= 0 || n > 512 * 1024 * 1024) throw new Error(this.error());
    const p = e.wnx_alloc(n);
    try {
      const written = Number(e.wnx_snapshot_save(this.handle, p, n));
      if (written !== n) throw new Error(this.error());
      return new Uint8Array(e.memory.buffer, p, n).slice();
    } finally {
      e.wnx_free(p, n);
    }
  }
  restore(state: Uint8Array): void {
    const e = this.exports;
    const p = e.wnx_alloc(state.length);
    try {
      new Uint8Array(e.memory.buffer, p, state.length).set(state);
      if (e.wnx_snapshot_load(this.handle, p, state.length)) throw new Error(this.error());
    } finally {
      e.wnx_free(p, state.length);
    }
  }
  framebuffer(): Uint8Array {
    const e = this.exports,
      p = e.wnx_alloc(640 * 400 * 4);
    try {
      const n = e.wnx_framebuffer(this.handle, p, 640 * 400 * 4);
      if (n !== 640 * 400 * 4) throw new Error('Framebuffer read failed');
      return new Uint8Array(e.memory.buffer, p, n).slice();
    } finally {
      e.wnx_free(p, 640 * 400 * 4);
    }
  }
  get retired(): number {
    return this.exports.wnx_instret(this.handle);
  }
  dispose(): void {
    if (this.handle) this.exports.wnx_destroy(this.handle);
    this.handle = 0;
  }
}
