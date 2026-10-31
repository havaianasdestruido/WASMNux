//! VirtIO-MMIO block device (**legacy MMIO interface, version 1**). Linux's
//! `virtio_mmio` driver supports both the legacy and the VirtIO-1.0 MMIO
//! transports; the legacy register layout is implemented here first because
//! it is smaller to audit. Moving to the modern transport is tracked in
//! TODO.MD (Phase 2) and must not be confused with it: this device honestly
//! reports `VERSION = 1` so the guest uses the matching legacy driver path.
//!
//! Requests are processed synchronously on `QUEUE_NOTIFY` by walking the
//! descriptor chain in guest RAM, exactly as a device model is allowed to.

use crate::block::{BlockBackend, SECTOR_SIZE};
use crate::bus::MmioDevice;
use crate::error::{Result, VmError};
use crate::memory::Ram;

const MAGIC: u32 = 0x7472_6976; // "virt"
const VERSION: u32 = 1;
const DEVICE_ID_BLOCK: u32 = 2;
const VENDOR_ID: u32 = 0x574E_5558; // "WNUX"
const QUEUE_NUM_MAX: u32 = 128;
const QUEUE_ALIGN: u32 = 4096;
const PAGE_SIZE: u64 = 4096;

const REG_MAGIC: u64 = 0x00;
const REG_VERSION: u64 = 0x04;
const REG_DEVICE_ID: u64 = 0x08;
const REG_VENDOR_ID: u64 = 0x0C;
const REG_DEVICE_FEATURES: u64 = 0x10;
const REG_DEVICE_FEATURES_SEL: u64 = 0x14;
const REG_DRIVER_FEATURES: u64 = 0x20;
const REG_DRIVER_FEATURES_SEL: u64 = 0x24;
const REG_GUEST_PAGE_SIZE: u64 = 0x28;
const REG_QUEUE_SEL: u64 = 0x30;
const REG_QUEUE_NUM_MAX: u64 = 0x34;
const REG_QUEUE_NUM: u64 = 0x38;
const REG_QUEUE_ALIGN: u64 = 0x3C;
const REG_QUEUE_PFN: u64 = 0x40;
const REG_QUEUE_NOTIFY: u64 = 0x50;
const REG_INTERRUPT_STATUS: u64 = 0x60;
const REG_INTERRUPT_ACK: u64 = 0x64;
const REG_STATUS: u64 = 0x70;
const REG_CONFIG: u64 = 0x100;

// VirtIO block request types (spec §5.2.6).
const REQ_READ: u32 = 0;
const REQ_WRITE: u32 = 1;
const REQ_FLUSH: u32 = 4;

// VirtIO descriptor flags.
const DESC_FLAG_NEXT: u16 = 1;
const DESC_FLAG_WRITE: u16 = 2;

const STATUS_OK: u8 = 0;
const STATUS_IOERR: u8 = 1;
const STATUS_UNSUPP: u8 = 2;

/// Legacy VirtIO-MMIO block device over a [`BlockBackend`].
#[derive(Debug)]
pub struct VirtioBlock<B: BlockBackend> {
    backend: B,
    device_features_sel: u32,
    driver_features: [u32; 2],
    driver_features_sel: u32,
    guest_page_size: u32,
    queue_sel: u32,
    queue_num: u32,
    queue_pfn: u32,
    interrupt_status: u32,
    status: u32,
    last_avail_index: u16,
    /// Set when the guest kicks the queue; the bus processes descriptors
    /// because only the bus can reach guest RAM and this device together.
    notify_pending: bool,
}

impl<B: BlockBackend> VirtioBlock<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            device_features_sel: 0,
            driver_features: [0; 2],
            driver_features_sel: 0,
            guest_page_size: PAGE_SIZE as u32,
            queue_sel: 0,
            queue_num: 0,
            queue_pfn: 0,
            interrupt_status: 0,
            status: 0,
            last_avail_index: 0,
            notify_pending: false,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Whether the device interrupt line is asserted.
    pub fn irq_pending(&self) -> bool {
        self.interrupt_status != 0
    }

    fn reset(&mut self) {
        self.status = 0;
        self.queue_num = 0;
        self.queue_pfn = 0;
        self.queue_sel = 0;
        self.interrupt_status = 0;
        self.last_avail_index = 0;
        self.driver_features = [0; 2];
        self.notify_pending = false;
    }

    fn queue_addresses(&self) -> Option<(u64, u64, u64)> {
        if self.queue_pfn == 0 || self.queue_num == 0 {
            return None;
        }
        let base = u64::from(self.queue_pfn) * u64::from(self.guest_page_size);
        let n = u64::from(self.queue_num);
        let desc = base;
        let avail = desc + n * 16;
        let used_unaligned = avail + 2 + 2 + 2 * n + 2;
        let used = align_up(used_unaligned, u64::from(QUEUE_ALIGN));
        Some((desc, avail, used))
    }

    /// Processes queued requests. Called by the bus after a queue kick,
    /// with exclusive access to guest RAM. Returns the number of completed
    /// descriptor chains; malformed guest chains complete with an I/O error
    /// status instead of panicking — guest memory is never trusted.
    pub fn process_queue(&mut self, ram: &mut Ram) -> usize {
        self.notify_pending = false;
        let Some((desc_base, avail_base, used_base)) = self.queue_addresses() else {
            return 0;
        };
        let n = self.queue_num;
        let Ok(avail_index) = ram.read_int(avail_base + 2, 2) else {
            return 0;
        };
        let avail_index = avail_index as u16;
        let mut completed = 0_usize;
        while self.last_avail_index != avail_index {
            let ring_slot = u64::from(self.last_avail_index % n as u16);
            let head = match ram.read_int(avail_base + 4 + ring_slot * 2, 2) {
                Ok(v) => v as u16,
                Err(_) => break,
            };
            self.last_avail_index = self.last_avail_index.wrapping_add(1);
            if u32::from(head) >= n {
                continue;
            }
            let status = self.run_chain(ram, desc_base, head);
            // Append to the used ring.
            let Ok(used_index) = ram.read_int(used_base + 2, 2) else {
                break;
            };
            let used_slot = used_base + 4 + (used_index % u64::from(n)) * 8;
            let _ = status;
            if ram.write_int(used_slot, 4, u64::from(head)).is_err()
                || ram.write_int(used_slot + 4, 4, 0).is_err()
                || ram
                    .write_int(used_base + 2, 2, used_index.wrapping_add(1))
                    .is_err()
            {
                break;
            }
            completed += 1;
        }
        if completed > 0 {
            self.interrupt_status |= 1;
        }
        completed
    }

    /// Runs one request chain: `outhdr`, data descriptors, `inhdr` status.
    fn run_chain(&mut self, ram: &mut Ram, desc_base: u64, head: u16) -> u8 {
        let chain = match read_chain(ram, desc_base, head, self.queue_num) {
            Ok(chain) => chain,
            Err(()) => return STATUS_IOERR,
        };
        if chain.is_empty() {
            return STATUS_IOERR;
        }
        // outhdr: { type: u32, reserved: u32, sector: u64 }
        let hdr = chain[0];
        if hdr.len < 16 || hdr.flags & DESC_FLAG_WRITE != 0 {
            return STATUS_IOERR;
        }
        let req_type = match ram.read_int(hdr.addr, 4) {
            Ok(v) => v as u32,
            Err(_) => return STATUS_IOERR,
        };
        let sector = match ram.read_int(hdr.addr + 8, 8) {
            Ok(v) => v,
            Err(_) => return STATUS_IOERR,
        };
        let status_desc = chain[chain.len() - 1];
        let data_descs = &chain[1..chain.len().saturating_sub(1)];
        let status = match req_type {
            REQ_READ | REQ_WRITE => {
                let mut offset = sector.saturating_mul(SECTOR_SIZE as u64);
                let mut result = STATUS_OK;
                for desc in data_descs {
                    let len = desc.len as usize;
                    let mut buf = vec![0_u8; len];
                    if req_type == REQ_READ {
                        let expect_write = desc.flags & DESC_FLAG_WRITE != 0;
                        if !expect_write
                            || self.backend.read_at(offset, &mut buf).is_err()
                            || ram.write(desc.addr, &buf).is_err()
                        {
                            result = STATUS_IOERR;
                            break;
                        }
                    } else if ram.read(desc.addr, &mut buf).is_err()
                        || self.backend.write_at(offset, &buf).is_err()
                    {
                        result = STATUS_IOERR;
                        break;
                    }
                    offset = offset.saturating_add(len as u64);
                }
                result
            }
            REQ_FLUSH => {
                if self.backend.flush().is_ok() {
                    STATUS_OK
                } else {
                    STATUS_IOERR
                }
            }
            _ => STATUS_UNSUPP,
        };
        if status_desc.flags & DESC_FLAG_WRITE != 0 && status_desc.len >= 1 {
            let _ = ram.write_int(status_desc.addr, 1, u64::from(status));
        }
        status
    }

    /// Whether a queue kick is waiting for [`Self::process_queue`].
    pub fn take_notify_pending(&mut self) -> bool {
        let was = self.notify_pending;
        self.notify_pending = false;
        was
    }
}

#[derive(Debug, Clone, Copy)]
struct Descriptor {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

fn read_chain(
    ram: &Ram,
    desc_base: u64,
    head: u16,
    queue_num: u32,
) -> std::result::Result<Vec<Descriptor>, ()> {
    let mut chain = Vec::new();
    let mut index = head;
    loop {
        if u32::from(index) >= queue_num || chain.len() >= queue_num as usize {
            return Err(());
        }
        let base = desc_base + u64::from(index) * 16;
        let desc = Descriptor {
            addr: ram.read_int(base, 8).map_err(|_| ())?,
            len: ram.read_int(base + 8, 4).map_err(|_| ())? as u32,
            flags: ram.read_int(base + 12, 2).map_err(|_| ())? as u16,
            next: ram.read_int(base + 14, 2).map_err(|_| ())? as u16,
        };
        chain.push(desc);
        if desc.flags & DESC_FLAG_NEXT == 0 {
            return Ok(chain);
        }
        index = desc.next;
    }
}

const fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}

impl<B: BlockBackend> MmioDevice for VirtioBlock<B> {
    fn read(&mut self, offset: u64, size: usize) -> Result<u64> {
        if offset >= REG_CONFIG {
            // Configuration space: capacity (in sectors) at byte 0.
            let capacity = self.backend.sector_count();
            let shift = (offset - REG_CONFIG) * 8;
            let value = if shift >= 64 {
                0
            } else {
                match size {
                    1 => (capacity >> shift) & 0xFF,
                    2 => (capacity >> shift) & 0xFFFF,
                    4 => (capacity >> shift) & 0xFFFF_FFFF,
                    8 if shift == 0 => capacity,
                    _ => return Err(VmError::UnsupportedAccessSize { size }),
                }
            };
            return Ok(value);
        }
        if size != 4 {
            return Err(VmError::UnsupportedAccessSize { size });
        }
        let value = match offset {
            REG_MAGIC => MAGIC,
            REG_VERSION => VERSION,
            REG_DEVICE_ID => DEVICE_ID_BLOCK,
            REG_VENDOR_ID => VENDOR_ID,
            // Features: none beyond the mandatory set, both pages read 0.
            REG_DEVICE_FEATURES => 0,
            REG_QUEUE_NUM_MAX => {
                if self.queue_sel == 0 {
                    QUEUE_NUM_MAX
                } else {
                    0
                }
            }
            REG_QUEUE_NUM => self.queue_num,
            REG_QUEUE_ALIGN => QUEUE_ALIGN,
            REG_QUEUE_PFN => self.queue_pfn,
            REG_INTERRUPT_STATUS => self.interrupt_status,
            REG_STATUS => self.status,
            _ => 0,
        };
        Ok(u64::from(value))
    }

    fn write(&mut self, offset: u64, size: usize, value: u64) -> Result<()> {
        if size != 4 {
            return Err(VmError::UnsupportedAccessSize { size });
        }
        let value = (value & 0xFFFF_FFFF) as u32;
        match offset {
            REG_DEVICE_FEATURES_SEL => self.device_features_sel = value,
            REG_DRIVER_FEATURES => {
                let sel = (self.driver_features_sel as usize).min(1);
                self.driver_features[sel] = value;
            }
            REG_DRIVER_FEATURES_SEL => self.driver_features_sel = value,
            REG_GUEST_PAGE_SIZE => {
                if value.is_power_of_two() && value >= 4096 {
                    self.guest_page_size = value;
                }
            }
            REG_QUEUE_SEL => self.queue_sel = value,
            REG_QUEUE_NUM => {
                if self.queue_sel == 0 {
                    self.queue_num = value.min(QUEUE_NUM_MAX);
                }
            }
            REG_QUEUE_PFN => {
                if self.queue_sel == 0 {
                    self.queue_pfn = value;
                }
            }
            REG_QUEUE_NOTIFY => {
                if value == 0 {
                    self.notify_pending = true;
                }
            }
            REG_INTERRUPT_ACK => self.interrupt_status &= !value,
            REG_STATUS => {
                if value == 0 {
                    self.reset();
                } else {
                    self.status = value;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::VirtioBlock;
    use crate::block::{BlockBackend, VecBlockBackend, SECTOR_SIZE};
    use crate::bus::MmioDevice;
    use crate::memory::Ram;

    const RAM_BASE: u64 = 0x8000_0000;
    const QUEUE_PAGE: u64 = RAM_BASE + 0x10_0000;
    const OUT_HDR: u64 = RAM_BASE + 0x20_0000;
    const DATA: u64 = RAM_BASE + 0x20_1000;
    const STATUS: u64 = RAM_BASE + 0x20_2000;

    fn setup() -> (Ram, VirtioBlock<VecBlockBackend>) {
        let ram = Ram::new(RAM_BASE, 8 * 1024 * 1024).expect("ram");
        let backend = VecBlockBackend::new(16 * SECTOR_SIZE).expect("disk");
        let mut dev = VirtioBlock::new(backend);
        dev.write(0x30, 4, 0).expect("queue sel");
        dev.write(0x38, 4, 8).expect("queue num");
        dev.write(0x40, 4, (QUEUE_PAGE / 4096) as u64)
            .expect("queue pfn");
        (ram, dev)
    }

    fn put_desc(ram: &mut Ram, idx: u64, addr: u64, len: u32, flags: u16, next: u16) {
        let base = QUEUE_PAGE + idx * 16;
        ram.write_int(base, 8, addr).expect("desc addr");
        ram.write_int(base + 8, 4, u64::from(len))
            .expect("desc len");
        ram.write_int(base + 12, 2, u64::from(flags))
            .expect("desc flags");
        ram.write_int(base + 14, 2, u64::from(next))
            .expect("desc next");
    }

    fn post_request(ram: &mut Ram, req_type: u64, sector: u64, data_len: u32) {
        // queue_num = 8: avail at QUEUE_PAGE + 8*16; head descriptor 0.
        let avail = QUEUE_PAGE + 8 * 16;
        ram.write_int(OUT_HDR, 4, req_type).expect("req type");
        ram.write_int(OUT_HDR + 8, 8, sector).expect("sector");
        let data_flags: u16 = if req_type == 0 { 2 | 1 } else { 1 }; // read: WRITE|NEXT
        put_desc(ram, 0, OUT_HDR, 16, 1, 1);
        put_desc(ram, 1, DATA, data_len, data_flags, 2);
        put_desc(ram, 2, STATUS, 1, 2, 0);
        ram.write_int(avail + 4, 2, 0).expect("avail ring head");
        ram.write_int(avail + 2, 2, 1).expect("avail index");
        ram.write_int(STATUS, 1, 0xFF).expect("poison status");
    }

    #[test]
    fn register_file_identifies_a_legacy_block_device() {
        let (_ram, mut dev) = setup();
        assert_eq!(dev.read(0x00, 4).expect("magic"), 0x7472_6976);
        assert_eq!(dev.read(0x04, 4).expect("version"), 1);
        assert_eq!(dev.read(0x08, 4).expect("device id"), 2);
        assert_eq!(dev.read(0x34, 4).expect("num max"), 128);
        assert_eq!(dev.read(0x100, 8).expect("capacity"), 16);
    }

    #[test]
    fn writes_then_reads_a_sector_through_the_queue() {
        let (mut ram, mut dev) = setup();
        let payload = [0x5A_u8; SECTOR_SIZE];
        ram.write(DATA, &payload).expect("guest data");
        post_request(&mut ram, 1, 3, SECTOR_SIZE as u32);
        assert_eq!(dev.process_queue(&mut ram), 1);
        assert_eq!(ram.read_int(STATUS, 1).expect("status"), 0, "OK status");
        assert!(dev.irq_pending());
        let mut check = [0_u8; SECTOR_SIZE];
        dev.backend_mut()
            .read_at(3 * SECTOR_SIZE as u64, &mut check)
            .expect("backend read");
        assert_eq!(check, payload);

        // Read it back into a different guest buffer state.
        ram.write(DATA, &[0_u8; SECTOR_SIZE])
            .expect("clear guest buf");
        post_request(&mut ram, 0, 3, SECTOR_SIZE as u32);
        assert_eq!(dev.process_queue(&mut ram), 1);
        let mut back = [0_u8; SECTOR_SIZE];
        ram.read(DATA, &mut back).expect("read back");
        assert_eq!(back, payload);
    }

    #[test]
    fn malformed_chains_complete_with_error_instead_of_panicking() {
        let (mut ram, mut dev) = setup();
        // Descriptor 0 points at itself forever.
        put_desc(&mut ram, 0, OUT_HDR, 16, 1, 0);
        let avail = QUEUE_PAGE + 8 * 16;
        ram.write_int(avail + 4, 2, 0).expect("head");
        ram.write_int(avail + 2, 2, 1).expect("avail idx");
        assert_eq!(dev.process_queue(&mut ram), 1);
    }
}
