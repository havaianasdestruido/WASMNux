//! VirtIO 1.2 over MMIO v2 (`VERSION = 2`) split-virtqueue transport and devices:
//! - `VirtioBlkMmio` (`DeviceID = 2`) backed by `wasmnux_storage::SparseBlockDisk`
//! - `VirtioNetMmio` (`DeviceID = 1`) backed by `wasmnux_net::VirtualNetworkSwitch`
//! - `VirtioRngMmio` (`DeviceID = 4`) deterministic/seeded entropy source

use wasmnux_net::{BridgeHttpRequest, BridgeHttpResponse, VirtualNetworkSwitch, GUEST_MAC};
use wasmnux_riscv::bus::{GuestRam, MmioDevice, PlicState};
use wasmnux_riscv::trap::Trap;
use wasmnux_storage::{SparseBlockDisk, SECTOR_SIZE};

pub const VIRTIO_MMIO_MAGIC: u32 = 0x7472_6976; // "virt"
pub const VIRTIO_MMIO_VERSION: u32 = 2;
pub const VIRTIO_VENDOR_ID: u32 = 0x574E_5558; // "WNUX"

pub const VIRTIO_DEV_NET: u32 = 1;
pub const VIRTIO_DEV_BLK: u32 = 2;
pub const VIRTIO_DEV_RNG: u32 = 4;

pub const VIRTIO_F_VERSION_1: u64 = 1u64 << 32;
pub const VIRTIO_BLK_F_BLK_SIZE: u64 = 1u64 << 6;
pub const VIRTIO_BLK_F_FLUSH: u64 = 1u64 << 9;

pub const VIRTIO_NET_F_MAC: u64 = 1u64 << 5;
pub const VIRTIO_NET_F_STATUS: u64 = 1u64 << 16;

pub const VIRTIO_MMIO_INT_VRING: u32 = 1 << 0;
pub const VIRTIO_MMIO_INT_CONFIG: u32 = 1 << 1;

pub const VRING_DESC_F_NEXT: u16 = 1;
pub const VRING_DESC_F_WRITE: u16 = 2;

pub const VIRTIO_QUEUE_MAX: u32 = 256;

/// VirtIO 1.0+ network header length (`struct virtio_net_hdr_mrg_rxbuf` = 12 bytes).
pub const VIRTIO_NET_HDR_V1_LEN: usize = 12;

#[derive(Clone, Debug, Default)]
pub struct VirtQueue {
    pub num: u32,
    pub ready: bool,
    pub desc_addr: u64,
    pub avail_addr: u64,
    pub used_addr: u64,
    pub last_avail_idx: u16,
}

impl VirtQueue {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Clone, Copy, Debug)]
pub struct VringDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

/// Pop the next available descriptor chain from `vq` if any is pending.
pub fn pop_avail_chain(vq: &mut VirtQueue, ram: &GuestRam) -> Option<(u16, Vec<VringDesc>)> {
    if !vq.ready || vq.num == 0 || vq.num > VIRTIO_QUEUE_MAX {
        return None;
    }
    let mut hdr = [0u8; 4];
    ram.read_bytes(vq.avail_addr, &mut hdr).ok()?;
    let avail_idx = u16::from_le_bytes([hdr[2], hdr[3]]);
    if vq.last_avail_idx == avail_idx {
        return None;
    }

    let slot = u64::from(vq.last_avail_idx % (vq.num as u16));
    let mut head_buf = [0u8; 2];
    ram.read_bytes(vq.avail_addr + 4 + slot * 2, &mut head_buf)
        .ok()?;
    let head = u16::from_le_bytes(head_buf);
    vq.last_avail_idx = vq.last_avail_idx.wrapping_add(1);

    let mut chain = Vec::with_capacity(8);
    let mut idx = head;
    let max_hops = vq.num as usize;
    for _ in 0..max_hops {
        if u32::from(idx) >= vq.num {
            return None;
        }
        let mut raw = [0u8; 16];
        ram.read_bytes(vq.desc_addr + u64::from(idx) * 16, &mut raw)
            .ok()?;
        let desc = VringDesc {
            addr: u64::from_le_bytes([
                raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
            ]),
            len: u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]),
            flags: u16::from_le_bytes([raw[12], raw[13]]),
            next: u16::from_le_bytes([raw[14], raw[15]]),
        };
        chain.push(desc);
        if (desc.flags & VRING_DESC_F_NEXT) == 0 {
            break;
        }
        idx = desc.next;
    }
    Some((head, chain))
}

/// Push a completed descriptor chain (`head`, `written_len`) into `vq`'s Used ring.
pub fn push_used_elem(vq: &VirtQueue, ram: &mut GuestRam, head: u16, written_len: u32) -> bool {
    if !vq.ready || vq.num == 0 {
        return false;
    }
    let mut hdr = [0u8; 4];
    if ram.read_bytes(vq.used_addr, &mut hdr).is_err() {
        return false;
    }
    let used_idx = u16::from_le_bytes([hdr[2], hdr[3]]);
    let slot = u64::from(used_idx % (vq.num as u16));
    let elem_addr = vq.used_addr + 4 + slot * 8;

    let mut elem = [0u8; 8];
    elem[0..4].copy_from_slice(&u32::from(head).to_le_bytes());
    elem[4..8].copy_from_slice(&written_len.to_le_bytes());
    if ram.write_bytes(elem_addr, &elem).is_err() {
        return false;
    }
    let next_idx = used_idx.wrapping_add(1);
    ram.write_bytes(vq.used_addr + 2, &next_idx.to_le_bytes())
        .is_ok()
}

/// Common VirtIO MMIO v2 register state shared across Block, Net, and RNG devices.
pub struct VirtioMmioState {
    pub device_id: u32,
    pub irq: u32,
    pub device_features: u64,
    pub device_features_sel: u32,
    pub driver_features: u64,
    pub driver_features_sel: u32,
    pub queue_sel: u32,
    pub queues: Vec<VirtQueue>,
    pub interrupt_status: u32,
    pub status: u32,
    pub config_generation: u32,
    pub config_space: Vec<u8>,
}

impl VirtioMmioState {
    pub fn new(
        device_id: u32,
        irq: u32,
        num_queues: usize,
        device_features: u64,
        config: Vec<u8>,
    ) -> Self {
        Self {
            device_id,
            irq,
            device_features: device_features | VIRTIO_F_VERSION_1,
            device_features_sel: 0,
            driver_features: 0,
            driver_features_sel: 0,
            queue_sel: 0,
            queues: vec![VirtQueue::default(); num_queues],
            interrupt_status: 0,
            status: 0,
            config_generation: 0,
            config_space: config,
        }
    }

    pub fn reset(&mut self, plic: &mut PlicState) {
        self.device_features_sel = 0;
        self.driver_features = 0;
        self.driver_features_sel = 0;
        self.queue_sel = 0;
        for q in &mut self.queues {
            q.reset();
        }
        self.interrupt_status = 0;
        self.status = 0;
        plic.set_source_level(self.irq, false);
    }

    #[inline]
    pub fn signal_used_buffer(&mut self, plic: &mut PlicState) {
        self.interrupt_status |= VIRTIO_MMIO_INT_VRING;
        plic.set_source_level(self.irq, true);
    }

    pub fn read_reg(&self, offset: u64, size: usize) -> u64 {
        if offset >= 0x100 {
            let cfg_off = (offset - 0x100) as usize;
            let mut out = [0u8; 8];
            let take = size.min(8);
            for (i, dst) in out.iter_mut().enumerate().take(take) {
                if let Some(&b) = self.config_space.get(cfg_off + i) {
                    *dst = b;
                }
            }
            return u64::from_le_bytes(out);
        }

        let q = self.queues.get(self.queue_sel as usize);
        let val: u32 = match offset {
            0x000 => VIRTIO_MMIO_MAGIC,
            0x004 => VIRTIO_MMIO_VERSION,
            0x008 => self.device_id,
            0x00C => VIRTIO_VENDOR_ID,
            0x010 => match self.device_features_sel {
                0 => self.device_features as u32,
                1 => (self.device_features >> 32) as u32,
                _ => 0,
            },
            0x034 => {
                if q.is_some() {
                    VIRTIO_QUEUE_MAX
                } else {
                    0
                }
            }
            0x044 => q.map_or(0, |vq| u32::from(vq.ready)),
            0x060 => self.interrupt_status,
            0x070 => self.status,
            0x0FC => self.config_generation,
            _ => 0,
        };
        u64::from(val)
    }

    /// Returns `Some(queue_idx)` when `QueueNotify` (`0x050`) is written.
    pub fn write_reg(
        &mut self,
        offset: u64,
        size: usize,
        value: u64,
        plic: &mut PlicState,
    ) -> Option<usize> {
        if offset >= 0x100 {
            let cfg_off = (offset - 0x100) as usize;
            let bytes = value.to_le_bytes();
            let take = size.min(8);
            for (i, &b) in bytes.iter().enumerate().take(take) {
                if let Some(slot) = self.config_space.get_mut(cfg_off + i) {
                    *slot = b;
                }
            }
            return None;
        }

        let v = value as u32;
        match offset {
            0x014 => self.device_features_sel = v,
            0x020 => {
                if self.driver_features_sel == 0 {
                    self.driver_features = (self.driver_features & !0xFFFF_FFFF) | u64::from(v);
                } else if self.driver_features_sel == 1 {
                    self.driver_features =
                        (self.driver_features & 0xFFFF_FFFF) | (u64::from(v) << 32);
                }
            }
            0x024 => self.driver_features_sel = v,
            0x030 => self.queue_sel = v,
            0x038 => {
                if let Some(vq) = self.queues.get_mut(self.queue_sel as usize) {
                    vq.num = v.min(VIRTIO_QUEUE_MAX);
                }
            }
            0x044 => {
                if let Some(vq) = self.queues.get_mut(self.queue_sel as usize) {
                    vq.ready = (v & 1) != 0;
                }
            }
            0x050 => {
                return Some(v as usize);
            }
            0x064 => {
                self.interrupt_status &= !v;
                if self.interrupt_status == 0 {
                    plic.set_source_level(self.irq, false);
                }
            }
            0x070 => {
                if v == 0 {
                    self.reset(plic);
                } else {
                    self.status = v;
                }
            }
            0x080 => {
                if let Some(vq) = self.queues.get_mut(self.queue_sel as usize) {
                    vq.desc_addr = (vq.desc_addr & !0xFFFF_FFFF) | u64::from(v);
                }
            }
            0x084 => {
                if let Some(vq) = self.queues.get_mut(self.queue_sel as usize) {
                    vq.desc_addr = (vq.desc_addr & 0xFFFF_FFFF) | (u64::from(v) << 32);
                }
            }
            0x090 => {
                if let Some(vq) = self.queues.get_mut(self.queue_sel as usize) {
                    vq.avail_addr = (vq.avail_addr & !0xFFFF_FFFF) | u64::from(v);
                }
            }
            0x094 => {
                if let Some(vq) = self.queues.get_mut(self.queue_sel as usize) {
                    vq.avail_addr = (vq.avail_addr & 0xFFFF_FFFF) | (u64::from(v) << 32);
                }
            }
            0x0A0 => {
                if let Some(vq) = self.queues.get_mut(self.queue_sel as usize) {
                    vq.used_addr = (vq.used_addr & !0xFFFF_FFFF) | u64::from(v);
                }
            }
            0x0A4 => {
                if let Some(vq) = self.queues.get_mut(self.queue_sel as usize) {
                    vq.used_addr = (vq.used_addr & 0xFFFF_FFFF) | (u64::from(v) << 32);
                }
            }
            _ => {}
        }
        None
    }
}

// ============================================================================
// 1. VirtioBlkMmio (DeviceID = 2)
// ============================================================================

const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const VIRTIO_BLK_T_FLUSH: u32 = 4;
const VIRTIO_BLK_T_GET_ID: u32 = 8;

const VIRTIO_BLK_S_OK: u8 = 0;
const VIRTIO_BLK_S_IOERR: u8 = 1;
const VIRTIO_BLK_S_UNSUPP: u8 = 2;

pub struct VirtioBlkMmio {
    pub mmio: VirtioMmioState,
    pub disk: SparseBlockDisk,
    pub read_ops: u64,
    pub write_ops: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
}

impl VirtioBlkMmio {
    pub fn new(irq: u32, disk: SparseBlockDisk) -> Self {
        let mut cfg = vec![0u8; 32];
        cfg[0..8].copy_from_slice(&disk.capacity_sectors().to_le_bytes());
        cfg[20..24].copy_from_slice(&512u32.to_le_bytes()); // blk_size = 512

        Self {
            mmio: VirtioMmioState::new(
                VIRTIO_DEV_BLK,
                irq,
                1,
                VIRTIO_BLK_F_BLK_SIZE | VIRTIO_BLK_F_FLUSH,
                cfg,
            ),
            disk,
            read_ops: 0,
            write_ops: 0,
            bytes_read: 0,
            bytes_written: 0,
        }
    }

    pub fn sync_capacity_config(&mut self) {
        self.mmio.config_space[0..8].copy_from_slice(&self.disk.capacity_sectors().to_le_bytes());
        self.mmio.config_generation = self.mmio.config_generation.wrapping_add(1);
    }

    fn process_queue(&mut self, ram: &mut GuestRam, plic: &mut PlicState) {
        let mut completed_any = false;
        while let Some((head, chain)) = pop_avail_chain(&mut self.mmio.queues[0], ram) {
            let written = self.handle_request(&chain, ram);
            if push_used_elem(&self.mmio.queues[0], ram, head, written) {
                completed_any = true;
            }
        }
        if completed_any {
            self.mmio.signal_used_buffer(plic);
        }
    }

    fn handle_request(&mut self, chain: &[VringDesc], ram: &mut GuestRam) -> u32 {
        if chain.len() < 2 {
            return 0;
        }
        // Gather readable bytes before writable descriptors, and locate final 1-byte status descriptor
        let last = match chain.last() {
            Some(d) if (d.flags & VRING_DESC_F_WRITE) != 0 && d.len >= 1 => d,
            _ => return 0,
        };
        let status_addr = last.addr + u64::from(last.len - 1);

        // Read 16-byte virtio_blk_outhdr from start of readable descriptors
        let mut req_hdr = [0u8; 16];
        let mut hdr_filled = 0usize;
        for d in chain {
            if (d.flags & VRING_DESC_F_WRITE) != 0 {
                break;
            }
            let take = (16 - hdr_filled).min(d.len as usize);
            if take > 0 {
                if ram
                    .read_bytes(d.addr, &mut req_hdr[hdr_filled..hdr_filled + take])
                    .is_err()
                {
                    let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_IOERR]);
                    return 1;
                }
                hdr_filled += take;
            }
            if hdr_filled == 16 {
                break;
            }
        }
        if hdr_filled < 16 {
            let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_IOERR]);
            return 1;
        }

        let req_type = u32::from_le_bytes([req_hdr[0], req_hdr[1], req_hdr[2], req_hdr[3]]);
        let sector = u64::from_le_bytes([
            req_hdr[8],
            req_hdr[9],
            req_hdr[10],
            req_hdr[11],
            req_hdr[12],
            req_hdr[13],
            req_hdr[14],
            req_hdr[15],
        ]);

        match req_type {
            VIRTIO_BLK_T_IN => {
                // Collect total writable data length (excluding the final 1-byte status byte)
                let mut total_writable = 0usize;
                for d in chain {
                    if (d.flags & VRING_DESC_F_WRITE) != 0 {
                        total_writable = total_writable.saturating_add(d.len as usize);
                    }
                }
                if total_writable < 1 {
                    return 0;
                }
                let data_len = total_writable - 1;
                let mut buf = vec![0u8; data_len];
                let Some(byte_off) = sector.checked_mul(SECTOR_SIZE as u64) else {
                    let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_IOERR]);
                    return 1;
                };
                if self.disk.read_at(byte_off, &mut buf).is_err() {
                    let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_IOERR]);
                    return 1;
                }
                let mut copied = 0usize;
                for (i, d) in chain.iter().enumerate() {
                    if (d.flags & VRING_DESC_F_WRITE) == 0 {
                        continue;
                    }
                    let usable = if i + 1 == chain.len() {
                        (d.len as usize).saturating_sub(1)
                    } else {
                        d.len as usize
                    };
                    if usable > 0 {
                        if ram
                            .write_bytes(d.addr, &buf[copied..copied + usable])
                            .is_err()
                        {
                            let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_IOERR]);
                            return 1;
                        }
                        copied += usable;
                    }
                }
                let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_OK]);
                self.read_ops += 1;
                self.bytes_read += data_len as u64;
                (data_len as u32) + 1
            }
            VIRTIO_BLK_T_OUT => {
                // Gather all readable bytes after the 16-byte header
                let mut skip = 16usize;
                let mut payload = Vec::new();
                for d in chain {
                    if (d.flags & VRING_DESC_F_WRITE) != 0 {
                        break;
                    }
                    let dlen = d.len as usize;
                    if skip >= dlen {
                        skip -= dlen;
                        continue;
                    }
                    let take = dlen - skip;
                    let start = payload.len();
                    payload.resize(start + take, 0);
                    if ram
                        .read_bytes(d.addr + (skip as u64), &mut payload[start..])
                        .is_err()
                    {
                        let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_IOERR]);
                        return 1;
                    }
                    skip = 0;
                }
                let Some(byte_off) = sector.checked_mul(SECTOR_SIZE as u64) else {
                    let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_IOERR]);
                    return 1;
                };
                match self.disk.write_at(byte_off, &payload) {
                    Ok(()) => {
                        let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_OK]);
                        self.write_ops += 1;
                        self.bytes_written += payload.len() as u64;
                    }
                    Err(_) => {
                        let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_IOERR]);
                    }
                }
                1
            }
            VIRTIO_BLK_T_FLUSH => {
                let _ = self.disk.flush();
                let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_OK]);
                1
            }
            VIRTIO_BLK_T_GET_ID => {
                let mut id_bytes = [0u8; 20];
                let name = b"wasmnux-vda";
                id_bytes[..name.len()].copy_from_slice(name);
                let mut written = 0u32;
                for (i, d) in chain.iter().enumerate() {
                    if (d.flags & VRING_DESC_F_WRITE) == 0 {
                        continue;
                    }
                    let usable = if i + 1 == chain.len() {
                        (d.len as usize).saturating_sub(1)
                    } else {
                        d.len as usize
                    };
                    let take = usable.min(20 - written as usize);
                    if take > 0 {
                        let _ = ram.write_bytes(
                            d.addr,
                            &id_bytes[written as usize..written as usize + take],
                        );
                        written += take as u32;
                    }
                }
                let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_OK]);
                written + 1
            }
            _ => {
                let _ = ram.write_bytes(status_addr, &[VIRTIO_BLK_S_UNSUPP]);
                1
            }
        }
    }
}

impl MmioDevice for VirtioBlkMmio {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn read(
        &mut self,
        offset: u64,
        size: usize,
        _ram: &mut GuestRam,
        _plic: &mut PlicState,
    ) -> Result<u64, Trap> {
        Ok(self.mmio.read_reg(offset, size))
    }

    fn write(
        &mut self,
        offset: u64,
        size: usize,
        value: u64,
        ram: &mut GuestRam,
        plic: &mut PlicState,
    ) -> Result<(), Trap> {
        if let Some(q_idx) = self.mmio.write_reg(offset, size, value, plic) {
            if q_idx == 0 {
                self.process_queue(ram, plic);
            }
        }
        Ok(())
    }
}

// ============================================================================
// 2. VirtioNetMmio (DeviceID = 1, Queue 0 = RX, Queue 1 = TX)
// ============================================================================

pub struct VirtioNetMmio {
    pub mmio: VirtioMmioState,
    pub switch: VirtualNetworkSwitch,
    pub tx_packets: u64,
    pub rx_packets: u64,
}

impl VirtioNetMmio {
    pub fn new(irq: u32, mac: [u8; 6]) -> Self {
        let mut cfg = vec![0u8; 16];
        cfg[0..6].copy_from_slice(&mac);
        cfg[6..8].copy_from_slice(&1u16.to_le_bytes()); // VIRTIO_NET_S_LINK_UP

        Self {
            mmio: VirtioMmioState::new(
                VIRTIO_DEV_NET,
                irq,
                2,
                VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS,
                cfg,
            ),
            switch: VirtualNetworkSwitch::new(),
            tx_packets: 0,
            rx_packets: 0,
        }
    }

    pub fn new_default(irq: u32) -> Self {
        Self::new(irq, GUEST_MAC)
    }

    pub fn drain_http_requests(&mut self) -> Vec<BridgeHttpRequest> {
        let mut out = Vec::new();
        while let Some(req) = self.switch.pop_http_request() {
            out.push(req);
        }
        out
    }

    pub fn deliver_http_response(
        &mut self,
        resp: BridgeHttpResponse,
        ram: &mut GuestRam,
        plic: &mut PlicState,
    ) {
        self.switch.complete_http_response(resp);
        self.flush_rx_to_guest(ram, plic);
    }

    fn process_tx(&mut self, ram: &mut GuestRam, plic: &mut PlicState) {
        let mut used_any = false;
        while let Some((head, chain)) = pop_avail_chain(&mut self.mmio.queues[1], ram) {
            let mut total = Vec::new();
            for d in &chain {
                if (d.flags & VRING_DESC_F_WRITE) != 0 {
                    continue;
                }
                let start = total.len();
                total.resize(start + (d.len as usize), 0);
                if ram.read_bytes(d.addr, &mut total[start..]).is_err() {
                    total.clear();
                    break;
                }
            }
            if total.len() > VIRTIO_NET_HDR_V1_LEN {
                self.switch
                    .receive_frame_from_guest(&total[VIRTIO_NET_HDR_V1_LEN..]);
                self.tx_packets += 1;
            }
            if push_used_elem(&self.mmio.queues[1], ram, head, 0) {
                used_any = true;
            }
        }
        if used_any {
            self.mmio.signal_used_buffer(plic);
        }
        self.flush_rx_to_guest(ram, plic);
    }

    pub fn flush_rx_to_guest(&mut self, ram: &mut GuestRam, plic: &mut PlicState) {
        let mut delivered_any = false;
        while self.switch.has_frames_for_guest() {
            let Some((head, chain)) = pop_avail_chain(&mut self.mmio.queues[0], ram) else {
                break;
            };
            let Some(eth_frame) = self.switch.pop_frame_for_guest() else {
                break;
            };
            let mut packet = Vec::with_capacity(VIRTIO_NET_HDR_V1_LEN + eth_frame.len());
            // 12-byte virtio_net_hdr_mrg_rxbuf: flags=0, gso_type=0, ..., num_buffers=1 at [10..12]
            let mut net_hdr = [0u8; VIRTIO_NET_HDR_V1_LEN];
            net_hdr[10..12].copy_from_slice(&1u16.to_le_bytes());
            packet.extend_from_slice(&net_hdr);
            packet.extend_from_slice(&eth_frame);

            let mut copied = 0usize;
            for d in &chain {
                if (d.flags & VRING_DESC_F_WRITE) == 0 {
                    continue;
                }
                let rem = packet.len().saturating_sub(copied);
                if rem == 0 {
                    break;
                }
                let take = rem.min(d.len as usize);
                if ram
                    .write_bytes(d.addr, &packet[copied..copied + take])
                    .is_err()
                {
                    break;
                }
                copied += take;
            }
            if push_used_elem(&self.mmio.queues[0], ram, head, copied as u32) {
                delivered_any = true;
                self.rx_packets += 1;
            }
        }
        if delivered_any {
            self.mmio.signal_used_buffer(plic);
        }
    }
}

impl MmioDevice for VirtioNetMmio {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn read(
        &mut self,
        offset: u64,
        size: usize,
        _ram: &mut GuestRam,
        _plic: &mut PlicState,
    ) -> Result<u64, Trap> {
        Ok(self.mmio.read_reg(offset, size))
    }

    fn write(
        &mut self,
        offset: u64,
        size: usize,
        value: u64,
        ram: &mut GuestRam,
        plic: &mut PlicState,
    ) -> Result<(), Trap> {
        if let Some(q_idx) = self.mmio.write_reg(offset, size, value, plic) {
            match q_idx {
                0 => self.flush_rx_to_guest(ram, plic),
                1 => self.process_tx(ram, plic),
                _ => {}
            }
        }
        Ok(())
    }
}

// ============================================================================
// 3. VirtioRngMmio (DeviceID = 4)
// ============================================================================

pub struct VirtioRngMmio {
    pub mmio: VirtioMmioState,
    state: [u64; 2],
}

impl VirtioRngMmio {
    pub fn new(irq: u32, seed: [u8; 16]) -> Self {
        let mut s0 = u64::from_le_bytes([
            seed[0], seed[1], seed[2], seed[3], seed[4], seed[5], seed[6], seed[7],
        ]);
        let mut s1 = u64::from_le_bytes([
            seed[8], seed[9], seed[10], seed[11], seed[12], seed[13], seed[14], seed[15],
        ]);
        if s0 == 0 && s1 == 0 {
            s0 = 0x9E37_79B9_7F4A_7C15;
            s1 = 0xD1B5_4A32_D192_ED03;
        }
        Self {
            mmio: VirtioMmioState::new(VIRTIO_DEV_RNG, irq, 1, 0, Vec::new()),
            state: [s0, s1],
        }
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        // xoroshiro128++
        let s0 = self.state[0];
        let mut s1 = self.state[1];
        let result = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
        s1 ^= s0;
        self.state[0] = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
        self.state[1] = s1.rotate_left(28);
        result
    }

    fn fill_entropy(&mut self, dst: &mut [u8]) {
        let mut i = 0;
        while i < dst.len() {
            let bytes = self.next_u64().to_le_bytes();
            let take = (dst.len() - i).min(8);
            dst[i..i + take].copy_from_slice(&bytes[..take]);
            i += take;
        }
    }

    fn process_queue(&mut self, ram: &mut GuestRam, plic: &mut PlicState) {
        let mut completed = false;
        while let Some((head, chain)) = pop_avail_chain(&mut self.mmio.queues[0], ram) {
            let mut total_written = 0u32;
            for d in &chain {
                if (d.flags & VRING_DESC_F_WRITE) == 0 || d.len == 0 {
                    continue;
                }
                let len = (d.len as usize).min(4096);
                let mut buf = vec![0u8; len];
                self.fill_entropy(&mut buf);
                if ram.write_bytes(d.addr, &buf).is_ok() {
                    total_written += len as u32;
                }
            }
            if push_used_elem(&self.mmio.queues[0], ram, head, total_written) {
                completed = true;
            }
        }
        if completed {
            self.mmio.signal_used_buffer(plic);
        }
    }
}

impl MmioDevice for VirtioRngMmio {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn read(
        &mut self,
        offset: u64,
        size: usize,
        _ram: &mut GuestRam,
        _plic: &mut PlicState,
    ) -> Result<u64, Trap> {
        Ok(self.mmio.read_reg(offset, size))
    }

    fn write(
        &mut self,
        offset: u64,
        size: usize,
        value: u64,
        ram: &mut GuestRam,
        plic: &mut PlicState,
    ) -> Result<(), Trap> {
        if let Some(q_idx) = self.mmio.write_reg(offset, size, value, plic) {
            if q_idx == 0 {
                self.process_queue(ram, plic);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmnux_riscv::trap::AccessType;
    use wasmnux_riscv::GUEST_RAM_BASE;

    fn setup_queue(
        dev: &mut dyn MmioDevice,
        ram: &mut GuestRam,
        plic: &mut PlicState,
        q_idx: u64,
        desc_pa: u64,
        avail_pa: u64,
        used_pa: u64,
    ) {
        dev.write(0x030, 4, q_idx, ram, plic).unwrap(); // QueueSel
        dev.write(0x038, 4, 16, ram, plic).unwrap(); // QueueNum = 16
        dev.write(0x080, 4, desc_pa & 0xFFFF_FFFF, ram, plic)
            .unwrap();
        dev.write(0x084, 4, desc_pa >> 32, ram, plic).unwrap();
        dev.write(0x090, 4, avail_pa & 0xFFFF_FFFF, ram, plic)
            .unwrap();
        dev.write(0x094, 4, avail_pa >> 32, ram, plic).unwrap();
        dev.write(0x0A0, 4, used_pa & 0xFFFF_FFFF, ram, plic)
            .unwrap();
        dev.write(0x0A4, 4, used_pa >> 32, ram, plic).unwrap();
        dev.write(0x044, 4, 1, ram, plic).unwrap(); // QueueReady = 1
    }

    fn write_desc(
        ram: &mut GuestRam,
        desc_pa: u64,
        idx: u16,
        addr: u64,
        len: u32,
        flags: u16,
        next: u16,
    ) {
        let mut raw = [0u8; 16];
        raw[0..8].copy_from_slice(&addr.to_le_bytes());
        raw[8..12].copy_from_slice(&len.to_le_bytes());
        raw[12..14].copy_from_slice(&flags.to_le_bytes());
        raw[14..16].copy_from_slice(&next.to_le_bytes());
        ram.write_bytes(desc_pa + u64::from(idx) * 16, &raw)
            .unwrap();
    }

    #[test]
    fn virtio_blk_mmio_write_and_read_roundtrip_with_irq() {
        let disk = SparseBlockDisk::new(8 * 1024 * 1024).unwrap();
        let mut blk = VirtioBlkMmio::new(1, disk);
        let mut ram = GuestRam::new(4 * 1024 * 1024);
        let mut plic = PlicState::new();
        // Enable IRQ 1 in PLIC S-mode context 1 with priority 2
        plic.write(0x0000_0004, 4, 2);
        plic.write(0x0000_2080, 4, 1 << 1);

        assert_eq!(
            blk.read(0x000, 4, &mut ram, &mut plic).unwrap(),
            VIRTIO_MMIO_MAGIC as u64
        );
        assert_eq!(blk.read(0x004, 4, &mut ram, &mut plic).unwrap(), 2);
        blk.write(0x014, 4, 1, &mut ram, &mut plic).unwrap();
        assert_ne!(blk.read(0x010, 4, &mut ram, &mut plic).unwrap() & 1, 0); // VIRTIO_F_VERSION_1

        let desc_pa = GUEST_RAM_BASE + 0x1000;
        let avail_pa = GUEST_RAM_BASE + 0x2000;
        let used_pa = GUEST_RAM_BASE + 0x3000;
        setup_queue(&mut blk, &mut ram, &mut plic, 0, desc_pa, avail_pa, used_pa);

        // 1. Write 512 bytes to sector 7 via 3-descriptor chain (hdr, data, status)
        let hdr_pa = GUEST_RAM_BASE + 0x4000;
        let data_pa = GUEST_RAM_BASE + 0x5000;
        let status_pa = GUEST_RAM_BASE + 0x6000;

        let mut out_hdr = [0u8; 16];
        out_hdr[0..4].copy_from_slice(&VIRTIO_BLK_T_OUT.to_le_bytes());
        out_hdr[8..16].copy_from_slice(&7u64.to_le_bytes());
        ram.write_bytes(hdr_pa, &out_hdr).unwrap();

        let mut sector_payload = [0u8; 512];
        for (i, b) in sector_payload.iter_mut().enumerate() {
            *b = ((i * 5 + 11) & 0xFF) as u8;
        }
        ram.write_bytes(data_pa, &sector_payload).unwrap();
        ram.write_bytes(status_pa, &[0xFF]).unwrap();

        write_desc(&mut ram, desc_pa, 0, hdr_pa, 16, VRING_DESC_F_NEXT, 1);
        write_desc(&mut ram, desc_pa, 1, data_pa, 512, VRING_DESC_F_NEXT, 2);
        write_desc(&mut ram, desc_pa, 2, status_pa, 1, VRING_DESC_F_WRITE, 0);

        ram.write_scalar(avail_pa + 2, 2, 1, AccessType::Store)
            .unwrap(); // avail.idx = 1
        ram.write_scalar(avail_pa + 4, 2, 0, AccessType::Store)
            .unwrap(); // ring[0] = head 0
        blk.write(0x050, 4, 0, &mut ram, &mut plic).unwrap(); // QueueNotify(0)

        assert_eq!(ram.read_scalar(status_pa, 1, AccessType::Load).unwrap(), 0); // VIRTIO_BLK_S_OK
        assert!(plic.supervisor_external_pending());
        // Ack interrupt
        blk.write(0x064, 4, VIRTIO_MMIO_INT_VRING as u64, &mut ram, &mut plic)
            .unwrap();

        // 2. Read sector 7 back into a fresh buffer at 0x7000
        let read_dst_pa = GUEST_RAM_BASE + 0x7000;
        out_hdr[0..4].copy_from_slice(&VIRTIO_BLK_T_IN.to_le_bytes());
        ram.write_bytes(hdr_pa, &out_hdr).unwrap();
        ram.write_bytes(status_pa, &[0xFF]).unwrap();

        write_desc(&mut ram, desc_pa, 3, hdr_pa, 16, VRING_DESC_F_NEXT, 4);
        write_desc(
            &mut ram,
            desc_pa,
            4,
            read_dst_pa,
            512,
            VRING_DESC_F_WRITE | VRING_DESC_F_NEXT,
            5,
        );
        write_desc(&mut ram, desc_pa, 5, status_pa, 1, VRING_DESC_F_WRITE, 0);

        ram.write_scalar(avail_pa + 2, 2, 2, AccessType::Store)
            .unwrap(); // avail.idx = 2
        ram.write_scalar(avail_pa + 6, 2, 3, AccessType::Store)
            .unwrap(); // ring[1] = head 3
        blk.write(0x050, 4, 0, &mut ram, &mut plic).unwrap();

        assert_eq!(ram.read_scalar(status_pa, 1, AccessType::Load).unwrap(), 0);
        let mut read_back = [0u8; 512];
        ram.read_bytes(read_dst_pa, &mut read_back).unwrap();
        assert_eq!(read_back, sector_payload);
    }

    #[test]
    fn virtio_net_mmio_arp_roundtrip_and_virtio_rng_entropy() {
        let mut net = VirtioNetMmio::new_default(2);
        let mut ram = GuestRam::new(4 * 1024 * 1024);
        let mut plic = PlicState::new();

        // Verify byte-by-byte MAC config reads (as Linux virtio_cread_bytes performs)
        for (i, &expected) in GUEST_MAC.iter().enumerate() {
            assert_eq!(
                net.read(0x100 + (i as u64), 1, &mut ram, &mut plic)
                    .unwrap() as u8,
                expected
            );
        }

        // Queue 0 = RX, Queue 1 = TX
        let rx_desc = GUEST_RAM_BASE + 0x1000;
        let rx_avail = GUEST_RAM_BASE + 0x2000;
        let rx_used = GUEST_RAM_BASE + 0x3000;
        let tx_desc = GUEST_RAM_BASE + 0x4000;
        let tx_avail = GUEST_RAM_BASE + 0x5000;
        let tx_used = GUEST_RAM_BASE + 0x6000;

        setup_queue(&mut net, &mut ram, &mut plic, 0, rx_desc, rx_avail, rx_used);
        setup_queue(&mut net, &mut ram, &mut plic, 1, tx_desc, tx_avail, tx_used);

        // Post an RX buffer at 0x8000
        let rx_buf = GUEST_RAM_BASE + 0x8000;
        write_desc(&mut ram, rx_desc, 0, rx_buf, 2048, VRING_DESC_F_WRITE, 0);
        ram.write_scalar(rx_avail + 2, 2, 1, AccessType::Store)
            .unwrap();
        ram.write_scalar(rx_avail + 4, 2, 0, AccessType::Store)
            .unwrap();

        // Build 12-byte virtio_net_hdr_v1 + 42-byte ARP request for 10.0.2.2 in TX buffer at 0x9000
        let tx_buf = GUEST_RAM_BASE + 0x9000;
        let mut tx_pkt = vec![0u8; VIRTIO_NET_HDR_V1_LEN + 42];
        let arp = &mut tx_pkt[VIRTIO_NET_HDR_V1_LEN..];
        arp[0..6].copy_from_slice(&[0xFF; 6]);
        arp[6..12].copy_from_slice(&GUEST_MAC);
        arp[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
        arp[14..16].copy_from_slice(&1u16.to_be_bytes());
        arp[16..18].copy_from_slice(&0x0800u16.to_be_bytes());
        arp[18] = 6;
        arp[19] = 4;
        arp[20..22].copy_from_slice(&1u16.to_be_bytes());
        arp[22..28].copy_from_slice(&GUEST_MAC);
        arp[28..32].copy_from_slice(&[10, 0, 2, 15]);
        arp[38..42].copy_from_slice(&[10, 0, 2, 2]);
        ram.write_bytes(tx_buf, &tx_pkt).unwrap();

        write_desc(&mut ram, tx_desc, 0, tx_buf, tx_pkt.len() as u32, 0, 0);
        ram.write_scalar(tx_avail + 2, 2, 1, AccessType::Store)
            .unwrap();
        ram.write_scalar(tx_avail + 4, 2, 0, AccessType::Store)
            .unwrap();
        net.write(0x050, 4, 1, &mut ram, &mut plic).unwrap(); // QueueNotify(TX=1)

        assert_eq!(net.tx_packets, 1);
        assert_eq!(net.rx_packets, 1);
        let rx_used_len = ram.read_scalar(rx_used + 8, 4, AccessType::Load).unwrap();
        assert_eq!(rx_used_len, (VIRTIO_NET_HDR_V1_LEN + 42) as u64);
        // Check num_buffers == 1 at offset 10..12 and ARP opcode == 2 (Reply) at offset 12 + 20..22
        assert_eq!(
            ram.read_scalar(rx_buf + 10, 2, AccessType::Load).unwrap(),
            1
        );
        assert_eq!(
            u16::from_be(
                ram.read_scalar(rx_buf + 12 + 20, 2, AccessType::Load)
                    .unwrap() as u16
            ),
            2
        );

        // Also verify VirtioRngMmio fills non-zero entropy
        let mut rng = VirtioRngMmio::new(3, [0x42; 16]);
        let rng_desc = GUEST_RAM_BASE + 0xA000;
        let rng_avail = GUEST_RAM_BASE + 0xB000;
        let rng_used = GUEST_RAM_BASE + 0xC000;
        let rng_dst = GUEST_RAM_BASE + 0xD000;
        setup_queue(
            &mut rng, &mut ram, &mut plic, 0, rng_desc, rng_avail, rng_used,
        );
        write_desc(&mut ram, rng_desc, 0, rng_dst, 64, VRING_DESC_F_WRITE, 0);
        ram.write_scalar(rng_avail + 2, 2, 1, AccessType::Store)
            .unwrap();
        ram.write_scalar(rng_avail + 4, 2, 0, AccessType::Store)
            .unwrap();
        rng.write(0x050, 4, 0, &mut ram, &mut plic).unwrap();

        let mut entropy = [0u8; 64];
        ram.read_bytes(rng_dst, &mut entropy).unwrap();
        assert!(entropy.iter().any(|&b| b != 0));
    }
}
