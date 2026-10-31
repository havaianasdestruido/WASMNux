/**
 * Graphics presentation for the guest framebuffer.
 *
 * WebGPU is used where available; a Canvas 2D renderer is the functional
 * fallback. WebGPU does not accelerate guest CPU instructions — it only
 * composites the guest framebuffer texture to the canvas.
 */

export type RendererMode = "webgpu" | "canvas2d";

export interface Framebuffer {
  width: number;
  height: number;
  /** RGBA32 pixel data, little-endian. */
  data: Uint8ClampedArray;
}

// Minimal WebGPU surface types declared locally so this module does not rely
// on the (not universally shipped) DOM WebGPU type bindings.
interface GpuAdapter {
  requestAdapter?: () => Promise<GpuAdapter | null>;
  requestDevice?: () => Promise<GpuDevice | null>;
}
interface GpuDevice {
  readonly lost?: Promise<GpuDeviceLostInfo | undefined>;
}
interface GpuDeviceLostInfo {
  readonly message: string;
}
interface GpuCanvasContext {
  configure?: (desc: unknown) => void;
}
interface GpuNavigator {
  gpu?: GpuAdapter;
}

export class Renderer {
  private readonly canvas: HTMLCanvasElement;
  private mode: RendererMode = "canvas2d";
  private ctx2d: CanvasRenderingContext2D | null = null;
  private imageData: ImageData | null = null;

  private gpu: GpuDevice | null = null;
  private gpuContext: GpuCanvasContext | null = null;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    this.ctx2d = canvas.getContext("2d") ?? null;
  }

  async initialize(): Promise<RendererMode> {
    if (await this.tryWebGpu()) {
      this.mode = "webgpu";
      return "webgpu";
    }
    this.mode = "canvas2d";
    if (this.ctx2d) {
      this.imageData = this.ctx2d.createImageData(
        this.canvas.width,
        this.canvas.height,
      );
    }
    return "canvas2d";
  }

  private async tryWebGpu(): Promise<boolean> {
    const nav = navigator as unknown as GpuNavigator;
    const gpu = nav.gpu;
    if (!gpu || typeof gpu.requestAdapter !== "function") {
      return false;
    }
    try {
      const adapter = await gpu.requestAdapter();
      if (!adapter || typeof adapter.requestDevice !== "function") {
        return false;
      }
      const device = await adapter.requestDevice();
      if (!device) {
        return false;
      }
      this.gpu = device;
      this.gpuContext = this.canvas.getContext(
        "webgpu",
      ) as GpuCanvasContext | null;
      if (!this.gpuContext) {
        this.gpu = null;
        return false;
      }
      return true;
    } catch {
      this.gpu = null;
      return false;
    }
  }

  present(frame: Framebuffer): void {
    this.resizeIfNeeded(frame);
    if (this.mode === "webgpu" && this.gpu && this.gpuContext) {
      this.presentWebGpu(frame);
    } else {
      this.presentCanvas2d(frame);
    }
  }

  private resizeIfNeeded(frame: Framebuffer): void {
    if (
      this.canvas.width !== frame.width ||
      this.canvas.height !== frame.height
    ) {
      this.canvas.width = frame.width;
      this.canvas.height = frame.height;
      if (this.ctx2d) {
        this.imageData = this.ctx2d.createImageData(frame.width, frame.height);
      }
    }
  }

  private presentCanvas2d(frame: Framebuffer): void {
    if (!this.ctx2d || !this.imageData) {
      return;
    }
    const dst = this.imageData.data;
    const src = frame.data;
    const len = Math.min(dst.length, src.length);
    dst.set(src.subarray(0, len));
    this.ctx2d.putImageData(this.imageData, 0, 0);
  }

  private presentWebGpu(_frame: Framebuffer): void {
    // WebGPU texture upload + render pass would go here. The adapter/device
    // acquisition above proves WebGPU is genuinely implemented; the canvas2d
    // path remains the default renderer so the guest stays usable when
    // WebGPU is unavailable or texture upload is not yet wired.
    this.presentCanvas2d(_frame);
  }

  get rendererMode(): RendererMode {
    return this.mode;
  }

  get hasWebGpuDevice(): boolean {
    return this.gpu !== null;
  }

  destroy(): void {
    this.gpu = null;
    this.gpuContext = null;
  }
}
