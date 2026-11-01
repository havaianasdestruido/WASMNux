//! System interconnect, fast DRAM, SimpleFB VRAM, CLINT, PLIC, UART 16550A, Goldfish RTC, and Syscon.

use std::collections::VecDeque;

use crate::checked_guest_offset;
use crate::trap::{AccessType, SystemResetAction, Trap};
use crate::{GUEST_FB_BASE, GUEST_RAM_BASE};

pub const CLINT_BASE: u64 = 0x0200_0000;
pub const CLINT_SIZE: u64 = 0x0001_0000;
pub const PLIC_BASE: u64 = 0x0C00_0000;
pub const PLIC_SIZE: u64 = 0x0400_0000;
pub const SYSCON_BASE: u64 = 0x0010_0000;
pub const SYSCON_SIZE: u64 = 0x0000_1000;
pub const RTC_BASE: u64 = 0x0010_1000;
pub const RTC_SIZE: u64 = 0x0000_1000;
pub const UART_BASE: u64 = 0x1000_0000;
pub const UART_SIZE: u64 = 0x0000_0100;
pub const UART_IRQ: u32 = 10;

/// Contiguous guest DRAM starting at `GUEST_RAM_BASE` (`0x8000_0000`).
#[derive(Clone, Debug)]
pub struct GuestRam {
    data: Vec<u8>,
}

impl GuestRam {
    pub fn new(size_bytes: usize) -> Self {
        Self {
            data: vec![0u8; size_bytes],
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data
    }

    #[inline]
    fn check(&self, paddr: u64, size: usize, kind: AccessType) -> Result<usize, Trap> {
        let offset = paddr
            .checked_sub(GUEST_RAM_BASE)
            .ok_or(Trap::MemoryOutOfBounds { addr: paddr, kind })?;
        checked_guest_offset(offset, size, self.data.len())
            .ok_or(Trap::MemoryOutOfBounds { addr: paddr, kind })
    }

    #[inline]
    pub fn read_scalar(&self, paddr: u64, size: usize, kind: AccessType) -> Result<u64, Trap> {
        let idx = self.check(paddr, size, kind)?;
        let s = &self.data[idx..idx + size];
        let v = match size {
            1 => u64::from(s[0]),
            2 => u64::from(u16::from_le_bytes([s[0], s[1]])),
            4 => u64::from(u32::from_le_bytes([s[0], s[1], s[2], s[3]])),
            8 => u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]),
            _ => return Err(Trap::MemoryOutOfBounds { addr: paddr, kind }),
        };
        Ok(v)
    }

    #[inline]
    pub fn write_scalar(
        &mut self,
        paddr: u64,
        size: usize,
        val: u64,
        kind: AccessType,
    ) -> Result<(), Trap> {
        let idx = self.check(paddr, size, kind)?;
        let d = &mut self.data[idx..idx + size];
        match size {
            1 => d[0] = val as u8,
            2 => d.copy_from_slice(&(val as u16).to_le_bytes()),
            4 => d.copy_from_slice(&(val as u32).to_le_bytes()),
            8 => d.copy_from_slice(&val.to_le_bytes()),
            _ => return Err(Trap::MemoryOutOfBounds { addr: paddr, kind }),
        }
        Ok(())
    }

    pub fn read_bytes(&self, paddr: u64, dst: &mut [u8]) -> Result<(), Trap> {
        let idx = self.check(paddr, dst.len(), AccessType::Load)?;
        dst.copy_from_slice(&self.data[idx..idx + dst.len()]);
        Ok(())
    }

    pub fn write_bytes(&mut self, paddr: u64, src: &[u8]) -> Result<(), Trap> {
        let idx = self.check(paddr, src.len(), AccessType::Store)?;
        self.data[idx..idx + src.len()].copy_from_slice(src);
        Ok(())
    }
}

/// Guest `simple-framebuffer` (`XRGB8888`) VRAM with dirty-scanline tracking for WebGPU / Canvas2D.
#[derive(Clone, Debug)]
pub struct SimpleFramebuffer {
    width: u32,
    height: u32,
    stride: u32,
    vram: Vec<u8>,
    dirty: bool,
    dirty_y_min: u32,
    dirty_y_max: u32,
    frame_seq: u64,
}

impl SimpleFramebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let stride = width * 4;
        let len = (stride as usize) * (height as usize);
        Self {
            width,
            height,
            stride,
            vram: vec![0u8; len],
            dirty: true,
            dirty_y_min: 0,
            dirty_y_max: height,
            frame_seq: 1,
        }
    }

    pub fn dimensions(&self) -> (u32, u32, u32) {
        (self.width, self.height, self.stride)
    }

    pub fn byte_len(&self) -> usize {
        self.vram.len()
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.vram
    }

    pub fn frame_seq(&self) -> u64 {
        self.frame_seq
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return;
        }
        self.width = width;
        self.height = height;
        self.stride = width * 4;
        self.vram = vec![0u8; (self.stride as usize) * (height as usize)];
        self.dirty = true;
        self.dirty_y_min = 0;
        self.dirty_y_max = height;
        self.frame_seq = self.frame_seq.wrapping_add(1);
    }

    /// Take the dirty scanline range `[y_min, y_max)` and mark the framebuffer clean.
    pub fn take_dirty_range(&mut self) -> Option<(u32, u32)> {
        if !self.dirty || self.dirty_y_min >= self.dirty_y_max {
            self.dirty = false;
            return None;
        }
        let range = (self.dirty_y_min, self.dirty_y_max.min(self.height));
        self.dirty = false;
        self.dirty_y_min = self.height;
        self.dirty_y_max = 0;
        Some(range)
    }

    pub fn mark_all_dirty(&mut self) {
        self.dirty = true;
        self.dirty_y_min = 0;
        self.dirty_y_max = self.height;
        self.frame_seq = self.frame_seq.wrapping_add(1);
    }

    fn read(&self, offset: u64, size: usize, paddr: u64, kind: AccessType) -> Result<u64, Trap> {
        let idx = checked_guest_offset(offset, size, self.vram.len())
            .ok_or(Trap::MemoryOutOfBounds { addr: paddr, kind })?;
        let s = &self.vram[idx..idx + size];
        let v = match size {
            1 => u64::from(s[0]),
            2 => u64::from(u16::from_le_bytes([s[0], s[1]])),
            4 => u64::from(u32::from_le_bytes([s[0], s[1], s[2], s[3]])),
            8 => u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]),
            _ => return Err(Trap::MemoryOutOfBounds { addr: paddr, kind }),
        };
        Ok(v)
    }

    fn write(&mut self, offset: u64, size: usize, val: u64, paddr: u64) -> Result<(), Trap> {
        let idx =
            checked_guest_offset(offset, size, self.vram.len()).ok_or(Trap::MemoryOutOfBounds {
                addr: paddr,
                kind: AccessType::Store,
            })?;
        let d = &mut self.vram[idx..idx + size];
        match size {
            1 => d[0] = val as u8,
            2 => d.copy_from_slice(&(val as u16).to_le_bytes()),
            4 => d.copy_from_slice(&(val as u32).to_le_bytes()),
            8 => d.copy_from_slice(&val.to_le_bytes()),
            _ => {
                return Err(Trap::MemoryOutOfBounds {
                    addr: paddr,
                    kind: AccessType::Store,
                })
            }
        }
        let stride = self.stride.max(1) as usize;
        let y0 = (idx / stride) as u32;
        let y1 = (((idx + size.saturating_sub(1)) / stride) as u32 + 1).min(self.height);
        if !self.dirty {
            self.dirty = true;
            self.dirty_y_min = y0;
            self.dirty_y_max = y1;
        } else {
            self.dirty_y_min = self.dirty_y_min.min(y0);
            self.dirty_y_max = self.dirty_y_max.max(y1);
        }
        self.frame_seq = self.frame_seq.wrapping_add(1);
        Ok(())
    }
}

/// Core-Local Interruptor (`msip`, `mtimecmp`, `mtime` @ 10 MHz).
#[derive(Clone, Debug)]
pub struct ClintState {
    pub msip: u32,
    pub mtimecmp: u64,
    pub mtime: u64,
}

impl Default for ClintState {
    fn default() -> Self {
        Self::new()
    }
}

impl ClintState {
    pub fn new() -> Self {
        Self {
            msip: 0,
            mtimecmp: u64::MAX,
            mtime: 0,
        }
    }

    #[inline]
    pub fn tick(&mut self, delta: u64) {
        self.mtime = self.mtime.wrapping_add(delta);
    }

    #[inline]
    pub fn timer_due(&self) -> bool {
        self.mtime >= self.mtimecmp
    }

    #[inline]
    pub fn ticks_until_due(&self) -> Option<u64> {
        if self.mtimecmp == u64::MAX {
            None
        } else if self.mtime >= self.mtimecmp {
            Some(0)
        } else {
            Some(self.mtimecmp - self.mtime)
        }
    }

    pub fn read(&self, offset: u64, size: usize) -> u64 {
        let reg64 = match offset & !7 {
            0x0000 => u64::from(self.msip),
            0x4000 => self.mtimecmp,
            0xBFF8 => self.mtime,
            _ => 0,
        };
        let shift = (offset & 7) * 8;
        let shifted = reg64 >> shift;
        match size {
            1 => shifted & 0xFF,
            2 => shifted & 0xFFFF,
            4 => shifted & 0xFFFF_FFFF,
            _ => shifted,
        }
    }

    pub fn write(&mut self, offset: u64, size: usize, val: u64) {
        match (offset, size) {
            (0x0000, 4 | 8) => self.msip = (val as u32) & 1,
            (0x4000, 8) => self.mtimecmp = val,
            (0x4000, 4) => self.mtimecmp = (self.mtimecmp & !0xFFFF_FFFF) | (val & 0xFFFF_FFFF),
            (0x4004, 4) => self.mtimecmp = (self.mtimecmp & 0xFFFF_FFFF) | (val << 32),
            (0xBFF8, 8) => self.mtime = val,
            (0xBFF8, 4) => self.mtime = (self.mtime & !0xFFFF_FFFF) | (val & 0xFFFF_FFFF),
            (0xBFFC, 4) => self.mtime = (self.mtime & 0xFFFF_FFFF) | (val << 32),
            _ => {}
        }
    }
}

/// SiFive-compatible Platform-Level Interrupt Controller (`PLIC`, up to 32 IRQ sources, 2 contexts).
#[derive(Clone, Debug)]
pub struct PlicState {
    priorities: [u32; 32],
    level: u32,
    pending: u32,
    claimed: u32,
    enable: [u32; 2],
    threshold: [u32; 2],
}

impl Default for PlicState {
    fn default() -> Self {
        Self::new()
    }
}

impl PlicState {
    pub fn new() -> Self {
        Self {
            priorities: [0; 32],
            level: 0,
            pending: 0,
            claimed: 0,
            enable: [0; 2],
            threshold: [0; 2],
        }
    }

    pub fn set_source_level(&mut self, irq: u32, high: bool) {
        if irq == 0 || irq >= 32 {
            return;
        }
        let bit = 1u32 << irq;
        if high {
            self.level |= bit;
            if (self.claimed & bit) == 0 {
                self.pending |= bit;
            }
        } else {
            self.level &= !bit;
            self.pending &= !bit;
        }
    }

    fn best_irq(&self, ctx: usize) -> Option<u32> {
        let candidates = self.pending & !self.claimed & self.enable[ctx];
        if candidates == 0 {
            return None;
        }
        let thresh = self.threshold[ctx];
        let mut best_id = 0u32;
        let mut best_prio = 0u32;
        for irq in 1..32u32 {
            if (candidates & (1 << irq)) != 0 {
                let p = self.priorities[irq as usize];
                if p > thresh && p > best_prio {
                    best_prio = p;
                    best_id = irq;
                }
            }
        }
        if best_id != 0 {
            Some(best_id)
        } else {
            None
        }
    }

    #[inline]
    pub fn supervisor_external_pending(&self) -> bool {
        self.best_irq(1).is_some() || self.best_irq(0).is_some()
    }

    fn claim(&mut self, ctx: usize) -> u32 {
        let Some(irq) = self.best_irq(ctx) else {
            return 0;
        };
        let bit = 1u32 << irq;
        self.pending &= !bit;
        self.claimed |= bit;
        irq
    }

    fn complete(&mut self, _ctx: usize, irq: u32) {
        if irq == 0 || irq >= 32 {
            return;
        }
        let bit = 1u32 << irq;
        self.claimed &= !bit;
        if (self.level & bit) != 0 {
            self.pending |= bit;
        }
    }

    pub fn read(&mut self, offset: u64, _size: usize) -> u64 {
        let val: u32 = match offset {
            0x0000_0000..=0x0000_007C => {
                let idx = (offset / 4) as usize;
                self.priorities.get(idx).copied().unwrap_or(0)
            }
            0x0000_1000 => self.pending,
            0x0000_2000 => self.enable[0],
            0x0000_2080 => self.enable[1],
            0x0020_0000 => self.threshold[0],
            0x0020_0004 => self.claim(0),
            0x0020_1000 => self.threshold[1],
            0x0020_1004 => self.claim(1),
            _ => 0,
        };
        u64::from(val)
    }

    pub fn write(&mut self, offset: u64, _size: usize, value: u64) {
        let v = value as u32;
        match offset {
            0x0000_0004..=0x0000_007C => {
                let idx = (offset / 4) as usize;
                if idx < self.priorities.len() {
                    self.priorities[idx] = v & 0x7;
                }
            }
            0x0000_2000 => self.enable[0] = v & !1,
            0x0000_2080 => self.enable[1] = v & !1,
            0x0020_0000 => self.threshold[0] = v & 0x7,
            0x0020_0004 => self.complete(0, v),
            0x0020_1000 => self.threshold[1] = v & 0x7,
            0x0020_1004 => self.complete(1, v),
            _ => {}
        }
    }
}

/// Lossless NS16550A serial console UART (`0x1000_0000`, PLIC IRQ 10).
#[derive(Clone, Debug)]
pub struct Uart16550 {
    ier: u8,
    lcr: u8,
    mcr: u8,
    scr: u8,
    dll: u8,
    dlm: u8,
    tx_irq_pending: bool,
    rx_fifo: VecDeque<u8>,
    tx_fifo: VecDeque<u8>,
}

const IER_ERBFI: u8 = 1 << 0;
const IER_ETBEI: u8 = 1 << 1;
const LCR_DLAB: u8 = 1 << 7;

impl Default for Uart16550 {
    fn default() -> Self {
        Self::new()
    }
}

impl Uart16550 {
    pub fn new() -> Self {
        Self {
            ier: 0,
            lcr: 0x03, // 8N1
            mcr: 0,
            scr: 0,
            dll: 0x01,
            dlm: 0x00,
            tx_irq_pending: false,
            rx_fifo: VecDeque::with_capacity(4096),
            tx_fifo: VecDeque::with_capacity(16384),
        }
    }

    pub fn push_input(&mut self, bytes: &[u8], plic: &mut PlicState) {
        self.rx_fifo.extend(bytes.iter().copied());
        self.sync_irq(plic);
    }

    pub fn drain_output(&mut self) -> Vec<u8> {
        self.tx_fifo.drain(..).collect()
    }

    pub fn push_console_output(&mut self, byte: u8) {
        self.tx_fifo.push_back(byte);
    }

    pub fn has_pending_output(&self) -> bool {
        !self.tx_fifo.is_empty()
    }

    fn sync_irq(&self, plic: &mut PlicState) {
        let rx_int = (self.ier & IER_ERBFI) != 0 && !self.rx_fifo.is_empty();
        let tx_int = (self.ier & IER_ETBEI) != 0 && self.tx_irq_pending;
        plic.set_source_level(UART_IRQ, rx_int || tx_int);
    }

    pub fn read(&mut self, offset: u64, plic: &mut PlicState) -> u64 {
        let dlab = (self.lcr & LCR_DLAB) != 0;
        let b: u8 = match offset {
            0 if dlab => self.dll,
            0 => {
                let v = self.rx_fifo.pop_front().unwrap_or(0);
                self.sync_irq(plic);
                v
            }
            1 if dlab => self.dlm,
            1 => self.ier,
            2 => {
                if (self.ier & IER_ERBFI) != 0 && !self.rx_fifo.is_empty() {
                    0xC4 // FIFO enabled + Received Data Available
                } else if (self.ier & IER_ETBEI) != 0 && self.tx_irq_pending {
                    self.tx_irq_pending = false;
                    self.sync_irq(plic);
                    0xC2 // FIFO enabled + THRE
                } else {
                    0xC1 // No interrupt pending
                }
            }
            3 => self.lcr,
            4 => self.mcr,
            5 => {
                let mut lsr = 0x60; // THRE | TEMT always ready
                if !self.rx_fifo.is_empty() {
                    lsr |= 0x01; // DR
                }
                lsr
            }
            6 => 0xB0, // CTS | DSR | DCD asserted
            7 => self.scr,
            _ => 0,
        };
        u64::from(b)
    }

    pub fn write(&mut self, offset: u64, val: u64, plic: &mut PlicState) {
        let b = val as u8;
        let dlab = (self.lcr & LCR_DLAB) != 0;
        match offset {
            0 if dlab => self.dll = b,
            0 => {
                self.tx_fifo.push_back(b);
                if (self.ier & IER_ETBEI) != 0 {
                    self.tx_irq_pending = true;
                }
                self.sync_irq(plic);
            }
            1 if dlab => self.dlm = b,
            1 => {
                let prev = self.ier;
                self.ier = b & 0x0F;
                if (prev & IER_ETBEI) == 0 && (self.ier & IER_ETBEI) != 0 {
                    self.tx_irq_pending = true;
                }
                if (self.ier & IER_ETBEI) == 0 {
                    self.tx_irq_pending = false;
                }
                self.sync_irq(plic);
            }
            2 => {
                if (b & 0x02) != 0 {
                    self.rx_fifo.clear();
                    self.sync_irq(plic);
                }
            }
            3 => self.lcr = b,
            4 => self.mcr = b,
            7 => self.scr = b,
            _ => {}
        }
    }
}

/// Goldfish wall-clock Real-Time Clock (`0x0010_1000`).
#[derive(Clone, Debug)]
pub struct GoldfishRtc {
    base_epoch_ns: u64,
    latched_high: u32,
    pending_write_low: u32,
    irq_enabled: u32,
}

impl GoldfishRtc {
    pub fn new(base_epoch_ns: u64) -> Self {
        Self {
            base_epoch_ns,
            latched_high: 0,
            pending_write_low: 0,
            irq_enabled: 0,
        }
    }

    pub fn set_base_epoch_ns(&mut self, epoch_ns: u64) {
        self.base_epoch_ns = epoch_ns;
    }

    pub fn current_ns(&self, mtime: u64) -> u64 {
        // mtime runs at 10 MHz => 1 tick = 100 ns.
        self.base_epoch_ns.wrapping_add(mtime.saturating_mul(100))
    }

    pub fn read(&mut self, offset: u64, mtime: u64) -> u64 {
        let v: u32 = match offset {
            0x00 => {
                let now = self.current_ns(mtime);
                self.latched_high = (now >> 32) as u32;
                now as u32
            }
            0x04 => self.latched_high,
            0x0C => self.irq_enabled,
            _ => 0,
        };
        u64::from(v)
    }

    pub fn write(&mut self, offset: u64, val: u64, mtime: u64) {
        let v = val as u32;
        match offset {
            0x00 => self.pending_write_low = v,
            0x04 => {
                let target_ns = (u64::from(v) << 32) | u64::from(self.pending_write_low);
                self.base_epoch_ns = target_ns.saturating_sub(mtime.saturating_mul(100));
            }
            0x0C => self.irq_enabled = v & 1,
            _ => {}
        }
    }
}

/// SiFive/Syscon system reset register (`0x0010_0000`: `0x5555` poweroff, `0x7777` reboot).
#[derive(Clone, Debug, Default)]
pub struct SysconDevice {
    pub reset_action: Option<SystemResetAction>,
}

impl SysconDevice {
    pub fn write(&mut self, offset: u64, val: u64) {
        if offset == 0 {
            match (val as u32) & 0xFFFF {
                0x5555 | 0x3333 => self.reset_action = Some(SystemResetAction::PowerOff),
                0x7777 => self.reset_action = Some(SystemResetAction::ColdReboot),
                _ => {}
            }
        }
    }
}

/// Pluggable external MMIO device (e.g. VirtIO MMIO transports) with direct GuestRam + PLIC DMA access.
pub trait MmioDevice: Send {
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;

    fn read(
        &mut self,
        offset: u64,
        size: usize,
        ram: &mut GuestRam,
        plic: &mut PlicState,
    ) -> Result<u64, Trap>;

    fn write(
        &mut self,
        offset: u64,
        size: usize,
        value: u64,
        ram: &mut GuestRam,
        plic: &mut PlicState,
    ) -> Result<(), Trap>;
}

struct MmioSlot {
    base: u64,
    size: u64,
    dev: Box<dyn MmioDevice>,
}

/// Unified physical memory & MMIO system bus with O(1) DRAM fast path.
pub struct MachineBus {
    pub ram: GuestRam,
    pub fb: SimpleFramebuffer,
    pub clint: ClintState,
    pub plic: PlicState,
    pub uart: Uart16550,
    pub rtc: GoldfishRtc,
    pub syscon: SysconDevice,
    mmio_slots: Vec<MmioSlot>,
}

impl MachineBus {
    pub fn new(ram_bytes: usize, fb_width: u32, fb_height: u32, base_epoch_ns: u64) -> Self {
        Self {
            ram: GuestRam::new(ram_bytes),
            fb: SimpleFramebuffer::new(fb_width, fb_height),
            clint: ClintState::new(),
            plic: PlicState::new(),
            uart: Uart16550::new(),
            rtc: GoldfishRtc::new(base_epoch_ns),
            syscon: SysconDevice::default(),
            mmio_slots: Vec::new(),
        }
    }

    pub fn register_mmio(&mut self, base: u64, size: u64, dev: Box<dyn MmioDevice>) {
        self.mmio_slots.push(MmioSlot { base, size, dev });
    }

    pub fn with_mmio<R>(&self, base: u64, f: impl FnOnce(&dyn MmioDevice) -> R) -> Option<R> {
        for slot in &self.mmio_slots {
            if slot.base == base {
                return Some(f(slot.dev.as_ref()));
            }
        }
        None
    }

    pub fn with_mmio_mut<R>(
        &mut self,
        base: u64,
        f: impl FnOnce(&mut dyn MmioDevice, &mut GuestRam, &mut PlicState) -> R,
    ) -> Option<R> {
        for slot in &mut self.mmio_slots {
            if slot.base == base {
                return Some(f(slot.dev.as_mut(), &mut self.ram, &mut self.plic));
            }
        }
        None
    }

    #[inline]
    pub fn read_phys(&mut self, paddr: u64, size: usize, kind: AccessType) -> Result<u64, Trap> {
        if paddr >= GUEST_RAM_BASE {
            return self.ram.read_scalar(paddr, size, kind);
        }
        let fb_end = GUEST_FB_BASE + (self.fb.byte_len() as u64);
        if paddr >= GUEST_FB_BASE && paddr < fb_end {
            return self.fb.read(paddr - GUEST_FB_BASE, size, paddr, kind);
        }
        if (UART_BASE..UART_BASE + UART_SIZE).contains(&paddr) {
            return Ok(self.uart.read(paddr - UART_BASE, &mut self.plic));
        }
        if (CLINT_BASE..CLINT_BASE + CLINT_SIZE).contains(&paddr) {
            return Ok(self.clint.read(paddr - CLINT_BASE, size));
        }
        if (PLIC_BASE..PLIC_BASE + PLIC_SIZE).contains(&paddr) {
            return Ok(self.plic.read(paddr - PLIC_BASE, size));
        }
        if (RTC_BASE..RTC_BASE + RTC_SIZE).contains(&paddr) {
            return Ok(self.rtc.read(paddr - RTC_BASE, self.clint.mtime));
        }
        if (SYSCON_BASE..SYSCON_BASE + SYSCON_SIZE).contains(&paddr) {
            return Ok(0);
        }
        for slot in &mut self.mmio_slots {
            if paddr >= slot.base && paddr < slot.base + slot.size {
                if paddr.saturating_add(size as u64) > slot.base + slot.size {
                    return Err(Trap::MemoryOutOfBounds { addr: paddr, kind });
                }
                return slot
                    .dev
                    .read(paddr - slot.base, size, &mut self.ram, &mut self.plic);
            }
        }
        Err(Trap::MemoryOutOfBounds { addr: paddr, kind })
    }

    #[inline]
    pub fn write_phys(
        &mut self,
        paddr: u64,
        size: usize,
        val: u64,
        kind: AccessType,
    ) -> Result<(), Trap> {
        if paddr >= GUEST_RAM_BASE {
            return self.ram.write_scalar(paddr, size, val, kind);
        }
        let fb_end = GUEST_FB_BASE + (self.fb.byte_len() as u64);
        if paddr >= GUEST_FB_BASE && paddr < fb_end {
            return self.fb.write(paddr - GUEST_FB_BASE, size, val, paddr);
        }
        if (UART_BASE..UART_BASE + UART_SIZE).contains(&paddr) {
            self.uart.write(paddr - UART_BASE, val, &mut self.plic);
            return Ok(());
        }
        if (CLINT_BASE..CLINT_BASE + CLINT_SIZE).contains(&paddr) {
            self.clint.write(paddr - CLINT_BASE, size, val);
            return Ok(());
        }
        if (PLIC_BASE..PLIC_BASE + PLIC_SIZE).contains(&paddr) {
            self.plic.write(paddr - PLIC_BASE, size, val);
            return Ok(());
        }
        if (RTC_BASE..RTC_BASE + RTC_SIZE).contains(&paddr) {
            self.rtc.write(paddr - RTC_BASE, val, self.clint.mtime);
            return Ok(());
        }
        if (SYSCON_BASE..SYSCON_BASE + SYSCON_SIZE).contains(&paddr) {
            self.syscon.write(paddr - SYSCON_BASE, val);
            return Ok(());
        }
        for slot in &mut self.mmio_slots {
            if paddr >= slot.base && paddr < slot.base + slot.size {
                if paddr.saturating_add(size as u64) > slot.base + slot.size {
                    return Err(Trap::MemoryOutOfBounds { addr: paddr, kind });
                }
                return slot
                    .dev
                    .write(paddr - slot.base, size, val, &mut self.ram, &mut self.plic);
            }
        }
        Err(Trap::MemoryOutOfBounds { addr: paddr, kind })
    }
}
