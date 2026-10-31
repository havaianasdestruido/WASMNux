/// <reference types="@webgpu/types" />
/** Presents the guest's simple-framebuffer scanout. WebGPU is a presentation path only. */
export const FB_WIDTH = 640;
export const FB_HEIGHT = 400;
export const FB_BYTES = FB_WIDTH * FB_HEIGHT * 4;

const SHADER = `
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
struct Out { @builtin(position) pos: vec4f, @location(0) uv: vec2f };
@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
  var p = array<vec2f, 3>(vec2f(-1.0, -3.0), vec2f(3.0, 1.0), vec2f(-1.0, 1.0));
  var o: Out;
  o.pos = vec4f(p[i], 0.0, 1.0);
  o.uv = vec2f((p[i].x + 1.0) * 0.5, (1.0 - p[i].y) * 0.5);
  return o;
}
@fragment fn fs(o: Out) -> @location(0) vec4f {
  return textureSample(tex, samp, o.uv);
}
`;

export type RendererKind = 'webgpu' | 'canvas2d';
export class FramebufferRenderer {
  kind: RendererKind = 'canvas2d';
  private device: GPUDevice | undefined;
  private context: GPUCanvasContext | undefined;
  private texture: GPUTexture | undefined;
  private pipeline: GPURenderPipeline | undefined;
  private bindGroup: GPUBindGroup | undefined;
  private sampler: GPUSampler | undefined;
  private format: GPUTextureFormat = 'bgra8unorm';
  private canvas2d: CanvasRenderingContext2D | null = null;
  private image: ImageData | undefined;
  private disposed = false;
  private lost = false;
  constructor(
    private canvas: HTMLCanvasElement,
    private onKind: (kind: RendererKind, reason?: string) => void,
  ) {
    canvas.width = FB_WIDTH;
    canvas.height = FB_HEIGHT;
  }
  async init(): Promise<void> {
    if (this.disposed) return;
    try {
      if (!navigator.gpu) throw new Error('WebGPU unavailable');
      const adapter = await navigator.gpu.requestAdapter();
      if (!adapter) throw new Error('No WebGPU adapter');
      const device = await adapter.requestDevice();
      if (this.disposed) {
        device.destroy();
        return;
      }
      this.device = device;
      this.format = navigator.gpu.getPreferredCanvasFormat();
      // Guest scanout is fixed BGRA; use a BGRA source texture and render to the preferred canvas format.
      const context = this.canvas.getContext('webgpu');
      if (!context) throw new Error('WebGPU canvas context unavailable');
      this.context = context;
      context.configure({ device, format: this.format, alphaMode: 'opaque' });
      this.texture = device.createTexture({
        size: [FB_WIDTH, FB_HEIGHT],
        format: 'bgra8unorm',
        usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST,
      });
      this.sampler = device.createSampler({ magFilter: 'linear', minFilter: 'linear' });
      const module = device.createShaderModule({ code: SHADER });
      const info = await module.getCompilationInfo();
      const errors = info.messages.filter((m) => m.type === 'error');
      if (errors.length) throw new Error(errors.map((m) => m.message).join('; '));
      this.pipeline = device.createRenderPipeline({
        layout: 'auto',
        vertex: { module, entryPoint: 'vs' },
        fragment: { module, entryPoint: 'fs', targets: [{ format: this.format }] },
        primitive: { topology: 'triangle-list' },
      });
      this.bindGroup = device.createBindGroup({
        layout: this.pipeline.getBindGroupLayout(0),
        entries: [
          { binding: 0, resource: this.texture.createView() },
          { binding: 1, resource: this.sampler },
        ],
      });
      device.lost
        .then((info) => {
          if (this.disposed) return;
          this.lost = true;
          this.releaseGpu();
          this.kind = 'canvas2d';
          this.onKind('canvas2d', `WebGPU device lost: ${info.message}; using Canvas fallback`);
          // Attempt recreation once; if it fails, Canvas remains usable.
          void this.init().catch(() => undefined);
        })
        .catch(() => undefined);
      device.addEventListener('uncapturederror', (event) => {
        this.lost = true;
        this.releaseGpu();
        this.kind = 'canvas2d';
        this.onKind('canvas2d', `WebGPU error: ${event.error.message}`);
      });
      this.kind = 'webgpu';
      this.lost = false;
      this.onKind('webgpu');
    } catch (e) {
      this.releaseGpu();
      this.kind = 'canvas2d';
      this.onKind('canvas2d', e instanceof Error ? e.message : String(e));
    }
    if (this.kind === 'canvas2d') {
      this.canvas2d = this.canvas.getContext('2d');
      this.image = this.canvas2d?.createImageData(FB_WIDTH, FB_HEIGHT) ?? undefined;
    }
  }
  private releaseGpu(): void {
    this.texture?.destroy();
    this.texture = undefined;
    this.bindGroup = undefined;
    this.pipeline = undefined;
    this.sampler = undefined;
    this.context?.unconfigure();
    this.context = undefined;
    this.device?.destroy();
    this.device = undefined;
  }
  /** pixels are the guest's raw 640x400 BGRA scanout. */
  render(pixels: Uint8Array): void {
    if (this.disposed || pixels.length !== FB_BYTES) return;
    if (
      this.kind === 'webgpu' &&
      this.device &&
      this.texture &&
      this.pipeline &&
      this.bindGroup &&
      this.context &&
      !this.lost
    ) {
      try {
        this.device.queue.writeTexture(
          { texture: this.texture },
          pixels.buffer,
          { offset: pixels.byteOffset, bytesPerRow: FB_WIDTH * 4, rowsPerImage: FB_HEIGHT },
          [FB_WIDTH, FB_HEIGHT],
        );
        const encoder = this.device.createCommandEncoder();
        const pass = encoder.beginRenderPass({
          colorAttachments: [
            {
              view: this.context.getCurrentTexture().createView(),
              clearValue: { r: 0, g: 0, b: 0, a: 1 },
              loadOp: 'clear',
              storeOp: 'store',
            },
          ],
        });
        pass.setPipeline(this.pipeline);
        pass.setBindGroup(0, this.bindGroup);
        pass.draw(3);
        pass.end();
        this.device.queue.submit([encoder.finish()]);
        return;
      } catch {
        this.lost = true;
        this.releaseGpu();
        this.kind = 'canvas2d';
        this.canvas2d = this.canvas.getContext('2d');
        this.image = this.canvas2d?.createImageData(FB_WIDTH, FB_HEIGHT) ?? undefined;
        this.onKind('canvas2d', 'WebGPU presentation failed; using Canvas fallback');
      }
    }
    if (this.canvas2d && this.image) {
      const out = this.image.data;
      for (let i = 0; i < pixels.length; i += 4) {
        out[i] = pixels[i + 2]!;
        out[i + 1] = pixels[i + 1]!;
        out[i + 2] = pixels[i]!;
        out[i + 3] = 255;
      }
      this.canvas2d.putImageData(this.image, 0, 0);
    }
  }
  dispose(): void {
    this.disposed = true;
    this.releaseGpu();
    this.canvas2d = null;
    this.image = undefined;
  }
}
