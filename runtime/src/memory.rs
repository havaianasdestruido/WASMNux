//! Guest physical memory.
//!
//! A single, bounds-checked byte buffer that models the guest's physical RAM.
//! Every guest load/store is range-checked so the guest can never read or write
//! outside its own memory, which is the primary sandbox boundary.

use core::fmt;

/// Errors returned by guest memory operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryError {
    /// The requested address lies outside the guest memory region.
    OutOfBounds { addr: u64, size: u64, len: u64 },
    /// The access is not naturally aligned for its size.
    Unaligned { addr: u64, size: u64 },
    /// The access crossed the end of the guest memory mid-operation.
    MisalignedAccess { addr: u64, size: u64 },
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryError::OutOfBounds { addr, size, len } => write!(
                f,
                "guest memory out of bounds: addr=0x{:x} size={} len=0x{:x}",
                addr, size, len
            ),
            MemoryError::Unaligned { addr, size } => write!(
                f,
                "guest memory unaligned access: addr=0x{:x} size={}",
                addr, size
            ),
            MemoryError::MisalignedAccess { addr, size } => write!(
                f,
                "guest memory misaligned access: addr=0x{:x} size={}",
                addr, size
            ),
        }
    }
}

impl std::error::Error for MemoryError {}

/// Guest physical memory.
///
/// All addresses are guest-physical. Accesses are bounds-checked against `len`.
/// Reads/writes of multi-byte values are little-endian (RISC-V is little-endian).
#[derive(Debug, Clone)]
pub struct GuestMemory {
    data: Vec<u8>,
}

impl GuestMemory {
    /// Create a guest memory region of `size` bytes, zero-initialized.
    pub fn new(size: usize) -> Self {
        Self {
            data: vec![0; size],
        }
    }

    /// Create a guest memory region initialized from a slice (used for ELF/DTB
    /// images loaded at a fixed base address).
    pub fn from_slice(base: u64, bytes: &[u8], len: usize) -> Result<Self, MemoryError> {
        let end = base
            .checked_add(bytes.len() as u64)
            .ok_or(MemoryError::OutOfBounds {
                addr: base,
                size: bytes.len() as u64,
                len: len as u64,
            })?;
        if end > len as u64 {
            return Err(MemoryError::OutOfBounds {
                addr: base,
                size: bytes.len() as u64,
                len: len as u64,
            });
        }
        let mut data = vec![0; len];
        data[base as usize..base as usize + bytes.len()].copy_from_slice(bytes);
        Ok(Self { data })
    }

    /// Guest memory size in bytes.
    pub fn len(&self) -> u64 {
        self.data.len() as u64
    }

    /// Returns true if the memory region is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Grow the memory by `additional` bytes, zero-initialized.
    pub fn grow(&mut self, additional: usize) {
        self.data.resize(self.data.len() + additional, 0);
    }

    /// Read a single byte at `addr`.
    pub fn read_u8(&self, addr: u64) -> Result<u8, MemoryError> {
        self.check_range(addr, 1)?;
        Ok(self.data[addr as usize])
    }

    /// Read a little-endian 16-bit value at `addr` (must be 2-byte aligned).
    pub fn read_u16(&self, addr: u64) -> Result<u16, MemoryError> {
        self.check_range(addr, 2)?;
        self.check_aligned(addr, 2)?;
        Ok(u16::from_le_bytes([
            self.data[addr as usize],
            self.data[addr as usize + 1],
        ]))
    }

    /// Read a little-endian 32-bit value at `addr` (must be 4-byte aligned).
    pub fn read_u32(&self, addr: u64) -> Result<u32, MemoryError> {
        self.check_range(addr, 4)?;
        self.check_aligned(addr, 4)?;
        Ok(u32::from_le_bytes([
            self.data[addr as usize],
            self.data[addr as usize + 1],
            self.data[addr as usize + 2],
            self.data[addr as usize + 3],
        ]))
    }

    /// Read a little-endian 64-bit value at `addr` (must be 8-byte aligned).
    pub fn read_u64(&self, addr: u64) -> Result<u64, MemoryError> {
        self.check_range(addr, 8)?;
        self.check_aligned(addr, 8)?;
        Ok(u64::from_le_bytes([
            self.data[addr as usize],
            self.data[addr as usize + 1],
            self.data[addr as usize + 2],
            self.data[addr as usize + 3],
            self.data[addr as usize + 4],
            self.data[addr as usize + 5],
            self.data[addr as usize + 6],
            self.data[addr as usize + 7],
        ]))
    }

    /// Write a single byte at `addr`.
    pub fn write_u8(&mut self, addr: u64, value: u8) -> Result<(), MemoryError> {
        self.check_range(addr, 1)?;
        self.data[addr as usize] = value;
        Ok(())
    }

    /// Write a little-endian 16-bit value at `addr` (must be 2-byte aligned).
    pub fn write_u16(&mut self, addr: u64, value: u16) -> Result<(), MemoryError> {
        self.check_range(addr, 2)?;
        self.check_aligned(addr, 2)?;
        let bytes = value.to_le_bytes();
        self.data[addr as usize] = bytes[0];
        self.data[addr as usize + 1] = bytes[1];
        Ok(())
    }

    /// Write a little-endian 32-bit value at `addr` (must be 4-byte aligned).
    pub fn write_u32(&mut self, addr: u64, value: u32) -> Result<(), MemoryError> {
        self.check_range(addr, 4)?;
        self.check_aligned(addr, 4)?;
        let bytes = value.to_le_bytes();
        self.data[addr as usize..addr as usize + 4].copy_from_slice(&bytes);
        Ok(())
    }

    /// Write a little-endian 64-bit value at `addr` (must be 8-byte aligned).
    pub fn write_u64(&mut self, addr: u64, value: u64) -> Result<(), MemoryError> {
        self.check_range(addr, 8)?;
        self.check_aligned(addr, 8)?;
        let bytes = value.to_le_bytes();
        self.data[addr as usize..addr as usize + 8].copy_from_slice(&bytes);
        Ok(())
    }

    /// Read a raw slice of `len` bytes starting at `addr` (no alignment req.).
    pub fn read_bytes(&self, addr: u64, buf: &mut [u8]) -> Result<(), MemoryError> {
        self.check_range(addr, buf.len() as u64)?;
        buf.copy_from_slice(&self.data[addr as usize..addr as usize + buf.len()]);
        Ok(())
    }

    /// Write a raw slice of bytes starting at `addr` (no alignment req.).
    pub fn write_bytes(&mut self, addr: u64, buf: &[u8]) -> Result<(), MemoryError> {
        self.check_range(addr, buf.len() as u64)?;
        self.data[addr as usize..addr as usize + buf.len()].copy_from_slice(buf);
        Ok(())
    }

    /// Return a mutable reference to the underlying byte buffer.
    ///
    /// Used by the WASM host to expose guest memory to TypeScript. The host
    /// must not write beyond `len()`.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Return an immutable reference to the underlying byte buffer.
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    fn check_range(&self, addr: u64, size: u64) -> Result<(), MemoryError> {
        let end = addr.checked_add(size).ok_or(MemoryError::OutOfBounds {
            addr,
            size,
            len: self.len(),
        })?;
        if end > self.len() {
            return Err(MemoryError::OutOfBounds {
                addr,
                size,
                len: self.len(),
            });
        }
        Ok(())
    }

    fn check_aligned(&self, addr: u64, size: u64) -> Result<(), MemoryError> {
        if !addr.is_multiple_of(size) {
            return Err(MemoryError::Unaligned { addr, size });
        }
        Ok(())
    }
}

impl Default for GuestMemory {
    fn default() -> Self {
        Self::new(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_u8_roundtrip() {
        let mut m = GuestMemory::new(16);
        m.write_u8(0, 0xAB).unwrap();
        assert_eq!(m.read_u8(0).unwrap(), 0xAB);
    }

    #[test]
    fn write_read_u32_roundtrip() {
        let mut m = GuestMemory::new(64);
        m.write_u32(0x10, 0xDEADBEEF).unwrap();
        assert_eq!(m.read_u32(0x10).unwrap(), 0xDEADBEEF);
        assert_eq!(m.read_u8(0x10).unwrap(), 0xEF);
        assert_eq!(m.read_u8(0x13).unwrap(), 0xDE);
    }

    #[test]
    fn write_read_u64_roundtrip() {
        let mut m = GuestMemory::new(128);
        m.write_u64(0x20, 0x1122334455667788).unwrap();
        assert_eq!(m.read_u64(0x20).unwrap(), 0x1122334455667788);
    }

    #[test]
    fn out_of_bounds_read() {
        let m = GuestMemory::new(16);
        assert!(m.read_u32(13).is_err());
        assert!(m.read_u64(9).is_err());
    }

    #[test]
    fn unaligned_access_rejected() {
        let mut m = GuestMemory::new(64);
        assert!(m.write_u32(1, 0x42).is_err());
        assert!(m.read_u64(3).is_err());
    }

    #[test]
    fn grow_extends_memory() {
        let mut m = GuestMemory::new(16);
        assert_eq!(m.len(), 16);
        m.grow(16);
        assert_eq!(m.len(), 32);
        assert_eq!(m.read_u8(20).unwrap(), 0);
    }

    #[test]
    fn from_slice_bounds() {
        let payload = [1u8, 2, 3, 4];
        let m = GuestMemory::from_slice(0x1000, &payload, 0x2000).unwrap();
        let mut buf = [0u8; 4];
        m.read_bytes(0x1000, &mut buf).unwrap();
        assert_eq!(buf, payload);
        let too_big = GuestMemory::from_slice(0x1000, &[0u8; 0x3000], 0x2000);
        assert!(too_big.is_err());
    }
}
