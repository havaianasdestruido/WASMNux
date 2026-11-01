/**
 * Shared types and pure pixel-conversion utilities for WebGPU & Canvas2D framebuffer renderers.
 */

import type { FramebufferDirtyUpdate } from '../wasm/bindings';

export type FramebufferRendererBackend = 'webgpu' | 'canvas2d';
export type RendererDisplayFilter = 'crisp' | 'smooth' | 'crt';

export interface FramebufferRendererMetrics {
  readonly backend: FramebufferRendererBackend;
  readonly adapterInfo: string;
  readonly fbWidth: number;
  readonly fbHeight: number;
  readonly viewportWidth: number;
  readonly viewportHeight: number;
  readonly framesPresented: number;
  readonly dirtyUploads: number;
  readonly uploadedBytes: number;
  readonly deviceLostRecoveries: number;
  readonly filter: RendererDisplayFilter;
}

export interface FramebufferRenderer {
  readonly backend: FramebufferRendererBackend;
  init(): Promise<void>;
  applyDirtyUpdate(update: FramebufferDirtyUpdate): void;
  present(): void;
  resizeViewport(cssWidth: number, cssHeight: number, devicePixelRatio?: number): void;
  setFilter(filter: RendererDisplayFilter): void;
  getMetrics(): FramebufferRendererMetrics;
  readRgbaPixels(): Promise<Uint8Array>;
  simulateDeviceLoss(): Promise<void>;
  destroy(): void;
}

/**
 * Convert guest `SimpleFB` `XRGB8888` (`u32` `0x00RRGGBB` stored little-endian as `[B, G, R, X]`)
 * scanlines into tightly packed `RGBA8888` (`[R, G, B, 255]`).
 */
export function convertBgrxToRgba(
  bgrx: Uint8Array,
  pixelCount: number,
  target?: Uint8ClampedArray,
): Uint8ClampedArray {
  const byteLen = pixelCount * 4;
  if (bgrx.byteLength < byteLen) {
    throw new RangeError(
      `BGRX buffer (${String(bgrx.byteLength)} B) smaller than ${String(byteLen)} B`,
    );
  }
  const out =
    target !== undefined && target.byteLength === byteLen ? target : new Uint8ClampedArray(byteLen);

  for (let i = 0; i < byteLen; i += 4) {
    const b = bgrx[i] ?? 0;
    const g = bgrx[i + 1] ?? 0;
    const r = bgrx[i + 2] ?? 0;
    out[i] = r;
    out[i + 1] = g;
    out[i + 2] = b;
    out[i + 3] = 255;
  }
  return out;
}

/**
 * Compute aspect-ratio-preserving letterbox/pillarbox destination rectangle inside a viewport.
 */
export function computeAspectFitRect(
  srcWidth: number,
  srcHeight: number,
  dstWidth: number,
  dstHeight: number,
): { readonly x: number; readonly y: number; readonly width: number; readonly height: number } {
  const safeSrcW = Math.max(1, srcWidth);
  const safeSrcH = Math.max(1, srcHeight);
  const safeDstW = Math.max(1, dstWidth);
  const safeDstH = Math.max(1, dstHeight);

  const srcAspect = safeSrcW / safeSrcH;
  const dstAspect = safeDstW / safeDstH;

  if (dstAspect > srcAspect) {
    const fitH = safeDstH;
    const fitW = Math.max(1, Math.round(fitH * srcAspect));
    return {
      x: Math.floor((safeDstW - fitW) / 2),
      y: 0,
      width: fitW,
      height: fitH,
    };
  }
  const fitW = safeDstW;
  const fitH = Math.max(1, Math.round(fitW / srcAspect));
  return {
    x: 0,
    y: Math.floor((safeDstH - fitH) / 2),
    width: fitW,
    height: fitH,
  };
}
