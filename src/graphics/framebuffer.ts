/** RGBA framebuffer presentation only. This is not GPU passthrough. */
export type RendererKind = "webgpu" | "canvas2d";
export interface PresenterOptions {
  forceCanvas?: boolean;
  onBackendChange?: (kind: RendererKind, reason: string) => void;
}

const MAX_DIMENSION = 4096;
const MAX_FRAME_BYTES = 64 * 1024 * 1024;
const SHADER = `
@group(0) @binding(0) var framebuffer: texture_2d<f32>;
@vertex fn vertex_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
  let positions = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
  return vec4f(positions[index], 0.0, 1.0);
}
@fragment fn fragment_main(@builtin(position) position: vec4f) -> @location(0) vec4f {
  return textureLoad(framebuffer, vec2i(position.xy), 0);
}
`;

interface GpuBackend {
  device: GPUDevice;
  context: GPUCanvasContext;
  pipeline: GPURenderPipeline;
  texture?: GPUTexture;
  bindGroup?: GPUBindGroup;
  width: number;
  height: number;
}

interface Frame {
  width: number;
  height: number;
  bytes: Uint8Array<ArrayBuffer>;
}

function validateFrame(width: number, height: number, bytes: Uint8Array): void {
  if (
    !Number.isInteger(width) ||
    !Number.isInteger(height) ||
    width < 1 ||
    height < 1 ||
    width > MAX_DIMENSION ||
    height > MAX_DIMENSION ||
    width * height * 4 > MAX_FRAME_BYTES ||
    !(bytes instanceof Uint8Array) ||
    bytes.byteLength !== width * height * 4
  ) {
    throw new Error("Invalid RGBA framebuffer dimensions or length");
  }
}

export class FramebufferPresenter {
  private canvasElement: HTMLCanvasElement;
  private gpu: GpuBackend | undefined;
  private context2d: CanvasRenderingContext2D | undefined;
  private lastFrame: Frame | undefined;
  private disposed = false;
  private currentKind: RendererKind = "canvas2d";

  private constructor(
    container: HTMLElement,
    private readonly options: PresenterOptions,
  ) {
    this.canvasElement = document.createElement("canvas");
    this.canvasElement.className = "framebuffer-canvas";
    this.canvasElement.setAttribute("aria-label", "Framebuffer output");
    container.append(this.canvasElement);
  }

  static async create(
    container: HTMLElement,
    options: PresenterOptions = {},
  ): Promise<FramebufferPresenter> {
    const presenter = new FramebufferPresenter(container, options);
    try {
      await presenter.initialize();
      return presenter;
    } catch (error) {
      presenter.dispose();
      throw error;
    }
  }

  get kind(): RendererKind {
    return this.currentKind;
  }

  get canvas(): HTMLCanvasElement {
    return this.canvasElement;
  }

  private async initialize(): Promise<void> {
    let device: GPUDevice | undefined;
    if (!this.options.forceCanvas && typeof navigator !== "undefined" && navigator.gpu) {
      try {
        const adapter = await navigator.gpu.requestAdapter();
        if (!adapter) throw new Error("No WebGPU adapter");
        device = await adapter.requestDevice();
        const module = device.createShaderModule({ code: SHADER });
        const diagnostics = await module.getCompilationInfo();
        if (diagnostics.messages.some((message) => message.type === "error"))
          throw new Error("Framebuffer shader compilation failed");
        const format = navigator.gpu.getPreferredCanvasFormat();
        const pipeline = await device.createRenderPipelineAsync({
          layout: "auto",
          vertex: { module, entryPoint: "vertex_main" },
          fragment: { module, entryPoint: "fragment_main", targets: [{ format }] },
          primitive: { topology: "triangle-list" },
        });
        const context = this.canvasElement.getContext("webgpu");
        if (!context) throw new Error("WebGPU canvas unavailable");
        context.configure({ device, format, alphaMode: "opaque" });
        this.gpu = { device, context, pipeline, width: 0, height: 0 };
        this.currentKind = "webgpu";
        device.addEventListener("uncapturederror", () =>
          this.fallback("WebGPU error; using Canvas 2D"),
        );
        void device.lost.then(
          () => this.fallback("WebGPU device lost; using Canvas 2D"),
          () => this.fallback("WebGPU device failure; using Canvas 2D"),
        );
        return;
      } catch {
        device?.destroy();
      }
    }
    this.installCanvas2d();
  }

  private installCanvas2d(): void {
    // A canvas that already has a WebGPU context cannot be reused for 2D.
    const replacement = document.createElement("canvas");
    replacement.className = this.canvasElement.className;
    replacement.setAttribute("aria-label", "Framebuffer output");
    this.canvasElement.replaceWith(replacement);
    this.canvasElement = replacement;
    const context = replacement.getContext("2d", { alpha: false });
    if (!context) throw new Error("Canvas 2D is unavailable");
    this.context2d = context;
    this.currentKind = "canvas2d";
  }

  private fallback(reason: string): void {
    if (this.disposed || this.currentKind !== "webgpu") return;
    this.gpu?.texture?.destroy();
    this.gpu?.context.unconfigure();
    this.gpu?.device.destroy();
    this.gpu = undefined;
    try {
      this.installCanvas2d();
      if (this.lastFrame) this.drawCanvas(this.lastFrame);
      this.options.onBackendChange?.("canvas2d", reason);
    } catch {
      this.dispose();
      this.options.onBackendChange?.("canvas2d", "Display recovery failed");
    }
  }

  private drawCanvas(frame: Frame): void {
    if (!this.context2d) throw new Error("Canvas renderer unavailable");
    this.canvasElement.width = frame.width;
    this.canvasElement.height = frame.height;
    this.context2d.putImageData(
      new ImageData(new Uint8ClampedArray(frame.bytes), frame.width, frame.height),
      0,
      0,
    );
  }

  async present(width: number, height: number, bytes: Uint8Array): Promise<void> {
    if (this.disposed) throw new Error("Framebuffer presenter is disposed");
    validateFrame(width, height, bytes);
    const frame: Frame = { width, height, bytes: bytes.slice() };
    this.lastFrame = frame;
    const gpu = this.gpu;
    if (!gpu) {
      this.drawCanvas(frame);
      return;
    }
    try {
      if (gpu.width !== width || gpu.height !== height || !gpu.texture || !gpu.bindGroup) {
        gpu.texture?.destroy();
        this.canvasElement.width = width;
        this.canvasElement.height = height;
        gpu.texture = gpu.device.createTexture({
          size: [width, height, 1],
          format: "rgba8unorm",
          usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST,
        });
        gpu.bindGroup = gpu.device.createBindGroup({
          layout: gpu.pipeline.getBindGroupLayout(0),
          entries: [{ binding: 0, resource: gpu.texture.createView() }],
        });
        gpu.width = width;
        gpu.height = height;
      }
      gpu.device.queue.writeTexture(
        { texture: gpu.texture },
        frame.bytes,
        { bytesPerRow: width * 4, rowsPerImage: height },
        [width, height, 1],
      );
      const encoder = gpu.device.createCommandEncoder();
      const pass = encoder.beginRenderPass({
        colorAttachments: [
          {
            view: gpu.context.getCurrentTexture().createView(),
            loadOp: "clear",
            storeOp: "store",
            clearValue: [0, 0, 0, 1],
          },
        ],
      });
      pass.setPipeline(gpu.pipeline);
      pass.setBindGroup(0, gpu.bindGroup);
      pass.draw(3);
      pass.end();
      gpu.device.queue.submit([encoder.finish()]);
      await gpu.device.queue.onSubmittedWorkDone();
    } catch {
      this.fallback("WebGPU rendering failed; using Canvas 2D");
      if (!this.context2d) throw new Error("Framebuffer rendering failed");
    }
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.gpu?.texture?.destroy();
    this.gpu?.context.unconfigure();
    this.gpu?.device.destroy();
    this.gpu = undefined;
    this.context2d = undefined;
    this.lastFrame = undefined;
    this.canvasElement.remove();
  }
}
