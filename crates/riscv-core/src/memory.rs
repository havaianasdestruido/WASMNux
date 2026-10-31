//! Fixed, fallibly allocated guest RAM. No host mmap or filesystem mapping.
use std::sync::atomic::{AtomicUsize, Ordering};
pub const PAGE_SIZE: u64 = 4096;
pub const RAM_BASE: u64 = 0x8000_0000;
pub const INVALID_PAGENUM: u64 = u64::MAX;
pub fn page(addr: u64) -> u64 {
    addr >> 12
}
pub fn page_offset(addr: u64) -> u64 {
    addr & 4095
}
pub fn avail(addr: u64, n: u64) -> bool {
    page(addr) == page(addr.wrapping_add(n).wrapping_sub(1)) && n > 0
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemError {
    InvalidAddr,
    OutOfMemory,
}
impl std::fmt::Display for MemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidAddr => "invalid guest address",
            Self::OutOfMemory => "guest RAM allocation failed",
        })
    }
}
impl std::error::Error for MemError {}
pub struct Memory {
    data: Box<[u8]>,
    ptr: *mut u8,
    len: usize,
    pub ram_base: u64,
    pub ram_end: u64,
    next_page: AtomicUsize,
}
// All machine execution and device I/O are serialized on one worker.
unsafe impl Send for Memory {}
unsafe impl Sync for Memory {}
impl Memory {
    pub fn try_new(base: u64, size: u64) -> Result<Self, MemError> {
        if size == 0
            || size > 1024 * 1024 * 1024
            || size % PAGE_SIZE != 0
            || base % PAGE_SIZE != 0
            || base.checked_add(size).is_none()
        {
            return Err(MemError::InvalidAddr);
        }
        let mut v = Vec::new();
        v.try_reserve_exact(size as usize)
            .map_err(|_| MemError::OutOfMemory)?;
        v.resize(size as usize, 0);
        let data = v.into_boxed_slice();
        let ptr = data.as_ptr().cast_mut();
        let len = data.len();
        Ok(Self {
            data,
            ptr,
            len,
            ram_base: base,
            ram_end: base + size,
            next_page: AtomicUsize::new(0),
        })
    }
    pub fn new(base: u64, size: u64) -> Self {
        Self::try_new(base, size).expect("test RAM allocation")
    }
    pub fn size(&self) -> u64 {
        self.data.len() as u64
    }
    pub fn contains(&self, a: u64) -> bool {
        (self.ram_base..self.ram_end).contains(&a)
    }
    pub fn contains_range(&self, a: u64, n: u64) -> bool {
        a >= self.ram_base && n <= self.size() && a - self.ram_base <= self.size() - n
    }
    pub fn offset(&self, a: u64) -> u64 {
        a.wrapping_sub(self.ram_base)
    }
    pub fn alloc_page(&self) -> Result<u64, MemError> {
        let n = self.next_page.fetch_add(1, Ordering::SeqCst);
        if n >= self.len / 4096 {
            return Err(MemError::OutOfMemory);
        }
        Ok(self.ram_base / 4096 + n as u64)
    }
    pub fn page_offset_of(&self, n: u64) -> Result<u64, MemError> {
        let base = self.ram_base / 4096;
        if n < base || n - base >= self.len as u64 / 4096 {
            return Err(MemError::InvalidAddr);
        }
        Ok((n - base) * 4096)
    }
    pub fn ptr_at(&self, o: u64) -> *mut u8 {
        assert!(o < self.size());
        unsafe { self.ptr.add(o as usize) }
    }
    fn check(&self, o: u64, n: u64) {
        assert!(
            o <= self.size() && n <= self.size() - o,
            "guest RAM access out of bounds"
        )
    }
    pub fn read_u8(&self, o: u64) -> u8 {
        self.check(o, 1);
        unsafe { *self.ptr.add(o as usize) }
    }
    pub fn read_u16(&self, o: u64) -> u16 {
        self.check(o, 2);
        unsafe { self.ptr.add(o as usize).cast::<u16>().read_unaligned() }
    }
    pub fn read_u32(&self, o: u64) -> u32 {
        self.check(o, 4);
        unsafe { self.ptr.add(o as usize).cast::<u32>().read_unaligned() }
    }
    pub fn read_u64(&self, o: u64) -> u64 {
        self.check(o, 8);
        unsafe { self.ptr.add(o as usize).cast::<u64>().read_unaligned() }
    }
    pub fn write_u8(&self, o: u64, v: u8) {
        self.check(o, 1);
        unsafe { *self.ptr.add(o as usize) = v }
    }
    pub fn write_u16(&self, o: u64, v: u16) {
        self.check(o, 2);
        unsafe { self.ptr.add(o as usize).cast::<u16>().write_unaligned(v) }
    }
    pub fn write_u32(&self, o: u64, v: u32) {
        self.check(o, 4);
        unsafe { self.ptr.add(o as usize).cast::<u32>().write_unaligned(v) }
    }
    pub fn write_u64(&self, o: u64, v: u64) {
        self.check(o, 8);
        unsafe { self.ptr.add(o as usize).cast::<u64>().write_unaligned(v) }
    }
    pub fn read_bytes(&self, o: u64, b: &mut [u8]) {
        self.check(o, b.len() as u64);
        unsafe { std::ptr::copy_nonoverlapping(self.ptr.add(o as usize), b.as_mut_ptr(), b.len()) }
    }
    pub fn write_bytes(&self, o: u64, b: &[u8]) {
        self.check(o, b.len() as u64);
        unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), self.ptr.add(o as usize), b.len()) }
    }
    pub fn zero(&self, o: u64, n: u64) {
        self.check(o, n);
        unsafe { std::ptr::write_bytes(self.ptr.add(o as usize), 0, n as usize) }
    }
    /// Export only while the machine is paused; callers must not retain the pointer.
    pub unsafe fn ram(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
    pub unsafe fn guest_slice(&self, a: u64, n: u64) -> Option<&[u8]> {
        if self.contains_range(a, n) {
            unsafe {
                Some(std::slice::from_raw_parts(
                    self.ptr.add(self.offset(a) as usize),
                    n as usize,
                ))
            }
        } else {
            None
        }
    }
    pub unsafe fn guest_slice_mut(&self, a: u64, n: u64) -> Option<&mut [u8]> {
        if self.contains_range(a, n) {
            unsafe {
                Some(std::slice::from_raw_parts_mut(
                    self.ptr.add(self.offset(a) as usize),
                    n as usize,
                ))
            }
        } else {
            None
        }
    }
    pub fn fill(&self, o: u64, n: u64, v: u8) {
        self.check(o, n);
        unsafe { std::ptr::write_bytes(self.ptr.add(o as usize), v, n as usize) }
    }
    pub fn load_u8(&self, a: u64) -> Option<u8> {
        self.contains(a).then(|| self.read_u8(self.offset(a)))
    }
    pub fn load_u16(&self, a: u64) -> Option<u16> {
        self.contains_range(a, 2)
            .then(|| self.read_u16(self.offset(a)))
    }
    pub fn load_u32(&self, a: u64) -> Option<u32> {
        self.contains_range(a, 4)
            .then(|| self.read_u32(self.offset(a)))
    }
    pub fn load_u64(&self, a: u64) -> Option<u64> {
        self.contains_range(a, 8)
            .then(|| self.read_u64(self.offset(a)))
    }
    pub fn store_u8(&self, a: u64, v: u8) -> Option<()> {
        self.contains(a).then(|| self.write_u8(self.offset(a), v))
    }
    pub fn store_u16(&self, a: u64, v: u16) -> Option<()> {
        self.contains_range(a, 2)
            .then(|| self.write_u16(self.offset(a), v))
    }
    pub fn store_u32(&self, a: u64, v: u32) -> Option<()> {
        self.contains_range(a, 4)
            .then(|| self.write_u32(self.offset(a), v))
    }
    pub fn store_u64(&self, a: u64, v: u64) -> Option<()> {
        self.contains_range(a, 8)
            .then(|| self.write_u64(self.offset(a), v))
    }
    pub fn note_dma_write(&self, _a: u64, _n: u64) {}
    pub fn is_code(&self, _o: u64) -> bool {
        false
    }
    pub fn mark_code(&self, _o: u64) -> bool {
        false
    }
    pub fn code_dirty(&self) -> bool {
        false
    }
    pub fn take_code_dirty(&self) -> bool {
        false
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_and_little_endian() {
        let m = Memory::new(RAM_BASE, 4096);
        m.write_u64(m.offset(RAM_BASE), 0x1122334455667788);
        assert_eq!(m.read_u8(m.offset(RAM_BASE)), 0x88);
        assert!(m.contains_range(RAM_BASE, 4096));
        assert!(!m.contains_range(RAM_BASE + 4095, 2));
        assert!(!m.contains_range(u64::MAX, 2));
    }
    #[test]
    fn allocation_rejected() {
        for n in [0, 1, u64::MAX, 1024 * 1024 * 1024 + 4096] {
            assert!(Memory::try_new(RAM_BASE, n).is_err());
        }
    }
    #[test]
    #[should_panic(expected = "guest RAM access out of bounds")]
    fn read_rejected() {
        Memory::new(RAM_BASE, 4096).read_u64(4095);
    }
}
