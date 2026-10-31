# Vendored rsemu CPU and device core

Source: https://github.com/evanphx/riscv-emu
Revision: 0a320bc4d8009eec2438b7544747c22aa121781f (2026-10-17)
License: MIT (LICENSE, preserved verbatim).

Only the CPU, ISA, interrupt/timer/UART and VirtIO transport files used by this
project are included. Native JIT, host syscall emulation, GUI, networking,
files and thread orchestration are not browser dependencies. Local changes
are documented in PATCHES.md and can be audited against the pinned revision.
