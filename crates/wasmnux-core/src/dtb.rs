//! Flattened Device Tree (FDT) generation for the WASMNux `virt` machine.
//! The kernel discovers RAM, CPUs, PLIC, CLINT, UART, VirtIO-MMIO and the
//! optional framebuffer exclusively from this blob — exactly the role QEMU's
//! generated DTB plays. Encoder per `devicetree` spec §5 (version 17).

use std::collections::HashMap;

const FDT_MAGIC: u32 = 0xD00D_FEED;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_END: u32 = 9;
const FDT_VERSION: u32 = 17;
const FDT_LAST_COMPAT: u32 = 16;

/// Incremental FDT builder.
#[derive(Debug, Default)]
pub struct FdtBuilder {
    structure: Vec<u8>,
    strings: Vec<u8>,
    string_offsets: HashMap<String, u32>,
    reservations: Vec<(u64, u64)>,
}

fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_be_bytes());
}

fn pad_to_4(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

impl FdtBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserves a physical memory range (e.g. the framebuffer carve-out).
    pub fn reserve(&mut self, address: u64, size: u64) {
        self.reservations.push((address, size));
    }

    fn string_offset(&mut self, name: &str) -> u32 {
        if let Some(&offset) = self.string_offsets.get(name) {
            return offset;
        }
        let offset = self.strings.len() as u32;
        self.strings.extend_from_slice(name.as_bytes());
        self.strings.push(0);
        self.string_offsets.insert(name.to_owned(), offset);
        offset
    }

    pub fn begin_node(&mut self, name: &str) {
        push_u32(&mut self.structure, FDT_BEGIN_NODE);
        self.structure.extend_from_slice(name.as_bytes());
        self.structure.push(0);
        pad_to_4(&mut self.structure);
    }

    pub fn end_node(&mut self) {
        push_u32(&mut self.structure, FDT_END_NODE);
    }

    pub fn property_bytes(&mut self, name: &str, value: &[u8]) {
        let name_offset = self.string_offset(name);
        push_u32(&mut self.structure, FDT_PROP);
        push_u32(&mut self.structure, value.len() as u32);
        push_u32(&mut self.structure, name_offset);
        self.structure.extend_from_slice(value);
        pad_to_4(&mut self.structure);
    }

    pub fn property_empty(&mut self, name: &str) {
        self.property_bytes(name, &[]);
    }

    pub fn property_u32(&mut self, name: &str, value: u32) {
        self.property_bytes(name, &value.to_be_bytes());
    }

    pub fn property_u64(&mut self, name: &str, value: u64) {
        self.property_bytes(name, &value.to_be_bytes());
    }

    pub fn property_cells(&mut self, name: &str, cells: &[u32]) {
        let mut bytes = Vec::with_capacity(cells.len() * 4);
        for cell in cells {
            bytes.extend_from_slice(&cell.to_be_bytes());
        }
        self.property_bytes(name, &bytes);
    }

    /// A two-cell `reg` entry (address + size) for `#address-cells = 2`.
    pub fn property_reg(&mut self, name: &str, address: u64, size: u64) {
        self.property_cells(
            name,
            &[
                (address >> 32) as u32,
                (address & 0xFFFF_FFFF) as u32,
                (size >> 32) as u32,
                (size & 0xFFFF_FFFF) as u32,
            ],
        );
    }

    pub fn property_string(&mut self, name: &str, value: &str) {
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        self.property_bytes(name, &bytes);
    }

    /// A NUL-separated string list (`compatible` style).
    pub fn property_string_list(&mut self, name: &str, values: &[&str]) {
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(value.as_bytes());
            bytes.push(0);
        }
        self.property_bytes(name, &bytes);
    }

    /// Serialises the finished FDT (header, reservation map, structure,
    /// strings). The caller must have closed the root node.
    pub fn finish(mut self) -> Vec<u8> {
        push_u32(&mut self.structure, FDT_END);
        let header_size = 40_u32;
        let rsvmap_size = ((self.reservations.len() + 1) * 16) as u32;
        let struct_offset = header_size + rsvmap_size;
        let strings_offset = struct_offset + self.structure.len() as u32;
        let total = strings_offset + self.strings.len() as u32;

        let mut out = Vec::with_capacity(total as usize);
        push_u32(&mut out, FDT_MAGIC);
        push_u32(&mut out, total);
        push_u32(&mut out, struct_offset);
        push_u32(&mut out, strings_offset);
        push_u32(&mut out, header_size); // off_mem_rsvmap
        push_u32(&mut out, FDT_VERSION);
        push_u32(&mut out, FDT_LAST_COMPAT);
        push_u32(&mut out, 0); // boot_cpuid_phys
        push_u32(&mut out, self.strings.len() as u32);
        push_u32(&mut out, self.structure.len() as u32);
        for (address, size) in &self.reservations {
            out.extend_from_slice(&address.to_be_bytes());
            out.extend_from_slice(&size.to_be_bytes());
        }
        out.extend_from_slice(&0_u64.to_be_bytes());
        out.extend_from_slice(&0_u64.to_be_bytes());
        out.extend_from_slice(&self.structure);
        out.extend_from_slice(&self.strings);
        debug_assert_eq!(out.len(), total as usize);
        out
    }
}

/// Everything the DTB needs to describe one machine instance.
#[derive(Debug, Clone)]
pub struct MachineDtbParams {
    pub ram_base: u64,
    pub ram_size: u64,
    pub bootargs: String,
    pub initrd: Option<(u64, u64)>,
    pub framebuffer: Option<FramebufferInfo>,
}

/// A `simple-framebuffer` region at the top of RAM.
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    pub address: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

impl FramebufferInfo {
    pub fn size_bytes(&self) -> u64 {
        u64::from(self.stride) * u64::from(self.height)
    }
}

const PHANDLE_CPU_INTC: u32 = 1;
const PHANDLE_PLIC: u32 = 2;

/// Builds the complete DTB for this machine.
#[must_use]
pub fn build_machine_dtb(params: &MachineDtbParams) -> Vec<u8> {
    use crate::bus::{
        CLINT_BASE, CLINT_SIZE, IRQ_UART, IRQ_VIRTIO, PLIC_BASE, PLIC_SIZE, UART_BASE, UART_SIZE,
        VIRTIO_BASE, VIRTIO_SIZE,
    };
    use crate::clint::TIMEBASE_FREQUENCY;

    let mut fdt = FdtBuilder::new();
    if let Some(fb) = &params.framebuffer {
        fdt.reserve(fb.address, fb.size_bytes());
    }
    fdt.begin_node("");
    fdt.property_string_list("compatible", &["wasmnux,virt", "riscv-virtio"]);
    fdt.property_string("model", "WASMNux RISC-V virt");
    fdt.property_u32("#address-cells", 2);
    fdt.property_u32("#size-cells", 2);

    // /chosen
    fdt.begin_node("chosen");
    fdt.property_string("bootargs", &params.bootargs);
    fdt.property_string("stdout-path", "/soc/uart@10000000");
    if let Some((start, end)) = params.initrd {
        fdt.property_u64("linux,initrd-start", start);
        fdt.property_u64("linux,initrd-end", end);
    }
    fdt.end_node();

    // /memory
    fdt.begin_node(&format!("memory@{:x}", params.ram_base));
    fdt.property_string("device_type", "memory");
    fdt.property_reg("reg", params.ram_base, params.ram_size);
    fdt.end_node();

    // /cpus/cpu@0 (+ interrupt controller)
    fdt.begin_node("cpus");
    fdt.property_u32("#address-cells", 1);
    fdt.property_u32("#size-cells", 0);
    fdt.property_u32("timebase-frequency", TIMEBASE_FREQUENCY);
    fdt.begin_node("cpu@0");
    fdt.property_string("device_type", "cpu");
    fdt.property_u32("reg", 0);
    fdt.property_string("status", "okay");
    fdt.property_string("compatible", "riscv");
    // Must exactly match what the CPU advertises in misa: RV64IMAC + S/U.
    fdt.property_string("riscv,isa", "rv64imac_zicsr_zifencei");
    fdt.property_string("mmu-type", "riscv,sv39");
    fdt.begin_node("interrupt-controller");
    fdt.property_u32("#interrupt-cells", 1);
    fdt.property_empty("interrupt-controller");
    fdt.property_string("compatible", "riscv,cpu-intc");
    fdt.property_u32("phandle", PHANDLE_CPU_INTC);
    fdt.end_node();
    fdt.end_node();
    fdt.end_node();

    // /soc
    fdt.begin_node("soc");
    fdt.property_string("compatible", "simple-bus");
    fdt.property_u32("#address-cells", 2);
    fdt.property_u32("#size-cells", 2);
    fdt.property_empty("ranges");

    fdt.begin_node(&format!("clint@{CLINT_BASE:x}"));
    fdt.property_string_list("compatible", &["sifive,clint0", "riscv,clint0"]);
    fdt.property_reg("reg", CLINT_BASE, CLINT_SIZE);
    // <&cpu_intc 3 (MSIP)> <&cpu_intc 7 (MTIP)>
    fdt.property_cells(
        "interrupts-extended",
        &[PHANDLE_CPU_INTC, 3, PHANDLE_CPU_INTC, 7],
    );
    fdt.end_node();

    fdt.begin_node(&format!("plic@{PLIC_BASE:x}"));
    fdt.property_string_list("compatible", &["sifive,plic-1.0.0", "riscv,plic0"]);
    fdt.property_reg("reg", PLIC_BASE, PLIC_SIZE);
    fdt.property_u32("#interrupt-cells", 1);
    fdt.property_empty("interrupt-controller");
    // Context order matches plic.rs: M context first (MEI=11), then S (SEI=9).
    fdt.property_cells(
        "interrupts-extended",
        &[PHANDLE_CPU_INTC, 11, PHANDLE_CPU_INTC, 9],
    );
    fdt.property_u32("riscv,ndev", crate::plic::MAX_SOURCE);
    fdt.property_u32("phandle", PHANDLE_PLIC);
    fdt.end_node();

    fdt.begin_node(&format!("uart@{UART_BASE:x}"));
    fdt.property_string("compatible", "ns16550a");
    fdt.property_reg("reg", UART_BASE, UART_SIZE);
    fdt.property_u32("interrupts", IRQ_UART);
    fdt.property_u32("interrupt-parent", PHANDLE_PLIC);
    fdt.property_u32("clock-frequency", 1_843_200);
    fdt.property_u32("current-speed", 115_200);
    fdt.end_node();

    fdt.begin_node(&format!("virtio@{VIRTIO_BASE:x}"));
    fdt.property_string("compatible", "virtio,mmio");
    fdt.property_reg("reg", VIRTIO_BASE, VIRTIO_SIZE);
    fdt.property_u32("interrupts", IRQ_VIRTIO);
    fdt.property_u32("interrupt-parent", PHANDLE_PLIC);
    fdt.end_node();

    fdt.end_node(); // /soc

    if let Some(fb) = &params.framebuffer {
        fdt.begin_node("framebuffer");
        fdt.property_string("compatible", "simple-framebuffer");
        fdt.property_reg("reg", fb.address, fb.size_bytes());
        fdt.property_u32("width", fb.width);
        fdt.property_u32("height", fb.height);
        fdt.property_u32("stride", fb.stride);
        fdt.property_string("format", "x8r8g8b8");
        fdt.end_node();
    }

    fdt.end_node(); // /
    fdt.finish()
}

#[cfg(test)]
mod tests {
    use super::{build_machine_dtb, FramebufferInfo, MachineDtbParams};

    fn params() -> MachineDtbParams {
        MachineDtbParams {
            ram_base: 0x8000_0000,
            ram_size: 128 * 1024 * 1024,
            bootargs: "console=ttyS0 root=/dev/vda rw".to_owned(),
            initrd: Some((0x8400_0000, 0x8400_1000)),
            framebuffer: Some(FramebufferInfo {
                address: 0x87F0_0000,
                width: 1024,
                height: 600,
                stride: 1024 * 4,
            }),
        }
    }

    fn read_u32be(data: &[u8], at: usize) -> u32 {
        u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
    }

    #[test]
    fn header_is_consistent_with_the_blob() {
        let dtb = build_machine_dtb(&params());
        assert_eq!(read_u32be(&dtb, 0), 0xD00D_FEED);
        assert_eq!(read_u32be(&dtb, 4) as usize, dtb.len());
        let struct_offset = read_u32be(&dtb, 8) as usize;
        let strings_offset = read_u32be(&dtb, 12) as usize;
        let struct_size = read_u32be(&dtb, 36) as usize;
        let strings_size = read_u32be(&dtb, 32) as usize;
        assert_eq!(struct_offset + struct_size, strings_offset);
        assert_eq!(strings_offset + strings_size, dtb.len());
        assert_eq!(read_u32be(&dtb, 20), 17);
    }

    #[test]
    fn structure_block_describes_all_devices_and_terminates() {
        let dtb = build_machine_dtb(&params());
        let struct_offset = read_u32be(&dtb, 8) as usize;
        let struct_size = read_u32be(&dtb, 36) as usize;
        let block = &dtb[struct_offset..struct_offset + struct_size];

        // Walk tokens: count BEGIN_NODEs, ensure END is the final token.
        let mut at = 0_usize;
        let mut nodes = 0_usize;
        let mut depth = 0_i64;
        let mut last_token = 0_u32;
        let mut saw_end = false;
        while at + 4 <= block.len() {
            let token = read_u32be(block, at);
            last_token = token;
            at += 4;
            match token {
                1 => {
                    nodes += 1;
                    depth += 1;
                    while block[at] != 0 {
                        at += 1;
                    }
                    at = (at + 4) & !3;
                }
                2 => depth -= 1,
                3 => {
                    let len = read_u32be(block, at) as usize;
                    at += 8 + ((len + 3) & !3);
                }
                4 => {}
                9 => {
                    saw_end = true;
                    break;
                }
                other => panic!("unexpected FDT token {other}"),
            }
        }
        assert!(saw_end && last_token == 9);
        assert_eq!(depth, 0, "nodes must balance");
        // root, chosen, memory, cpus, cpu, intc, soc, clint, plic, uart,
        // virtio, framebuffer
        assert_eq!(nodes, 12);

        let as_text = String::from_utf8_lossy(&dtb);
        for needle in [
            "ns16550a",
            "virtio,mmio",
            "sifive,plic-1.0.0",
            "simple-framebuffer",
            "rv64imac",
        ] {
            assert!(as_text.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn framebuffer_reservation_is_present() {
        let dtb = build_machine_dtb(&params());
        // Reservation map starts right after the 40-byte header.
        let addr = u64::from_be_bytes(dtb[40..48].try_into().expect("addr"));
        let size = u64::from_be_bytes(dtb[48..56].try_into().expect("size"));
        assert_eq!(addr, 0x87F0_0000);
        assert_eq!(size, 1024 * 4 * 600);
    }
}
