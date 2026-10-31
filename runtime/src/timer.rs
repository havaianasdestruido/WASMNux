//! CLINT (Core Local Interruptor) timer.
//!
//! Memory-mapped timer:
//!   mtime        @ 0x0200BFF8  (64-bit, read-only, time base)
//!   mtimecmp[N]  @ 0x02004000 + 8*N  (64-bit compare; mtime>=mtimecmp => MTIP)

use crate::csr::{CsrFile, IRQ_M_TIMER};

/// CLINT base address (qemu "virt" map).
pub const CLINT_BASE: u64 = 0x0200_0000;
/// mtime register offset (within a 0x4000-sized mtime block).
pub const CLINT_MTIME_OFFSET: u64 = 0xBFF8;
/// mtimecmp base offset (per hart: 8 bytes each).
pub const CLINT_MTIMECMP_BASE: u64 = 0x4000;

pub struct Clint {
    /// Wall-clock time base, increments each tick.
    pub mtime: u64,
    /// One compare value per hart.
    pub mtimecmp: Vec<u64>,
    /// Number of harts.
    pub harts: usize,
}

impl Clint {
    pub fn new(harts: usize) -> Self {
        Self {
            mtime: 0,
            mtimecmp: vec![u64::MAX; harts],
            harts,
        }
    }

    /// Advance time by `delta` ticks, then update MTIP pending bits in `csr`.
    pub fn tick(&mut self, delta: u64, csr: &mut CsrFile) {
        self.mtime = self.mtime.saturating_add(delta);
        for cmp in self.mtimecmp.iter_mut() {
            if *cmp != u64::MAX && self.mtime >= *cmp {
                csr.raise_interrupt(IRQ_M_TIMER);
                // leave pending; software clears it by reprogramming mtimecmp.
            }
        }
    }

    /// Machine-mode address decode + read (8-byte little-endian).
    pub fn read(&self, addr: u64) -> Option<u64> {
        if addr < CLINT_BASE {
            return None;
        }
        let off = addr - CLINT_BASE;
        // mtime block: 0xBFF8 .. 0xC000 (8 bytes)
        if (CLINT_MTIME_OFFSET..CLINT_MTIME_OFFSET + 8).contains(&off) {
            return Some(self.mtime);
        }
        // mtimecmp blocks
        if (CLINT_MTIMECMP_BASE..CLINT_MTIMECMP_BASE + (self.harts as u64 * 8)).contains(&off) {
            let hart = ((off - CLINT_MTIMECMP_BASE) / 8) as usize;
            if hart < self.harts {
                return Some(self.mtimecmp[hart]);
            }
        }
        None
    }

    /// Machine-mode address decode + write (8-byte little-endian).
    pub fn write(&mut self, addr: u64, value: u64, csr: &mut CsrFile) -> bool {
        if addr < CLINT_BASE {
            return false;
        }
        let off = addr - CLINT_BASE;
        if (CLINT_MTIME_OFFSET..CLINT_MTIME_OFFSET + 8).contains(&off) {
            // mtime is writable only for test/set; ignore writes here.
            return true;
        }
        if (CLINT_MTIMECMP_BASE..CLINT_MTIMECMP_BASE + (self.harts as u64 * 8)).contains(&off) {
            let hart = ((off - CLINT_MTIMECMP_BASE) / 8) as usize;
            if hart < self.harts {
                self.mtimecmp[hart] = value;
                // If time already past the compare, raise timer IRQ.
                if self.mtime >= value {
                    csr.raise_interrupt(IRQ_M_TIMER);
                } else {
                    csr.clear_interrupt(IRQ_M_TIMER);
                }
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csr::CsrFile;

    #[test]
    fn timer_compare_raises_interrupt() {
        let mut clint = Clint::new(1);
        let mut csr = CsrFile::new();
        csr.mie = crate::csr::MIP_MTIP;
        csr.mstatus |= crate::csr::MSTATUS_MIE;
        clint.mtimecmp[0] = 100;
        clint.mtime = 99;
        clint.tick(1, &mut csr); // mtime -> 100
        assert!(csr.pending_interrupt() == Some(IRQ_M_TIMER));
    }

    #[test]
    fn write_mtimecmp_sets_compare() {
        let mut clint = Clint::new(1);
        let mut csr = CsrFile::new();
        assert!(clint.write(CLINT_BASE + CLINT_MTIMECMP_BASE, 50, &mut csr));
        assert_eq!(clint.mtimecmp[0], 50);
    }

    #[test]
    fn address_outside_clint_rejected() {
        let mut clint = Clint::new(1);
        assert!(clint.read(0).is_none());
        assert!(!clint.write(0, 0, &mut CsrFile::new()));
    }
}
