import { describe, expect, it } from 'vitest';
import { computeAspectFitRect, convertBgrxToRgba } from '../../src/gpu/types';
import { CONVERT_BGRX_TO_RGBA_WGSL, PRESENT_FRAMEBUFFER_WGSL } from '../../src/gpu/webgpu-renderer';

describe('Framebuffer Pixel Conversion & Aspect-Ratio Geometry (Phase 7)', () => {
  it('converts little-endian XRGB8888 (BGRX) scanlines to opaque RGBA8888 accurately', () => {
    // Two pixels: 0x0038BDF8 (B=0xF8, G=0xBD, R=0x38, X=0x00) and 0x00D70A53 (B=0x53, G=0x0A, R=0xD7, X=0x00)
    const bgrx = new Uint8Array([0xf8, 0xbd, 0x38, 0x00, 0x53, 0x0a, 0xd7, 0x77]);
    const rgba = convertBgrxToRgba(bgrx, 2);

    expect(Array.from(rgba)).toEqual([
      0x38,
      0xbd,
      0xf8,
      0xff, // Pixel 0 -> R=0x38, G=0xBD, B=0xF8, A=255
      0xd7,
      0x0a,
      0x53,
      0xff, // Pixel 1 -> R=0xD7, G=0x0A, B=0x53, A=255
    ]);
  });

  it('rejects undersized BGRX buffers safely', () => {
    expect(() => convertBgrxToRgba(new Uint8Array(4), 2)).toThrow(RangeError);
  });

  it('computes pillarbox and letterbox aspect-fit rectangles accurately', () => {
    // 640x400 (16:10 = 1.6) inside 1600x800 (2.0) -> pillarboxed width 1280, x = 160
    expect(computeAspectFitRect(640, 400, 1600, 800)).toEqual({
      x: 160,
      y: 0,
      width: 1280,
      height: 800,
    });

    // 640x400 (1.6) inside 640x640 (1.0) -> letterboxed height 400, y = 120
    expect(computeAspectFitRect(640, 400, 640, 640)).toEqual({
      x: 0,
      y: 120,
      width: 640,
      height: 400,
    });
  });

  it('includes both WGSL compute conversion and presentation shader entry points', () => {
    expect(CONVERT_BGRX_TO_RGBA_WGSL).toContain('@compute @workgroup_size(16, 16, 1)');
    expect(CONVERT_BGRX_TO_RGBA_WGSL).toContain('texture_storage_2d<rgba8unorm, write>');
    expect(PRESENT_FRAMEBUFFER_WGSL).toContain('fn vs_main');
    expect(PRESENT_FRAMEBUFFER_WGSL).toContain('fn fs_main');
  });
});
