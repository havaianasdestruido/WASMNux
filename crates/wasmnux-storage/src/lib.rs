//! Sparse Copy-on-Write 64 KiB chunked virtual block storage with CRC32 integrity,
//! snapshots, quota simulation, and validated `.wasmnux-disk` import/export bundles.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Standard VirtIO block sector size in bytes.
pub const SECTOR_SIZE: usize = 512;

/// Sparse Copy-on-Write chunk size (`64 KiB` = 128 sectors).
pub const CHUNK_SIZE: usize = 64 * 1024;

/// Maximum permitted virtual disk capacity (`2 GiB`) to enforce WASM/browser safety bounds.
pub const MAX_DISK_CAPACITY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Binary magic header for `.wasmnux-disk` export/import bundles.
pub const DISK_BUNDLE_MAGIC: &[u8; 8] = b"WASMNUXD";

/// Current `.wasmnux-disk` binary schema version.
pub const DISK_BUNDLE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    InvalidCapacity(u64),
    InvalidProfileId(String),
    OutOfBounds {
        offset: u64,
        len: usize,
        capacity: u64,
    },
    ChunkIndexOutOfRange {
        chunk_index: u32,
        total_chunks: u32,
    },
    InvalidChunkLength(usize),
    QuotaExceeded {
        max_overlay_chunks: usize,
    },
    ReadOnly,
    CorruptBundle(&'static str),
    ChecksumMismatch {
        expected: u32,
        actual: u32,
    },
    ProfileMismatch {
        expected: String,
        actual: String,
    },
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCapacity(cap) => write!(f, "invalid disk capacity: {cap} bytes"),
            Self::InvalidProfileId(id) => write!(f, "invalid profile identifier: {id}"),
            Self::OutOfBounds {
                offset,
                len,
                capacity,
            } => write!(
                f,
                "disk access out of bounds: offset={offset} len={len} capacity={capacity}"
            ),
            Self::ChunkIndexOutOfRange {
                chunk_index,
                total_chunks,
            } => write!(
                f,
                "chunk index {chunk_index} exceeds total chunks {total_chunks}"
            ),
            Self::InvalidChunkLength(len) => {
                write!(f, "invalid chunk length {len} (expected {CHUNK_SIZE})")
            }
            Self::QuotaExceeded { max_overlay_chunks } => {
                write!(
                    f,
                    "storage quota exceeded (limit {max_overlay_chunks} chunks)"
                )
            }
            Self::ReadOnly => write!(f, "virtual disk is mounted read-only"),
            Self::CorruptBundle(msg) => write!(f, "corrupt disk bundle: {msg}"),
            Self::ChecksumMismatch { expected, actual } => write!(
                f,
                "CRC32 checksum mismatch: expected 0x{expected:08x}, got 0x{actual:08x}"
            ),
            Self::ProfileMismatch { expected, actual } => {
                write!(f, "profile mismatch: expected '{expected}', got '{actual}'")
            }
        }
    }
}

impl std::error::Error for StorageError {}

/// Validate a profile or snapshot identifier to prevent path traversal and namespace collisions.
pub fn validate_identifier(id: &str) -> Result<(), StorageError> {
    if id.is_empty() || id.len() > 63 {
        return Err(StorageError::InvalidProfileId(id.to_owned()));
    }
    let bytes = id.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return Err(StorageError::InvalidProfileId(id.to_owned()));
    }
    for &b in bytes {
        let valid = b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_';
        if !valid {
            return Err(StorageError::InvalidProfileId(id.to_owned()));
        }
    }
    Ok(())
}

/// Compute IEEE 802.3 CRC32 checksum using a compile-time lookup table.
pub const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

#[inline]
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32_update(0xFFFF_FFFF, bytes) ^ 0xFFFF_FFFF
}

#[inline]
pub fn crc32_update(mut state: u32, bytes: &[u8]) -> u32 {
    for &b in bytes {
        let idx = ((state ^ u32::from(b)) & 0xFF) as usize;
        state = (state >> 8) ^ CRC32_TABLE[idx];
    }
    state
}

#[derive(Clone, Debug)]
pub struct DirtyChunkRecord {
    pub chunk_index: u32,
    pub crc32: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct DiskSnapshot {
    pub capacity_bytes: u64,
    pub read_only: bool,
    pub overlay: BTreeMap<u32, Box<[u8; CHUNK_SIZE]>>,
    pub generation: u64,
}

/// Sparse 64 KiB Copy-on-Write block device backing VirtIO-Blk (`/dev/vda`).
pub struct SparseBlockDisk {
    capacity_bytes: u64,
    total_chunks: u32,
    base_chunks: BTreeMap<u32, Box<[u8; CHUNK_SIZE]>>,
    overlay_chunks: BTreeMap<u32, Box<[u8; CHUNK_SIZE]>>,
    dirty_chunks: BTreeSet<u32>,
    read_only: bool,
    max_overlay_chunks: Option<usize>,
    generation: u64,
    bytes_read: u64,
    bytes_written: u64,
    flushes: u64,
}

impl SparseBlockDisk {
    pub fn new(capacity_bytes: u64) -> Result<Self, StorageError> {
        if capacity_bytes == 0
            || capacity_bytes > MAX_DISK_CAPACITY_BYTES
            || !capacity_bytes.is_multiple_of(CHUNK_SIZE as u64)
        {
            return Err(StorageError::InvalidCapacity(capacity_bytes));
        }
        let total_chunks = u32::try_from(capacity_bytes / (CHUNK_SIZE as u64))
            .map_err(|_| StorageError::InvalidCapacity(capacity_bytes))?;
        Ok(Self {
            capacity_bytes,
            total_chunks,
            base_chunks: BTreeMap::new(),
            overlay_chunks: BTreeMap::new(),
            dirty_chunks: BTreeSet::new(),
            read_only: false,
            max_overlay_chunks: None,
            generation: 0,
            bytes_read: 0,
            bytes_written: 0,
            flushes: 0,
        })
    }

    /// Create a sparse disk initialized from a raw disk image (skipping all-zero 64 KiB blocks).
    pub fn from_raw_base_image(
        image: &[u8],
        min_capacity_bytes: u64,
    ) -> Result<Self, StorageError> {
        let raw_len = image.len() as u64;
        let needed = raw_len.max(min_capacity_bytes);
        let aligned = needed.div_ceil(CHUNK_SIZE as u64) * (CHUNK_SIZE as u64);
        let mut disk = Self::new(aligned)?;
        for (idx, slice) in image.chunks(CHUNK_SIZE).enumerate() {
            if slice.iter().all(|&b| b == 0) {
                continue;
            }
            let mut block = Box::new([0u8; CHUNK_SIZE]);
            block[..slice.len()].copy_from_slice(slice);
            disk.base_chunks.insert(idx as u32, block);
        }
        Ok(disk)
    }

    pub fn capacity_bytes(&self) -> u64 {
        self.capacity_bytes
    }

    pub fn capacity_sectors(&self) -> u64 {
        self.capacity_bytes / (SECTOR_SIZE as u64)
    }

    pub fn total_chunks(&self) -> u32 {
        self.total_chunks
    }

    pub fn base_chunk_count(&self) -> usize {
        self.base_chunks.len()
    }

    pub fn overlay_chunk_count(&self) -> usize {
        self.overlay_chunks.len()
    }

    pub fn overlay_bytes(&self) -> u64 {
        (self.overlay_chunks.len() as u64) * (CHUNK_SIZE as u64)
    }

    pub fn dirty_chunk_count(&self) -> usize {
        self.dirty_chunks.len()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn stats(&self) -> (u64, u64, u64) {
        (self.bytes_read, self.bytes_written, self.flushes)
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn set_max_overlay_chunks(&mut self, limit: Option<usize>) {
        self.max_overlay_chunks = limit;
    }

    /// Load an immutable base chunk (`64 KiB`) without marking it dirty.
    pub fn load_base_chunk(&mut self, chunk_index: u32, data: &[u8]) -> Result<(), StorageError> {
        if chunk_index >= self.total_chunks {
            return Err(StorageError::ChunkIndexOutOfRange {
                chunk_index,
                total_chunks: self.total_chunks,
            });
        }
        if data.len() != CHUNK_SIZE {
            return Err(StorageError::InvalidChunkLength(data.len()));
        }
        if data.iter().all(|&b| b == 0) {
            self.base_chunks.remove(&chunk_index);
            return Ok(());
        }
        let mut buf = Box::new([0u8; CHUNK_SIZE]);
        buf.copy_from_slice(data);
        self.base_chunks.insert(chunk_index, buf);
        Ok(())
    }

    /// Hydrate a persisted overlay chunk from OPFS/IndexedDB with optional CRC32 verification.
    pub fn hydrate_overlay_chunk(
        &mut self,
        chunk_index: u32,
        data: &[u8],
        expected_crc32: Option<u32>,
    ) -> Result<(), StorageError> {
        if chunk_index >= self.total_chunks {
            return Err(StorageError::ChunkIndexOutOfRange {
                chunk_index,
                total_chunks: self.total_chunks,
            });
        }
        if data.len() != CHUNK_SIZE {
            return Err(StorageError::InvalidChunkLength(data.len()));
        }
        if let Some(expected) = expected_crc32 {
            let actual = crc32(data);
            if actual != expected {
                return Err(StorageError::ChecksumMismatch { expected, actual });
            }
        }
        let mut buf = Box::new([0u8; CHUNK_SIZE]);
        buf.copy_from_slice(data);
        self.overlay_chunks.insert(chunk_index, buf);
        self.dirty_chunks.remove(&chunk_index);
        Ok(())
    }

    /// Discard all mutable overlay writes, restoring the clean base disk image.
    pub fn reset_overlay(&mut self) {
        self.overlay_chunks.clear();
        self.dirty_chunks.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn read_at(&mut self, offset: u64, dst: &mut [u8]) -> Result<(), StorageError> {
        let end = offset
            .checked_add(dst.len() as u64)
            .ok_or(StorageError::OutOfBounds {
                offset,
                len: dst.len(),
                capacity: self.capacity_bytes,
            })?;
        if end > self.capacity_bytes {
            return Err(StorageError::OutOfBounds {
                offset,
                len: dst.len(),
                capacity: self.capacity_bytes,
            });
        }

        let mut remaining = dst.len();
        let mut dst_pos = 0usize;
        let mut cur_off = offset;

        while remaining > 0 {
            let chunk_idx = (cur_off / (CHUNK_SIZE as u64)) as u32;
            let in_chunk = (cur_off % (CHUNK_SIZE as u64)) as usize;
            let take = remaining.min(CHUNK_SIZE - in_chunk);

            if let Some(ov) = self.overlay_chunks.get(&chunk_idx) {
                dst[dst_pos..dst_pos + take].copy_from_slice(&ov[in_chunk..in_chunk + take]);
            } else if let Some(base) = self.base_chunks.get(&chunk_idx) {
                dst[dst_pos..dst_pos + take].copy_from_slice(&base[in_chunk..in_chunk + take]);
            } else {
                dst[dst_pos..dst_pos + take].fill(0);
            }

            cur_off += take as u64;
            dst_pos += take;
            remaining -= take;
        }

        self.bytes_read = self.bytes_read.wrapping_add(dst.len() as u64);
        Ok(())
    }

    pub fn write_at(&mut self, offset: u64, src: &[u8]) -> Result<(), StorageError> {
        if self.read_only {
            return Err(StorageError::ReadOnly);
        }
        let end = offset
            .checked_add(src.len() as u64)
            .ok_or(StorageError::OutOfBounds {
                offset,
                len: src.len(),
                capacity: self.capacity_bytes,
            })?;
        if end > self.capacity_bytes {
            return Err(StorageError::OutOfBounds {
                offset,
                len: src.len(),
                capacity: self.capacity_bytes,
            });
        }
        if src.is_empty() {
            return Ok(());
        }

        // Pre-verify quota across all newly touched chunks so writes are all-or-nothing.
        if let Some(limit) = self.max_overlay_chunks {
            let first_chunk = (offset / (CHUNK_SIZE as u64)) as u32;
            let last_chunk = ((end - 1) / (CHUNK_SIZE as u64)) as u32;
            let mut new_needed = 0usize;
            for idx in first_chunk..=last_chunk {
                if !self.overlay_chunks.contains_key(&idx) {
                    new_needed += 1;
                }
            }
            if self.overlay_chunks.len() + new_needed > limit {
                return Err(StorageError::QuotaExceeded {
                    max_overlay_chunks: limit,
                });
            }
        }

        let mut remaining = src.len();
        let mut src_pos = 0usize;
        let mut cur_off = offset;

        while remaining > 0 {
            let chunk_idx = (cur_off / (CHUNK_SIZE as u64)) as u32;
            let in_chunk = (cur_off % (CHUNK_SIZE as u64)) as usize;
            let take = remaining.min(CHUNK_SIZE - in_chunk);

            if !self.overlay_chunks.contains_key(&chunk_idx) {
                let mut fresh = Box::new([0u8; CHUNK_SIZE]);
                if let Some(base) = self.base_chunks.get(&chunk_idx) {
                    fresh.copy_from_slice(&base[..]);
                }
                self.overlay_chunks.insert(chunk_idx, fresh);
            }
            if let Some(chunk) = self.overlay_chunks.get_mut(&chunk_idx) {
                chunk[in_chunk..in_chunk + take].copy_from_slice(&src[src_pos..src_pos + take]);
            }
            self.dirty_chunks.insert(chunk_idx);

            cur_off += take as u64;
            src_pos += take;
            remaining -= take;
        }

        self.bytes_written = self.bytes_written.wrapping_add(src.len() as u64);
        Ok(())
    }

    pub fn flush(&mut self) -> u64 {
        self.flushes = self.flushes.wrapping_add(1);
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    /// Drain dirty chunk records for persistence to OPFS / IndexedDB.
    pub fn drain_dirty_chunks(&mut self) -> Vec<DirtyChunkRecord> {
        let mut out = Vec::with_capacity(self.dirty_chunks.len());
        let indices: Vec<u32> = self.dirty_chunks.iter().copied().collect();
        self.dirty_chunks.clear();
        for idx in indices {
            if let Some(chunk) = self.overlay_chunks.get(&idx) {
                let data = chunk.to_vec();
                let checksum = crc32(&data);
                out.push(DirtyChunkRecord {
                    chunk_index: idx,
                    crc32: checksum,
                    data,
                });
            }
        }
        out
    }

    /// Re-mark specific chunks dirty if a host storage commit failed mid-flight.
    pub fn requeue_dirty_chunks(&mut self, indices: &[u32]) {
        for &idx in indices {
            if self.overlay_chunks.contains_key(&idx) {
                self.dirty_chunks.insert(idx);
            }
        }
    }

    pub fn snapshot(&self) -> DiskSnapshot {
        DiskSnapshot {
            capacity_bytes: self.capacity_bytes,
            read_only: self.read_only,
            overlay: self.overlay_chunks.clone(),
            generation: self.generation,
        }
    }

    pub fn restore_snapshot(&mut self, snap: &DiskSnapshot) -> Result<(), StorageError> {
        if snap.capacity_bytes != self.capacity_bytes {
            return Err(StorageError::InvalidCapacity(snap.capacity_bytes));
        }
        self.read_only = snap.read_only;
        self.overlay_chunks = snap.overlay.clone();
        self.dirty_chunks = self.overlay_chunks.keys().copied().collect();
        self.generation = snap.generation.wrapping_add(1);
        Ok(())
    }

    /// Export mutable overlay state as a checksummed `.wasmnux-disk` binary bundle.
    pub fn export_overlay_bundle(&self, profile_id: &str) -> Result<Vec<u8>, StorageError> {
        validate_identifier(profile_id)?;
        let id_bytes = profile_id.as_bytes();
        let chunk_count = self.overlay_chunks.len() as u32;
        let estimated = 32 + id_bytes.len() + (self.overlay_chunks.len() * (8 + CHUNK_SIZE)) + 4;
        let mut out = Vec::with_capacity(estimated);

        out.extend_from_slice(DISK_BUNDLE_MAGIC);
        out.extend_from_slice(&DISK_BUNDLE_VERSION.to_le_bytes());
        out.extend_from_slice(&(id_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(id_bytes);
        out.extend_from_slice(&self.capacity_bytes.to_le_bytes());
        out.extend_from_slice(&chunk_count.to_le_bytes());

        for (&idx, chunk) in &self.overlay_chunks {
            let csum = crc32(&chunk[..]);
            out.extend_from_slice(&idx.to_le_bytes());
            out.extend_from_slice(&csum.to_le_bytes());
            out.extend_from_slice(&chunk[..]);
        }

        let bundle_crc = crc32(&out);
        out.extend_from_slice(&bundle_crc.to_le_bytes());
        Ok(out)
    }

    /// Validate and import an overlay bundle onto this disk (preserving immutable base chunks).
    pub fn import_overlay_bundle(
        &mut self,
        expected_profile_id: Option<&str>,
        bundle: &[u8],
    ) -> Result<String, StorageError> {
        // Minimum header (8 + 4 + 2 + 1 + 8 + 4) + trailer (4) = 31 bytes.
        if bundle.len() < 31 {
            return Err(StorageError::CorruptBundle("bundle shorter than header"));
        }
        if &bundle[0..8] != DISK_BUNDLE_MAGIC {
            return Err(StorageError::CorruptBundle("invalid magic header"));
        }
        let payload_end = bundle.len() - 4;
        let expected_bundle_crc = u32::from_le_bytes([
            bundle[payload_end],
            bundle[payload_end + 1],
            bundle[payload_end + 2],
            bundle[payload_end + 3],
        ]);
        let actual_bundle_crc = crc32(&bundle[..payload_end]);
        if actual_bundle_crc != expected_bundle_crc {
            return Err(StorageError::ChecksumMismatch {
                expected: expected_bundle_crc,
                actual: actual_bundle_crc,
            });
        }

        let version = u32::from_le_bytes([bundle[8], bundle[9], bundle[10], bundle[11]]);
        if version != DISK_BUNDLE_VERSION {
            return Err(StorageError::CorruptBundle("unsupported bundle version"));
        }

        let id_len = u16::from_le_bytes([bundle[12], bundle[13]]) as usize;
        let mut cursor = 14usize;
        if cursor
            .checked_add(id_len + 12)
            .is_none_or(|n| n > payload_end)
        {
            return Err(StorageError::CorruptBundle("truncated profile metadata"));
        }
        let profile_id = std::str::from_utf8(&bundle[cursor..cursor + id_len])
            .map_err(|_| StorageError::CorruptBundle("non-UTF8 profile ID"))?
            .to_owned();
        validate_identifier(&profile_id)?;
        cursor += id_len;

        if let Some(expected) = expected_profile_id {
            if expected != profile_id {
                return Err(StorageError::ProfileMismatch {
                    expected: expected.to_owned(),
                    actual: profile_id,
                });
            }
        }

        let cap = u64::from_le_bytes([
            bundle[cursor],
            bundle[cursor + 1],
            bundle[cursor + 2],
            bundle[cursor + 3],
            bundle[cursor + 4],
            bundle[cursor + 5],
            bundle[cursor + 6],
            bundle[cursor + 7],
        ]);
        cursor += 8;
        if cap != self.capacity_bytes {
            return Err(StorageError::InvalidCapacity(cap));
        }

        let count = u32::from_le_bytes([
            bundle[cursor],
            bundle[cursor + 1],
            bundle[cursor + 2],
            bundle[cursor + 3],
        ]);
        cursor += 4;
        if count > self.total_chunks {
            return Err(StorageError::CorruptBundle("chunk count exceeds capacity"));
        }

        let record_size = 8 + CHUNK_SIZE;
        let expected_rem = (count as usize)
            .checked_mul(record_size)
            .ok_or(StorageError::CorruptBundle("chunk size overflow"))?;
        if payload_end - cursor != expected_rem {
            return Err(StorageError::CorruptBundle("payload length mismatch"));
        }

        let mut parsed_overlay = BTreeMap::new();
        for _ in 0..count {
            let idx = u32::from_le_bytes([
                bundle[cursor],
                bundle[cursor + 1],
                bundle[cursor + 2],
                bundle[cursor + 3],
            ]);
            let expected_crc = u32::from_le_bytes([
                bundle[cursor + 4],
                bundle[cursor + 5],
                bundle[cursor + 6],
                bundle[cursor + 7],
            ]);
            cursor += 8;
            if idx >= self.total_chunks {
                return Err(StorageError::ChunkIndexOutOfRange {
                    chunk_index: idx,
                    total_chunks: self.total_chunks,
                });
            }
            let data = &bundle[cursor..cursor + CHUNK_SIZE];
            cursor += CHUNK_SIZE;
            let actual_crc = crc32(data);
            if actual_crc != expected_crc {
                return Err(StorageError::ChecksumMismatch {
                    expected: expected_crc,
                    actual: actual_crc,
                });
            }
            if parsed_overlay.contains_key(&idx) {
                return Err(StorageError::CorruptBundle("duplicate chunk index"));
            }
            let mut block = Box::new([0u8; CHUNK_SIZE]);
            block.copy_from_slice(data);
            parsed_overlay.insert(idx, block);
        }

        if let Some(limit) = self.max_overlay_chunks {
            if parsed_overlay.len() > limit {
                return Err(StorageError::QuotaExceeded {
                    max_overlay_chunks: limit,
                });
            }
        }

        self.overlay_chunks = parsed_overlay;
        self.dirty_chunks = self.overlay_chunks.keys().copied().collect();
        self.generation = self.generation.wrapping_add(1);
        Ok(profile_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0x0000_0000);
    }

    #[test]
    fn identifier_validation_blocks_traversal_and_special_chars() {
        assert!(validate_identifier("vanilla").is_ok());
        assert!(validate_identifier("debian-13_trixie").is_ok());
        assert!(validate_identifier("").is_err());
        assert!(validate_identifier("../etc/passwd").is_err());
        assert!(validate_identifier("Vanilla").is_err());
        assert!(validate_identifier("-leading-dash").is_err());
        assert!(validate_identifier("a/b").is_err());
    }

    #[test]
    fn sparse_cow_reads_writes_across_chunk_boundaries_and_tracks_dirty() {
        let mut base_raw = vec![0u8; CHUNK_SIZE * 2];
        base_raw[0..8].copy_from_slice(b"BASEHEAD");
        base_raw[CHUNK_SIZE - 4..CHUNK_SIZE + 4].copy_from_slice(b"BOUNDARY");

        let mut disk = SparseBlockDisk::from_raw_base_image(&base_raw, (CHUNK_SIZE * 4) as u64)
            .expect("valid sparse disk");
        assert_eq!(disk.base_chunk_count(), 2);
        assert_eq!(disk.overlay_chunk_count(), 0);

        // Cross-chunk write preserves untouched base bytes in both chunks.
        let patch = b"OVERWRITE_ACROSS_CHUNKS!";
        disk.write_at((CHUNK_SIZE - 8) as u64, patch)
            .expect("cross-chunk write");
        assert_eq!(disk.overlay_chunk_count(), 2);
        assert_eq!(disk.dirty_chunk_count(), 2);

        let mut head = [0u8; 8];
        disk.read_at(0, &mut head).expect("read base head");
        assert_eq!(&head, b"BASEHEAD");

        let mut check = vec![0u8; patch.len()];
        disk.read_at((CHUNK_SIZE - 8) as u64, &mut check)
            .expect("read cross-chunk");
        assert_eq!(&check, patch);

        let dirty = disk.drain_dirty_chunks();
        assert_eq!(dirty.len(), 2);
        assert_eq!(disk.dirty_chunk_count(), 0);
    }

    #[test]
    fn enforces_bounds_readonly_and_atomic_quota_limits() {
        let mut disk = SparseBlockDisk::new((CHUNK_SIZE * 4) as u64).expect("disk");
        assert!(matches!(
            disk.write_at((CHUNK_SIZE * 4) as u64, &[1]),
            Err(StorageError::OutOfBounds { .. })
        ));
        assert!(matches!(
            disk.read_at(u64::MAX - 2, &mut [0u8; 8]),
            Err(StorageError::OutOfBounds { .. })
        ));

        disk.set_read_only(true);
        assert_eq!(disk.write_at(0, b"nope"), Err(StorageError::ReadOnly));
        disk.set_read_only(false);

        disk.set_max_overlay_chunks(Some(1));
        // Spanning chunks 0 and 1 must fail atomically without partially mutating chunk 0.
        let res = disk.write_at((CHUNK_SIZE - 2) as u64, b"ABCD");
        assert_eq!(
            res,
            Err(StorageError::QuotaExceeded {
                max_overlay_chunks: 1
            })
        );
        assert_eq!(disk.overlay_chunk_count(), 0);
    }

    #[test]
    fn snapshot_export_and_tamper_resistant_import_work() {
        let mut disk = SparseBlockDisk::new((CHUNK_SIZE * 4) as u64).expect("disk");
        disk.write_at(512, b"original-state").expect("write");
        let snap = disk.snapshot();

        disk.write_at(512, b"modified-state").expect("modify");
        let exported = disk.export_overlay_bundle("vanilla").expect("export");

        disk.restore_snapshot(&snap).expect("restore");
        let mut buf = [0u8; 14];
        disk.read_at(512, &mut buf).expect("read restored");
        assert_eq!(&buf, b"original-state");

        // Reject wrong profile on import.
        assert!(matches!(
            disk.import_overlay_bundle(Some("debian"), &exported),
            Err(StorageError::ProfileMismatch { .. })
        ));

        // Reject tampered byte in bundle.
        let mut tampered = exported.clone();
        let mid = tampered.len() / 2;
        tampered[mid] ^= 0xFF;
        assert!(matches!(
            disk.import_overlay_bundle(Some("vanilla"), &tampered),
            Err(StorageError::ChecksumMismatch { .. })
        ));

        // Valid import restores modified state.
        let id = disk
            .import_overlay_bundle(Some("vanilla"), &exported)
            .expect("valid import");
        assert_eq!(id, "vanilla");
        disk.read_at(512, &mut buf).expect("read imported");
        assert_eq!(&buf, b"modified-state");
    }
}
