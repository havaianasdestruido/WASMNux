/**
 * Dedicated Web Worker entrypoint running the WASMNux Rust/WASM virtualization engine off the UI thread.
 */

import { VmEngineHost } from './vm-engine-host';
import type { WorkerMessage } from './protocol';

interface DedicatedWorkerScope {
  onmessage: ((ev: MessageEvent<unknown>) => void) | null;
  postMessage: (message: WorkerMessage, transfer?: Transferable[]) => void;
}

const workerSelf = globalThis as unknown as DedicatedWorkerScope;

const host = new VmEngineHost((msg, transfer) => {
  if (transfer && transfer.length > 0) {
    workerSelf.postMessage(msg, transfer);
  } else {
    workerSelf.postMessage(msg);
  }
});

workerSelf.onmessage = (event: MessageEvent<unknown>): void => {
  void host.handleRawMessage(event.data);
};
