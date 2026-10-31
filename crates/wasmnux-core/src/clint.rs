//! Core Local Interruptor (SiFive-style CLINT) for a single hart: `msip`,
//! `mtimecmp` and `mtime`. The host advances `mtime` as the CPU executes so
//! timer behaviour is deterministic in tests and bounded in the browser.

use crate::bus::MmioDevice;
use crate::error::{Result, VmError};

const MSIP_OFFSET: u64 = 0x0000;
const MTIMECMP_OFFSET: u64 = 0x4000;
const MTIME_OFFSET: u64 = 0xBFF8;

/// Advertised `mtime` frequency (DTB `timebase-frequency`).
pub const TIMEBASE_FREQUENCY: u32 = 10_000_000;

/// Single-hart CLINT.
#[derive(Debug, Default)]
pub struct Clint {
    msip: u32,
    mtimecmp: u64,
    mtime: u64,
}

impl Clint {
    pub fn new() -> Self {
        Self {
            msip: 0,
            mtimecmp: u64::MAX,
            mtime: 0,
        }
    }

    /// Advances machine time by `ticks`.
    pub fn advance(&mut self, ticks: u64) {
        self.mtime = self.mtime.wrapping_add(ticks);
    }

    pub fn mtime(&self) -> u64 {
        self.mtime
    }

    pub fn mtimecmp(&self) -> u64 {
        self.mtimecmp
    }

    /// Programs `mtimecmp` (SBI `set_timer` path).
    pub fn set_mtimecmp(&mut self, value: u64) {
        self.mtimecmp = value;
    }

    /// Machine timer interrupt pending (level: `mtime >= mtimecmp`).
    pub fn timer_pending(&self) -> bool {
        self.mtime >= self.mtimecmp
    }

    /// Machine software interrupt pending.
    pub fn software_pending(&self) -> bool {
        self.msip & 1 != 0
    }
}

impl MmioDevice for Clint {
    fn read(&mut self, offset: u64, size: usize) -> Result<u64> {
        let value = match (offset, size) {
            (MSIP_OFFSET, 4) => u64::from(self.msip),
            (MTIMECMP_OFFSET, 4) => self.mtimecmp & 0xFFFF_FFFF,
            (o, 4) if o == MTIMECMP_OFFSET + 4 => self.mtimecmp >> 32,
            (MTIMECMP_OFFSET, 8) => self.mtimecmp,
            (MTIME_OFFSET, 4) => self.mtime & 0xFFFF_FFFF,
            (o, 4) if o == MTIME_OFFSET + 4 => self.mtime >> 32,
            (MTIME_OFFSET, 8) => self.mtime,
            _ => return Err(VmError::UnsupportedAccessSize { size }),
        };
        Ok(value)
    }

    fn write(&mut self, offset: u64, size: usize, value: u64) -> Result<()> {
        match (offset, size) {
            (MSIP_OFFSET, 4) => self.msip = (value & 1) as u32,
            (MTIMECMP_OFFSET, 4) => {
                self.mtimecmp = (self.mtimecmp & !0xFFFF_FFFF) | (value & 0xFFFF_FFFF);
            }
            (o, 4) if o == MTIMECMP_OFFSET + 4 => {
                self.mtimecmp = (self.mtimecmp & 0xFFFF_FFFF) | (value << 32);
            }
            (MTIMECMP_OFFSET, 8) => self.mtimecmp = value,
            (MTIME_OFFSET, 4) => {
                self.mtime = (self.mtime & !0xFFFF_FFFF) | (value & 0xFFFF_FFFF);
            }
            (o, 4) if o == MTIME_OFFSET + 4 => {
                self.mtime = (self.mtime & 0xFFFF_FFFF) | (value << 32);
            }
            (MTIME_OFFSET, 8) => self.mtime = value,
            _ => return Err(VmError::UnsupportedAccessSize { size }),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Clint;
    use crate::bus::MmioDevice;

    #[test]
    fn timer_fires_at_mtimecmp() {
        let mut clint = Clint::new();
        assert!(!clint.timer_pending());
        clint.write(0x4000, 8, 100).expect("set mtimecmp");
        clint.advance(99);
        assert!(!clint.timer_pending());
        clint.advance(1);
        assert!(clint.timer_pending());
        assert_eq!(clint.read(0xBFF8, 8).expect("mtime"), 100);
    }

    #[test]
    fn msip_and_halfword_accesses() {
        let mut clint = Clint::new();
        clint.write(0, 4, 1).expect("msip");
        assert!(clint.software_pending());
        clint.write(0x4000, 4, 0xDEAD_BEEF).expect("cmp lo");
        clint.write(0x4004, 4, 0x1234).expect("cmp hi");
        assert_eq!(clint.mtimecmp(), 0x0000_1234_DEAD_BEEF);
        assert!(clint.read(0x4000, 2).is_err());
    }
}
