//! Whole-machine assembly: CPU + bus + boot protocol + lifecycle.
//! This module is deliberately the *only* place that knows how a kernel
//! image, firmware ELF, initrd, DTB and disk become a booting system.

use crate::block::VecBlockBackend;
use crate::bus::{SystemBus, KERNEL_OFFSET, RAM_BASE};
use crate::cpu::{Cpu, StepOutcome};
use crate::dtb::{build_machine_dtb, FramebufferInfo, MachineDtbParams};
use crate::elf::ElfImage;
use crate::error::{Result, VmError};
use crate::plic::{CONTEXT_M, CONTEXT_S};
use crate::sbi::{handle_sbi_call, SbiOutcome};

/// Space reserved at the top of RAM (below the framebuffer) for the DTB.
const DTB_RESERVE: u64 = 64 * 1024;
const PAGE: u64 = 4096;

/// Which firmware path boots the kernel (see `sbi.rs` / ARCHITECTURE.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BootMode {
    /// OpenSBI (or compatible) firmware ELF runs in M-mode first.
    #[default]
    Firmware,
    /// Kernel entry in S-mode, SBI serviced by the built-in shim.
    DirectSbi,
}

/// User-visible machine configuration.
#[derive(Debug, Clone)]
pub struct MachineConfig {
    pub ram_size_bytes: usize,
    pub bootargs: String,
    pub framebuffer: Option<(u32, u32)>,
    pub boot_mode: BootMode,
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            ram_size_bytes: 256 * 1024 * 1024,
            bootargs: "console=ttyS0 earlycon=sbi panic=-1 rw".to_owned(),
            framebuffer: Some((1024, 600)),
            boot_mode: BootMode::Firmware,
        }
    }
}

/// Lifecycle state. Legal transitions are enforced by [`Machine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineState {
    Created,
    Running,
    Paused,
    Halted,
}

impl MachineState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Halted => "halted",
        }
    }
}

/// Why a `run_slice` call returned to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunReason {
    SliceExhausted,
    WaitingForInterrupt,
    Shutdown,
    Reboot,
}

/// Outcome of one host-driven execution slice.
#[derive(Debug, Clone, Copy)]
pub struct RunReport {
    pub reason: RunReason,
    pub instructions: u64,
}

/// The virtual machine.
#[derive(Debug)]
pub struct Machine {
    config: MachineConfig,
    cpu: Cpu,
    bus: SystemBus,
    state: MachineState,
    kernel_image: Option<Vec<u8>>,
    firmware_elf: Option<Vec<u8>>,
    initrd: Option<Vec<u8>>,
    framebuffer: Option<FramebufferInfo>,
    dtb_address: u64,
    total_instructions: u64,
}

impl Machine {
    /// Creates a machine in the `Created` state (no images loaded).
    ///
    /// # Errors
    /// Invalid configuration (RAM too small for the boot protocol regions).
    pub fn new(config: MachineConfig) -> Result<Self> {
        if config.ram_size_bytes < 16 * 1024 * 1024 {
            return Err(VmError::InvalidConfig(
                "RAM must be at least 16 MiB for kernel + DTB + framebuffer layout".into(),
            ));
        }
        let bus = SystemBus::new(config.ram_size_bytes)?;
        let framebuffer = config.framebuffer.map(|(width, height)| {
            let stride = width * 4;
            let size = u64::from(stride) * u64::from(height);
            let address =
                align_down(RAM_BASE + config.ram_size_bytes as u64 - size, PAGE);
            FramebufferInfo {
                address,
                width,
                height,
                stride,
            }
        });
        Ok(Self {
            config,
            cpu: Cpu::new(RAM_BASE),
            bus,
            state: MachineState::Created,
            kernel_image: None,
            firmware_elf: None,
            initrd: None,
            framebuffer,
            dtb_address: 0,
            total_instructions: 0,
        })
    }

    pub fn state(&self) -> MachineState {
        self.state
    }

    pub fn config(&self) -> &MachineConfig {
        &self.config
    }

    pub fn cpu(&self) -> &Cpu {
        &self.cpu
    }

    pub fn total_instructions(&self) -> u64 {
        self.total_instructions
    }

    fn require_created(&self, action: &'static str) -> Result<()> {
        if self.state != MachineState::Created {
            return Err(VmError::IllegalTransition {
                from: self.state.as_str(),
                action,
            });
        }
        Ok(())
    }

    /// Stores a raw kernel `Image` (written to RAM at boot).
    ///
    /// # Errors
    /// Illegal state, empty image, or an image too large for RAM.
    pub fn load_kernel_image(&mut self, image: Vec<u8>) -> Result<()> {
        self.require_created("load kernel")?;
        let usable = self.usable_top() - (RAM_BASE + KERNEL_OFFSET);
        if image.is_empty() || image.len() as u64 > usable {
            return Err(VmError::InvalidImage(format!(
                "kernel image of {} bytes does not fit the {} usable bytes",
                image.len(),
                usable
            )));
        }
        self.kernel_image = Some(image);
        Ok(())
    }

    /// Stores an OpenSBI-style firmware ELF (validated now, loaded at boot).
    ///
    /// # Errors
    /// Illegal state or a malformed ELF.
    pub fn load_firmware_elf(&mut self, elf: Vec<u8>) -> Result<()> {
        self.require_created("load firmware")?;
        ElfImage::parse(&elf)?;
        self.firmware_elf = Some(elf);
        Ok(())
    }

    /// Stores an initramfs (placed below the DTB at boot).
    ///
    /// # Errors
    /// Illegal state or an oversized initrd.
    pub fn load_initrd(&mut self, initrd: Vec<u8>) -> Result<()> {
        self.require_created("load initrd")?;
        if initrd.len() as u64 > self.usable_top() - (RAM_BASE + KERNEL_OFFSET) {
            return Err(VmError::InvalidImage("initrd does not fit in RAM".into()));
        }
        self.initrd = Some(initrd);
        Ok(())
    }

    /// Attaches a disk image to the VirtIO block device.
    ///
    /// # Errors
    /// Illegal state or a non-sector-multiple image.
    pub fn attach_disk_image(&mut self, image: Vec<u8>) -> Result<()> {
        self.require_created("attach disk")?;
        let backend = VecBlockBackend::from_bytes(image)?;
        self.bus.attach_disk(backend);
        Ok(())
    }

    /// Creates and attaches a fresh zeroed disk of `size` bytes.
    ///
    /// # Errors
    /// Illegal state or an invalid size.
    pub fn create_disk(&mut self, size: usize) -> Result<()> {
        self.require_created("create disk")?;
        let backend = VecBlockBackend::new(size)?;
        self.bus.attach_disk(backend);
        Ok(())
    }

    /// The first address not usable by kernel/initrd (bottom of the
    /// DTB/framebuffer reservation).
    fn usable_top(&self) -> u64 {
        let ram_end = RAM_BASE + self.config.ram_size_bytes as u64;
        let top = self
            .framebuffer
            .map_or(ram_end, |fb| fb.address);
        top - DTB_RESERVE
    }

    /// Boots (or cold-reboots) the machine: RAM is zeroed, firmware/kernel,
    /// initrd and the generated DTB are (re)loaded, and the hart starts at
    /// the boot-protocol entry with `a0 = 0` (hartid), `a1 = DTB address`.
    ///
    /// # Errors
    /// Missing kernel/firmware for the configured boot mode, or image
    /// placement failures.
    pub fn boot(&mut self) -> Result<()> {
        let kernel = self
            .kernel_image
            .as_ref()
            .ok_or_else(|| VmError::InvalidImage("no kernel image loaded".into()))?;
        let firmware_entry = match self.config.boot_mode {
            BootMode::Firmware => {
                let elf_bytes = self.firmware_elf.as_ref().ok_or_else(|| {
                    VmError::InvalidImage(
                        "BootMode::Firmware requires an OpenSBI firmware ELF; none loaded"
                            .into(),
                    )
                })?;
                Some(ElfImage::parse(elf_bytes)?.entry())
            }
            BootMode::DirectSbi => None,
        };

        self.bus.ram_mut().zero_fill();
        if let Some(elf_bytes) = &self.firmware_elf {
            let elf = ElfImage::parse(elf_bytes)?;
            elf.load_into(self.bus.ram_mut())?;
        }
        let kernel_entry = RAM_BASE + KERNEL_OFFSET;
        self.bus.ram_mut().write(kernel_entry, kernel)?;

        let dtb_address = self.usable_top();
        self.dtb_address = dtb_address;
        let initrd_range = match &self.initrd {
            Some(initrd) => {
                let start = align_down(dtb_address - initrd.len() as u64, PAGE);
                if start < kernel_entry + kernel.len() as u64 {
                    return Err(VmError::InvalidImage(
                        "initrd overlaps the kernel image".into(),
                    ));
                }
                self.bus.ram_mut().write(start, initrd)?;
                Some((start, start + initrd.len() as u64))
            }
            None => None,
        };

        let dtb = build_machine_dtb(&MachineDtbParams {
            ram_base: RAM_BASE,
            ram_size: self.config.ram_size_bytes as u64,
            bootargs: self.config.bootargs.clone(),
            initrd: initrd_range,
            framebuffer: self.framebuffer,
        });
        if dtb.len() as u64 > DTB_RESERVE {
            return Err(VmError::InvalidImage("generated DTB exceeds reservation".into()));
        }
        self.bus.ram_mut().write(dtb_address, &dtb)?;

        match firmware_entry {
            Some(entry) => {
                self.cpu.reset(entry);
                self.cpu.set_reg(10, 0);
                self.cpu.set_reg(11, dtb_address);
            }
            None => self.cpu.reset_supervisor(kernel_entry, 0, dtb_address),
        }
        self.state = MachineState::Running;
        Ok(())
    }

    /// Pauses a running machine.
    ///
    /// # Errors
    /// Illegal transition when not running.
    pub fn pause(&mut self) -> Result<()> {
        if self.state != MachineState::Running {
            return Err(VmError::IllegalTransition {
                from: self.state.as_str(),
                action: "pause",
            });
        }
        self.state = MachineState::Paused;
        Ok(())
    }

    /// Resumes a paused machine.
    ///
    /// # Errors
    /// Illegal transition when not paused.
    pub fn resume(&mut self) -> Result<()> {
        if self.state != MachineState::Paused {
            return Err(VmError::IllegalTransition {
                from: self.state.as_str(),
                action: "resume",
            });
        }
        self.state = MachineState::Running;
        Ok(())
    }

    /// Host-initiated power-off.
    ///
    /// # Errors
    /// Illegal transition from `Created`/`Halted`.
    pub fn power_off(&mut self) -> Result<()> {
        match self.state {
            MachineState::Running | MachineState::Paused => {
                self.state = MachineState::Halted;
                Ok(())
            }
            other => Err(VmError::IllegalTransition {
                from: other.as_str(),
                action: "power off",
            }),
        }
    }

    /// Executes up to `max_instructions` guest instructions.
    ///
    /// # Errors
    /// Illegal transition when the machine is not running. Guest faults are
    /// delivered to the guest as traps and are never host errors.
    pub fn run_slice(&mut self, max_instructions: u64) -> Result<RunReport> {
        if self.state != MachineState::Running {
            return Err(VmError::IllegalTransition {
                from: self.state.as_str(),
                action: "run",
            });
        }
        let direct = self.config.boot_mode == BootMode::DirectSbi;
        let mut executed = 0_u64;
        let report = loop {
            if executed >= max_instructions {
                break RunReport {
                    reason: RunReason::SliceExhausted,
                    instructions: executed,
                };
            }
            // Refresh device→CPU wiring before each instruction: cheap at
            // interpreter speeds and keeps level interrupts exact.
            self.bus.sync_interrupts();
            self.cpu.set_time_value(self.bus.clint().mtime());
            let timer = self.bus.clint().timer_pending();
            self.cpu.set_external_interrupts(
                self.bus.clint().software_pending(),
                timer && !direct,
                self.bus.plic().interrupt_pending(CONTEXT_M),
                self.bus.plic().interrupt_pending(CONTEXT_S),
            );
            if direct {
                self.cpu.set_supervisor_timer_pending(timer);
            }

            match self.cpu.step(&mut self.bus, direct) {
                StepOutcome::Executed => {
                    executed += 1;
                    self.bus.clint_mut().advance(1);
                }
                StepOutcome::WaitingForInterrupt => {
                    break RunReport {
                        reason: RunReason::WaitingForInterrupt,
                        instructions: executed,
                    };
                }
                StepOutcome::SbiCall => {
                    executed += 1;
                    let outcome = handle_sbi_call(&mut self.cpu, &mut self.bus);
                    self.cpu.set_pc(self.cpu.pc() + 4);
                    match outcome {
                        SbiOutcome::Continue => {}
                        SbiOutcome::Shutdown => {
                            self.state = MachineState::Halted;
                            break RunReport {
                                reason: RunReason::Shutdown,
                                instructions: executed,
                            };
                        }
                        SbiOutcome::Reboot => {
                            break RunReport {
                                reason: RunReason::Reboot,
                                instructions: executed,
                            };
                        }
                    }
                }
            }
        };
        self.total_instructions += report.instructions;
        if report.reason == RunReason::Reboot {
            // Cold-reboot semantics: full image reload. If the reload
            // itself fails the machine halts and surfaces the error.
            self.state = MachineState::Halted;
            self.boot()?;
        }
        Ok(report)
    }

    /// Queues terminal input for the guest. Returns `false` if the bounded
    /// UART input queue is full (host should retry after guest consumes).
    pub fn write_serial_input(&mut self, bytes: &[u8]) -> bool {
        self.bus.uart_mut().host_write(bytes)
    }

    /// Drains guest serial output for the host terminal.
    pub fn read_serial_output(&mut self) -> Vec<u8> {
        self.bus.uart_mut().take_output()
    }

    /// The framebuffer description, if one is configured.
    pub fn framebuffer(&self) -> Option<FramebufferInfo> {
        self.framebuffer
    }

    /// Current framebuffer contents (x8r8g8b8 as advertised in the DTB).
    pub fn framebuffer_bytes(&self) -> Option<&[u8]> {
        let fb = self.framebuffer?;
        let offset = (fb.address - RAM_BASE) as usize;
        let size = fb.size_bytes() as usize;
        self.bus.ram().as_slice().get(offset..offset + size)
    }

    /// The current disk image bytes (host persistence path).
    pub fn disk_image(&self) -> Option<&[u8]> {
        self.bus.disk().map(VecBlockBackend::image)
    }

    /// Whether the guest wrote to the disk since the host last persisted it.
    pub fn disk_dirty(&self) -> bool {
        self.bus.disk().map_or(false, VecBlockBackend::is_dirty)
    }

    /// Marks the disk clean after the host persisted `disk_image`.
    pub fn mark_disk_clean(&mut self) {
        if let Some(disk) = self.bus.disk_mut() {
            disk.mark_clean();
        }
    }

    /// Address the DTB was placed at by the most recent boot.
    pub fn dtb_address(&self) -> u64 {
        self.dtb_address
    }
}

const fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::{BootMode, Machine, MachineConfig, MachineState, RunReason};

    fn enc_i(imm: i32, rs1: u32, rd: u32) -> u32 {
        (((imm as u32) & 0xFFF) << 20) | (rs1 << 15) | (rd << 7) | 0x13
    }

    /// A microscopic "kernel": SBI putchar 'A', then legacy SBI shutdown.
    fn tiny_kernel() -> Vec<u8> {
        let words = [
            enc_i(1, 0, 17),        // addi a7, x0, 1 (legacy putchar)
            enc_i('A' as i32, 0, 10), // addi a0, x0, 'A'
            0x0000_0073,            // ecall
            enc_i(8, 0, 17),        // addi a7, x0, 8 (legacy shutdown)
            0x0000_0073,            // ecall
        ];
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    fn direct_machine() -> Machine {
        Machine::new(MachineConfig {
            ram_size_bytes: 32 * 1024 * 1024,
            bootargs: "console=ttyS0".to_owned(),
            framebuffer: None,
            boot_mode: BootMode::DirectSbi,
        })
        .expect("machine")
    }

    #[test]
    fn tiny_guest_prints_and_powers_off_through_the_real_boot_path() {
        let mut machine = direct_machine();
        machine.load_kernel_image(tiny_kernel()).expect("kernel");
        machine.boot().expect("boot");
        assert_eq!(machine.state(), MachineState::Running);
        let report = machine.run_slice(10_000).expect("run");
        assert_eq!(report.reason, RunReason::Shutdown);
        assert_eq!(machine.state(), MachineState::Halted);
        assert_eq!(machine.read_serial_output(), b"A".to_vec());
        assert!(machine.total_instructions() >= 5);
    }

    #[test]
    fn lifecycle_rejects_illegal_transitions() {
        let mut machine = direct_machine();
        assert!(machine.run_slice(1).is_err(), "run before boot");
        assert!(machine.boot().is_err(), "boot without kernel");
        assert!(machine.pause().is_err(), "pause while created");
        machine.load_kernel_image(tiny_kernel()).expect("kernel");
        machine.boot().expect("boot");
        assert!(machine.load_kernel_image(tiny_kernel()).is_err(), "load while running");
        machine.pause().expect("pause");
        assert!(machine.run_slice(1).is_err(), "run while paused");
        machine.resume().expect("resume");
        machine.power_off().expect("power off");
        assert_eq!(machine.state(), MachineState::Halted);
    }

    #[test]
    fn firmware_mode_without_firmware_fails_cleanly() {
        let mut machine = Machine::new(MachineConfig::default()).expect("machine");
        machine.load_kernel_image(tiny_kernel()).expect("kernel");
        let error = machine.boot().expect_err("must require firmware");
        assert!(error.to_string().contains("firmware ELF"), "actionable message: {error}");
        assert_eq!(machine.state(), MachineState::Created);
    }

    #[test]
    fn dtb_lands_where_a1_points() {
        let mut machine = direct_machine();
        machine.load_kernel_image(tiny_kernel()).expect("kernel");
        machine.boot().expect("boot");
        assert_eq!(machine.cpu().reg(11), machine.dtb_address());
        assert_eq!(machine.cpu().reg(10), 0, "a0 = hartid 0");
    }
}
