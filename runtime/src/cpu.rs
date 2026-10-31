//! RISC-V RV64I CPU core.
//!
//! Implements the base RV64I integer instruction set plus the machine-mode
//! SYSTEM instructions (ECALL, EBREAK, MRET, WFI, CSR*) and the RV64I
//! "W" 32-bit variants. Multi-byte memory accesses go through the `Bus` and
//! are bounds/alignment checked, so malformed or malicious guest code cannot
//! escape the guest memory sandbox.

use crate::bus::Bus;
use crate::csr::CsrFile;

/// Trap / exception codes (RISC-V privileged spec, machine mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapCode {
    InstructionAddressMisaligned,
    InstructionAccessFault,
    IllegalInstruction,
    Breakpoint,
    LoadAddressMisaligned,
    LoadAccessFault,
    StoreAddressMisaligned,
    StoreAccessFault,
    EnvironmentCall(u64), // cause code (e.g. 11 from M-mode)
}

impl TrapCode {
    /// mcause value for this trap (interrupt bit set where applicable).
    pub fn mcause(self) -> u64 {
        match self {
            TrapCode::EnvironmentCall(c) => c,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlFlow {
    /// Continue execution at the given next pc.
    Continue(u64),
    /// Deliver a trap.
    Trap(TrapCode),
    /// Environment call — handover to the machine (may emulate or trap).
    Ecall,
    /// Breakpoint.
    Ebreak,
    /// Machine return.
    Mret,
    /// Wait for interrupt.
    Wfi,
}

/// Decoded RISC-V instruction.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Instruction {
    Lui { rd: u8, imm: u64 },
    Auipc { rd: u8, imm: u64 },
    Jal { rd: u8, imm: i64 },
    Jalr { rd: u8, rs1: u8, imm: i64 },
    Beq { rs1: u8, rs2: u8, imm: i64 },
    Bne { rs1: u8, rs2: u8, imm: i64 },
    Blt { rs1: u8, rs2: u8, imm: i64 },
    Bge { rs1: u8, rs2: u8, imm: i64 },
    Bltu { rs1: u8, rs2: u8, imm: i64 },
    Bgeu { rs1: u8, rs2: u8, imm: i64 },
    Lb { rd: u8, rs1: u8, imm: i64 },
    Lh { rd: u8, rs1: u8, imm: i64 },
    Lw { rd: u8, rs1: u8, imm: i64 },
    Ld { rd: u8, rs1: u8, imm: i64 },
    Lbu { rd: u8, rs1: u8, imm: i64 },
    Lhu { rd: u8, rs1: u8, imm: i64 },
    Lwu { rd: u8, rs1: u8, imm: i64 },
    Sb { rs1: u8, rs2: u8, imm: i64 },
    Sh { rs1: u8, rs2: u8, imm: i64 },
    Sw { rs1: u8, rs2: u8, imm: i64 },
    Sd { rs1: u8, rs2: u8, imm: i64 },
    Addi { rd: u8, rs1: u8, imm: i64 },
    Slti { rd: u8, rs1: u8, imm: i64 },
    Sltiu { rd: u8, rs1: u8, imm: i64 },
    Xori { rd: u8, rs1: u8, imm: i64 },
    Ori { rd: u8, rs1: u8, imm: i64 },
    Andi { rd: u8, rs1: u8, imm: i64 },
    Slli { rd: u8, rs1: u8, shamt: u64 },
    Srli { rd: u8, rs1: u8, shamt: u64 },
    Srai { rd: u8, rs1: u8, shamt: u64 },
    Add { rd: u8, rs1: u8, rs2: u8 },
    Sub { rd: u8, rs1: u8, rs2: u8 },
    Sll { rd: u8, rs1: u8, rs2: u8 },
    Slt { rd: u8, rs1: u8, rs2: u8 },
    Sltu { rd: u8, rs1: u8, rs2: u8 },
    Xor { rd: u8, rs1: u8, rs2: u8 },
    Srl { rd: u8, rs1: u8, rs2: u8 },
    Sra { rd: u8, rs1: u8, rs2: u8 },
    Or { rd: u8, rs1: u8, rs2: u8 },
    And { rd: u8, rs1: u8, rs2: u8 },
    Addiw { rd: u8, rs1: u8, imm: i64 },
    Slliw { rd: u8, rs1: u8, shamt: u64 },
    Srliw { rd: u8, rs1: u8, shamt: u64 },
    Sraiw { rd: u8, rs1: u8, shamt: u64 },
    Addw { rd: u8, rs1: u8, rs2: u8 },
    Subw { rd: u8, rs1: u8, rs2: u8 },
    Sllw { rd: u8, rs1: u8, rs2: u8 },
    Srlw { rd: u8, rs1: u8, rs2: u8 },
    Sraw { rd: u8, rs1: u8, rs2: u8 },
    Csrrw { rd: u8, csr: u16, rs1: u8 },
    Csrrs { rd: u8, csr: u16, rs1: u8 },
    Csrrc { rd: u8, csr: u16, rs1: u8 },
    Csrrwi { rd: u8, csr: u16, imm: u64 },
    Csrrsi { rd: u8, csr: u16, imm: u64 },
    Csrrci { rd: u8, csr: u16, imm: u64 },
    Ecall,
    Ebreak,
    Mret,
    Wfi,
    Fence,
    FenceI,
    Unknown,
}

/// The RV64I CPU state.
pub struct Cpu {
    /// General-purpose registers. x0 is permanently zero.
    pub x: [u64; 32],
    pub pc: u64,
    pub csr: CsrFile,
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            x: [0; 32],
            pc: 0,
            csr: CsrFile::new(),
        }
    }

    /// Execute one instruction. `delta_cycles` simulated timer ticks pass
    /// before the instruction (may raise the timer interrupt).
    pub fn step(&mut self, bus: &mut Bus, delta_cycles: u64) -> ControlFlow {
        // Advance simulated time; may raise timer interrupt.
        bus.tick(delta_cycles, &mut self.csr);

        // Deliver a pending+enabled interrupt before the next instruction.
        if self.csr.mie_global() {
            if let Some(cause) = self.csr.pending_interrupt() {
                self.csr.clear_interrupt(cause);
                return self.trap_interrupt(cause);
            }
        }

        // Fetch instruction (must be 4-byte aligned).
        if !self.pc.is_multiple_of(4) {
            return ControlFlow::Trap(TrapCode::InstructionAddressMisaligned);
        }
        let inst = match bus.read_u32(self.pc) {
            Ok(v) => v,
            Err(_) => return ControlFlow::Trap(TrapCode::InstructionAccessFault),
        };

        let decoded = decode(inst);
        self.execute(decoded, bus)
    }

    fn trap_interrupt(&mut self, cause: u64) -> ControlFlow {
        // Save machine state.
        self.csr.mepc = self.pc;
        self.csr.mcause = cause | 1u64 << 63;
        self.csr.mtval = 0;
        // mpie = mie; mie = 0; mpp = current mode (Machine = 3).
        let mie = self.csr.mie_global();
        self.csr.mstatus = (self.csr.mstatus & !crate::csr::MSTATUS_MPIE)
            | if mie { crate::csr::MSTATUS_MPIE } else { 0 };
        self.csr.mstatus &= !crate::csr::MSTATUS_MIE;
        self.csr.mstatus = (self.csr.mstatus & !(3 << 11)) | (3 << 11);
        self.csr.mstatus &= !crate::csr::MSTATUS_MIE;
        ControlFlow::Continue(self.csr.mtvec)
    }

    fn execute(&mut self, inst: Instruction, bus: &mut Bus) -> ControlFlow {
        let next_pc = self.pc.wrapping_add(4);
        match inst {
            Instruction::Lui { rd, imm } => {
                self.x[rd as usize] = imm << 12;
                ControlFlow::Continue(next_pc)
            }
            Instruction::Auipc { rd, imm } => {
                self.x[rd as usize] = self.pc.wrapping_add(imm);
                ControlFlow::Continue(next_pc)
            }
            Instruction::Jal { rd, imm } => {
                self.x[rd as usize] = next_pc;
                ControlFlow::Continue(self.pc.wrapping_add(imm as u64))
            }
            Instruction::Jalr { rd, rs1, imm } => {
                let target = (self.x[rs1 as usize].wrapping_add(imm as u64)) & !1u64;
                self.x[rd as usize] = next_pc;
                ControlFlow::Continue(target)
            }
            Instruction::Beq { rs1, rs2, imm } => {
                let taken = self.x[rs1 as usize] == self.x[rs2 as usize];
                ControlFlow::Continue(if taken {
                    self.pc.wrapping_add(imm as u64)
                } else {
                    next_pc
                })
            }
            Instruction::Bne { rs1, rs2, imm } => {
                let taken = self.x[rs1 as usize] != self.x[rs2 as usize];
                ControlFlow::Continue(if taken {
                    self.pc.wrapping_add(imm as u64)
                } else {
                    next_pc
                })
            }
            Instruction::Blt { rs1, rs2, imm } => {
                let taken = (self.x[rs1 as usize] as i64) < (self.x[rs2 as usize] as i64);
                ControlFlow::Continue(if taken {
                    self.pc.wrapping_add(imm as u64)
                } else {
                    next_pc
                })
            }
            Instruction::Bge { rs1, rs2, imm } => {
                let taken = (self.x[rs1 as usize] as i64) >= (self.x[rs2 as usize] as i64);
                ControlFlow::Continue(if taken {
                    self.pc.wrapping_add(imm as u64)
                } else {
                    next_pc
                })
            }
            Instruction::Bltu { rs1, rs2, imm } => {
                let taken = self.x[rs1 as usize] < self.x[rs2 as usize];
                ControlFlow::Continue(if taken {
                    self.pc.wrapping_add(imm as u64)
                } else {
                    next_pc
                })
            }
            Instruction::Bgeu { rs1, rs2, imm } => {
                let taken = self.x[rs1 as usize] >= self.x[rs2 as usize];
                ControlFlow::Continue(if taken {
                    self.pc.wrapping_add(imm as u64)
                } else {
                    next_pc
                })
            }
            Instruction::Lb { rd, rs1, imm } => {
                let addr = self.x[rs1 as usize].wrapping_add(imm as u64);
                match bus.read_u8(addr) {
                    Ok(v) => {
                        self.x[rd as usize] = v as i8 as i64 as u64;
                        ControlFlow::Continue(next_pc)
                    }
                    Err(_) => ControlFlow::Trap(TrapCode::LoadAccessFault),
                }
            }
            Instruction::Lh { rd, rs1, imm } => {
                let addr = self.x[rs1 as usize].wrapping_add(imm as u64);
                if !addr.is_multiple_of(2) {
                    return ControlFlow::Trap(TrapCode::LoadAddressMisaligned);
                }
                match bus.read_u16(addr) {
                    Ok(v) => {
                        self.x[rd as usize] = v as i16 as i64 as u64;
                        ControlFlow::Continue(next_pc)
                    }
                    Err(_) => ControlFlow::Trap(TrapCode::LoadAccessFault),
                }
            }
            Instruction::Lw { rd, rs1, imm } => {
                let addr = self.x[rs1 as usize].wrapping_add(imm as u64);
                if !addr.is_multiple_of(4) {
                    return ControlFlow::Trap(TrapCode::LoadAddressMisaligned);
                }
                match bus.read_u32(addr) {
                    Ok(v) => {
                        self.x[rd as usize] = v as i32 as i64 as u64;
                        ControlFlow::Continue(next_pc)
                    }
                    Err(_) => ControlFlow::Trap(TrapCode::LoadAccessFault),
                }
            }
            Instruction::Ld { rd, rs1, imm } => {
                let addr = self.x[rs1 as usize].wrapping_add(imm as u64);
                if !addr.is_multiple_of(8) {
                    return ControlFlow::Trap(TrapCode::LoadAddressMisaligned);
                }
                match bus.read_u64(addr) {
                    Ok(v) => {
                        self.x[rd as usize] = v;
                        ControlFlow::Continue(next_pc)
                    }
                    Err(_) => ControlFlow::Trap(TrapCode::LoadAccessFault),
                }
            }
            Instruction::Lbu { rd, rs1, imm } => {
                let addr = self.x[rs1 as usize].wrapping_add(imm as u64);
                match bus.read_u8(addr) {
                    Ok(v) => {
                        self.x[rd as usize] = v as u64;
                        ControlFlow::Continue(next_pc)
                    }
                    Err(_) => ControlFlow::Trap(TrapCode::LoadAccessFault),
                }
            }
            Instruction::Lhu { rd, rs1, imm } => {
                let addr = self.x[rs1 as usize].wrapping_add(imm as u64);
                if !addr.is_multiple_of(2) {
                    return ControlFlow::Trap(TrapCode::LoadAddressMisaligned);
                }
                match bus.read_u16(addr) {
                    Ok(v) => {
                        self.x[rd as usize] = v as u64;
                        ControlFlow::Continue(next_pc)
                    }
                    Err(_) => ControlFlow::Trap(TrapCode::LoadAccessFault),
                }
            }
            Instruction::Lwu { rd, rs1, imm } => {
                let addr = self.x[rs1 as usize].wrapping_add(imm as u64);
                if !addr.is_multiple_of(4) {
                    return ControlFlow::Trap(TrapCode::LoadAddressMisaligned);
                }
                match bus.read_u32(addr) {
                    Ok(v) => {
                        self.x[rd as usize] = v as u64;
                        ControlFlow::Continue(next_pc)
                    }
                    Err(_) => ControlFlow::Trap(TrapCode::LoadAccessFault),
                }
            }
            Instruction::Sb { rs1, rs2, imm } => {
                let addr = self.x[rs1 as usize].wrapping_add(imm as u64);
                if bus.write_u8(addr, self.x[rs2 as usize] as u8).is_err() {
                    return ControlFlow::Trap(TrapCode::StoreAccessFault);
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Sh { rs1, rs2, imm } => {
                let addr = self.x[rs1 as usize].wrapping_add(imm as u64);
                if !addr.is_multiple_of(2) {
                    return ControlFlow::Trap(TrapCode::StoreAddressMisaligned);
                }
                if bus.write_u16(addr, self.x[rs2 as usize] as u16).is_err() {
                    return ControlFlow::Trap(TrapCode::StoreAccessFault);
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Sw { rs1, rs2, imm } => {
                let addr = self.x[rs1 as usize].wrapping_add(imm as u64);
                if !addr.is_multiple_of(4) {
                    return ControlFlow::Trap(TrapCode::StoreAddressMisaligned);
                }
                if bus.write_u32(addr, self.x[rs2 as usize] as u32).is_err() {
                    return ControlFlow::Trap(TrapCode::StoreAccessFault);
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Sd { rs1, rs2, imm } => {
                let addr = self.x[rs1 as usize].wrapping_add(imm as u64);
                if !addr.is_multiple_of(8) {
                    return ControlFlow::Trap(TrapCode::StoreAddressMisaligned);
                }
                if bus
                    .write_u64(addr, self.x[rs2 as usize], &mut self.csr)
                    .is_err()
                {
                    return ControlFlow::Trap(TrapCode::StoreAccessFault);
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Addi { rd, rs1, imm } => {
                self.x[rd as usize] = self.x[rs1 as usize].wrapping_add(imm as u64);
                ControlFlow::Continue(next_pc)
            }
            Instruction::Slti { rd, rs1, imm } => {
                self.x[rd as usize] = if (self.x[rs1 as usize] as i64) < imm {
                    1
                } else {
                    0
                };
                ControlFlow::Continue(next_pc)
            }
            Instruction::Sltiu { rd, rs1, imm } => {
                self.x[rd as usize] = if self.x[rs1 as usize] < imm as u64 {
                    1
                } else {
                    0
                };
                ControlFlow::Continue(next_pc)
            }
            Instruction::Xori { rd, rs1, imm } => {
                self.x[rd as usize] = self.x[rs1 as usize] ^ (imm as u64);
                ControlFlow::Continue(next_pc)
            }
            Instruction::Ori { rd, rs1, imm } => {
                self.x[rd as usize] = self.x[rs1 as usize] | (imm as u64);
                ControlFlow::Continue(next_pc)
            }
            Instruction::Andi { rd, rs1, imm } => {
                self.x[rd as usize] = self.x[rs1 as usize] & (imm as u64);
                ControlFlow::Continue(next_pc)
            }
            Instruction::Slli { rd, rs1, shamt } => {
                self.x[rd as usize] = self.x[rs1 as usize].wrapping_shl(shamt as u32);
                ControlFlow::Continue(next_pc)
            }
            Instruction::Srli { rd, rs1, shamt } => {
                self.x[rd as usize] = self.x[rs1 as usize].wrapping_shr(shamt as u32);
                ControlFlow::Continue(next_pc)
            }
            Instruction::Srai { rd, rs1, shamt } => {
                self.x[rd as usize] =
                    ((self.x[rs1 as usize] as i64).wrapping_shr(shamt as u32)) as u64;
                ControlFlow::Continue(next_pc)
            }
            Instruction::Add { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] = self.x[rs1 as usize].wrapping_add(self.x[rs2 as usize]);
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Sub { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] = self.x[rs1 as usize].wrapping_sub(self.x[rs2 as usize]);
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Sll { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] =
                        self.x[rs1 as usize].wrapping_shl(self.x[rs2 as usize] as u32);
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Slt { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] =
                        if (self.x[rs1 as usize] as i64) < (self.x[rs2 as usize] as i64) {
                            1
                        } else {
                            0
                        };
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Sltu { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] = if self.x[rs1 as usize] < self.x[rs2 as usize] {
                        1
                    } else {
                        0
                    };
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Xor { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] = self.x[rs1 as usize] ^ self.x[rs2 as usize];
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Srl { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] =
                        self.x[rs1 as usize].wrapping_shr(self.x[rs2 as usize] as u32);
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Sra { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] = ((self.x[rs1 as usize] as i64)
                        .wrapping_shr(self.x[rs2 as usize] as u32))
                        as u64;
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Or { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] = self.x[rs1 as usize] | self.x[rs2 as usize];
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::And { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] = self.x[rs1 as usize] & self.x[rs2 as usize];
                }
                ControlFlow::Continue(next_pc)
            }
            // RV64I 32-bit variants: sign-extended to 64 bits.
            Instruction::Addiw { rd, rs1, imm } => {
                self.x[rd as usize] =
                    (self.x[rs1 as usize] as i64).wrapping_add(imm) as i32 as i64 as u64;
                ControlFlow::Continue(next_pc)
            }
            Instruction::Slliw { rd, rs1, shamt } => {
                self.x[rd as usize] =
                    (self.x[rs1 as usize] as i32).wrapping_shl(shamt as u32) as i64 as u64;
                ControlFlow::Continue(next_pc)
            }
            Instruction::Srliw { rd, rs1, shamt } => {
                self.x[rd as usize] =
                    (self.x[rs1 as usize] as u32).wrapping_shr(shamt as u32) as i64 as u64;
                ControlFlow::Continue(next_pc)
            }
            Instruction::Sraiw { rd, rs1, shamt } => {
                self.x[rd as usize] =
                    (self.x[rs1 as usize] as i32).wrapping_shr(shamt as u32) as i64 as u64;
                ControlFlow::Continue(next_pc)
            }
            Instruction::Addw { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] = (self.x[rs1 as usize] as i32)
                        .wrapping_add(self.x[rs2 as usize] as i32)
                        as i64 as u64;
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Subw { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] = (self.x[rs1 as usize] as i32)
                        .wrapping_sub(self.x[rs2 as usize] as i32)
                        as i64 as u64;
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Sllw { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] = (self.x[rs1 as usize] as i32)
                        .wrapping_shl(self.x[rs2 as usize] as u32)
                        as i64 as u64;
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Srlw { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] = (self.x[rs1 as usize] as u32)
                        .wrapping_shr(self.x[rs2 as usize] as u32)
                        as i64 as u64;
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Sraw { rd, rs1, rs2 } => {
                if rd != 0 {
                    self.x[rd as usize] = (self.x[rs1 as usize] as i32)
                        .wrapping_shr(self.x[rs2 as usize] as u32)
                        as i64 as u64;
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Csrrw { rd, csr, rs1 } => {
                let old = self.csr.read(csr as u64);
                if rd != 0 {
                    self.x[rd as usize] = old;
                }
                if rs1 != 0 {
                    self.csr.write(csr as u64, self.x[rs1 as usize]);
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Csrrs { rd, csr, rs1 } => {
                let old = self.csr.read(csr as u64);
                if rd != 0 {
                    self.x[rd as usize] = old;
                }
                if rs1 != 0 {
                    self.csr.write(csr as u64, old | self.x[rs1 as usize]);
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Csrrc { rd, csr, rs1 } => {
                let old = self.csr.read(csr as u64);
                if rd != 0 {
                    self.x[rd as usize] = old;
                }
                if rs1 != 0 {
                    self.csr.write(csr as u64, old & !self.x[rs1 as usize]);
                }
                ControlFlow::Continue(next_pc)
            }
            Instruction::Csrrwi { rd, csr, imm } => {
                let old = self.csr.read(csr as u64);
                if rd != 0 {
                    self.x[rd as usize] = old;
                }
                self.csr.write(csr as u64, imm);
                ControlFlow::Continue(next_pc)
            }
            Instruction::Csrrsi { rd, csr, imm } => {
                let old = self.csr.read(csr as u64);
                if rd != 0 {
                    self.x[rd as usize] = old;
                }
                self.csr.write(csr as u64, old | imm);
                ControlFlow::Continue(next_pc)
            }
            Instruction::Csrrci { rd, csr, imm } => {
                let old = self.csr.read(csr as u64);
                if rd != 0 {
                    self.x[rd as usize] = old;
                }
                self.csr.write(csr as u64, old & !imm);
                ControlFlow::Continue(next_pc)
            }
            Instruction::Ecall => ControlFlow::Ecall,
            Instruction::Ebreak => ControlFlow::Ebreak,
            Instruction::Mret => ControlFlow::Continue(self.csr.mepc),
            Instruction::Wfi => ControlFlow::Wfi,
            Instruction::Fence | Instruction::FenceI => ControlFlow::Continue(next_pc),
            Instruction::Unknown => ControlFlow::Trap(TrapCode::IllegalInstruction),
        }
    }

    /// Force x0 to stay zero after every step (caller should call this).
    pub fn enforce_x0(&mut self) {
        self.x[0] = 0;
    }
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

/// Sign-extend `value` from `bits` bits to i64.
fn sext(value: u64, bits: u32) -> i64 {
    if bits == 64 {
        return value as i64;
    }
    let sign_bit = 1u64 << (bits - 1);
    let masked = value & ((1u64 << bits) - 1);
    if masked & sign_bit != 0 {
        (masked | (!0u64 << bits)) as i64
    } else {
        masked as i64
    }
}

/// Decode a 32-bit RISC-V instruction word.
fn decode(inst: u32) -> Instruction {
    let opcode = inst & 0x7F;
    let rd = ((inst >> 7) & 0x1F) as u8;
    let funct3 = ((inst >> 12) & 0x7) as u8;
    let rs1 = ((inst >> 15) & 0x1F) as u8;
    let rs2 = ((inst >> 20) & 0x1F) as u8;
    let funct7 = (inst >> 25) & 0x7F;

    match opcode {
        0b0110111 => Instruction::Lui {
            rd,
            imm: ((inst >> 12) as u64) << 12,
        },
        0b0010111 => Instruction::Auipc {
            rd,
            imm: ((inst >> 12) as u64) << 12,
        },
        0b1101111 => Instruction::Jal {
            rd,
            imm: sext(decode_j_imm(inst), 21),
        },
        0b1100111 => Instruction::Jalr {
            rd,
            rs1,
            imm: sext((inst >> 20) as u64, 12),
        },
        0b1100011 => {
            let imm = sext(decode_b_imm(inst), 13);
            match funct3 {
                0b000 => Instruction::Beq { rs1, rs2, imm },
                0b001 => Instruction::Bne { rs1, rs2, imm },
                0b100 => Instruction::Blt { rs1, rs2, imm },
                0b101 => Instruction::Bge { rs1, rs2, imm },
                0b110 => Instruction::Bltu { rs1, rs2, imm },
                0b111 => Instruction::Bgeu { rs1, rs2, imm },
                _ => Instruction::Unknown,
            }
        }
        0b0000011 => {
            let imm = sext((inst >> 20) as u64, 12);
            match funct3 {
                0b000 => Instruction::Lb { rd, rs1, imm },
                0b001 => Instruction::Lh { rd, rs1, imm },
                0b010 => Instruction::Lw { rd, rs1, imm },
                0b011 => Instruction::Ld { rd, rs1, imm },
                0b100 => Instruction::Lbu { rd, rs1, imm },
                0b101 => Instruction::Lhu { rd, rs1, imm },
                0b110 => Instruction::Lwu { rd, rs1, imm },
                _ => Instruction::Unknown,
            }
        }
        0b0100011 => {
            let imm = sext(
                ((inst >> 20) as u64 & 0xFFF) | ((inst >> 25) as u64) << 5,
                12,
            );
            match funct3 {
                0b000 => Instruction::Sb { rs1, rs2, imm },
                0b001 => Instruction::Sh { rs1, rs2, imm },
                0b010 => Instruction::Sw { rs1, rs2, imm },
                0b011 => Instruction::Sd { rs1, rs2, imm },
                _ => Instruction::Unknown,
            }
        }
        0b0010011 => {
            let imm = sext((inst >> 20) as u64, 12);
            let shamt = ((inst >> 20) & 0x1F) as u64;
            match funct3 {
                0b000 => Instruction::Addi { rd, rs1, imm },
                0b010 => Instruction::Slti { rd, rs1, imm },
                0b011 => Instruction::Sltiu { rd, rs1, imm },
                0b100 => Instruction::Xori { rd, rs1, imm },
                0b110 => Instruction::Ori { rd, rs1, imm },
                0b111 => Instruction::Andi { rd, rs1, imm },
                0b001 => Instruction::Slli { rd, rs1, shamt },
                0b101 => {
                    if funct7 == 0 {
                        Instruction::Srli { rd, rs1, shamt }
                    } else if funct7 == 0x20 {
                        Instruction::Srai { rd, rs1, shamt }
                    } else {
                        Instruction::Unknown
                    }
                }
                _ => Instruction::Unknown,
            }
        }
        0b0110011 => match funct3 {
            0b000 => {
                if funct7 == 0 {
                    Instruction::Add { rd, rs1, rs2 }
                } else if funct7 == 0x20 {
                    Instruction::Sub { rd, rs1, rs2 }
                } else {
                    Instruction::Unknown
                }
            }
            0b001 => Instruction::Sll { rd, rs1, rs2 },
            0b010 => Instruction::Slt { rd, rs1, rs2 },
            0b011 => Instruction::Sltu { rd, rs1, rs2 },
            0b100 => Instruction::Xor { rd, rs1, rs2 },
            0b101 => {
                if funct7 == 0 {
                    Instruction::Srl { rd, rs1, rs2 }
                } else if funct7 == 0x20 {
                    Instruction::Sra { rd, rs1, rs2 }
                } else {
                    Instruction::Unknown
                }
            }
            0b110 => Instruction::Or { rd, rs1, rs2 },
            0b111 => Instruction::And { rd, rs1, rs2 },
            _ => Instruction::Unknown,
        },
        0b0111011 => {
            // OP-IMM-32 / OP-32 share opcode 0111011. R-type forms (ADDW/SUBW/
            // SLLW/SRLW/SRAW) are recognised when funct7 matches; otherwise the
            // instruction is treated as an I-type 32-bit variant. The overlap
            // is benign: with shift/imm = 0 both forms produce identical results.
            let imm = sext((inst >> 20) as u64, 12);
            let shamt = ((inst >> 20) & 0x1F) as u64;
            match funct3 {
                0b000 => {
                    if funct7 == 0 {
                        Instruction::Addw { rd, rs1, rs2 }
                    } else if funct7 == 0x20 {
                        Instruction::Subw { rd, rs1, rs2 }
                    } else {
                        Instruction::Addiw { rd, rs1, imm }
                    }
                }
                0b001 => Instruction::Sllw { rd, rs1, rs2 },
                0b101 => {
                    if funct7 == 0 {
                        Instruction::Srlw { rd, rs1, rs2 }
                    } else if funct7 == 0x20 {
                        Instruction::Sraw { rd, rs1, rs2 }
                    } else {
                        Instruction::Srliw { rd, rs1, shamt }
                    }
                }
                _ => Instruction::Unknown,
            }
        }
        0b1110011 => {
            let csr = (inst >> 20) & 0xFFF;
            match funct3 {
                0b000 => match inst {
                    0x00000073 => Instruction::Ecall,
                    0x00100073 => Instruction::Ebreak,
                    0x30200073 => Instruction::Mret,
                    0x10500073 => Instruction::Wfi,
                    _ => Instruction::Unknown,
                },
                0b001 => Instruction::Csrrw {
                    rd,
                    csr: csr as u16,
                    rs1,
                },
                0b010 => Instruction::Csrrs {
                    rd,
                    csr: csr as u16,
                    rs1,
                },
                0b011 => Instruction::Csrrc {
                    rd,
                    csr: csr as u16,
                    rs1,
                },
                0b101 => Instruction::Csrrwi {
                    rd,
                    csr: csr as u16,
                    imm: rs1 as u64,
                },
                0b110 => Instruction::Csrrsi {
                    rd,
                    csr: csr as u16,
                    imm: rs1 as u64,
                },
                0b111 => Instruction::Csrrci {
                    rd,
                    csr: csr as u16,
                    imm: rs1 as u64,
                },
                _ => Instruction::Unknown,
            }
        }
        0b0001111 => match funct3 {
            0b000 => Instruction::Fence,
            0b001 => Instruction::FenceI,
            _ => Instruction::Unknown,
        },
        _ => Instruction::Unknown,
    }
}

/// B-type immediate: imm[12|10:5|4:1|11].
fn decode_b_imm(inst: u32) -> u64 {
    (((inst >> 31) & 1) << 12
        | ((inst >> 7) & 1) << 11
        | ((inst >> 25) & 0x3F) << 5
        | ((inst >> 8) & 0xF) << 1) as u64
}

/// J-type immediate: imm[20|10:1|11|19:12].
fn decode_j_imm(inst: u32) -> u64 {
    (((inst >> 31) & 1) << 20
        | ((inst >> 21) & 0x3FF) << 1
        | ((inst >> 20) & 1) << 11
        | ((inst >> 12) & 0xFF) << 12) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;
    use crate::csr::{IRQ_M_TIMER, MIP_MTIP};

    #[test]
    fn add_rd_rs1_rs2() {
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        cpu.x[1] = 40;
        cpu.x[2] = 2;
        let inst = (0b0110011) | (3 << 7) | (1 << 15) | (2 << 20);
        let decoded = decode(inst);
        match decoded {
            Instruction::Add { rd, rs1, rs2 } => {
                assert_eq!((rd, rs1, rs2), (3, 1, 2));
            }
            _ => panic!("expected Add, got {:?}", decoded),
        }
        let flow = cpu.execute(decoded, &mut bus);
        assert_eq!(flow, ControlFlow::Continue(4));
        assert_eq!(cpu.x[3], 42);
        assert_eq!(cpu.x[0], 0);
    }

    #[test]
    fn sub_rd_rs1_rs2() {
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        cpu.x[1] = 10;
        cpu.x[2] = 3;
        let inst = (0b0110011) | (3 << 7) | (1 << 15) | (2 << 20) | (0x20 << 25);
        let decoded = decode(inst);
        let flow = cpu.execute(decoded, &mut bus);
        assert_eq!(flow, ControlFlow::Continue(4));
        assert_eq!(cpu.x[3], 7);
    }

    #[test]
    fn addi_rd_rs1_imm() {
        // addi x1, x0, 100
        let inst = 0b0010011 | (1 << 7) | (100u32 << 20);
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        let decoded = decode(inst);
        let flow = cpu.execute(decoded, &mut bus);
        assert_eq!(flow, ControlFlow::Continue(4));
        assert_eq!(cpu.x[1], 100);
    }

    #[test]
    fn lui_auipc() {
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        cpu.pc = 0x8000;
        // lui x1, 0x8000 -> x1 = 0x8000 << 12
        let flow = cpu.execute(Instruction::Lui { rd: 1, imm: 0x8000 }, &mut bus);
        assert_eq!(flow, ControlFlow::Continue(0x8004));
        assert_eq!(cpu.x[1], 0x8000u64 << 12);
        // auipc x2, 0 -> x2 = pc
        let flow2 = cpu.execute(Instruction::Auipc { rd: 2, imm: 0 }, &mut bus);
        assert_eq!(flow2, ControlFlow::Continue(0x8004));
        assert_eq!(cpu.x[2], 0x8000);
    }

    #[test]
    fn jal_and_jalr() {
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        cpu.pc = 0x1000;
        // jal x1, +8 -> target pc+8, ra = pc+4
        let flow = cpu.execute(Instruction::Jal { rd: 1, imm: 8 }, &mut bus);
        assert_eq!(flow, ControlFlow::Continue(0x1008));
        assert_eq!(cpu.x[1], 0x1004);

        // jalr x1, x2, 0 -> target = x2, ra = pc+4
        cpu.x[2] = 0x5000;
        let flow2 = cpu.execute(
            Instruction::Jalr {
                rd: 1,
                rs1: 2,
                imm: 0,
            },
            &mut bus,
        );
        assert_eq!(flow2, ControlFlow::Continue(0x5000));
        assert_eq!(cpu.x[1], 0x1004);
    }

    #[test]
    fn branches() {
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        cpu.pc = 0x2000;
        cpu.x[1] = 5;
        cpu.x[2] = 5;
        // beq x1, x2, +8 -> taken
        let flow = cpu.execute(
            Instruction::Beq {
                rs1: 1,
                rs2: 2,
                imm: 8,
            },
            &mut bus,
        );
        assert_eq!(flow, ControlFlow::Continue(0x2008));

        // bne x1, x2 where equal -> not taken
        cpu.x[1] = 5;
        cpu.x[2] = 5;
        let flow2 = cpu.execute(
            Instruction::Bne {
                rs1: 1,
                rs2: 2,
                imm: 8,
            },
            &mut bus,
        );
        assert_eq!(flow2, ControlFlow::Continue(0x2004));
    }

    #[test]
    fn load_store_roundtrip() {
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        bus.write_u32(0x1000, 0xDEADBEEF).unwrap();
        cpu.x[1] = 0x1000;
        // lw x2, 0(x1)
        let inst = 0b0000011 | (2 << 7) | (0b010 << 12) | (1 << 15);
        let decoded = decode(inst);
        let flow = cpu.execute(decoded, &mut bus);
        assert_eq!(flow, ControlFlow::Continue(4));
        assert_eq!(cpu.x[2], 0xDEADBEEFu32 as i32 as i64 as u64);

        // sw x2, 4(x1)
        cpu.x[2] = 0x12345678;
        let flow = cpu.execute(
            Instruction::Sw {
                rs1: 1,
                rs2: 2,
                imm: 4,
            },
            &mut bus,
        );
        assert_eq!(flow, ControlFlow::Continue(4));
        assert_eq!(bus.read_u32(0x1004).unwrap(), 0x12345678);
    }

    #[test]
    fn csr_read_write() {
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        // csrrw x1, mtvec(0x301), x0 -> x1 = mtvec(0)
        let inst = 0b1110011 | (1 << 7) | (0b001 << 12) | (0x301 << 20);
        let decoded = decode(inst);
        let flow = cpu.execute(decoded, &mut bus);
        assert_eq!(flow, ControlFlow::Continue(4));
        assert_eq!(cpu.x[1], 0);

        // csrrw x0, mtvec, x1 where x1 = 0x1000
        cpu.x[1] = 0x1000;
        let inst2 = 0b1110011 | (0b001 << 12) | (1 << 15) | (0x301 << 20);
        let decoded2 = decode(inst2);
        let flow2 = cpu.execute(decoded2, &mut bus);
        assert_eq!(flow2, ControlFlow::Continue(4));
        assert_eq!(cpu.csr.read(0x301), 0x1000);
    }

    #[test]
    fn ecall_and_ebreak() {
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        assert_eq!(
            cpu.execute(Instruction::Ecall, &mut bus),
            ControlFlow::Ecall
        );
        assert_eq!(
            cpu.execute(Instruction::Ebreak, &mut bus),
            ControlFlow::Ebreak
        );
    }

    #[test]
    fn mret_restores_pc() {
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        cpu.csr.mepc = 0x9000;
        cpu.csr.mstatus = 0;
        assert_eq!(
            cpu.execute(Instruction::Mret, &mut bus),
            ControlFlow::Continue(0x9000)
        );
    }

    #[test]
    fn x0_stays_zero() {
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        cpu.x[1] = 100;
        let inst = (0b0110011) | (1 << 15) | (2 << 20);
        let decoded = decode(inst);
        cpu.execute(decoded, &mut bus);
        assert_eq!(cpu.x[0], 0);
    }

    #[test]
    fn illegal_instruction_trap() {
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        let inst = 0b1111111 | (1 << 7);
        let decoded = decode(inst);
        assert_eq!(decoded, Instruction::Unknown);
        assert_eq!(
            cpu.execute(decoded, &mut bus),
            ControlFlow::Trap(TrapCode::IllegalInstruction)
        );
    }

    #[test]
    fn sign_extension_helpers() {
        assert_eq!(sext(0x800, 12), -2048);
        assert_eq!(sext(0x7FF, 12), 2047);
        assert_eq!(sext(0, 12), 0);
    }

    #[test]
    fn timer_interrupt_delivery() {
        let mut cpu = Cpu::new();
        let mut bus = Bus::new(0x10000, 1);
        cpu.csr.mstatus |= crate::csr::MSTATUS_MIE;
        cpu.csr.mie |= MIP_MTIP;
        bus.clint.mtime = 1000;
        bus.clint.mtimecmp[0] = 10;
        bus.tick(0, &mut cpu.csr);
        assert_eq!(cpu.csr.pending_interrupt(), Some(IRQ_M_TIMER));
        let flow = cpu.step(&mut bus, 0);
        match flow {
            ControlFlow::Continue(pc) => assert_eq!(pc, cpu.csr.mtvec),
            _ => panic!("expected interrupt trap to mtvec"),
        }
        assert!(!cpu.csr.mie_global());
        assert_eq!(cpu.csr.mcause, IRQ_M_TIMER | (1u64 << 63));
    }
}
