//! Minimal Flattened Device Tree (FDT) emitter.
//!
//! Produces a binary device tree blob (FDT) describing a simple RISC-V
//! machine (one hart, CLINT timer, UART, memory). This is best-effort: a real
//! Linux boot needs a complete, board-accurate FDT plus an MMU, both of which
//! are documented as future extensions.

const FDT_MAGIC: u32 = 0xD00D_FEED;
const FDT_BEGIN_NODE: u32 = 0x00000001;
const FDT_END_NODE: u32 = 0x00000002;
const FDT_PROP: u32 = 0x00000003;
const FDT_END: u32 = 0x00000009;

/// A small FDT builder. All multi-byte integers are big-endian.
pub struct FdtBuilder {
    struct_buf: Vec<u8>,
    strings: Vec<String>,
    strings_block: Vec<u8>,
}

impl FdtBuilder {
    pub fn new() -> Self {
        Self {
            struct_buf: Vec::new(),
            strings: Vec::new(),
            strings_block: Vec::new(),
        }
    }

    fn str_offset(&mut self, s: &str) -> u32 {
        if let Some(i) = self.strings.iter().position(|x| x == s) {
            return i as u32;
        }
        let off = self.strings_block.len() as u32;
        self.strings.push(s.to_string());
        self.strings_block.extend_from_slice(s.as_bytes());
        self.strings_block.push(0);
        off
    }

    fn be_u32(&mut self, v: u32) {
        self.struct_buf.extend_from_slice(&v.to_be_bytes());
    }
    fn be_u64(&mut self, v: u64) {
        self.struct_buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn begin_node(&mut self, name: &str) {
        self.be_u32(FDT_BEGIN_NODE);
        self.struct_buf.extend_from_slice(name.as_bytes());
        self.struct_buf.push(0);
        self.pad_to(4);
    }

    pub fn end_node(&mut self) {
        self.be_u32(FDT_END_NODE);
    }

    pub fn prop_empty(&mut self, name: &str) {
        self.be_u32(FDT_PROP);
        self.be_u32(0); // len
        let nameoff = self.str_offset(name);
        self.be_u32(nameoff);
    }

    pub fn prop_u32(&mut self, name: &str, value: u32) {
        self.be_u32(FDT_PROP);
        self.be_u32(4);
        let nameoff = self.str_offset(name);
        self.be_u32(nameoff);
        self.struct_buf.extend_from_slice(&value.to_be_bytes());
    }

    pub fn prop_u64(&mut self, name: &str, value: u64) {
        self.be_u32(FDT_PROP);
        self.be_u32(8);
        let nameoff = self.str_offset(name);
        self.be_u32(nameoff);
        self.be_u64(value);
    }

    pub fn prop_string(&mut self, name: &str, value: &str) {
        let valoff = self.str_offset(value);
        let nameoff = self.str_offset(name);
        self.be_u32(FDT_PROP);
        self.be_u32(4);
        self.be_u32(nameoff);
        self.struct_buf.extend_from_slice(&valoff.to_be_bytes());
    }

    fn pad_to(&mut self, align: usize) {
        while !self.struct_buf.len().is_multiple_of(align) {
            self.struct_buf.push(0);
        }
    }

    pub fn finish(mut self) -> Vec<u8> {
        // End structure.
        self.be_u32(FDT_END);
        self.pad_to(8);

        let size_dt_struct = self.struct_buf.len() as u32;
        let size_dt_strings = self.strings_block.len() as u32;

        // Header (all big-endian). off_mem_rsvmap = 0 (empty map), size 0.
        // The header is padded to 0x700 bytes; the struct block follows it.
        let mut header = Vec::new();
        let total_size = (0x700usize)
            .checked_add(size_dt_struct as usize)
            .and_then(|s| s.checked_add(size_dt_strings as usize))
            .expect("fdt size overflow");
        header.extend_from_slice(&FDT_MAGIC.to_be_bytes());
        header.extend_from_slice(&(total_size as u32).to_be_bytes()); // totalsize
        header.extend_from_slice(&(0x700u32).to_be_bytes()); // off_dt_struct
        header.extend_from_slice(&((0x700 + size_dt_struct as usize) as u32).to_be_bytes()); // off_dt_strings
        header.extend_from_slice(&0u32.to_be_bytes()); // off_mem_rsvmap
        header.extend_from_slice(&17u32.to_be_bytes()); // version
        header.extend_from_slice(&16u32.to_be_bytes()); // last_comp_version
        header.extend_from_slice(&0u32.to_be_bytes()); // boot_cpuid_phys
        header.extend_from_slice(&size_dt_strings.to_be_bytes());
        header.extend_from_slice(&size_dt_struct.to_be_bytes());
        // pad header to 0x700
        while header.len() < 0x700 {
            header.push(0);
        }

        let mut out = header;
        out.extend_from_slice(&self.struct_buf);
        out.extend_from_slice(&self.strings_block);
        out
    }
}

impl Default for FdtBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fdt_header_magic_and_size() {
        let mut b = FdtBuilder::new();
        b.begin_node("/");
        b.prop_string("compatible", "riscv-virt");
        b.prop_u32("#address-cells", 2);
        b.prop_u32("#size-cells", 2);
        b.end_node();
        let blob = b.finish();
        assert!(blob.len() >= 0x700);
        let magic = u32::from_be_bytes([blob[0], blob[1], blob[2], blob[3]]);
        assert_eq!(magic, FDT_MAGIC);
        let total = u32::from_be_bytes([blob[4], blob[5], blob[6], blob[7]]) as usize;
        assert_eq!(total, blob.len());
        // Strings block should contain "riscv-virt".
        let s = String::from_utf8_lossy(&blob[0x700..]);
        assert!(s.contains("riscv-virt"));
    }

    #[test]
    fn fdt_memory_node() {
        let mut b = FdtBuilder::new();
        b.begin_node("/memory");
        b.prop_u64("reg", 0x8000_0000);
        b.prop_u64("reg", 0x4000_0000); // note: overwritten in real use; here just tests u64 emit
        b.end_node();
        let blob = b.finish();
        assert!(u32::from_be_bytes([blob[4], blob[5], blob[6], blob[7]]) as usize == blob.len());
    }
}
