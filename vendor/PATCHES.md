# Vendored emulator patches

Upstream: https://github.com/evanphx/riscv-emu at `0a320bc4d8009eec2438b7544747c22aa121781f` (MIT). Only the RV64 interpreter, devices, and snapshot support needed by WASMNux are vendored under `vendor/riscv-emu`; the native JIT, CLI, and unrelated devices are omitted.

Local changes (all required for a bounded browser worker and genuine Linux boot):

1. **WFI and bounded execution slices.** `Cpu::step`/`run` return to the host on WFI and after a bounded instruction budget instead of spinning or running unbounded native loops. The browser worker drives ticks; no host thread sleeps inside WASM.
2. **Rust 1.83 / wasm32 compatibility.** Removed nightly-only and std-feature assumptions that failed the pinned AOSP Rust 1.83.0-dev toolchain; replaced host-time calls with the injected `wnx_wall_time_ns` import.
3. **JIT removal.** The x86-64 JIT is not compiled for wasm32; interpreter-only execution is the sole browser path. No claim of guest CPU acceleration is made.
4. **FDT padding and board nodes.** Device tree generation pads to alignment and emits the WASMNux memory, CLINT, PLIC, UART, VirtIO block/entropy, syscon, reserved-memory, and simple-framebuffer nodes expected by the pinned kernel.
5. **Mmio/Clint/Bus tick forwarding.** `Mmio::tick`, CLINT, and Bus forward timer ticks so Linux timer interrupts advance under bounded worker slices.
6. **VirtIO queue completion fix.** Upstream `virtio/mmio.rs::process_queue` treated `Ok(0)` from a device as an asynchronous/pending result, swallowing synchronous completions (including block FLUSH). The vendored path distinguishes completed descriptors from genuine async backpressure; guest writes/flushes are acknowledged only after the host block backend succeeds.
7. **Snapshot save/load hooks.** CPU/RAM/device state is serialized through the crate's snapshot module; VirtIO block/entropy devices implement quiesce/save/load with no hidden host state. `Machine::save_state` quiesces, serializes, then resumes devices so a snapshot does not leave the guest paused.
8. **Memory-Box / empty-initrd fixes.** Corrected guest RAM boxing and skipped zero-length initrd placement so OpenSBI/Linux do not fault on an empty DTB memory region.

Every patch is exercised by the real kernel/Debian boots recorded in `docs/TEST_REPORT.md`; no patch replaces guest execution with host command simulation.
