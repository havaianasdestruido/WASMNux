#![allow(dead_code)]

//! Machine: ties the CPU, bus (memory + CLINT + UART), and device tree
//! together into a runnable guest. Provides the step/loop lifecycle and the
//! surface used by the WASM/TypeScript bindings.

use crate::bus::Bus;
use crate::cpu::{ControlFlow, Cpu, TrapCode};
use crate::device_tree::FdtBuilder;
use crate::elf::{load_elf64, ElfError};
use crate::memory::MemoryError;
use crate::uart::{UART_BASE, UART_THR};

/// Result of a single executed instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    Executed,
    EcallHandled,
    Ebreak,
    WaitForInterrupt,
    Halted,
}

/// Errors the machine can produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineError {
    Trap(TrapCode),
    Elf(ElfError),
    Memory(MemoryError),
    AlreadyRunning,
    NotRunning,
    InvalidArgument(String),
}

impl core::fmt::Display for MachineError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MachineError::Trap(t) => write!(f, "guest trap: {:?}", t),
            MachineError::Elf(e) => write!(f, "elf error: {}", e),
            MachineError::Memory(e) => write!(f, "memory error: {}", e),
            MachineError::AlreadyRunning => write!(f, "machine is already running"),
            MachineError::NotRunning => write!(f, "machine is not running"),
            MachineError::InvalidArgument(s) => write!(f, "invalid argument: {}", s),
        }
    }
}

impl std::error::Error for MachineError {}

/// Configuration for a machine.
#[derive(Debug, Clone, Copy)]
pub struct MachineConfig {
    /// Guest RAM size in bytes.
    pub mem_size: usize,
    /// Number of harts.
    pub harts: usize,
    /// Simulated timer ticks advanced per executed instruction.
    pub ticks_per_instruction: u64,
    /// Load address for the device tree blob.
    pub dtb_load_addr: u64,
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            mem_size: 256 * 1024 * 1024, // 256 MiB
            harts: 1,
            ticks_per_instruction: 1,
            dtb_load_addr: 0x0800_0000,
        }
    }
}

/// A runnable guest machine.
pub struct Machine {
    pub config: MachineConfig,
    pub bus: Bus,
    pub cpu: Cpu,
    pub running: bool,
    pub cycles: u64,
    pub entry: u64,
    pub halt_requested: bool,
}

impl Machine {
    pub fn new(config: MachineConfig) -> Self {
        let mut bus = Bus::new(config.mem_size, config.harts);
        // Default machine timer compare far in the future.
        bus.clint.mtimecmp[0] = u64::MAX;
        Self {
            config,
            bus,
            cpu: Cpu::new(),
            running: false,
            cycles: 0,
            entry: 0,
            halt_requested: false,
        }
    }

    /// Load an ELF64 image into guest memory and set the entry point.
    pub fn load_elf(&mut self, image: &[u8]) -> Result<u64, MachineError> {
        let entry = load_elf64(image, &mut self.bus.mem).map_err(MachineError::Elf)?;
        self.entry = entry;
        Ok(entry)
    }

    /// Build and load a minimal device tree, returning its guest address.
    pub fn load_device_tree(&mut self) -> Result<u64, MachineError> {
        let mut fdt = FdtBuilder::new();
        fdt.begin_node("/");
        fdt.prop_string("compatible", "riscv-virt");
        fdt.prop_u32("#address-cells", 2);
        fdt.prop_u32("#size-cells", 2);
        fdt.end_node();

        fdt.begin_node("memory");
        fdt.prop_u64("reg", 0x8000_0000);
        fdt.prop_u64("reg", self.config.mem_size as u64);
        fdt.end_node();

        fdt.begin_node("cpus");
        fdt.prop_u32("#address-cells", 2);
        fdt.prop_u32("#size-cells", 0);
        fdt.begin_node("cpu@0");
        fdt.prop_u32("reg", 0);
        fdt.prop_string("device_type", "cpu");
        fdt.end_node();
        fdt.end_node();

        fdt.begin_node("soc");
        fdt.prop_u32("#address-cells", 2);
        fdt.prop_u32("#size-cells", 2);
        fdt.begin_node("clint");
        fdt.prop_u32("reg", 0x0200_0000);
        fdt.prop_u64("reg", 0x10000);
        fdt.end_node();
        fdt.begin_node("uart@10000000");
        fdt.prop_u32("reg", 0x1000_0000);
        fdt.prop_u64("reg", 0x1000);
        fdt.end_node();
        fdt.end_node();

        fdt.begin_node("chosen");
        fdt.prop_string("bootargs", "console=ttyS0 earlyprintk");
        fdt.end_node();

        let blob = fdt.finish();
        let addr = self.config.dtb_load_addr;
        self.bus
            .mem
            .write_bytes(addr, &blob)
            .map_err(MachineError::Memory)?;
        Ok(addr)
    }

    /// Set the initial guest arguments (a0 = hart id, a1 = dtb pointer, pc = entry).
    pub fn set_boot_args(&mut self, hart_id: u64, dtb_addr: u64) {
        self.cpu.pc = self.entry;
        self.cpu.x[10] = hart_id; // a0
        self.cpu.x[11] = dtb_addr; // a1
        self.cpu.x[0] = 0;
    }

    /// Execute a single instruction.
    pub fn step(&mut self) -> Result<StepResult, MachineError> {
        if self.halt_requested {
            return Ok(StepResult::Halted);
        }
        let flow = self
            .cpu
            .step(&mut self.bus, self.config.ticks_per_instruction);
        self.cycles += 1;
        self.cpu.enforce_x0();
        match flow {
            ControlFlow::Continue(pc) => {
                self.cpu.pc = pc;
                Ok(StepResult::Executed)
            }
            ControlFlow::Ecall => self.handle_ecall(),
            ControlFlow::Ebreak => Ok(StepResult::Ebreak),
            ControlFlow::Mret => {
                self.cpu.pc = self.cpu.csr.mepc;
                Ok(StepResult::Executed)
            }
            ControlFlow::Wfi => {
                // Wait: advance time and check for a pending interrupt.
                self.bus
                    .tick(self.config.ticks_per_instruction, &mut self.cpu.csr);
                if self.cpu.csr.mie_global() && self.cpu.csr.pending_interrupt().is_some() {
                    Ok(StepResult::Executed) // next step will deliver the interrupt
                } else {
                    Ok(StepResult::WaitForInterrupt)
                }
            }
            ControlFlow::Trap(code) => Err(MachineError::Trap(code)),
        }
    }

    /// Handle a machine-mode ECALL. Implements a tiny SBI-like console putchar
    /// (a7 == 1, char in a0) and shutdown (a7 == 93).
    fn handle_ecall(&mut self) -> Result<StepResult, MachineError> {
        let a7 = self.cpu.x[17];
        match a7 {
            1 => {
                let ch = self.cpu.x[10] as u8; // a0
                self.bus.uart.write_byte(UART_BASE + UART_THR, ch);
                self.cpu.pc = self.cpu.pc.wrapping_add(4);
                Ok(StepResult::EcallHandled)
            }
            93 => {
                self.halt_requested = true;
                Ok(StepResult::Halted)
            }
            _ => Err(MachineError::Trap(TrapCode::EnvironmentCall(a7))),
        }
    }

    /// Run until halt, breakpoint, max_instructions, or error.
    pub fn run(&mut self, max_instructions: u64) -> Result<StepResult, MachineError> {
        self.running = true;
        let mut result = StepResult::Executed;
        for _ in 0..max_instructions {
            match self.step()? {
                StepResult::Halted | StepResult::Ebreak => {
                    result = StepResult::Halted;
                    break;
                }
                StepResult::WaitForInterrupt => {
                    // Avoid spinning forever with no interrupt: treat as halt after
                    // a few iterations is handled by max_instructions.
                    result = StepResult::WaitForInterrupt;
                    break;
                }
                r => result = r,
            }
        }
        self.running = false;
        Ok(result)
    }

    /// Request a graceful halt.
    pub fn halt(&mut self) {
        self.halt_requested = true;
    }

    /// Reset the machine to a clean state (keeps config).
    pub fn reset(&mut self) {
        self.bus = Bus::new(self.config.mem_size, self.config.harts);
        self.bus.clint.mtimecmp[0] = u64::MAX;
        self.cpu = Cpu::new();
        self.running = false;
        self.cycles = 0;
        self.entry = 0;
        self.halt_requested = false;
    }

    /// Read guest memory into a host slice (bounds-checked).
    pub fn read_mem(&self, addr: u64, buf: &mut [u8]) -> Result<(), MemoryError> {
        self.bus.mem.read_bytes(addr, buf)
    }

    /// Write a host slice into guest memory (bounds-checked).
    pub fn write_mem(&mut self, addr: u64, buf: &[u8]) -> Result<(), MemoryError> {
        self.bus.mem.write_bytes(addr, buf)
    }

    /// Guest memory size in bytes.
    pub fn mem_size(&self) -> u64 {
        self.bus.mem.len()
    }

    /// Current program counter.
    pub fn pc(&self) -> u64 {
        self.cpu.pc
    }

    /// Register value by index.
    pub fn reg(&self, idx: usize) -> Option<u64> {
        if idx < 32 {
            Some(self.cpu.x[idx])
        } else {
            None
        }
    }

    /// Drain the serial console output since the last drain.
    pub fn drain_console(&mut self) -> Vec<u8> {
        self.bus.drain_uart()
    }

    /// Total simulated cycles executed.
    pub fn cycles(&self) -> u64 {
        self.cycles
    }

    /// Current mtime (simulated time base).
    pub fn mtime(&self) -> u64 {
        self.bus.clint.mtime
    }
}

impl Default for Machine {
    fn default() -> Self {
        Self::new(MachineConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_machine_defaults() {
        let m = Machine::new(MachineConfig::default());
        assert_eq!(m.mem_size(), 256 * 1024 * 1024);
        assert_eq!(m.pc(), 0);
        assert!(!m.running);
    }

    #[test]
    fn reset_clears_state() {
        let mut m = Machine::new(MachineConfig::default());
        m.bus.mem.write_u32(0x100, 0x42).unwrap();
        m.cpu.x[1] = 7;
        m.reset();
        assert_eq!(m.cpu.x[1], 0);
        assert_eq!(m.bus.read_u32(0x100).unwrap(), 0);
    }

    #[test]
    fn set_boot_args() {
        let mut m = Machine::new(MachineConfig::default());
        m.entry = 0x8000;
        m.set_boot_args(0, 0x88000000);
        assert_eq!(m.pc(), 0x8000);
        assert_eq!(m.reg(10), Some(0));
        assert_eq!(m.reg(11), Some(0x88000000));
    }

    #[test]
    fn load_device_tree_writes_blob() {
        let mut m = Machine::new(MachineConfig::default());
        let addr = m.load_device_tree().unwrap();
        assert_eq!(addr, 0x0800_0000);
        let mut buf = [0u8; 4];
        m.read_mem(addr, &mut buf).unwrap();
        assert_eq!(u32::from_be_bytes(buf), 0xD00D_FEED);
    }

    #[test]
    fn ecall_putchar_prints() {
        let mut m = Machine::new(MachineConfig {
            mem_size: 0x1000,
            ..Default::default()
        });
        m.bus.mem.write_u32(0x100, 0x00000073).unwrap(); // ecall
        m.cpu.x[17] = 1; // a7 = putchar
        m.cpu.x[10] = b'H' as u64; // a0
        m.cpu.pc = 0x100;
        let r = m.step().unwrap();
        assert_eq!(r, StepResult::EcallHandled);
        assert_eq!(m.drain_console(), vec![b'H']);
    }

    #[test]
    fn ecall_exit_halt() {
        let mut m = Machine::new(MachineConfig {
            mem_size: 0x1000,
            ..Default::default()
        });
        m.bus.mem.write_u32(0x100, 0x00000073).unwrap(); // ecall
        m.cpu.x[17] = 93; // a7 = exit
        m.cpu.pc = 0x100;
        let r = m.step().unwrap();
        assert_eq!(r, StepResult::Halted);
        assert!(m.halt_requested);
    }

    #[test]
    fn run_executes_simple_loop() {
        // Program: li x1,5; li x2,3; add x3,x1,x2; ecall exit
        // Build instructions manually:
        // li x1,5 -> addi x1,x0,5 : 0b0010011 | (1<<7) | (0<<12) | (0<<15) | (5<<20)
        // li x2,3 -> addi x2,x0,3
        // add x3,x1,x2 : 0b0110011 | (3<<7) | (0<<12) | (1<<15) | (2<<20) | (0<<25)
        // ecall exit : a7=93; ecall
        let mut m = Machine::new(MachineConfig {
            mem_size: 0x10000,
            ..Default::default()
        });
        let base: u64 = 0x1000;
        m.bus
            .mem
            .write_u32(base, 0b0010011 | (1 << 7) | (5 << 20))
            .unwrap(); // addi x1,x0,5
        m.bus
            .mem
            .write_u32(base + 4, 0b0010011 | (2 << 7) | (3 << 20))
            .unwrap(); // addi x2,x0,3
        m.bus
            .mem
            .write_u32(base + 8, 0b0110011 | (3 << 7) | (1 << 15) | (2 << 20))
            .unwrap(); // add x3,x1,x2
                       // set a7=93 then ecall
        m.bus
            .mem
            .write_u32(base + 12, 0b0010011 | (17 << 7) | (93 << 20))
            .unwrap(); // addi x17,x0,93
        m.bus.mem.write_u32(base + 16, 0x00000073).unwrap(); // ecall
        m.cpu.pc = base;
        let result = m.run(100).unwrap();
        assert_eq!(result, StepResult::Halted);
        assert_eq!(m.reg(3), Some(8));
        assert!(m.halt_requested);
    }
}
