//! Host-side error type. Guest faults are **not** host errors: they are
//! delivered to the guest as RISC-V traps by the CPU core.

use std::fmt;

/// Errors surfaced to the host (TypeScript / test harness).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmError {
    /// A physical address or length fell outside the target region.
    OutOfBounds { addr: u64, len: usize },
    /// An access size the device does not support was requested.
    UnsupportedAccessSize { size: usize },
    /// The ELF image failed validation.
    InvalidElf(&'static str),
    /// A kernel / initrd / disk image failed validation.
    InvalidImage(String),
    /// A lifecycle transition was attempted from an illegal state.
    IllegalTransition {
        from: &'static str,
        action: &'static str,
    },
    /// The machine configuration is invalid.
    InvalidConfig(String),
    /// A block backend operation failed.
    Block(String),
}

impl fmt::Display for VmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds { addr, len } => {
                write!(f, "out-of-bounds access at {addr:#x} (len {len})")
            }
            Self::UnsupportedAccessSize { size } => write!(f, "unsupported access size {size}"),
            Self::InvalidElf(why) => write!(f, "invalid ELF image: {why}"),
            Self::InvalidImage(why) => write!(f, "invalid image: {why}"),
            Self::IllegalTransition { from, action } => {
                write!(f, "cannot {action} while machine is {from}")
            }
            Self::InvalidConfig(why) => write!(f, "invalid machine configuration: {why}"),
            Self::Block(why) => write!(f, "block backend error: {why}"),
        }
    }
}

impl std::error::Error for VmError {}

/// Convenience result alias used across the core.
pub type Result<T> = std::result::Result<T, VmError>;
