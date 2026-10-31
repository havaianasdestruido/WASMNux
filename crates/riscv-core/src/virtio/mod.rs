#[path = "../../../../vendor/riscv-emu/virtio/mmio.rs"]
mod mmio;
pub use mmio::*;
pub mod block;
mod entropy;
pub use entropy::VirtioEntropy;
