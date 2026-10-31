//! `wasmnux-core` — a pure-`std` RISC-V RV64 emulator core compiled to
//! WebAssembly for WASMNux.
//!
//! No external crates are used (the build sandbox cannot reach crates.io).
//! WASM/TypeScript interop is through the raw WebAssembly JavaScript API
//! (exported `extern "C"` functions plus the exported WASM linear memory),
//! which is an equivalent stable interoperability mechanism to `wasm-bindgen`.

pub mod bus;
pub mod cpu;
pub mod csr;
pub mod device_tree;
pub mod elf;
pub mod machine;
pub mod memory;
pub mod timer;
pub mod uart;
pub mod wasm_exports;

pub use bus::Bus;
pub use cpu::{ControlFlow, Cpu, TrapCode};
pub use csr::{CsrFile, IRQ_M_EXT, IRQ_M_SOFT, IRQ_M_TIMER, MCAUSE_INTERRUPT_MASK};
pub use device_tree::FdtBuilder;
pub use elf::{load_elf64, ElfError};
pub use machine::{Machine, MachineConfig, MachineError, StepResult};
pub use memory::{GuestMemory, MemoryError};
pub use timer::Clint;
pub use uart::Uart;
