/**
 * Framebuffer presentation. The guest exposes an x8r8g8b8 framebuffer via
 * the `simple-framebuffer` DTB node; bytes in memory are therefore
 * [B, G, R, X] per pixel (little-endian 0xXXRRGGBB).
 *
 *  - WebGpuRenderer uploads frames to a texture and does the BGR→RGB
 *    swizzle + scaling in a WGSL fragment shader (genuine GPU work:
 *    texture upload, format conversion, presentation). It handles
 *    device loss and non-256-aligned strides.
 *  - Canvas2dRenderer is the always-available fallback so the guest stays
 *    usable without WebGPU.
 *
 * WebGPU here accelerates *presentation only*. It is not guest GPU
 * passthrough and the project never markets it as such.
 */

export interface FramebufferFrame {
  readonly width: number;
  readonly height: number;
  readonly stride: number;
  /** x8r8g8b8 pixels, `stride * height` bytes. */
  readonly pixels: Uint8Array;
}

export interface FramebufferRenderer {
  readonly kind: "webgpu" | "canvas2d";
  present(frame: FramebufferFrame): void;
  dispose(): void;
}

/**
 * Converts x8r8g8b8 (B,G,R,X bytes) to RGBA bytes for Canvas2D ImageData.
 * Pure function; unit-tested.
 */
export function swizzleXrgbToRgba(frame: FramebufferFrame, out?: Uint8ClampedArray): Uint8ClampedArray {
  const { width, height, stride, pixels } = frame;
  const result = out ?? new Uint8ClampedArray(width * height * 4);
  if (result.length !== width * height * 4) {
    throw new Error(`RGBA buffer must be ${width * height * 4} bytes, got ${result.length}`);
  }
  for (let y = 0; y < height; y += 1) {
    const srcRow = y * stride;
    const dstRow = y * width * 4;
    for (let x = 0; x < width; x += 1) {
      const s = srcRow + x * 4;
      const d = dstRow + x * 4;
      result[d] = pixels[s + 2] as number; // R
      result[d + 1] = pixels[s + 1] as number; // G
      result[d + 2] = pixels[s] as number; // B
      result[d + 3] = 0xff;
    }
  }
  return result;
}

/**
 * Deterministic colour-bar + gradient pattern in guest pixel format.
 * Used ONLY as a renderer self-test (the UI labels it as such); it never
 * pretends to be guest output.
 */
export function generateTestPattern(width: number, height: number): FramebufferFrame {
  const stride = width * 4;
  const pixels = new Uint8Array(stride * height);
  const bars: ReadonlyArray<readonly [number, number, number]> = [
    [255, 255, 255],
    [255, 255, 0],
    [0, 255, 255],
    [0, 255, 0],
    [255, 0, 255],
    [255, 0, 0],
    [0, 0, 255],
    [0, 0, 0],
  ];
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const offset = y * stride + x * 4;
      if (y < height / 2) {
        const bar = bars[Math.min(bars.length - 1, Math.floor((x / width) * bars.length))] as readonly [number, number, number];
        pixels[offset] = bar[2];
        pixels[offset + 1] = bar[1];
        pixels[offset + 2] = bar[0];
      } else {
        pixels[offset] = Math.floor((y / height) * 255);
        pixels[offset + 1] = Math.floor((x / width) * 255);
        pixels[offset + 2] = 128;
      }
      pixels[offset + 3] = 0xff;
    }
  }
  return { width, height, stride, pixels };
}

/** Canvas2D fallback renderer (works in every browser with a canvas). */
export class Canvas2dRenderer implements FramebufferRenderer {
  readonly kind = "canvas2d" as const;
  readonly #canvas: HTMLCanvasElement;
  readonly #ctx: CanvasRenderingContext2D;
  #scratch: HTMLCanvasElement | undefined;

  constructor(canvas: HTMLCanvasElement) {
    const ctx = canvas.getContext("2d");
    if (ctx === null) {
      throw new Error("Canvas 2D context unavailable");
    }
    this.#canvas = canvas;
    this.#ctx = ctx;
    this.#ctx.imageSmoothingEnabled = true;
  }

  present(frame: FramebufferFrame): void {
    if (this.#canvas.width !== frame.width || this.#canvas.height !== frame.height) {
      this.#canvas.width = frame.width;
      this.#canvas.height = frame.height;
    }
    const rgba = swizzleXrgbToRgba(frame);
    const image = new ImageData(rgba, frame.width, frame.height);
    if (this.#scratch === undefined) {
      this.#scratch = document.createElement("canvas");
    }
    this.#scratch.width = frame.width;
    this.#scratch.height = frame.height;
    const scratchCtx = this.#scratch.getContext("2d");
    if (scratchCtx === null) {
      throw new Error("scratch Canvas 2D context unavailable");
    }
    scratchCtx.putImageData(image, 0, 0);
    this.#ctx.drawImage(this.#scratch, 0, 0, frame.width, frame.height);
  }

  dispose(): void {
    this.#scratch = undefined;
  }
}

const WGSL = /* wgsl */ `
@group(0) @binding(0) var frameTexture: texture_2d<f32>;
@group(0) @binding(1) var frameSampler: sampler;

struct VertexOut {
  @builtin(position) position: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) index: u32) -> VertexOut {
  // Fullscreen triangle.
  var positions = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(3.0, -1.0),
    vec2<f32>(-1.0, 3.0),
  );
  var uvs = array<vec2<f32>, 3>(
    vec2<f32>(0.0, 1.0),
    vec2<f32>(2.0, 1.0),
    vec2<f32>(0.0, -1.0),
  );
  var out: VertexOut;
  out.position = vec4<f32>(positions[index], 0.0, 1.0);
  out.uv = uvs[index];
  return out;
}

@fragment
fn fs(input: VertexOut) -> @location(0) vec4<f32> {
  // Guest format x8r8g8b8 stored in rgba8unorm => channels hold (B,G,R,x).
  let texel = textureSample(frameTexture, frameSampler, input.uv);
  return vec4<f32>(texel.b, texel.g, texel.r, 1.0);
}`;

/**
 * WebGPU renderer. Create via {@link WebGpuRenderer.create}; holds a
 * device and a persistent texture resized to the frame geometry.
 */
export class WebGpuRenderer implements FramebufferRenderer {
  readonly kind = "webgpu" as const;
  readonly #canvas: HTMLCanvasElement;
  readonly #device: GPUDevice;
  readonly #context: GPUCanvasContext;
  readonly #pipeline: GPURenderPipeline;
  readonly #sampler: GPUSampler;
  readonly #onDeviceLost: ((reason: string) => void) | undefined;
  #texture: GPUTexture | undefined;
  #bindGroup: GPUBindGroup | undefined;
  #textureWidth = 0;
  #textureHeight = 0;
  #disposed = false;

  private constructor(
    canvas: HTMLCanvasElement,
    device: GPUDevice,
    onDeviceLost: ((reason: string) => void) | undefined,
  ) {
    this.#canvas = canvas;
    this.#device = device;
    this.#onDeviceLost = onDeviceLost;
    const context = canvas.getContext("webgpu");
    if (context === null) {
      throw new Error("canvas.getContext('webgpu') returned null");
    }
    this.#context = context;
    const format = navigator.gpu.getPreferredCanvasFormat();
    this.#context.configure({ device, format, alphaMode: "opaque" });
    const module = device.createShaderModule({ code: WGSL });
    this.#pipeline = device.createRenderPipeline({
      layout: "auto",
      vertex: { module, entryPoint: "vs" },
      fragment: { module, entryPoint: "fs", targets: [{ format }] },
      primitive: { topology: "triangle-list" },
    });
    this.#sampler = device.createSampler({ magFilter: "linear", minFilter: "linear" });
  }

  /**
   * Acquires adapter+device and builds the pipeline. Throws an Error with
   * an actionable message; callers fall back to Canvas2D on rejection.
   */
  static async create(
    canvas: HTMLCanvasElement,
    onDeviceLost?: (reason: string) => void,
  ): Promise<WebGpuRenderer> {
    if (navigator.gpu === undefined) {
      throw new Error("WebGPU (navigator.gpu) is not available in this browser");
    }
    const adapter = await navigator.gpu.requestAdapter({ powerPreference: "low-power" });
    if (adapter === null) {
      throw new Error("WebGPU requestAdapter() returned null (no suitable GPU/driver)");
    }
    const device = await adapter.requestDevice();
    const renderer = new WebGpuRenderer(canvas, device, onDeviceLost);
    void device.lost.then((info) => {
      if (!renderer.#disposed) {
        renderer.#onDeviceLost?.(`WebGPU device lost (${info.reason}): ${info.message}`);
      }
    });
    return renderer;
  }

  #ensureTexture(width: number, height: number): void {
    if (
      this.#texture !== undefined &&
      this.#textureWidth === width &&
      this.#textureHeight === height
    ) {
      return;
    }
    this.#texture?.destroy();
    this.#texture = this.#device.createTexture({
      size: { width, height },
      format: "rgba8unorm",
      usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST,
    });
    this.#textureWidth = width;
    this.#textureHeight = height;
    this.#bindGroup = this.#device.createBindGroup({
      layout: this.#pipeline.getBindGroupLayout(0),
      entries: [
        { binding: 0, resource: this.#texture.createView() },
        { binding: 1, resource: this.#sampler },
      ],
    });
    this.#canvas.width = width;
    this.#canvas.height = height;
  }

  present(frame: FramebufferFrame): void {
    if (this.#disposed) {
      return;
    }
    const { width, height, stride, pixels } = frame;
    this.#ensureTexture(width, height);
    const texture = this.#texture as GPUTexture;
    // queue.writeTexture requires bytesPerRow to be a multiple of 256.
    if (stride % 256 === 0) {
      this.#device.queue.writeTexture(
        { texture },
        pixels as BufferSource,
        { bytesPerRow: stride, rowsPerImage: height },
        { width, height },
      );
    } else {
      const alignedRow = Math.ceil(stride / 256) * 256;
      const repacked = new Uint8Array(alignedRow * height);
      for (let y = 0; y < height; y += 1) {
        repacked.set(pixels.subarray(y * stride, y * stride + stride), y * alignedRow);
      }
      this.#device.queue.writeTexture(
        { texture },
        repacked as BufferSource,
        { bytesPerRow: alignedRow, rowsPerImage: height },
        { width, height },
      );
    }
    const encoder = this.#device.createCommandEncoder();
    const pass = encoder.beginRenderPass({
      colorAttachments: [
        {
          view: this.#context.getCurrentTexture().createView(),
          clearValue: { r: 0, g: 0, b: 0, a: 1 },
          loadOp: "clear",
          storeOp: "store",
        },
      ],
    });
    pass.setPipeline(this.#pipeline);
    pass.setBindGroup(0, this.#bindGroup as GPUBindGroup);
    pass.draw(3);
    pass.end();
    this.#device.queue.submit([encoder.finish()]);
  }

  dispose(): void {
    this.#disposed = true;
    this.#texture?.destroy();
    this.#device.destroy();
  }
}

/**
 * Creates the best renderer for this browser: WebGPU when it truly works,
 * Canvas2D otherwise. `onFallback` is told exactly why WebGPU was not used
 * (including later device loss, after which callers recreate a renderer).
 */
export async function createFramebufferRenderer(
  canvas: HTMLCanvasElement,
  onFallback: (reason: string) => void,
): Promise<FramebufferRenderer> {
  try {
    return await WebGpuRenderer.create(canvas, onFallback);
  } catch (error) {
    onFallback(error instanceof Error ? error.message : String(error));
    return new Canvas2dRenderer(canvas);
  }
}
