//! Block-storage backend abstraction. The synchronous core talks to this
//! trait; the WASM layer currently backs it with an in-memory image that the
//! TypeScript side loads from / saves to its persistent chunk store
//! (OPFS/IndexedDB). An async, directly-bridged backend is Phase-4 work and
//! is tracked in TODO.MD — it is not silently assumed here.

use crate::error::{Result, VmError};

/// Sector size exposed to the guest (VirtIO block standard).
pub const SECTOR_SIZE: usize = 512;

/// A guest block device backing store, addressed in bytes.
pub trait BlockBackend {
    /// Total capacity in bytes.
    fn capacity(&self) -> u64;

    /// Reads `buf.len()` bytes at byte offset `offset`.
    ///
    /// # Errors
    /// [`VmError::OutOfBounds`] when the range exceeds capacity.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()>;

    /// Writes `data` at byte offset `offset`.
    ///
    /// # Errors
    /// [`VmError::OutOfBounds`] when the range exceeds capacity.
    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()>;

    /// Flushes any buffered writes to durable storage.
    ///
    /// # Errors
    /// Backend-specific I/O failures.
    fn flush(&mut self) -> Result<()>;

    /// Number of 512-byte sectors (VirtIO capacity unit).
    fn sector_count(&self) -> u64 {
        self.capacity() / SECTOR_SIZE as u64
    }
}

/// In-memory block backend used by tests and the v1 WASM disk bridge.
#[derive(Debug)]
pub struct VecBlockBackend {
    data: Vec<u8>,
    flush_count: u64,
    dirty: bool,
}

impl VecBlockBackend {
    /// Creates a zeroed disk of `size` bytes.
    ///
    /// # Errors
    /// [`VmError::InvalidConfig`] if `size` is zero or not sector-aligned.
    pub fn new(size: usize) -> Result<Self> {
        Self::from_bytes(vec![0; size])
    }

    /// Creates a disk from an existing image, flagging it clean.
    ///
    /// # Errors
    /// [`VmError::InvalidConfig`] if the image is empty or not sector-aligned.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        if data.is_empty() || data.len() % SECTOR_SIZE != 0 {
            return Err(VmError::InvalidConfig(format!(
                "disk image size {} is empty or not a multiple of {SECTOR_SIZE}",
                data.len()
            )));
        }
        Ok(Self {
            data,
            flush_count: 0,
            dirty: false,
        })
    }

    /// Whether writes have occurred since creation/last `take_image`.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Number of flush requests honoured (guest FLUSH commands).
    pub fn flush_count(&self) -> u64 {
        self.flush_count
    }

    /// Borrows the whole image (host-side export to persistent storage).
    pub fn image(&self) -> &[u8] {
        &self.data
    }

    /// Marks the image clean after the host persisted it.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    fn check_range(&self, offset: u64, len: usize) -> Result<usize> {
        let start =
            usize::try_from(offset).map_err(|_| VmError::OutOfBounds { addr: offset, len })?;
        if start
            .checked_add(len)
            .is_some_and(|end| end <= self.data.len())
        {
            Ok(start)
        } else {
            Err(VmError::OutOfBounds { addr: offset, len })
        }
    }
}

impl BlockBackend for VecBlockBackend {
    fn capacity(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start = self.check_range(offset, buf.len())?;
        buf.copy_from_slice(&self.data[start..start + buf.len()]);
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()> {
        let start = self.check_range(offset, data.len())?;
        self.data[start..start + data.len()].copy_from_slice(data);
        self.dirty = true;
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.flush_count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockBackend, VecBlockBackend, SECTOR_SIZE};

    #[test]
    fn sector_round_trip_and_dirty_tracking() {
        let mut disk = VecBlockBackend::new(4 * SECTOR_SIZE).expect("disk");
        assert_eq!(disk.sector_count(), 4);
        assert!(!disk.is_dirty());
        let payload = [0xA5_u8; SECTOR_SIZE];
        disk.write_at(SECTOR_SIZE as u64, &payload).expect("write");
        assert!(disk.is_dirty());
        let mut back = [0_u8; SECTOR_SIZE];
        disk.read_at(SECTOR_SIZE as u64, &mut back).expect("read");
        assert_eq!(back, payload);
        disk.flush().expect("flush");
        assert_eq!(disk.flush_count(), 1);
    }

    #[test]
    fn rejects_out_of_range_and_bad_sizes() {
        assert!(VecBlockBackend::new(0).is_err());
        assert!(VecBlockBackend::new(SECTOR_SIZE + 1).is_err());
        let mut disk = VecBlockBackend::new(SECTOR_SIZE).expect("disk");
        let mut buf = [0_u8; SECTOR_SIZE];
        assert!(disk.read_at(1, &mut buf).is_err());
        assert!(disk.write_at(SECTOR_SIZE as u64, &[1]).is_err());
    }
}
