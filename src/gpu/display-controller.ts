/**
 * High-level display controller managing `<canvas>` lifecycle, WebGPU <-> Canvas2D backend
 * selection, automatic fallback on WebGPU unavailability or unrecoverable device loss,
 * `ResizeObserver` viewport updates, and shadow frame retention across backend switches.
 */

import type { FramebufferDirtyUpdate } from '../wasm/bindings';
import { Canvas2DFramebufferRenderer } from './canvas2d-renderer';
import type {
  FramebufferRenderer,
  FramebufferRendererBackend,
  FramebufferRendererMetrics,
  RendererDisplayFilter,
} from './types';
import { WebGpuFramebufferRenderer } from './webgpu-renderer';

export type PreferredRendererMode = 'auto' | 'webgpu' | 'canvas2d';

export interface DisplayControllerOptions {
  readonly container: HTMLElement;
  readonly preferredMode?: PreferredRendererMode;
  readonly initialFilter?: RendererDisplayFilter;
  readonly onBackendChanged?: (
    backend: FramebufferRendererBackend,
    adapterInfo: string,
    fallbackReason?: string,
  ) => void;
}

export class FramebufferDisplayController {
  private readonly container: HTMLElement;
  private readonly onBackendChanged: DisplayControllerOptions['onBackendChanged'];
  private renderer: FramebufferRenderer | null = null;
  private canvas: HTMLCanvasElement | null = null;
  private resizeObserver: ResizeObserver | null = null;

  private preferredMode: PreferredRendererMode;
  private filter: RendererDisplayFilter;
  private fbWidth = 640;
  private fbHeight = 400;
  private shadowBgrx = new Uint8Array(640 * 400 * 4);
  private hasShadowContent = false;
  private destroyed = false;

  public constructor(opts: DisplayControllerOptions) {
    this.container = opts.container;
    this.preferredMode = opts.preferredMode ?? 'auto';
    this.filter = opts.initialFilter ?? 'crisp';
    this.onBackendChanged = opts.onBackendChanged;
  }

  public async init(): Promise<FramebufferRendererBackend> {
    await this.switchMode(this.preferredMode);
    if (typeof ResizeObserver !== 'undefined') {
      this.resizeObserver = new ResizeObserver((entries) => {
        for (const entry of entries) {
          const w = entry.contentRect.width;
          const h = entry.contentRect.height;
          if (w > 0 && h > 0) {
            this.renderer?.resizeViewport(
              w,
              h,
              typeof window !== 'undefined' ? window.devicePixelRatio : 1,
            );
          }
        }
      });
      this.resizeObserver.observe(this.container);
    }
    return this.activeBackend;
  }

  public get activeBackend(): FramebufferRendererBackend {
    return this.renderer?.backend ?? 'canvas2d';
  }

  public get canvasElement(): HTMLCanvasElement | null {
    return this.canvas;
  }

  private mountFreshCanvas(): HTMLCanvasElement {
    if (this.canvas !== null && this.canvas.parentElement === this.container) {
      this.container.removeChild(this.canvas);
    }
    const c = document.createElement('canvas');
    c.className = 'wasmnux-fb-canvas';
    c.dataset.testid = 'fb-canvas';
    c.style.width = '100%';
    c.style.height = '100%';
    c.style.display = 'block';
    this.container.appendChild(c);
    this.canvas = c;
    return c;
  }

  public async switchMode(mode: PreferredRendererMode): Promise<FramebufferRendererBackend> {
    if (this.destroyed) {
      throw new Error('FramebufferDisplayController destroyed');
    }
    this.preferredMode = mode;
    if (this.renderer !== null) {
      this.renderer.destroy();
      this.renderer = null;
    }

    if (mode === 'auto' || mode === 'webgpu') {
      const webgpuCanvas = this.mountFreshCanvas();
      const candidate = new WebGpuFramebufferRenderer(webgpuCanvas, {
        onFallbackRequired: (reason) => {
          void this.activateCanvas2dFallback(reason);
        },
      });
      try {
        await candidate.init();
        candidate.setFilter(this.filter);
        this.renderer = candidate;
        this.replayShadowIfPresent(candidate);
        const m = candidate.getMetrics();
        this.onBackendChanged?.('webgpu', m.adapterInfo);
        return 'webgpu';
      } catch (err: unknown) {
        candidate.destroy();
        const reason = err instanceof Error ? err.message : String(err);
        if (mode === 'webgpu') {
          await this.activateCanvas2dFallback(`WebGPU unavailable (${reason}); using Canvas 2D`);
          return 'canvas2d';
        }
        await this.activateCanvas2dFallback(reason);
        return 'canvas2d';
      }
    }

    await this.activateCanvas2dFallback();
    return 'canvas2d';
  }

  private async activateCanvas2dFallback(reason?: string): Promise<void> {
    if (this.destroyed) {
      return;
    }
    if (this.renderer !== null) {
      this.renderer.destroy();
      this.renderer = null;
    }
    const c2dCanvas = this.mountFreshCanvas();
    const fallback = new Canvas2DFramebufferRenderer(c2dCanvas);
    await fallback.init();
    fallback.setFilter(this.filter);
    this.renderer = fallback;
    this.replayShadowIfPresent(fallback);
    const m = fallback.getMetrics();
    this.onBackendChanged?.('canvas2d', m.adapterInfo, reason);
  }

  private replayShadowIfPresent(target: FramebufferRenderer): void {
    const rect = this.container.getBoundingClientRect();
    if (rect.width > 0 && rect.height > 0) {
      target.resizeViewport(
        rect.width,
        rect.height,
        typeof window !== 'undefined' ? window.devicePixelRatio : 1,
      );
    }
    if (this.hasShadowContent) {
      target.applyDirtyUpdate({
        width: this.fbWidth,
        height: this.fbHeight,
        yMin: 0,
        yMax: this.fbHeight,
        pixels: this.shadowBgrx.slice(),
      });
    }
  }

  public applyDirtyUpdate(update: FramebufferDirtyUpdate): void {
    if (this.destroyed || update.yMax <= update.yMin) {
      return;
    }
    if (update.width !== this.fbWidth || update.height !== this.fbHeight) {
      this.fbWidth = update.width;
      this.fbHeight = update.height;
      this.shadowBgrx = new Uint8Array(update.width * update.height * 4);
    }
    const rowCount = update.yMax - update.yMin;
    const dstOffset = update.yMin * this.fbWidth * 4;
    const byteLen = rowCount * this.fbWidth * 4;
    if (dstOffset + byteLen <= this.shadowBgrx.byteLength && update.pixels.byteLength >= byteLen) {
      this.shadowBgrx.set(update.pixels.subarray(0, byteLen), dstOffset);
      this.hasShadowContent = true;
    }
    this.renderer?.applyDirtyUpdate(update);
  }

  public setFilter(filter: RendererDisplayFilter): void {
    this.filter = filter;
    this.renderer?.setFilter(filter);
  }

  public getMetrics(): FramebufferRendererMetrics | null {
    return this.renderer?.getMetrics() ?? null;
  }

  public async readRgbaPixels(): Promise<Uint8Array> {
    if (this.renderer === null) {
      return new Uint8Array(0);
    }
    return this.renderer.readRgbaPixels();
  }

  public async simulateDeviceLoss(): Promise<void> {
    if (this.renderer !== null) {
      await this.renderer.simulateDeviceLoss();
    }
  }

  public clearScreen(): void {
    this.shadowBgrx.fill(0);
    this.hasShadowContent = true;
    this.renderer?.applyDirtyUpdate({
      width: this.fbWidth,
      height: this.fbHeight,
      yMin: 0,
      yMax: this.fbHeight,
      pixels: this.shadowBgrx,
    });
  }

  public destroy(): void {
    this.destroyed = true;
    this.resizeObserver?.disconnect();
    this.resizeObserver = null;
    this.renderer?.destroy();
    this.renderer = null;
    if (this.canvas !== null && this.canvas.parentElement === this.container) {
      this.container.removeChild(this.canvas);
    }
    this.canvas = null;
  }
}
