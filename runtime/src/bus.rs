//! Memory bus: dispatches guest physical accesses to RAM, the CLINT timer,
//! and the UART. Anything not claimed by a device falls back to RAM.

use crate::csr::CsrFile;
use crate::memory::GuestMemory;
use crate::memory::MemoryError;
use crate::timer::Clint;
use crate::uart::Uart;

pub struct Bus {
    pub mem: GuestMemory,
    pub clint: Clint,
    pub uart: Uart,
}

impl Bus {
    pub fn new(mem_size: usize, harts: usize) -> Self {
        Self {
            mem: GuestMemory::new(mem_size),
            clint: Clint::new(harts),
            uart: Uart::new(),
        }
    }

    /// Read 8, 16, 32, or 64 little-endian bytes from `addr`. Devices are
    /// only accessed with naturally-aligned, single-register accesses.
    pub fn read_u8(&self, addr: u64) -> Result<u8, MemoryError> {
        if let Some(v) = self.uart.read_byte(addr) {
            return Ok(v);
        }
        self.mem.read_u8(addr)
    }

    pub fn read_u16(&self, addr: u64) -> Result<u16, MemoryError> {
        self.mem.read_u16(addr)
    }

    pub fn read_u32(&self, addr: u64) -> Result<u32, MemoryError> {
        // CLINT mtimecmp / mtime reads are 64-bit; 32-bit reads fall through to RAM.
        self.mem.read_u32(addr)
    }

    pub fn read_u64(&self, addr: u64) -> Result<u64, MemoryError> {
        if let Some(v) = self.clint.read(addr) {
            return Ok(v);
        }
        self.mem.read_u64(addr)
    }

    pub fn write_u8(&mut self, addr: u64, value: u8) -> Result<(), MemoryError> {
        if self.uart.write_byte(addr, value) {
            return Ok(());
        }
        self.mem.write_u8(addr, value)
    }

    pub fn write_u16(&mut self, addr: u64, value: u16) -> Result<(), MemoryError> {
        self.mem.write_u16(addr, value)
    }

    pub fn write_u32(&mut self, addr: u64, value: u32) -> Result<(), MemoryError> {
        self.mem.write_u32(addr, value)
    }

    pub fn write_u64(
        &mut self,
        addr: u64,
        value: u64,
        csr: &mut CsrFile,
    ) -> Result<(), MemoryError> {
        if self.clint.write(addr, value, csr) {
            return Ok(());
        }
        self.mem.write_u64(addr, value)
    }

    /// Advance all time-based devices (CLINT) by `delta` ticks.
    pub fn tick(&mut self, delta: u64, csr: &mut CsrFile) {
        self.clint.tick(delta, csr);
    }

    /// Drain UART output (returns bytes since last drain).
    pub fn drain_uart(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.uart.output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csr::CsrFile;

    #[test]
    fn uart_write_through_bus() {
        let mut bus = Bus::new(0x10000, 1);
        bus.write_u8(crate::uart::UART_BASE + crate::uart::UART_THR, b'X')
            .unwrap();
        let out = bus.drain_uart();
        assert_eq!(out, vec![b'X']);
    }

    #[test]
    fn clint_read_through_bus() {
        let mut bus = Bus::new(0x10000, 1);
        bus.clint.mtime = 12345;
        assert_eq!(
            bus.read_u64(crate::timer::CLINT_BASE + crate::timer::CLINT_MTIME_OFFSET)
                .unwrap(),
            12345
        );
    }

    #[test]
    fn clint_write_sets_compare_through_bus() {
        let mut bus = Bus::new(0x10000, 1);
        let mut csr = CsrFile::new();
        bus.write_u64(
            crate::timer::CLINT_BASE + crate::timer::CLINT_MTIMECMP_BASE,
            7,
            &mut csr,
        )
        .unwrap();
        assert_eq!(bus.clint.mtimecmp[0], 7);
    }

    #[test]
    fn ram_still_works_past_devices() {
        let mut bus = Bus::new(0x10000, 1);
        bus.write_u32(0x1000, 0xDEADBEEF).unwrap();
        assert_eq!(bus.read_u32(0x1000).unwrap(), 0xDEADBEEF);
    }
}
