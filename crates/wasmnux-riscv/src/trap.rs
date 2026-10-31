//! RISC-V privilege levels, memory access types, architectural traps, and VM exit reasons.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PrivMode {
    User = 0,
    Supervisor = 1,
    Machine = 3,
}

impl PrivMode {
    pub fn from_bits(bits: u8) -> Self {
        match bits {
            0 => Self::User,
            1 => Self::Supervisor,
            _ => Self::Machine,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    Fetch,
    Load,
    Store,
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trap {
    IllegalInstruction(u32),
    Ebreak,
    Ecall,
    PageFault { addr: u64, kind: AccessType },
    MisalignedAccess { addr: u64, kind: AccessType },
    MemoryOutOfBounds { addr: u64, kind: AccessType },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemResetAction {
    PowerOff,
    ColdReboot,
    WarmReboot,
}
