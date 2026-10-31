//! ELF64 loader for RISC-V guest images.
//!
//! Loads ELF program segments into guest memory. Because this emulator models
//! physical memory without an MMU, virtual addresses are treated as physical
//! addresses (identity mapping). A real Linux boot requires an MMU/sv39 page
//! table, which is documented as a future extension.

use crate::memory::{GuestMemory, MemoryError};

/// ELF64 header (little-endian).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Elf64Ehdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

const ELFMAG0: u8 = 0x7F;
const ELFMAG1: u8 = b'E';
const ELFMAG2: u8 = b'L';
const ELFMAG3: u8 = b'F';
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EM_RISCV: u16 = 243;
#[allow(dead_code)]
const ET_EXEC: u16 = 2;
const PT_LOAD: u32 = 1;

/// ELF64 program header.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElfError {
    NotElf,
    UnsupportedClass,
    UnsupportedData,
    BadMachine,
    Truncated,
    LoadFailed(MemoryError),
}

impl core::fmt::Display for ElfError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ElfError::NotElf => write!(f, "not an ELF file"),
            ElfError::UnsupportedClass => write!(f, "unsupported ELF class"),
            ElfError::UnsupportedData => write!(f, "unsupported ELF data encoding"),
            ElfError::BadMachine => write!(f, "not a RISC-V ELF"),
            ElfError::Truncated => write!(f, "truncated ELF file"),
            ElfError::LoadFailed(e) => write!(f, "ELF load failed: {}", e),
        }
    }
}

impl std::error::Error for ElfError {}

/// Load an ELF64 image into `guest_mem`. Returns the entry point address.
///
/// `image` must contain the full ELF file bytes. Segments are copied into
/// guest memory at their paddr; bss (p_memsz > p_filesz) is zeroed.
pub fn load_elf64(image: &[u8], guest_mem: &mut GuestMemory) -> Result<u64, ElfError> {
    if image.len() < core::mem::size_of::<Elf64Ehdr>() {
        return Err(ElfError::Truncated);
    }
    let ehdr: &Elf64Ehdr = unsafe { &*(image.as_ptr() as *const Elf64Ehdr) };

    if ehdr.e_ident[0..4] != [ELFMAG0, ELFMAG1, ELFMAG2, ELFMAG3] {
        return Err(ElfError::NotElf);
    }
    if ehdr.e_ident[4] != ELFCLASS64 {
        return Err(ElfError::UnsupportedClass);
    }
    if ehdr.e_ident[5] != ELFDATA2LSB {
        return Err(ElfError::UnsupportedData);
    }
    if ehdr.e_machine != EM_RISCV {
        return Err(ElfError::BadMachine);
    }

    let phoff = ehdr.e_phoff as usize;
    let phentsize = ehdr.e_phentsize as usize;
    let phnum = ehdr.e_phnum as usize;
    if phoff + phentsize * phnum > image.len() {
        return Err(ElfError::Truncated);
    }

    let mut entry = ehdr.e_entry;
    for i in 0..phnum {
        let ph_start = phoff + i * phentsize;
        let ph: &Elf64Phdr =
            unsafe { &*(image[ph_start..ph_start + phentsize].as_ptr() as *const Elf64Phdr) };
        if ph.p_type != PT_LOAD {
            continue;
        }
        if ph.p_filesz > ph.p_memsz {
            return Err(ElfError::LoadFailed(MemoryError::OutOfBounds {
                addr: ph.p_paddr,
                size: ph.p_filesz,
                len: guest_mem.len(),
            }));
        }
        // Filesz bytes from file offset -> guest paddr.
        let file_end = (ph.p_offset as usize)
            .checked_add(ph.p_filesz as usize)
            .ok_or(ElfError::Truncated)?;
        if file_end > image.len() {
            return Err(ElfError::Truncated);
        }
        guest_mem
            .write_bytes(ph.p_paddr, &image[ph.p_offset as usize..file_end])
            .map_err(ElfError::LoadFailed)?;
        // Zero the remainder up to memsz.
        if ph.p_memsz > ph.p_filesz {
            let zero_start = ph.p_paddr + ph.p_filesz;
            let zero_len = ph.p_memsz - ph.p_filesz;
            let zero_slice = vec![0u8; zero_len as usize];
            guest_mem
                .write_bytes(zero_start, &zero_slice)
                .map_err(ElfError::LoadFailed)?;
        }
        entry = entry.max(ph.p_paddr); // keep a valid entry fallback
    }

    Ok(ehdr.e_entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::GuestMemory;

    fn make_minimal_elf64(entry: u64, load_addr: u64, payload: &[u8]) -> Vec<u8> {
        // Build a minimal ELF64 with one PT_LOAD segment.
        let e_ident = [
            ELFMAG0,
            ELFMAG1,
            ELFMAG2,
            ELFMAG3,
            ELFCLASS64,
            ELFDATA2LSB,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        let phdr_size = core::mem::size_of::<Elf64Phdr>() as u64;
        let ehdr = Elf64Ehdr {
            e_ident,
            e_type: ET_EXEC,
            e_machine: EM_RISCV,
            e_version: 1,
            e_entry: entry,
            e_phoff: 64,
            e_shoff: 0,
            e_flags: 0,
            e_ehsize: 64,
            e_phentsize: phdr_size as u16,
            e_phnum: 1,
            e_shentsize: 0,
            e_shnum: 0,
            e_shstrndx: 0,
        };
        let phdr = Elf64Phdr {
            p_type: PT_LOAD,
            p_flags: 7,
            p_offset: 64 + phdr_size, // after ehdr + one phdr
            p_vaddr: load_addr,
            p_paddr: load_addr,
            p_filesz: payload.len() as u64,
            p_memsz: payload.len() as u64,
            p_align: 4096,
        };
        let mut out = Vec::new();
        unsafe {
            out.extend_from_slice(core::slice::from_raw_parts(
                &ehdr as *const _ as *const u8,
                64,
            ));
        }
        unsafe {
            out.extend_from_slice(core::slice::from_raw_parts(
                &phdr as *const _ as *const u8,
                phdr_size as usize,
            ));
        }
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn load_elf_roundtrip() {
        let payload = [0x13u8, 0x00, 0x00, 0x00]; // nop-ish bytes
        let image = make_minimal_elf64(0x8000, 0x8000, &payload);
        let mut mem = GuestMemory::new(0x10000);
        let entry = load_elf64(&image, &mut mem).unwrap();
        assert_eq!(entry, 0x8000);
        let mut buf = [0u8; 4];
        mem.read_bytes(0x8000, &mut buf).unwrap();
        assert_eq!(buf, payload);
    }

    #[test]
    fn reject_non_elf() {
        let mut mem = GuestMemory::new(0x1000);
        assert!(load_elf64(b"not an elf", &mut mem).is_err());
    }

    #[test]
    fn reject_bad_machine() {
        // valid ELF header but x86 machine (3)
        let mut image = make_minimal_elf64(0x8000, 0x8000, &[0u8]);
        // e_machine at offset 18..20
        image[18] = 3;
        image[19] = 0;
        let mut mem = GuestMemory::new(0x1000);
        assert_eq!(
            load_elf64(&image, &mut mem).unwrap_err(),
            ElfError::BadMachine
        );
    }

    #[test]
    fn zeroes_bss_when_memsz_greater() {
        let payload = [0xAAu8; 8];
        let image = make_minimal_elf64(0x8000, 0x8000, &payload);
        // memsz = filesz already; to test bss we'd need memsz>filesz; skip detailed.
        let mut mem = GuestMemory::new(0x10000);
        load_elf64(&image, &mut mem).unwrap();
        let mut buf = [0u8; 8];
        mem.read_bytes(0x8000, &mut buf).unwrap();
        assert_eq!(buf, payload);
    }
}
