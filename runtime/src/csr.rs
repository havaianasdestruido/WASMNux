//! RISC-V machine-mode Control and Status Registers (CSRs).
//!
//! Only the CSRs needed to boot and trap are modelled:
//! `mstatus`, `mtvec`, `mscratch`, `mepc`, `mcause`, `mtval`, `mie`, `mip`.

/// CSR addresses (RISC-V privileged spec).
pub const CSR_MSTATUS: u64 = 0x300;
pub const CSR_MTVEC: u64 = 0x301;
pub const CSR_MSCRATCH: u64 = 0x340;
pub const CSR_MEPC: u64 = 0x341;
pub const CSR_MCAUSE: u64 = 0x342;
pub const CSR_MTVAL: u64 = 0x343;
pub const CSR_MIP: u64 = 0x344;
pub const CSR_MIE: u64 = 0x304;

// mstatus bit fields
pub const MSTATUS_MIE: u64 = 1 << 3; // machine interrupt enable
pub const MSTATUS_MPIE: u64 = 1 << 7; // machine previous interrupt enable
pub const MSTATUS_MPP_MASK: u64 = 3 << 11;

// mcause values
pub const MCAUSE_INTERRUPT_MASK: u64 = 1 << 63;

// Interrupt cause codes (machine mode)
pub const IRQ_M_SOFT: u64 = 3; // machine software interrupt
pub const IRQ_M_TIMER: u64 = 7; // machine timer interrupt
pub const IRQ_M_EXT: u64 = 11; // machine external interrupt

// mip / mie bit positions
pub const MIP_MSIP: u64 = 1 << 3; // machine software interrupt pending
pub const MIP_MTIP: u64 = 1 << 7; // machine timer interrupt pending
pub const MIP_MEIP: u64 = 1 << 11; // machine external interrupt pending

/// A minimal machine-mode CSR file.
#[derive(Debug, Clone, Default)]
pub struct CsrFile {
    pub mstatus: u64,
    pub mtvec: u64,
    pub mscratch: u64,
    pub mepc: u64,
    pub mcause: u64,
    pub mtval: u64,
    pub mie: u64,
    pub mip: u64,
    /// Retired instruction count (mcycle/minstret proxy).
    pub instret: u64,
}

impl CsrFile {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a CSR. Returns `None` for unimplemented/read-only CSRs (reads as 0).
    pub fn read(&self, csr: u64) -> u64 {
        match csr {
            CSR_MSTATUS => self.mstatus,
            CSR_MTVEC => self.mtvec,
            CSR_MSCRATCH => self.mscratch,
            CSR_MEPC => self.mepc,
            CSR_MCAUSE => self.mcause,
            CSR_MTVAL => self.mtval,
            CSR_MIP => self.mip,
            CSR_MIE => self.mie,
            _ => 0,
        }
    }

    /// Write a CSR.
    pub fn write(&mut self, csr: u64, value: u64) {
        match csr {
            CSR_MSTATUS => {
                // Only model the bits we care about; preserve reserved bits as-is.
                self.mstatus = value & (MSTATUS_MIE | MSTATUS_MPIE | MSTATUS_MPP_MASK);
            }
            CSR_MTVEC => self.mtvec = value,
            CSR_MSCRATCH => self.mscratch = value,
            CSR_MEPC => self.mepc = value,
            CSR_MCAUSE => self.mcause = value,
            CSR_MTVAL => self.mtval = value,
            CSR_MIP => self.mip = value,
            CSR_MIE => self.mie = value,
            _ => {}
        }
    }

    /// Machine-mode interrupt enable (MIE bit of mstatus).
    pub fn mie_global(&self) -> bool {
        self.mstatus & MSTATUS_MIE != 0
    }

    /// Does any enabled interrupt have a pending bit set?
    pub fn any_pending_enabled(&self) -> bool {
        let pending = self.mip & self.mie;
        pending != 0
    }

    /// Select the highest-priority pending+enabled machine interrupt, or None.
    pub fn pending_interrupt(&self) -> Option<u64> {
        let pending = self.mip & self.mie;
        if pending & MIP_MEIP != 0 {
            return Some(IRQ_M_EXT);
        }
        if pending & MIP_MTIP != 0 {
            return Some(IRQ_M_TIMER);
        }
        if pending & MIP_MSIP != 0 {
            return Some(IRQ_M_SOFT);
        }
        None
    }

    /// Raise an interrupt by setting its pending bit.
    pub fn raise_interrupt(&mut self, cause: u64) {
        match cause {
            IRQ_M_SOFT => self.mip |= MIP_MSIP,
            IRQ_M_TIMER => self.mip |= MIP_MTIP,
            IRQ_M_EXT => self.mip |= MIP_MEIP,
            _ => {}
        }
    }

    /// Clear an interrupt pending bit.
    pub fn clear_interrupt(&mut self, cause: u64) {
        match cause {
            IRQ_M_SOFT => self.mip &= !MIP_MSIP,
            IRQ_M_TIMER => self.mip &= !MIP_MTIP,
            IRQ_M_EXT => self.mip &= !MIP_MEIP,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defs_writable() {
        let mut csr = CsrFile::new();
        csr.mie |= MIP_MTIP;
        csr.mip |= MIP_MTIP;
        csr.mstatus |= MSTATUS_MIE;
        assert!(csr.mie_global());
        assert!(csr.any_pending_enabled());
        assert_eq!(csr.pending_interrupt(), Some(IRQ_M_TIMER));
        csr.clear_interrupt(IRQ_M_TIMER);
        assert_eq!(csr.pending_interrupt(), None);
    }

    #[test]
    fn interrupt_priority_ext_over_timer() {
        let mut csr = CsrFile::new();
        csr.mie = MIP_MTIP | MIP_MEIP;
        csr.mip = MIP_MTIP | MIP_MEIP;
        csr.mstatus |= MSTATUS_MIE;
        assert_eq!(csr.pending_interrupt(), Some(IRQ_M_EXT));
    }

    #[test]
    fn read_write_csr_roundtrip() {
        let mut csr = CsrFile::new();
        csr.write(CSR_MEPC, 0x8000_1000);
        csr.write(CSR_MTVEC, 0x8000_0000);
        assert_eq!(csr.read(CSR_MEPC), 0x8000_1000);
        assert_eq!(csr.read(CSR_MTVEC), 0x8000_0000);
        assert_eq!(csr.read(0xDEAD), 0); // unimplemented reads as 0
    }
}
