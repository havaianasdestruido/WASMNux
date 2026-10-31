//! Guest physical RAM: a single contiguous region with bounds-checked,
//! little-endian accessors. All guest-visible addresses are validated here
//! so a misbehaving guest can never address host memory.

use crate::error::{Result, VmError};

/// Contiguous guest RAM starting at a fixed physical base address.
#[derive(Debug)]
pub struct Ram {
    base: u64,
    data: Vec<u8>,
}

impl Ram {
    /// Creates zeroed RAM of `size` bytes mapped at `base`.
    ///
    /// # Errors
    /// Returns [`VmError::InvalidConfig`] for a zero or overflowing size.
    pub fn new(base: u64, size: usize) -> Result<Self> {
        if size == 0 {
            return Err(VmError::InvalidConfig("RAM size must be non-zero".into()));
        }
        if base.checked_add(size as u64).is_none() {
            return Err(VmError::InvalidConfig("RAM region overflows u64".into()));
        }
        Ok(Self {
            base,
            data: vec![0; size],
        })
    }

    pub fn base(&self) -> u64 {
        self.base
    }

    pub fn size(&self) -> usize {
        self.data.len()
    }

    pub fn end(&self) -> u64 {
        self.base + self.data.len() as u64
    }

    pub fn contains(&self, addr: u64, len: usize) -> bool {
        self.offset(addr, len).is_some()
    }

    fn offset(&self, addr: u64, len: usize) -> Option<usize> {
        let start = addr.checked_sub(self.base)? as usize;
        let end = start.checked_add(len)?;
        if end <= self.data.len() {
            Some(start)
        } else {
            None
        }
    }

    /// Reads `buf.len()` bytes starting at physical `addr`.
    ///
    /// # Errors
    /// [`VmError::OutOfBounds`] if the range leaves RAM.
    pub fn read(&self, addr: u64, buf: &mut [u8]) -> Result<()> {
        let start = self.offset(addr, buf.len()).ok_or(VmError::OutOfBounds {
            addr,
            len: buf.len(),
        })?;
        buf.copy_from_slice(&self.data[start..start + buf.len()]);
        Ok(())
    }

    /// Writes `data` starting at physical `addr`.
    ///
    /// # Errors
    /// [`VmError::OutOfBounds`] if the range leaves RAM.
    pub fn write(&mut self, addr: u64, data: &[u8]) -> Result<()> {
        let start = self.offset(addr, data.len()).ok_or(VmError::OutOfBounds {
            addr,
            len: data.len(),
        })?;
        self.data[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }

    /// Reads an unsigned little-endian integer of `size` (1, 2, 4 or 8) bytes.
    ///
    /// # Errors
    /// [`VmError::OutOfBounds`] or [`VmError::UnsupportedAccessSize`].
    pub fn read_int(&self, addr: u64, size: usize) -> Result<u64> {
        match size {
            1 | 2 | 4 | 8 => {}
            _ => return Err(VmError::UnsupportedAccessSize { size }),
        }
        let mut buf = [0_u8; 8];
        self.read(addr, &mut buf[..size])?;
        Ok(u64::from_le_bytes(buf))
    }

    /// Writes the low `size` bytes of `value` little-endian at `addr`.
    ///
    /// # Errors
    /// [`VmError::OutOfBounds`] or [`VmError::UnsupportedAccessSize`].
    pub fn write_int(&mut self, addr: u64, size: usize, value: u64) -> Result<()> {
        match size {
            1 | 2 | 4 | 8 => {}
            _ => return Err(VmError::UnsupportedAccessSize { size }),
        }
        self.write(addr, &value.to_le_bytes()[..size])
    }

    /// Zeroes all of RAM (cold-boot semantics for a fresh boot/reboot).
    pub fn zero_fill(&mut self) {
        self.data.fill(0);
    }

    /// Borrows the raw backing store (host-side inspection, e.g. framebuffer
    /// presentation and test assertions). Never exposed to the guest.
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::Ram;
    use crate::error::VmError;

    const BASE: u64 = 0x8000_0000;

    #[test]
    fn read_write_round_trip() {
        let mut ram = Ram::new(BASE, 0x1000).expect("ram");
        ram.write_int(BASE + 8, 8, 0x0123_4567_89AB_CDEF)
            .expect("write");
        assert_eq!(
            ram.read_int(BASE + 8, 8).expect("read"),
            0x0123_4567_89AB_CDEF
        );
        assert_eq!(ram.read_int(BASE + 8, 1).expect("read byte"), 0xEF);
        assert_eq!(ram.read_int(BASE + 12, 4).expect("read word"), 0x0123_4567);
    }

    #[test]
    fn out_of_bounds_is_rejected() {
        let mut ram = Ram::new(BASE, 0x100).expect("ram");
        assert_eq!(
            ram.read_int(BASE - 8, 8),
            Err(VmError::OutOfBounds {
                addr: BASE - 8,
                len: 8
            })
        );
        assert!(ram.write_int(BASE + 0xFC, 8, 0).is_err());
        assert!(ram.write(BASE + 0x80, &[0_u8; 0x81]).is_err());
        assert!(ram.write_int(BASE, 3, 0).is_err());
    }

    #[test]
    fn zero_sized_ram_is_rejected() {
        assert!(Ram::new(BASE, 0).is_err());
    }
}
