//! RISC-V Control and Status Register (CSR) file with spec-compliant aliasing and privilege checks.

use crate::trap::{PrivMode, Trap};

pub const CSR_FFLAGS: u16 = 0x001;
pub const CSR_FRM: u16 = 0x002;
pub const CSR_FCSR: u16 = 0x003;

pub const CSR_SSTATUS: u16 = 0x100;
pub const CSR_SIE: u16 = 0x104;
pub const CSR_STVEC: u16 = 0x105;
pub const CSR_SCOUNTEREN: u16 = 0x106;
pub const CSR_SENVCFG: u16 = 0x10A;
pub const CSR_SSCRATCH: u16 = 0x140;
pub const CSR_SEPC: u16 = 0x141;
pub const CSR_SCAUSE: u16 = 0x142;
pub const CSR_STVAL: u16 = 0x143;
pub const CSR_SIP: u16 = 0x144;
pub const CSR_STIMECMP: u16 = 0x14D;
pub const CSR_SATP: u16 = 0x180;

pub const CSR_MSTATUS: u16 = 0x300;
pub const CSR_MISA: u16 = 0x301;
pub const CSR_MEDELEG: u16 = 0x302;
pub const CSR_MIDELEG: u16 = 0x303;
pub const CSR_MIE: u16 = 0x304;
pub const CSR_MTVEC: u16 = 0x305;
pub const CSR_MCOUNTEREN: u16 = 0x306;
pub const CSR_MENVCFG: u16 = 0x30A;
pub const CSR_MSCRATCH: u16 = 0x340;
pub const CSR_MEPC: u16 = 0x341;
pub const CSR_MCAUSE: u16 = 0x342;
pub const CSR_MTVAL: u16 = 0x343;
pub const CSR_MIP: u16 = 0x344;

pub const CSR_CYCLE: u16 = 0xC00;
pub const CSR_TIME: u16 = 0xC01;
pub const CSR_INSTRET: u16 = 0xC02;

pub const CSR_MVENDORID: u16 = 0xF11;
pub const CSR_MARCHID: u16 = 0xF12;
pub const CSR_MIMPID: u16 = 0xF13;
pub const CSR_MHARTID: u16 = 0xF14;

pub const SSTATUS_SIE: u64 = 1 << 1;
pub const SSTATUS_SPIE: u64 = 1 << 5;
pub const SSTATUS_UBE: u64 = 1 << 6;
pub const SSTATUS_SPP: u64 = 1 << 8;
pub const SSTATUS_VS_MASK: u64 = 0b11 << 9;
pub const SSTATUS_FS_MASK: u64 = 0b11 << 13;
pub const SSTATUS_FS_INITIAL: u64 = 0b01 << 13;
pub const SSTATUS_FS_DIRTY: u64 = 0b11 << 13;
pub const SSTATUS_XS_MASK: u64 = 0b11 << 15;
pub const SSTATUS_SUM: u64 = 1 << 18;
pub const SSTATUS_MXR: u64 = 1 << 19;
pub const SSTATUS_UXL_64: u64 = 2u64 << 32;
pub const SSTATUS_SD: u64 = 1u64 << 63;

pub const SSTATUS_WRITABLE_MASK: u64 =
    SSTATUS_SIE | SSTATUS_SPIE | SSTATUS_SPP | SSTATUS_FS_MASK | SSTATUS_SUM | SSTATUS_MXR;
pub const SSTATUS_READ_MASK: u64 = SSTATUS_WRITABLE_MASK
    | SSTATUS_UBE
    | SSTATUS_VS_MASK
    | SSTATUS_XS_MASK
    | SSTATUS_UXL_64
    | SSTATUS_SD;

pub const SIE_SSIE: u64 = 1 << 1;
pub const SIE_STIE: u64 = 1 << 5;
pub const SIE_SEIE: u64 = 1 << 9;
pub const S_INT_MASK: u64 = SIE_SSIE | SIE_STIE | SIE_SEIE;

/// Default MISA extension bits for `RV64IMAFDCSU`.
pub const MISA_EXT_RV64GC: u64 = (1 << 0)  // A
    | (1 << 2)  // C
    | (1 << 3)  // D
    | (1 << 5)  // F
    | (1 << 8)  // I
    | (1 << 12) // M
    | (1 << 18) // S
    | (1 << 20); // U

#[derive(Clone, Debug)]
pub struct CsrSnapshot {
    pub regs: Vec<u64>,
}

pub struct CsrFile {
    regs: [u64; 4096],
}

impl CsrFile {
    pub fn new(hart_id: usize, ext_mask: u64) -> Self {
        let mut s = Self { regs: [0u64; 4096] };
        s.reset(hart_id, ext_mask);
        s
    }

    pub fn reset(&mut self, hart_id: usize, ext_mask: u64) {
        self.regs = [0u64; 4096];
        self.regs[CSR_MHARTID as usize] = hart_id as u64;
        self.regs[CSR_MISA as usize] = (2u64 << 62) | ext_mask;
        // Enable UXL=64, SXL=64, FS=Initial so S-mode boot code can use FP immediately,
        // and enable CY/TM/IR in mcounteren/scounteren for fast vDSO rdtime.
        let init_mstatus = (2u64 << 32) | (2u64 << 34) | SSTATUS_FS_INITIAL;
        self.regs[CSR_MSTATUS as usize] = init_mstatus;
        self.regs[CSR_MCOUNTEREN as usize] = 0x7;
        self.regs[CSR_SCOUNTEREN as usize] = 0x7;
        self.regs[CSR_MEDELEG as usize] = 0xB1FF;
        self.regs[CSR_MIDELEG as usize] = S_INT_MASK;
        self.regs[CSR_STIMECMP as usize] = u64::MAX;
    }

    #[inline]
    fn sync_sd(&mut self) {
        let mut m = self.regs[CSR_MSTATUS as usize];
        if (m & SSTATUS_FS_MASK) == SSTATUS_FS_DIRTY {
            m |= SSTATUS_SD;
        } else {
            m &= !SSTATUS_SD;
        }
        self.regs[CSR_MSTATUS as usize] = m;
    }

    #[inline]
    pub fn fs_enabled(&self) -> bool {
        (self.regs[CSR_MSTATUS as usize] & SSTATUS_FS_MASK) != 0
    }

    #[inline]
    pub fn mark_fs_dirty(&mut self) {
        self.regs[CSR_MSTATUS as usize] |= SSTATUS_FS_DIRTY | SSTATUS_SD;
    }

    #[inline]
    pub fn sstatus(&self) -> u64 {
        self.regs[CSR_MSTATUS as usize] & SSTATUS_READ_MASK
    }

    pub fn set_sstatus(&mut self, val: u64) {
        let m = self.regs[CSR_MSTATUS as usize];
        self.regs[CSR_MSTATUS as usize] =
            (m & !SSTATUS_WRITABLE_MASK) | (val & SSTATUS_WRITABLE_MASK);
        self.sync_sd();
    }

    /// Direct internal CSR read (no privilege checks).
    #[inline]
    pub fn read_raw(&self, csr: u16) -> u64 {
        match csr {
            CSR_FFLAGS => self.regs[CSR_FCSR as usize] & 0x1F,
            CSR_FRM => (self.regs[CSR_FCSR as usize] >> 5) & 0x7,
            CSR_FCSR => self.regs[CSR_FCSR as usize] & 0xFF,
            CSR_SSTATUS => self.sstatus(),
            CSR_SIE => self.regs[CSR_MIE as usize] & S_INT_MASK,
            CSR_SIP => self.regs[CSR_MIP as usize] & S_INT_MASK,
            _ => self.regs[csr as usize],
        }
    }

    /// Direct internal CSR write (no privilege checks).
    pub fn write_raw(&mut self, csr: u16, value: u64) {
        if (csr & 0xC00) == 0xC00 {
            return;
        }
        match csr {
            CSR_FFLAGS => {
                let cur = self.regs[CSR_FCSR as usize] & !0x1F;
                self.regs[CSR_FCSR as usize] = cur | (value & 0x1F);
                self.mark_fs_dirty();
            }
            CSR_FRM => {
                let cur = self.regs[CSR_FCSR as usize] & !(0x7 << 5);
                self.regs[CSR_FCSR as usize] = cur | ((value & 0x7) << 5);
                self.mark_fs_dirty();
            }
            CSR_FCSR => {
                self.regs[CSR_FCSR as usize] = value & 0xFF;
                self.mark_fs_dirty();
            }
            CSR_SSTATUS => {
                self.set_sstatus(value);
            }
            CSR_MSTATUS => {
                let preserve_xl = (2u64 << 32) | (2u64 << 34);
                self.regs[CSR_MSTATUS as usize] = (value & !((0b1111u64) << 32)) | preserve_xl;
                self.sync_sd();
            }
            CSR_SIE => {
                let m = self.regs[CSR_MIE as usize];
                self.regs[CSR_MIE as usize] = (m & !S_INT_MASK) | (value & S_INT_MASK);
            }
            CSR_SIP => {
                // Only SSIP is software-writable via SIP in S-mode; STIP/SEIP are driven by timer/PLIC,
                // though raw internal callers use set_sip_bits / clear_sip_bits.
                let m = self.regs[CSR_MIP as usize];
                self.regs[CSR_MIP as usize] = (m & !SIE_SSIE) | (value & SIE_SSIE);
            }
            CSR_SEPC | CSR_MEPC => {
                let has_c = ((self.regs[CSR_MISA as usize] >> 2) & 1) != 0;
                let align_mask = if has_c { !1u64 } else { !3u64 };
                self.regs[csr as usize] = value & align_mask;
            }
            CSR_SATP => {
                let mode = value >> 60;
                // Support Bare (0) and Sv39 (8); ignore unsupported WARL probes cleanly.
                if mode == 0 || mode == 8 {
                    self.regs[CSR_SATP as usize] = value;
                }
            }
            _ => {
                self.regs[csr as usize] = value;
            }
        }
    }

    /// Architecturally checked CSR access from instruction stream.
    pub fn check_access(
        &self,
        priv_mode: PrivMode,
        csr: u16,
        will_write: bool,
        instr: u32,
    ) -> Result<(), Trap> {
        let required_priv = (csr >> 8) & 0x3;
        if (priv_mode as u16) < required_priv {
            return Err(Trap::IllegalInstruction(instr));
        }
        if will_write && ((csr >> 10) & 0x3) == 0b11 {
            return Err(Trap::IllegalInstruction(instr));
        }
        if matches!(csr, CSR_FFLAGS | CSR_FRM | CSR_FCSR) && !self.fs_enabled() {
            return Err(Trap::IllegalInstruction(instr));
        }
        // Check counteren for cycle (0xC00), time (0xC01), instret (0xC02) and HPM counters (0xC03..0xC1F).
        if (0xC00..=0xC1F).contains(&csr) {
            let bit = csr - 0xC00;
            if priv_mode != PrivMode::Machine {
                let mcen = self.regs[CSR_MCOUNTEREN as usize];
                if ((mcen >> bit) & 1) == 0 {
                    return Err(Trap::IllegalInstruction(instr));
                }
            }
            if priv_mode == PrivMode::User {
                let scen = self.regs[CSR_SCOUNTEREN as usize];
                if ((scen >> bit) & 1) == 0 {
                    return Err(Trap::IllegalInstruction(instr));
                }
            }
        }
        Ok(())
    }

    #[inline]
    pub fn set_mip_bit(&mut self, mask: u64, active: bool) {
        if active {
            self.regs[CSR_MIP as usize] |= mask;
        } else {
            self.regs[CSR_MIP as usize] &= !mask;
        }
    }

    #[inline]
    pub fn set_time(&mut self, time: u64) {
        self.regs[CSR_TIME as usize] = time;
        self.regs[CSR_CYCLE as usize] = time;
    }

    #[inline]
    pub fn time(&self) -> u64 {
        self.regs[CSR_TIME as usize]
    }

    #[inline]
    pub fn increment_instret(&mut self, delta: u64) {
        self.regs[CSR_INSTRET as usize] = self.regs[CSR_INSTRET as usize].wrapping_add(delta);
    }

    pub fn snapshot(&self) -> CsrSnapshot {
        CsrSnapshot {
            regs: self.regs.to_vec(),
        }
    }

    pub fn restore(&mut self, snap: &CsrSnapshot) -> Result<(), &'static str> {
        if snap.regs.len() != self.regs.len() {
            return Err("CSR snapshot length mismatch");
        }
        self.regs.copy_from_slice(&snap.regs);
        Ok(())
    }
}
