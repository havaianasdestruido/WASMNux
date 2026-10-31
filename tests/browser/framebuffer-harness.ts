import { FramebufferPresenter } from "../../src/graphics/framebuffer";

const display = document.getElementById("display");
if (!display) throw new Error("Missing framebuffer harness");
let presenter: FramebufferPresenter | undefined;
let capturedDevice: GPUDevice | undefined;
let lastChange = "";

function current(): FramebufferPresenter {
  if (!presenter) throw new Error("Presenter not initialized");
  return presenter;
}

const api = {
  async start(forceCanvas: boolean, captureDevice = false) {
    presenter?.dispose();
    capturedDevice = undefined;
    lastChange = "";
    const gpu = navigator.gpu;
    const original = gpu?.requestAdapter.bind(gpu);
    if (gpu && original && captureDevice) {
      gpu.requestAdapter = async (options?: GPURequestAdapterOptions) => {
        const adapter = await original(options);
        if (adapter) {
          const requestDevice = adapter.requestDevice.bind(adapter);
          adapter.requestDevice = async (descriptor?: GPUDeviceDescriptor) => {
            capturedDevice = await requestDevice(descriptor);
            return capturedDevice;
          };
        }
        return adapter;
      };
    }
    try {
      presenter = await FramebufferPresenter.create(display, {
        forceCanvas,
        onBackendChange: (kind, reason) => {
          lastChange = `${kind}: ${reason}`;
        },
      });
      return presenter.kind;
    } finally {
      if (gpu && original && captureDevice) gpu.requestAdapter = original;
    }
  },
  async draw(width: number, height: number, bytes: number[]) {
    await current().present(width, height, Uint8Array.from(bytes));
  },
  async pixels() {
    const canvas = current().canvas;
    const image = await createImageBitmap(canvas);
    const copy = document.createElement("canvas");
    copy.width = canvas.width;
    copy.height = canvas.height;
    const context = copy.getContext("2d");
    if (!context) throw new Error("No test canvas context");
    context.drawImage(image, 0, 0);
    image.close();
    return Array.from(context.getImageData(0, 0, copy.width, copy.height).data);
  },
  kind() {
    return current().kind;
  },
  change() {
    return lastChange;
  },
  lose() {
    if (!capturedDevice) throw new Error("No captured WebGPU device");
    capturedDevice.destroy();
  },
  dispose() {
    current().dispose();
  },
  canvasCount() {
    return display.querySelectorAll("canvas").length;
  },
};

declare global {
  interface Window {
    frameTest: typeof api;
  }
}
window.frameTest = api;
