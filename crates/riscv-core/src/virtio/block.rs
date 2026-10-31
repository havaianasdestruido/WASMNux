//! Synchronous VirtIO block. Flush is acknowledged only after backend success.
use super::*;
use crate::dev::Plic;
use crate::machine::BlockDevice;
use crate::memory::Memory;
use crate::snapshot::{StateReader, StateWriter};
use std::sync::{Arc, Mutex};
struct Disk {
    backend: Arc<Mutex<dyn BlockDevice>>,
    sectors: u64,
    readonly: bool,
}
pub fn create(
    start: u64,
    plic: Plic,
    irq: u32,
    mem: Arc<Memory>,
    backend: Arc<Mutex<dyn BlockDevice>>,
    sectors: u64,
    readonly: bool,
) -> VirtioMmio {
    let st = VirtioState::new(
        "virtio-blk",
        2,
        (1 << 2) | (1 << 6) | (1 << 9) | if readonly { 1 << 5 } else { 0 },
        start,
        4096,
        plic,
        irq,
        mem,
        1,
    );
    VirtioMmio::new(
        "virtio",
        VirtioDevice::new(
            st,
            Box::new(Disk {
                backend,
                sectors,
                readonly,
            }),
        ),
    )
}
impl Handler for Disk {
    fn save_state(&self, _st: &VirtioState, _w: &mut StateWriter) -> Result<(), String> {
        Ok(())
    }
    fn load_state(&mut self, _st: &mut VirtioState, _r: &mut StateReader) -> Result<(), String> {
        Ok(())
    }
    fn config_load(&mut self, _: &mut VirtioState, offset: u64, size: u64) -> Option<u64> {
        if size != 4 {
            return None;
        }
        match offset {
            0x100 => Some(self.sectors as u32 as u64),
            0x104 => Some(self.sectors >> 32),
            0x108 => Some(16 * 1024 * 1024),
            0x10c => Some(126),
            0x114 => Some(512),
            _ => None,
        }
    }
    fn execute_descriptor_chain(
        &mut self,
        st: &mut VirtioState,
        _q: usize,
        mut idx: u16,
    ) -> Result<u32, String> {
        let mut desc = st.guest.load_desc(&st.queues[0], idx)?;
        if desc.len != 16
            || desc.flags & VIRTQ_DESC_F_WRITE != 0
            || desc.flags & VIRTQ_DESC_F_NEXT == 0
        {
            return Err("invalid block header".into());
        }
        let typ = st.guest.read_u32(desc.addr)?;
        let sector = st.guest.read_u64(desc.addr + 8)?;
        if typ != 0 && typ != 1 && typ != 4 {
            return Err("unsupported block request".into());
        }
        let mut transferred = 0u32;
        let mut status = 0u8;
        let mut status_seen = false;
        for count in 0..128 {
            if desc.flags & VIRTQ_DESC_F_NEXT == 0 {
                return Err("missing block status".into());
            }
            idx = desc.next;
            if idx as usize >= st.queues[0].num as usize {
                return Err("invalid descriptor index".into());
            }
            desc = st.guest.load_desc(&st.queues[0], idx)?;
            if desc.flags & VIRTQ_DESC_F_NEXT == 0 {
                if desc.len != 1 || desc.flags & VIRTQ_DESC_F_WRITE == 0 {
                    return Err("invalid block status".into());
                }
                status_seen = true;
                break;
            }
            if count == 127 {
                return Err("block descriptor chain too long".into());
            }
            if desc.len == 0 || desc.len > 16 * 1024 * 1024 {
                return Err(format!("invalid block data length {}", desc.len));
            }
            let bytes = desc.len as u64;
            let offset = sector
                .checked_mul(512)
                .and_then(|n| n.checked_add(transferred as u64))
                .ok_or("block offset overflow")?;
            if bytes
                > self
                    .sectors
                    .checked_mul(512)
                    .and_then(|n| n.checked_sub(offset))
                    .ok_or("block request out of bounds")?
            {
                while desc.flags & VIRTQ_DESC_F_NEXT != 0 {
                    idx = desc.next;
                    if idx as usize >= st.queues[0].num as usize {
                        return Err("invalid status descriptor".into());
                    }
                    desc = st.guest.load_desc(&st.queues[0], idx)?;
                }
                if desc.len != 1 || desc.flags & VIRTQ_DESC_F_WRITE == 0 {
                    return Err("invalid status descriptor".into());
                }
                st.guest.write_u8(desc.addr, 1)?;
                return Ok(transferred);
            }
            if typ == 0 {
                if desc.flags & VIRTQ_DESC_F_WRITE == 0 {
                    return Err("read buffer not writable".into());
                }
                let mut buf = vec![0; desc.len as usize];
                if self
                    .backend
                    .lock()
                    .map_err(|_| "disk lock poisoned")?
                    .read_at(offset, &mut buf)
                    .is_err()
                {
                    status = 1
                }
                st.guest.write(desc.addr, &buf)?;
            } else if typ == 1 {
                if self.readonly || desc.flags & VIRTQ_DESC_F_WRITE != 0 {
                    status = if self.readonly { 2 } else { 1 }
                } else {
                    let mut buf = vec![0; desc.len as usize];
                    st.guest.read(desc.addr, &mut buf)?;
                    if self
                        .backend
                        .lock()
                        .map_err(|_| "disk lock poisoned")?
                        .write_at(offset, &buf)
                        .is_err()
                    {
                        status = 1
                    }
                }
            } else {
                return Err("flush must not have a data descriptor".into());
            }
            transferred += desc.len;
        }
        if !status_seen {
            return Err("missing block status".into());
        }
        if typ == 4
            && self
                .backend
                .lock()
                .map_err(|_| "disk lock poisoned")?
                .flush()
                .is_err()
        {
            status = 1
        }
        st.guest.write_u8(desc.addr, status)?;
        Ok(transferred)
    }
}
