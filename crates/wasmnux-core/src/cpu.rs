//! RV64IMAC + Zicsr/Zifencei CPU with M/S/U privilege modes and an Sv39
//! MMU, following the RISC-V unprivileged + privileged specifications.
//!
//! Deliberately an interpreter: the first correctness milestone is booting
//! an unmodified upstream kernel, not peak MIPS. Floating point (F/D) is
//! **not** implemented yet and `misa` says so honestly; gates Debian's
//! `lp64d` userspace (see TODO.MD Phase 2). Address-translation details:
//! A/D bits are updated in page tables (Svade-style) so kernels that do not
//! emulate A/D faults work unchanged.

use crate::bus::SystemBus;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const CSR_SSTATUS: u16 = 0x100;
const CSR_SIE: u16 = 0x104;
const CSR_STVEC: u16 = 0x105;
const CSR_SEPC: u16 = 0x141;
const CSR_SCAUSE: u16 = 0x142;
const CSR_STVAL: u16 = 0x143;
const CSR_SIP: u16 = 0x144;
const CSR_SATP: u16 = 0x180;

const CSR_MSTATUS: u16 = 0x300;
const CSR_MISA: u16 = 0x301;
const CSR_MEDELEG: u16 = 0x302;
const CSR_MIDELEG: u16 = 0x303;
const CSR_MIE: u16 = 0x304;
const CSR_MTVEC: u16 = 0x305;
const CSR_MEPC: u16 = 0x341;
const CSR_MCAUSE: u16 = 0x342;
const CSR_MTVAL: u16 = 0x343;
const CSR_MIP: u16 = 0x344;
const CSR_MCYCLE: u16 = 0xB00;
const CSR_MINSTRET: u16 = 0xB02;

const CSR_CYCLE: u16 = 0xC00;
const CSR_TIME: u16 = 0xC01;
const CSR_INSTRET: u16 = 0xC02;

const MSTATUS_SIE: u64 = 1 << 1;
const MSTATUS_MIE: u64 = 1 << 3;
const MSTATUS_SPIE: u64 = 1 << 5;
const MSTATUS_MPIE: u64 = 1 << 7;
const MSTATUS_SPP: u64 = 1 << 8;
const MSTATUS_MPP: u64 = 0b11 << 11;
const MSTATUS_MPRV: u64 = 1 << 17;
const MSTATUS_SUM: u64 = 1 << 18;
const MSTATUS_MXR: u64 = 1 << 19;

const SSTATUS_MASK: u64 = MSTATUS_SIE | MSTATUS_SPIE | MSTATUS_SPP | MSTATUS_SUM | MSTATUS_MXR;

const MIP_SSIP: u64 = 1 << 1;
const MIP_MSIP: u64 = 1 << 3;
const MIP_STIP: u64 = 1 << 5;
const MIP_MTIP: u64 = 1 << 7;
const MIP_SEIP: u64 = 1 << 9;
const MIP_MEIP: u64 = 1 << 11;
const MIP_WRITABLE_MASK: u64 = MIP_SSIP | MIP_STIP | MIP_SEIP;
const MIE_MASK: u64 = MIP_SSIP | MIP_MSIP | MIP_STIP | MIP_MTIP | MIP_SEIP | MIP_MEIP;
const SIE_MASK: u64 = MIP_SSIP | MIP_STIP | MIP_SEIP;
/// Exception bits that may be delegated to S-mode.
const MEDELEG_MASK: u64 = 0xB3FF;

const CAUSE_FETCH_ACCESS: u64 = 1;
const CAUSE_ILLEGAL: u64 = 2;
const CAUSE_BREAKPOINT: u64 = 3;
const CAUSE_LOAD_ACCESS: u64 = 5;
const CAUSE_STORE_ACCESS: u64 = 7;
const CAUSE_ECALL_U: u64 = 8;
const CAUSE_ECALL_S: u64 = 9;
const CAUSE_ECALL_M: u64 = 11;
const CAUSE_FETCH_PAGE: u64 = 12;
const CAUSE_LOAD_PAGE: u64 = 13;
const CAUSE_STORE_PAGE: u64 = 15;

/// `misa`: RV64 (MXL=2) with I, M, A, C, S, U.
const MISA_VALUE: u64 = (2_u64 << 62)
    | (1 << 0)  // A
    | (1 << 2)  // C
    | (1 << 8)  // I
    | (1 << 12) // M
    | (1 << 18) // S
    | (1 << 20); // U

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// RISC-V privilege level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Privilege {
    User = 0,
    Supervisor = 1,
    Machine = 3,
}

impl Privilege {
    fn from_bits(bits: u64) -> Self {
        match bits & 0b11 {
            1 => Self::Supervisor,
            3 => Self::Machine,
            _ => Self::User,
        }
    }
}

/// Result of one CPU step, as seen by the machine/host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    /// An instruction executed (or a trap was delivered to the guest).
    Executed,
    /// The hart executed WFI and no interrupt is pending; the host should
    /// wait for device activity instead of busy-spinning.
    WaitingForInterrupt,
    /// The guest executed `ecall` from S-mode and the host asked to service
    /// SBI calls itself (built-in firmware shim). PC still points at the
    /// `ecall`; the machine must advance it after handling.
    SbiCall,
}

#[derive(Debug, Clone, Copy)]
struct Trap {
    cause: u64,
    tval: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Fetch,
    Load,
    Store,
}

#[derive(Debug, Clone, Copy)]
enum Flow {
    Next(u64),
    Wfi(u64),
    SbiCall,
}

/// A single RISC-V hart.
#[derive(Debug)]
pub struct Cpu {
    regs: [u64; 32],
    pc: u64,
    privilege: Privilege,
    csr: Box<[u64; 4096]>,
    reservation: Option<u64>,
    /// Device-driven `mip` bits (MSIP/MTIP/MEIP, plus PLIC SEIP line).
    external_mip: u64,
    /// Value returned by the `time` CSR (synced from CLINT by the machine).
    time_value: u64,
    cycles: u64,
    waiting: bool,
}

impl Cpu {
    /// Creates a hart in M-mode at reset vector `entry`, registers cleared.
    pub fn new(entry: u64) -> Self {
        Self {
            regs: [0; 32],
            pc: entry,
            privilege: Privilege::Machine,
            csr: Box::new([0; 4096]),
            reservation: None,
            external_mip: 0,
            time_value: 0,
            cycles: 0,
            waiting: false,
        }
    }

    /// Hard reset to `entry` in M-mode (firmware boot path).
    pub fn reset(&mut self, entry: u64) {
        *self = Self::new(entry);
    }

    /// Prepares a direct S-mode kernel entry (built-in SBI boot path):
    /// PC = kernel entry, a0 = hartid, a1 = DTB address, as the RISC-V
    /// kernel boot protocol requires from firmware.
    pub fn reset_supervisor(&mut self, entry: u64, hartid: u64, dtb_address: u64) {
        self.reset(entry);
        self.privilege = Privilege::Supervisor;
        self.set_reg(10, hartid);
        self.set_reg(11, dtb_address);
        // sstatus: SIE off, SPP clear; satp bare. Kernel assumes this state.
        self.csr[CSR_MSTATUS as usize] = 0;
        self.csr[CSR_SATP as usize] = 0;
    }

    pub fn pc(&self) -> u64 {
        self.pc
    }

    pub fn set_pc(&mut self, pc: u64) {
        self.pc = pc;
    }

    pub fn privilege(&self) -> Privilege {
        self.privilege
    }

    pub fn cycles(&self) -> u64 {
        self.cycles
    }

    pub fn reg(&self, index: usize) -> u64 {
        if index == 0 {
            0
        } else {
            self.regs[index & 31]
        }
    }

    pub fn set_reg(&mut self, index: usize, value: u64) {
        if index != 0 && index < 32 {
            self.regs[index] = value;
        }
    }

    /// Syncs device-driven interrupt lines (machine calls this per slice).
    /// `seip_line` is the PLIC S-context output; it ORs into `mip.SEIP`.
    pub fn set_external_interrupts(&mut self, msip: bool, mtip: bool, meip: bool, seip: bool) {
        let mut bits = 0_u64;
        if msip {
            bits |= MIP_MSIP;
        }
        if mtip {
            bits |= MIP_MTIP;
        }
        if meip {
            bits |= MIP_MEIP;
        }
        if seip {
            bits |= MIP_SEIP;
        }
        self.external_mip = bits;
    }

    /// Sets/clears the SBI-driven supervisor timer pending bit.
    pub fn set_supervisor_timer_pending(&mut self, pending: bool) {
        if pending {
            self.csr[CSR_MIP as usize] |= MIP_STIP;
        } else {
            self.csr[CSR_MIP as usize] &= !MIP_STIP;
        }
    }

    pub fn set_time_value(&mut self, value: u64) {
        self.time_value = value;
    }

    fn mip(&self) -> u64 {
        (self.csr[CSR_MIP as usize] & MIP_WRITABLE_MASK) | self.external_mip
    }

    fn mstatus(&self) -> u64 {
        self.csr[CSR_MSTATUS as usize]
    }

    // -----------------------------------------------------------------------
    // CSR access
    // -----------------------------------------------------------------------

    fn csr_exists(address: u16) -> bool {
        matches!(
            address,
            CSR_SSTATUS
                | CSR_SIE
                | CSR_STVEC
                | 0x106 // scounteren
                | 0x10A // senvcfg
                | 0x140 // sscratch
                | CSR_SEPC
                | CSR_SCAUSE
                | CSR_STVAL
                | CSR_SIP
                | CSR_SATP
                | CSR_MSTATUS
                | CSR_MISA
                | CSR_MEDELEG
                | CSR_MIDELEG
                | CSR_MIE
                | CSR_MTVEC
                | 0x306 // mcounteren
                | 0x30A // menvcfg
                | 0x310 // mstatush
                | 0x320 // mcountinhibit
                | 0x340 // mscratch
                | CSR_MEPC
                | CSR_MCAUSE
                | CSR_MTVAL
                | CSR_MIP
                | 0x34A // mtinst
                | CSR_MCYCLE
                | CSR_MINSTRET
                | 0xF11 // mvendorid
                | 0xF12 // marchid
                | 0xF13 // mimpid
                | 0xF14 // mhartid
                | CSR_CYCLE
                | CSR_TIME
                | CSR_INSTRET
        ) || (0x3A0..=0x3A3).contains(&address)  // pmpcfg0/2 (RV64: even only, accept range)
            || (0x3B0..=0x3BF).contains(&address) // pmpaddr0..15
            || (0xB03..=0xB1F).contains(&address) // mhpmcounter3..31
            || (0xC03..=0xC1F).contains(&address) // hpmcounter3..31
            || (0x323..=0x33F).contains(&address) // mhpmevent3..31
    }

    fn read_csr(&self, address: u16) -> Option<u64> {
        if !Self::csr_exists(address) {
            return None;
        }
        let value = match address {
            CSR_SSTATUS => self.mstatus() & SSTATUS_MASK,
            CSR_SIE => self.csr[CSR_MIE as usize] & SIE_MASK,
            CSR_SIP => self.mip() & SIE_MASK,
            CSR_MIP => self.mip(),
            CSR_MISA => MISA_VALUE,
            CSR_CYCLE | CSR_MCYCLE => self.cycles,
            CSR_INSTRET | CSR_MINSTRET => self.cycles,
            CSR_TIME => self.time_value,
            0x310 | 0xF11 | 0xF12 | 0xF13 | 0xF14 => 0,
            other => self.csr[other as usize],
        };
        Some(value)
    }

    fn write_csr(&mut self, address: u16, value: u64) -> bool {
        if !Self::csr_exists(address) {
            return false;
        }
        match address {
            CSR_MISA | 0xF11 | 0xF12 | 0xF13 | 0xF14 | 0x310 => { /* read-only in practice */ }
            CSR_SSTATUS => {
                let current = self.mstatus();
                self.csr[CSR_MSTATUS as usize] = (current & !SSTATUS_MASK) | (value & SSTATUS_MASK);
            }
            CSR_SIE => {
                let current = self.csr[CSR_MIE as usize];
                self.csr[CSR_MIE as usize] = (current & !SIE_MASK) | (value & SIE_MASK);
            }
            CSR_SIP => {
                // Only SSIP is S-writable in mip.
                if value & MIP_SSIP != 0 {
                    self.csr[CSR_MIP as usize] |= MIP_SSIP;
                } else {
                    self.csr[CSR_MIP as usize] &= !MIP_SSIP;
                }
            }
            CSR_MIP => {
                self.csr[CSR_MIP as usize] =
                    (self.csr[CSR_MIP as usize] & !MIP_WRITABLE_MASK) | (value & MIP_WRITABLE_MASK);
            }
            CSR_MIE => self.csr[CSR_MIE as usize] = value & MIE_MASK,
            CSR_MIDELEG => self.csr[CSR_MIDELEG as usize] = value & SIE_MASK,
            CSR_MEDELEG => self.csr[CSR_MEDELEG as usize] = value & MEDELEG_MASK,
            CSR_MSTATUS => {
                let mut sanitised = value;
                // MPP=2 (reserved hypervisor encoding) is WARL → User.
                if (sanitised & MSTATUS_MPP) >> 11 == 2 {
                    sanitised &= !MSTATUS_MPP;
                }
                self.csr[CSR_MSTATUS as usize] = sanitised;
            }
            CSR_SATP => {
                let mode = value >> 60;
                if mode == 0 || mode == 8 {
                    self.csr[CSR_SATP as usize] = value;
                }
                // Other modes: WARL-ignored so the kernel's Sv48/Sv57 probe
                // falls back to Sv39, as on real Sv39-only hardware.
            }
            CSR_MEPC | CSR_SEPC => self.csr[address as usize] = value & !1,
            CSR_MTVEC | CSR_STVEC => self.csr[address as usize] = value & !0b10,
            CSR_MCYCLE => self.cycles = value,
            CSR_MINSTRET | CSR_CYCLE | CSR_TIME | CSR_INSTRET => {}
            other => self.csr[other as usize] = value,
        }
        true
    }

    // -----------------------------------------------------------------------
    // Traps and interrupts
    // -----------------------------------------------------------------------

    fn deliver_trap(&mut self, trap: Trap, is_interrupt: bool, to_supervisor: bool) {
        let cause = trap.cause | if is_interrupt { 1_u64 << 63 } else { 0 };
        if to_supervisor {
            self.csr[CSR_SEPC as usize] = self.pc;
            self.csr[CSR_SCAUSE as usize] = cause;
            self.csr[CSR_STVAL as usize] = trap.tval;
            let mut status = self.mstatus();
            if status & MSTATUS_SIE != 0 {
                status |= MSTATUS_SPIE;
            } else {
                status &= !MSTATUS_SPIE;
            }
            status &= !MSTATUS_SIE;
            if self.privilege == Privilege::Supervisor {
                status |= MSTATUS_SPP;
            } else {
                status &= !MSTATUS_SPP;
            }
            self.csr[CSR_MSTATUS as usize] = status;
            self.privilege = Privilege::Supervisor;
            let tvec = self.csr[CSR_STVEC as usize];
            self.pc = trap_vector(tvec, cause, is_interrupt);
        } else {
            self.csr[CSR_MEPC as usize] = self.pc;
            self.csr[CSR_MCAUSE as usize] = cause;
            self.csr[CSR_MTVAL as usize] = trap.tval;
            let mut status = self.mstatus();
            if status & MSTATUS_MIE != 0 {
                status |= MSTATUS_MPIE;
            } else {
                status &= !MSTATUS_MPIE;
            }
            status &= !MSTATUS_MIE;
            status = (status & !MSTATUS_MPP) | ((self.privilege as u64) << 11);
            self.csr[CSR_MSTATUS as usize] = status;
            self.privilege = Privilege::Machine;
            let tvec = self.csr[CSR_MTVEC as usize];
            self.pc = trap_vector(tvec, cause, is_interrupt);
        }
    }

    fn handle_exception(&mut self, trap: Trap) {
        let delegated = trap.cause < 64
            && (self.csr[CSR_MEDELEG as usize] >> trap.cause) & 1 == 1
            && self.privilege != Privilege::Machine;
        self.deliver_trap(trap, false, delegated);
    }

    /// Returns (interrupt code, deliver-to-S) for the highest-priority
    /// interrupt that is enabled *and* globally taken in the current mode.
    fn pending_interrupt(&self) -> Option<(u64, bool)> {
        let pending = self.mip() & self.csr[CSR_MIE as usize];
        if pending == 0 {
            return None;
        }
        // Spec priority: MEI, MSI, MTI, SEI, SSI, STI.
        for code in [11_u64, 3, 7, 9, 1, 5] {
            if (pending >> code) & 1 == 0 {
                continue;
            }
            let delegated = (self.csr[CSR_MIDELEG as usize] >> code) & 1 == 1;
            if delegated && self.privilege != Privilege::Machine {
                let sie = self.mstatus() & MSTATUS_SIE != 0;
                if self.privilege == Privilege::User || sie {
                    return Some((code, true));
                }
            } else {
                let mie_global = self.mstatus() & MSTATUS_MIE != 0;
                if self.privilege != Privilege::Machine || mie_global {
                    return Some((code, false));
                }
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // MMU (Sv39)
    // -----------------------------------------------------------------------

    fn effective_privilege(&self, access: Access) -> Privilege {
        if access != Access::Fetch && self.mstatus() & MSTATUS_MPRV != 0 {
            Privilege::from_bits((self.mstatus() & MSTATUS_MPP) >> 11)
        } else {
            self.privilege
        }
    }

    fn translate(&self, bus: &mut SystemBus, vaddr: u64, access: Access) -> Result<u64, Trap> {
        let effective = self.effective_privilege(access);
        if effective == Privilege::Machine {
            return Ok(vaddr);
        }
        let satp = self.csr[CSR_SATP as usize];
        if satp >> 60 != 8 {
            return Ok(vaddr); // Bare mode (other modes are WARL-rejected on write).
        }
        let page_fault = |tval: u64| Trap {
            cause: match access {
                Access::Fetch => CAUSE_FETCH_PAGE,
                Access::Load => CAUSE_LOAD_PAGE,
                Access::Store => CAUSE_STORE_PAGE,
            },
            tval,
        };
        let access_fault = |tval: u64| Trap {
            cause: match access {
                Access::Fetch => CAUSE_FETCH_ACCESS,
                Access::Load => CAUSE_LOAD_ACCESS,
                Access::Store => CAUSE_STORE_ACCESS,
            },
            tval,
        };

        let mut table = (satp & ((1_u64 << 44) - 1)) << 12;
        let mut level = 2_i32;
        loop {
            let vpn = (vaddr >> (12 + 9 * level as u64)) & 0x1FF;
            let pte_address = table + vpn * 8;
            let pte = bus
                .read_phys(pte_address, 8)
                .map_err(|_| access_fault(vaddr))?;
            let valid = pte & 1 != 0;
            let readable = pte & (1 << 1) != 0;
            let writable = pte & (1 << 2) != 0;
            let executable = pte & (1 << 3) != 0;
            let user_page = pte & (1 << 4) != 0;
            if !valid || (!readable && writable) {
                return Err(page_fault(vaddr));
            }
            let ppn = (pte >> 10) & ((1_u64 << 44) - 1);
            if readable || executable {
                // Leaf PTE.
                if effective == Privilege::User && !user_page {
                    return Err(page_fault(vaddr));
                }
                if effective == Privilege::Supervisor && user_page {
                    if access == Access::Fetch {
                        return Err(page_fault(vaddr));
                    }
                    if self.mstatus() & MSTATUS_SUM == 0 {
                        return Err(page_fault(vaddr));
                    }
                }
                let allowed = match access {
                    Access::Fetch => executable,
                    Access::Load => readable || (self.mstatus() & MSTATUS_MXR != 0 && executable),
                    Access::Store => writable,
                };
                if !allowed {
                    return Err(page_fault(vaddr));
                }
                if level > 0 && ppn & ((1_u64 << (9 * level as u64)) - 1) != 0 {
                    return Err(page_fault(vaddr)); // misaligned superpage
                }
                // Svade-style A/D update.
                let mut new_pte = pte | (1 << 6);
                if access == Access::Store {
                    new_pte |= 1 << 7;
                }
                if new_pte != pte {
                    let _ = bus.write_phys(pte_address, 8, new_pte);
                }
                let offset_mask = (1_u64 << (12 + 9 * level as u64)) - 1;
                let ppn_mask = if level > 0 {
                    !((1_u64 << (9 * level as u64)) - 1)
                } else {
                    u64::MAX
                };
                return Ok(((ppn & ppn_mask) << 12) | (vaddr & offset_mask));
            }
            if level == 0 {
                return Err(page_fault(vaddr));
            }
            table = ppn << 12;
            level -= 1;
        }
    }

    fn load_memory(
        &self,
        bus: &mut SystemBus,
        vaddr: u64,
        size: usize,
        access: Access,
    ) -> Result<u64, Trap> {
        let mut value = 0_u64;
        for i in 0..size {
            let paddr = self.translate(bus, vaddr + i as u64, access)?;
            let mut byte = [0_u8; 1];
            bus.read_phys_bytes(paddr, &mut byte).map_err(|_| Trap {
                cause: match access {
                    Access::Fetch => CAUSE_FETCH_ACCESS,
                    _ => CAUSE_LOAD_ACCESS,
                },
                tval: vaddr,
            })?;
            value |= u64::from(byte[0]) << (8 * i);
        }
        Ok(value)
    }

    fn store_memory(
        &self,
        bus: &mut SystemBus,
        vaddr: u64,
        size: usize,
        value: u64,
    ) -> Result<(), Trap> {
        for i in 0..size {
            let paddr = self.translate(bus, vaddr + i as u64, Access::Store)?;
            let byte = (value >> (8 * i)) & 0xFF;
            bus.write_phys(paddr, 1, byte).map_err(|_| Trap {
                cause: CAUSE_STORE_ACCESS,
                tval: vaddr,
            })?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Step / fetch / decode
    // -----------------------------------------------------------------------

    /// Executes one instruction (or delivers a pending interrupt).
    pub fn step(&mut self, bus: &mut SystemBus, intercept_sbi: bool) -> StepOutcome {
        self.cycles = self.cycles.wrapping_add(1);
        if self.waiting {
            if self.mip() & self.csr[CSR_MIE as usize] != 0 {
                self.waiting = false;
            } else {
                return StepOutcome::WaitingForInterrupt;
            }
        }
        if let Some((code, to_supervisor)) = self.pending_interrupt() {
            self.deliver_trap(
                Trap {
                    cause: code,
                    tval: 0,
                },
                true,
                to_supervisor,
            );
            return StepOutcome::Executed;
        }

        let pc = self.pc;
        let half = match self.load_memory(bus, pc, 2, Access::Fetch) {
            Ok(v) => v as u16,
            Err(trap) => {
                self.handle_exception(trap);
                return StepOutcome::Executed;
            }
        };
        let flow = if half & 0b11 != 0b11 {
            self.execute_compressed(bus, half)
        } else {
            match self.load_memory(bus, pc, 4, Access::Fetch) {
                Ok(v) => self.execute32(bus, v as u32, intercept_sbi),
                Err(trap) => Err(trap),
            }
        };
        match flow {
            Ok(Flow::Next(next_pc)) => {
                self.pc = next_pc;
                StepOutcome::Executed
            }
            Ok(Flow::Wfi(next_pc)) => {
                self.pc = next_pc;
                self.waiting = true;
                StepOutcome::Executed
            }
            Ok(Flow::SbiCall) => StepOutcome::SbiCall,
            Err(trap) => {
                // PC still points at the faulting instruction: exactly what
                // mepc/sepc must record.
                self.handle_exception(trap);
                StepOutcome::Executed
            }
        }
    }

    fn raise_illegal(insn: u64) -> Trap {
        Trap {
            cause: CAUSE_ILLEGAL,
            tval: insn,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn execute32(
        &mut self,
        bus: &mut SystemBus,
        insn: u32,
        intercept_sbi: bool,
    ) -> Result<Flow, Trap> {
        let pc = self.pc;
        let next = pc + 4;
        let opcode = insn & 0x7F;
        let rd = ((insn >> 7) & 0x1F) as usize;
        let funct3 = (insn >> 12) & 0x7;
        let rs1 = ((insn >> 15) & 0x1F) as usize;
        let rs2 = ((insn >> 20) & 0x1F) as usize;
        let funct7 = (insn >> 25) & 0x7F;
        let a = self.reg(rs1);
        let b = self.reg(rs2);

        match opcode {
            0x37 => self.set_reg(rd, sext(u64::from(insn & 0xFFFF_F000), 32)), // LUI
            0x17 => self.set_reg(rd, pc.wrapping_add(sext(u64::from(insn & 0xFFFF_F000), 32))), // AUIPC
            0x6F => {
                // JAL
                self.set_reg(rd, next);
                return Ok(Flow::Next(pc.wrapping_add(imm_j(insn))));
            }
            0x67 => {
                // JALR
                if funct3 != 0 {
                    return Err(Self::raise_illegal(u64::from(insn)));
                }
                let target = a.wrapping_add(imm_i(insn)) & !1;
                self.set_reg(rd, next);
                return Ok(Flow::Next(target));
            }
            0x63 => {
                // Branches
                let taken = match funct3 {
                    0b000 => a == b,
                    0b001 => a != b,
                    0b100 => (a as i64) < (b as i64),
                    0b101 => (a as i64) >= (b as i64),
                    0b110 => a < b,
                    0b111 => a >= b,
                    _ => return Err(Self::raise_illegal(u64::from(insn))),
                };
                if taken {
                    return Ok(Flow::Next(pc.wrapping_add(imm_b(insn))));
                }
            }
            0x03 => {
                // Loads
                let address = a.wrapping_add(imm_i(insn));
                let (size, signed) = match funct3 {
                    0b000 => (1, true),
                    0b001 => (2, true),
                    0b010 => (4, true),
                    0b011 => (8, true),
                    0b100 => (1, false),
                    0b101 => (2, false),
                    0b110 => (4, false),
                    _ => return Err(Self::raise_illegal(u64::from(insn))),
                };
                let raw = self.load_memory(bus, address, size, Access::Load)?;
                let value = if signed {
                    sext(raw, (size * 8) as u32)
                } else {
                    raw
                };
                self.set_reg(rd, value);
            }
            0x23 => {
                // Stores
                let address = a.wrapping_add(imm_s(insn));
                let size = match funct3 {
                    0b000 => 1,
                    0b001 => 2,
                    0b010 => 4,
                    0b011 => 8,
                    _ => return Err(Self::raise_illegal(u64::from(insn))),
                };
                self.store_memory(bus, address, size, b)?;
                self.reservation = None;
            }
            0x13 => {
                // OP-IMM
                let imm = imm_i(insn);
                let shamt = (insn >> 20) & 0x3F;
                let value = match funct3 {
                    0b000 => a.wrapping_add(imm),
                    0b010 => u64::from((a as i64) < (imm as i64)),
                    0b011 => u64::from(a < imm),
                    0b100 => a ^ imm,
                    0b110 => a | imm,
                    0b111 => a & imm,
                    0b001 => {
                        if (insn >> 26) & 0x3F != 0 {
                            return Err(Self::raise_illegal(u64::from(insn)));
                        }
                        a << shamt
                    }
                    0b101 => match (insn >> 26) & 0x3F {
                        0b000000 => a >> shamt,
                        0b010000 => ((a as i64) >> shamt) as u64,
                        _ => return Err(Self::raise_illegal(u64::from(insn))),
                    },
                    _ => unreachable!(),
                };
                self.set_reg(rd, value);
            }
            0x1B => {
                // OP-IMM-32
                let imm = imm_i(insn);
                let shamt = (insn >> 20) & 0x1F;
                let value = match funct3 {
                    0b000 => sext(a.wrapping_add(imm) & 0xFFFF_FFFF, 32),
                    0b001 => {
                        if funct7 != 0 {
                            return Err(Self::raise_illegal(u64::from(insn)));
                        }
                        sext(((a as u32) << shamt) as u64, 32)
                    }
                    0b101 => match funct7 {
                        0b0000000 => sext(((a as u32) >> shamt) as u64, 32),
                        0b0100000 => sext((((a as u32 as i32) >> shamt) as u32) as u64, 32),
                        _ => return Err(Self::raise_illegal(u64::from(insn))),
                    },
                    _ => return Err(Self::raise_illegal(u64::from(insn))),
                };
                self.set_reg(rd, value);
            }
            0x33 => {
                // OP (incl. M)
                let shamt = (b & 0x3F) as u32;
                let value = match (funct7, funct3) {
                    (0, 0b000) => a.wrapping_add(b),
                    (0x20, 0b000) => a.wrapping_sub(b),
                    (0, 0b001) => a << shamt,
                    (0, 0b010) => u64::from((a as i64) < (b as i64)),
                    (0, 0b011) => u64::from(a < b),
                    (0, 0b100) => a ^ b,
                    (0, 0b101) => a >> shamt,
                    (0x20, 0b101) => ((a as i64) >> shamt) as u64,
                    (0, 0b110) => a | b,
                    (0, 0b111) => a & b,
                    (1, 0b000) => a.wrapping_mul(b),
                    (1, 0b001) => (((a as i64 as i128) * (b as i64 as i128)) >> 64) as u64,
                    (1, 0b010) => (((a as i64 as i128) * (b as u128 as i128)) >> 64) as u64,
                    (1, 0b011) => (((a as u128) * (b as u128)) >> 64) as u64,
                    (1, 0b100) => div_signed(a, b),
                    (1, 0b101) => a.checked_div(b).unwrap_or(u64::MAX),
                    (1, 0b110) => rem_signed(a, b),
                    (1, 0b111) => a.checked_rem(b).unwrap_or(a),
                    _ => return Err(Self::raise_illegal(u64::from(insn))),
                };
                self.set_reg(rd, value);
            }
            0x3B => {
                // OP-32 (incl. M word forms)
                let shamt = (b & 0x1F) as u32;
                let value = match (funct7, funct3) {
                    (0, 0b000) => sext64_32(a.wrapping_add(b)),
                    (0x20, 0b000) => sext64_32(a.wrapping_sub(b)),
                    (0, 0b001) => sext(((a as u32) << shamt) as u64, 32),
                    (0, 0b101) => sext(((a as u32) >> shamt) as u64, 32),
                    (0x20, 0b101) => sext((((a as u32 as i32) >> shamt) as u32) as u64, 32),
                    (1, 0b000) => sext64_32(a.wrapping_mul(b)),
                    (1, 0b100) => {
                        let (x, y) = (a as u32 as i32, b as u32 as i32);
                        let r = if y == 0 { -1 } else { x.wrapping_div(y) };
                        sext(r as u32 as u64, 32)
                    }
                    (1, 0b101) => {
                        let (x, y) = (a as u32, b as u32);
                        let r = x.checked_div(y).unwrap_or(u32::MAX);
                        sext(r as u64, 32)
                    }
                    (1, 0b110) => {
                        let (x, y) = (a as u32 as i32, b as u32 as i32);
                        let r = if y == 0 { x } else { x.wrapping_rem(y) };
                        sext(r as u32 as u64, 32)
                    }
                    (1, 0b111) => {
                        let (x, y) = (a as u32, b as u32);
                        let r = x.checked_rem(y).unwrap_or(x);
                        sext(r as u64, 32)
                    }
                    _ => return Err(Self::raise_illegal(u64::from(insn))),
                };
                self.set_reg(rd, value);
            }
            0x0F => {
                // FENCE / FENCE.I: no caches to invalidate; architecturally
                // complete as NOPs in this single-hart interpreter.
                if funct3 > 1 {
                    return Err(Self::raise_illegal(u64::from(insn)));
                }
            }
            0x73 => {
                return self.execute_system(insn, next, intercept_sbi);
            }
            0x2F => {
                return self.execute_atomic(bus, insn).map(|()| Flow::Next(next));
            }
            _ => return Err(Self::raise_illegal(u64::from(insn))),
        }
        Ok(Flow::Next(next))
    }

    fn execute_system(&mut self, insn: u32, next: u64, intercept_sbi: bool) -> Result<Flow, Trap> {
        let funct3 = (insn >> 12) & 0x7;
        if funct3 == 0 {
            match insn {
                0x0000_0073 => {
                    // ECALL
                    if intercept_sbi && self.privilege == Privilege::Supervisor {
                        return Ok(Flow::SbiCall);
                    }
                    let cause = match self.privilege {
                        Privilege::User => CAUSE_ECALL_U,
                        Privilege::Supervisor => CAUSE_ECALL_S,
                        Privilege::Machine => CAUSE_ECALL_M,
                    };
                    Err(Trap { cause, tval: 0 })
                }
                0x0010_0073 => Err(Trap {
                    cause: CAUSE_BREAKPOINT,
                    tval: self.pc,
                }),
                0x3020_0073 => {
                    // MRET
                    if self.privilege != Privilege::Machine {
                        return Err(Self::raise_illegal(u64::from(insn)));
                    }
                    let mut status = self.mstatus();
                    let target_priv = Privilege::from_bits((status & MSTATUS_MPP) >> 11);
                    if status & MSTATUS_MPIE != 0 {
                        status |= MSTATUS_MIE;
                    } else {
                        status &= !MSTATUS_MIE;
                    }
                    status |= MSTATUS_MPIE;
                    status &= !MSTATUS_MPP;
                    if target_priv != Privilege::Machine {
                        status &= !MSTATUS_MPRV;
                    }
                    self.csr[CSR_MSTATUS as usize] = status;
                    self.privilege = target_priv;
                    Ok(Flow::Next(self.csr[CSR_MEPC as usize]))
                }
                0x1020_0073 => {
                    // SRET
                    if self.privilege == Privilege::User {
                        return Err(Self::raise_illegal(u64::from(insn)));
                    }
                    let mut status = self.mstatus();
                    let target_priv = if status & MSTATUS_SPP != 0 {
                        Privilege::Supervisor
                    } else {
                        Privilege::User
                    };
                    if status & MSTATUS_SPIE != 0 {
                        status |= MSTATUS_SIE;
                    } else {
                        status &= !MSTATUS_SIE;
                    }
                    status |= MSTATUS_SPIE;
                    status &= !MSTATUS_SPP;
                    self.csr[CSR_MSTATUS as usize] = status;
                    self.privilege = target_priv;
                    Ok(Flow::Next(self.csr[CSR_SEPC as usize]))
                }
                0x1050_0073 => Ok(Flow::Wfi(next)),
                _ if (insn >> 25) == 0b000_1001 => Ok(Flow::Next(next)), // SFENCE.VMA: no TLB cache
                _ => Err(Self::raise_illegal(u64::from(insn))),
            }
        } else {
            // CSR instructions.
            let address = ((insn >> 20) & 0xFFF) as u16;
            let rd = ((insn >> 7) & 0x1F) as usize;
            let rs1 = ((insn >> 15) & 0x1F) as usize;
            let min_privilege = (address >> 8) & 0b11;
            let has_privilege = matches!(
                (min_privilege, self.privilege),
                (0, _) | (1, Privilege::Supervisor | Privilege::Machine) | (3, Privilege::Machine)
            );
            if !has_privilege {
                return Err(Self::raise_illegal(u64::from(insn)));
            }
            let read_only = (address >> 10) & 0b11 == 0b11;
            let (operand, will_write) = match funct3 {
                0b001 => (self.reg(rs1), true),
                0b010 | 0b011 => (self.reg(rs1), rs1 != 0),
                0b101 => (u64::from(rs1 as u32), true),
                0b110 | 0b111 => (u64::from(rs1 as u32), rs1 != 0),
                _ => return Err(Self::raise_illegal(u64::from(insn))),
            };
            if read_only && will_write {
                return Err(Self::raise_illegal(u64::from(insn)));
            }
            let old = match self.read_csr(address) {
                Some(v) => v,
                None => return Err(Self::raise_illegal(u64::from(insn))),
            };
            if will_write {
                let new_value = match funct3 {
                    0b001 | 0b101 => operand,
                    0b010 | 0b110 => old | operand,
                    _ => old & !operand,
                };
                if !self.write_csr(address, new_value) {
                    return Err(Self::raise_illegal(u64::from(insn)));
                }
            }
            self.set_reg(rd, old);
            Ok(Flow::Next(next))
        }
    }

    fn execute_atomic(&mut self, bus: &mut SystemBus, insn: u32) -> Result<(), Trap> {
        let funct3 = (insn >> 12) & 0x7;
        let rd = ((insn >> 7) & 0x1F) as usize;
        let rs1 = ((insn >> 15) & 0x1F) as usize;
        let rs2 = ((insn >> 20) & 0x1F) as usize;
        let funct5 = (insn >> 27) & 0x1F;
        let address = self.reg(rs1);
        let size = match funct3 {
            0b010 => 4,
            0b011 => 8,
            _ => return Err(Self::raise_illegal(u64::from(insn))),
        };
        match funct5 {
            0b00010 => {
                // LR
                if rs2 != 0 {
                    return Err(Self::raise_illegal(u64::from(insn)));
                }
                let value = self.load_memory(bus, address, size, Access::Load)?;
                self.reservation = Some(address);
                self.set_reg(rd, sext(value, (size * 8) as u32));
            }
            0b00011 => {
                // SC (single hart: succeeds iff a matching reservation is held)
                if self.reservation == Some(address) {
                    self.store_memory(bus, address, size, self.reg(rs2))?;
                    self.set_reg(rd, 0);
                } else {
                    self.set_reg(rd, 1);
                }
                self.reservation = None;
            }
            _ => {
                // AMO: read-modify-write, result is the original value.
                let original = self.load_memory(bus, address, size, Access::Load)?;
                let operand = self.reg(rs2);
                let updated = if size == 4 {
                    let (x, y) = (original as u32, operand as u32);
                    u64::from(match funct5 {
                        0b00000 => x.wrapping_add(y),
                        0b00001 => y,
                        0b00100 => x ^ y,
                        0b01000 => x | y,
                        0b01100 => x & y,
                        0b10000 => ((x as i32).min(y as i32)) as u32,
                        0b10100 => ((x as i32).max(y as i32)) as u32,
                        0b11000 => x.min(y),
                        0b11100 => x.max(y),
                        _ => return Err(Self::raise_illegal(u64::from(insn))),
                    })
                } else {
                    match funct5 {
                        0b00000 => original.wrapping_add(operand),
                        0b00001 => operand,
                        0b00100 => original ^ operand,
                        0b01000 => original | operand,
                        0b01100 => original & operand,
                        0b10000 => ((original as i64).min(operand as i64)) as u64,
                        0b10100 => ((original as i64).max(operand as i64)) as u64,
                        0b11000 => original.min(operand),
                        0b11100 => original.max(operand),
                        _ => return Err(Self::raise_illegal(u64::from(insn))),
                    }
                };
                self.store_memory(bus, address, size, updated)?;
                self.set_reg(rd, sext(original, (size * 8) as u32));
                self.reservation = None;
            }
        }
        Ok(())
    }

    /// RVC (compressed) decoder. Floating-point loads/stores are illegal
    /// until the F/D extensions land, matching `misa`.
    #[allow(clippy::too_many_lines)]
    fn execute_compressed(&mut self, bus: &mut SystemBus, insn: u16) -> Result<Flow, Trap> {
        let pc = self.pc;
        let next = pc + 2;
        let raw = u64::from(insn);
        if insn == 0 {
            return Err(Self::raise_illegal(0));
        }
        let funct3 = (insn >> 13) & 0x7;
        let rd_full = ((insn >> 7) & 0x1F) as usize;
        let rs1_prime = 8 + ((insn >> 7) & 0x7) as usize;
        let rd_prime = 8 + ((insn >> 2) & 0x7) as usize;
        let imm6 = sext(raw >> 2 & 0x1F | ((raw >> 12) & 1) << 5, 6);

        match insn & 0b11 {
            0b00 => {
                // Quadrant 0: stack-relative and register-offset memory ops.
                let lw_offset = (((insn >> 10) & 0x7) << 3)
                    | (((insn >> 6) & 1) << 2)
                    | (((insn >> 5) & 1) << 6);
                let ld_offset = (((insn >> 10) & 0x7) << 3) | (((insn >> 5) & 0x3) << 6);
                match funct3 {
                    0b000 => {
                        // C.ADDI4SPN
                        let imm = (((insn >> 11) & 0x3) << 4)
                            | (((insn >> 7) & 0xF) << 6)
                            | (((insn >> 6) & 1) << 2)
                            | (((insn >> 5) & 1) << 3);
                        if imm == 0 {
                            return Err(Self::raise_illegal(raw));
                        }
                        let value = self.reg(2).wrapping_add(u64::from(imm));
                        self.set_reg(rd_prime, value);
                    }
                    0b010 => {
                        let address = self.reg(rs1_prime).wrapping_add(u64::from(lw_offset));
                        let value = self.load_memory(bus, address, 4, Access::Load)?;
                        self.set_reg(rd_prime, sext(value, 32));
                    }
                    0b011 => {
                        let address = self.reg(rs1_prime).wrapping_add(u64::from(ld_offset));
                        let value = self.load_memory(bus, address, 8, Access::Load)?;
                        self.set_reg(rd_prime, value);
                    }
                    0b110 => {
                        let address = self.reg(rs1_prime).wrapping_add(u64::from(lw_offset));
                        self.store_memory(bus, address, 4, self.reg(rd_prime))?;
                    }
                    0b111 => {
                        let address = self.reg(rs1_prime).wrapping_add(u64::from(ld_offset));
                        self.store_memory(bus, address, 8, self.reg(rd_prime))?;
                    }
                    _ => return Err(Self::raise_illegal(raw)),
                }
            }
            0b01 => {
                match funct3 {
                    0b000 => {
                        // C.ADDI / C.NOP
                        let value = self.reg(rd_full).wrapping_add(imm6);
                        self.set_reg(rd_full, value);
                    }
                    0b001 => {
                        // C.ADDIW
                        if rd_full == 0 {
                            return Err(Self::raise_illegal(raw));
                        }
                        let value = self.reg(rd_full).wrapping_add(imm6);
                        self.set_reg(rd_full, sext(value & 0xFFFF_FFFF, 32));
                    }
                    0b010 => self.set_reg(rd_full, imm6), // C.LI
                    0b011 => {
                        if rd_full == 2 {
                            // C.ADDI16SP
                            let imm = (((insn >> 12) & 1) << 9)
                                | (((insn >> 6) & 1) << 4)
                                | (((insn >> 5) & 1) << 6)
                                | (((insn >> 3) & 0x3) << 7)
                                | (((insn >> 2) & 1) << 5);
                            if imm == 0 {
                                return Err(Self::raise_illegal(raw));
                            }
                            let value = self.reg(2).wrapping_add(sext(u64::from(imm), 10));
                            self.set_reg(2, value);
                        } else {
                            // C.LUI
                            if rd_full == 0 || imm6 == 0 {
                                return Err(Self::raise_illegal(raw));
                            }
                            self.set_reg(rd_full, imm6 << 12);
                        }
                    }
                    0b100 => {
                        let shamt = (((insn >> 12) & 1) << 5) | ((insn >> 2) & 0x1F);
                        let a = self.reg(rs1_prime);
                        match (insn >> 10) & 0x3 {
                            0b00 => self.set_reg(rs1_prime, a >> shamt),
                            0b01 => self.set_reg(rs1_prime, ((a as i64) >> shamt) as u64),
                            0b10 => {
                                let value = a & imm6;
                                self.set_reg(rs1_prime, value);
                            }
                            _ => {
                                let b = self.reg(rd_prime);
                                let sub = (insn >> 5) & 0x3;
                                let value = if (insn >> 12) & 1 == 0 {
                                    match sub {
                                        0b00 => a.wrapping_sub(b),
                                        0b01 => a ^ b,
                                        0b10 => a | b,
                                        _ => a & b,
                                    }
                                } else {
                                    match sub {
                                        0b00 => sext(a.wrapping_sub(b) & 0xFFFF_FFFF, 32),
                                        0b01 => sext(a.wrapping_add(b) & 0xFFFF_FFFF, 32),
                                        _ => return Err(Self::raise_illegal(raw)),
                                    }
                                };
                                self.set_reg(rs1_prime, value);
                            }
                        }
                    }
                    0b101 => return Ok(Flow::Next(pc.wrapping_add(cj_offset(insn)))), // C.J
                    0b110 | 0b111 => {
                        // C.BEQZ / C.BNEZ
                        let is_zero = self.reg(rs1_prime) == 0;
                        if (funct3 == 0b110) == is_zero {
                            return Ok(Flow::Next(pc.wrapping_add(cb_offset(insn))));
                        }
                    }
                    _ => unreachable!(),
                }
            }
            0b10 => {
                let rs2 = ((insn >> 2) & 0x1F) as usize;
                match funct3 {
                    0b000 => {
                        // C.SLLI
                        if rd_full == 0 {
                            return Err(Self::raise_illegal(raw));
                        }
                        let shamt = (((insn >> 12) & 1) << 5) | ((insn >> 2) & 0x1F);
                        let value = self.reg(rd_full) << shamt;
                        self.set_reg(rd_full, value);
                    }
                    0b010 => {
                        // C.LWSP
                        if rd_full == 0 {
                            return Err(Self::raise_illegal(raw));
                        }
                        let offset = (((insn >> 12) & 1) << 5)
                            | (((insn >> 4) & 0x7) << 2)
                            | (((insn >> 2) & 0x3) << 6);
                        let address = self.reg(2).wrapping_add(u64::from(offset));
                        let value = self.load_memory(bus, address, 4, Access::Load)?;
                        self.set_reg(rd_full, sext(value, 32));
                    }
                    0b011 => {
                        // C.LDSP
                        if rd_full == 0 {
                            return Err(Self::raise_illegal(raw));
                        }
                        let offset = (((insn >> 12) & 1) << 5)
                            | (((insn >> 5) & 0x3) << 3)
                            | (((insn >> 2) & 0x7) << 6);
                        let address = self.reg(2).wrapping_add(u64::from(offset));
                        let value = self.load_memory(bus, address, 8, Access::Load)?;
                        self.set_reg(rd_full, value);
                    }
                    0b100 => {
                        if (insn >> 12) & 1 == 0 {
                            if rs2 == 0 {
                                // C.JR
                                if rd_full == 0 {
                                    return Err(Self::raise_illegal(raw));
                                }
                                return Ok(Flow::Next(self.reg(rd_full)));
                            }
                            // C.MV
                            let value = self.reg(rs2);
                            self.set_reg(rd_full, value);
                        } else if rs2 == 0 && rd_full == 0 {
                            return Err(Trap {
                                cause: CAUSE_BREAKPOINT,
                                tval: pc,
                            });
                        } else if rs2 == 0 {
                            // C.JALR
                            let target = self.reg(rd_full);
                            self.set_reg(1, next);
                            return Ok(Flow::Next(target));
                        } else {
                            // C.ADD
                            let value = self.reg(rd_full).wrapping_add(self.reg(rs2));
                            self.set_reg(rd_full, value);
                        }
                    }
                    0b110 => {
                        // C.SWSP
                        let offset = (((insn >> 9) & 0xF) << 2) | (((insn >> 7) & 0x3) << 6);
                        let address = self.reg(2).wrapping_add(u64::from(offset));
                        self.store_memory(bus, address, 4, self.reg(rs2))?;
                        self.reservation = None;
                    }
                    0b111 => {
                        // C.SDSP
                        let offset = (((insn >> 10) & 0x7) << 3) | (((insn >> 7) & 0x7) << 6);
                        let address = self.reg(2).wrapping_add(u64::from(offset));
                        self.store_memory(bus, address, 8, self.reg(rs2))?;
                        self.reservation = None;
                    }
                    _ => return Err(Self::raise_illegal(raw)),
                }
            }
            _ => unreachable!(),
        }
        Ok(Flow::Next(next))
    }
}

fn cj_offset(insn: u16) -> u64 {
    let imm = (((insn >> 12) & 1) << 11)
        | (((insn >> 11) & 1) << 4)
        | (((insn >> 9) & 0x3) << 8)
        | (((insn >> 8) & 1) << 10)
        | (((insn >> 7) & 1) << 6)
        | (((insn >> 6) & 1) << 7)
        | (((insn >> 3) & 0x7) << 1)
        | (((insn >> 2) & 1) << 5);
    sext(u64::from(imm), 12)
}

fn cb_offset(insn: u16) -> u64 {
    let imm = (((insn >> 12) & 1) << 8)
        | (((insn >> 10) & 0x3) << 6)
        | (((insn >> 5) & 0x3) << 3)
        | (((insn >> 3) & 0x3) << 1)
        | (((insn >> 2) & 1) << 5);
    sext(u64::from(imm), 9)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn trap_vector(tvec: u64, cause: u64, is_interrupt: bool) -> u64 {
    let base = tvec & !0b11;
    if is_interrupt && (tvec & 1) != 0 {
        base + 4 * (cause & ((1_u64 << 63) - 1))
    } else {
        base
    }
}

/// Sign-extends the low `bits` of `value` to 64 bits.
fn sext(value: u64, bits: u32) -> u64 {
    let shift = 64 - bits;
    (((value << shift) as i64) >> shift) as u64
}

fn sext64_32(value: u64) -> u64 {
    sext(value & 0xFFFF_FFFF, 32)
}

fn imm_i(insn: u32) -> u64 {
    sext(u64::from(insn >> 20), 12)
}

fn imm_s(insn: u32) -> u64 {
    let imm = (((insn >> 25) & 0x7F) << 5) | ((insn >> 7) & 0x1F);
    sext(u64::from(imm), 12)
}

fn imm_b(insn: u32) -> u64 {
    let imm = (((insn >> 31) & 1) << 12)
        | (((insn >> 7) & 1) << 11)
        | (((insn >> 25) & 0x3F) << 5)
        | (((insn >> 8) & 0xF) << 1);
    sext(u64::from(imm), 13)
}

fn imm_j(insn: u32) -> u64 {
    let imm = (((insn >> 31) & 1) << 20)
        | (((insn >> 12) & 0xFF) << 12)
        | (((insn >> 20) & 1) << 11)
        | (((insn >> 21) & 0x3FF) << 1);
    sext(u64::from(imm), 21)
}

fn div_signed(a: u64, b: u64) -> u64 {
    let (x, y) = (a as i64, b as i64);
    if y == 0 {
        u64::MAX
    } else {
        x.wrapping_div(y) as u64
    }
}

fn rem_signed(a: u64, b: u64) -> u64 {
    let (x, y) = (a as i64, b as i64);
    if y == 0 {
        a
    } else {
        x.wrapping_rem(y) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::{Cpu, Privilege, StepOutcome};
    use crate::bus::{SystemBus, RAM_BASE};

    // --- tiny assemblers so tests build instructions from fields, not hex --
    fn i_type(imm: i32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
        (((imm as u32) & 0xFFF) << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | opcode
    }
    fn r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
        (funct7 << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | opcode
    }
    fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
        i_type(imm, rs1, 0, rd, 0x13)
    }

    fn machine_with(program: &[u32]) -> (Cpu, SystemBus) {
        let mut bus = SystemBus::new(1024 * 1024).expect("bus");
        for (i, word) in program.iter().enumerate() {
            bus.ram_mut()
                .write_int(RAM_BASE + (i as u64) * 4, 4, u64::from(*word))
                .expect("load program");
        }
        (Cpu::new(RAM_BASE), bus)
    }

    fn run(cpu: &mut Cpu, bus: &mut SystemBus, steps: usize) {
        for _ in 0..steps {
            assert_eq!(cpu.step(bus, false), StepOutcome::Executed);
        }
    }

    #[test]
    fn integer_arithmetic_and_multiplication() {
        // x1 = 40 + 2; x2 = 6; x3 = x1 * x2; x4 = x3 / x2; x5 = x1 - x2
        let program = [
            addi(1, 0, 40),
            addi(1, 1, 2),
            addi(2, 0, 6),
            r_type(1, 2, 1, 0, 3, 0x33),     // mul x3, x1, x2
            r_type(1, 2, 3, 0b100, 4, 0x33), // div x4, x3, x2
            r_type(0x20, 2, 1, 0, 5, 0x33),  // sub x5, x1, x2
        ];
        let (mut cpu, mut bus) = machine_with(&program);
        run(&mut cpu, &mut bus, program.len());
        assert_eq!(cpu.reg(1), 42);
        assert_eq!(cpu.reg(3), 252);
        assert_eq!(cpu.reg(4), 42);
        assert_eq!(cpu.reg(5), 36);
        assert_eq!(cpu.reg(0), 0);
    }

    #[test]
    fn division_by_zero_uses_spec_results() {
        let program = [
            addi(1, 0, 7),
            r_type(1, 0, 1, 0b100, 2, 0x33), // div x2, x1, x0 -> -1
            r_type(1, 0, 1, 0b110, 3, 0x33), // rem x3, x1, x0 -> 7
        ];
        let (mut cpu, mut bus) = machine_with(&program);
        run(&mut cpu, &mut bus, program.len());
        assert_eq!(cpu.reg(2), u64::MAX);
        assert_eq!(cpu.reg(3), 7);
    }

    #[test]
    fn loads_stores_and_sign_extension() {
        // sd/ld a full word, lb a negative byte.
        let data_addr = RAM_BASE + 0x400;
        let program = [
            // x1 = 0x8000_0000 + 0x400, built as (1 << 31) + 0x400.
            addi(1, 0, 1),
            i_type(31, 1, 0b001, 1, 0x13), // slli x1, x1, 31
            addi(1, 1, 0x400),
            addi(2, 0, -5),
            // sb x2, 0(x1)
            ((2 << 20) | (1 << 15) | 0x23),
            // lb x3, 0(x1)
            i_type(0, 1, 0, 3, 0x03),
            // lbu x4, 0(x1)
            i_type(0, 1, 0b100, 4, 0x03),
            // sd x2, 8(x1) ; ld x5, 8(x1)
            ((2 << 20) | (1 << 15) | (0b011 << 12) | (8 << 7) | 0x23),
            i_type(8, 1, 0b011, 5, 0x03),
        ];
        let (mut cpu, mut bus) = machine_with(&program);
        run(&mut cpu, &mut bus, program.len());
        assert_eq!(cpu.reg(1), data_addr);
        assert_eq!(cpu.reg(3) as i64, -5);
        assert_eq!(cpu.reg(4), 251);
        assert_eq!(cpu.reg(5) as i64, -5);
    }

    #[test]
    fn branch_skips_instructions() {
        // x1=1; beq x1,x1,+8 (skip next); x1=99; x2=7
        let beq = (1 << 20) | (1 << 15) | (0b0100 << 8) | 0x63;
        let program = [addi(1, 0, 1), beq, addi(1, 0, 99), addi(2, 0, 7)];
        let (mut cpu, mut bus) = machine_with(&program);
        run(&mut cpu, &mut bus, 3);
        assert_eq!(cpu.reg(1), 1);
        assert_eq!(cpu.reg(2), 7);
    }

    #[test]
    fn csr_write_read_and_misa() {
        // csrw mscratch, x1 (csrrw x0, 0x340, x1); csrr x2, misa (0x301)
        let program = [
            addi(1, 0, 77),
            i_type(0x340, 1, 0b001, 0, 0x73),
            i_type(0x340, 0, 0b010, 3, 0x73), // csrrs x3, mscratch, x0 = read
            i_type(0x301, 0, 0b010, 2, 0x73), // csrr x2, misa
        ];
        let (mut cpu, mut bus) = machine_with(&program);
        run(&mut cpu, &mut bus, program.len());
        assert_eq!(cpu.reg(3), 77);
        let misa = cpu.reg(2);
        assert_eq!(misa >> 62, 2, "MXL must be RV64");
        assert_ne!(misa & (1 << 8), 0, "I extension bit");
        assert_eq!(misa & (1 << 5), 0, "F must honestly read as absent");
    }

    #[test]
    fn illegal_instruction_traps_to_m_mode() {
        // mtvec = RAM_BASE+0x100 handler: csrr x1, mcause; (then we stop)
        let handler = i_type(0x342, 0, 0b010, 1, 0x73);
        let mut program = vec![
            addi(1, 0, 1),
            i_type(31, 1, 0b001, 1, 0x13), // slli x1, x1, 31 -> 0x80000000
            addi(1, 1, 0x100),             // x1 = handler addr
            i_type(0x305, 1, 0b001, 0, 0x73), // csrw mtvec, x1
            0xFFFF_FFFF,                   // illegal instruction
        ];
        program.resize(0x40, 0x0000_0013); // nops up to +0x100
        program.push(handler);
        let (mut cpu, mut bus) = machine_with(&program);
        run(&mut cpu, &mut bus, 5);
        assert_eq!(cpu.pc(), RAM_BASE + 0x100);
        assert_eq!(cpu.privilege(), Privilege::Machine);
        cpu.step(&mut bus, false);
        assert_eq!(cpu.reg(1), 2, "mcause must be illegal instruction (2)");
    }

    #[test]
    fn ecall_from_supervisor_intercepts_or_traps() {
        let (mut cpu, mut bus) = machine_with(&[0x0000_0073]);
        cpu.reset_supervisor(RAM_BASE, 0, RAM_BASE + 0x1000);
        assert_eq!(cpu.step(&mut bus, true), StepOutcome::SbiCall);
        assert_eq!(cpu.pc(), RAM_BASE, "PC must stay on ecall for the shim");
    }

    #[test]
    fn sv39_translate_maps_and_protects() {
        use super::Access;
        let (mut cpu, mut bus) = machine_with(&[0x0000_0013]);
        cpu.privilege = Privilege::Supervisor;
        // Root table at RAM+0x1000, mid at +0x2000, leaf at +0x3000.
        // VA 0x40_0000: VPN2=0, VPN1=0x100? compute: 0x400000 >> 21 = 2 -> VPN1=2, VPN2=0.
        let root = RAM_BASE + 0x1000;
        let mid = RAM_BASE + 0x2000;
        let leaf = RAM_BASE + 0x3000;
        let data_page = RAM_BASE + 0x8000;
        let data_page_ro = RAM_BASE + 0x9000;
        let ppn = |addr: u64| (addr >> 12) << 10;
        bus.ram_mut()
            .write_int(root, 8, ppn(mid) | 1)
            .expect("root pte");
        bus.ram_mut()
            .write_int(mid + 2 * 8, 8, ppn(leaf) | 1)
            .expect("mid pte");
        // V|R|W|A|D leaf for VPN0=0 → data_page; and a V|R leaf for VPN0=1.
        bus.ram_mut()
            .write_int(leaf, 8, ppn(data_page) | 0xC7)
            .expect("leaf rw");
        bus.ram_mut()
            .write_int(leaf + 8, 8, ppn(data_page_ro) | 0x03)
            .expect("leaf ro");
        cpu.write_csr(
            super::CSR_SATP,
            (8_u64 << 60) | ((root >> 12) & ((1 << 44) - 1)),
        );
        assert_eq!(
            cpu.translate(&mut bus, 0x40_0000 + 0x42, Access::Load)
                .expect("translate"),
            data_page + 0x42
        );
        assert!(
            cpu.translate(&mut bus, 0x40_1000, Access::Store).is_err(),
            "RO page must fault on store"
        );
        assert!(
            cpu.translate(&mut bus, 0x40_1000, Access::Fetch).is_err(),
            "non-X page must fault on fetch"
        );
        assert!(
            cpu.translate(&mut bus, 0x80_0000, Access::Load).is_err(),
            "unmapped must fault"
        );
        // Sv48 attempt is WARL-rejected: satp keeps Sv39 mode.
        let before = cpu.read_csr(super::CSR_SATP).expect("satp");
        cpu.write_csr(super::CSR_SATP, (9_u64 << 60) | 0x1234);
        assert_eq!(cpu.read_csr(super::CSR_SATP).expect("satp after"), before);
    }
}
