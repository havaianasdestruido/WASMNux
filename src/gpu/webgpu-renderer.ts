/**
 * Genuine WebGPU framebuffer compute & presentation renderer (`WebGpuFramebufferRenderer`).
 *
 * GPU Pipeline Architecture:
 * 1. Compute Pipeline (`CONVERT_BGRX_TO_RGBA_WGSL`):
 *    Reads dirty `SimpleFB` `XRGB8888` (`u32` `0x00RRGGBB`) scanlines from a GPU storage buffer
 *    and unpacks/converts `(R, G, B, 1.0)` texels directly into a persistent `rgba8unorm`
 *    `GPUTexture` on the GPU. Unchanged scanlines incur zero upload or compute work.
 * 2. Render Pipeline (`PRESENT_FRAMEBUFFER_WGSL`):
 *    Draws a fullscreen triangle sampling the `rgba8unorm` texture with aspect-ratio-preserving
 *    letterbox/pillarbox UV mapping and selectable GPU post-processing (`crisp`, `smooth`, `crt`).
 * 3. Device-Loss Recovery (`device.lost`):
 *    Retains a compact `BGRX8888` shadow frame so if the GPU device resets or is lost,
 *    `WebGpuFramebufferRenderer` automatically re-initializes `GPUAdapter` + `GPUDevice`,
 *    rebuilds pipelines, and re-uploads the full guest framebuffer without interrupting Linux.
 */

import type { FramebufferDirtyUpdate } from '../wasm/bindings';
import type {
  FramebufferRenderer,
  FramebufferRendererMetrics,
  RendererDisplayFilter,
} from './types';

export const CONVERT_BGRX_TO_RGBA_WGSL = /* wgsl */ `
struct DirtyRegionUniform {
  width: u32,
  y_offset: u32,
  row_count: u32,
  _pad: u32,
};

@group(0) @binding(0) var<uniform> region: DirtyRegionUniform;
@group(0) @binding(1) var<storage, read> src_bgrx: array<u32>;
@group(0) @binding(2) var dst_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (gid.x >= region.width || gid.y >= region.row_count) {
    return;
  }
  let idx: u32 = gid.y * region.width + gid.x;
  let pixel: u32 = src_bgrx[idx];
  let b: f32 = f32(pixel & 0xFFu) / 255.0;
  let g: f32 = f32((pixel >> 8u) & 0xFFu) / 255.0;
  let r: f32 = f32((pixel >> 16u) & 0xFFu) / 255.0;
  textureStore(dst_tex, vec2<i32>(i32(gid.x), i32(region.y_offset + gid.y)), vec4<f32>(r, g, b, 1.0));
}
`;

export const PRESENT_FRAMEBUFFER_WGSL = /* wgsl */ `
struct PresentUniform {
  src_size: vec2<f32>,
  dst_size: vec2<f32>,
  filter_mode: u32,
  _pad0: u32,
  _pad1: u32,
  _pad2: u32,
};

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var<uniform> params: PresentUniform;
@group(0) @binding(1) var fb_tex: texture_2d<f32>;
@group(0) @binding(2) var nearest_samp: sampler;
@group(0) @binding(3) var linear_samp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
  var positions = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -3.0),
    vec2<f32>(3.0, 1.0),
    vec2<f32>(-1.0, 1.0)
  );
  let p = positions[vid];
  var out: VsOut;
  out.pos = vec4<f32>(p, 0.0, 1.0);
  out.uv = vec2<f32>((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
  return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
  let src_aspect = params.src_size.x / max(params.src_size.y, 1.0);
  let dst_aspect = params.dst_size.x / max(params.dst_size.y, 1.0);

  var mapped_uv = in.uv;
  if (dst_aspect > src_aspect) {
    let scale = dst_aspect / src_aspect;
    mapped_uv.x = (in.uv.x - 0.5) * scale + 0.5;
  } else {
    let scale = src_aspect / dst_aspect;
    mapped_uv.y = (in.uv.y - 0.5) * scale + 0.5;
  }

  let nearest_color = textureSampleLevel(fb_tex, nearest_samp, clamp(mapped_uv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
  let linear_color = textureSampleLevel(fb_tex, linear_samp, clamp(mapped_uv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);

  if (mapped_uv.x < 0.0 || mapped_uv.x > 1.0 || mapped_uv.y < 0.0 || mapped_uv.y > 1.0) {
    return vec4<f32>(0.02, 0.03, 0.06, 1.0);
  }

  var color = select(nearest_color, linear_color, params.filter_mode == 1u);
  if (params.filter_mode == 2u) {
    let scan_y = floor(mapped_uv.y * params.src_size.y);
    let scan_atten = select(1.0, 0.80, (u32(scan_y) % 2u) == 1u);
    let vig_d = distance(mapped_uv, vec2<f32>(0.5, 0.5));
    let vignette = clamp(1.08 - vig_d * 0.35, 0.72, 1.0);
    color = vec4<f32>(color.rgb * scan_atten * vignette, 1.0);
  }
  return color;
}
`;

export interface WebGpuRendererOptions {
  readonly onFallbackRequired?: (reason: string) => void;
}

interface GpuResources {
  readonly device: GPUDevice;
  readonly context: GPUCanvasContext;
  readonly format: GPUTextureFormat;
  readonly computePipeline: GPUComputePipeline;
  readonly renderPipeline: GPURenderPipeline;
  readonly regionUniformBuf: GPUBuffer;
  readonly presentUniformBuf: GPUBuffer;
  readonly nearestSampler: GPUSampler;
  readonly linearSampler: GPUSampler;
  stagingBuf: GPUBuffer;
  stagingCapacityBytes: number;
  fbTexture: GPUTexture;
  fbTextureView: GPUTextureView;
  computeBindGroup: GPUBindGroup;
  presentBindGroup: GPUBindGroup;
}

export class WebGpuFramebufferRenderer implements FramebufferRenderer {
  public readonly backend = 'webgpu' as const;
  private readonly canvas: HTMLCanvasElement;
  private readonly onFallbackRequired: ((reason: string) => void) | undefined;

  private gpu: GpuResources | null = null;
  private adapterInfo = 'WebGPU';
  private fbWidth = 640;
  private fbHeight = 400;
  private viewportWidth = 640;
  private viewportHeight = 400;
  private shadowBgrx = new Uint8Array(640 * 400 * 4);
  private filter: RendererDisplayFilter = 'crisp';

  private framesPresented = 0;
  private dirtyUploads = 0;
  private uploadedBytes = 0;
  private deviceLostRecoveries = 0;
  private recovering = false;
  private destroyed = false;

  public constructor(canvas: HTMLCanvasElement, opts?: WebGpuRendererOptions) {
    this.canvas = canvas;
    this.onFallbackRequired = opts?.onFallbackRequired;
  }

  public async init(): Promise<void> {
    if (this.destroyed) {
      throw new Error('WebGpuFramebufferRenderer already destroyed');
    }
    this.viewportWidth = Math.max(1, this.canvas.clientWidth || this.fbWidth);
    this.viewportHeight = Math.max(1, this.canvas.clientHeight || this.fbHeight);
    this.canvas.width = this.viewportWidth;
    this.canvas.height = this.viewportHeight;

    await this.initializeDeviceAndPipelines();
    this.uploadFullShadowFrame();
    this.present();
  }

  private async initializeDeviceAndPipelines(): Promise<void> {
    const maybeGpu =
      typeof navigator !== 'undefined' ? (navigator as Partial<Navigator>).gpu : undefined;
    if (maybeGpu === undefined) {
      throw new Error('navigator.gpu is unavailable in this browser');
    }

    const adapter = await maybeGpu.requestAdapter({ powerPreference: 'high-performance' });
    if (adapter === null) {
      throw new Error('WebGPU requestAdapter() returned null');
    }

    const info = adapter.info;
    this.adapterInfo =
      [info.vendor, info.architecture, info.device, info.description]
        .filter((part) => typeof part === 'string' && part.trim().length > 0)
        .join(' / ') || 'WebGPU Adapter';

    const device = await adapter.requestDevice();
    const context = this.canvas.getContext('webgpu');
    if (context === null) {
      device.destroy();
      throw new Error('Failed to acquire <canvas> webgpu context');
    }

    const format = navigator.gpu.getPreferredCanvasFormat();
    context.configure({
      device,
      format,
      alphaMode: 'opaque',
    });

    const computeModule = device.createShaderModule({
      label: 'wasmnux-bgrx-to-rgba-compute',
      code: CONVERT_BGRX_TO_RGBA_WGSL,
    });
    const renderModule = device.createShaderModule({
      label: 'wasmnux-present-fb-render',
      code: PRESENT_FRAMEBUFFER_WGSL,
    });

    const computePipeline = device.createComputePipeline({
      label: 'wasmnux-compute-pipeline',
      layout: 'auto',
      compute: {
        module: computeModule,
        entryPoint: 'main',
      },
    });

    const renderPipeline = device.createRenderPipeline({
      label: 'wasmnux-render-pipeline',
      layout: 'auto',
      vertex: {
        module: renderModule,
        entryPoint: 'vs_main',
      },
      fragment: {
        module: renderModule,
        entryPoint: 'fs_main',
        targets: [{ format }],
      },
      primitive: {
        topology: 'triangle-list',
      },
    });

    const regionUniformBuf = device.createBuffer({
      label: 'wasmnux-dirty-region-uniform',
      size: 16,
      usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
    });

    const presentUniformBuf = device.createBuffer({
      label: 'wasmnux-present-uniform',
      size: 32,
      usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
    });

    const nearestSampler = device.createSampler({
      magFilter: 'nearest',
      minFilter: 'nearest',
    });
    const linearSampler = device.createSampler({
      magFilter: 'linear',
      minFilter: 'linear',
    });

    const stagingCapacityBytes = Math.max(16, this.fbWidth * this.fbHeight * 4);
    const stagingBuf = device.createBuffer({
      label: 'wasmnux-bgrx-staging',
      size: stagingCapacityBytes,
      usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
    });

    const fbTexture = device.createTexture({
      label: 'wasmnux-fb-rgba8',
      size: { width: this.fbWidth, height: this.fbHeight, depthOrArrayLayers: 1 },
      format: 'rgba8unorm',
      usage:
        GPUTextureUsage.STORAGE_BINDING |
        GPUTextureUsage.TEXTURE_BINDING |
        GPUTextureUsage.COPY_SRC,
    });
    const fbTextureView = fbTexture.createView();

    const computeBindGroup = device.createBindGroup({
      layout: computePipeline.getBindGroupLayout(0),
      entries: [
        { binding: 0, resource: { buffer: regionUniformBuf } },
        { binding: 1, resource: { buffer: stagingBuf } },
        { binding: 2, resource: fbTextureView },
      ],
    });

    const presentBindGroup = device.createBindGroup({
      layout: renderPipeline.getBindGroupLayout(0),
      entries: [
        { binding: 0, resource: { buffer: presentUniformBuf } },
        { binding: 1, resource: fbTextureView },
        { binding: 2, resource: nearestSampler },
        { binding: 3, resource: linearSampler },
      ],
    });

    this.gpu = {
      device,
      context,
      format,
      computePipeline,
      renderPipeline,
      regionUniformBuf,
      presentUniformBuf,
      nearestSampler,
      linearSampler,
      stagingBuf,
      stagingCapacityBytes,
      fbTexture,
      fbTextureView,
      computeBindGroup,
      presentBindGroup,
    };

    void device.lost.then((lossInfo) => {
      if (this.destroyed || this.gpu?.device !== device) {
        return;
      }
      void this.handleDeviceLost(lossInfo.message || lossInfo.reason);
    });
  }

  private async handleDeviceLost(reason: string): Promise<void> {
    if (this.destroyed || this.recovering) {
      return;
    }
    this.recovering = true;
    try {
      this.releaseGpuObjects(false);
      await this.initializeDeviceAndPipelines();
      this.recovering = false;
      this.uploadFullShadowFrame();
      this.present();
      this.deviceLostRecoveries += 1;
    } catch (err: unknown) {
      this.recovering = false;
      const msg = err instanceof Error ? err.message : String(err);
      this.onFallbackRequired?.(`WebGPU device lost (${reason}) and recovery failed: ${msg}`);
    }
  }

  private ensureGuestDimensions(width: number, height: number): void {
    const w = Math.max(1, width);
    const h = Math.max(1, height);
    if (w === this.fbWidth && h === this.fbHeight) {
      return;
    }
    this.fbWidth = w;
    this.fbHeight = h;
    this.shadowBgrx = new Uint8Array(w * h * 4);

    const g = this.gpu;
    if (g === null) {
      return;
    }

    g.fbTexture.destroy();
    g.stagingBuf.destroy();

    const cap = Math.max(16, w * h * 4);
    g.stagingBuf = g.device.createBuffer({
      label: 'wasmnux-bgrx-staging',
      size: cap,
      usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
    });
    g.stagingCapacityBytes = cap;

    g.fbTexture = g.device.createTexture({
      label: 'wasmnux-fb-rgba8',
      size: { width: w, height: h, depthOrArrayLayers: 1 },
      format: 'rgba8unorm',
      usage:
        GPUTextureUsage.STORAGE_BINDING |
        GPUTextureUsage.TEXTURE_BINDING |
        GPUTextureUsage.COPY_SRC,
    });
    g.fbTextureView = g.fbTexture.createView();

    g.computeBindGroup = g.device.createBindGroup({
      layout: g.computePipeline.getBindGroupLayout(0),
      entries: [
        { binding: 0, resource: { buffer: g.regionUniformBuf } },
        { binding: 1, resource: { buffer: g.stagingBuf } },
        { binding: 2, resource: g.fbTextureView },
      ],
    });
    g.presentBindGroup = g.device.createBindGroup({
      layout: g.renderPipeline.getBindGroupLayout(0),
      entries: [
        { binding: 0, resource: { buffer: g.presentUniformBuf } },
        { binding: 1, resource: g.fbTextureView },
        { binding: 2, resource: g.nearestSampler },
        { binding: 3, resource: g.linearSampler },
      ],
    });
  }

  private uploadFullShadowFrame(): void {
    if (this.gpu === null || this.recovering) {
      return;
    }
    this.dispatchComputeConversion(0, this.fbHeight, this.shadowBgrx);
  }

  private dispatchComputeConversion(yMin: number, rowCount: number, pixels: Uint8Array): void {
    const g = this.gpu;
    if (g === null || rowCount <= 0) {
      return;
    }
    const byteLen = this.fbWidth * rowCount * 4;
    if (pixels.byteLength < byteLen || byteLen > g.stagingCapacityBytes) {
      return;
    }

    const regionData = new Uint32Array([this.fbWidth, yMin, rowCount, 0]);
    g.device.queue.writeBuffer(g.regionUniformBuf, 0, regionData);
    g.device.queue.writeBuffer(g.stagingBuf, 0, pixels.buffer, pixels.byteOffset, byteLen);

    const encoder = g.device.createCommandEncoder({ label: 'wasmnux-compute-encoder' });
    const pass = encoder.beginComputePass({ label: 'wasmnux-bgrx-convert-pass' });
    pass.setPipeline(g.computePipeline);
    pass.setBindGroup(0, g.computeBindGroup);
    pass.dispatchWorkgroups(Math.ceil(this.fbWidth / 16), Math.ceil(rowCount / 16), 1);
    pass.end();
    g.device.queue.submit([encoder.finish()]);
  }

  public applyDirtyUpdate(update: FramebufferDirtyUpdate): void {
    if (this.destroyed || update.yMax <= update.yMin) {
      return;
    }
    this.ensureGuestDimensions(update.width, update.height);

    const rowCount = update.yMax - update.yMin;
    const dstOffset = update.yMin * this.fbWidth * 4;
    const byteLen = rowCount * this.fbWidth * 4;
    if (dstOffset + byteLen <= this.shadowBgrx.byteLength && update.pixels.byteLength >= byteLen) {
      this.shadowBgrx.set(update.pixels.subarray(0, byteLen), dstOffset);
    }

    if (this.gpu !== null && !this.recovering) {
      this.dispatchComputeConversion(update.yMin, rowCount, update.pixels);
      this.dirtyUploads += 1;
      this.uploadedBytes += byteLen;
      this.present();
    }
  }

  public present(): void {
    const g = this.gpu;
    if (this.destroyed || g === null || this.recovering) {
      return;
    }

    const filterMode = this.filter === 'smooth' ? 1 : this.filter === 'crt' ? 2 : 0;
    const uniformBuf = new ArrayBuffer(32);
    const f32 = new Float32Array(uniformBuf, 0, 4);
    f32[0] = this.fbWidth;
    f32[1] = this.fbHeight;
    f32[2] = this.viewportWidth;
    f32[3] = this.viewportHeight;
    const u32 = new Uint32Array(uniformBuf, 16, 4);
    u32[0] = filterMode;
    g.device.queue.writeBuffer(g.presentUniformBuf, 0, uniformBuf);

    const targetView = g.context.getCurrentTexture().createView();
    const encoder = g.device.createCommandEncoder({ label: 'wasmnux-present-encoder' });
    const pass = encoder.beginRenderPass({
      colorAttachments: [
        {
          view: targetView,
          clearValue: { r: 0.02, g: 0.03, b: 0.06, a: 1.0 },
          loadOp: 'clear',
          storeOp: 'store',
        },
      ],
    });
    pass.setPipeline(g.renderPipeline);
    pass.setBindGroup(0, g.presentBindGroup);
    pass.draw(3, 1, 0, 0);
    pass.end();
    g.device.queue.submit([encoder.finish()]);
    this.framesPresented += 1;
  }

  public resizeViewport(cssWidth: number, cssHeight: number, devicePixelRatio = 1): void {
    if (this.destroyed) {
      return;
    }
    const dpr = Math.max(1, Math.min(4, devicePixelRatio || 1));
    const pxW = Math.max(1, Math.round(cssWidth * dpr));
    const pxH = Math.max(1, Math.round(cssHeight * dpr));
    if (this.canvas.width !== pxW || this.canvas.height !== pxH) {
      this.canvas.width = pxW;
      this.canvas.height = pxH;
    }
    this.viewportWidth = pxW;
    this.viewportHeight = pxH;
    this.present();
  }

  public setFilter(filter: RendererDisplayFilter): void {
    this.filter = filter;
    this.present();
  }

  public getMetrics(): FramebufferRendererMetrics {
    return {
      backend: 'webgpu',
      adapterInfo: this.adapterInfo,
      fbWidth: this.fbWidth,
      fbHeight: this.fbHeight,
      viewportWidth: this.viewportWidth,
      viewportHeight: this.viewportHeight,
      framesPresented: this.framesPresented,
      dirtyUploads: this.dirtyUploads,
      uploadedBytes: this.uploadedBytes,
      deviceLostRecoveries: this.deviceLostRecoveries,
      filter: this.filter,
    };
  }

  /**
   * Read back GPU-converted `rgba8unorm` pixels directly from `fbTexture` for automated verification.
   */
  public async readRgbaPixels(): Promise<Uint8Array> {
    const g = this.gpu;
    if (this.destroyed || g === null) {
      return new Uint8Array(0);
    }
    // WebGPU copyTextureToBuffer requires bytesPerRow to be a multiple of 256.
    const unpaddedRowBytes = this.fbWidth * 4;
    const paddedRowBytes = Math.ceil(unpaddedRowBytes / 256) * 256;
    const readbackSize = paddedRowBytes * this.fbHeight;

    const readbackBuf = g.device.createBuffer({
      label: 'wasmnux-rgba-readback',
      size: readbackSize,
      usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
    });

    const encoder = g.device.createCommandEncoder();
    encoder.copyTextureToBuffer(
      { texture: g.fbTexture },
      { buffer: readbackBuf, bytesPerRow: paddedRowBytes, rowsPerImage: this.fbHeight },
      { width: this.fbWidth, height: this.fbHeight, depthOrArrayLayers: 1 },
    );
    g.device.queue.submit([encoder.finish()]);

    await readbackBuf.mapAsync(GPUMapMode.READ);
    const mapped = new Uint8Array(readbackBuf.getMappedRange());
    const out = new Uint8Array(this.fbWidth * this.fbHeight * 4);
    for (let y = 0; y < this.fbHeight; y += 1) {
      const srcStart = y * paddedRowBytes;
      out.set(mapped.subarray(srcStart, srcStart + unpaddedRowBytes), y * unpaddedRowBytes);
    }
    readbackBuf.unmap();
    readbackBuf.destroy();
    return out;
  }

  public async simulateDeviceLoss(): Promise<void> {
    const g = this.gpu;
    if (g === null || this.destroyed) {
      return;
    }
    g.device.destroy();
    // Wait until `device.lost` recovery completes
    const deadline = performance.now() + 5_000;
    const prevRecoveries = this.deviceLostRecoveries;
    while (this.deviceLostRecoveries <= prevRecoveries && performance.now() < deadline) {
      await new Promise((r) => setTimeout(r, 20));
    }
  }

  private releaseGpuObjects(unconfigureCanvas: boolean): void {
    const g = this.gpu;
    this.gpu = null;
    if (g === null) {
      return;
    }
    try {
      if (unconfigureCanvas) {
        g.context.unconfigure();
      }
    } catch {
      // Ignore unconfigure on already-lost context
    }
    g.stagingBuf.destroy();
    g.regionUniformBuf.destroy();
    g.presentUniformBuf.destroy();
    g.fbTexture.destroy();
    g.device.destroy();
  }

  public destroy(): void {
    this.destroyed = true;
    this.releaseGpuObjects(true);
  }
}
