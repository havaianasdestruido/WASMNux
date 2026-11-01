/**
 * Software Canvas 2D fallback renderer for Linux `SimpleFB` (`XRGB8888` / `BGRX8888`).
 * Ensures full Linux usability when WebGPU is unavailable, blocked, or disabled by user toggle.
 */

import type { FramebufferDirtyUpdate } from '../wasm/bindings';
import {
  computeAspectFitRect,
  convertBgrxToRgba,
  type FramebufferRenderer,
  type FramebufferRendererMetrics,
  type RendererDisplayFilter,
} from './types';

export class Canvas2DFramebufferRenderer implements FramebufferRenderer {
  public readonly backend = 'canvas2d' as const;
  private readonly canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D | null = null;
  private backingCanvas: HTMLCanvasElement | null = null;
  private backingCtx: CanvasRenderingContext2D | null = null;
  private rowImageData: ImageData | null = null;

  private fbWidth = 640;
  private fbHeight = 400;
  private viewportWidth = 640;
  private viewportHeight = 400;
  private filter: RendererDisplayFilter = 'crisp';
  private framesPresented = 0;
  private dirtyUploads = 0;
  private uploadedBytes = 0;
  private destroyed = false;

  public constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
  }

  public init(): Promise<void> {
    if (this.destroyed) {
      return Promise.reject(new Error('Canvas2DFramebufferRenderer already destroyed'));
    }
    const ctx = this.canvas.getContext('2d', { alpha: false });
    if (ctx === null) {
      return Promise.reject(new Error('Failed to acquire 2D rendering context on <canvas>'));
    }
    this.ctx = ctx;
    this.ensureBackingSurface(this.fbWidth, this.fbHeight);
    this.resizeViewport(
      this.canvas.clientWidth || this.fbWidth,
      this.canvas.clientHeight || this.fbHeight,
      typeof window !== 'undefined' ? window.devicePixelRatio : 1,
    );
    this.present();
    return Promise.resolve();
  }

  private ensureBackingSurface(width: number, height: number): void {
    if (
      this.backingCanvas !== null &&
      this.fbWidth === width &&
      this.fbHeight === height &&
      this.backingCtx !== null
    ) {
      return;
    }
    this.fbWidth = Math.max(1, width);
    this.fbHeight = Math.max(1, height);

    const off = document.createElement('canvas');
    off.width = this.fbWidth;
    off.height = this.fbHeight;
    const offCtx = off.getContext('2d', { willReadFrequently: true });
    if (offCtx === null) {
      throw new Error('Failed to create backing 2D canvas context');
    }
    offCtx.fillStyle = '#090d16';
    offCtx.fillRect(0, 0, this.fbWidth, this.fbHeight);

    this.backingCanvas = off;
    this.backingCtx = offCtx;
    this.rowImageData = null;
  }

  public applyDirtyUpdate(update: FramebufferDirtyUpdate): void {
    if (this.destroyed || update.yMax <= update.yMin) {
      return;
    }
    this.ensureBackingSurface(update.width, update.height);
    const bCtx = this.backingCtx;
    if (bCtx === null) {
      return;
    }

    const rowCount = update.yMax - update.yMin;
    const pixelCount = update.width * rowCount;
    if (
      this.rowImageData === null ||
      this.rowImageData.width !== update.width ||
      this.rowImageData.height !== rowCount
    ) {
      this.rowImageData = bCtx.createImageData(update.width, rowCount);
    }

    convertBgrxToRgba(update.pixels, pixelCount, this.rowImageData.data);
    bCtx.putImageData(this.rowImageData, 0, update.yMin);

    this.dirtyUploads += 1;
    this.uploadedBytes += update.pixels.byteLength;
    this.present();
  }

  public present(): void {
    if (this.destroyed || this.ctx === null || this.backingCanvas === null) {
      return;
    }
    const ctx = this.ctx;
    ctx.fillStyle = '#05080f';
    ctx.fillRect(0, 0, this.viewportWidth, this.viewportHeight);

    const rect = computeAspectFitRect(
      this.fbWidth,
      this.fbHeight,
      this.viewportWidth,
      this.viewportHeight,
    );
    ctx.imageSmoothingEnabled = this.filter === 'smooth';
    ctx.drawImage(
      this.backingCanvas,
      0,
      0,
      this.fbWidth,
      this.fbHeight,
      rect.x,
      rect.y,
      rect.width,
      rect.height,
    );

    if (this.filter === 'crt') {
      ctx.fillStyle = 'rgba(0, 0, 0, 0.18)';
      for (let y = rect.y; y < rect.y + rect.height; y += 3) {
        ctx.fillRect(rect.x, y, rect.width, 1);
      }
    }
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
      backend: 'canvas2d',
      adapterInfo: 'HTML5 Canvas 2D Software Rasterizer',
      fbWidth: this.fbWidth,
      fbHeight: this.fbHeight,
      viewportWidth: this.viewportWidth,
      viewportHeight: this.viewportHeight,
      framesPresented: this.framesPresented,
      dirtyUploads: this.dirtyUploads,
      uploadedBytes: this.uploadedBytes,
      deviceLostRecoveries: 0,
      filter: this.filter,
    };
  }

  public readRgbaPixels(): Promise<Uint8Array> {
    if (this.backingCtx === null) {
      return Promise.resolve(new Uint8Array(0));
    }
    const img = this.backingCtx.getImageData(0, 0, this.fbWidth, this.fbHeight);
    return Promise.resolve(new Uint8Array(img.data.buffer.slice(0)));
  }

  public simulateDeviceLoss(): Promise<void> {
    this.present();
    return Promise.resolve();
  }

  public destroy(): void {
    this.destroyed = true;
    this.ctx = null;
    this.backingCtx = null;
    this.backingCanvas = null;
    this.rowImageData = null;
  }
}
