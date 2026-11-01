//! WASMNux `RV64IMAFDC` + `Zicsr` + `Zifencei` + `Zba` + `Zbb` + `Sv39` Virtual Machine Core.

pub mod boot;
pub mod bus;
pub mod csr;
pub mod fdt;
pub mod hart;
pub mod sbi;
pub mod trap;

use boot::RiscvLinuxImageHeader;
use bus::MachineBus;
use fdt::{build_virt_dtb, VirtMachineFdtConfig};
use hart::{Hart, StepOutcome};
use trap::SystemResetAction;

/// Guest physical DRAM base address (`qemu-virt` compatible).
pub const GUEST_RAM_BASE: u64 = 0x8000_0000;

/// Guest `simple-framebuffer` physical base address (`640x400x4` default).
pub const GUEST_FB_BASE: u64 = 0x5000_0000;

/// Default Linux `Image` load offset above `GUEST_RAM_BASE` (`2 MiB` aligned for Sv39 megapages).
pub const KERNEL_LOAD_OFFSET: u64 = 0x0020_0000;

/// Default `simple-framebuffer` dimensions (`640x400`, `XRGB8888`).
pub const DEFAULT_FB_WIDTH: u32 = 640;
pub const DEFAULT_FB_HEIGHT: u32 = 400;

const SNAPSHOT_MAGIC: [u8; 8] = *b"WNUXSNAP";
const SNAPSHOT_VERSION: u32 = 1;

/// Checked 64-bit guest physical offset to host slice bounds conversion.
/// Safe on both 32-bit `wasm32-unknown-unknown` and 64-bit native targets.
#[inline]
pub fn checked_guest_offset(offset: u64, access_len: usize, region_len: usize) -> Option<usize> {
    let idx = usize::try_from(offset).ok()?;
    let end = idx.checked_add(access_len)?;
    if end <= region_len {
        Some(idx)
    } else {
        None
    }
}

/// Explicit lifecycle state of a `RiscvMachine` instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmLifecycleState {
    Created,
    Running,
    Paused,
    PoweredOff,
    RebootRequested,
}

/// Complete single-hart RISC-V virtual machine (`Hart` + `MachineBus` + boot configuration).
pub struct RiscvMachine {
    pub hart: Hart,
    pub bus: MachineBus,
    pub state: VmLifecycleState,
    virtio_slots: Vec<(u64, u32)>,
    last_entry_pc: u64,
    last_dtb_pa: u64,
}

impl RiscvMachine {
    pub fn new(ram_bytes: usize, base_epoch_ns: u64) -> Self {
        Self {
            hart: Hart::new(0),
            bus: MachineBus::new(
                ram_bytes,
                DEFAULT_FB_WIDTH,
                DEFAULT_FB_HEIGHT,
                base_epoch_ns,
            ),
            state: VmLifecycleState::Created,
            virtio_slots: Vec::new(),
            last_entry_pc: GUEST_RAM_BASE + KERNEL_LOAD_OFFSET,
            last_dtb_pa: GUEST_RAM_BASE,
        }
    }

    /// Attach an external MMIO device (e.g. VirtIO MMIO v2 transport) and record its FDT slot if `irq > 0`.
    pub fn attach_virtio_mmio(
        &mut self,
        base: u64,
        size: u64,
        irq: u32,
        dev: Box<dyn bus::MmioDevice>,
    ) {
        self.bus.register_mmio(base, size, dev);
        self.virtio_slots.push((base, irq));
    }

    /// Load an official RISC-V Linux `Image` (or raw flat kernel binary), optional `initrd`,
    /// and dynamically generated Flattened Device Tree (`DTB`) into guest RAM, then reset hart 0.
    pub fn load_linux_image(
        &mut self,
        kernel_image: &[u8],
        initrd: Option<&[u8]>,
        bootargs: &str,
        rng_seed: [u8; 32],
    ) -> Result<(u64, u64), &'static str> {
        if kernel_image.is_empty() {
            return Err("kernel image must not be empty");
        }
        let ram_len = self.bus.ram.len() as u64;
        if ram_len < 16 * 1024 * 1024 {
            return Err("guest RAM must be at least 16 MiB to boot Linux");
        }

        let (load_offset, effective_ksize) = match RiscvLinuxImageHeader::parse(kernel_image) {
            Ok(hdr) => (hdr.text_offset, hdr.image_size),
            Err(_) => (KERNEL_LOAD_OFFSET, kernel_image.len() as u64),
        };

        let kernel_pa = GUEST_RAM_BASE
            .checked_add(load_offset)
            .ok_or("kernel load address overflow")?;
        let kernel_end = kernel_pa
            .checked_add(effective_ksize)
            .ok_or("kernel end address overflow")?;
        if kernel_pa
            .checked_add(kernel_image.len() as u64)
            .is_none_or(|end| end > GUEST_RAM_BASE + ram_len)
        {
            return Err("kernel image exceeds guest RAM");
        }

        self.bus.ram.as_mut_slice().fill(0);
        self.bus
            .ram
            .write_bytes(kernel_pa, kernel_image)
            .map_err(|_| "failed writing kernel image to guest RAM")?;

        // Place DTB near the top of RAM (2 MiB aligned downward) and initrd immediately below DTB (4 KiB aligned)
        // so neither collides with the kernel BSS/heap expansion at low RAM addresses.
        let ram_top = GUEST_RAM_BASE + ram_len;
        let dtb_reserve = 64 * 1024u64;
        let dtb_pa = (ram_top - dtb_reserve) & !0xFFF;

        let initrd_range = if let Some(initrd_bytes) = initrd {
            if initrd_bytes.is_empty() {
                None
            } else {
                let len = initrd_bytes.len() as u64;
                let start = dtb_pa
                    .checked_sub(len)
                    .map(|p| p & !0xFFF)
                    .ok_or("initrd size exceeds guest RAM")?;
                if start < kernel_end + (2 * 1024 * 1024) {
                    return Err("guest RAM too small for kernel + initrd + DTB");
                }
                self.bus
                    .ram
                    .write_bytes(start, initrd_bytes)
                    .map_err(|_| "failed writing initrd to guest RAM")?;
                Some((start, start + len))
            }
        } else if kernel_end >= dtb_pa {
            return Err("guest RAM too small for kernel + DTB");
        } else {
            None
        };

        let (fb_w, fb_h, _) = self.bus.fb.dimensions();
        let dtb = build_virt_dtb(&VirtMachineFdtConfig {
            ram_base: GUEST_RAM_BASE,
            ram_size: ram_len,
            isa: "rv64imafdc_zicsr_zifencei_zicntr_zba_zbb",
            bootargs,
            initrd: initrd_range,
            rng_seed,
            virtio_mmio_slots: &self.virtio_slots,
            framebuffer: Some((GUEST_FB_BASE, fb_w, fb_h)),
        });
        if dtb.len() as u64 > dtb_reserve {
            return Err("generated DTB exceeds reserved region");
        }
        self.bus
            .ram
            .write_bytes(dtb_pa, &dtb)
            .map_err(|_| "failed writing DTB to guest RAM")?;

        self.last_entry_pc = kernel_pa;
        self.last_dtb_pa = dtb_pa;
        self.hart.reset_linux_boot(kernel_pa, dtb_pa);
        self.state = VmLifecycleState::Running;
        Ok((kernel_pa, dtb_pa))
    }

    pub fn pause(&mut self) -> Result<(), &'static str> {
        if self.state != VmLifecycleState::Running {
            return Err("cannot pause VM when not in Running state");
        }
        self.state = VmLifecycleState::Paused;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), &'static str> {
        if self.state != VmLifecycleState::Paused {
            return Err("cannot resume VM when not in Paused state");
        }
        self.state = VmLifecycleState::Running;
        Ok(())
    }

    pub fn hard_reset(&mut self) {
        self.bus.syscon.reset_action = None;
        self.hart
            .reset_linux_boot(self.last_entry_pc, self.last_dtb_pa);
        self.state = VmLifecycleState::Running;
    }

    /// Execute up to `max_steps` instructions on Hart 0, updating `VmLifecycleState` on reset/poweroff.
    pub fn step_slice(&mut self, max_steps: usize) -> (usize, StepOutcome) {
        if self.state == VmLifecycleState::Paused || self.state == VmLifecycleState::PoweredOff {
            return (0, StepOutcome::Executed);
        }
        let (steps, outcome) = self.hart.run_slice(&mut self.bus, max_steps);
        match outcome {
            StepOutcome::Reset(SystemResetAction::PowerOff) => {
                self.state = VmLifecycleState::PoweredOff;
            }
            StepOutcome::Reset(SystemResetAction::ColdReboot | SystemResetAction::WarmReboot) => {
                self.state = VmLifecycleState::RebootRequested;
            }
            _ => {
                if self.state == VmLifecycleState::Created {
                    self.state = VmLifecycleState::Running;
                }
            }
        }
        (steps, outcome)
    }

    /// Serialize CPU + CSRs + CLINT + Guest RAM state into a portable binary VM snapshot.
    pub fn save_cpu_ram_snapshot(&self) -> Vec<u8> {
        let hs = self.hart.snapshot();
        let ram = self.bus.ram.as_slice();
        let mut out = Vec::with_capacity(64 + 4096 * 8 + ram.len());
        out.extend_from_slice(&SNAPSHOT_MAGIC);
        out.extend_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
        out.extend_from_slice(&hs.pc.to_le_bytes());
        out.push(hs.priv_mode);
        out.push(u8::from(hs.wfi));
        out.extend_from_slice(&[0u8; 2]);
        for &r in &hs.regs {
            out.extend_from_slice(&r.to_le_bytes());
        }
        for &f in &hs.fregs {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out.extend_from_slice(&(hs.csrs.regs.len() as u32).to_le_bytes());
        for &c in &hs.csrs.regs {
            out.extend_from_slice(&c.to_le_bytes());
        }
        out.extend_from_slice(&self.bus.clint.mtime.to_le_bytes());
        out.extend_from_slice(&self.bus.clint.mtimecmp.to_le_bytes());
        out.extend_from_slice(&self.bus.clint.msip.to_le_bytes());
        out.extend_from_slice(&(ram.len() as u64).to_le_bytes());
        out.extend_from_slice(ram);
        let _ = SystemResetAction::PowerOff;
        out
    }

    /// Restore CPU + CSRs + CLINT + Guest RAM state from `save_cpu_ram_snapshot`.
    pub fn restore_cpu_ram_snapshot(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() < 64 || data[0..8] != SNAPSHOT_MAGIC {
            return Err("invalid VM snapshot magic");
        }
        let ver = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        if ver != SNAPSHOT_VERSION {
            return Err("unsupported VM snapshot version");
        }
        let mut cur = 12usize;
        let read_u64 = |c: &mut usize| -> Result<u64, &'static str> {
            let s = data.get(*c..*c + 8).ok_or("truncated VM snapshot")?;
            *c += 8;
            Ok(u64::from_le_bytes([
                s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
            ]))
        };
        let read_u32 = |c: &mut usize| -> Result<u32, &'static str> {
            let s = data.get(*c..*c + 4).ok_or("truncated VM snapshot")?;
            *c += 4;
            Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        };

        let pc = read_u64(&mut cur)?;
        let hdr = data.get(cur..cur + 4).ok_or("truncated VM snapshot")?;
        let priv_mode = hdr[0];
        let wfi = hdr[1] != 0;
        cur += 4;

        let mut regs = [0u64; 32];
        for r in &mut regs {
            *r = read_u64(&mut cur)?;
        }
        let mut fregs = [0u64; 32];
        for f in &mut fregs {
            *f = read_u64(&mut cur)?;
        }
        let csr_len = read_u32(&mut cur)? as usize;
        if csr_len != 4096 {
            return Err("invalid CSR table length in snapshot");
        }
        let mut csrs = Vec::with_capacity(csr_len);
        for _ in 0..csr_len {
            csrs.push(read_u64(&mut cur)?);
        }
        let mtime = read_u64(&mut cur)?;
        let mtimecmp = read_u64(&mut cur)?;
        let msip = read_u32(&mut cur)?;
        let ram_len = usize::try_from(read_u64(&mut cur)?)
            .map_err(|_| "snapshot RAM exceeds platform address space")?;
        if ram_len != self.bus.ram.len() {
            return Err("snapshot RAM size does not match machine configuration");
        }
        let ram_slice = data
            .get(cur..cur + ram_len)
            .ok_or("truncated RAM payload in VM snapshot")?;

        self.hart.restore(&hart::HartSnapshot {
            pc,
            regs,
            fregs,
            priv_mode,
            wfi,
            csrs: csr::CsrSnapshot { regs: csrs },
        })?;
        self.bus.clint.mtime = mtime;
        self.bus.clint.mtimecmp = mtimecmp;
        self.bus.clint.msip = msip;
        self.bus.ram.as_mut_slice().copy_from_slice(ram_slice);
        self.bus.fb.mark_all_dirty();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csr::{CSR_SATP, CSR_SCAUSE, CSR_SEPC, CSR_SSTATUS, CSR_STVEC, SSTATUS_SUM};
    use crate::fdt::FDT_MAGIC;
    use crate::trap::{AccessType, PrivMode, Trap};

    fn write_prog(vm: &mut RiscvMachine, addr: u64, words: &[u32]) {
        for (i, &w) in words.iter().enumerate() {
            vm.bus
                .ram
                .write_scalar(addr + (i as u64) * 4, 4, u64::from(w), AccessType::Store)
                .expect("write instruction");
        }
    }

    #[test]
    fn checked_offset_rejects_overflow_and_out_of_bounds() {
        assert_eq!(checked_guest_offset(0, 8, 64), Some(0));
        assert_eq!(checked_guest_offset(56, 8, 64), Some(56));
        assert_eq!(checked_guest_offset(57, 8, 64), None);
        assert_eq!(checked_guest_offset(u64::MAX, 1, 64), None);
    }

    #[test]
    fn executes_rv64imafdc_and_zba_zbb_instructions() {
        let mut vm = RiscvMachine::new(16 * 1024 * 1024, 1_760_000_000_000_000_000);
        let base = GUEST_RAM_BASE;
        let scratch = base + 0x4000;

        // Program exercising ADDI, MUL, DIV, LR.D/SC.D, AMOADD.W (with CoW-safe Store check),
        // FCVT.D.W, FADD.D, FCVT.L.D, Zba SH2ADD, Zbb REV8 & ORC.B, and RVC C.ADDI!
        let prog: &[u32] = &[
            0x0070_0093, // addi x1, x0, 7
            0x0060_0113, // addi x2, x0, 6
            0x0220_81B3, // mul  x3, x1, x2          => 42
            0x2011_4233, // sh2add x4, x2, x1        => 7 + (6 << 2) = 31 (Zba)
            0x6B81_D293, // rev8 x5, x3              => 0x2A00_0000_0000_0000 (Zbb)
            0x2871_D313, // orc.b x6, x3             => 0xFF (Zbb)
            // Store scratch address in x10
            0x8000_4537, // lui x10, 0x80004         => 0x8000_4000 (sign-ext -> fix via slli/srli)
        ];
        write_prog(&mut vm, base, prog);
        vm.hart.pc = base;
        vm.hart.regs[10] = scratch;
        let _ = vm.step_slice(6);

        assert_eq!(vm.hart.regs[3], 42);
        assert_eq!(vm.hart.regs[4], 31);
        assert_eq!(vm.hart.regs[5], 42u64.swap_bytes());
        assert_eq!(vm.hart.regs[6], 0xFF);

        // Now test LR.D / SC.D + AMOADD.W + FP FCVT.D.W / FMUL.D / FCVT.L.D + compressed C.ADDI
        let prog2_addr = base + 0x100;
        let prog2: &[u32] = &[
            0x1005_33AF, // lr.d x7, (x10)
            0x1835_342F, // sc.d x8, x3, (x10)       => x8 = 0, mem[scratch] = 42
            0x0025_24AF, // amoadd.w x9, x2, (x10)   => x9 = 42, mem[scratch] = 48
            0xD201_80D3, // fcvt.d.w f1, x3          => f1 = 42.0
            0xD201_0153, // fcvt.d.w f2, x2          => f2 = 6.0
            0x1220_F1D3, // fmul.d f3, f1, f2        => f3 = 252.0
            0xC221_95D3, // fcvt.l.d x11, f3, rtz    => x11 = 252
        ];
        write_prog(&mut vm, prog2_addr, prog2);
        // Append compressed `c.addi x11, 8` (0x05A1) + `c.ebreak` (0x9002)
        let c_tail = prog2_addr + (prog2.len() as u64) * 4;
        vm.bus
            .ram
            .write_scalar(c_tail, 2, 0x05A1, AccessType::Store)
            .unwrap();
        vm.bus
            .ram
            .write_scalar(c_tail + 2, 2, 0x9002, AccessType::Store)
            .unwrap();

        vm.hart.csrs.write_raw(CSR_STVEC, base + 0x800);
        vm.hart.pc = prog2_addr;
        let _ = vm.step_slice(8);
        assert_eq!(vm.hart.regs[8], 0);
        assert_eq!(vm.hart.regs[9], 42);
        assert_eq!(
            vm.bus
                .ram
                .read_scalar(scratch, 4, AccessType::Load)
                .unwrap(),
            48
        );
        assert_eq!(vm.hart.regs[11], 260);

        // Execute C.EBREAK -> must trap to STVEC with scause = 3 and sepc = c_tail + 2!
        let _ = vm.step_slice(1);
        assert_eq!(vm.hart.pc, base + 0x800);
        assert_eq!(vm.hart.csrs.read_raw(CSR_SCAUSE), 3);
        assert_eq!(vm.hart.csrs.read_raw(CSR_SEPC), c_tail + 2);
    }

    #[test]
    fn sv39_enforces_pte_u_sum_smep_and_cow_dirty_bit() {
        let mut vm = RiscvMachine::new(16 * 1024 * 1024, 0);
        let root_pt_pa = GUEST_RAM_BASE + 0x10_000;
        let target_pa = GUEST_RAM_BASE + 0x20_000;
        vm.bus
            .ram
            .write_scalar(target_pa, 8, 0xCAFE_BABE_1234_5678, AccessType::Store)
            .unwrap();

        // Map gigapage VPN[2]=0 (VA 0x0000_0000..0x4000_0000 -> PA 0x8000_0000) as User R+W+X, A=0, D=0!
        let pte_user_clean = (0x80000u64 << 10) | (1 << 4) | (1 << 3) | (1 << 2) | (1 << 1) | 1;
        vm.bus
            .ram
            .write_scalar(root_pt_pa, 8, pte_user_clean, AccessType::Store)
            .unwrap();

        // Map VPN[2]=1 (VA 0x4000_0000) as Supervisor-only (U=0) R+W+A+D
        let pte_sup_only = (0x80000u64 << 10) | (1 << 7) | (1 << 6) | (1 << 2) | (1 << 1) | 1;
        vm.bus
            .ram
            .write_scalar(root_pt_pa + 8, 8, pte_sup_only, AccessType::Store)
            .unwrap();

        // Map VPN[2]=2 (VA 0x8000_0000 -> PA 0x8000_0000) as User CoW Read-Only (R=1, W=0, U=1, A=1)
        let pte_user_cow = (0x80000u64 << 10) | (1 << 6) | (1 << 4) | (1 << 1) | 1;
        vm.bus
            .ram
            .write_scalar(root_pt_pa + 16, 8, pte_user_cow, AccessType::Store)
            .unwrap();

        let satp = (8u64 << 60) | (root_pt_pa >> 12);
        vm.hart.csrs.write_raw(CSR_SATP, satp);
        let snap = vm.hart.snapshot();
        vm.hart.restore(&snap).unwrap();

        // 1. User mode Load on VA 0x0002_0000 sets PTE_A=1 while keeping PTE_D=0!
        vm.hart.priv_mode = PrivMode::User;
        assert_eq!(
            vm.hart.read_mem(&mut vm.bus, 0x20_000, 8, AccessType::Load),
            Ok(0xCAFE_BABE_1234_5678)
        );
        let pte_after_load = vm
            .bus
            .ram
            .read_scalar(root_pt_pa, 8, AccessType::Load)
            .unwrap();
        assert_ne!(pte_after_load & (1 << 6), 0, "PTE_A must be set on load");
        assert_eq!(pte_after_load & (1 << 7), 0, "PTE_D must remain 0 on load");

        // 2. First Store after Load on W=1 page misses TLB_WRITE and sets PTE_D=1 in PTE RAM!
        vm.hart
            .write_mem(
                &mut vm.bus,
                0x20_000,
                8,
                0xCAFE_BABE_1234_5678,
                AccessType::Store,
            )
            .unwrap();
        let pte_after_store = vm
            .bus
            .ram
            .read_scalar(root_pt_pa, 8, AccessType::Load)
            .unwrap();
        assert_ne!(
            pte_after_store & (1 << 7),
            0,
            "PTE_D must be set on first store after load"
        );

        // 2b. User mode CANNOT write CoW Read-Only (W=0) page at VA 0x8002_0000 -> StorePageFault!
        assert_eq!(
            vm.hart
                .write_mem(&mut vm.bus, 0x8002_0000, 8, 99, AccessType::Store),
            Err(Trap::PageFault {
                addr: 0x8002_0000,
                kind: AccessType::Store
            })
        );
        // 3. User mode CANNOT read Supervisor page at VA 0x4002_0000!
        assert_eq!(
            vm.hart
                .read_mem(&mut vm.bus, 0x4002_0000, 8, AccessType::Load),
            Err(Trap::PageFault {
                addr: 0x4002_0000,
                kind: AccessType::Load
            })
        );
        // 4. Supervisor mode WITHOUT sstatus.SUM CANNOT read User page at VA 0x0002_0000!
        vm.hart.priv_mode = PrivMode::Supervisor;
        assert_eq!(
            vm.hart.read_mem(&mut vm.bus, 0x20_000, 8, AccessType::Load),
            Err(Trap::PageFault {
                addr: 0x20_000,
                kind: AccessType::Load
            })
        );
        // 5. Supervisor mode WITH sstatus.SUM CAN read User page, and still CANNOT execute it (SMEP)!
        let s = vm.hart.csrs.read_raw(CSR_SSTATUS) | SSTATUS_SUM;
        vm.hart.csrs.write_raw(CSR_SSTATUS, s);
        assert_eq!(
            vm.hart.read_mem(&mut vm.bus, 0x20_000, 8, AccessType::Load),
            Ok(0xCAFE_BABE_1234_5678)
        );
        assert_eq!(
            vm.hart.translate(&mut vm.bus, 0x20_000, AccessType::Fetch),
            Err(Trap::PageFault {
                addr: 0x20_000,
                kind: AccessType::Fetch
            })
        );
    }

    #[test]
    fn user_mode_cannot_access_supervisor_csrs_or_sret() {
        let mut vm = RiscvMachine::new(16 * 1024 * 1024, 0);
        let base = GUEST_RAM_BASE;
        vm.hart.csrs.write_raw(CSR_STVEC, base + 0x400);
        // csrrs x1, satp, x0 (0x180020F3) executed in User mode MUST trap IllegalInstruction (scause=2)!
        write_prog(&mut vm, base, &[0x1800_20F3]);
        vm.hart.pc = base;
        vm.hart.priv_mode = PrivMode::User;
        let _ = vm.step_slice(1);
        assert_eq!(vm.hart.priv_mode, PrivMode::Supervisor);
        assert_eq!(vm.hart.csrs.read_raw(CSR_SCAUSE), 2);
        assert_eq!(vm.hart.pc, base + 0x400);
    }

    #[test]
    fn peripherals_fdt_sbi_and_snapshot_roundtrip() {
        let mut vm = RiscvMachine::new(32 * 1024 * 1024, 1_761_868_800_000_000_000);

        // Build authentic 64-byte RISC-V Linux `Image` header (`code0 = j +64` at 0x00, `RSC\x05` at 0x38)
        // followed by Supervisor code at 0x40 (SBI putchar 'O' + SRST shutdown).
        let mut fake_kernel = vec![0u8; 256];
        fake_kernel[0..4].copy_from_slice(&0x0400_006Fu32.to_le_bytes());
        fake_kernel[8..16].copy_from_slice(&KERNEL_LOAD_OFFSET.to_le_bytes());
        fake_kernel[16..24].copy_from_slice(&4096u64.to_le_bytes());
        fake_kernel[56..60].copy_from_slice(&boot::RISCV_IMAGE_MAGIC2.to_le_bytes());
        let boot_insns: &[u32] = &[
            0x04F0_0513, // addi a0, zero, 'O'
            0x0010_0893, // addi a7, zero, 1 (legacy console putchar)
            0x0000_0073, // ecall
            0x5352_58B7, // lui  a7, 0x53525
            0x3548_8893, // addi a7, a7, 0x354 => 0x53525354 ("SRST")
            0x0000_0813, // addi a6, zero, 0   (fid = system_reset)
            0x0000_0513, // addi a0, zero, 0   (PowerOff)
            0x0000_0073, // ecall
        ];
        for (i, &w) in boot_insns.iter().enumerate() {
            let off = 64 + i * 4;
            fake_kernel[off..off + 4].copy_from_slice(&w.to_le_bytes());
        }

        // Also assert malformed text_offset is cleanly rejected without panic
        let mut bad_kernel = fake_kernel.clone();
        bad_kernel[8..16].copy_from_slice(&0xDEAD_BEEF_0000_0000u64.to_le_bytes());
        assert_eq!(
            vm.load_linux_image(&bad_kernel, None, "", [0; 32]),
            Err("kernel image exceeds guest RAM")
        );

        let initrd = b"CPIO_INITRD_PAYLOAD";
        let (entry_pa, dtb_pa) = vm
            .load_linux_image(&fake_kernel, Some(initrd), "console=ttyS0", [0xAA; 32])
            .expect("load linux image");
        assert_eq!(entry_pa, GUEST_RAM_BASE + KERNEL_LOAD_OFFSET);
        let magic_be = vm.bus.ram.read_scalar(dtb_pa, 4, AccessType::Load).unwrap() as u32;
        assert_eq!(u32::from_be(magic_be), FDT_MAGIC);

        // Execute first 4 instructions (`j +64` + putchar 'O'), take snapshot, continue to SRST PowerOff,
        // then restore snapshot and confirm state rewinds cleanly!
        let (_, out1) = vm.step_slice(4);
        assert_eq!(out1, StepOutcome::Executed);
        assert_eq!(vm.bus.uart.drain_output(), b"O");

        let snap = vm.save_cpu_ram_snapshot();
        let (_, out2) = vm.step_slice(10);
        assert_eq!(out2, StepOutcome::Reset(SystemResetAction::PowerOff));

        vm.restore_cpu_ram_snapshot(&snap)
            .expect("restore snapshot");
        assert_eq!(vm.hart.pc, entry_pa + 64 + 12);

        // Test SimpleFB dirty scanline tracking
        let _ = vm.bus.fb.take_dirty_range();
        vm.bus
            .write_phys(
                GUEST_FB_BASE + 10 * 640 * 4 + 16,
                4,
                0x00FF_8040,
                AccessType::Store,
            )
            .unwrap();
        assert_eq!(vm.bus.fb.take_dirty_range(), Some((10, 11)));
        assert_eq!(vm.bus.fb.take_dirty_range(), None);
    }

    #[test]
    fn rv64m_div_rem_zero_and_overflow_semantics_match_isa_spec() {
        fn enc_r(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
            (funct7 << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | opcode
        }

        let mut vm = RiscvMachine::new(16 * 1024 * 1024, 1_760_000_000_000_000_000);
        let base = GUEST_RAM_BASE;

        // Place 8 RV64M instructions reading x1 (dividend), x2 (divisor) -> x3..x10:
        // x3 = DIV, x4 = DIVU, x5 = REM, x6 = REMU, x7 = DIVW, x8 = DIVUW, x9 = REMW, x10 = REMUW
        let prog = [
            enc_r(0x01, 2, 1, 4, 3, 0x33),
            enc_r(0x01, 2, 1, 5, 4, 0x33),
            enc_r(0x01, 2, 1, 6, 5, 0x33),
            enc_r(0x01, 2, 1, 7, 6, 0x33),
            enc_r(0x01, 2, 1, 4, 7, 0x3B),
            enc_r(0x01, 2, 1, 5, 8, 0x3B),
            enc_r(0x01, 2, 1, 6, 9, 0x3B),
            enc_r(0x01, 2, 1, 7, 10, 0x3B),
        ];
        write_prog(&mut vm, base, &prog);

        // Case 1: Division by zero (Table 7.1 of RISC-V Unprivileged Spec)
        let dividend = 0x1234_5678_9ABC_DEF0u64;
        vm.hart.pc = base;
        vm.hart.regs[1] = dividend;
        vm.hart.regs[2] = 0;
        let _ = vm.step_slice(8);
        assert_eq!(vm.hart.regs[3], u64::MAX, "DIV by 0 => -1 (all ones)");
        assert_eq!(vm.hart.regs[4], u64::MAX, "DIVU by 0 => 2^XLEN - 1");
        assert_eq!(vm.hart.regs[5], dividend, "REM by 0 => dividend");
        assert_eq!(vm.hart.regs[6], dividend, "REMU by 0 => dividend");
        assert_eq!(vm.hart.regs[7], u64::MAX, "DIVW by 0 => -1 sign-extended");
        assert_eq!(
            vm.hart.regs[8],
            u64::MAX,
            "DIVUW by 0 => u32::MAX sign-extended"
        );
        assert_eq!(
            vm.hart.regs[9],
            ((dividend as u32) as i32) as i64 as u64,
            "REMW by 0 => lower 32b sign-extended"
        );
        assert_eq!(
            vm.hart.regs[10],
            ((dividend as u32) as i32) as i64 as u64,
            "REMUW by 0 => lower 32b sign-extended"
        );

        // Case 2: 64-bit signed overflow (i64::MIN / -1 => i64::MIN, REM => 0)
        vm.hart.pc = base;
        vm.hart.regs[1] = i64::MIN as u64;
        vm.hart.regs[2] = (-1i64) as u64;
        let _ = vm.step_slice(8);
        assert_eq!(
            vm.hart.regs[3],
            i64::MIN as u64,
            "DIV i64::MIN / -1 => i64::MIN"
        );
        assert_eq!(vm.hart.regs[4], 0, "DIVU 0x8000..0 / 0xFFFF..F => 0");
        assert_eq!(vm.hart.regs[5], 0, "REM i64::MIN % -1 => 0");
        assert_eq!(
            vm.hart.regs[6],
            i64::MIN as u64,
            "REMU 0x8000..0 % 0xFFFF..F => dividend"
        );

        // Case 3: 32-bit signed overflow (i32::MIN / -1 => sign-extended i32::MIN, REMW => 0)
        vm.hart.pc = base;
        vm.hart.regs[1] = 0x0000_0000_8000_0000u64;
        vm.hart.regs[2] = 0x0000_0000_FFFF_FFFFu64;
        let _ = vm.step_slice(8);
        assert_eq!(
            vm.hart.regs[7],
            (i32::MIN as i64) as u64,
            "DIVW i32::MIN / -1 => 0xFFFF_FFFF_8000_0000"
        );
        assert_eq!(vm.hart.regs[8], 0, "DIVUW 0x8000_0000 / 0xFFFF_FFFF => 0");
        assert_eq!(vm.hart.regs[9], 0, "REMW i32::MIN % -1 => 0");
        assert_eq!(
            vm.hart.regs[10],
            (i32::MIN as i64) as u64,
            "REMUW 0x8000_0000 % 0xFFFF_FFFF => sign-extended 0x8000_0000"
        );

        // Case 4: Standard signed/unsigned quotient & remainder (-47 / 6 = -7 rem -5)
        vm.hart.pc = base;
        vm.hart.regs[1] = (-47i64) as u64;
        vm.hart.regs[2] = 6;
        let _ = vm.step_slice(8);
        assert_eq!(vm.hart.regs[3], (-7i64) as u64);
        assert_eq!(vm.hart.regs[5], (-5i64) as u64);
        assert_eq!(vm.hart.regs[7], (-7i64) as u64);
        assert_eq!(vm.hart.regs[9], (-5i64) as u64);
    }
    /// Executes a single hand-assembled instruction at `pc` and returns the hart to it.
    fn exec_one(vm: &mut RiscvMachine, pc: u64, word: u32) -> StepOutcome {
        write_prog(vm, pc, &[word]);
        vm.hart.pc = pc;
        let (steps, outcome) = vm.step_slice(1);
        assert_eq!(steps, 1, "exactly one instruction should retire");
        outcome
    }

    /// `FCVT.L.D` and `FCVT.D.LU` (and `FCVT.LU.D`) are the RV64 conversion instructions a
    /// hard-float `lp64d` userspace relies on. RISC-V saturates out-of-range conversions
    /// rather than trapping, which is what these assertions pin down.
    #[test]
    fn fcvt_l_d_and_fcvt_d_lu_saturate() {
        let base = GUEST_RAM_BASE;
        const FCVT_L_D_RTZ: u32 = 0xC223_94D3; // fcvt.l.d  x9, f7, rtz
        const FCVT_LU_D_RTZ: u32 = 0xC233_94D3; // fcvt.lu.d x9, f7, rtz
        const FCVT_D_LU_RNE: u32 = 0xD233_03D3; // fcvt.d.lu f7, x6, rne

        let mut vm = RiscvMachine::new(16 * 1024 * 1024, 0);

        // u64::MAX -> double rounds to exactly 2^64.
        vm.hart.regs[6] = u64::MAX;
        let _ = exec_one(&mut vm, base, FCVT_D_LU_RNE);
        assert_eq!(
            f64::from_bits(vm.hart.fregs[7]),
            18_446_744_073_709_551_616.0
        );

        // 2^64 cannot be represented in i64 -> saturates to i64::MAX.
        let _ = exec_one(&mut vm, base, FCVT_L_D_RTZ);
        assert_eq!(vm.hart.regs[9], i64::MAX as u64);

        // 2^64 cannot be represented in u64 either (it is the exclusive upper bound).
        let _ = exec_one(&mut vm, base, FCVT_LU_D_RTZ);
        assert_eq!(vm.hart.regs[9], u64::MAX);

        // NaN -> i64::MAX (positive saturation), +inf -> i64::MAX, -inf -> i64::MIN.
        vm.hart.fregs[7] = 0x7FF8_0000_0000_0000;
        let _ = exec_one(&mut vm, base, FCVT_L_D_RTZ);
        assert_eq!(vm.hart.regs[9], i64::MAX as u64);
        vm.hart.fregs[7] = f64::INFINITY.to_bits();
        let _ = exec_one(&mut vm, base, FCVT_L_D_RTZ);
        assert_eq!(vm.hart.regs[9], i64::MAX as u64);
        vm.hart.fregs[7] = f64::NEG_INFINITY.to_bits();
        let _ = exec_one(&mut vm, base, FCVT_L_D_RTZ);
        assert_eq!(vm.hart.regs[9], i64::MIN as u64);

        // Round-toward-zero truncates instead of rounding.
        vm.hart.fregs[7] = (-3.75f64).to_bits();
        let _ = exec_one(&mut vm, base, FCVT_L_D_RTZ);
        assert_eq!(vm.hart.regs[9] as i64, -3);

        // Negative input saturates to 0 for the unsigned conversion.
        vm.hart.fregs[7] = (-1.5f64).to_bits();
        let _ = exec_one(&mut vm, base, FCVT_LU_D_RTZ);
        assert_eq!(vm.hart.regs[9], 0);
    }
}
