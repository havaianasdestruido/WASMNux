//! `RV64IMAFDC` + `Zicsr` + `Zifencei` + `Zba` + `Zbb` Hart + hardened `Sv39` MMU & TLB.

use crate::bus::MachineBus;
use crate::csr::{
    CsrFile, CsrSnapshot, CSR_CYCLE, CSR_FCSR, CSR_INSTRET, CSR_MCAUSE, CSR_MEDELEG, CSR_MEPC,
    CSR_MIDELEG, CSR_MIE, CSR_MIP, CSR_MSTATUS, CSR_MTVAL, CSR_MTVEC, CSR_SATP, CSR_SCAUSE,
    CSR_SEPC, CSR_STIMECMP, CSR_STVAL, CSR_STVEC, CSR_TIME, MISA_EXT_RV64GC, SIE_SEIE, SIE_SSIE,
    SIE_STIE, SSTATUS_MXR, SSTATUS_SIE, SSTATUS_SPIE, SSTATUS_SPP, SSTATUS_SUM,
};
use crate::sbi::VirtualSbi;
use crate::trap::{AccessType, PrivMode, SystemResetAction, Trap};

const PAGE_SHIFT: u64 = 12;
const PAGE_SIZE: u64 = 1 << PAGE_SHIFT;
const PAGE_MASK: u64 = PAGE_SIZE - 1;

const PTE_V: u64 = 1 << 0;
const PTE_R: u64 = 1 << 1;
const PTE_W: u64 = 1 << 2;
const PTE_X: u64 = 1 << 3;
const PTE_U: u64 = 1 << 4;
const PTE_A: u64 = 1 << 6;
const PTE_D: u64 = 1 << 7;
const PTE_PPN_MASK: u64 = (1u64 << 44) - 1;

const TLB_BITS: usize = 9;
const TLB_SIZE: usize = 1 << TLB_BITS;
const TLB_MASK: usize = TLB_SIZE - 1;

#[derive(Clone, Copy, Default)]
struct TlbEntry {
    vpn_tag: u64,
    ppn_base: u64,
    flags: u8,
}

const TLB_VALID: u8 = 1 << 0;
const TLB_READ: u8 = 1 << 1;
const TLB_WRITE: u8 = 1 << 2;
const TLB_EXEC: u8 = 1 << 3;
const TLB_USER: u8 = 1 << 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    Executed,
    WfiIdle,
    Reset(SystemResetAction),
}

#[derive(Clone, Debug)]
pub struct HartSnapshot {
    pub pc: u64,
    pub regs: [u64; 32],
    pub fregs: [u64; 32],
    pub priv_mode: u8,
    pub wfi: bool,
    pub csrs: CsrSnapshot,
}

pub struct Hart {
    pub hart_id: usize,
    pub pc: u64,
    pub regs: [u64; 32],
    pub fregs: [u64; 32],
    pub csrs: CsrFile,
    pub priv_mode: PrivMode,
    pub reservation: Option<u64>,
    pub wfi: bool,
    pub sbi: VirtualSbi,
    satp_cached: u64,
    tlb: Vec<TlbEntry>,
}

impl Hart {
    pub fn new(hart_id: usize) -> Self {
        Self {
            hart_id,
            pc: 0,
            regs: [0u64; 32],
            fregs: [0u64; 32],
            csrs: CsrFile::new(hart_id, MISA_EXT_RV64GC),
            priv_mode: PrivMode::Supervisor,
            reservation: None,
            wfi: false,
            sbi: VirtualSbi::new(),
            satp_cached: 0,
            tlb: vec![TlbEntry::default(); TLB_SIZE],
        }
    }

    pub fn reset_linux_boot(&mut self, entry_pc: u64, dtb_pa: u64) {
        self.regs = [0u64; 32];
        self.fregs = [0u64; 32];
        self.csrs.reset(self.hart_id, MISA_EXT_RV64GC);
        self.pc = entry_pc;
        self.regs[10] = self.hart_id as u64; // a0 = hartid
        self.regs[11] = dtb_pa; // a1 = FDT physical address
        self.priv_mode = PrivMode::Supervisor;
        self.reservation = None;
        self.wfi = false;
        self.sbi.reset_action = None;
        self.satp_cached = 0;
        self.flush_tlb();
    }

    #[inline]
    pub fn flush_tlb(&mut self) {
        self.tlb.fill(TlbEntry::default());
    }

    pub fn snapshot(&self) -> HartSnapshot {
        HartSnapshot {
            pc: self.pc,
            regs: self.regs,
            fregs: self.fregs,
            priv_mode: self.priv_mode as u8,
            wfi: self.wfi,
            csrs: self.csrs.snapshot(),
        }
    }

    pub fn restore(&mut self, snap: &HartSnapshot) -> Result<(), &'static str> {
        self.csrs.restore(&snap.csrs)?;
        self.pc = snap.pc;
        self.regs = snap.regs;
        self.fregs = snap.fregs;
        self.regs[0] = 0;
        self.priv_mode = PrivMode::from_bits(snap.priv_mode);
        self.reservation = None;
        self.wfi = snap.wfi;
        self.satp_cached = self.csrs.read_raw(CSR_SATP);
        self.flush_tlb();
        Ok(())
    }

    #[inline]
    fn tlb_index(vpn: u64) -> usize {
        ((vpn as usize) ^ ((vpn as usize) >> TLB_BITS)) & TLB_MASK
    }

    /// Translate virtual address `vaddr` into physical address `paddr` for a single byte/aligned unit.
    #[inline]
    pub fn translate(
        &mut self,
        bus: &mut MachineBus,
        vaddr: u64,
        kind: AccessType,
    ) -> Result<u64, Trap> {
        if self.priv_mode == PrivMode::Machine {
            return Ok(vaddr);
        }
        let satp = self.satp_cached;
        let mode = satp >> 60;
        if mode == 0 {
            return Ok(vaddr);
        }
        if mode != 8 {
            return Err(Trap::PageFault { addr: vaddr, kind });
        }

        // Canonical Sv39 check: bits [63:39] must all equal bit 38.
        let sign = ((vaddr as i64) >> 38) as u64;
        if sign != 0 && sign != u64::MAX {
            return Err(Trap::PageFault { addr: vaddr, kind });
        }

        let vpn = (vaddr >> PAGE_SHIFT) & 0x7FF_FFFF;
        let idx = Self::tlb_index(vpn);
        let entry = self.tlb[idx];
        if (entry.flags & TLB_VALID) != 0 && entry.vpn_tag == vpn {
            let is_user_page = (entry.flags & TLB_USER) != 0;
            let priv_ok = match self.priv_mode {
                PrivMode::User => is_user_page,
                PrivMode::Supervisor => {
                    if !is_user_page {
                        true
                    } else if kind == AccessType::Fetch {
                        false // SMEP: Supervisor cannot execute PTE_U pages
                    } else {
                        (self.csrs.sstatus() & SSTATUS_SUM) != 0
                    }
                }
                PrivMode::Machine => true,
            };
            if priv_ok {
                let perm_ok = match kind {
                    AccessType::Fetch => (entry.flags & TLB_EXEC) != 0,
                    AccessType::Load | AccessType::Debug => {
                        (entry.flags & TLB_READ) != 0
                            || ((entry.flags & TLB_EXEC) != 0
                                && (self.csrs.sstatus() & SSTATUS_MXR) != 0)
                    }
                    AccessType::Store => (entry.flags & TLB_WRITE) != 0,
                };
                if perm_ok {
                    return Ok(entry.ppn_base | (vaddr & PAGE_MASK));
                }
            }
        }

        self.walk_sv39(bus, vaddr, vpn, satp, kind)
    }

    fn walk_sv39(
        &mut self,
        bus: &mut MachineBus,
        vaddr: u64,
        vpn: u64,
        satp: u64,
        kind: AccessType,
    ) -> Result<u64, Trap> {
        let vpns = [
            (vaddr >> 12) & 0x1FF,
            (vaddr >> 21) & 0x1FF,
            (vaddr >> 30) & 0x1FF,
        ];
        let mut table_pa = (satp & PTE_PPN_MASK) << PAGE_SHIFT;

        for level in (0..3).rev() {
            let pte_pa = table_pa + vpns[level] * 8;
            let pte = bus
                .read_phys(pte_pa, 8, AccessType::Load)
                .map_err(|_| Trap::PageFault { addr: vaddr, kind })?;

            if (pte & PTE_V) == 0 {
                return Err(Trap::PageFault { addr: vaddr, kind });
            }
            let r = (pte & PTE_R) != 0;
            let w = (pte & PTE_W) != 0;
            let x = (pte & PTE_X) != 0;

            if !r && w {
                return Err(Trap::PageFault { addr: vaddr, kind });
            }

            let ppn = (pte >> 10) & PTE_PPN_MASK;
            if !r && !x {
                if level == 0 {
                    return Err(Trap::PageFault { addr: vaddr, kind });
                }
                table_pa = ppn << PAGE_SHIFT;
                continue;
            }

            // Misaligned superpage check: lower PPN components must be zero.
            if level == 2 && (ppn & 0x3FFFF) != 0 {
                return Err(Trap::PageFault { addr: vaddr, kind });
            }
            if level == 1 && (ppn & 0x1FF) != 0 {
                return Err(Trap::PageFault { addr: vaddr, kind });
            }

            let is_user_page = (pte & PTE_U) != 0;
            match self.priv_mode {
                PrivMode::User => {
                    if !is_user_page {
                        return Err(Trap::PageFault { addr: vaddr, kind });
                    }
                }
                PrivMode::Supervisor => {
                    if is_user_page {
                        if kind == AccessType::Fetch {
                            return Err(Trap::PageFault { addr: vaddr, kind });
                        }
                        if (self.csrs.sstatus() & SSTATUS_SUM) == 0 {
                            return Err(Trap::PageFault { addr: vaddr, kind });
                        }
                    }
                }
                PrivMode::Machine => {}
            }

            let sstatus = self.csrs.sstatus();
            let mxr = (sstatus & SSTATUS_MXR) != 0;
            let allowed = match kind {
                AccessType::Fetch => x,
                AccessType::Load | AccessType::Debug => r || (x && mxr),
                AccessType::Store => w,
            };
            if !allowed {
                return Err(Trap::PageFault { addr: vaddr, kind });
            }

            // Hardware Svadu A/D updates:
            // Set PTE_A on any non-debug access, and set PTE_D ONLY on Store (when W=1).
            // Because TLB_WRITE is gated on (updated_pte & PTE_D) != 0, a Load on a clean
            // writable page (W=1, D=0) sets PTE_A=1 while leaving PTE_D=0 and TLB_WRITE=0;
            // the first subsequent Store misses TLB_WRITE, re-walks, and atomically sets PTE_D=1!
            let mut updated_pte = pte;
            if kind != AccessType::Debug {
                updated_pte |= PTE_A;
                if kind == AccessType::Store {
                    updated_pte |= PTE_D;
                }
                if updated_pte != pte {
                    bus.write_phys(pte_pa, 8, updated_pte, AccessType::Store)
                        .map_err(|_| Trap::PageFault { addr: vaddr, kind })?;
                }
            }

            let resolved_ppn = match level {
                2 => (ppn & !0x3FFFF) | (vpn & 0x3FFFF),
                1 => (ppn & !0x1FF) | (vpn & 0x1FF),
                _ => ppn,
            };
            let ppn_base = resolved_ppn << PAGE_SHIFT;

            if kind != AccessType::Debug {
                let mut flags = TLB_VALID;
                if r {
                    flags |= TLB_READ;
                }
                if w && (updated_pte & PTE_D) != 0 {
                    flags |= TLB_WRITE;
                }
                if x {
                    flags |= TLB_EXEC;
                }
                if is_user_page {
                    flags |= TLB_USER;
                }
                self.tlb[Self::tlb_index(vpn)] = TlbEntry {
                    vpn_tag: vpn,
                    ppn_base,
                    flags,
                };
            }

            return Ok(ppn_base | (vaddr & PAGE_MASK));
        }

        Err(Trap::PageFault { addr: vaddr, kind })
    }

    #[inline]
    pub fn read_mem(
        &mut self,
        bus: &mut MachineBus,
        vaddr: u64,
        size: usize,
        kind: AccessType,
    ) -> Result<u64, Trap> {
        if ((vaddr as usize) & 0xFFF) + size <= 4096 {
            let paddr = self.translate(bus, vaddr, kind)?;
            return bus.read_phys(paddr, size, kind);
        }
        // Cross-page unaligned load: translate both pages cleanly.
        let mut bytes = [0u8; 8];
        for (i, slot) in bytes.iter_mut().enumerate().take(size) {
            let va = vaddr.wrapping_add(i as u64);
            let pa = self.translate(bus, va, kind)?;
            *slot = bus.read_phys(pa, 1, kind)? as u8;
        }
        Ok(u64::from_le_bytes(bytes))
    }

    #[inline]
    pub fn write_mem(
        &mut self,
        bus: &mut MachineBus,
        vaddr: u64,
        size: usize,
        val: u64,
        kind: AccessType,
    ) -> Result<(), Trap> {
        if ((vaddr as usize) & 0xFFF) + size <= 4096 {
            let paddr = self.translate(bus, vaddr, kind)?;
            return bus.write_phys(paddr, size, val, kind);
        }
        // Cross-page unaligned store: pre-validate BOTH pages before mutating either page!
        let next_page_va = (vaddr & !PAGE_MASK).wrapping_add(PAGE_SIZE);
        let _pa0 = self.translate(bus, vaddr, kind)?;
        let _pa1 = self.translate(bus, next_page_va, kind)?;
        let bytes = val.to_le_bytes();
        for (i, &b) in bytes.iter().enumerate().take(size) {
            let va = vaddr.wrapping_add(i as u64);
            let pa = self.translate(bus, va, kind)?;
            bus.write_phys(pa, 1, u64::from(b), kind)?;
        }
        Ok(())
    }

    /// Synchronize timer (`STIP` via CLINT or `Sstc` `stimecmp`) and external (`SEIP` via PLIC) interrupt lines.
    #[inline]
    pub fn sync_interrupt_lines(&mut self, bus: &MachineBus) {
        let mtime = bus.clint.mtime;
        self.csrs.set_time(mtime);
        let stimecmp = self.csrs.read_raw(CSR_STIMECMP);
        let timer_due = bus.clint.timer_due() || mtime >= stimecmp;
        self.csrs.set_mip_bit(SIE_STIE, timer_due);
        self.csrs
            .set_mip_bit(SIE_SEIE, bus.plic.supervisor_external_pending());
    }

    fn check_pending_interrupt(&mut self) -> Option<u64> {
        let pending = self.csrs.read_raw(CSR_MIP) & self.csrs.read_raw(CSR_MIE);
        if pending == 0 {
            return None;
        }
        if pending != 0 {
            self.wfi = false;
        }

        let s_deleg = self.csrs.read_raw(CSR_MIDELEG);
        let s_pending = pending & s_deleg;
        if s_pending != 0 {
            let s_enabled = match self.priv_mode {
                PrivMode::User => true,
                PrivMode::Supervisor => (self.csrs.sstatus() & SSTATUS_SIE) != 0,
                PrivMode::Machine => false,
            };
            if s_enabled {
                if (s_pending & SIE_SEIE) != 0 {
                    return Some((1u64 << 63) | 9);
                }
                if (s_pending & SIE_SSIE) != 0 {
                    return Some((1u64 << 63) | 1);
                }
                if (s_pending & SIE_STIE) != 0 {
                    return Some((1u64 << 63) | 5);
                }
            }
        }
        None
    }

    fn take_trap(&mut self, cause: u64, tval: u64) {
        self.reservation = None;
        self.wfi = false;

        let is_interrupt = (cause >> 63) != 0;
        let code = cause & 0x7FFF_FFFF_FFFF_FFFF;
        let deleg = if is_interrupt {
            self.csrs.read_raw(CSR_MIDELEG)
        } else {
            self.csrs.read_raw(CSR_MEDELEG)
        };
        let to_supervisor = self.priv_mode != PrivMode::Machine && ((deleg >> code) & 1) != 0;

        if to_supervisor {
            self.csrs.write_raw(CSR_SEPC, self.pc);
            self.csrs.write_raw(CSR_SCAUSE, cause);
            self.csrs.write_raw(CSR_STVAL, tval);

            let mut sstatus = self.csrs.sstatus();
            if (sstatus & SSTATUS_SIE) != 0 {
                sstatus |= SSTATUS_SPIE;
            } else {
                sstatus &= !SSTATUS_SPIE;
            }
            sstatus &= !SSTATUS_SIE;
            if self.priv_mode == PrivMode::Supervisor {
                sstatus |= SSTATUS_SPP;
            } else {
                sstatus &= !SSTATUS_SPP;
            }
            self.csrs.set_sstatus(sstatus);
            self.priv_mode = PrivMode::Supervisor;

            let stvec = self.csrs.read_raw(CSR_STVEC);
            let base = stvec & !0x3;
            let mode = stvec & 0x3;
            self.pc = if is_interrupt && mode == 1 {
                base.wrapping_add(code * 4)
            } else {
                base
            };
        } else {
            self.csrs.write_raw(CSR_MEPC, self.pc);
            self.csrs.write_raw(CSR_MCAUSE, cause);
            self.csrs.write_raw(CSR_MTVAL, tval);
            let mstatus = self.csrs.read_raw(CSR_MSTATUS);
            let mpp = (self.priv_mode as u64) << 11;
            let mpie = ((mstatus >> 3) & 1) << 7;
            let next_mstatus = (mstatus & !((1 << 3) | (1 << 7) | (0b11 << 11))) | mpp | mpie;
            self.csrs.write_raw(CSR_MSTATUS, next_mstatus);
            self.priv_mode = PrivMode::Machine;

            let mtvec = self.csrs.read_raw(CSR_MTVEC);
            let base = mtvec & !0x3;
            let mode = mtvec & 0x3;
            self.pc = if is_interrupt && mode == 1 {
                base.wrapping_add(code * 4)
            } else {
                base
            };
        }
    }

    fn trap_to_cause_tval(&self, trap: Trap) -> (u64, u64) {
        match trap {
            Trap::IllegalInstruction(bits) => (2, u64::from(bits)),
            Trap::Ebreak => (3, self.pc),
            Trap::MisalignedAccess { addr, kind } => {
                let c = match kind {
                    AccessType::Fetch => 0,
                    AccessType::Load | AccessType::Debug => 4,
                    AccessType::Store => 6,
                };
                (c, addr)
            }
            Trap::MemoryOutOfBounds { addr, kind } => {
                let c = match kind {
                    AccessType::Fetch => 1,
                    AccessType::Load | AccessType::Debug => 5,
                    AccessType::Store => 7,
                };
                (c, addr)
            }
            Trap::Ecall => {
                let c = match self.priv_mode {
                    PrivMode::User => 8,
                    PrivMode::Supervisor => 9,
                    PrivMode::Machine => 11,
                };
                (c, 0)
            }
            Trap::PageFault { addr, kind } => {
                let c = match kind {
                    AccessType::Fetch => 12,
                    AccessType::Load | AccessType::Debug => 13,
                    AccessType::Store => 15,
                };
                (c, addr)
            }
        }
    }

    /// Run up to `max_steps` instructions (or until WFI idle or system reset).
    pub fn run_slice(&mut self, bus: &mut MachineBus, max_steps: usize) -> (usize, StepOutcome) {
        let mut executed = 0usize;
        while executed < max_steps {
            if let Some(action) = bus
                .syscon
                .reset_action
                .take()
                .or_else(|| self.sbi.reset_action.take())
            {
                return (executed, StepOutcome::Reset(action));
            }

            if (executed & 63) == 0 {
                self.sync_interrupt_lines(bus);
            }

            if let Some(int_cause) = self.check_pending_interrupt() {
                self.take_trap(int_cause, 0);
            } else if self.wfi {
                // Fast-forward mtime toward next timer deadline up to remaining budget, or yield to Worker.
                let stimecmp = self.csrs.read_raw(CSR_STIMECMP);
                let clint_due = bus.clint.ticks_until_due();
                let sstc_due = if stimecmp == u64::MAX {
                    None
                } else {
                    Some(stimecmp.saturating_sub(bus.clint.mtime))
                };
                let next_due = match (clint_due, sstc_due) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (Some(a), None) => Some(a),
                    (None, Some(b)) => Some(b),
                    (None, None) => None,
                };
                let remaining = (max_steps - executed) as u64;
                if let Some(ticks) = next_due {
                    if ticks <= remaining {
                        bus.clint.tick(ticks.max(1));
                        self.sync_interrupt_lines(bus);
                        if self.check_pending_interrupt().is_some() {
                            continue;
                        }
                    }
                }
                // Advance time by up to `remaining` slice ticks toward next timer and yield to host event loop.
                let step_ticks = next_due.unwrap_or(remaining).min(remaining).max(1);
                bus.clint.tick(step_ticks);
                self.sync_interrupt_lines(bus);
                return (executed, StepOutcome::WfiIdle);
            }

            match self.step_one(bus) {
                Ok(()) => {
                    self.regs[0] = 0;
                    executed += 1;
                    bus.clint.tick(1);
                    self.csrs.increment_instret(1);
                }
                Err(Trap::Ecall) if self.priv_mode == PrivMode::Supervisor => {
                    let mut sip_ssip_set = None;
                    let flush = self
                        .sbi
                        .handle_ecall(&mut self.regs, bus, &mut sip_ssip_set);
                    if let Some(active) = sip_ssip_set {
                        self.csrs.set_mip_bit(SIE_SSIE, active);
                    }
                    if flush {
                        self.flush_tlb();
                    }
                    self.regs[0] = 0;
                    self.pc = self.pc.wrapping_add(4);
                    executed += 1;
                    bus.clint.tick(1);
                    self.csrs.increment_instret(1);
                    self.sync_interrupt_lines(bus);
                }
                Err(trap) => {
                    let (cause, tval) = self.trap_to_cause_tval(trap);
                    self.take_trap(cause, tval);
                    self.regs[0] = 0;
                    executed += 1;
                    bus.clint.tick(1);
                }
            }
        }
        self.sync_interrupt_lines(bus);
        if let Some(action) = bus
            .syscon
            .reset_action
            .take()
            .or_else(|| self.sbi.reset_action.take())
        {
            return (executed, StepOutcome::Reset(action));
        }
        (executed, StepOutcome::Executed)
    }

    #[inline]
    fn fetch_instruction(&mut self, bus: &mut MachineBus) -> Result<(u32, u64), Trap> {
        let pc = self.pc;
        if (pc & 1) != 0 {
            return Err(Trap::MisalignedAccess {
                addr: pc,
                kind: AccessType::Fetch,
            });
        }
        // Fast single-page 32-bit fetch when not on the final 2 bytes of a 4 KiB page.
        if (pc & 0xFFF) <= 0xFFC {
            let pa = self.translate(bus, pc, AccessType::Fetch)?;
            let word = bus.read_phys(pa, 4, AccessType::Fetch)? as u32;
            if (word & 0x3) != 0x3 {
                return Ok((word & 0xFFFF, 2));
            }
            return Ok((word, 4));
        }
        // Halfword at 0xFFE: read low u16 first so a compressed 16-bit instruction at page end
        // never faults on the following unmapped page.
        let pa_lo = self.translate(bus, pc, AccessType::Fetch)?;
        let lo = bus.read_phys(pa_lo, 2, AccessType::Fetch)? as u32;
        if (lo & 0x3) != 0x3 {
            return Ok((lo, 2));
        }
        let pc_hi = pc.wrapping_add(2);
        let pa_hi = self.translate(bus, pc_hi, AccessType::Fetch)?;
        let hi = bus.read_phys(pa_hi, 2, AccessType::Fetch)? as u32;
        Ok((lo | (hi << 16), 4))
    }

    #[inline]
    fn require_fp(&self, raw_instr: u32) -> Result<(), Trap> {
        if !self.csrs.fs_enabled() {
            return Err(Trap::IllegalInstruction(raw_instr));
        }
        Ok(())
    }

    #[inline]
    fn f32_unbox(raw: u64) -> f32 {
        if (raw >> 32) == 0xFFFF_FFFF {
            f32::from_bits(raw as u32)
        } else {
            f32::from_bits(0x7FC0_0000)
        }
    }

    #[inline]
    fn f32_box(val: f32) -> u64 {
        let bits = if val.is_nan() {
            0x7FC0_0000
        } else {
            val.to_bits()
        };
        0xFFFF_FFFF_0000_0000 | u64::from(bits)
    }

    #[inline]
    fn f64_canon(val: f64) -> u64 {
        if val.is_nan() {
            0x7FF8_0000_0000_0000
        } else {
            val.to_bits()
        }
    }

    #[inline]
    fn round_f64(&self, v: f64, rm: u32, raw_instr: u32) -> Result<f64, Trap> {
        let mode = if rm == 0b111 {
            ((self.csrs.read_raw(CSR_FCSR) >> 5) & 0x7) as u32
        } else {
            rm
        };
        let r = match mode {
            0b000 => v.round_ties_even(), // RNE
            0b001 => v.trunc(),           // RTZ
            0b010 => v.floor(),           // RDN
            0b011 => v.ceil(),            // RUP
            0b100 => v.round(),           // RMM (ties away from zero)
            _ => return Err(Trap::IllegalInstruction(raw_instr)),
        };
        Ok(r)
    }

    fn step_one(&mut self, bus: &mut MachineBus) -> Result<(), Trap> {
        let (raw_instr, len) = self.fetch_instruction(bus)?;
        let instr = if len == 2 {
            expand_rvc(raw_instr as u16, self)?
        } else {
            raw_instr
        };

        if instr == 0 {
            // Compressed HINT / NOP fast return
            self.pc = self.pc.wrapping_add(len);
            return Ok(());
        }

        let opcode = instr & 0x7F;
        let rd = ((instr >> 7) & 0x1F) as usize;
        let funct3 = (instr >> 12) & 0x7;
        let rs1 = ((instr >> 15) & 0x1F) as usize;
        let rs2 = ((instr >> 20) & 0x1F) as usize;
        let funct7 = instr >> 25;
        let mut next_pc = self.pc.wrapping_add(len);

        match opcode {
            0x37 => {
                // LUI
                let imm = ((instr & 0xFFFF_F000) as i32) as i64 as u64;
                if rd != 0 {
                    self.regs[rd] = imm;
                }
            }
            0x17 => {
                // AUIPC
                let imm = ((instr & 0xFFFF_F000) as i32) as i64 as u64;
                if rd != 0 {
                    self.regs[rd] = self.pc.wrapping_add(imm);
                }
            }
            0x6F => {
                // JAL
                let imm20 = ((instr >> 31) & 1) << 20;
                let imm10_1 = ((instr >> 21) & 0x3FF) << 1;
                let imm11 = ((instr >> 20) & 1) << 11;
                let imm19_12 = ((instr >> 12) & 0xFF) << 12;
                let raw = (imm20 | imm19_12 | imm11 | imm10_1) as i32;
                let offset = ((raw << 11) >> 11) as i64 as u64;
                let ret = next_pc;
                next_pc = self.pc.wrapping_add(offset);
                if rd != 0 {
                    self.regs[rd] = ret;
                }
            }
            0x67 => {
                // JALR
                if funct3 != 0 {
                    return Err(Trap::IllegalInstruction(raw_instr));
                }
                let imm = ((instr as i32) >> 20) as i64 as u64;
                let target = self.regs[rs1].wrapping_add(imm) & !1;
                if rd != 0 {
                    self.regs[rd] = next_pc;
                }
                next_pc = target;
            }
            0x63 => {
                // BRANCH
                let imm12 = ((instr >> 31) & 1) << 12;
                let imm10_5 = ((instr >> 25) & 0x3F) << 5;
                let imm4_1 = ((instr >> 8) & 0xF) << 1;
                let imm11 = ((instr >> 7) & 1) << 11;
                let raw = (imm12 | imm11 | imm10_5 | imm4_1) as i32;
                let offset = ((raw << 19) >> 19) as i64 as u64;
                let a = self.regs[rs1];
                let b = self.regs[rs2];
                let take = match funct3 {
                    0 => a == b,
                    1 => a != b,
                    4 => (a as i64) < (b as i64),
                    5 => (a as i64) >= (b as i64),
                    6 => a < b,
                    7 => a >= b,
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                if take {
                    next_pc = self.pc.wrapping_add(offset);
                }
            }
            0x03 => {
                // LOAD
                let imm = ((instr as i32) >> 20) as i64 as u64;
                let addr = self.regs[rs1].wrapping_add(imm);
                let val = match funct3 {
                    0 => (self.read_mem(bus, addr, 1, AccessType::Load)? as i8) as i64 as u64,
                    1 => (self.read_mem(bus, addr, 2, AccessType::Load)? as i16) as i64 as u64,
                    2 => (self.read_mem(bus, addr, 4, AccessType::Load)? as i32) as i64 as u64,
                    3 => self.read_mem(bus, addr, 8, AccessType::Load)?,
                    4 => self.read_mem(bus, addr, 1, AccessType::Load)? & 0xFF,
                    5 => self.read_mem(bus, addr, 2, AccessType::Load)? & 0xFFFF,
                    6 => self.read_mem(bus, addr, 4, AccessType::Load)? & 0xFFFF_FFFF,
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                if rd != 0 {
                    self.regs[rd] = val;
                }
            }
            0x23 => {
                // STORE
                let imm = ((((instr >> 25) << 5) | ((instr >> 7) & 0x1F)) as i32) << 20 >> 20;
                let addr = self.regs[rs1].wrapping_add(imm as i64 as u64);
                let v = self.regs[rs2];
                match funct3 {
                    0 => self.write_mem(bus, addr, 1, v, AccessType::Store)?,
                    1 => self.write_mem(bus, addr, 2, v, AccessType::Store)?,
                    2 => self.write_mem(bus, addr, 4, v, AccessType::Store)?,
                    3 => self.write_mem(bus, addr, 8, v, AccessType::Store)?,
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                }
            }
            0x07 => {
                // LOAD-FP (FLW / FLD)
                self.require_fp(raw_instr)?;
                let imm = ((instr as i32) >> 20) as i64 as u64;
                let addr = self.regs[rs1].wrapping_add(imm);
                match funct3 {
                    2 => {
                        let bits = self.read_mem(bus, addr, 4, AccessType::Load)? & 0xFFFF_FFFF;
                        self.fregs[rd] = 0xFFFF_FFFF_0000_0000 | bits;
                    }
                    3 => {
                        self.fregs[rd] = self.read_mem(bus, addr, 8, AccessType::Load)?;
                    }
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                }
                self.csrs.mark_fs_dirty();
            }
            0x27 => {
                // STORE-FP (FSW / FSD)
                self.require_fp(raw_instr)?;
                let imm = ((((instr >> 25) << 5) | ((instr >> 7) & 0x1F)) as i32) << 20 >> 20;
                let addr = self.regs[rs1].wrapping_add(imm as i64 as u64);
                let v = self.fregs[rs2];
                match funct3 {
                    2 => self.write_mem(bus, addr, 4, v & 0xFFFF_FFFF, AccessType::Store)?,
                    3 => self.write_mem(bus, addr, 8, v, AccessType::Store)?,
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                }
            }
            0x13 => {
                // OP-IMM + Zbb immediate ops
                let imm12 = instr >> 20;
                let imm = ((instr as i32) >> 20) as i64 as u64;
                let src = self.regs[rs1];
                let shamt = imm12 & 0x3F;
                let top6 = imm12 >> 6;
                let res = match funct3 {
                    0 => src.wrapping_add(imm),
                    2 => u64::from((src as i64) < (imm as i64)),
                    3 => u64::from(src < imm),
                    4 => src ^ imm,
                    6 => src | imm,
                    7 => src & imm,
                    1 => match imm12 {
                        0x600 => u64::from(src.leading_zeros()),  // CLZ (Zbb)
                        0x601 => u64::from(src.trailing_zeros()), // CTZ (Zbb)
                        0x602 => u64::from(src.count_ones()),     // CPOP (Zbb)
                        0x604 => ((src as i8) as i64) as u64,     // SEXT.B (Zbb)
                        0x605 => ((src as i16) as i64) as u64,    // SEXT.H (Zbb)
                        _ if top6 == 0 => src << shamt,
                        _ if top6 == 0b001010 => src | (1u64 << shamt), // BSETI (Zbs bonus)
                        _ if top6 == 0b010010 => src & !(1u64 << shamt), // BCLRI (Zbs bonus)
                        _ if top6 == 0b011010 => src ^ (1u64 << shamt), // BINVI (Zbs bonus)
                        _ => return Err(Trap::IllegalInstruction(raw_instr)),
                    },
                    5 => match imm12 {
                        0x287 => {
                            // ORC.B (Zbb)
                            let mut out = 0u64;
                            for i in 0..8 {
                                if ((src >> (i * 8)) & 0xFF) != 0 {
                                    out |= 0xFFu64 << (i * 8);
                                }
                            }
                            out
                        }
                        0x6B8 => src.swap_bytes(), // REV8 (Zbb)
                        _ if top6 == 0 => src >> shamt,
                        _ if top6 == 0b010000 => ((src as i64) >> shamt) as u64,
                        _ if top6 == 0b011000 => src.rotate_right(shamt), // RORI (Zbb)
                        _ if top6 == 0b010010 => (src >> shamt) & 1,      // BEXTI (Zbs bonus)
                        _ => return Err(Trap::IllegalInstruction(raw_instr)),
                    },
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                if rd != 0 {
                    self.regs[rd] = res;
                }
            }
            0x1B => {
                // OP-IMM-32 + Zba/Zbb 32-bit immediate ops
                let imm12 = instr >> 20;
                let imm = ((instr as i32) >> 20) as i64 as u64;
                let src64 = self.regs[rs1];
                let src32 = src64 as u32;
                let shamt5 = imm12 & 0x1F;
                let shamt6 = imm12 & 0x3F;
                let res = match funct3 {
                    0 => ((src32.wrapping_add(imm as u32)) as i32) as i64 as u64,
                    1 => match imm12 {
                        0x600 => u64::from(src32.leading_zeros()),  // CLZW (Zbb)
                        0x601 => u64::from(src32.trailing_zeros()), // CTZW (Zbb)
                        0x602 => u64::from(src32.count_ones()),     // CPOPW (Zbb)
                        _ if (imm12 >> 6) == 0b000010 => u64::from(src32) << shamt6, // SLLI.UW (Zba)
                        _ if funct7 == 0 => ((src32 << shamt5) as i32) as i64 as u64,
                        _ => return Err(Trap::IllegalInstruction(raw_instr)),
                    },
                    5 => match funct7 {
                        0x00 => ((src32 >> shamt5) as i32) as i64 as u64,
                        0x20 => (((src32 as i32) >> shamt5) as i64) as u64,
                        0x30 => ((src32.rotate_right(shamt5)) as i32) as i64 as u64, // RORIW (Zbb)
                        _ => return Err(Trap::IllegalInstruction(raw_instr)),
                    },
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                if rd != 0 {
                    self.regs[rd] = res;
                }
            }
            0x33 => {
                // OP (RV64I + RV64M + Zba + Zbb)
                let a = self.regs[rs1];
                let b = self.regs[rs2];
                let shamt = (b & 0x3F) as u32;
                let res = match (funct7, funct3) {
                    (0x00, 0) => a.wrapping_add(b),
                    (0x20, 0) => a.wrapping_sub(b),
                    (0x00, 1) => a << shamt,
                    (0x00, 2) => u64::from((a as i64) < (b as i64)),
                    (0x00, 3) => u64::from(a < b),
                    (0x00, 4) => a ^ b,
                    (0x00, 5) => a >> shamt,
                    (0x20, 5) => ((a as i64) >> shamt) as u64,
                    (0x00, 6) => a | b,
                    (0x00, 7) => a & b,
                    // M extension
                    (0x01, 0) => a.wrapping_mul(b),
                    (0x01, 1) => (((a as i64 as i128) * (b as i64 as i128)) >> 64) as u64,
                    (0x01, 2) => (((a as i64 as i128) * (b as u128 as i128)) >> 64) as u64,
                    (0x01, 3) => (((a as u128) * (b as u128)) >> 64) as u64,
                    (0x01, 4) => {
                        let x = a as i64;
                        let y = b as i64;
                        match y {
                            0 => u64::MAX,
                            _ => x.wrapping_div(y) as u64,
                        }
                    }
                    (0x01, 5) => a.checked_div(b).unwrap_or(u64::MAX),
                    (0x01, 6) => {
                        let x = a as i64;
                        let y = b as i64;
                        match y {
                            0 => x as u64,
                            _ => x.wrapping_rem(y) as u64,
                        }
                    }
                    (0x01, 7) => a.checked_rem(b).unwrap_or(a),
                    // Zba: SH1ADD / SH2ADD / SH3ADD
                    (0x10, 2) => b.wrapping_add(a << 1),
                    (0x10, 4) => b.wrapping_add(a << 2),
                    (0x10, 6) => b.wrapping_add(a << 3),
                    // Zbb: ANDN / ORN / XNOR / MIN / MAX / MINU / MAXU / ROL / ROR
                    (0x20, 7) => a & !b,
                    (0x20, 6) => a | !b,
                    (0x20, 4) => !(a ^ b),
                    (0x05, 4) => (a as i64).min(b as i64) as u64,
                    (0x05, 5) => a.min(b),
                    (0x05, 6) => (a as i64).max(b as i64) as u64,
                    (0x05, 7) => a.max(b),
                    (0x30, 1) => a.rotate_left(shamt),
                    (0x30, 5) => a.rotate_right(shamt),
                    // Zbs bonus: BSET / BCLR / BINV / BEXT
                    (0x14, 1) => a | (1u64 << shamt),
                    (0x24, 1) => a & !(1u64 << shamt),
                    (0x34, 1) => a ^ (1u64 << shamt),
                    (0x24, 5) => (a >> shamt) & 1,
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                if rd != 0 {
                    self.regs[rd] = res;
                }
            }
            0x3B => {
                // OP-32 (RV64I + RV64M + Zba + Zbb)
                let a64 = self.regs[rs1];
                let b64 = self.regs[rs2];
                let a = a64 as u32;
                let b = b64 as u32;
                let shamt = b & 0x1F;
                let res = match (funct7, funct3) {
                    (0x00, 0) => (a.wrapping_add(b) as i32) as i64 as u64,
                    (0x20, 0) => (a.wrapping_sub(b) as i32) as i64 as u64,
                    (0x00, 1) => ((a << shamt) as i32) as i64 as u64,
                    (0x00, 5) => ((a >> shamt) as i32) as i64 as u64,
                    (0x20, 5) => (((a as i32) >> shamt) as i64) as u64,
                    // M-32
                    (0x01, 0) => (a.wrapping_mul(b) as i32) as i64 as u64,
                    (0x01, 4) => {
                        let x = a as i32;
                        let y = b as i32;
                        let r = match y {
                            0 => -1,
                            _ => x.wrapping_div(y),
                        };
                        r as i64 as u64
                    }
                    (0x01, 5) => {
                        let r = a.checked_div(b).unwrap_or(u32::MAX);
                        (r as i32) as i64 as u64
                    }
                    (0x01, 6) => {
                        let x = a as i32;
                        let y = b as i32;
                        let r = match y {
                            0 => x,
                            _ => x.wrapping_rem(y),
                        };
                        r as i64 as u64
                    }
                    (0x01, 7) => {
                        let r = a.checked_rem(b).unwrap_or(a);
                        (r as i32) as i64 as u64
                    }
                    // Zba: ADD.UW / SH1ADD.UW / SH2ADD.UW / SH3ADD.UW + Zbb ZEXT.H
                    (0x04, 0) => u64::from(a).wrapping_add(b64),
                    (0x04, 4) if rs2 == 0 => u64::from(a as u16),
                    (0x10, 2) => b64.wrapping_add(u64::from(a) << 1),
                    (0x10, 4) => b64.wrapping_add(u64::from(a) << 2),
                    (0x10, 6) => b64.wrapping_add(u64::from(a) << 3),
                    // Zbb: ROLW / RORW
                    (0x30, 1) => (a.rotate_left(shamt) as i32) as i64 as u64,
                    (0x30, 5) => (a.rotate_right(shamt) as i32) as i64 as u64,
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                if rd != 0 {
                    self.regs[rd] = res;
                }
            }
            0x2F => {
                // AMO (RV64A)
                self.exec_amo(bus, instr, raw_instr, rd, funct3, rs1, rs2)?;
            }
            0x43 | 0x47 | 0x4B | 0x4F => {
                // FMADD / FMSUB / FNMSUB / FNMADD (.S / .D)
                self.require_fp(raw_instr)?;
                let rs3 = ((instr >> 27) & 0x1F) as usize;
                let fmt = (instr >> 25) & 0x3;
                match fmt {
                    0 => {
                        let a = Self::f32_unbox(self.fregs[rs1]);
                        let b = Self::f32_unbox(self.fregs[rs2]);
                        let c = Self::f32_unbox(self.fregs[rs3]);
                        let v = match opcode {
                            0x43 => a.mul_add(b, c),
                            0x47 => a.mul_add(b, -c),
                            0x4B => (-a).mul_add(b, c),
                            _ => (-a).mul_add(b, -c),
                        };
                        self.fregs[rd] = Self::f32_box(v);
                    }
                    1 => {
                        let a = f64::from_bits(self.fregs[rs1]);
                        let b = f64::from_bits(self.fregs[rs2]);
                        let c = f64::from_bits(self.fregs[rs3]);
                        let v = match opcode {
                            0x43 => a.mul_add(b, c),
                            0x47 => a.mul_add(b, -c),
                            0x4B => (-a).mul_add(b, c),
                            _ => (-a).mul_add(b, -c),
                        };
                        self.fregs[rd] = Self::f64_canon(v);
                    }
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                }
                self.csrs.mark_fs_dirty();
            }
            0x53 => {
                // OP-FP (RV64F & RV64D)
                self.exec_fp(instr, raw_instr, rd, funct3, rs1, rs2, funct7)?;
            }
            0x0F => {
                // FENCE / FENCE.I / PAUSE
                match funct3 {
                    0 | 1 => {}
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                }
            }
            0x73 => {
                // SYSTEM + Zicsr
                self.exec_system(instr, raw_instr, rd, funct3, rs1, &mut next_pc)?;
            }
            _ => return Err(Trap::IllegalInstruction(raw_instr)),
        }

        self.pc = next_pc;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn exec_amo(
        &mut self,
        bus: &mut MachineBus,
        instr: u32,
        raw_instr: u32,
        rd: usize,
        funct3: u32,
        rs1: usize,
        rs2: usize,
    ) -> Result<(), Trap> {
        let funct5 = instr >> 27;
        let addr = self.regs[rs1];
        match funct3 {
            2 => {
                // .W (4-byte naturally aligned)
                if (addr & 3) != 0 {
                    let kind = if funct5 == 0x02 {
                        AccessType::Load
                    } else {
                        AccessType::Store
                    };
                    return Err(Trap::MisalignedAccess { addr, kind });
                }
                let src = self.regs[rs2] as u32;
                match funct5 {
                    0x02 => {
                        if rs2 != 0 {
                            return Err(Trap::IllegalInstruction(raw_instr));
                        }
                        let pa = self.translate(bus, addr, AccessType::Load)?;
                        let val = bus.read_phys(pa, 4, AccessType::Load)? as u32;
                        self.reservation = Some(pa & !3);
                        if rd != 0 {
                            self.regs[rd] = (val as i32) as i64 as u64;
                        }
                    }
                    0x03 => {
                        let pa = self.translate(bus, addr, AccessType::Store)?;
                        let ok = self.reservation == Some(pa & !3);
                        self.reservation = None;
                        if ok {
                            bus.write_phys(pa, 4, u64::from(src), AccessType::Store)?;
                            if rd != 0 {
                                self.regs[rd] = 0;
                            }
                        } else if rd != 0 {
                            self.regs[rd] = 1;
                        }
                    }
                    _ => {
                        // Translate with AccessType::Store FIRST so CoW / unmapped pages raise StorePageFault (15)!
                        let pa = self.translate(bus, addr, AccessType::Store)?;
                        let old = bus.read_phys(pa, 4, AccessType::Store)? as u32;
                        let newv = match funct5 {
                            0x01 => src,
                            0x00 => old.wrapping_add(src),
                            0x04 => old ^ src,
                            0x0C => old & src,
                            0x08 => old | src,
                            0x10 => (old as i32).min(src as i32) as u32,
                            0x14 => (old as i32).max(src as i32) as u32,
                            0x18 => old.min(src),
                            0x1C => old.max(src),
                            _ => return Err(Trap::IllegalInstruction(raw_instr)),
                        };
                        bus.write_phys(pa, 4, u64::from(newv), AccessType::Store)?;
                        if rd != 0 {
                            self.regs[rd] = (old as i32) as i64 as u64;
                        }
                    }
                }
            }
            3 => {
                // .D (8-byte naturally aligned)
                if (addr & 7) != 0 {
                    let kind = if funct5 == 0x02 {
                        AccessType::Load
                    } else {
                        AccessType::Store
                    };
                    return Err(Trap::MisalignedAccess { addr, kind });
                }
                let src = self.regs[rs2];
                match funct5 {
                    0x02 => {
                        if rs2 != 0 {
                            return Err(Trap::IllegalInstruction(raw_instr));
                        }
                        let pa = self.translate(bus, addr, AccessType::Load)?;
                        let val = bus.read_phys(pa, 8, AccessType::Load)?;
                        self.reservation = Some(pa & !7);
                        if rd != 0 {
                            self.regs[rd] = val;
                        }
                    }
                    0x03 => {
                        let pa = self.translate(bus, addr, AccessType::Store)?;
                        let ok = self.reservation == Some(pa & !7);
                        self.reservation = None;
                        if ok {
                            bus.write_phys(pa, 8, src, AccessType::Store)?;
                            if rd != 0 {
                                self.regs[rd] = 0;
                            }
                        } else if rd != 0 {
                            self.regs[rd] = 1;
                        }
                    }
                    _ => {
                        let pa = self.translate(bus, addr, AccessType::Store)?;
                        let old = bus.read_phys(pa, 8, AccessType::Store)?;
                        let newv = match funct5 {
                            0x01 => src,
                            0x00 => old.wrapping_add(src),
                            0x04 => old ^ src,
                            0x0C => old & src,
                            0x08 => old | src,
                            0x10 => (old as i64).min(src as i64) as u64,
                            0x14 => (old as i64).max(src as i64) as u64,
                            0x18 => old.min(src),
                            0x1C => old.max(src),
                            _ => return Err(Trap::IllegalInstruction(raw_instr)),
                        };
                        bus.write_phys(pa, 8, newv, AccessType::Store)?;
                        if rd != 0 {
                            self.regs[rd] = old;
                        }
                    }
                }
            }
            _ => return Err(Trap::IllegalInstruction(raw_instr)),
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn exec_fp(
        &mut self,
        _instr: u32,
        raw_instr: u32,
        rd: usize,
        rm: u32,
        rs1: usize,
        rs2: usize,
        funct7: u32,
    ) -> Result<(), Trap> {
        self.require_fp(raw_instr)?;
        match funct7 {
            0x00 | 0x04 | 0x08 | 0x0C | 0x2C | 0x10 | 0x14 => {
                // Single-precision float -> float
                let a = Self::f32_unbox(self.fregs[rs1]);
                let b = Self::f32_unbox(self.fregs[rs2]);
                let out_bits = match funct7 {
                    0x00 => Self::f32_box(a + b),
                    0x04 => Self::f32_box(a - b),
                    0x08 => Self::f32_box(a * b),
                    0x0C => Self::f32_box(a / b),
                    0x2C => {
                        if rs2 != 0 {
                            return Err(Trap::IllegalInstruction(raw_instr));
                        }
                        Self::f32_box(a.sqrt())
                    }
                    0x10 => {
                        let ab = a.to_bits();
                        let bb = b.to_bits();
                        let sign = match rm {
                            0 => bb & 0x8000_0000,
                            1 => (!bb) & 0x8000_0000,
                            2 => (ab ^ bb) & 0x8000_0000,
                            _ => return Err(Trap::IllegalInstruction(raw_instr)),
                        };
                        0xFFFF_FFFF_0000_0000 | u64::from((ab & 0x7FFF_FFFF) | sign)
                    }
                    0x14 => {
                        let v = match rm {
                            0 => f32_riscv_min(a, b),
                            1 => f32_riscv_max(a, b),
                            _ => return Err(Trap::IllegalInstruction(raw_instr)),
                        };
                        Self::f32_box(v)
                    }
                    _ => unreachable!(),
                };
                self.fregs[rd] = out_bits;
                self.csrs.mark_fs_dirty();
            }
            0x01 | 0x05 | 0x09 | 0x0D | 0x2D | 0x11 | 0x15 => {
                // Double-precision float -> float
                let a = f64::from_bits(self.fregs[rs1]);
                let b = f64::from_bits(self.fregs[rs2]);
                let out_bits = match funct7 {
                    0x01 => Self::f64_canon(a + b),
                    0x05 => Self::f64_canon(a - b),
                    0x09 => Self::f64_canon(a * b),
                    0x0D => Self::f64_canon(a / b),
                    0x2D => {
                        if rs2 != 0 {
                            return Err(Trap::IllegalInstruction(raw_instr));
                        }
                        Self::f64_canon(a.sqrt())
                    }
                    0x11 => {
                        let ab = self.fregs[rs1];
                        let bb = self.fregs[rs2];
                        let sign = match rm {
                            0 => bb & (1u64 << 63),
                            1 => (!bb) & (1u64 << 63),
                            2 => (ab ^ bb) & (1u64 << 63),
                            _ => return Err(Trap::IllegalInstruction(raw_instr)),
                        };
                        (ab & 0x7FFF_FFFF_FFFF_FFFF) | sign
                    }
                    0x15 => {
                        let v = match rm {
                            0 => f64_riscv_min(a, b),
                            1 => f64_riscv_max(a, b),
                            _ => return Err(Trap::IllegalInstruction(raw_instr)),
                        };
                        Self::f64_canon(v)
                    }
                    _ => unreachable!(),
                };
                self.fregs[rd] = out_bits;
                self.csrs.mark_fs_dirty();
            }
            0x20 => {
                // FCVT.S.D
                if rs2 != 1 {
                    return Err(Trap::IllegalInstruction(raw_instr));
                }
                let d = f64::from_bits(self.fregs[rs1]);
                self.fregs[rd] = Self::f32_box(d as f32);
                self.csrs.mark_fs_dirty();
            }
            0x21 => {
                // FCVT.D.S
                if rs2 != 0 {
                    return Err(Trap::IllegalInstruction(raw_instr));
                }
                let s = Self::f32_unbox(self.fregs[rs1]);
                self.fregs[rd] = Self::f64_canon(f64::from(s));
                self.csrs.mark_fs_dirty();
            }
            0x50 => {
                // FEQ.S / FLT.S / FLE.S -> integer rd
                let a = Self::f32_unbox(self.fregs[rs1]);
                let b = Self::f32_unbox(self.fregs[rs2]);
                let v = match rm {
                    2 => u64::from(a == b),
                    1 => u64::from(a < b),
                    0 => u64::from(a <= b),
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                if rd != 0 {
                    self.regs[rd] = v;
                }
            }
            0x51 => {
                // FEQ.D / FLT.D / FLE.D -> integer rd
                let a = f64::from_bits(self.fregs[rs1]);
                let b = f64::from_bits(self.fregs[rs2]);
                let v = match rm {
                    2 => u64::from(a == b),
                    1 => u64::from(a < b),
                    0 => u64::from(a <= b),
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                if rd != 0 {
                    self.regs[rd] = v;
                }
            }
            0x60 | 0x61 => {
                // FCVT.{W,WU,L,LU}.{S,D} -> integer rd
                let d = if funct7 == 0x60 {
                    f64::from(Self::f32_unbox(self.fregs[rs1]))
                } else {
                    f64::from_bits(self.fregs[rs1])
                };
                let rounded = self.round_f64(d, rm, raw_instr)?;
                let int_val = match rs2 {
                    0 => f64_to_i32_sat(rounded) as i64 as u64,
                    1 => (f64_to_u32_sat(rounded) as i32) as i64 as u64,
                    2 => f64_to_i64_sat(rounded) as u64,
                    3 => f64_to_u64_sat(rounded),
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                if rd != 0 {
                    self.regs[rd] = int_val;
                }
            }
            0x68 => {
                // FCVT.S.{W,WU,L,LU}
                let x = self.regs[rs1];
                let v = match rs2 {
                    0 => (x as i32) as f32,
                    1 => (x as u32) as f32,
                    2 => (x as i64) as f32,
                    3 => x as f32,
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                self.fregs[rd] = Self::f32_box(v);
                self.csrs.mark_fs_dirty();
            }
            0x69 => {
                // FCVT.D.{W,WU,L,LU}
                let x = self.regs[rs1];
                let v = match rs2 {
                    0 => f64::from(x as i32),
                    1 => f64::from(x as u32),
                    2 => (x as i64) as f64,
                    3 => x as f64,
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                self.fregs[rd] = Self::f64_canon(v);
                self.csrs.mark_fs_dirty();
            }
            0x70 => {
                if rs2 != 0 {
                    return Err(Trap::IllegalInstruction(raw_instr));
                }
                let v = match rm {
                    0 => ((self.fregs[rs1] as u32) as i32) as i64 as u64, // FMV.X.W
                    1 => f32_classify(self.fregs[rs1]),                   // FCLASS.S
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                if rd != 0 {
                    self.regs[rd] = v;
                }
            }
            0x71 => {
                if rs2 != 0 {
                    return Err(Trap::IllegalInstruction(raw_instr));
                }
                let v = match rm {
                    0 => self.fregs[rs1],               // FMV.X.D
                    1 => f64_classify(self.fregs[rs1]), // FCLASS.D
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                };
                if rd != 0 {
                    self.regs[rd] = v;
                }
            }
            0x78 => {
                if rs2 != 0 || rm != 0 {
                    return Err(Trap::IllegalInstruction(raw_instr));
                }
                self.fregs[rd] = 0xFFFF_FFFF_0000_0000 | (self.regs[rs1] & 0xFFFF_FFFF);
                self.csrs.mark_fs_dirty();
            }
            0x79 => {
                if rs2 != 0 || rm != 0 {
                    return Err(Trap::IllegalInstruction(raw_instr));
                }
                self.fregs[rd] = self.regs[rs1];
                self.csrs.mark_fs_dirty();
            }
            _ => return Err(Trap::IllegalInstruction(raw_instr)),
        }
        Ok(())
    }

    fn exec_system(
        &mut self,
        instr: u32,
        raw_instr: u32,
        rd: usize,
        funct3: u32,
        rs1: usize,
        next_pc: &mut u64,
    ) -> Result<(), Trap> {
        let imm12 = instr >> 20;
        match funct3 {
            0 => {
                if rd != 0 {
                    return Err(Trap::IllegalInstruction(raw_instr));
                }
                if (instr >> 25) == 0x09 {
                    // SFENCE.VMA
                    if self.priv_mode == PrivMode::User {
                        return Err(Trap::IllegalInstruction(raw_instr));
                    }
                    self.flush_tlb();
                    return Ok(());
                }
                if rs1 != 0 {
                    return Err(Trap::IllegalInstruction(raw_instr));
                }
                match imm12 {
                    0x000 => return Err(Trap::Ecall),
                    0x001 => return Err(Trap::Ebreak),
                    0x102 => {
                        // SRET
                        if self.priv_mode == PrivMode::User {
                            return Err(Trap::IllegalInstruction(raw_instr));
                        }
                        self.reservation = None;
                        let mut sstatus = self.csrs.sstatus();
                        let spie = (sstatus & SSTATUS_SPIE) != 0;
                        if spie {
                            sstatus |= SSTATUS_SIE;
                        } else {
                            sstatus &= !SSTATUS_SIE;
                        }
                        sstatus |= SSTATUS_SPIE;
                        let spp = (sstatus & SSTATUS_SPP) != 0;
                        sstatus &= !SSTATUS_SPP;
                        self.csrs.set_sstatus(sstatus);
                        self.priv_mode = if spp {
                            PrivMode::Supervisor
                        } else {
                            PrivMode::User
                        };
                        *next_pc = self.csrs.read_raw(CSR_SEPC);
                    }
                    0x302 => {
                        // MRET
                        if self.priv_mode != PrivMode::Machine {
                            return Err(Trap::IllegalInstruction(raw_instr));
                        }
                        self.reservation = None;
                        let mstatus = self.csrs.read_raw(CSR_MSTATUS);
                        let mpp = ((mstatus >> 11) & 0b11) as u8;
                        let mpie = (mstatus >> 7) & 1;
                        let next_mstatus =
                            (mstatus & !((1 << 3) | (0b11 << 11))) | (mpie << 3) | (1 << 7);
                        self.csrs.write_raw(CSR_MSTATUS, next_mstatus);
                        self.priv_mode = PrivMode::from_bits(mpp);
                        *next_pc = self.csrs.read_raw(CSR_MEPC);
                    }
                    0x105 => {
                        // WFI
                        if self.priv_mode == PrivMode::User {
                            return Err(Trap::IllegalInstruction(raw_instr));
                        }
                        self.wfi = true;
                    }
                    _ => return Err(Trap::IllegalInstruction(raw_instr)),
                }
            }
            1..=3 | 5..=7 => {
                let csr = imm12 as u16;
                let zimm = rs1 as u64;
                let will_write = match funct3 {
                    1 | 5 => true,
                    2 | 3 => rs1 != 0,
                    6 | 7 => zimm != 0,
                    _ => false,
                };
                self.csrs
                    .check_access(self.priv_mode, csr, will_write, raw_instr)?;
                let old = match csr {
                    CSR_CYCLE | CSR_TIME | CSR_INSTRET => self.csrs.read_raw(csr),
                    _ => self.csrs.read_raw(csr),
                };
                if will_write {
                    let next = match funct3 {
                        1 => self.regs[rs1],
                        2 => old | self.regs[rs1],
                        3 => old & !self.regs[rs1],
                        5 => zimm,
                        6 => old | zimm,
                        7 => old & !zimm,
                        _ => old,
                    };
                    self.csrs.write_raw(csr, next);
                    if csr == CSR_SATP {
                        self.satp_cached = self.csrs.read_raw(CSR_SATP);
                        self.flush_tlb();
                    }
                }
                if rd != 0 {
                    self.regs[rd] = old;
                }
            }
            _ => return Err(Trap::IllegalInstruction(raw_instr)),
        }
        Ok(())
    }
}

/// Expand a 16-bit RV64C instruction (`C` + `Zcd`) into an equivalent 32-bit RV64IMAFD instruction
/// (or `0` for valid RVC HINT / NOP encodings).
fn expand_rvc(c: u16, hart: &Hart) -> Result<u32, Trap> {
    if c == 0 {
        return Err(Trap::IllegalInstruction(0));
    }
    let raw = u32::from(c);
    let q = c & 0x3;
    let funct3 = (c >> 13) & 0x7;

    let enc_i = |imm12: i32, rs1: u32, f3: u32, rd: u32, op: u32| -> u32 {
        (((imm12 as u32) & 0xFFF) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
    };
    let enc_s = |imm12: u32, rs2: u32, rs1: u32, f3: u32, op: u32| -> u32 {
        (((imm12 >> 5) & 0x7F) << 25)
            | (rs2 << 20)
            | (rs1 << 15)
            | (f3 << 12)
            | ((imm12 & 0x1F) << 7)
            | op
    };
    let enc_r = |f7: u32, rs2: u32, rs1: u32, f3: u32, rd: u32, op: u32| -> u32 {
        (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
    };
    let enc_b = |imm: i32, rs2: u32, rs1: u32, f3: u32| -> u32 {
        let u = imm as u32;
        (((u >> 12) & 1) << 31)
            | (((u >> 5) & 0x3F) << 25)
            | (rs2 << 20)
            | (rs1 << 15)
            | (f3 << 12)
            | (((u >> 1) & 0xF) << 8)
            | (((u >> 11) & 1) << 7)
            | 0x63
    };
    let enc_j = |imm: i32, rd: u32| -> u32 {
        let u = imm as u32;
        (((u >> 20) & 1) << 31)
            | (((u >> 1) & 0x3FF) << 21)
            | (((u >> 11) & 1) << 20)
            | (((u >> 12) & 0xFF) << 12)
            | (rd << 7)
            | 0x6F
    };

    match q {
        0 => {
            let rd_p = u32::from(((c >> 2) & 0x7) + 8);
            let rs1_p = u32::from(((c >> 7) & 0x7) + 8);
            match funct3 {
                0 => {
                    // C.ADDI4SPN
                    let uimm = (u32::from((c >> 6) & 1) << 2)
                        | (u32::from((c >> 5) & 1) << 3)
                        | (u32::from((c >> 11) & 0x3) << 4)
                        | (u32::from((c >> 7) & 0xF) << 6);
                    if uimm == 0 {
                        return Err(Trap::IllegalInstruction(raw));
                    }
                    Ok(enc_i(uimm as i32, 2, 0, rd_p, 0x13))
                }
                1 => {
                    // C.FLD
                    let _ = hart;
                    let uimm = (u32::from((c >> 10) & 0x7) << 3) | (u32::from((c >> 5) & 0x3) << 6);
                    Ok(enc_i(uimm as i32, rs1_p, 3, rd_p, 0x07))
                }
                2 => {
                    // C.LW
                    let uimm = (u32::from((c >> 6) & 1) << 2)
                        | (u32::from((c >> 10) & 0x7) << 3)
                        | (u32::from((c >> 5) & 1) << 6);
                    Ok(enc_i(uimm as i32, rs1_p, 2, rd_p, 0x03))
                }
                3 => {
                    // C.LD
                    let uimm = (u32::from((c >> 10) & 0x7) << 3) | (u32::from((c >> 5) & 0x3) << 6);
                    Ok(enc_i(uimm as i32, rs1_p, 3, rd_p, 0x03))
                }
                5 => {
                    // C.FSD
                    let uimm = (u32::from((c >> 10) & 0x7) << 3) | (u32::from((c >> 5) & 0x3) << 6);
                    Ok(enc_s(uimm, rd_p, rs1_p, 3, 0x27))
                }
                6 => {
                    // C.SW
                    let uimm = (u32::from((c >> 6) & 1) << 2)
                        | (u32::from((c >> 10) & 0x7) << 3)
                        | (u32::from((c >> 5) & 1) << 6);
                    Ok(enc_s(uimm, rd_p, rs1_p, 2, 0x23))
                }
                7 => {
                    // C.SD
                    let uimm = (u32::from((c >> 10) & 0x7) << 3) | (u32::from((c >> 5) & 0x3) << 6);
                    Ok(enc_s(uimm, rd_p, rs1_p, 3, 0x23))
                }
                _ => Err(Trap::IllegalInstruction(raw)),
            }
        }
        1 => {
            let rd = u32::from((c >> 7) & 0x1F);
            let imm6_raw = i32::from(((c >> 12) & 1) << 5 | ((c >> 2) & 0x1F));
            let imm6 = (imm6_raw << 26) >> 26;
            match funct3 {
                0 => {
                    // C.NOP / C.ADDI / HINT
                    if rd == 0 {
                        Ok(0)
                    } else {
                        Ok(enc_i(imm6, rd, 0, rd, 0x13))
                    }
                }
                1 => {
                    // C.ADDIW (rd == 0 is reserved illegal!)
                    if rd == 0 {
                        return Err(Trap::IllegalInstruction(raw));
                    }
                    Ok(enc_i(imm6, rd, 0, rd, 0x1B))
                }
                2 => {
                    // C.LI (rd == 0 is HINT)
                    if rd == 0 {
                        Ok(0)
                    } else {
                        Ok(enc_i(imm6, 0, 0, rd, 0x13))
                    }
                }
                3 => {
                    if rd == 0 {
                        // HINT
                        return Ok(0);
                    }
                    if rd == 2 {
                        // C.ADDI16SP
                        let raw_sp = i32::from(
                            (((c >> 12) & 1) << 9)
                                | (((c >> 3) & 0x3) << 7)
                                | (((c >> 5) & 1) << 6)
                                | (((c >> 2) & 1) << 5)
                                | (((c >> 6) & 1) << 4),
                        );
                        let imm_sp = (raw_sp << 22) >> 22;
                        if imm_sp == 0 {
                            return Err(Trap::IllegalInstruction(raw));
                        }
                        Ok(enc_i(imm_sp, 2, 0, 2, 0x13))
                    } else {
                        // C.LUI
                        if imm6 == 0 {
                            return Err(Trap::IllegalInstruction(raw));
                        }
                        Ok((((imm6 as u32) & 0xFFFFF) << 12) | (rd << 7) | 0x37)
                    }
                }
                4 => {
                    let sub_op = (c >> 10) & 0x3;
                    let rd_p = u32::from(((c >> 7) & 0x7) + 8);
                    let rs2_p = u32::from(((c >> 2) & 0x7) + 8);
                    let shamt = u32::from(((c >> 12) & 1) << 5 | ((c >> 2) & 0x1F));
                    match sub_op {
                        0 => Ok(enc_i(shamt as i32, rd_p, 5, rd_p, 0x13)), // C.SRLI
                        1 => Ok(enc_i((0x400 | shamt) as i32, rd_p, 5, rd_p, 0x13)), // C.SRAI
                        2 => Ok(enc_i(imm6, rd_p, 7, rd_p, 0x13)),         // C.ANDI
                        3 => {
                            let bit12 = (c >> 12) & 1;
                            let f2 = (c >> 5) & 0x3;
                            match (bit12, f2) {
                                (0, 0) => Ok(enc_r(0x20, rs2_p, rd_p, 0, rd_p, 0x33)), // C.SUB
                                (0, 1) => Ok(enc_r(0x00, rs2_p, rd_p, 4, rd_p, 0x33)), // C.XOR
                                (0, 2) => Ok(enc_r(0x00, rs2_p, rd_p, 6, rd_p, 0x33)), // C.OR
                                (0, 3) => Ok(enc_r(0x00, rs2_p, rd_p, 7, rd_p, 0x33)), // C.AND
                                (1, 0) => Ok(enc_r(0x20, rs2_p, rd_p, 0, rd_p, 0x3B)), // C.SUBW
                                (1, 1) => Ok(enc_r(0x00, rs2_p, rd_p, 0, rd_p, 0x3B)), // C.ADDW
                                _ => Err(Trap::IllegalInstruction(raw)),
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                5 => {
                    // C.J
                    let raw_j = i32::from(
                        (((c >> 12) & 1) << 11)
                            | (((c >> 8) & 1) << 10)
                            | (((c >> 9) & 0x3) << 8)
                            | (((c >> 6) & 1) << 7)
                            | (((c >> 7) & 1) << 6)
                            | (((c >> 2) & 1) << 5)
                            | (((c >> 11) & 1) << 4)
                            | (((c >> 3) & 0x7) << 1),
                    );
                    let imm_j = (raw_j << 20) >> 20;
                    Ok(enc_j(imm_j, 0))
                }
                6 | 7 => {
                    // C.BEQZ / C.BNEZ
                    let rs1_p = u32::from(((c >> 7) & 0x7) + 8);
                    let raw_b = i32::from(
                        (((c >> 12) & 1) << 8)
                            | (((c >> 5) & 0x3) << 6)
                            | (((c >> 2) & 1) << 5)
                            | (((c >> 10) & 0x3) << 3)
                            | (((c >> 3) & 0x3) << 1),
                    );
                    let imm_b = (raw_b << 23) >> 23;
                    let f3 = if funct3 == 6 { 0 } else { 1 };
                    Ok(enc_b(imm_b, 0, rs1_p, f3))
                }
                _ => unreachable!(),
            }
        }
        2 => {
            let rd = u32::from((c >> 7) & 0x1F);
            let rs2 = u32::from((c >> 2) & 0x1F);
            match funct3 {
                0 => {
                    // C.SLLI (rd == 0 is HINT)
                    if rd == 0 {
                        Ok(0)
                    } else {
                        let shamt = u32::from(((c >> 12) & 1) << 5 | ((c >> 2) & 0x1F));
                        Ok(enc_i(shamt as i32, rd, 1, rd, 0x13))
                    }
                }
                1 => {
                    // C.FLDSP
                    let uimm = (u32::from((c >> 12) & 1) << 5)
                        | (u32::from((c >> 5) & 0x3) << 3)
                        | (u32::from((c >> 2) & 0x7) << 6);
                    Ok(enc_i(uimm as i32, 2, 3, rd, 0x07))
                }
                2 => {
                    // C.LWSP (rd == 0 is reserved illegal!)
                    if rd == 0 {
                        return Err(Trap::IllegalInstruction(raw));
                    }
                    let uimm = (u32::from((c >> 12) & 1) << 5)
                        | (u32::from((c >> 4) & 0x7) << 2)
                        | (u32::from((c >> 2) & 0x3) << 6);
                    Ok(enc_i(uimm as i32, 2, 2, rd, 0x03))
                }
                3 => {
                    // C.LDSP (rd == 0 is reserved illegal!)
                    if rd == 0 {
                        return Err(Trap::IllegalInstruction(raw));
                    }
                    let uimm = (u32::from((c >> 12) & 1) << 5)
                        | (u32::from((c >> 5) & 0x3) << 3)
                        | (u32::from((c >> 2) & 0x7) << 6);
                    Ok(enc_i(uimm as i32, 2, 3, rd, 0x03))
                }
                4 => {
                    let bit12 = (c >> 12) & 1;
                    if bit12 == 0 {
                        if rs2 == 0 {
                            // C.JR
                            if rd == 0 {
                                return Err(Trap::IllegalInstruction(raw));
                            }
                            Ok(enc_i(0, rd, 0, 0, 0x67))
                        } else if rd == 0 {
                            // HINT
                            Ok(0)
                        } else {
                            // C.MV
                            Ok(enc_r(0, rs2, 0, 0, rd, 0x33))
                        }
                    } else if rd == 0 && rs2 == 0 {
                        // C.EBREAK -> EBREAK (0x00100073)
                        Ok(0x0010_0073)
                    } else if rs2 == 0 {
                        // C.JALR
                        Ok(enc_i(0, rd, 0, 1, 0x67))
                    } else if rd == 0 {
                        // HINT
                        Ok(0)
                    } else {
                        // C.ADD
                        Ok(enc_r(0, rs2, rd, 0, rd, 0x33))
                    }
                }
                5 => {
                    // C.FSDSP
                    let uimm = (u32::from((c >> 10) & 0x7) << 3) | (u32::from((c >> 7) & 0x7) << 6);
                    Ok(enc_s(uimm, rs2, 2, 3, 0x27))
                }
                6 => {
                    // C.SWSP
                    let uimm = (u32::from((c >> 9) & 0xF) << 2) | (u32::from((c >> 7) & 0x3) << 6);
                    Ok(enc_s(uimm, rs2, 2, 2, 0x23))
                }
                7 => {
                    // C.SDSP
                    let uimm = (u32::from((c >> 10) & 0x7) << 3) | (u32::from((c >> 7) & 0x7) << 6);
                    Ok(enc_s(uimm, rs2, 2, 3, 0x23))
                }
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
}

fn f32_riscv_min(a: f32, b: f32) -> f32 {
    if a.is_nan() && b.is_nan() {
        f32::from_bits(0x7FC0_0000)
    } else if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_negative() || b.is_sign_negative() {
            -0.0
        } else {
            0.0
        }
    } else if a < b {
        a
    } else {
        b
    }
}

fn f32_riscv_max(a: f32, b: f32) -> f32 {
    if a.is_nan() && b.is_nan() {
        f32::from_bits(0x7FC0_0000)
    } else if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_positive() || b.is_sign_positive() {
            0.0
        } else {
            -0.0
        }
    } else if a > b {
        a
    } else {
        b
    }
}

fn f64_riscv_min(a: f64, b: f64) -> f64 {
    if a.is_nan() && b.is_nan() {
        f64::from_bits(0x7FF8_0000_0000_0000)
    } else if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_negative() || b.is_sign_negative() {
            -0.0
        } else {
            0.0
        }
    } else if a < b {
        a
    } else {
        b
    }
}

fn f64_riscv_max(a: f64, b: f64) -> f64 {
    if a.is_nan() && b.is_nan() {
        f64::from_bits(0x7FF8_0000_0000_0000)
    } else if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_positive() || b.is_sign_positive() {
            0.0
        } else {
            -0.0
        }
    } else if a > b {
        a
    } else {
        b
    }
}

fn f64_to_i32_sat(x: f64) -> i32 {
    if x.is_nan() || x >= f64::from(i32::MAX) {
        i32::MAX
    } else if x <= f64::from(i32::MIN) {
        i32::MIN
    } else {
        x as i32
    }
}

fn f64_to_u32_sat(x: f64) -> u32 {
    if x.is_nan() || x >= f64::from(u32::MAX) {
        u32::MAX
    } else if x <= 0.0 {
        0
    } else {
        x as u32
    }
}

fn f64_to_i64_sat(x: f64) -> i64 {
    if x.is_nan() || x >= 9_223_372_036_854_775_808.0 {
        i64::MAX
    } else if x <= -9_223_372_036_854_775_808.0 {
        i64::MIN
    } else {
        x as i64
    }
}

fn f64_to_u64_sat(x: f64) -> u64 {
    if x.is_nan() || x >= 18_446_744_073_709_551_616.0 {
        u64::MAX
    } else if x <= 0.0 {
        0
    } else {
        x as u64
    }
}

fn f32_classify(raw64: u64) -> u64 {
    if (raw64 >> 32) != 0xFFFF_FFFF {
        return 1 << 9; // Improperly NaN-boxed => treated as canonical qNaN
    }
    let bits = raw64 as u32;
    let sign = (bits >> 31) != 0;
    let exp = (bits >> 23) & 0xFF;
    let frac = bits & 0x7F_FFFF;
    let bit = match (sign, exp, frac) {
        (true, 0xFF.., 0) => 0,
        (true, 1..=0xFE, _) => 1,
        (true, 0, 1..) => 2,
        (true, 0, 0) => 3,
        (false, 0, 0) => 4,
        (false, 0, 1..) => 5,
        (false, 1..=0xFE, _) => 6,
        (false, 0xFF.., 0) => 7,
        _ => {
            if (frac & 0x40_0000) == 0 {
                8 // signaling NaN
            } else {
                9 // quiet NaN
            }
        }
    };
    1u64 << bit
}

fn f64_classify(bits: u64) -> u64 {
    let sign = (bits >> 63) != 0;
    let exp = (bits >> 52) & 0x7FF;
    let frac = bits & 0x000F_FFFF_FFFF_FFFF;
    let bit = match (sign, exp, frac) {
        (true, 0x7FF.., 0) => 0,
        (true, 1..=0x7FE, _) => 1,
        (true, 0, 1..) => 2,
        (true, 0, 0) => 3,
        (false, 0, 0) => 4,
        (false, 0, 1..) => 5,
        (false, 1..=0x7FE, _) => 6,
        (false, 0x7FF.., 0) => 7,
        _ => {
            if (frac & (1u64 << 51)) == 0 {
                8
            } else {
                9
            }
        }
    };
    1u64 << bit
}
