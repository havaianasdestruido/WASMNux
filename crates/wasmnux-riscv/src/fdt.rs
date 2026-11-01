//! Flattened Device Tree (`DTB`) binary generator for the WASMNux `riscv64` virtual machine.

use std::collections::HashMap;

pub const FDT_MAGIC: u32 = 0xD00D_FEED;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_END: u32 = 9;

#[derive(Debug, Clone)]
pub struct VirtMachineFdtConfig<'a> {
    pub ram_base: u64,
    pub ram_size: u64,
    pub isa: &'a str,
    pub bootargs: &'a str,
    pub initrd: Option<(u64, u64)>,
    pub rng_seed: [u8; 32],
    pub virtio_mmio_slots: &'a [(u64, u32)],
    pub framebuffer: Option<(u64, u32, u32)>,
}

pub fn build_virt_dtb(cfg: &VirtMachineFdtConfig<'_>) -> Vec<u8> {
    let mut fdt = FdtBuilder::new();

    let plic_phandle = 1u32;
    let syscon_phandle = 2u32;
    let cpu_phandle = 0x100u32;
    let cpu_intc_phandle = 0x200u32;

    fdt.begin_node("");
    fdt.prop_str_list("compatible", &["riscv-virtio", "qemu,virt"]);
    fdt.prop_u32("#address-cells", 2);
    fdt.prop_u32("#size-cells", 2);
    fdt.prop_str("model", "WASMNux RV64GC Virtual Machine");

    fdt.begin_node("chosen");
    fdt.prop_str("stdout-path", "/soc/uart@10000000");
    fdt.prop_u32("boot-hartid", 0);
    if !cfg.bootargs.is_empty() {
        fdt.prop_str("bootargs", cfg.bootargs);
    }
    if let Some((start, end)) = cfg.initrd {
        fdt.prop_u64("linux,initrd-start", start);
        fdt.prop_u64("linux,initrd-end", end);
    }
    fdt.prop_bytes("rng-seed", &cfg.rng_seed);
    fdt.end_node();

    fdt.begin_node("aliases");
    fdt.prop_str("serial0", "/soc/uart@10000000");
    fdt.end_node();

    fdt.begin_node("cpus");
    fdt.prop_u32("#address-cells", 1);
    fdt.prop_u32("#size-cells", 0);
    fdt.prop_u32("timebase-frequency", 10_000_000);

    fdt.begin_node("cpu@0");
    fdt.prop_str("device_type", "cpu");
    fdt.prop_str("compatible", "riscv");
    fdt.prop_u32("reg", 0);
    fdt.prop_str("riscv,isa", cfg.isa);
    fdt.prop_str("riscv,isa-base", "rv64i");
    fdt.prop_str_list(
        "riscv,isa-extensions",
        &[
            "i", "m", "a", "f", "d", "c", "zicsr", "zifencei", "zicntr", "zba", "zbb",
        ],
    );
    fdt.prop_str("mmu-type", "riscv,sv39");
    fdt.prop_str("status", "okay");
    fdt.prop_u32("phandle", cpu_phandle);

    fdt.begin_node("interrupt-controller");
    fdt.prop_u32("#interrupt-cells", 1);
    fdt.prop_empty("interrupt-controller");
    fdt.prop_str("compatible", "riscv,cpu-intc");
    fdt.prop_u32("phandle", cpu_intc_phandle);
    fdt.end_node();

    fdt.end_node(); // cpu@0
    fdt.end_node(); // cpus

    let mem_node = format!("memory@{:x}", cfg.ram_base);
    fdt.begin_node(&mem_node);
    fdt.prop_str("device_type", "memory");
    fdt.prop_u64s("reg", &[cfg.ram_base, cfg.ram_size]);
    fdt.end_node();

    if let Some((fb_base, width, height)) = cfg.framebuffer {
        let stride = width * 4;
        let fb_size = u64::from(stride) * u64::from(height);
        let fb_node = format!("framebuffer@{fb_base:x}");
        fdt.begin_node(&fb_node);
        fdt.prop_str("compatible", "simple-framebuffer");
        fdt.prop_u64s("reg", &[fb_base, fb_size]);
        fdt.prop_u32("width", width);
        fdt.prop_u32("height", height);
        fdt.prop_u32("stride", stride);
        fdt.prop_str("format", "x8r8g8b8");
        fdt.prop_str("status", "okay");
        fdt.end_node();
    }

    fdt.begin_node("soc");
    fdt.prop_u32("#address-cells", 2);
    fdt.prop_u32("#size-cells", 2);
    fdt.prop_str("compatible", "simple-bus");
    fdt.prop_empty("ranges");

    // Syscon / SiFive test finisher (`poweroff` & `reboot`)
    fdt.begin_node("test@100000");
    fdt.prop_str_list("compatible", &["sifive,test1", "sifive,test0", "syscon"]);
    fdt.prop_u64s("reg", &[0x0010_0000, 0x1000]);
    fdt.prop_u32("phandle", syscon_phandle);
    fdt.end_node();

    // Goldfish wall-clock RTC
    fdt.begin_node("rtc@101000");
    fdt.prop_str("compatible", "google,goldfish-rtc");
    fdt.prop_u64s("reg", &[0x0010_1000, 0x1000]);
    fdt.prop_u32("interrupt-parent", plic_phandle);
    fdt.prop_u32("interrupts", 11);
    fdt.end_node();

    // Core-Local Interruptor (CLINT)
    fdt.begin_node("clint@2000000");
    fdt.prop_str_list("compatible", &["sifive,clint0", "riscv,clint0"]);
    fdt.prop_u64s("reg", &[0x0200_0000, 0x0001_0000]);
    fdt.prop_u32s(
        "interrupts-extended",
        &[cpu_intc_phandle, 3, cpu_intc_phandle, 7],
    );
    fdt.end_node();

    // Platform-Level Interrupt Controller (PLIC)
    fdt.begin_node("plic@c000000");
    fdt.prop_u32("phandle", plic_phandle);
    fdt.prop_str_list("compatible", &["sifive,plic-1.0.0", "riscv,plic0"]);
    fdt.prop_u32("#address-cells", 0);
    fdt.prop_u32("#interrupt-cells", 1);
    fdt.prop_empty("interrupt-controller");
    fdt.prop_u64s("reg", &[0x0C00_0000, 0x0400_0000]);
    fdt.prop_u32("riscv,ndev", 31);
    fdt.prop_u32s(
        "interrupts-extended",
        &[cpu_intc_phandle, 11, cpu_intc_phandle, 9],
    );
    fdt.end_node();

    // NS16550A UART Serial Console
    fdt.begin_node("uart@10000000");
    fdt.prop_str("compatible", "ns16550a");
    fdt.prop_u64s("reg", &[0x1000_0000, 0x100]);
    fdt.prop_u32("interrupt-parent", plic_phandle);
    fdt.prop_u32("interrupts", 10);
    fdt.prop_u32("clock-frequency", 3_686_400);
    fdt.prop_u32("reg-shift", 0);
    fdt.prop_u32("reg-io-width", 1);
    fdt.end_node();

    // VirtIO MMIO v2 transports (declared in reverse address order so Linux names vda @ 0x10001000 first)
    for &(base, irq) in cfg.virtio_mmio_slots.iter().rev() {
        let node = format!("virtio_mmio@{base:x}");
        fdt.begin_node(&node);
        fdt.prop_str("compatible", "virtio,mmio");
        fdt.prop_u64s("reg", &[base, 0x1000]);
        fdt.prop_u32("interrupt-parent", plic_phandle);
        fdt.prop_u32("interrupts", irq);
        fdt.end_node();
    }

    fdt.end_node(); // soc

    fdt.begin_node("poweroff");
    fdt.prop_str("compatible", "syscon-poweroff");
    fdt.prop_u32("regmap", syscon_phandle);
    fdt.prop_u32("offset", 0);
    fdt.prop_u32("value", 0x5555);
    fdt.end_node();

    fdt.begin_node("reboot");
    fdt.prop_str("compatible", "syscon-reboot");
    fdt.prop_u32("regmap", syscon_phandle);
    fdt.prop_u32("offset", 0);
    fdt.prop_u32("value", 0x7777);
    fdt.end_node();

    fdt.end_node(); // root
    fdt.finish()
}

struct FdtBuilder {
    struct_buf: Vec<u8>,
    strings_buf: Vec<u8>,
    str_offsets: HashMap<String, u32>,
}

impl FdtBuilder {
    fn new() -> Self {
        Self {
            struct_buf: Vec::with_capacity(4096),
            strings_buf: Vec::with_capacity(1024),
            str_offsets: HashMap::new(),
        }
    }

    fn begin_node(&mut self, name: &str) {
        self.push_u32(FDT_BEGIN_NODE);
        self.struct_buf.extend_from_slice(name.as_bytes());
        self.struct_buf.push(0);
        self.pad4();
    }

    fn end_node(&mut self) {
        self.push_u32(FDT_END_NODE);
    }

    fn prop_empty(&mut self, name: &str) {
        self.prop_bytes(name, &[]);
    }

    fn prop_u32(&mut self, name: &str, v: u32) {
        self.prop_bytes(name, &v.to_be_bytes());
    }

    fn prop_u64(&mut self, name: &str, v: u64) {
        self.prop_bytes(name, &v.to_be_bytes());
    }

    fn prop_u32s(&mut self, name: &str, vals: &[u32]) {
        let mut b = Vec::with_capacity(vals.len() * 4);
        for &v in vals {
            b.extend_from_slice(&v.to_be_bytes());
        }
        self.prop_bytes(name, &b);
    }

    fn prop_u64s(&mut self, name: &str, vals: &[u64]) {
        let mut b = Vec::with_capacity(vals.len() * 8);
        for &v in vals {
            b.extend_from_slice(&v.to_be_bytes());
        }
        self.prop_bytes(name, &b);
    }

    fn prop_str(&mut self, name: &str, val: &str) {
        let mut b = Vec::with_capacity(val.len() + 1);
        b.extend_from_slice(val.as_bytes());
        b.push(0);
        self.prop_bytes(name, &b);
    }

    fn prop_str_list(&mut self, name: &str, list: &[&str]) {
        let mut b = Vec::new();
        for s in list {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        self.prop_bytes(name, &b);
    }

    fn prop_bytes(&mut self, name: &str, val: &[u8]) {
        let nameoff = self.intern_str(name);
        self.push_u32(FDT_PROP);
        self.push_u32(val.len() as u32);
        self.push_u32(nameoff);
        self.struct_buf.extend_from_slice(val);
        self.pad4();
    }

    fn intern_str(&mut self, name: &str) -> u32 {
        if let Some(&off) = self.str_offsets.get(name) {
            return off;
        }
        let off = self.strings_buf.len() as u32;
        self.strings_buf.extend_from_slice(name.as_bytes());
        self.strings_buf.push(0);
        self.str_offsets.insert(name.to_owned(), off);
        off
    }

    fn push_u32(&mut self, v: u32) {
        self.struct_buf.extend_from_slice(&v.to_be_bytes());
    }

    fn pad4(&mut self) {
        while (self.struct_buf.len() & 3) != 0 {
            self.struct_buf.push(0);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        self.push_u32(FDT_END);

        let header_size = 40u32;
        let mem_rsvmap_off = header_size;
        let mem_rsvmap_size = 16u32;
        let dt_struct_off = mem_rsvmap_off + mem_rsvmap_size;
        let dt_struct_size = self.struct_buf.len() as u32;
        let dt_strings_off = dt_struct_off + dt_struct_size;
        let dt_strings_size = self.strings_buf.len() as u32;
        let total_size = dt_strings_off + dt_strings_size;

        let mut out = Vec::with_capacity(total_size as usize);
        for v in [
            FDT_MAGIC,
            total_size,
            dt_struct_off,
            dt_strings_off,
            mem_rsvmap_off,
            17, // version
            16, // last_comp_version
            0,  // boot_cpuid_phys
            dt_strings_size,
            dt_struct_size,
        ] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(&[0u8; 16]); // Empty mem_rsvmap terminator
        out.extend_from_slice(&self.struct_buf);
        out.extend_from_slice(&self.strings_buf);
        out
    }
}
