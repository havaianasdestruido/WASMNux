//! WASMNux `RV64IMAFDC` + `Zicsr` + `Zifencei` + `Zba` + `Zbb` + `Sv39` Virtual Machine Core.

pub mod boot;
pub mod bus;
pub mod csr;
pub mod fdt;
pub mod hart;
pub mod sbi;
pub mod trap;

use boot::RiscvLinuxImageHeader;
use bus::{MachineBus, MAX_HARTS};
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
/// `WNUXSNAP` payload version. v2 adds per-hart records, per-hart CLINT registers and HSM state.
const SNAPSHOT_VERSION: u32 = 2;
/// Legacy single-hart snapshot version, still accepted by `restore_cpu_ram_snapshot`.
const SNAPSHOT_VERSION_V1: u32 = 1;

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

/// Complete RISC-V virtual machine: one [`Hart`] per modelled CPU sharing a [`MachineBus`].
///
/// Hart 0 is the boot hart. Secondary harts start halted (HSM `STOPPED`) and are brought up by
/// the guest through SBI `hart_start`, exactly like `qemu-virt` with `-smp N`.
pub struct RiscvMachine {
    /// Hart 0 (boot hart). Kept as a direct field so uniprocessor code and tests stay simple.
    pub hart: Hart,
    /// All harts including hart 0 (`harts[0]` is conceptually the same CPU as `hart`).
    pub harts: Vec<Hart>,
    pub bus: MachineBus,
    pub state: VmLifecycleState,
    hart_count: usize,
    virtio_slots: Vec<(u64, u32)>,
    last_entry_pc: u64,
    last_dtb_pa: u64,
}

impl RiscvMachine {
    pub fn new(ram_bytes: usize, base_epoch_ns: u64) -> Self {
        Self::new_with_harts(ram_bytes, base_epoch_ns, 1)
    }

    /// Create a machine with `hart_count` RV64GC harts (clamped to `1..=MAX_HARTS`).
    pub fn new_with_harts(ram_bytes: usize, base_epoch_ns: u64, hart_count: usize) -> Self {
        let harts_n = hart_count.clamp(1, MAX_HARTS);
        let mut harts: Vec<Hart> = (0..harts_n).map(Hart::new).collect();
        // Secondary harts are parked until the guest issues SBI hart_start.
        for h in harts.iter_mut().skip(1) {
            h.wfi = true;
        }
        let mut bus = MachineBus::new(
            ram_bytes,
            DEFAULT_FB_WIDTH,
            DEFAULT_FB_HEIGHT,
            base_epoch_ns,
        );
        bus.set_hart_count(harts_n);
        for h in harts.iter_mut().skip(1) {
            bus.hart_halt[h.hart_id] = true;
        }
        let primary = Hart::new(0);
        Self {
            hart: primary,
            harts,
            bus,
            state: VmLifecycleState::Created,
            hart_count: harts_n,
            virtio_slots: Vec::new(),
            last_entry_pc: GUEST_RAM_BASE + KERNEL_LOAD_OFFSET,
            last_dtb_pa: GUEST_RAM_BASE,
        }
    }

    /// Number of modelled harts (`1` = uniprocessor).
    #[inline]
    pub fn hart_count(&self) -> usize {
        self.hart_count
    }

    /// Mirror the `hart` field (hart 0 accessor) into `self.harts[0]`, the authoritative vector.
    #[inline]
    fn sync_primary_hart_in(&mut self) {
        let snapshot = self.hart.csrs.snapshot();
        self.harts[0].pc = self.hart.pc;
        self.harts[0].regs = self.hart.regs;
        self.harts[0].fregs = self.hart.fregs;
        self.harts[0].priv_mode = self.hart.priv_mode;
        self.harts[0].wfi = self.hart.wfi;
        self.harts[0].reservation = self.hart.reservation;
        let _ = self.harts[0].csrs.restore(&snapshot);
        self.harts[0].sbi.reset_action = self.hart.sbi.reset_action;
    }

    /// Mirror `self.harts[0]` back into the `hart` field after a multi-hart slice.
    #[inline]
    fn sync_primary_hart_out(&mut self) {
        let snapshot = self.harts[0].csrs.snapshot();
        self.hart.pc = self.harts[0].pc;
        self.hart.regs = self.harts[0].regs;
        self.hart.fregs = self.harts[0].fregs;
        self.hart.priv_mode = self.harts[0].priv_mode;
        self.hart.wfi = self.harts[0].wfi;
        self.hart.reservation = self.harts[0].reservation;
        let _ = self.hart.csrs.restore(&snapshot);
        self.hart.sbi.reset_action = self.harts[0].sbi.reset_action;
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
            hart_count: self.hart_count,
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
        self.reset_all_harts_for_boot(kernel_pa, dtb_pa);
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
        let (entry, dtb) = (self.last_entry_pc, self.last_dtb_pa);
        self.reset_all_harts_for_boot(entry, dtb);
        self.state = VmLifecycleState::Running;
    }

    /// Reset every hart to the Linux boot protocol entry state and re-park secondaries.
    fn reset_all_harts_for_boot(&mut self, entry_pc: u64, dtb_pa: u64) {
        self.bus.clint.reset_all_harts();
        self.bus.ipi_pending = None;
        self.bus.slice_remainder_hart = 0;
        for h in self.harts.iter_mut() {
            h.reset_linux_boot(entry_pc, dtb_pa);
            h.wfi = false;
        }
        for h in self.harts.iter_mut().skip(1) {
            // Park secondaries in WFI exactly like a real firmware would: they spin on
            // SBI hart_start, which is modelled by leaving them halted until the request.
            h.wfi = true;
        }
        self.bus.hart_halt = [false; MAX_HARTS];
        for hart in 1..self.hart_count {
            self.bus.hart_halt[hart] = true;
        }
        self.bus.hart_scratch_targets = [None; MAX_HARTS];
        self.bus.hsm_requests = (0..self.hart_count)
            .map(|hart_id| crate::sbi::HartHsmState {
                hart_id,
                started: hart_id == 0,
                start_request: None,
            })
            .collect();
        self.sync_primary_hart_out();
    }

    /// Consume pending SBI `hart_start` requests: reset the target hart to the requested entry
    /// address with `a0 = hartid`, `a1 = opaque`, and unpark it.
    fn service_hsm_start_requests(&mut self) {
        for idx in 0..self.harts.len() {
            let Some(req) = self.bus.hsm_requests.get(idx).cloned() else {
                continue;
            };
            let Some((start_addr, opaque)) = req.start_request else {
                continue;
            };
            if let Some(slot) = self.bus.hsm_requests.get_mut(idx) {
                slot.start_request = None;
                slot.started = true;
            }
            let hart = &mut self.harts[idx];
            hart.reset_linux_boot(start_addr, 0);
            hart.regs[10] = idx as u64;
            hart.regs[11] = opaque;
            hart.wfi = false;
            self.bus.hart_halt[idx] = false;
            self.bus.hart_scratch_targets[idx] = None;
        }
    }

    /// Execute up to `max_steps` instructions across all runnable harts (round-robin, equal
    /// quanta), updating `VmLifecycleState` on reset/poweroff.
    ///
    /// With `hart_count == 1` this is exactly a single-hart slice. With N harts the step budget is
    /// split evenly; a remainder is handed to a rotating hart so the executed total stays exact.
    pub fn step_slice(&mut self, max_steps: usize) -> (usize, StepOutcome) {
        if self.state == VmLifecycleState::Paused || self.state == VmLifecycleState::PoweredOff {
            return (0, StepOutcome::Executed);
        }
        self.sync_primary_hart_in();
        self.service_hsm_start_requests();

        let runnable = self.runnable_harts();
        if runnable.is_empty() {
            return (0, StepOutcome::WfiIdle);
        }

        let n = runnable.len();
        let base = max_steps / n;
        let remainder = max_steps % n;
        let mut total_steps = 0usize;
        let mut any_reset = None;
        let mut any_active = false;

        // Rotate the remainder so no hart is systematically starved.
        let rotation = self.bus.slice_remainder_hart % n;
        self.bus.slice_remainder_hart = self.bus.slice_remainder_hart.wrapping_add(1);

        let ipi = self.bus.ipi_pending.take();

        // Distribute the remainder one step at a time, starting at the rotating hart, so short
        // slices never starve low-numbered harts.
        let extra_rotation = rotation;
        for (order, hart_idx) in runnable.iter().copied().enumerate() {
            let mut budget = base;
            if remainder > 0 {
                let pick = (extra_rotation + order) % n;
                if pick < remainder {
                    budget += 1;
                }
            }
            let _ = order;
            if budget == 0 {
                continue;
            }
            // Apply a pending guest IPI to this hart (mask-selected), then clear for others.
            let ssip = ipi.and_then(|(mask, level)| {
                if mask == u64::MAX || (mask & (1u64 << (hart_idx as u32))) != 0 {
                    Some(level)
                } else {
                    None
                }
            });
            self.bus.current_hart = hart_idx;
            let (steps, outcome) =
                self.harts[hart_idx].run_slice_with_ipi(&mut self.bus, budget, ssip);
            total_steps += steps;
            match outcome {
                StepOutcome::Reset(action) => {
                    any_reset = Some(action);
                    break;
                }
                StepOutcome::WfiIdle => {}
                StepOutcome::Executed => any_active = true,
            }
            if steps > 0 {
                any_active = true;
            }
        }
        self.sync_primary_hart_out();

        let outcome = match any_reset {
            Some(SystemResetAction::PowerOff) => {
                self.state = VmLifecycleState::PoweredOff;
                StepOutcome::Reset(SystemResetAction::PowerOff)
            }
            Some(action) => {
                self.state = VmLifecycleState::RebootRequested;
                StepOutcome::Reset(action)
            }
            None => {
                if self.state == VmLifecycleState::Created {
                    self.state = VmLifecycleState::Running;
                }
                if any_active {
                    StepOutcome::Executed
                } else {
                    StepOutcome::WfiIdle
                }
            }
        };
        (total_steps, outcome)
    }

    /// Indices of harts that should receive cycles this slice (not HSM-halted, not parked).
    fn runnable_harts(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.hart_count);
        for idx in 0..self.harts.len() {
            if idx > 0 && self.bus.hart_halt.get(idx).copied().unwrap_or(false) {
                continue;
            }
            out.push(idx);
        }
        if out.is_empty() {
            out.push(0);
        }
        out
    }

    /// Serialize every hart (registers + CSRs), the CLINT, HSM state and Guest RAM into a
    /// portable binary VM snapshot (`WNUXSNAP` v2, little-endian).
    ///
    /// Layout: magic, version, hart_count, per-hart records (pc, priv, wfi, 32 x regs,
    /// 32 x fregs, 4096 x CSRs), CLINT `mtime` + per-hart `mtimecmp`/`msip`, HSM started/halted
    /// flags, RAM length, RAM bytes.
    pub fn save_cpu_ram_snapshot(&self) -> Vec<u8> {
        let primary = self.hart.snapshot();
        let ram = self.bus.ram.as_slice();
        let mut out = Vec::with_capacity(128 + self.harts.len() * 34_000 + ram.len());
        out.extend_from_slice(&SNAPSHOT_MAGIC);
        out.extend_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.harts.len() as u32).to_le_bytes());
        for (idx, h) in self.harts.iter().enumerate() {
            // Hart 0\'s authoritative CPU state always comes from `self.hart`, which callers may
            // have mutated directly (tests, debugger-style inspection, the WASM ABI).
            let hs = if idx == 0 {
                primary.clone()
            } else {
                h.snapshot()
            };
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
            out.extend_from_slice(&4096u32.to_le_bytes());
            for &c in &hs.csrs.regs {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out.extend_from_slice(&self.bus.clint.mtime.to_le_bytes());
        for &cmp in &self.bus.clint.mtimecmp {
            out.extend_from_slice(&cmp.to_le_bytes());
        }
        for &sip in &self.bus.clint.msip {
            out.extend_from_slice(&sip.to_le_bytes());
        }
        let mut hsm_bits = 0u32;
        for (idx, req) in self.bus.hsm_requests.iter().enumerate() {
            if req.started {
                hsm_bits |= 1 << idx;
            }
            if self.bus.hart_halt.get(idx).copied().unwrap_or(false) {
                hsm_bits |= 1 << (idx + 16);
            }
        }
        out.extend_from_slice(&hsm_bits.to_le_bytes());
        out.extend_from_slice(&(ram.len() as u64).to_le_bytes());
        out.extend_from_slice(ram);
        out
    }

    /// Restore every hart + CLINT + HSM + Guest RAM state produced by `save_cpu_ram_snapshot`.
    ///
    /// Snapshot v1 (single hart, scalar CLINT `mtimecmp`/`msip`) payloads written before SMP was
    /// introduced are still accepted: they restore hart 0 and leave every secondary hart parked.
    pub fn restore_cpu_ram_snapshot(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() < 64 || data[0..8] != SNAPSHOT_MAGIC {
            return Err("invalid VM snapshot magic");
        }
        let ver = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        if ver == SNAPSHOT_VERSION_V1 {
            return self.restore_cpu_ram_snapshot_v1(data);
        }
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

        let hart_count = read_u32(&mut cur)? as usize;
        if hart_count != self.harts.len() {
            return Err("snapshot hart count does not match machine configuration");
        }
        let mut restored: Vec<hart::HartSnapshot> = Vec::with_capacity(hart_count);
        for _ in 0..hart_count {
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
            if read_u32(&mut cur)? as usize != 4096 {
                return Err("invalid CSR table length in snapshot");
            }
            let mut csrs = [0u64; 4096];
            for c in csrs.iter_mut() {
                *c = read_u64(&mut cur)?;
            }
            restored.push(hart::HartSnapshot {
                pc,
                regs,
                fregs,
                priv_mode,
                wfi,
                csrs: crate::csr::CsrSnapshot { regs: csrs },
            });
        }
        let mtime = read_u64(&mut cur)?;
        let mut mtimecmp = [u64::MAX; MAX_HARTS];
        for slot in mtimecmp.iter_mut() {
            *slot = read_u64(&mut cur)?;
        }
        let mut msip = [0u32; MAX_HARTS];
        for slot in msip.iter_mut() {
            *slot = read_u32(&mut cur)?;
        }
        let hsm_bits = read_u32(&mut cur)?;
        let ram_len = usize::try_from(read_u64(&mut cur)?)
            .map_err(|_| "snapshot RAM exceeds platform address space")?;
        if ram_len != self.bus.ram.len() {
            return Err("snapshot RAM size does not match machine configuration");
        }
        let ram_slice = data
            .get(cur..cur + ram_len)
            .ok_or("truncated RAM payload in VM snapshot")?;

        for (idx, snap) in restored.iter().enumerate() {
            self.harts[idx].restore(snap)?;
        }
        self.bus.clint.mtime = mtime;
        self.bus.clint.mtimecmp = mtimecmp;
        self.bus.clint.msip = msip;
        for req in self.bus.hsm_requests.iter_mut() {
            req.started = (hsm_bits & (1 << req.hart_id)) != 0;
            req.start_request = None;
        }
        for (idx, slot) in self.bus.hart_halt.iter_mut().enumerate() {
            *slot = idx < 16 && (hsm_bits & (1 << (idx + 16))) != 0;
        }
        self.bus.ipi_pending = None;
        self.bus.ram.as_mut_slice().copy_from_slice(ram_slice);
        self.bus.fb.mark_all_dirty();
        self.sync_primary_hart_out();
        Ok(())
    }

    /// Restore a legacy (`WNUXSNAP` v1) single-hart snapshot into hart 0.
    ///
    /// v1 layout: magic, version, pc, (priv, wfi, pad), 32 x regs, 32 x fregs, csr_len, csrs,
    /// `mtime`, `mtimecmp`, `msip`, ram_len, RAM. Secondary harts are re-parked so a v1 restore
    /// always leaves the machine in the same state the writer observed.
    fn restore_cpu_ram_snapshot_v1(&mut self, data: &[u8]) -> Result<(), &'static str> {
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
        if read_u32(&mut cur)? as usize != 4096 {
            return Err("invalid CSR table length in snapshot");
        }
        let mut csrs = [0u64; 4096];
        for c in csrs.iter_mut() {
            *c = read_u64(&mut cur)?;
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

        self.harts[0].restore(&hart::HartSnapshot {
            pc,
            regs,
            fregs,
            priv_mode,
            wfi,
            csrs: crate::csr::CsrSnapshot { regs: csrs },
        })?;
        // Secondary harts were never part of a v1 snapshot: park them at the primary entry.
        for idx in 1..self.harts.len() {
            self.bus.hart_halt[idx] = true;
            if let Some(req) = self.bus.hsm_requests.get_mut(idx) {
                req.started = false;
                req.start_request = None;
            }
        }
        self.bus.hsm_requests[0].started = true;
        self.bus.clint.mtime = mtime;
        self.bus.clint.mtimecmp = [u64::MAX; MAX_HARTS];
        self.bus.clint.msip = [0; MAX_HARTS];
        self.bus.clint.mtimecmp[0] = mtimecmp;
        self.bus.clint.msip[0] = msip & 1;
        self.bus.ipi_pending = None;
        self.bus.ram.as_mut_slice().copy_from_slice(ram_slice);
        self.bus.fb.mark_all_dirty();
        self.sync_primary_hart_out();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::RISCV_IMAGE_MAGIC2;
    use crate::csr::{
        CSR_SATP, CSR_SCAUSE, CSR_SEPC, CSR_SIP, CSR_SSTATUS, CSR_STVEC, SIE_SSIE, SSTATUS_SUM,
    };
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
    /// Multi-hart bring-up: the FDT advertises `N` CPUs, `hart_start` releases a parked hart into
    /// guest code, each hart keeps an independent timer/interrupt state, SBI `send_ipi` reaches a
    /// masked sibling, `hart_stop` parks it again, and snapshots cover every hart.
    #[test]
    fn smp_harts_boot_through_sbi_hsm_round_robin_and_snapshot_all_harts() {
        let mut vm = RiscvMachine::new_with_harts(32 * 1024 * 1024, 0, 2);
        assert_eq!(vm.hart_count(), 2);
        assert_eq!(vm.harts.len(), 2);
        // Secondary harts start parked (HSM STOPPED) and receive no cycles.
        assert!(vm.bus.hart_halt[1]);

        // FDT advertises both CPUs and per-hart interrupt maps.
        let kernel = {
            let mut k = vec![0u8; 128];
            k[0..4].copy_from_slice(&0x0400_006Fu32.to_le_bytes()); // j +64
            k[8..16].copy_from_slice(&KERNEL_LOAD_OFFSET.to_le_bytes());
            k[56..60].copy_from_slice(&RISCV_IMAGE_MAGIC2.to_le_bytes());
            k
        };
        let (_entry, dtb_pa) = vm
            .load_linux_image(&kernel, None, "console=ttyS0", [0u8; 32])
            .expect("load");
        let mut dtb = vec![0u8; 4096];
        vm.bus.ram.read_bytes(dtb_pa, &mut dtb).expect("read dtb");
        let dtb_text = String::from_utf8_lossy(&dtb);
        assert!(dtb_text.contains("cpu@0") && dtb_text.contains("cpu@1"));
        assert!(dtb_text.contains("rv64imafdc_zicsr_zifencei_zicntr_zba_zbb"));

        // Hart 1 stays parked: only hart 0 makes progress.
        vm.hart.pc = GUEST_RAM_BASE;
        write_prog(&mut vm, GUEST_RAM_BASE, &[0x0010_0093u32]); // addi x1, x0, 1
        let (steps, _) = vm.step_slice(8);
        assert!(steps > 0);
        assert_eq!(
            vm.harts[1].pc,
            entry_after_load(&vm),
            "parked hart must not run"
        );
        assert!(vm.bus.hart_halt[1]);

        // SBI hart_start(hartid = 1, start_addr = 0x8020_0100, opaque = 0x1234_5678) from hart 0.
        // Arguments are loaded from a small data area instead of being built in registers.
        // `load_linux_image` clears guest RAM, so the data area is written here (afterwards).
        let prog = vec![
            0x8000_02B7, // lui  x5, 0x80000          x5 = 0xFFFF_FFFF_8000_0000
            0x0202_9293, // slli x5, x5, 32           keep only the low 32 bits
            0x0202_D293, // srli x5, x5, 32           x5 = 0x0000_0000_8000_0000
            0x0C02_B583, // ld   a1, 0xC0(x5)         a1 = start address (data)
            0x0C82_B603, // ld   a2, 0xC8(x5)         a2 = opaque (data)
            0x0010_0513, // addi a0, x0, 1            hartid = 1
            0x0000_0813, // addi a6, x0, 0            fid = hart_start
            0x0048_58B7, // lui  a7, 0x485
            0x34D8_8893, // addi a7, a7, 0x34D        a7 = 0x0048534D ("HSM")
            0x0000_0073, // ecall
            0x0000_006F, // j .                       spin so a0 keeps the SBI return value
        ];
        write_prog(&mut vm, GUEST_RAM_BASE, &prog);
        // Release hart 0 from any wait state and run the ecall sequence. `reset_linux_boot` also
        // resets `hart.sbi`, so the reset action must be cleared *after* it.
        vm.hart.reset_linux_boot(GUEST_RAM_BASE, dtb_pa);
        vm.hart.hart_id = 0;
        vm.sync_primary_hart_in();
        vm.harts[0].sbi.reset_action = None;
        // Data area for the hart_start arguments: start_pc at +0xC0, opaque at +0xC8 (clear of
        // the 0x64-byte Linux Image header area used by the fake kernel).
        let start_pc = GUEST_RAM_BASE + 0x100;
        let opaque: u64 = 0x1234_5678;
        let data_pa = GUEST_RAM_BASE + 0xC0;
        vm.bus
            .ram
            .write_bytes(data_pa, &start_pc.to_le_bytes())
            .unwrap();
        vm.bus
            .ram
            .write_bytes(data_pa + 8, &opaque.to_le_bytes())
            .unwrap();
        let (_, _) = vm.step_slice(16);
        assert_eq!(vm.harts[0].regs[10], 0, "hart_start must succeed");
        assert_eq!(
            vm.bus.hsm_requests[1].start_request,
            Some((start_pc, opaque))
        );

        // The request is serviced at the start of the next slice: hart 1 resumes at `start_pc`
        // with a0 = hartid and a1 = opaque, and the request slot is drained.
        assert_eq!(
            vm.bus.hsm_requests[1].start_request,
            Some((start_pc, opaque))
        );
        assert!(!vm.bus.hsm_requests[1].started);
        // Give hart 1 a real instruction stream so it can retire something after the start.
        write_prog(&mut vm, start_pc, &[0x0060_0313u32, 0x0000_006Fu32]); // addi x6,x0,6 ; j .
        let (_, _) = vm.step_slice(4);
        assert!(
            vm.bus.hsm_requests[1].start_request.is_none(),
            "request drained"
        );
        assert!(!vm.bus.hart_halt[1]);
        assert!(vm.bus.hsm_requests[1].started);
        assert_eq!(vm.harts[1].regs[10], 1, "a0 = hartid");
        assert_eq!(vm.harts[1].regs[11], opaque, "a1 = opaque");
        assert_eq!(vm.harts[1].regs[6], 6, "hart 1 executed its entry code");
        assert_eq!(vm.harts[1].pc, start_pc + 4, "hart 1 spins at its own loop");

        // `hart_start` on an already-started hart reports SBI_ERR_ALREADY_STARTED (-6).
        vm.harts[0].pc = GUEST_RAM_BASE;
        vm.hart.pc = GUEST_RAM_BASE;
        let (_, _) = vm.step_slice(64);
        assert_eq!(vm.harts[0].regs[10] as i64, -6, "already started");

        // Hart 1 executes its own instructions in the same slice as hart 0.
        write_prog(&mut vm, start_pc, &[0x0070_0093u32]); // addi x1, x0, 7
        vm.hart.pc = GUEST_RAM_BASE + 0x800;
        vm.harts[0].pc = GUEST_RAM_BASE + 0x800;
        write_prog(&mut vm, GUEST_RAM_BASE + 0x800, &[0x0060_0113u32]); // addi x2, x0, 6
        vm.harts[1].pc = start_pc;
        let (steps, _) = vm.step_slice(64);
        assert!(steps > 0);
        assert_eq!(vm.harts[0].regs[2], 6, "hart 0 ran its own program");
        assert_eq!(vm.harts[1].regs[1], 7, "hart 1 ran its own program");

        // Timers are per hart: arming hart 1's CLINT comparator does not disturb hart 0.
        vm.bus.clint.mtimecmp[1] = 10;
        vm.bus.clint.mtime = 5;
        assert!(!vm.bus.clint.timer_due(1));
        assert!(vm.bus.clint.ticks_until_due(1) == Some(5));
        assert!(!vm.bus.clint.timer_due(0));
        vm.bus.clint.mtime = 11;
        assert!(vm.bus.clint.timer_due(1));
        assert!(!vm.bus.clint.timer_due(0));

        // Guest SBI send_ipi from hart 0 targets hart 1 only.
        vm.bus.ipi_pending = Some((0b10, true));
        vm.harts[0].pc = GUEST_RAM_BASE;
        vm.harts[1].pc = start_pc;
        vm.hart.pc = GUEST_RAM_BASE;
        vm.hart.wfi = false;
        let _ = vm.step_slice(8);
        assert_eq!(vm.harts[0].csrs.read_raw(CSR_SIP) & SIE_SSIE, 0);
        assert_ne!(vm.harts[1].csrs.read_raw(CSR_SIP) & SIE_SSIE, 0);

        // hart_stop parks hart 1 permanently until restarted.
        vm.bus.hart_halt[1] = true;
        assert_eq!(vm.runnable_harts(), vec![0]);

        // Snapshot v2 round-trips both harts + per-hart CLINT state.
        vm.harts[1].pc = start_pc;
        vm.harts[1].regs[3] = 0x1234_5678;
        vm.bus.clint.mtimecmp[1] = 4242;
        let snap = vm.save_cpu_ram_snapshot();
        vm.harts[1].regs[3] = 0;
        vm.bus.clint.mtimecmp[1] = 0;
        vm.harts[1].pc = 0;
        vm.restore_cpu_ram_snapshot(&snap)
            .expect("restore smp snapshot");
        assert_eq!(vm.harts[1].regs[3], 0x1234_5678);
        assert_eq!(vm.bus.clint.mtimecmp[1], 4242);
        assert_eq!(vm.harts[1].pc, start_pc);
        assert_eq!(vm.hart_count(), 2);
    }

    /// Snapshot v2 is the current format, but v1 payloads written by earlier releases are still
    /// accepted (single hart, scalar CLINT state). This test synthesizes a genuine v1 payload.
    #[test]
    fn legacy_snapshot_v1_restores_hart_zero_and_parks_secondaries() {
        let mut vm = RiscvMachine::new_with_harts(1024 * 1024, 0, 2);
        // `hart` is the authoritative view of hart 0 for snapshotting.
        vm.hart.pc = 0x8000_1234;
        vm.hart.regs[5] = 0xDEAD_BEEF;
        vm.bus.clint.mtime = 777;
        vm.bus.clint.mtimecmp[0] = 4242;
        vm.bus
            .ram
            .write_bytes(GUEST_RAM_BASE, &[0xAA, 0xBB, 0xCC, 0xDD])
            .unwrap();
        let snap_ram = vm.bus.ram.as_slice().to_vec();
        let primary = vm.hart.snapshot();

        // v1 writer, byte-for-byte as released before SMP.
        let mut v1: Vec<u8> = Vec::new();
        v1.extend_from_slice(&SNAPSHOT_MAGIC);
        v1.extend_from_slice(&1u32.to_le_bytes());
        v1.extend_from_slice(&primary.pc.to_le_bytes());
        v1.push(primary.priv_mode);
        v1.push(u8::from(primary.wfi));
        v1.extend_from_slice(&[0u8; 2]);
        for &r in &primary.regs {
            v1.extend_from_slice(&r.to_le_bytes());
        }
        for &f in &primary.fregs {
            v1.extend_from_slice(&f.to_le_bytes());
        }
        v1.extend_from_slice(&4096u32.to_le_bytes());
        for &c in &primary.csrs.regs {
            v1.extend_from_slice(&c.to_le_bytes());
        }
        v1.extend_from_slice(&777u64.to_le_bytes()); // mtime
        v1.extend_from_slice(&4242u64.to_le_bytes()); // mtimecmp (single hart)
        v1.extend_from_slice(&0u32.to_le_bytes()); // msip
        v1.extend_from_slice(&(snap_ram.len() as u64).to_le_bytes());
        v1.extend_from_slice(&snap_ram);

        // Mutate everything, then restore the legacy payload.
        vm.hart.pc = 0;
        vm.hart.regs[5] = 0;
        vm.bus.hart_halt[1] = false;
        vm.bus.clint.mtimecmp[0] = 0;
        vm.bus.clint.mtime = 0;
        vm.bus.ram.as_mut_slice().fill(0);
        vm.restore_cpu_ram_snapshot(&v1).expect("v1 restore");

        assert_eq!(vm.harts[0].pc, 0x8000_1234);
        assert_eq!(vm.harts[0].regs[5], 0xDEAD_BEEF);
        assert_eq!(vm.bus.clint.mtime, 777);
        assert_eq!(vm.bus.clint.mtimecmp[0], 4242);
        assert_eq!(
            vm.bus.clint.mtimecmp[1],
            u64::MAX,
            "secondary CLINT left disarmed"
        );
        assert_eq!(&vm.bus.ram.as_slice()[..4], &[0xAA, 0xBB, 0xCC, 0xDD]);
        // Secondary hart re-parked, primary marked started.
        assert!(vm.bus.hart_halt[1]);
        assert!(vm.bus.hsm_requests[0].started);
        assert!(!vm.bus.hsm_requests[1].started);
        // The restored machine mirrors hart 0 into the public `hart` field.
        assert_eq!(vm.hart.pc, 0x8000_1234);

        // A truncated or wrong-size v1 payload is still rejected.
        assert!(vm.restore_cpu_ram_snapshot(&v1[..32]).is_err());
        let mut bad = v1.clone();
        bad[..8].copy_from_slice(b"NOTSNAPV");
        assert!(vm.restore_cpu_ram_snapshot(&bad).is_err());
    }

    /// Helper: entry PC of the loaded image for the SMP test's parking assertions.
    fn entry_after_load(vm: &RiscvMachine) -> u64 {
        vm.harts[1].pc
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
    /// Guest RAM assembles values byte-by-byte, so unaligned accesses must be exact
    /// (no alignment fault, no host undefined behaviour) and must stay bounds-checked.
    /// Ported from the unaligned-access review in PR #2.
    #[test]
    fn unaligned_guest_access_is_byte_accurate_and_bounds_checked() {
        let mut vm = RiscvMachine::new(16 * 1024 * 1024, 0);
        let ram_end = GUEST_RAM_BASE + vm.bus.ram.len() as u64;

        // A u64 written at an odd address reads back identically...
        let odd = GUEST_RAM_BASE + 0x1001;
        vm.bus
            .write_phys(odd, 8, 0x1122_3344_5566_7788, AccessType::Store)
            .expect("unaligned 8-byte store");
        assert_eq!(
            vm.bus
                .read_phys(odd, 8, AccessType::Load)
                .expect("unaligned 8-byte load"),
            0x1122_3344_5566_7788
        );

        // ...and is stored little-endian at the expected byte offsets.
        for (offset, expected) in [0x88u8, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                vm.bus
                    .read_phys(odd + offset as u64, 1, AccessType::Load)
                    .expect("byte load"),
                u64::from(expected)
            );
        }

        // A 4-byte access straddling a 4-byte boundary is likewise exact.
        let straddle = GUEST_RAM_BASE + 0x2003;
        vm.bus
            .write_phys(straddle, 4, 0xDEAD_BEEF, AccessType::Store)
            .expect("straddling 4-byte store");
        assert_eq!(
            vm.bus
                .read_phys(straddle, 2, AccessType::Load)
                .expect("low half"),
            0xBEEF
        );
        assert_eq!(
            vm.bus
                .read_phys(straddle + 2, 2, AccessType::Load)
                .expect("high half"),
            0xDEAD
        );

        // Unaligned accesses that leave guest RAM must fault instead of wrapping.
        assert!(vm.bus.read_phys(ram_end - 3, 8, AccessType::Load).is_err());
        assert!(vm
            .bus
            .write_phys(ram_end - 1, 2, 0, AccessType::Store)
            .is_err());
        assert!(vm
            .bus
            .read_phys(GUEST_RAM_BASE - 4, 4, AccessType::Load)
            .is_err());
    }

    /// Malformed kernel payloads must be rejected loudly rather than booted as garbage.
    /// Ported from the malformed-executable review in PR #2.
    #[test]
    fn malformed_kernel_images_are_rejected() {
        // Header parsing: too short, then a wrong magic at the documented offset.
        assert!(RiscvLinuxImageHeader::parse(&[]).is_err());
        let mut short = [0u8; 32];
        short[0] = 1;
        assert!(RiscvLinuxImageHeader::parse(&short).is_err());

        let mut bad_magic = [0u8; 64];
        bad_magic[56..60].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        assert!(RiscvLinuxImageHeader::parse(&bad_magic).is_err());

        // A well-formed header reports the default 2 MiB load offset when text_offset is 0.
        let mut good = [0u8; 64];
        good[56..60].copy_from_slice(&RISCV_IMAGE_MAGIC2.to_le_bytes());
        good[16..24].copy_from_slice(&64u64.to_le_bytes());
        let header = RiscvLinuxImageHeader::parse(&good).expect("valid header");
        assert_eq!(header.text_offset, KERNEL_LOAD_OFFSET);
        assert_eq!(header.image_size, 64);

        // The loader refuses an empty payload...
        let mut vm = RiscvMachine::new(16 * 1024 * 1024, 0);
        assert_eq!(
            vm.load_linux_image(&[], None, "console=ttyS0", [0u8; 32]),
            Err("kernel image must not be empty")
        );

        // ...and refuses a payload that cannot fit in guest RAM.
        let oversized = vec![0u8; 16 * 1024 * 1024 + 4096];
        assert_eq!(
            vm.load_linux_image(&oversized, None, "console=ttyS0", [0u8; 32]),
            Err("kernel image exceeds guest RAM")
        );

        // A non-Image payload is still accepted as a raw kernel for bare-metal guests,
        // which is the documented behaviour for the mini-OS test kernels.
        let raw = vec![0x13u8; 4096];
        let (entry, dtb) = vm
            .load_linux_image(&raw, None, "console=ttyS0", [0u8; 32])
            .expect("raw kernel accepted");
        assert_eq!(entry, GUEST_RAM_BASE + KERNEL_LOAD_OFFSET);
        assert!(dtb > entry);
    }
}
