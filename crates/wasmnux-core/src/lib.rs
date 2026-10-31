//! WASMNux core: a dependency-free RISC-V 64 virtual machine that boots
//! genuine, unmodified Linux kernel binaries.
//!
//! The crate is host-agnostic (no browser/JS knowledge): the same code runs
//! in native tests and in WebAssembly via `wasmnux-wasm`. Architecture,
//! alternatives and honest limitations: `docs/ARCHITECTURE.md`.

pub mod block;
pub mod bus;
pub mod clint;
pub mod cpu;
pub mod crc;
pub mod dtb;
pub mod elf;
pub mod error;
pub mod machine;
pub mod memory;
pub mod plic;
pub mod sbi;
pub mod uart;
pub mod virtio_blk;

pub use block::{BlockBackend, VecBlockBackend};
pub use cpu::{Cpu, Privilege, StepOutcome};
pub use error::{Result, VmError};
pub use machine::{BootMode, Machine, MachineConfig, MachineState, RunReason, RunReport};

/// Core version string surfaced to the browser UI for diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
