//! RV64 Linux machine. CPU implementation is vendored under vendor/riscv-emu.
#![allow(clippy::all)]
pub mod clock;
#[path = "../../../vendor/riscv-emu/cpu/mod.rs"]
pub mod cpu;
#[path = "dev.rs"]
pub mod dev;
#[path = "../../../vendor/riscv-emu/fdt.rs"]
pub mod fdt;
#[path = "../../../vendor/riscv-emu/isa/mod.rs"]
pub mod isa;
#[path = "../../../vendor/riscv-emu/logger.rs"]
pub mod logger;
pub mod machine;
pub mod memory;
#[path = "../../../vendor/riscv-emu/mmu.rs"]
pub mod mmu;
pub mod snapshot;
#[path = "virtio/mod.rs"]
pub mod virtio;
pub use machine::{
    BlockDevice, Console, Entropy, Machine, MachineConfig, MachineError, FB_BASE, FB_HEIGHT,
    FB_SIZE, FB_WIDTH,
};
