use super::*;
use crate::dev::Plic;
use crate::machine::Entropy;
use crate::memory::Memory;
use crate::snapshot::{StateReader, StateWriter};
use std::sync::{Arc, Mutex};
pub struct VirtioEntropy {
    source: Arc<Mutex<dyn Entropy>>,
}
impl VirtioEntropy {
    pub fn create(
        start: u64,
        plic: Plic,
        irq: u32,
        mem: Arc<Memory>,
        source: Arc<Mutex<dyn Entropy>>,
    ) -> VirtioMmio {
        let st = VirtioState::new("virtio-rng", 4, 0, start, 4096, plic, irq, mem, 1);
        VirtioMmio::new("virtio", VirtioDevice::new(st, Box::new(Self { source })))
    }
}
impl Handler for VirtioEntropy {
    fn save_state(&self, _st: &VirtioState, _w: &mut StateWriter) -> Result<(), String> {
        Ok(())
    }
    fn load_state(&mut self, _st: &mut VirtioState, _r: &mut StateReader) -> Result<(), String> {
        Ok(())
    }

    fn execute_descriptor_chain(
        &mut self,
        st: &mut VirtioState,
        _q: usize,
        mut idx: u16,
    ) -> Result<u32, String> {
        let mut total = 0;
        for _ in 0..128 {
            let d = st.guest.load_desc(&st.queues[0], idx)?;
            if d.len > 65536 {
                return Err("entropy request too large".into());
            }
            if d.flags & VIRTQ_DESC_F_WRITE != 0 {
                let mut b = vec![0; d.len as usize];
                self.source
                    .lock()
                    .map_err(|_| "entropy lock poisoned")?
                    .fill(&mut b)?;
                st.guest.write(d.addr, &b)?;
                total += d.len;
            }
            if d.flags & VIRTQ_DESC_F_NEXT == 0 {
                return Ok(total);
            }
            idx = d.next;
            if idx as usize >= st.queues[0].num as usize {
                return Err("invalid entropy descriptor".into());
            }
        }
        Err("entropy chain too long".into())
    }
}
