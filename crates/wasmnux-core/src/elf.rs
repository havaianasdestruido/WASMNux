//! Minimal ELF64 little-endian loader for RISC-V firmware/kernel payloads
//! (e.g. OpenSBI `fw_jump.elf`). Only what the boot path needs: header
//! validation and `PT_LOAD` segments copied to their physical addresses.
//! Every field is bounds-checked; malformed input is an error, never UB.

use crate::error::{Result, VmError};
use crate::memory::Ram;

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELF_CLASS_64: u8 = 2;
const ELF_DATA_LE: u8 = 1;
const ELF_MACHINE_RISCV: u16 = 243;
const PT_LOAD: u32 = 1;

/// A parsed and validated ELF64 image.
#[derive(Debug)]
pub struct ElfImage<'a> {
    data: &'a [u8],
    entry: u64,
    program_headers: Vec<ProgramHeader>,
}

#[derive(Debug, Clone, Copy)]
struct ProgramHeader {
    offset: usize,
    paddr: u64,
    file_size: usize,
    mem_size: usize,
}

fn read_u16(data: &[u8], at: usize) -> Result<u16> {
    data.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or(VmError::InvalidElf("truncated header"))
}

fn read_u32(data: &[u8], at: usize) -> Result<u32> {
    data.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(VmError::InvalidElf("truncated header"))
}

fn read_u64(data: &[u8], at: usize) -> Result<u64> {
    data.get(at..at + 8)
        .map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
        .ok_or(VmError::InvalidElf("truncated header"))
}

impl<'a> ElfImage<'a> {
    /// Parses and validates an ELF64 LE RISC-V image.
    ///
    /// # Errors
    /// [`VmError::InvalidElf`] on any structural problem.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        if data.len() < 64 || data[0..4] != ELF_MAGIC {
            return Err(VmError::InvalidElf("missing ELF magic"));
        }
        if data[4] != ELF_CLASS_64 {
            return Err(VmError::InvalidElf("not a 64-bit ELF"));
        }
        if data[5] != ELF_DATA_LE {
            return Err(VmError::InvalidElf("not little-endian"));
        }
        if read_u16(data, 18)? != ELF_MACHINE_RISCV {
            return Err(VmError::InvalidElf("not a RISC-V ELF"));
        }
        let entry = read_u64(data, 24)?;
        let ph_offset = usize::try_from(read_u64(data, 32)?)
            .map_err(|_| VmError::InvalidElf("program header offset overflow"))?;
        let ph_entry_size = read_u16(data, 54)? as usize;
        let ph_count = read_u16(data, 56)? as usize;
        if ph_entry_size < 56 {
            return Err(VmError::InvalidElf("program header entry too small"));
        }
        let mut program_headers = Vec::new();
        for i in 0..ph_count {
            let at = ph_offset
                .checked_add(
                    i.checked_mul(ph_entry_size)
                        .ok_or(VmError::InvalidElf("program header offset overflow"))?,
                )
                .ok_or(VmError::InvalidElf("program header offset overflow"))?;
            if read_u32(data, at)? != PT_LOAD {
                continue;
            }
            let offset = usize::try_from(read_u64(data, at + 8)?)
                .map_err(|_| VmError::InvalidElf("segment offset overflow"))?;
            let paddr = read_u64(data, at + 24)?;
            let file_size = usize::try_from(read_u64(data, at + 32)?)
                .map_err(|_| VmError::InvalidElf("segment size overflow"))?;
            let mem_size = usize::try_from(read_u64(data, at + 40)?)
                .map_err(|_| VmError::InvalidElf("segment size overflow"))?;
            if mem_size < file_size {
                return Err(VmError::InvalidElf("memsz smaller than filesz"));
            }
            if offset
                .checked_add(file_size)
                .is_none_or(|end| end > data.len())
            {
                return Err(VmError::InvalidElf("segment extends past end of file"));
            }
            program_headers.push(ProgramHeader {
                offset,
                paddr,
                file_size,
                mem_size,
            });
        }
        if program_headers.is_empty() {
            return Err(VmError::InvalidElf("no PT_LOAD segments"));
        }
        Ok(Self {
            data,
            entry,
            program_headers,
        })
    }

    /// The ELF entry point.
    pub fn entry(&self) -> u64 {
        self.entry
    }

    /// Copies all `PT_LOAD` segments into guest RAM, zero-filling BSS.
    ///
    /// # Errors
    /// [`VmError::OutOfBounds`] if a segment does not fit in RAM.
    pub fn load_into(&self, ram: &mut Ram) -> Result<()> {
        for ph in &self.program_headers {
            let bytes = &self.data[ph.offset..ph.offset + ph.file_size];
            ram.write(ph.paddr, bytes)?;
            let bss = ph.mem_size - ph.file_size;
            if bss > 0 {
                ram.write(ph.paddr + ph.file_size as u64, &vec![0_u8; bss])?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ElfImage;
    use crate::memory::Ram;

    /// Builds a synthetic minimal ELF64 with one PT_LOAD segment.
    fn synth_elf(entry: u64, paddr: u64, payload: &[u8], mem_extra: usize) -> Vec<u8> {
        let mut elf = vec![0_u8; 64 + 56];
        elf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        elf[4] = 2;
        elf[5] = 1;
        elf[16..18].copy_from_slice(&2_u16.to_le_bytes());
        elf[18..20].copy_from_slice(&243_u16.to_le_bytes());
        elf[24..32].copy_from_slice(&entry.to_le_bytes());
        elf[32..40].copy_from_slice(&64_u64.to_le_bytes());
        elf[52..54].copy_from_slice(&64_u16.to_le_bytes());
        elf[54..56].copy_from_slice(&56_u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1_u16.to_le_bytes());
        let ph = 64;
        elf[ph..ph + 4].copy_from_slice(&1_u32.to_le_bytes());
        elf[ph + 8..ph + 16].copy_from_slice(&(120_u64).to_le_bytes());
        elf[ph + 24..ph + 32].copy_from_slice(&paddr.to_le_bytes());
        elf[ph + 32..ph + 40].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        elf[ph + 40..ph + 48].copy_from_slice(&((payload.len() + mem_extra) as u64).to_le_bytes());
        elf.extend_from_slice(payload);
        elf
    }

    #[test]
    fn loads_segment_and_zero_fills_bss() {
        let elf = synth_elf(0x8000_0000, 0x8000_0000, &[1, 2, 3, 4], 4);
        let image = ElfImage::parse(&elf).expect("parse");
        assert_eq!(image.entry(), 0x8000_0000);
        let mut ram = Ram::new(0x8000_0000, 0x1000).expect("ram");
        ram.write_int(0x8000_0004, 4, 0xFFFF_FFFF)
            .expect("pre-dirty BSS");
        image.load_into(&mut ram).expect("load");
        let mut buf = [0_u8; 8];
        ram.read(0x8000_0000, &mut buf).expect("read back");
        assert_eq!(buf, [1, 2, 3, 4, 0, 0, 0, 0]);
    }

    #[test]
    fn rejects_garbage_and_wrong_architecture() {
        assert!(ElfImage::parse(b"not an elf at all, definitely").is_err());
        let mut elf = synth_elf(0x8000_0000, 0x8000_0000, &[0], 0);
        elf[18..20].copy_from_slice(&62_u16.to_le_bytes()); // x86-64
        assert!(ElfImage::parse(&elf).is_err());
        let truncated = synth_elf(0x8000_0000, 0x8000_0000, &[1, 2, 3, 4], 0);
        assert!(ElfImage::parse(&truncated[..100]).is_err());
    }

    #[test]
    fn rejects_segments_outside_ram() {
        let elf = synth_elf(0x8000_0000, 0x9000_0000, &[1, 2, 3, 4], 0);
        let image = ElfImage::parse(&elf).expect("parse");
        let mut ram = Ram::new(0x8000_0000, 0x1000).expect("ram");
        assert!(image.load_into(&mut ram).is_err());
    }
}
