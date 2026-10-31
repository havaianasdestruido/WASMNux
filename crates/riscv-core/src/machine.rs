//! Platform-independent full-system RV64 machine and bounded execution.
use crate::cpu::Cpu;
use crate::dev::{Clint, GoldfishRtc, Plic, Syscon, Uart, UartPeer};
use crate::fdt::Fdt;
use crate::isa::Exception;
use crate::isa::{reg, Mode};
use crate::memory::{Memory, RAM_BASE};
pub const FB_WIDTH: u32 = 640;
pub const FB_HEIGHT: u32 = 400;
pub const FB_BASE: u64 = 0x8f000000;
pub const FB_SIZE: usize = (FB_WIDTH * FB_HEIGHT * 4) as usize;
use crate::mmu::Mmio;
use crate::snapshot::{StateReader, StateWriter};
use crate::virtio::{self, VirtioEntropy, VirtioMmio};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
pub trait BlockDevice: Send {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), String>;
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), String>;
    fn flush(&mut self) -> Result<(), String>;
}
pub trait Entropy: Send {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), String>;
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineError {
    InvalidConfig,
    InvalidImage,
    Allocation,
    InvalidState,
    Device,
}
impl std::fmt::Display for MachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MachineError {}
#[derive(Clone)]
pub struct MachineConfig {
    pub ram_bytes: u64,
    pub disk_sectors: u64,
    pub cmdline: String,
}
pub struct Console {
    input: Mutex<VecDeque<u8>>,
    pub output: Mutex<Vec<u8>>,
    pub overflow: Mutex<bool>,
}
impl Default for Console {
    fn default() -> Self {
        Self {
            input: Mutex::new(VecDeque::new()),
            output: Mutex::new(Vec::new()),
            overflow: Mutex::new(false),
        }
    }
}
impl Console {
    pub fn input(&self, b: &[u8]) -> Result<(), MachineError> {
        let mut q = self.input.lock().map_err(|_| MachineError::InvalidState)?;
        if q.len() + b.len() > 65536 {
            return Err(MachineError::InvalidConfig);
        }
        q.extend(b);
        Ok(())
    }
    pub fn drain(&self) -> Vec<u8> {
        self.output
            .lock()
            .map(|mut x| std::mem::take(&mut *x))
            .unwrap_or_default()
    }
}
impl UartPeer for Console {
    fn run(&self, _: Uart) {}
    fn halt(&self) -> Result<(), String> {
        Ok(())
    }
    fn receive(&self) -> (u8, bool) {
        let mut q = self.input.lock().unwrap();
        let b = q.pop_front().unwrap_or(0);
        (b, !q.is_empty())
    }
    fn send(&self, b: u8) {
        let mut out = self.output.lock().unwrap();
        if out.len() < 1024 * 1024 {
            out.push(b)
        } else {
            *self.overflow.lock().unwrap() = true
        }
    }
}
#[derive(Default)]
struct Bus {
    devices: Vec<Box<dyn Mmio>>,
}
impl Bus {
    fn add(&mut self, d: impl Mmio + 'static) {
        self.devices.push(Box::new(d))
    }
}
impl Mmio for Bus {
    fn tick(&mut self, now: u64) {
        for d in &mut self.devices {
            d.tick(now);
        }
    }
    fn contains(&self, a: u64, n: u64) -> bool {
        self.devices.iter().any(|d| d.contains(a, n))
    }
    fn load(&mut self, t: u64, a: u64, n: u64) -> Result<u64, Exception> {
        self.devices
            .iter_mut()
            .find(|d| d.contains(a, n))
            .ok_or(Exception::fatal("unmapped MMIO read"))?
            .load(t, a, n)
    }
    fn store(&mut self, t: u64, a: u64, n: u64, v: u64) -> Result<(), Exception> {
        self.devices
            .iter_mut()
            .find(|d| d.contains(a, n))
            .ok_or(Exception::fatal("unmapped MMIO write"))?
            .store(t, a, n, v)
    }
    fn halt(&mut self) -> Result<(), String> {
        for d in &mut self.devices {
            d.halt()?
        }
        Ok(())
    }
    fn save_state(&mut self, w: &mut StateWriter) -> Result<(), String> {
        for d in &mut self.devices {
            d.save_state(w)?;
        }
        Ok(())
    }
    fn load_state(&mut self, r: &mut StateReader) -> Result<(), String> {
        for d in &mut self.devices {
            d.load_state(r)?;
        }
        Ok(())
    }
}
pub struct Machine {
    pub cpu: Cpu,
    pub console: Arc<Console>,
    uart: Uart,
    pub(crate) virtio: Vec<VirtioMmio>,
    pub powered_off: bool,
}
impl Machine {
    pub fn new(
        cfg: &MachineConfig,
        firmware: &[u8],
        kernel: &[u8],
        initrd: &[u8],
        disk: Arc<Mutex<dyn BlockDevice>>,
        entropy: Arc<Mutex<dyn Entropy>>,
    ) -> Result<Self, MachineError> {
        if cfg.ram_bytes < 64 * 1024 * 1024
            || cfg.ram_bytes > 1024 * 1024 * 1024
            || firmware.is_empty()
            || firmware.len() > 2 * 1024 * 1024
            || kernel.is_empty()
            || kernel.len() > 64 * 1024 * 1024
            || initrd.len() > 64 * 1024 * 1024
            || cfg.cmdline.len() > 4096
        {
            return Err(MachineError::InvalidConfig);
        }
        let mem = Arc::new(
            Memory::try_new(RAM_BASE, cfg.ram_bytes).map_err(|_| MachineError::Allocation)?,
        );
        let mut cpu = Cpu::new(mem.clone());
        let plic = Plic::new(vec![cpu.signals.clone()], 0x0c000000);
        let console = Arc::new(Console::default());
        let uart = Uart::new("ttyS0", 0x10000000, plic.clone(), 10, console.clone());
        let mut bus = Bus::default();
        bus.add(plic.clone());
        bus.add(uart.clone());
        bus.add(Clint::new(cpu.signals.clone(), 0x02000000, 0x10000));
        bus.add(Syscon::new(cpu.signals.clone(), 0x10000100, 256));
        bus.add(GoldfishRtc::new(0x10100000, 4096));
        let rng = VirtioEntropy::create(0x10008000, plic.clone(), 1, mem.clone(), entropy);
        bus.add(rng.clone());
        let blk = virtio::block::create(
            0x10009000,
            plic.clone(),
            2,
            mem.clone(),
            disk,
            cfg.disk_sectors,
            false,
        );
        bus.add(blk.clone());
        let virtio = vec![rng, blk];
        cpu.bus = Some(Box::new(bus));
        cpu.set_mode(Mode::M);
        for (addr, data) in [
            (0x80000000, firmware),
            (0x80200000, kernel),
            (0x88000000, initrd),
        ] {
            if data.is_empty() {
                continue;
            }
            if !mem.contains_range(addr, data.len() as u64) {
                return Err(MachineError::InvalidImage);
            }
            mem.write_bytes(mem.offset(addr), data)
        }
        let dtb = device_tree(cfg, mem.size(), initrd.len() as u64);
        if !mem.contains_range(0x82200000, dtb.len() as u64) {
            return Err(MachineError::InvalidImage);
        }
        mem.write_bytes(mem.offset(0x82200000), &dtb);
        cpu.pc = 0x80000000;
        cpu.x[reg::A0 as usize] = 0;
        cpu.x[reg::A1 as usize] = 0x82200000;
        Ok(Self {
            cpu,
            console,
            uart,
            virtio,
            powered_off: false,
        })
    }
    /// Saves CPU, devices and RAM. Call only between bounded execution slices.
    pub fn save_state(&mut self, cfg: &MachineConfig) -> Result<Vec<u8>, String> {
        for d in &mut self.virtio {
            d.quiesce().map_err(|e| e.to_string())?;
        }
        let mut w = StateWriter::new();
        w.bytes(b"WNXSNAP1");
        w.u64(cfg.ram_bytes);
        w.u64(cfg.disk_sectors);
        w.bytes(cfg.cmdline.as_bytes());
        w.section("cpu", |w| {
            self.cpu.save_state(w);
            Ok(())
        })?;
        w.section("bus", |w| self.cpu.bus.as_mut().unwrap().save_state(w))?;
        w.section("ram", |w| {
            w.bytes(unsafe { self.cpu.mem.ram() });
            Ok(())
        })?;
        w.bool(self.powered_off);
        for d in &mut self.virtio {
            d.resume();
        }
        Ok(w.into_bytes())
    }
    pub fn load_state(&mut self, cfg: &MachineConfig, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > 1024 * 1024 * 1024 + 1024 * 1024 {
            return Err("snapshot too large".into());
        }
        let mut r = StateReader::new(bytes);
        if r.bytes()? != b"WNXSNAP1" {
            return Err("invalid snapshot magic".into());
        }
        if r.u64()? != cfg.ram_bytes
            || r.u64()? != cfg.disk_sectors
            || r.bytes()? != cfg.cmdline.as_bytes()
        {
            return Err("snapshot configuration mismatch".into());
        }
        // Validate into a temporary CPU state before touching RAM? CPU load is architectural and RAM copy follows.
        r.section("cpu", |r| self.cpu.load_state(r))?;
        r.section("bus", |r| self.cpu.bus.as_mut().unwrap().load_state(r))?;
        r.section("ram", |r| {
            let ram = r.bytes()?;
            if ram.len() as u64 != self.cpu.mem.size() {
                return Err("snapshot RAM size mismatch".into());
            }
            self.cpu.mem.write_bytes(0, ram);
            Ok(())
        })?;
        self.powered_off = r.bool()?;
        for d in &mut self.virtio {
            d.resume();
        }
        Ok(())
    }
    pub fn step(&mut self, budget: u32) -> Result<u64, MachineError> {
        if self.powered_off {
            return Ok(0);
        }
        if budget == 0 || budget > 2_000_000 {
            return Err(MachineError::InvalidConfig);
        }
        if !self
            .console
            .input
            .lock()
            .map_err(|_| MachineError::InvalidState)?
            .is_empty()
        {
            self.uart.input_available()
        }
        self.cpu.bus.as_mut().unwrap().tick(self.cpu.time);
        let r = self.cpu.run_slice(budget as u64);
        if self.cpu.signals.is_shutdown() {
            self.powered_off = true;
            self.cpu
                .bus
                .as_mut()
                .unwrap()
                .halt()
                .map_err(|_| MachineError::Device)?;
        }
        r.map_err(|_| MachineError::InvalidState)
    }
}
fn device_tree(cfg: &MachineConfig, ram: u64, initrd: u64) -> Vec<u8> {
    let mut f = Fdt::new();
    f.begin_node("");
    f.prop_str("model", "WASMNux RV64");
    f.prop_str("compatible", "riscv-virtio");
    f.prop_u32("#address-cells", 2);
    f.prop_u32("#size-cells", 2);
    f.begin_node("cpus");
    f.prop_u32("#address-cells", 1);
    f.prop_u32("#size-cells", 0);
    f.prop_u32("timebase-frequency", 100_000_000);
    f.begin_node("cpu@0");
    f.prop_str("device_type", "cpu");
    f.prop_u32("reg", 0);
    f.prop_str("status", "okay");
    f.prop_str("compatible", "riscv");
    f.prop_str("riscv,isa", "rv64imafdc_zicsr_zifencei_sstc");
    f.prop_str("riscv,isa-base", "rv64i");
    f.prop_strs(
        "riscv,isa-extensions",
        &["i", "m", "a", "f", "d", "c", "zicsr", "zifencei", "sstc"],
    );
    f.prop_str("mmu-type", "riscv,sv39");
    f.begin_node("interrupt-controller");
    f.prop_u32("#interrupt-cells", 1);
    f.prop_empty("interrupt-controller");
    f.prop_str("compatible", "riscv,cpu-intc");
    f.prop_u32("phandle", 1);
    f.end_node();
    f.end_node();
    f.end_node();
    f.begin_node_num("memory", RAM_BASE);
    f.prop_str("device_type", "memory");
    f.prop_u64_pair("reg", RAM_BASE, ram);
    f.end_node();
    f.begin_node("soc");
    f.prop_str("compatible", "simple-bus");
    f.prop_u32("#address-cells", 2);
    f.prop_u32("#size-cells", 2);
    f.prop_empty("ranges");
    f.begin_node_num("clint", 0x2000000);
    f.prop_str("compatible", "riscv,clint0");
    f.prop_u64_pair("reg", 0x2000000, 0x10000);
    f.prop_u32s("interrupts-extended", &[1, 3, 1, 7]);
    f.end_node();
    f.begin_node_num("plic", 0xc000000);
    f.prop_str("compatible", "sifive,plic-1.0.0");
    f.prop_u64_pair("reg", 0xc000000, 0x400000);
    f.prop_u32("#interrupt-cells", 1);
    f.prop_empty("interrupt-controller");
    f.prop_u32("riscv,ndev", 63);
    f.prop_u32("phandle", 2);
    f.prop_u32s("interrupts-extended", &[1, 11, 1, 9]);
    f.end_node();
    f.begin_node_num("uart", 0x10000000);
    f.prop_str("compatible", "ns16550a");
    f.prop_u64_pair("reg", 0x10000000, 256);
    f.prop_u32("clock-frequency", 24_000_000);
    f.prop_u32("reg-shift", 0);
    f.prop_u32("reg-io-width", 1);
    f.prop_u32s("interrupts-extended", &[2, 10]);
    f.end_node();
    f.begin_node_num("syscon", 0x10000100);
    f.prop_strs("compatible", &["syscon", "simple-mfd"]);
    f.prop_u64_pair("reg", 0x10000100, 256);
    f.prop_u32("phandle", 3);
    f.begin_node("poweroff");
    f.prop_str("compatible", "syscon-poweroff");
    f.prop_u32("regmap", 3);
    f.prop_u32("offset", 0);
    f.prop_u32("value", 0x706f6666);
    f.end_node();
    f.begin_node("reboot");
    f.prop_str("compatible", "syscon-reboot");
    f.prop_u32("regmap", 3);
    f.prop_u32("offset", 4);
    f.prop_u32("value", 0x72656274);
    f.end_node();
    f.end_node();
    f.begin_node_num("rtc", 0x10100000);
    f.prop_str("compatible", "google,goldfish-rtc");
    f.prop_u64_pair("reg", 0x10100000, 4096);
    f.prop_u32s("interrupts-extended", &[2, 12]);
    f.end_node();
    for (i, (addr, irq)) in [(0x10008000, 1), (0x10009000, 2)].into_iter().enumerate() {
        f.begin_node_num(&format!("virtio{}", i), addr);
        f.prop_str("compatible", "virtio,mmio");
        f.prop_u64_pair("reg", addr, 4096);
        f.prop_u32s("interrupts-extended", &[2, irq]);
        f.end_node();
    }
    f.end_node();
    f.begin_node("reserved-memory");
    f.prop_u32("#address-cells", 2);
    f.prop_u32("#size-cells", 2);
    f.prop_empty("ranges");
    f.begin_node_num("framebuffer", FB_BASE);
    f.prop_u64_pair("reg", FB_BASE, FB_SIZE as u64);
    f.prop_empty("no-map");
    f.end_node();
    f.end_node();
    f.begin_node_num("framebuffer", FB_BASE);
    f.prop_str("compatible", "simple-framebuffer");
    f.prop_u64_pair("reg", FB_BASE, FB_SIZE as u64);
    f.prop_u32("width", FB_WIDTH);
    f.prop_u32("height", FB_HEIGHT);
    f.prop_u32("stride", FB_WIDTH * 4);
    f.prop_str("format", "a8r8g8b8");
    f.end_node();
    f.begin_node("chosen");
    f.prop_str("bootargs", &cfg.cmdline);
    f.prop_str("stdout-path", "/soc/uart@10000000:115200n8");
    if initrd > 0 {
        f.prop_u64("linux,initrd-start", 0x88000000);
        f.prop_u64("linux,initrd-end", 0x88000000 + initrd);
    }
    f.end_node();
    f.end_node();
    f.output()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mmu::Mmio;
    use crate::snapshot::{StateReader, StateWriter};
    #[derive(Default)]
    struct TestDisk {
        data: Vec<u8>,
        fail: bool,
        flushes: usize,
    }
    impl BlockDevice for TestDisk {
        fn read_at(&mut self, o: u64, b: &mut [u8]) -> Result<(), String> {
            if self.fail {
                return Err("injected read failure".into());
            }
            let o = o as usize;
            b.copy_from_slice(&self.data[o..o + b.len()]);
            Ok(())
        }
        fn write_at(&mut self, o: u64, b: &[u8]) -> Result<(), String> {
            if self.fail {
                return Err("injected write failure".into());
            }
            let o = o as usize;
            self.data[o..o + b.len()].copy_from_slice(b);
            Ok(())
        }
        fn flush(&mut self) -> Result<(), String> {
            if self.fail {
                return Err("injected flush failure".into());
            }
            self.flushes += 1;
            Ok(())
        }
    }
    struct TestEntropy;
    impl Entropy for TestEntropy {
        fn fill(&mut self, b: &mut [u8]) -> Result<(), String> {
            b.fill(0x42);
            Ok(())
        }
    }
    fn machine(disk: Arc<Mutex<TestDisk>>) -> Machine {
        // Minimal architectural test image, not a kernel or a substitute boot claim.
        let cfg = MachineConfig {
            ram_bytes: 64 * 1024 * 1024,
            disk_sectors: 8,
            cmdline: "console=ttyS0".into(),
        };
        Machine::new(
            &cfg,
            &[0x93, 0, 0, 0],
            &[0x13, 0, 0, 0],
            &[],
            disk,
            Arc::new(Mutex::new(TestEntropy)),
        )
        .unwrap()
    }
    fn disk() -> Arc<Mutex<TestDisk>> {
        Arc::new(Mutex::new(TestDisk {
            data: vec![0; 4096],
            ..Default::default()
        }))
    }
    #[test]
    fn cpu_executes_real_rv64_instructions() {
        let mut m = machine(disk());
        m.cpu.x[1] = 19;
        m.cpu.x[2] = 23;
        // addi x1,x0,19; addi x2,x0,23; add x3,x1,x2; ebreak
        let code: [u32; 4] = [0x01300093, 0x01700113, 0x002081b3, 0x00100073];
        for (i, w) in code.iter().enumerate() {
            m.cpu.mem.write_u32(i as u64 * 4, *w)
        }
        assert_eq!(m.step(3).unwrap(), 3);
        assert_eq!(m.cpu.x[3], 42);
        assert_eq!(m.cpu.pc, 0x8000000c);
    }
    #[test]
    fn execution_budget_and_shutdown() {
        let mut m = machine(disk());
        assert!(m.step(0).is_err());
        assert!(m.step(2_000_001).is_err());
        m.cpu.shutdown();
        assert_eq!(m.step(1).unwrap(), 0);
        assert!(m.powered_off);
    }
    #[test]
    fn console_is_bounded_and_roundtrips() {
        let c = Console::default();
        c.input(b"hello").unwrap();
        assert_eq!(c.receive(), (b'h', true));
        assert_eq!(c.receive(), (b'e', true));
        assert!(c.input(&vec![0; 65536]).is_err());
        for _ in 0..1024 * 1024 + 1 {
            c.send(b'x')
        }
        assert!(*c.overflow.lock().unwrap());
        assert_eq!(c.drain().len(), 1024 * 1024);
    }
    #[test]
    fn invalid_images_rejected() {
        let cfg = MachineConfig {
            ram_bytes: 4096,
            disk_sectors: 8,
            cmdline: String::new(),
        };
        assert!(Machine::new(
            &cfg,
            &[],
            &[],
            &[],
            disk(),
            Arc::new(Mutex::new(TestEntropy))
        )
        .is_err());
    }
    fn submit(m: &mut Machine, kind: u32, sector: u64, payload: &[u8]) -> u8 {
        let mem = m.cpu.mem.clone();
        let base = 0x80100000u64;
        let header = base + 512;
        let data = base + 1024;
        let status_addr = base + 2048;
        let o = mem.offset(base);
        mem.write_bytes(o, &[0; 4096]);
        mem.write_u64(o, header);
        mem.write_u32(o + 8, 16);
        mem.write_u32(mem.offset(header), kind);
        mem.write_u64(mem.offset(header) + 8, sector);
        mem.write_u16(o + 12, 1); // header: NEXT
        mem.write_u16(o + 14, 1);
        if kind != 4 {
            mem.write_bytes(mem.offset(data), payload);
            mem.write_u64(o + 16, data);
            mem.write_u32(o + 24, payload.len() as u32);
            mem.write_u32(o + 28, if kind == 0 { 3 } else { 1 });
            mem.write_u16(o + 30, 2);
        }
        let status = if kind == 4 { 16 } else { 32 };
        mem.write_u64(o + status, status_addr);
        mem.write_u32(o + status + 8, 1);
        mem.write_u16(o + status + 12, 2);
        // Configure the legacy transport directly; queue processing is the real vendored transport.
        {
            let mut d = m.virtio[1].dev.lock().unwrap();
            let st = &mut d.state;
            st.driver_features[0] = (1 << 2) | (1 << 6) | (1 << 9);
            st.status = 15;
            let q = &mut st.queues[0];
            q.num = 8;
            q.ready = 1;
            q.desc_phys = base;
            q.avail_phys = base + 128;
            q.used_phys = base + 192;
            q.last_avail_idx = 0;
            st.guest.write_u16(base + 130, 1).unwrap();
            d.process_queue(0);
        }
        mem.read_u8(mem.offset(status_addr))
    }
    #[test]
    fn block_write_read_and_flush() {
        let d = disk();
        let mut m = machine(d.clone());
        let mut b = vec![0; 512];
        b[7] = 99;
        assert_eq!(submit(&mut m, 1, 2, &b), 0);
        assert_eq!(d.lock().unwrap().data[1031], 99);
        assert_eq!(submit(&mut m, 4, 0, &[]), 0);
        assert_eq!(d.lock().unwrap().flushes, 1);
        assert_eq!(submit(&mut m, 0, 2, &b), 0);
        assert_eq!(m.cpu.mem.read_u8(m.cpu.mem.offset(0x80100400) + 7), 99);
    }
    #[test]
    fn block_errors_are_not_acknowledged_as_success() {
        let d = disk();
        let mut m = machine(d.clone());
        d.lock().unwrap().fail = true;
        assert_eq!(submit(&mut m, 1, 0, &[1; 512]), 1);
        assert_eq!(submit(&mut m, 4, 0, &[]), 1);
        assert_eq!(submit(&mut m, 0, 0, &[0; 512]), 1);
        assert_eq!(d.lock().unwrap().flushes, 0);
    }
    #[test]
    fn block_bounds_rejected() {
        let mut m = machine(disk());
        assert_eq!(submit(&mut m, 1, 8, &[1; 512]), 1);
        assert_eq!(submit(&mut m, 0, 8, &[0; 512]), 1);
    }
    #[test]
    fn timer_and_uart_mmio() {
        let mut m = machine(disk());
        let mut bus = Bus::default();
        bus.add(Clint::new(m.cpu.signals.clone(), 0x2000000, 0x10000));
        bus.store(0, 0x2004000, 8, 100).unwrap();
        assert_eq!(bus.load(101, 0x200bff8, 8).unwrap(), 101);
        bus.tick(101);
        assert_ne!(m.cpu.signals.mip() & crate::isa::INT_MTIP, 0);
        m.console.input(b"A").unwrap();
        m.uart.input_available();
        assert_eq!(m.uart.load(0, 0x10000000, 1).unwrap(), 65);
    }
    #[test]
    fn snapshot_roundtrips_cpu_and_ram() {
        let cfg = MachineConfig {
            ram_bytes: 64 * 1024 * 1024,
            disk_sectors: 8,
            cmdline: "console=ttyS0".into(),
        };
        let disk = Arc::new(Mutex::new(TestDisk::default()));
        let mut m = Machine::new(
            &cfg,
            &[0; 4],
            &[0; 4],
            &[],
            disk,
            Arc::new(Mutex::new(TestEntropy)),
        )
        .unwrap();
        m.cpu.x[5] = 0x1234;
        m.cpu.pc = 0x80001000;
        m.cpu.mem.write_u64(0x1000, 0xabcd);
        let snap = m.save_state(&cfg).unwrap();
        m.cpu.x[5] = 0;
        m.cpu.mem.write_u64(0x1000, 0);
        m.load_state(&cfg, &snap).unwrap();
        assert_eq!(m.cpu.x[5], 0x1234);
        assert_eq!(m.cpu.mem.read_u64(0x1000), 0xabcd);
        let mut bad = snap.clone();
        bad[0] ^= 1;
        assert!(m.load_state(&cfg, &bad).is_err());
        assert!(m.load_state(&cfg, &snap[..snap.len() - 1]).is_err());
    }
    #[test]
    fn snapshot_rejects_unknown_boolean() {
        let mut w = StateWriter::new();
        w.u8(2);
        let bytes = w.into_bytes();
        let mut r = StateReader::new(&bytes);
        assert!(r.bool().is_err());
    }
}
